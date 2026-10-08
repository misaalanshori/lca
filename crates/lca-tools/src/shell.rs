//! Shell selection: which interpreter the `shell` tool runs, and how a
//! command reaches it without being mangled on the way.
//!
//! Ported from pi's `getShellConfig`
//! (`packages/coding-agent/src/utils/shell.ts`), adapted to LCA's config
//! keys and to a Windows whose `bash` on `PATH` is usually the WSL stub.
//! The owner's session log shows both traps this module exists to avoid:
//! `where bash` resolving to `C:\Windows\System32\bash.exe`, and a command
//! arriving at `cmd.exe` with its quotes backslash-escaped and its newlines
//! dropped.
//!
//! Resolution order (ADR-0041):
//! 1. an explicit interpreter path (`shell.path`) - anything, including the
//!    WSL stub, because it was asked for by name;
//! 2. the configured tool (`shell.tool`, default `auto`);
//! 3. on Windows, auto walks: Git Bash by known location, then `pwsh.exe`,
//!    then `powershell.exe`, then `cmd.exe`;
//! 4. on Unix, `sh` (the pre-existing behavior, unchanged).
//!
//! The resolution runs against a [`Probe`], so the Windows ladder is
//! exercisable on any host - the tests drive it with a fake filesystem and
//! environment.

use std::fmt;

/// The interpreter families the ladder knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Git Bash, MSYS2, Cygwin, or the WSL stub (only when named exactly).
    Bash,
    /// PowerShell 7.
    Pwsh,
    /// Windows PowerShell 5.1.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
    /// POSIX `sh`.
    Sh,
}

impl Kind {
    /// The name the config uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Bash => "bash",
            Kind::Pwsh => "pwsh",
            Kind::PowerShell => "powershell",
            Kind::Cmd => "cmd",
            Kind::Sh => "sh",
        }
    }

    /// The model-facing note on the dialect: what separators, variables,
    /// and paths mean here. The owner's log shows an agent burning ten
    /// tool calls working this out by probing; the tool description says
    /// it instead.
    pub fn dialect(self) -> &'static str {
        match self {
            Kind::Bash => {
                "POSIX shell: `;` and `&&` separate commands, `$VAR` expands \
                 variables, `/`-separated paths, forward-slash paths under \
                 the Git installation map drive letters (C: is /c)."
            }
            Kind::Pwsh | Kind::PowerShell => {
                "PowerShell: `;` separates commands, `$env:VAR` reads \
                 environment variables, `\\`-separated paths, pipes and \
                 redirects follow PowerShell's grammar."
            }
            Kind::Cmd => {
                "cmd.exe: `&` and `&&` separate commands, `%VAR%` expands \
                 variables, `\\`-separated paths, `copy`/`del`/`dir` are the \
                 native tools."
            }
            Kind::Sh => {
                "POSIX shell: `;` and `&&` separate commands, `$VAR` expands \
                 variables, `/`-separated paths."
            }
        }
    }

    /// Whether the dialect speaks POSIX shell.
    pub fn is_posix(self) -> bool {
        matches!(self, Kind::Bash | Kind::Sh)
    }

    /// The script file extension this dialect runs.
    pub fn script_extension(self) -> &'static str {
        match self {
            Kind::Bash | Kind::Sh => "sh",
            Kind::Pwsh | Kind::PowerShell => "ps1",
            Kind::Cmd => "cmd",
        }
    }
}

/// How a command string reaches the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// One argv element after `-c` (the POSIX path, byte-faithful because
    /// `execve` takes the string as-is).
    Argv,
    /// Written to a temporary script file and executed by path. Windows
    /// needs this: a command string crosses `CreateProcess` quoting into a
    /// shell that re-parses its command line, and both `\"` escapes and raw
    /// newlines are lost there.
    ScriptFile,
}

