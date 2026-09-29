//! The shell command analyzer: decide whether a command provably stays inside
//! the workspace, so a trusted folder can auto-approve it.
//!
//! Strict by design (permission-UX plan §3.3, ADR-0039): a command that is
//! not positively workspace-scoped is sent to the user for review. A false
//! "review" costs one prompt; a false "allow" costs the trust boundary, so
//! the analyzer refuses anything it cannot reason about — command
//! substitution, `cd`, shell indirection, privilege and disk tools, egress
//! clients, and any path or redirection that leaves the workspace.
//!
//! Trusting a folder means trusting the code in it: build tools may run
//! build scripts and proc macros. The analyzer polices the *filesystem
//! boundary* and a small high-risk set, not "can this run code".

use std::path::{Component, Path, PathBuf};

/// Programs that are never auto-approved under folder trust. They either
/// escalate privilege, can destroy the machine without a path argument, or
/// hide an arbitrary command behind themselves.
const DENY_PROGRAMS: &[&str] = &[
    // privilege
    "sudo",
    "su",
    "doas",
    "pkexec", // shells and indirection (the command is
    // hidden from this analyzer)
    "sh",
    "bash",
    "zsh",
    "dash",
    "ash",
    "ksh",
    "csh",
    "tcsh",
    "fish",
    "env",
    "xargs",
    "eval",
    "exec",
    "nohup",
    "setsid",
    "command",
    "builtin",
    "source", // system / disk
    "dd",
    "mkfs",
    "mkfs.ext4",
    "mkfs.xfs",
    "mkfs.btrfs",
    "mount",
    "umount",
    "chown",
    "chgrp",
    "chmod",
    "chattr",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "systemctl",
    "service",
    "init",
    "kill",
    "killall",
    "pkill",
    "iptables",
    "ip6tables",
    "nft",
    "ufw",
    "insmod",
    "rmmod",
    "modprobe",
    "sysctl", // network / egress
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "telnet",
    "ftp",
    "tftp",
    "socat",
];

/// A `git` subcommand that reaches the network or moves history off-machine.
const GIT_EGRESS: &[&str] = &[
    "push",
    "fetch",
    "pull",
    "clone",
    "remote",
    "submodule",
    "svn",
    "request-pull",
    "send-email",
];

/// Environment variables that change what a later program loads or executes.
const SENSITIVE_VARS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "NODE_OPTIONS",
    "PERL5LIB",
    "RUBYLIB",
    "RUSTC_WRAPPER",
    "RUSTC",
    "BASH_ENV",
    "ENV",
    "SHELL",
    "IFS",
];

/// Paths that are safe redirection/read targets anywhere.
const DEV_SAFE: &[&str] = &["/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr"];

/// Whether `command`, run with `cwd`, provably stays inside `workspace`.
pub fn workspace_scoped(command: &str, cwd: &Path, workspace: &Path) -> bool {
    if command.trim().is_empty() {
        return false;
    }
    // Command substitution, process substitution, and variable expansion can
    // all hide a path or a program from this analyzer.
    if command.contains('`')
        || command.contains("$(")
        || command.contains("${")
        || command.contains("<(")
        || command.contains(">(")
        || command.contains('$')
    {
        return false;
    }
    let workspace = canonical(workspace);
    let cwd = canonical(cwd);
    let mut any = false;
    for segment in segments(command) {
        if segment.trim().is_empty() {
            continue;
        }
        any = true;
        if !segment_ok(&segment, &cwd, &workspace) {
            return false;
        }
    }
    any
}

