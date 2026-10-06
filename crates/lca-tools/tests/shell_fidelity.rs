//! The tool-call fidelity corpus (ADR-0041, R2): commands must reach the
//! child byte-exact and run in full.
//!
//! The owner's session log produced the two defects this file exists to
//! keep dead: `echo "double"` arrived at `cmd.exe` as `\"double\"`, and a
//! two-line command ran only its first line, silently. Both were transport
//! faults - the same text written through the `write` tool ran fine - so
//! the assertions here go through the *real* executor and read back what
//! the shell actually received, not what a plan says it should have.
//!
//! Structure: a corpus of rows, each with the markers that prove every part
//! ran and (where the row cares) the exact bytes of a file the command
//! writes. `POSIX` rows run on Unix and on Windows Git Bash; `CMD` and
//! `POWERSHELL` rows run on the Windows ladders. The Windows half is
//! `#[cfg(windows)]`, so `windows-latest` is where it is proven.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lca_tools::shell::Shell;
use lca_tools::{CancelFlag, ExecOutcome, NativeOps, ToolOps};

/// Which shells a row's command is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `sh`/`bash` grammar.
    Posix,
    /// `cmd.exe` grammar.
    Cmd,
    /// PowerShell 7 and 5.1 grammar.
    PowerShell,
}

/// What the row's command writes to `lca-fid.bin`, if it writes one.
#[derive(Debug, Clone, Copy)]
enum FileExpect {
    /// The file's bytes are exactly this.
    Exact(&'static [u8]),
    /// The file's bytes contain this (encoding-tolerant rows).
    Contains(&'static [u8]),
}

/// One corpus row.
struct Row {
    name: &'static str,
    family: Family,
    command: &'static str,
    /// Substrings that must appear in the output: each stands for one part
    /// of the command that ran.
    markers: &'static [&'static str],
    /// Substrings that must not appear: the mangled forms.
    forbid: &'static [&'static str],
    /// What `lca-fid.bin` should hold afterwards.
    file: Option<FileExpect>,
}

const FID: &str = "lca-fid.bin";