/// The resolved interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// The program to spawn (an absolute path when the ladder resolved one).
    pub program: String,
    /// Which dialect it speaks.
    pub kind: Kind,
    /// Whether the user named this shell (`shell.path` or `shell.tool`)
    /// rather than the ladder picking it.
    pub explicit: bool,
    /// How the command reaches it.
    pub transport: Transport,
    /// A prefix prepended to every command (`shell.command_prefix`,
    /// gh #133); `None` runs commands as written.
    pub command_prefix: Option<String>,
}

impl Shell {
    /// A shell picked without probing: the ladder's last resort for the
    /// current platform. `NativeOps::default` uses this so a host without a
    /// usable bash still runs (`cmd.exe`/`sh` always exist).
    pub fn fallback(os: Os) -> Shell {
        match os {
            Os::Windows => Shell {
                program: "cmd.exe".to_string(),
                kind: Kind::Cmd,
                explicit: false,
                transport: Transport::ScriptFile,
                command_prefix: None,
            },
            Os::Unix => Shell {
                program: "sh".to_string(),
                kind: Kind::Sh,
                explicit: false,
                transport: Transport::Argv,
                command_prefix: None,
            },
        }
    }

    /// The one-line dialect note for the tool description.
    pub fn describe(&self) -> String {
        format!(
            "Runtime is {} (`{}`). Commands are written to a temporary script \
             file and run from it, so quotes, newlines, and special characters \
             reach the shell exactly as written. {}",
            self.kind.as_str(),
            self.program,
            self.kind.dialect()
        )
    }

    /// The command as the child sees it: the configured prefix joined
    /// with a newline first (gh #133, pi's `${commandPrefix}\n${command}`
    /// shape, so `export`/`source` lines take effect for the command),
    /// else the command untouched.
    pub fn command_text(&self, command: &str) -> String {
        match &self.command_prefix {
            Some(prefix) => format!("{prefix}\n{command}"),
            None => command.to_string(),
        }
    }

    /// The script file's extension for this dialect.
    pub fn script_extension(&self) -> &'static str {
        self.kind.script_extension()
    }

    /// The argv, after the program, that runs `script` as a file.
    pub fn script_args(&self, script: &str) -> Vec<String> {
        match self.kind {
            Kind::Bash | Kind::Sh => vec![script.to_string()],
            Kind::Pwsh | Kind::PowerShell => vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.to_string(),
            ],
            Kind::Cmd => vec!["/D".into(), "/C".into(), "call".into(), script.to_string()],
        }
    }

    /// The exact bytes the script file gets. `cmd.exe` wants CRLF line
    /// endings; the others take the command verbatim.
    pub fn script_text(&self, command: &str) -> String {
        match self.kind {
            Kind::Cmd => {
                let normalized = command.replace("\r\n", "\n").replace('\n', "\r\n");
                if normalized.ends_with("\r\n") {
                    normalized
                } else {
                    format!("{normalized}\r\n")
                }
            }
            _ => command.to_string(),
        }
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.kind.as_str(), self.program)
    }
}

/// Which platform the ladder is resolving for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// Linux, macOS, and every other POSIX target.
    Unix,
}

/// The environment and filesystem facts the ladder needs, so the Windows
/// ladder can be tested anywhere.
pub trait Probe {
    /// The host platform.
    fn os(&self) -> Os;
    /// One environment variable.
    fn env(&self, key: &str) -> Option<String>;
    /// Whether a path names an existing file.
    fn is_file(&self, path: &str) -> bool;
    /// Every `PATH` match for an executable name, in `PATH` order.
    fn which(&self, exe: &str) -> Vec<String>;
}

/// The real environment: `std::env`, `std::fs`, and a `PATH` walk (no
/// subprocess, so this works identically on every platform).
pub struct Real;

impl Probe for Real {
    fn os(&self) -> Os {
        if cfg!(windows) { Os::Windows } else { Os::Unix }
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    }

    fn is_file(&self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }

    fn which(&self, exe: &str) -> Vec<String> {
        let Some(path) = self.env("PATH") else {
            return Vec::new();
        };
        let sep = if self.os() == Os::Windows { ';' } else { ':' };
        path.split(sep)
            .filter(|entry| !entry.is_empty())
            .map(|dir| {
                if dir.ends_with('/') || dir.ends_with('\\') {
                    format!("{dir}{exe}")
                } else if self.os() == Os::Windows {
                    format!("{dir}\\{exe}")
                } else {
                    format!("{dir}/{exe}")
                }
            })
            .filter(|candidate| self.is_file(candidate))
            .collect()
    }
}

/// Whether a resolved `bash.exe` is the legacy WSL stub. Auto mode skips
/// it: it runs inside a different filesystem namespace (`/mnt/c/...`), so
/// silently switching to it changes what every path in the command means.
pub fn is_wsl_stub(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    normalized.ends_with("\\windows\\system32\\bash.exe")
        || normalized.ends_with("\\windows\\sysnative\\bash.exe")
}

/// Resolve the interpreter against the real environment.
pub fn resolve(tool: &str, explicit_path: Option<&str>) -> Result<Shell, String> {
    resolve_with(&Real, tool, explicit_path)
}

/// Resolve the interpreter against a probe. `tool` is `shell.tool`
/// (`auto`, `bash`, `pwsh`, `powershell`, `cmd`).
pub fn resolve_with(
    probe: &dyn Probe,
    tool: &str,
    explicit_path: Option<&str>,
) -> Result<Shell, String> {
    // 1. An exact interpreter the user named. It is used as-is, WSL stub or
    //    not: naming a path is an explicit statement of intent.
    if let Some(path) = explicit_path.filter(|p| !p.trim().is_empty()) {
        if !probe.is_file(path) {
            return Err(format!(
                "shell.path does not name a file: {path}. Check the path, or unset it to let \
                 the ladder choose."
            ));
        }
        let lower = path.to_lowercase();
        let kind = if lower.ends_with("bash.exe") || lower.ends_with("bash") {
            Kind::Bash
        } else if lower.ends_with("pwsh.exe") || lower.ends_with("pwsh") {
            Kind::Pwsh
        } else if lower.ends_with("powershell.exe") {
            Kind::PowerShell
        } else if lower.ends_with("cmd.exe") {
            Kind::Cmd
        } else {
            Kind::Sh
        };
        return Ok(build(probe.os(), path.to_string(), kind, true));
    }

    let tool = tool.trim().to_lowercase();
    let tool = if tool.is_empty() {
        "auto".to_string()
    } else {
        tool
    };

    if probe.os() == Os::Unix {
        return resolve_unix(probe, &tool);
    }
    resolve_windows(probe, &tool)
}

fn resolve_unix(probe: &dyn Probe, tool: &str) -> Result<Shell, String> {
    match tool {
        "auto" | "sh" => Ok(build(Os::Unix, "sh".to_string(), Kind::Sh, false)),
        "bash" => {
            if probe.is_file("/bin/bash") {
                return Ok(build(Os::Unix, "/bin/bash".to_string(), Kind::Bash, true));
            }
            if let Some(found) = probe.which("bash").into_iter().next() {
                return Ok(build(Os::Unix, found, Kind::Bash, true));
            }
            Err("shell.tool = bash, but no bash was found at /bin/bash or on PATH.".to_string())
        }
        "pwsh" => match probe.which("pwsh").into_iter().next() {
            Some(found) => Ok(build(Os::Unix, found, Kind::Pwsh, true)),
            None => Err(
                "shell.tool = pwsh, but no pwsh was found on PATH. Install PowerShell, or set \
                 shell.tool = auto."
                    .to_string(),
            ),
        },
        "powershell" | "cmd" => Err(format!(
            "shell.tool = {tool} is a Windows shell; on this platform use auto, bash, or pwsh."
        )),
        other => Err(format!(
            "unknown shell.tool value: {other}. Use auto, bash, pwsh, powershell, or cmd."
        )),
    }
}

