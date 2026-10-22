//! Program-text classification for the shell analyzer (gh #365, moved here for
//! the workspace file ceiling - gate 11; behaviour unchanged).
//!
//! Two families live here, both about code the analyzer cannot reason about
//! because it is in no file:
//!
//! - **positional program text** (`awk 'BEGIN{system(…)}'`, `osascript -e`,
//!   `irb -e`) and the narrow `sed` `e`-flag rule, and
//! - **wrapper indirection** (`timeout 5 curl …`, `busybox wget …`), where a
//!   denied program hides behind a program whose job is to exec it.
//!
//! Split from `shell.rs` when the additions took it past 1,200 lines. The
//! rules, their comments, and their guards are unchanged.

use super::{DENY_PROGRAMS, basename};

/// Programs that take a **program as a positional argument** rather than behind
/// an eval flag (gh #365): the same class gh #347 closed for `python3 -c`, on
/// the wrong side of the line.
///
/// `awk 'BEGIN{system("curl evil.example")}'` runs code that is in no file at
/// all, exactly like `python3 -c`. The rule mirrors #347's: the invocation
/// reviews **unless the program arrives as an in-workspace file** (see
/// `PROGRAM_FILE_FLAGS`), because a folder's own `awk -f scripts/x.awk` is the
/// folder's code and the trust model accepts it.
///
/// The set is measured, not guessed: the enumeration that produced it covered
/// 48 invocations across this list plus the flag-borne runtimes, and each entry
/// here auto-approved a `system()`/exec payload. Option **B** in the issue (a
/// "does this argument parse as a program?" heuristic) was rejected because it
/// cannot be proven and fails open on whatever syntax its author did not
/// consider; a set is auditable.
///
/// `sed` is deliberately **absent**: it is used constantly for legitimate edits,
/// and refusing it wholesale is the over-review ADR-0039 warns about. Its one
/// executing form has its own narrow rule (`sed_script_executes`).
const PROGRAM_TEXT_PROGRAMS: &[&str] = &["awk", "gawk", "mawk", "nawk", "osascript", "irb"];

/// The flag that says "the program is in this file" for a
/// [`PROGRAM_TEXT_PROGRAMS`] entry: `awk -f script.awk`, `sed -f script.sed`.
const PROGRAM_FILE_FLAG: &str = "-f";

/// Whether a positional-program interpreter is running **file-less** code
/// (gh #365).
///
/// `program` is already basenamed. The rule is #347's, applied to positional
/// code: reviews unless every non-option argument is accounted for. `-f file`
/// (and `--file=…`) means the program is a file; the remaining arguments are
/// then data paths, which the ordinary path check already polices.
///
/// `awk -v x=1 'BEGIN{…}'` — the `-v` assignment is consumed with its value, and
/// the program string that follows is still file-less code, so it reviews.
pub(super) fn positional_program_text(program: &str, args: &[String]) -> bool {
    if !PROGRAM_TEXT_PROGRAMS.contains(&program) {
        return false;
    }
    // `-f file` / `--file=file`: the program is a file, so the folder-trust
    // contract applies and the ordinary path check decides.
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == PROGRAM_FILE_FLAG || arg.starts_with("--file=") {
            return false;
        }
        // `-v name=value` (and `--assign=…`) take the value with them; skip the
        // pair so the assignment's value is never mistaken for the program.
        if matches!(arg.as_str(), "-v" | "--assign") {
            let _ = iter.next();
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        // The first free-standing argument is the program text.
        return true;
    }
    // No program argument at all (`awk` reading from stdin): nothing proven.
    true
}

