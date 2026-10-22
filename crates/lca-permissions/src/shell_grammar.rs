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