/// The corpus. Every `markers` entry is a token the command prints; a
/// transport that drops a line or eats a quote loses one.
fn corpus() -> Vec<Row> {
    vec![
        Row {
            name: "multi-line runs every line",
            family: Family::Posix,
            command: "echo one\necho two\necho three",
            markers: &["one", "two", "three"],
            forbid: &[],
            file: None,
        },
        Row {
            name: "double quotes reach the shell",
            family: Family::Posix,
            command: "printf '%s' '\"double\"' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"\"double\"")),
        },
        Row {
            name: "single quotes reach the shell",
            family: Family::Posix,
            command: "printf '%s' \"'single'\" > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"'single'")),
        },
        Row {
            name: "real newlines survive",
            family: Family::Posix,
            command: "printf 'a\\nb\\nc\\n' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"a\nb\nc\n")),
        },
        Row {
            name: "trailing spaces survive",
            family: Family::Posix,
            command: "printf '%s' 'trail   ' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"trail   ")),
        },
        Row {
            name: "shell metacharacters survive",
            family: Family::Posix,
            command: "printf '%s' 'a & b | c < d > e ^ f ( g ) ; h' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"a & b | c < d > e ^ f ( g ) ; h")),
        },
        Row {
            name: "backslashes survive",
            family: Family::Posix,
            command: "printf '%s' 'C:\\Users\\me\\proj' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"C:\\Users\\me\\proj")),
        },
        Row {
            name: "unicode survives",
            family: Family::Posix,
            command: "printf '%s' 'héllo✓' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact("héllo✓".as_bytes())),
        },
        Row {
            name: "an ANSI snippet survives",
            family: Family::Posix,
            command: "printf '\\033[31mred\\033[0m' > lca-fid.bin",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"\x1b[31mred\x1b[0m")),
        },
        Row {
            name: "CRLF in the command still runs every line",
            family: Family::Posix,
            command: "echo one\r\necho two",
            markers: &["one", "two"],
            forbid: &[],
            file: None,
        },
        Row {
            name: "empty lines do not stop the run",
            family: Family::Posix,
            command: "echo a\n\n\necho b",
            markers: &["a", "b"],
            forbid: &[],
            file: None,
        },
        Row {
            name: "no escape-mangled quotes",
            family: Family::Posix,
            command: "echo \"double\"",
            markers: &["double"],
            forbid: &["\\\""],
            file: None,
        },
        // --- cmd.exe -----------------------------------------------------
        Row {
            name: "cmd: multi-line runs every line",
            family: Family::Cmd,
            command: "echo one\r\necho two\r\necho three",
            markers: &["one", "two", "three"],
            forbid: &[],
            file: None,
        },
        Row {
            name: "cmd: quoted echo is not backslash-escaped",
            family: Family::Cmd,
            command: ">lca-fid.bin echo \"double\"",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"\"double\"\r\n")),
        },
        Row {
            name: "cmd: single quotes stay single quotes",
            family: Family::Cmd,
            command: ">lca-fid.bin echo 'single'",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"'single'\r\n")),
        },
        Row {
            name: "cmd: & separates commands",
            family: Family::Cmd,
            command: ">lca-fid.bin echo a & echo b",
            markers: &["a", "b"],
            forbid: &[],
            file: Some(FileExpect::Contains(b"a")),
        },
        Row {
            name: "cmd: %VAR% is left for cmd to expand",
            family: Family::Cmd,
            command: ">lca-fid.bin echo %USERPROFILE%",
            markers: &[],
            forbid: &["%USERPROFILE%"],
            file: None,
        },
        Row {
            name: "cmd: no escape-mangled quotes",
            family: Family::Cmd,
            command: "echo \"double\"",
            markers: &["double"],
            forbid: &["\\\""],
            file: None,
        },
        // --- PowerShell --------------------------------------------------
        Row {
            name: "powershell: multi-line runs every line",
            family: Family::PowerShell,
            command: "'one'\r\n'two'\r\n'three'",
            markers: &["one", "two", "three"],
            forbid: &[],
            file: None,
        },
        Row {
            name: "powershell: quotes survive",
            family: Family::PowerShell,
            command: "Set-Content -NoNewline -Path lca-fid.bin -Value '\"double\"'",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"\"double\"")),
        },
        Row {
            name: "powershell: metacharacters survive",
            family: Family::PowerShell,
            command: "Set-Content -NoNewline -Path lca-fid.bin -Value 'a & b | c < d > e ^ f ( g ) ; h'",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Exact(b"a & b | c < d > e ^ f ( g ) ; h")),
        },
        Row {
            name: "powershell: unicode survives",
            family: Family::PowerShell,
            command: "Set-Content -NoNewline -Encoding utf8 -Path lca-fid.bin -Value 'héllo✓'",
            markers: &[],
            forbid: &[],
            file: Some(FileExpect::Contains("héllo✓".as_bytes())),
        },
        Row {
            name: "powershell: $env: is left for PowerShell to expand",
            family: Family::PowerShell,
            command: "Write-Output \"home=$env:USERPROFILE\"",
            markers: &["home="],
            forbid: &[],
            file: None,
        },
    ]
}

/// Run one command through the real executor.
#[allow(clippy::expect_used)] // a test helper: a failure to spawn or drive the executor is the failure this file exists to report.
fn run(shell: &Shell, cwd: &Path, command: &str) -> (ExecOutcome, Vec<u8>) {
    let ops = NativeOps::new(shell.clone());
    let mut chunks: Vec<u8> = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    // The exec timeout is a hang guard, not the requirement - the markers
    // and bytes below are. A loaded Windows runner starved the pwsh
    // multi-line row past 30s once (CI 36891621022, 2026-10-01); 60s
    // absorbs the load without loosening what the row asserts.
    runtime
        .block_on(ops.exec(
            command,
            cwd,
            Some(Duration::from_secs(60)),
            &mut |chunk: &[u8]| chunks.extend_from_slice(chunk),
            CancelFlag::new(),
        ))
        .expect("exec")
}