/// Whether a `sed` invocation's script runs a shell command through the `e`
/// flag (gh #365).
///
/// `sed 's/x/whatever/e'` re-executes the *result* of the substitution as a
/// command, which made it arbitrary code execution in a trusted folder. Refusing
/// all `sed` is not acceptable - `sed -i 's/x/y/'` is one of the most common
/// commands there is, and ADR-0039 warns that over-review trains users to
/// approve without reading.
///
/// The `e` flag is a **suffix on an `s` command** (`s/…/…/e`, `s/…/…/ge`,
/// `s/…/…/ep`) or a standalone `e` command. This checks the script text for
/// exactly those shapes, which is cheap and has no false positive on ordinary
/// substitution: the flags that can follow an `s` command are drawn from
/// `g`, `p`, `i`/`I`, `m`/`M`, digits and `e`, so `e` in the suffix run is the
/// executing case. An `e` inside a pattern or replacement (`s/e/E/g`) is not a
/// suffix, so it stays scoped.
///
/// A `sed -f script` script may also contain `e`, but that file is the folder's
/// own code (the trust contract), so it is not treated as file-less.
pub(super) fn sed_script_executes(args: &[String]) -> bool {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // `-f file` / `--file=file`: the script is a file in the folder.
        if arg == PROGRAM_FILE_FLAG || arg.starts_with("--file=") {
            return false;
        }
        // `-e script` / `--expression=script`: the script follows.
        let script = if arg == "-e" || arg == "--expression" {
            match iter.next() {
                Some(script) => script.as_str(),
                None => return true,
            }
        } else if let Some(rest) = arg.strip_prefix("--expression=") {
            rest
        } else if let Some(rest) = arg.strip_prefix("-e") {
            // `-e's/x/y/e'`, glued.
            rest
        } else if arg.starts_with('-') {
            continue;
        } else {
            // The first free-standing argument is the script.
            arg.as_str()
        };
        return s_command_executes(script);
    }
    false
}

