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
//!
//! The module is partitioned by *what a reviewer needs to see together*:
//!
//! - `tokens.rs` - the lexical layer: split on unquoted separators, then into
//!   quote-aware tokens. No path context, no policy.
//! - `paths.rs` - **every containment decision** in one file: what counts as a
//!   path, the redirection target, and `path_ok`'s lexical-plus-canonical
//!   answer. That is the security-review argument for this split, not the
//!   line count.
//! - `grammar.rs` - per-tool grammar (program text in a positional or a flag's
//!   value, wrapper indirection) and the reviewable-alternative advice.
//! - here - the decision: [`workspace_scoped`] and its per-segment
//!   classification, with the deny tables they consult.
//!
//! Split for gate 11 (the 1,200-line ceiling). Every public path resolves as
//! it did before (`lca_permissions::shell::workspace_scoped`,
//! `…::reviewable_alternative`); a probe compiles those names against the
//! rlib. Nothing here widened: the helpers a sibling now calls are `pub(super)`,
//! which is exactly the reach they had as private items in one file.

use std::path::Path;

mod grammar;
mod paths;
mod tokens;

use grammar::{
    denied_program_behind_indirection, multi_call_applet, positional_program_text,
    program_valued_flag, sed_script_executes,
};
use paths::{canonical, looks_like_path, path_ok, redirection_target};
use tokens::{segments, tokens};

/// The reviewable form of an invocation refused for carrying a program in a
/// flag's value (gh #458), or `None` when it is not one of those refusals.
///
/// Re-exported here so it is reachable as `lca_permissions::shell::…` - the
/// refusal copy lives in the interface crates, and ADR-0039's rule is that the
/// copy tells the user what to DO. A row asserts the advice is not itself a
/// bypass.
pub use grammar::reviewable_alternative;

/// Executable extensions stripped from a program name before matching
/// (gh #347 fix-up). Windows spellings carry one; a file genuinely named
/// `x.bat` in the workspace is reached through `path_ok`, not this table.
const EXECUTABLE_SUFFIXES: &[&str] = &[".exe", ".cmd", ".bat", ".com", ".ps1"];

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

/// Inline-evaluation/module flags on language runtimes (gh #347). The code
/// they carry is not in the workspace, so nothing about it can be reasoned
/// about; a match sends the whole segment to review. Script-file
/// invocations (`python3 scripts/run.py`) stay scoped - the folder's own
/// code is trusted by the folder-trust contract.
const INTERPRETER_EVAL_FLAGS: &[(&str, &[&str])] = &[
    ("python", &["-c", "-m"]),
    ("python3", &["-c", "-m"]),
    ("pypy", &["-c", "-m"]),
    ("pypy3", &["-c", "-m"]),
    ("node", &["-e", "--eval", "-p", "--print"]),
    ("nodejs", &["-e", "--eval", "-p", "--print"]),
    ("bun", &["-e", "--eval", "-p", "--print"]),
    ("deno", &["-e", "--eval", "-p", "--print"]),
    ("perl", &["-e", "-E"]),
    ("ruby", &["-e"]),
    ("php", &["-r"]),
    ("lua", &["-e"]),
    ("pwsh", &["-c", "-command", "-encodedcommand"]),
    ("powershell", &["-c", "-command", "-encodedcommand"]),
];

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
    if interpreter_inline_eval(&name, &toks[i + 1..]) {
        return false;
    }
    if denied_program_behind_indirection(&name, &toks[i + 1..]) {
        return false;
    }
    if let Some(applet_args) = multi_call_applet(&name, &toks[i + 1..])
        && positional_program_text(&applet_args.0, &applet_args.1)
    {
        return false;
    }
    if positional_program_text(&name, &toks[i + 1..]) {
        return false;
    }
    if name == "sed" && sed_script_executes(&toks[i + 1..]) {
        return false;
    }
    // GH #458: a flag whose *value* is a program (or a shell string) is the
    // gh #347 class reached another way - `vim -c ':!id'`, `tar
    // --to-command=id`, and four sibling tar flags the enumeration found.
    if program_valued_flag(&name, &toks[i + 1..]) {
        return false;
    }
    if name == "git" {
        // GH #345: the subcommand can sit behind any number of global flags
        // (`git -C . push`, `git --no-pager push`). Classify past them; an
        // invocation we cannot classify is refused, never assumed benign.
        match git_subcommand(&toks[i + 1..]) {
            Some(sub) if GIT_EGRESS.contains(&sub) || sub == "config" => return false,
            None => return false,
            Some(_) => {}
        }
    }
    if name == "find"
        && toks
            .iter()
            .any(|t| matches!(t.as_str(), "-exec" | "-execdir" | "-ok" | "-delete"))
    {
        return false;
    }
    if name == "cd" {
        // The `cd <project> && …` idiom is everywhere, so allow a `cd` whose
        // target resolves inside the workspace. Later relative paths resolve
        // against the original cwd, which is conservative: it can over-review,
        // never over-allow (a path that would be inside only relative to the
        // new, deeper cwd is refused).
        return match toks.get(i + 1) {
            Some(target) if !target.starts_with('-') => path_ok(target, cwd, workspace),
            _ => false,
        };
    }
    if name == "pushd" || name == "popd" {
        return false;
    }
    if name == "rm" && toks.iter().any(|t| t == "--no-preserve-root") {
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

/// `git` global flags whose value is the NEXT token when written detached
/// (`-C <dir>`, `-c <kv>`). The `=`-attached form (`--git-dir=x`) consumes
/// only itself and is handled separately.
const GIT_FLAG_TAKES_ARG: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
];