/// A scratch directory with a fresh `lca-fid.bin` per row.
#[allow(clippy::expect_used)] // test fixture; a scratch dir that cannot be made is a failed test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-fidelity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Assert one row through one shell.
fn check(shell: &Shell, row: &Row, label: &str) {
    let dir = scratch(label);
    let file = dir.join(FID);
    let (outcome, output) = run(shell, &dir, row.command);
    let text = String::from_utf8_lossy(&output).into_owned();
    assert!(
        matches!(outcome, ExecOutcome::Exit { code: 0 }),
        "{} / {}: {:?}\n{text}",
        label,
        row.name,
        outcome
    );
    for marker in row.markers {
        assert!(
            text.contains(marker),
            "{} / {}: marker {marker:?} missing from output\n{text}",
            label,
            row.name
        );
    }
    for forbidden in row.forbid {
        assert!(
            !text.contains(forbidden),
            "{} / {}: mangled form {forbidden:?} present in output\n{text}",
            label,
            row.name
        );
    }
    if let Some(expect) = row.file {
        assert!(
            file.is_file(),
            "{} / {}: the command wrote no {FID}\n{text}",
            label,
            row.name
        );
        let bytes = std::fs::read(&file).unwrap_or_default();
        match expect {
            FileExpect::Exact(want) => assert_eq!(
                bytes, want,
                "{} / {}: file bytes differ\n{text}",
                label, row.name
            ),
            FileExpect::Contains(want) => assert!(
                bytes.windows(want.len()).any(|window| window == want),
                "{} / {}: file does not contain {want:?}: {bytes:?}",
                label,
                row.name
            ),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run every corpus row of `family` through `shell`. Platform-independent on
/// purpose: the Windows leg calls it too, and a `#[cfg(unix)]` here broke
/// the Windows build invisibly from a Linux dev machine.
fn run_family(shell: &Shell, family: Family, label: &str) {
    for row in corpus() {
        if row.family == family {
            check(shell, &row, label);
        }
    }
}

// Verifies: FR-TOOL-9 - the POSIX transport carries the command byte-exact
// and every line of a multi-line command runs.
#[cfg(unix)]
#[test]
fn the_posix_corpus_runs_byte_exact_through_sh() {
    use lca_tools::shell::Kind;
    let shell = Shell {
        program: "sh".to_string(),
        kind: Kind::Sh,
        explicit: false,
        transport: lca_tools::Transport::Argv,
    };
    run_family(&shell, Family::Posix, "sh");
}

#[cfg(unix)]
#[test]
fn the_posix_corpus_runs_byte_exact_through_bash_when_present() {
    use lca_tools::shell::Kind;
    if !Path::new("/bin/bash").is_file() {
        eprintln!("skip: no /bin/bash on this host");
        return;
    }
    let shell = Shell {
        program: "/bin/bash".to_string(),
        kind: Kind::Bash,
        explicit: true,
        transport: lca_tools::Transport::Argv,
    };
    run_family(&shell, Family::Posix, "bash");
}

// Verifies: FR-TOOL-9 - on Windows every command travels in a script file,
// and all four interpreters receive it unaltered (quote, newline, and
// metacharacter rows included). Runs on windows-latest CI.
#[cfg(windows)]
#[test]
fn the_corpus_runs_byte_exact_on_every_windows_shell() {
    use lca_tools::shell::{Real, resolve_with};
    // The ladder resolves each kind where it exists; a shell the runner
    // lacks is a named skip, and windows-latest has all four.
    let mut ran = 0;
    for (tool, family) in [
        ("bash", Family::Posix),
        ("pwsh", Family::PowerShell),
        ("powershell", Family::PowerShell),
        ("cmd", Family::Cmd),
    ] {
        match resolve_with(&Real, tool, None) {
            Ok(shell) => {
                run_family(&shell, family, tool);
                ran += 1;
            }
            Err(err) => eprintln!("skip: {tool} not available on this runner: {err}"),
        }
    }
    assert!(ran > 0, "no shell was available to run the corpus");
}