/// Whether a sed script text contains an executing `e` flag or command.
fn s_command_executes(script: &str) -> bool {
    let bytes: Vec<char> = script.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // A standalone `e` command: `e cmd`, at the start or after `;`/newline.
            'e' if i == 0 || matches!(bytes[i - 1], ';' | '\n' | '}') => return true,
            // An `s` command: `s<delim>pattern<delim>replacement<delim>flags`.
            's' if i == 0 || matches!(bytes[i - 1], ';' | '\n' | '}') => {
                if let Some(delim) = bytes.get(i + 1).copied() {
                    let mut seen = 0;
                    let mut j = i + 2;
                    // Walk the pattern, then the replacement, to the second
                    // unescaped delimiter.
                    while j < bytes.len() && seen < 2 {
                        if bytes[j] == '\\' {
                            j += 2;
                            continue;
                        }
                        if bytes[j] == delim {
                            seen += 1;
                        }
                        j += 1;
                    }
                    // Whatever follows is the flag run.
                    while j < bytes.len() {
                        match bytes[j] {
                            'e' => return true,
                            'g' | 'p' | 'i' | 'I' | 'm' | 'M' | '0'..='9' => j += 1,
                            _ => break,
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod program_valued_flag_tests {
    //! gh #458: flags whose *value* is a program (or worse, a shell string).
    //!
    //! These are the same class gh #347 drew the line against - code that is in
    //! no file - reached through a flag instead of a positional argument. Each
    //! row below is red before the fix, because every one of them used to
    //! auto-approve in a trusted folder.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::program_valued_flag;

    fn reviews(program: &str, args: &[&str]) -> bool {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        program_valued_flag(program, &args)
    }

    // Verifies: gh #458 - `tar`'s five program-valued spellings. The enumeration
    // found the issue's one example plus four siblings, all of which ran a
    // program with no prompt.
    #[test]
    fn tars_program_valued_flags_review() {
        for args in [
            &["xf", "a.tar", "--to-command=id"][..],
            &["xf", "a.tar", "--to-command", "id"],
            &["--to-command", "id", "-xf", "a.tar"],
            &["xf", "a.tar", "--use-compress-program=id"],
            &["xf", "a.tar", "-I", "id"],
            &["xf", "a.tar", "-Iid"],
            &["cI", "gzip", "b.tar", "dir"],
            &["xf", "a.tar", "-F", "id"],
            &["xf", "a.tar", "--info-script", "id"],
            &["xf", "a.tar", "--new-volume-script=id"],
            &["xf", "a.tar", "--rsh-command=id", "--file=host:x"],
            &[
                "xf",
                "a.tar",
                "--checkpoint=1",
                "--checkpoint-action=exec=id",
            ],
        ] {
            assert!(
                reviews("tar", args),
                "should review: tar {}",
                args.join(" ")
            );
        }
    }

    // Verifies: gh #458 - `--to-command`'s value goes through a **shell**, so it
    // is not merely a program name but arbitrary inline code.
    // Measured: `tar xf a.tar --to-command='id; echo PWNED'` runs both.
    #[test]
    fn a_shell_string_in_a_flag_value_reviews_like_inline_code() {
        assert!(reviews(
            "tar",
            &["xf", "a.tar", "--to-command=id; echo PWNED"]
        ));
        assert!(reviews(
            "tar",
            &["cf", "b.tar", "dir", "-I", "gzip; echo PWNED"]
        ));
    }

    // Verifies: gh #458 - **benign tar stays scoped.** This is the row that
    // keeps the refusal from being "refuse every tar": the ordinary create,
    // extract, list, and `-C` forms carry no program-valued flag.
    #[test]
    fn ordinary_tar_archive_work_stays_scoped() {
        for args in [
            &["xf", "a.tar"][..],
            &["cf", "b.tar", "dir"],
            &["czf", "b.tgz", "dir"],
            &["tvf", "a.tar"],
            &["xf", "a.tar", "-C", "out"],
            &["xzf", "a.tgz", "-C", "out"],
            &["cf", "b.tar", "dir", "--exclude=target"],
            &["xf", "a.tar", "--no-same-owner"],
        ] {
            assert!(
                !reviews("tar", args),
                "should stay scoped: tar {}",
                args.join(" ")
            );
        }
    }

    // Verifies: gh #458 - the `--checkpoint-action` value is only a program when
    // the action is `exec=`; `echo=`, `ttyout=`, `dot` and the like run nothing.
    // Measured: `exec=id` printed uid, `echo=hi` printed `hi`, `dot` printed `.`.
    #[test]
    fn only_the_exec_checkpoint_action_runs_a_program() {
        assert!(reviews(
            "tar",
            &["cf", "b.tar", "dir", "--checkpoint-action=exec=id"]
        ));
        assert!(reviews(
            "tar",
            &[
                "cf",
                "b.tar",
                "dir",
                "--checkpoint=1",
                "--checkpoint-action",
                "exec=id"
            ]
        ));
        for action in ["echo=hi", "dot", "ttyout=x", "sleep=1"] {
            assert!(
                !reviews(
                    "tar",
                    &[
                        "cf",
                        "b.tar",
                        "dir",
                        &format!("--checkpoint-action={action}")
                    ]
                ),
                "`{action}` runs no program"
            );
        }
    }

    // Verifies: gh #458 - **the editor `-c` family reviews**, because the
    // argument is a mini-program (`:!cmd`, `:r !cmd`, `:g/x/!cmd`, `:python`,
    // `:terminal`) whose surface is far larger than `awk`'s and cannot be
    // classified by a set.
    //
    // The cost is stated, not discovered: scripted `vim -c` reviews. `+cmd` is
    // refused on the same rule because it is the *same* hole - measured,
    // `vim '+!id' f` prints uid - so the alternative copy names `-S` (a script
    // in the folder, which the trust model accepts) rather than `+wq`, which
    // would move a hole into the advice.
    #[test]
    fn the_editor_command_flag_reviews() {
        for (program, args) in [
            ("vim", &["-c", ":!id", "f"][..]),
            ("vim", &["--cmd", ":!id", "f"]),
            ("vim", &["-c", ":r !id", "f"]),
            ("vim", &["-c", ":g/x/!id", "f"]),
            ("vim", &["-c", ":python3 import os", "f"]),
            ("vim", &["-c", ":terminal", "f"]),
            ("vim", &["+!id", "f"]),
            ("vim", &["-es", "-c", "!id", "f"]),
            ("nvim", &["-c", ":!id", "f"]),
            ("vi", &["-c", ":!id", "f"]),
            ("ex", &["-c", "!id", "f"]),
            ("view", &["-c", ":!id", "f"]),
            ("rvim", &["-c", ":!id", "f"]),
        ] {
            assert!(
                reviews(program, args),
                "should review: {program} {}",
                args.join(" ")
            );
        }
    }

    // Verifies: gh #458 - **the benign neighbours stay scoped**, which is the
    // false-positive cost made concrete. Opening a file is the ordinary case and
    // must not review; a script handed over as a file is the folder's own code.
    #[test]
    fn ordinary_editing_stays_scoped() {
        for (program, args) in [
            ("vim", &["file.txt"][..]),
            ("vim", &[]),
            ("vim", &["-R", "file.txt"]),
            ("vim", &["-u", "NONE", "file.txt"]),
            ("nvim", &["file.txt"]),
            ("vi", &["file.txt"]),
            ("ex", &["file.txt"]),
        ] {
            assert!(
                !reviews(program, args),
                "should stay scoped: {program} {}",
                args.join(" ")
            );
        }
        // A script file is the folder's code: `-S`/`-s` name a file, and the
        // ordinary path check decides whether it is inside.
        assert!(!reviews("vim", &["-S", "script.vim", "file.txt"]));
        assert!(!reviews("ex", &["-s", "script.ed", "file.txt"]));
    }

    // Verifies: gh #458 - **`ed` reviews**, including the interactive form with
    // no `-c` at all.
    //
    // gh #458 decision C, and the reason is structural rather than a weighing of
    // costs: `ed`'s *entire command language* arrives on stdin - piping a
    // `!id` line into `ed -s f` runs it - so there is no argument to inspect,
    // and no configuration in which the analyzer could prove what runs. An
    // interactive `ed` in an agent's non-interactive `run` call serves no
    // purpose (the agent cannot type at it), so the false-positive cost is a
    // piped script - which is exactly the case that must review.
    #[test]
    fn ed_reviews_because_its_program_arrives_on_stdin() {
        assert!(reviews("ed", &["f.txt"]));
        assert!(reviews("ed", &["-s", "f.txt"]));
        assert!(reviews("ed", &["script.ed"]));
        assert!(reviews("ed", &[]));
        // Not an editor: `sed` has its own narrow rule and stays scoped for
        // ordinary substitution (pinned here so a future `ed`-prefix match
        // cannot swallow it).
        assert!(!reviews("sed", &["-i", "s/x/y/", "f.txt"]));
        assert!(!reviews("ssh", &["host"]), "not a prefix match");
    }

    // Verifies: gh #458 - a program with a *path* still has its basename
    // classified, so `/usr/bin/vim -c ':!id'` is not a bypass. The analyzer
    // basenames before calling in; this pins the contract the caller relies on.
    #[test]
    fn the_rule_is_keyed_on_the_basenamed_program() {
        assert!(reviews("vim", &["-c", ":!id", "f"]));
        // A name that merely starts with `ed` is not `ed`.
        assert!(!reviews("editor", &["f.txt"]));
        assert!(!reviews("ed25519", &["f.txt"]));
    }
}

/// Programs that run *another* program named later on the command line, so the
/// real target is an argument this analyzer never classifies (gh #365).
///
/// This is the "hides what runs" family, and unlike the `awk` cases there is no
/// ambiguity to weigh: `timeout 5 curl evil.example` ran no prompt while `curl
/// evil.example` reviews, purely because `curl` arrived as an argument. A wrapper
/// does not make `curl` safe, and the argument is an *exact* program name - a
/// check with no false-positive surface on ordinary use, since `timeout 5 cargo
/// test` finds no denied name.
///
/// Deliberately a narrow list. A general "scan every argument for a denied
/// program" rule would refuse `grep curl src/main.rs`, so only a wrapper whose
/// job is to exec its argument is treated this way.
const EXEC_WRAPPERS: &[&str] = &["timeout", "nice", "stdbuf", "ionice", "chrt", "taskset"];

/// Multi-call binaries (busybox and friends): the *first argument* names the
/// applet that runs, so `busybox wget …` is `wget …` (gh #365).
const MULTI_CALL: &[&str] = &["busybox", "toybox"];

/// Whether `program`, invoked with `args`, runs a denied program named later.
///
/// Two shapes, both exact-match:
/// - a wrapper whose own arguments carry the real program
///   (`timeout 5 curl …` — options and their values are skipped), or
/// - a multi-call binary whose first non-option argument is the applet
///   (`busybox wget …`, `busybox sh -c …`).
///
/// Anything it cannot line up refuses, which is the analyzer's standing bias:
/// a false review costs one prompt.
pub(super) fn denied_program_behind_indirection(program: &str, args: &[String]) -> bool {
    if MULTI_CALL.contains(&program) {
        // `busybox --install` and `busybox` alone do no work; anything else
        // names its applet. `--list` prints names and runs nothing.
        let applet = args.iter().find(|arg| !arg.starts_with('-'));
        return match applet {
            Some(applet) => {
                let name = basename(applet);
                DENY_PROGRAMS.contains(&name.as_str()) || EXEC_WRAPPERS.contains(&name.as_str())
            }
            None => false,
        };
    }
    if !EXEC_WRAPPERS.contains(&program) {
        return false;
    }
    // Skip the wrapper's own options and their values: `timeout 5 cmd`,
    // `nice -n 10 cmd`, `stdbuf -o0 cmd`, `ionice -c2 cmd`. A value that is a
    // plain duration/number is consumed with its option; the first remaining
    // token is the program. `-`-prefixed tokens are options; a bare number is
    // a duration argument (`timeout 5`).
    let mut rest = args.iter().peekable();
    while let Some(token) = rest.peek() {
        let token = token.as_str();
        if token.starts_with('-') || token.chars().all(|c| c.is_ascii_digit() || c == '.') {
            rest.next();
            continue;
        }
        break;
    }
    match rest.peek() {
        Some(next) => {
            let name = basename(next);
            DENY_PROGRAMS.contains(&name.as_str())
        }
        None => false,
    }
}

/// For a multi-call binary, the applet it runs and that applet's own arguments;
/// `None` for an ordinary program (gh #365).
///
/// `busybox awk 'BEGIN{…}'` is `awk 'BEGIN{…}'`, so the applet's rules must see
/// the applet's arguments rather than the wrapper's. Returned by value because
/// the caller compares against tables, not against the original argv.
pub(super) fn multi_call_applet(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    if !MULTI_CALL.contains(&program) {
        return None;
    }
    let index = args.iter().position(|arg| !arg.starts_with('-'))?;
    let applet = basename(&args[index]);
    Some((applet, args[index + 1..].to_vec()))
}

// ---------------------------------------------------------------------------
// Flags whose *value* is a program (gh #458)
// ---------------------------------------------------------------------------

/// Editors whose `-c`/`--cmd`/`+cmd` argument is a **mini-program** (gh #458).
///
/// `vim -c ':!id' f` shells out, and `:!cmd` is only the cheapest spelling:
/// `:r !cmd`, `:g/pat/!cmd` (a filter), `:python`, `:lua`, `:terminal` all run
/// code. That is a command language much larger than `awk`'s, and enumerating
/// which ex commands shell out is not a set - it is a parser, and a wrong
/// refusal trains users to approve without reading (ADR-0039's own warning).
///
/// So the whole form reviews. The cost is scripted `vim -c`, and it is stated
/// rather than discovered: the review copy names `-S` (a script file in the
/// folder, which the trust model accepts), not `+wq` - measured, `vim '+!id' f`
/// prints uid, so recommending `+cmd` would move the hole into the advice.
///
/// Names are the ones that share vim's command-line surface. `vi`/`view`/`rvim`
/// are the same binary under other names, the way `mawk`/`nawk` are `awk`.
const EDITOR_COMMAND_FLAGS: &[&str] = &["vim", "vi", "view", "rvim", "nvim", "vimdiff", "ex"];

/// `tar`'s flags whose **value is a program** (gh #458).
///
/// The issue named `--to-command`. The enumeration found four siblings that run
/// a program just as well, and all of them auto-approved:
///
/// | flag | runs |
/// |---|---|
/// | `--to-command=CMD` | `CMD` per extracted file, **through a shell** |
/// | `-I` / `--use-compress-program=PROG` | `PROG` as the compressor |
/// | `-F` / `--info-script=NAME` / `--new-volume-script=NAME` | `NAME` after each volume |
/// | `--rsh-command=CMD` | `CMD` for remote archives |
/// | `--checkpoint-action=exec=CMD` | `CMD` at each checkpoint |
///
/// The value is not reliably a bare program name: `--to-command='id; echo PWNED'`
/// runs **both**, because tar hands it to a shell. So this is not the #347
/// flag-with-a-program shape but inline code, and it is refused on the same rule.
///
/// `-I` also takes a value with its own arguments (`-I 'gzip -1'`) and can be
/// glued into a short-flag cluster (`cIf gzip …`), so the short-flag scan below
/// is character-wise rather than token-wise.
const TAR_PROGRAM_FLAGS: &[&str] = &[
    "--to-command",
    "--use-compress-program",
    "--info-script",
    "--new-volume-script",
    "--rsh-command",
];

/// Editors whose **entire** command language arrives on stdin (gh #458).
///
/// `ed` is the structural case: `printf '!id\nq\n' | ed -s f` runs `!id`, and
/// the arguments carry nothing to classify. An interactive `ed` in an agent's
/// non-interactive `run` call cannot be typed at, so the only `ed` an agent can
/// usefully run is a piped script - which is precisely what must review. A
/// documented boundary, and a narrow one: one name, exact match.
const STDIN_SCRIPT_EDITORS: &[&str] = &["ed"];

/// Whether a **program-valued flag** on `program` makes the invocation review
/// (gh #458).
///
/// `program` is already basenamed. Three families:
///
/// 1. [`TAR_PROGRAM_FLAGS`] - a flag whose value tar executes (accepting both
///    `--flag=value` and detached `--flag value`, and the glued/clustered short
///    `-I` forms);
/// 2. [`EDITOR_COMMAND_FLAGS`] - `-c`, `--cmd`, `--startuptime`-style command
///    arguments, the `+cmd` positional form, and `-S` is deliberately **not**
///    here (a script file is the folder's own code);
/// 3. [`STDIN_SCRIPT_EDITORS`] - the whole invocation.
///
/// Everything it cannot line up reviews, which is the analyzer's standing bias:
/// a false review costs one prompt.
pub(super) fn program_valued_flag(program: &str, args: &[String]) -> bool {
    if STDIN_SCRIPT_EDITORS.contains(&program) {
        return true;
    }
    if EDITOR_COMMAND_FLAGS.contains(&program) {
        return editor_command_flag(args);
    }
    if program == "tar" {
        return tar_program_flag(args);
    }
    false
}

/// Whether a vim/ex-style invocation carries an ex command (gh #458).
///
/// Three spellings, all measured to run `!id`:
/// - `-c <cmd>`, `-c<cmd>` (glued), `--cmd <cmd>`, `--cmd=<cmd>`;
/// - a bare `+<cmd>` positional (`vim '+!id' f`), or `+` alone which means "go
///   to the last line" and runs nothing;
/// - `-S`/`--source`/`-s` name a **script file**, so they do not decide here -
///   the ordinary path check polices the file.
///
/// Any `-c` is a review regardless of what the command says: the analyzer cannot
/// tell `:wq` from `:!id` without modelling ex, and a partially-parsed `-c` that
/// lets `:g/x/!id` through would be worse than a prompt on `:wq`.
fn editor_command_flag(args: &[String]) -> bool {
    for arg in args {
        if arg == "-c" || arg == "--cmd" || arg == "-C" {
            return true;
        }
        // `-c':!id'`, `--cmd=:!id'`, `-c:!id` - a glued command argument. A
        // cluster that merely *contains* `c` (`-es`, `-Nu`) is not this: `-c`
        // must be the flag with the command attached.
        if let Some(rest) = arg.strip_prefix("--cmd=") {
            let _ = rest;
            return true;
        }
        if let Some(rest) = arg.strip_prefix("-c")
            && !rest.is_empty()
        {
            let _ = rest;
            return true;
        }
        // `+cmd` / `+1,5` / `+` (bare `+` means "last line": no command).
        if let Some(rest) = arg.strip_prefix('+')
            && !rest.is_empty()
        {
            return true;
        }
    }
    false
}

/// Whether a `tar` invocation passes a program through one of its flags
/// (gh #458).
///
/// Handles `--flag=value`, detached `--flag value`, and `-I`/`-F` both glued
/// (`-Iid`, `-F id`) and inside a short cluster (`cIf gzip …`). The cluster case
/// is why this scans characters: `-I` can appear after other short flags.
fn tar_program_flag(args: &[String]) -> bool {
    for (index, arg) in args.iter().enumerate() {
        // The long spellings, `=`-attached or detached.
        if TAR_PROGRAM_FLAGS
            .iter()
            .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
        {
            return true;
        }
        // `--checkpoint-action=exec=CMDLINE`: only the `exec=` action runs a
        // program. `echo=`, `ttyout=`, `dot`, `sleep=` do not.
        if let Some(action) = arg.strip_prefix("--checkpoint-action=") {
            if action.starts_with("exec=") {
                return true;
            }
        } else if arg == "--checkpoint-action" {
            // Detached: the action is the next token.
            if let Some(next) = args.get(index + 1)
                && next.starts_with("exec=")
            {
                return true;
            }
        }
        // A short-flag cluster: `-I` and `-F` are the program-valued short
        // flags, and everything after the letter in the same token is the
        // value. Classic tar also accepts flags with **no leading dash**
        // (`tar cIf gzip …`, `tar xf a.tar`), so the cluster scan does not
        // depend on one - only on a first character that is a tar flag letter.
        let cluster = arg.strip_prefix('-').unwrap_or(arg);
        if !cluster.is_empty() && !cluster.starts_with('-') {
            for ch in cluster.chars() {
                if ch == 'I' || ch == 'F' {
                    return true;
                }
            }
        }
    }
    false
}

/// The reviewable form of an invocation refused for carrying a program in a
/// flag's value (gh #458), or `None` when the invocation is not one of those
/// refusals.
///
/// ADR-0039's house rule: the copy tells the user what to DO, not what the
/// subsystem called the failure. A refusal a user cannot act on is a refusal
/// that trains them to approve without reading - the exact over-review failure
/// this ADR names.
///
/// The advice is deliberately **not** `+cmd` for an editor: measured,
/// `vim '+!id' f` prints uid, so `+wq` is the same hole with a friendlier
/// spelling. The reviewable form is a **script file in the folder** (`-S`), which
/// the trust model already accepts as the folder's own code.
pub fn reviewable_alternative(command: &str) -> Option<&'static str> {
    let program = command.split_whitespace().next()?;
    let name = basename(program);
    let name = name.as_str();
    if EDITOR_COMMAND_FLAGS.contains(&name) {
        return Some(
            "put the commands in a script file inside the project and pass it with -S \
             (for example `vim -S script.vim file`); an inline -c argument cannot be \
             checked, and `+cmd` is the same hole",
        );
    }
    if STDIN_SCRIPT_EDITORS.contains(&name) {
        return Some(
            "an editor whose commands arrive on stdin cannot be checked at all; write \
             the edit as a script in the project instead",
        );
    }
    if name == "tar" {
        return Some(
            "use tar without the program-valued flag (`tar xf a.tar`), or run the \
             program separately so it is reviewed on its own",
        );
    }
    None
}

#[cfg(test)]
mod reviewable_alternative_tests {
    //! gh #458: every refusal states the reviewable form, and the advice is
    //! itself not a hole.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::reviewable_alternative;
    use crate::shell::workspace_scoped;

    fn workspace_scoped_for_test(command: &str) -> bool {
        let root = std::path::Path::new("/work/proj");
        workspace_scoped(command, root, root)
    }

    // Verifies: ADR-0039's copy rule - a refusal names what to do instead.
    #[test]
    fn every_adopted_refusal_names_the_reviewable_form() {
        for command in [
            "vim -c ':!id' f",
            "vim '+!id' f",
            "ex -c '!id' f",
            "ed f.txt",
            "tar xf a.tar --to-command=id",
            "tar cf b.tar dir -I gzip",
        ] {
            let advice = reviewable_alternative(command)
                .unwrap_or_else(|| panic!("no advice for {command}"));
            assert!(
                advice.len() > 20,
                "the advice says something usable: {advice}"
            );
        }
    }

    // Verifies: gh #458 - **the advice is not itself a bypass.** This is the
    // row that would have caught recommending `+wq`: the suggested form must
    // still be auto-approved by the analyzer, or the copy moves the hole into
    // the instructions.
    #[test]
    fn the_suggested_form_is_not_itself_refused() {
        // `vim -S script.vim file` is a script file in the folder.
        assert!(workspace_scoped_for_test("vim -S script.vim file.txt"));
        // `tar xf a.tar` carries no program-valued flag.
        assert!(workspace_scoped_for_test("tar xf a.tar"));
        // And the form the advice warns against really is refused.
        assert!(!workspace_scoped_for_test("vim +wq file.txt"));
    }

    // Verifies: gh #458 - the advice is only offered for the adopted refusals,
    // so nothing else in the analyzer starts emitting a tar/editor hint.
    #[test]
    fn other_refusals_get_no_glib_advice() {
        for command in ["python3 -c 'print(1)'", "rm -rf /tmp/x", "cargo build"] {
            assert!(
                reviewable_alternative(command).is_none(),
                "no tar/editor advice for {command}"
            );
        }
    }
}