/// One segment (a list/pipeline element) is workspace-scoped.
fn segment_ok(segment: &str, cwd: &Path, workspace: &Path) -> bool {
    let toks = tokens(segment);
    if toks.is_empty() {
        return true;
    }
    // Skip leading `VAR=value` assignments; refuse a loader/env variable.
    let mut i = 0;
    while i < toks.len() {
        if !is_assignment(&toks[i]) {
            break;
        }
        let name = toks[i].split_once('=').map(|(name, _)| name).unwrap_or("");
        if SENSITIVE_VARS.iter().any(|v| v.eq_ignore_ascii_case(name)) {
            return false;
        }
        i += 1;
    }
    let Some(program) = toks.get(i) else {
        return false; // assignments only: nothing proven
    };
    if program.contains('/') && !path_ok(program, cwd, workspace) {
        return false;
    }
    let name = basename(program);
    if DENY_PROGRAMS.contains(&name.as_str()) {
        return false;
    }
    if name == "git"
        && let Some(sub) = toks.get(i + 1)
        && GIT_EGRESS.contains(&sub.as_str())
    {
        return false;
    }
    if name == "find"
        && toks
            .iter()
            .any(|t| matches!(t.as_str(), "-exec" | "-execdir" | "-ok" | "-delete"))
    {
        return false;
    }
    if name == "cd" || name == "pushd" || name == "popd" {
        return false;
    }
    if name == "rm" && toks.iter().any(|t| t == "--no-preserve-root") {
        return false;
    }
    if name == "git" && toks.get(i + 1).is_some_and(|s| s == "config") {
        return false;
    }
    if is_package_mutation(&name, &toks[i + 1..]) {
        return false;
    }
    // Every redirection target and path-like token must stay inside.
    let mut j = i + 1;
    while j < toks.len() {
        let token = &toks[j];
        if redirection_target(token, toks.get(j + 1)).is_some() {
            let target = redirection_target(token, toks.get(j + 1)).unwrap_or("");
            if !path_ok(target, cwd, workspace) {
                return false;
            }
            // An attached target (`>file`) consumes one token; a split one two.
            if token.contains(['>', '<'])
                && token.trim_start_matches(|c: char| c.is_ascii_digit()) != ">"
                && token.trim_start_matches(|c: char| c.is_ascii_digit()) != ">>"
                && token.trim_start_matches(|c: char| c.is_ascii_digit()) != "<"
                && token.trim_start_matches(|c: char| c.is_ascii_digit()) != "<<"
                && token != ">"
                && token != ">>"
                && token != "<"
            {
                j += 1;
            } else {
                j += 2;
            }
            continue;
        }
        // `--flag=/outside/path` carries its path after `=`.
        let candidate = token.rsplit_once('=').map(|(_, v)| v).unwrap_or(token);
        if looks_like_path(candidate) && !path_ok(candidate, cwd, workspace) {
            return false;
        }
        j += 1;
    }
    true
}

/// A package tool doing a publish or a global install (network/global state).
fn is_package_mutation(program: &str, args: &[String]) -> bool {
    let sub = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("");
    let joined: Vec<&str> = args.iter().map(String::as_str).collect();
    match program {
        "cargo" => matches!(sub, "install" | "publish" | "login"),
        "npm" | "pnpm" | "yarn" | "bun" => {
            matches!(sub, "install" | "i" | "add" | "publish" | "exec" | "dlx")
                || joined.contains(&"-g")
                || joined.contains(&"--global")
        }
        "pip" | "pip3" => sub == "install",
        "go" => matches!(sub, "install" | "get"),
        "apt" | "apt-get" | "dnf" | "yum" | "pacman" | "zypper" | "apk" | "brew" | "snap"
        | "nix" => true,
        _ => false,
    }
}

fn is_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn basename(program: &str) -> String {
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase()
}

/// If `token` is a redirection operator, its target: attached (`>file`) or the
/// following token (`> file`).
fn redirection_target<'a>(token: &'a str, next: Option<&'a String>) -> Option<&'a str> {
    let stripped = token.trim_start_matches(|c: char| c.is_ascii_digit());
    let op = stripped
        .strip_prefix("&>>")
        .or_else(|| stripped.strip_prefix("&>"))
        .or_else(|| stripped.strip_prefix(">>"))
        .or_else(|| stripped.strip_prefix(">|"))
        .or_else(|| stripped.strip_prefix(">>"))
        .or_else(|| stripped.strip_prefix("<<"))
        .or_else(|| stripped.strip_prefix(">&"))
        .or_else(|| stripped.strip_prefix("<&"))
        .or_else(|| stripped.strip_prefix(">"))
        .or_else(|| stripped.strip_prefix("<"))?;
    if stripped.starts_with("<<") {
        // A heredoc: the body is untracked, refuse it elsewhere by returning a
        // target that fails path validation.
        return Some("<<");
    }
    if op.is_empty() {
        next.map(String::as_str)
    } else {
        Some(op)
    }
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token.contains('/')
        || token.starts_with('~')
        || token == "."
        || token == ".."
        || token.contains("..")
        || (cfg!(windows) && (token.contains('\\') || token.as_bytes().get(1) == Some(&b':')))
}