/// `git` global flags that make the invocation unclassifiable, so the caller
/// refuses: `-c`/`--config-env` reconfigure git (transport helpers, hooks,
/// aliases - `git -c alias.p='!cmd' p` runs `cmd`), and `--exec-path`/
/// `--super-prefix` redirect where git finds its subcommands (arbitrary
/// execution).
const GIT_FLAG_REFUSE: &[&str] = &["-c", "--config-env", "--exec-path", "--super-prefix"];

/// The subcommand of a `git` invocation, skipping global flags and their
/// option-arguments. `args` is the tokens after `git`.
///
/// `None` means "cannot classify" and the caller refuses: a config override,
/// an exec-path redirect, a dangling flag-argument, or no subcommand at all.
fn git_subcommand(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let token = args[i].as_str();
        // An `=`-attached flag consumes only itself.
        if let Some((base, _)) = token.split_once('=') {
            if GIT_FLAG_REFUSE.contains(&base) {
                return None;
            }
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            if GIT_FLAG_REFUSE.contains(&token) {
                return None;
            }
            if GIT_FLAG_TAKES_ARG.contains(&token) {
                // The next token is this flag's argument; a dangling one is
                // refused (the loop's bound check returns `None`).
                i += 2;
                continue;
            }
            // `--` ends option parsing; the next token is the subcommand.
            if token == "--" {
                return args.get(i + 1).map(String::as_str);
            }
            i += 1;
            continue;
        }
        return Some(token);
    }
    None
}

/// Whether a language runtime invocation carries an inline-eval/module flag
/// (gh #347): the code is not in the workspace. `args` is the tokens after
/// the program. A flag may follow other flags, so every token is scanned;
/// `=`-attached forms (`--eval=code`) match on their base.
fn interpreter_inline_eval(program: &str, args: &[String]) -> bool {
    let Some((_, flags)) = INTERPRETER_EVAL_FLAGS
        .iter()
        .find(|(name, _)| interpreter_name_matches(program, name))
    else {
        return false;
    };
    args.iter()
        .any(|token| flags.iter().any(|flag| eval_flag_matches(token, flag)))
}

/// Whether a program name is `name` or `name` with a version suffix
/// (gh #347 fix-up): `python3.12`, `python2.7`, `pypy3.10`, `python3.13t`
/// all match their base runtime, but `node_modules` or `python3x` do not.
/// The suffix class is tight on purpose - digits, dots, and one trailing
/// `t` (free-threaded CPython) - so a same-prefix non-interpreter can
/// never match.
fn interpreter_name_matches(program: &str, name: &str) -> bool {
    if program == name {
        return true;
    }
    let Some(rest) = program.strip_prefix(name) else {
        return false;
    };
    // A version suffix: digits and dots, optionally a trailing `t`
    // (free-threaded CPython). Anything else - `_modules`, `x`, `-config` -
    // is a different program.
    let body = rest.strip_suffix('t').unwrap_or(rest);
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Whether a token carries `flag` (gh #347 fix-up): exact, `=`-attached
/// (`--eval=x`), or - for a single-letter flag - the glued short form the
/// real idioms use (`-c'print(1)'` tokenizes to `-cprint(1)`, `-eprint 1`).
/// A long flag is never matched by prefix, and `--`-prefixed tokens never
/// take the glued path.
fn eval_flag_matches(token: &str, flag: &str) -> bool {
    if let Some((base, _)) = token.split_once('=') {
        return base.eq_ignore_ascii_case(flag);
    }
    if token.eq_ignore_ascii_case(flag) {
        return true;
    }
    // The glued short form: a one-letter flag (`-c`) followed by its
    // argument in the same token. Only single-dash, exactly-two-char flags.
    flag.len() == 2
        && flag.starts_with('-')
        && token.starts_with('-')            // not `--…`
        && !token.starts_with("--")
        && token
            .get(..2)
            .is_some_and(|head| head.eq_ignore_ascii_case(flag))
        && token.len() > 2
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
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    // Strip a known executable extension before matching (gh #347 fix-up):
    // on Windows every spelling that exists there carries one (`python.exe`,
    // `bash.exe`, `node.exe`), and the pre-fix code let them miss both the
    // eval table and `DENY_PROGRAMS`. Case-insensitive, like the rest of
    // the name matching; a name with no extension is returned unchanged.
    let stem = EXECUTABLE_SUFFIXES
        .iter()
        .find_map(|suffix| {
            name.len()
                .checked_sub(suffix.len())
                .filter(|_| name.to_ascii_lowercase().ends_with(suffix))
                .map(|cut| &name[..cut])
        })
        .unwrap_or(name);
    stem.to_ascii_lowercase()
}

// Gh #339 finding 1's uniqueness row lives in its own file: `DENY_PROGRAMS` is a
// 120-line table, so a row about the table is exactly what should not add to
// this file's length.
#[cfg(test)]
mod deny_tests;

// The behaviour rows live in `tests.rs`; a plain `mod tests;` finds it because
// `mod.rs` owns this directory, so every test path stays `shell::tests::<row>`
// (gh #557: a renamed security row would be worse than a longer file).
// Re-indenting the block is the only change it took.
#[cfg(test)]
mod tests;