fn resolve_windows(probe: &dyn Probe, tool: &str) -> Result<Shell, String> {
    let mut searched: Vec<String> = Vec::new();

    let mut bash_candidates = Vec::new();
    let mut bash_locations: Vec<String> = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        match probe.env(key) {
            Some(base) => bash_locations.push(format!("{base}\\Git\\bin\\bash.exe")),
            None => bash_locations.push(format!("%{key}%\\Git\\bin\\bash.exe")),
        }
    }
    match probe.env("LOCALAPPDATA") {
        Some(base) => bash_locations.push(format!("{base}\\Programs\\Git\\bin\\bash.exe")),
        None => bash_locations.push("%LOCALAPPDATA%\\Programs\\Git\\bin\\bash.exe".to_string()),
    }
    bash_candidates.extend(bash_locations.iter().cloned());
    // Auto mode's PATH fallback for bash, minus the WSL stub; an explicit
    // `bash` choice takes the same list for the same reason.
    let path_bash = probe
        .which("bash.exe")
        .into_iter()
        .find(|candidate| !is_wsl_stub(candidate));

    let pwsh_candidates = {
        let mut list = probe.which("pwsh.exe");
        if let Some(base) = probe.env("ProgramFiles") {
            list.push(format!("{base}\\PowerShell\\7\\pwsh.exe"));
        }
        list
    };
    let system_root = probe
        .env("SystemRoot")
        .unwrap_or_else(|| "C:\\Windows".to_string());
    let powershell_candidates = vec![format!(
        "{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    )];
    let cmd_candidates = {
        let mut list = Vec::new();
        if let Some(comspec) = probe.env("ComSpec") {
            list.push(comspec);
        }
        list.push(format!("{system_root}\\System32\\cmd.exe"));
        list
    };

    let auto = match tool {
        "auto" => true,
        "bash" | "pwsh" | "powershell" | "cmd" => false,
        other => {
            return Err(format!(
                "unknown shell.tool value: {other}. Use auto, bash, pwsh, powershell, or cmd."
            ));
        }
    };
    let wants = |kind: &str| auto || tool == kind;

    if wants("bash") {
        for candidate in &bash_locations {
            if !searched.contains(candidate) {
                searched.push(candidate.clone());
            }
        }
        for candidate in bash_candidates.iter().chain(path_bash.iter()) {
            searched.push(candidate.clone());
            if probe.is_file(candidate) {
                return Ok(build(Os::Windows, candidate.clone(), Kind::Bash, !auto));
            }
        }
    }
    if wants("pwsh") {
        for candidate in &pwsh_candidates {
            searched.push(candidate.clone());
            if probe.is_file(candidate) {
                return Ok(build(Os::Windows, candidate.clone(), Kind::Pwsh, !auto));
            }
        }
    }
    if wants("powershell") {
        for candidate in &powershell_candidates {
            searched.push(candidate.clone());
            if probe.is_file(candidate) {
                return Ok(build(
                    Os::Windows,
                    candidate.clone(),
                    Kind::PowerShell,
                    !auto,
                ));
            }
        }
    }
    if wants("cmd") {
        for candidate in &cmd_candidates {
            searched.push(candidate.clone());
            if probe.is_file(candidate) {
                return Ok(build(Os::Windows, candidate.clone(), Kind::Cmd, !auto));
            }
        }
    }

    let wsl_note = if path_bash.is_none() && !probe.which("bash.exe").is_empty() {
        "\nNote: the only bash on PATH is the WSL stub (System32\\bash.exe); auto mode skips it \
         because it runs in a different filesystem namespace. Set shell.path to use it on \
         purpose."
    } else {
        ""
    };
    Err(format!(
        "no {tool} shell found. Searched:\n{}{wsl_note}\nInstall Git for Windows, PowerShell, or \
         set shell.path to an interpreter.",
        searched
            .iter()
            .map(|path| format!("  {path}\n"))
            .collect::<String>()
    ))
}

fn build(_os: Os, program: String, kind: Kind, explicit: bool) -> Shell {
    let transport = if _os == Os::Windows {
        Transport::ScriptFile
    } else {
        Transport::Argv
    };
    Shell {
        program,
        kind,
        explicit,
        transport,
        command_prefix: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A Windows filesystem and environment the ladder can be driven
    /// against, which is the point: the ladder's traps are Windows-only and
    /// this suite runs anywhere.
    struct Fake {
        os: Os,
        env: HashMap<String, String>,
        files: Vec<String>,
        path: Vec<String>,
    }

    impl Fake {
        fn windows() -> Fake {
            Fake {
                os: Os::Windows,
                env: HashMap::new(),
                files: Vec::new(),
                path: Vec::new(),
            }
        }

        fn env(mut self, key: &str, value: &str) -> Fake {
            self.env.insert(key.to_string(), value.to_string());
            self
        }

        fn file(mut self, path: &str) -> Fake {
            self.files.push(path.to_string());
            self
        }

        fn path_entry(mut self, dir: &str) -> Fake {
            self.path.push(dir.to_string());
            self
        }
    }

    impl Probe for Fake {
        fn os(&self) -> Os {
            self.os
        }
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).cloned()
        }
        fn is_file(&self, path: &str) -> bool {
            self.files.iter().any(|f| f.eq_ignore_ascii_case(path))
        }
        fn which(&self, exe: &str) -> Vec<String> {
            self.path
                .iter()
                .map(|dir| format!("{dir}\\{exe}"))
                .filter(|candidate| self.is_file(candidate))
                .collect()
        }
    }

    // Verifies: FR-TOOL-8 - the ladder, with the WSL-stub trap as the case
    // that matters. The owner's trap: `where bash` finds the WSL stub
    // first; auto mode must skip it and keep looking.
    #[test]
    fn auto_skips_the_wsl_stub_and_finds_git_bash_by_known_location() {
        let probe = Fake::windows()
            .env("SystemRoot", "C:\\Windows")
            .env("ProgramFiles", "C:\\Program Files")
            .path_entry("C:\\Windows\\System32")
            .file("C:\\Windows\\System32\\bash.exe")
            .file("C:\\Program Files\\Git\\bin\\bash.exe");
        let shell = resolve_with(&probe, "auto", None).expect("resolved");
        assert_eq!(shell.kind, Kind::Bash);
        assert_eq!(shell.program, "C:\\Program Files\\Git\\bin\\bash.exe");
        assert!(!shell.explicit, "the ladder picked it");
        assert_eq!(shell.transport, Transport::ScriptFile);
    }

    #[test]
    fn auto_skips_the_wsl_stub_on_path_and_falls_through_to_pwsh() {
        let probe = Fake::windows()
            .env("SystemRoot", "C:\\Windows")
            .path_entry("C:\\Windows\\System32")
            .file("C:\\Windows\\System32\\bash.exe")
            .path_entry("C:\\Program Files\\PowerShell\\7")
            .file("C:\\Program Files\\PowerShell\\7\\pwsh.exe");
        let shell = resolve_with(&probe, "auto", None).expect("resolved");
        assert_eq!(shell.kind, Kind::Pwsh);
        assert_eq!(shell.program, "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
    }

    #[test]
    fn the_windows_ladder_is_bash_pwsh_powershell_cmd_in_order() {
        let base = || {
            Fake::windows()
                .env("SystemRoot", "C:\\Windows")
                .path_entry("C:\\Windows\\System32")
                .file("C:\\Windows\\System32\\cmd.exe")
                .file("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
                .path_entry("C:\\tools")
                .file("C:\\tools\\pwsh.exe")
        };
        assert_eq!(
            resolve_with(&base(), "auto", None).unwrap().kind,
            Kind::Pwsh
        );
        assert_eq!(
            resolve_with(&base(), "powershell", None).unwrap().kind,
            Kind::PowerShell
        );
        assert_eq!(resolve_with(&base(), "cmd", None).unwrap().kind, Kind::Cmd);
        // With only cmd present, auto reaches it last.
        let only_cmd = Fake::windows()
            .env("SystemRoot", "C:\\Windows")
            .file("C:\\Windows\\System32\\cmd.exe");
        let shell = resolve_with(&only_cmd, "auto", None).expect("cmd");
        assert_eq!(shell.kind, Kind::Cmd);
        assert_eq!(shell.program, "C:\\Windows\\System32\\cmd.exe");
    }

    #[test]
    fn comspec_wins_for_cmd_and_missing_shells_name_what_was_searched() {
        let probe = Fake::windows()
            .env("SystemRoot", "C:\\Windows")
            .env("ComSpec", "D:\\tools\\cmd.exe")
            .file("D:\\tools\\cmd.exe");
        assert_eq!(
            resolve_with(&probe, "cmd", None).unwrap().program,
            "D:\\tools\\cmd.exe"
        );

        let empty = Fake::windows().env("SystemRoot", "C:\\Windows");
        let err = resolve_with(&empty, "auto", None).expect_err("nothing found");
        assert!(err.contains("Searched:"), "{err}");
        assert!(err.contains("Git\\bin\\bash.exe"), "{err}");
    }

    #[test]
    fn an_explicit_path_may_select_the_wsl_stub() {
        let probe = Fake::windows().file("C:\\Windows\\System32\\bash.exe");
        let shell = resolve_with(&probe, "auto", Some("C:\\Windows\\System32\\bash.exe"))
            .expect("explicit");
        assert_eq!(shell.kind, Kind::Bash);
        assert!(shell.explicit);

        let missing = resolve_with(&probe, "auto", Some("C:\\nope\\bash.exe"));
        assert!(
            missing
                .expect_err("missing")
                .contains("does not name a file")
        );
    }

    #[test]
    fn unix_auto_is_sh_and_bash_is_opt_in() {
        let probe = Fake {
            os: Os::Unix,
            env: HashMap::new(),
            files: vec!["/bin/bash".into()],
            path: vec![],
        };
        let auto = resolve_with(&probe, "auto", None).unwrap();
        assert_eq!(auto.kind, Kind::Sh);
        assert_eq!(auto.transport, Transport::Argv);
        let bash = resolve_with(&probe, "bash", None).unwrap();
        assert_eq!(bash.program, "/bin/bash");
        assert_eq!(bash.kind, Kind::Bash);
    }

    #[test]
    fn script_transport_shapes_match_the_shells() {
        let bash = build(Os::Windows, "C:\\git\\bash.exe".into(), Kind::Bash, false);
        assert_eq!(bash.script_args("C:\\t\\x.sh"), vec!["C:\\t\\x.sh"]);
        assert_eq!(bash.script_text("echo one\necho two"), "echo one\necho two");

        let pwsh = build(Os::Windows, "pwsh.exe".into(), Kind::Pwsh, false);
        assert_eq!(
            pwsh.script_args("C:\\t\\x.ps1"),
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "C:\\t\\x.ps1"
            ]
        );

        let cmd = build(Os::Windows, "cmd.exe".into(), Kind::Cmd, false);
        assert_eq!(
            cmd.script_args("C:\\t\\x.cmd"),
            vec!["/D", "/C", "call", "C:\\t\\x.cmd"]
        );
        assert_eq!(
            cmd.script_text("echo one\necho two"),
            "echo one\r\necho two\r\n"
        );
        assert_eq!(cmd.script_text("echo one\r\n"), "echo one\r\n");
    }

    // Verifies: gh #133 - a configured prefix joins the command
    // with a newline (pi's `${commandPrefix}\\n${command}` shape);
    // absent, the command passes through untouched.
    #[test]
    fn the_command_prefix_joins_with_a_newline() {
        let mut shell = build(Os::Unix, "/bin/bash".into(), Kind::Bash, false);
        assert_eq!(shell.command_text("echo hi"), "echo hi");
        shell.command_prefix = Some("export LCA_PROBE=1".to_string());
        assert_eq!(shell.command_text("echo hi"), "export LCA_PROBE=1\necho hi");
    }

    #[test]
    fn the_description_names_the_shell_and_its_dialect() {
        let shell = build(Os::Windows, "C:\\git\\bash.exe".into(), Kind::Bash, false);
        let text = shell.describe();
        assert!(
            text.contains("Runtime is bash (`C:\\git\\bash.exe`)"),
            "{text}"
        );
        assert!(text.contains("$VAR"), "{text}");
        let cmd = build(Os::Windows, "cmd.exe".into(), Kind::Cmd, false);
        assert!(cmd.describe().contains("%VAR%"), "{}", cmd.describe());
    }

    #[test]
    fn the_stub_detector_matches_only_the_stub() {
        assert!(is_wsl_stub("C:\\Windows\\System32\\bash.exe"));
        assert!(is_wsl_stub("c:/windows/system32/bash.exe"));
        assert!(!is_wsl_stub("C:\\Program Files\\Git\\bin\\bash.exe"));
        assert!(!is_wsl_stub("C:\\msys64\\usr\\bin\\bash.exe"));
    }
}

/// The transport's invariants for arbitrary command text, not only the
/// corpus rows.
#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Verifies: FR-TOOL-9 - on POSIX the script text is the command
        /// byte for byte, whatever the text holds (quotes, newlines,
        /// backslashes, unicode, control bytes).
        #[test]
        fn posix_script_text_is_the_command_verbatim(command in "\\PC*") {
            let shell = build(Os::Unix, "sh".into(), Kind::Sh, false);
            prop_assert_eq!(shell.script_text(&command), command);
        }

        /// Verifies: FR-TOOL-9 - the Windows transport keeps the command
        /// body out of argv: argv is a fixed shape ending in the script
        /// path, and a multi-line command never appears in it. This is the
        /// invariant the `CreateProcess`/`cmd` requoting bug violated.
        #[test]
        fn windows_transports_keep_the_body_out_of_argv(command in "\\PC*") {
            for kind in [Kind::Bash, Kind::Pwsh, Kind::PowerShell, Kind::Cmd] {
                let shell = build(Os::Windows, "shell.exe".into(), kind, false);
                let path = "C:\\tmp\\lca-cmd-1.sh";
                let args = shell.script_args(path);
                prop_assert_eq!(args.last().map(String::as_str), Some(path));
                prop_assert!(args.len() <= 6, "argv is a fixed shape: {args:?}");
                if command.contains('\n') {
                    prop_assert!(
                        !args.iter().any(|arg| arg.contains('\n')),
                        "a multi-line body never rides in argv: {args:?}"
                    );
                }
            }
        }

        /// Verifies: FR-TOOL-9 - cmd's script text differs from the command
        /// only by line-ending normalization and a final newline; the
        /// command's own bytes survive in order.
        #[test]
        fn cmd_script_text_only_normalizes_line_endings(command in "\\PC*") {
            let shell = build(Os::Windows, "cmd.exe".into(), Kind::Cmd, false);
            let text = shell.script_text(&command);
            prop_assert!(text.ends_with("\r\n"));
            let normalized = text.replace("\r\n", "\n");
            let expected = command.replace("\r\n", "\n");
            let expected = if expected.ends_with('\n') { expected } else { format!("{expected}\n") };
            prop_assert_eq!(normalized, expected);
        }
    }
}