/// Whether a path token resolves inside `workspace` (base `cwd`).
fn path_ok(token: &str, cwd: &Path, workspace: &Path) -> bool {
    let token = token.trim_matches(['"', '\'']);
    if token.is_empty() {
        return false;
    }
    // A leftover redirection operator (an untracked heredoc body, `>&`, …).
    if token.starts_with('<') || token.starts_with('>') {
        return false;
    }
    if DEV_SAFE.contains(&token) {
        return true;
    }
    if token.contains("://") {
        return false; // a URL, not a path
    }
    if token.starts_with('~') {
        return false; // the home directory is outside the workspace
    }
    let raw = Path::new(token);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };
    let normalized = lexical_normalize(&joined);
    normalized == workspace || normalized.starts_with(workspace)
}

/// Normalize `.`/`..` lexically (a non-existent path cannot be canonicalized).
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> PathBuf {
        PathBuf::from("/work/proj")
    }

    fn ok(command: &str) -> bool {
        workspace_scoped(command, &ws(), &ws())
    }

    // Verifies: FR-PERM-20 (the workspace-scoped allow table)
    #[test]
    fn plain_build_and_dev_commands_are_scoped() {
        for command in [
            "cargo build",
            "cargo test --release",
            "cargo clippy --all-targets -- -D warnings",
            "cargo fmt --check",
            "ls -la",
            "cat src/main.rs",
            "grep -rn foo src/",
            "rg 'pattern' crates/",
            "sed -i 's/a/b/' src/main.rs",
            "mkdir -p target/debug",
            "rm -rf node_modules",
            "rm -f ./tmp.txt",
            "mv src/a.rs src/b.rs",
            "cp -r assets/ dist/",
            "python3 scripts/run.py",
            "node index.js",
            "make",
            "git status",
            "git diff --stat",
            "git add src/main.rs",
            "git commit -m 'x'",
            "printf 'quit\\n' | ./target/debug/chess",
            "wc -l src/*.rs",
            "RUST_LOG=debug cargo run",
        ] {
            assert!(ok(command), "should be scoped: {command}");
        }
    }

    #[test]
    fn outside_paths_always_review() {
        for command in [
            "cat /etc/passwd",
            "rm -rf /",
            "rm -rf /tmp/x",
            "ls ~",
            "cat ~/.ssh/id_rsa",
            "cp secret ../other/",
            "mv a /etc/b",
            "echo hi > /tmp/x",
            "echo hi >> ../outside",
            "cat < /etc/shadow",
            "ls /dev/sda",
            "python /opt/tool.py",
            "sed -i 's/a/b/' /etc/hosts",
        ] {
            assert!(!ok(command), "should review: {command}");
        }
    }

    #[test]
    fn indirection_and_substitution_always_review() {
        for command in [
            "sh -c 'rm -rf /'",
            "bash script.sh",
            "env rm -rf /",
            "xargs rm",
            "eval 'ls'",
            "echo $(cat /etc/passwd)",
            "echo `id`",
            "cat <(curl x)",
            "FOO=$(id) ls",
            "FOO=$HOME ls",
        ] {
            assert!(!ok(command), "should review: {command}");
        }
    }

    #[test]
    fn privilege_disk_and_egress_always_review() {
        for command in [
            "sudo rm",
            "su",
            "dd if=/dev/zero of=x",
            "chmod 777 src",
            "chown root x",
            "systemctl restart x",
            "kill -9 1",
            "curl https://example.com",
            "wget https://example.com",
            "ssh host",
            "scp a host:/b",
            "rsync -a . host:/x",
            "git push",
            "git fetch origin",
            "git clone https://x",
            "cargo install ripgrep",
            "cargo publish",
            "npm install",
            "npm i -g foo",
            "pip install requests",
            "apt-get install x",
            "go install x",
        ] {
            assert!(!ok(command), "should review: {command}");
        }
    }

    #[test]
    fn sensitive_env_and_cd_and_find_exec_review() {
        for command in [
            "PATH=/evil ls",
            "LD_PRELOAD=/evil/x ls",
            "cd src && rm -rf /",
            "cd /tmp",
            "find . -exec rm {} ;",
            "find . -delete",
            "ls --file=/etc/passwd",
            "ls --file=/etc/passwd extra",
        ] {
            assert!(!ok(command), "should review: {command}");
        }
    }

    #[test]
    fn dev_null_and_in_repo_absolute_paths_are_fine() {
        assert!(ok("cargo build > /dev/null 2>&1"));
        assert!(ok("cargo test 2>&1"));
        assert!(ok("cargo build 2>/dev/null"));
        assert!(!ok("cat <<EOF"));
        assert!(!ok("cat <<-'EOF'
hi
EOF"));
        assert!(ok("ls /work/proj/src"));
        assert!(ok("rm -f /work/proj/target/x"));
        // A path that only looks inside because of `..` lexical folding is fine.
        assert!(ok("ls src/../src"));
        // `..` that escapes is not.
        assert!(!ok("ls ../../etc"));
    }

    #[test]
    fn empty_and_assignments_only_review() {
        assert!(!ok(""));
        assert!(!ok("   "));
        assert!(!ok("FOO=bar"));
    }

    // The tokenizer must not be fooled by separators inside quotes.
    #[test]
    fn separators_inside_quotes_do_not_split() {
        let toks = tokens(r#"echo 'a ; b | c' "d e""#);
        assert_eq!(toks, vec!["echo", "a ; b | c", "d e"]);
    }

    #[test]
    fn segments_split_on_shell_separators() {
        assert_eq!(segments("a && b | c ; d"), vec!["a ", " b ", " c ", " d"]);
        assert!(segments("   ").is_empty());
    }
}

// ---------------------------------------------------------------------------
// A minimal, quote-aware shell scanner
// ---------------------------------------------------------------------------

/// Split on unquoted `;`, `&`, `|`, and newlines.
fn segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut state = Quote::None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            Quote::None => match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push('\\');
                        current.push(next);
                    }
                }
                '\'' => {
                    state = Quote::Single;
                    current.push(c);
                }
                '"' => {
                    state = Quote::Double;
                    current.push(c);
                }
                ';' | '&' | '|' | '\n' => {
                    let segment = std::mem::take(&mut current);
                    if !segment.trim().is_empty() {
                        out.push(segment);
                    }
                }
                _ => current.push(c),
            },
            Quote::Single => {
                current.push(c);
                if c == '\'' {
                    state = Quote::None;
                }
            }
            Quote::Double => {
                current.push(c);
                if c == '"' {
                    state = Quote::None;
                } else if c == '\\'
                    && let Some(next) = chars.next()
                {
                    current.push(next);
                }
            }
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

#[derive(Clone, Copy)]
enum Quote {
    None,
    Single,
    Double,
}

/// Split one segment into tokens: unquoted whitespace separates; quotes group
/// and are stripped; redirection operators stay attached to their target when
/// written without a space.
fn tokens(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut state = Quote::None;
    let mut chars = segment.chars().peekable();
    let flush = |current: &mut String, out: &mut Vec<String>| {
        if !current.is_empty() {
            out.push(std::mem::take(current));
        }
    };
    while let Some(c) = chars.next() {
        match state {
            Quote::None => match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '\'' => state = Quote::Single,
                '"' => state = Quote::Double,
                c if c.is_whitespace() => flush(&mut current, &mut out),
                _ => current.push(c),
            },
            Quote::Single => {
                if c == '\'' {
                    state = Quote::None;
                } else {
                    current.push(c);
                }
            }
            Quote::Double => {
                if c == '"' {
                    state = Quote::None;
                } else if c == '\\'
                    && let Some(next) = chars.next()
                {
                    current.push(next);
                } else {
                    current.push(c);
                }
            }
        }
    }
    flush(&mut current, &mut out);
    out
}
