//! The resume command (gh #403 gap 2): **one** formatter for every place that
//! names how to come back.
//!
//! Two call sites need this string - the fullscreen-exit notice (gh #82, which
//! predates this) and the exit notice (gh #403 gap 2) - so it lives here rather
//! than in either of them. Two `format!`s would drift, the same rule #456 applied
//! to its grant mapping.
//!
//! Split from `run.rs` for the 1,200-line file ceiling (gate 11); behaviour
//! unchanged.

/// Quote one argument for a POSIX shell, or for `cmd`-ish Windows shells.
///
/// Only the characters that *matter* are quoted: a plain `~/.lca` stays
/// unquoted so the common command is clean, and a path with a space, a quote, or
/// a metacharacter comes back single-quoted with the embedded `'` escaped the
/// POSIX way (`'` -> `'\''`). Windows gets double quotes with `"` doubled,
/// because single quotes are not special to `cmd`/PowerShell the same way.
///
/// ponytail: two quoting dialects, not every shell on earth. The printed hint is
/// for a human pasting into the shell that launched `lca`; upgrade to a
/// shell-probing quote if a third dialect ever reports breakage.
fn shell_quote(arg: &str) -> String {
    let windows = cfg!(windows);
    // Conservative allow-list: anything not plainly inert gets quoted. A `'` is
    // inert to `cmd` but opens a quote to a POSIX shell, so it is unsafe on the
    // POSIX side only.
    let safe = |byte: char| {
        byte.is_ascii_alphanumeric() || "-_./~:+@%=,;".contains(byte) || (windows && byte == '\'')
    };
    // A leading `-` would read as a flag to the tool being invoked, so it is
    // quoted even though it is otherwise inert.
    if !arg.is_empty() && arg.chars().all(safe) && !arg.starts_with('-') {
        return arg.to_string();
    }
    if windows {
        format!("\"{}\"", arg.replace('"', "\"\""))
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// The command that brings this session back (gh #403 gap 2): **one** formatter,
/// called by the fullscreen-exit hint and by the exit notice, so the two cannot
/// drift apart.
///
/// `session_dir` is the session's own directory when it is *not* under the
/// default data home - a relocated home, or a session carried in from elsewhere -
/// in which case `--session` is named as well, because `--resume <id>` alone
/// resolves against the current data home and would fail. It is quoted so the
/// printed line is pasteable on a path with spaces.
///
/// Pure and total: no I/O, no id validation. The caller decides *whether* to
/// print (TTY, saved session); this decides only *what* the command is.
pub fn resume_command(session_id: &str, session_dir: Option<&std::path::Path>) -> String {
    match session_dir {
        Some(dir) => format!("lca --session {}", shell_quote(&dir.display().to_string())),
        None => format!("lca --resume {}", shell_quote(session_id)),
    }
}

/// The exit notice (gh #403 gap 2): the command that brings this session back,
/// for a user who quits any way other than leaving fullscreen.
///
/// `None` when there is nothing honest to say, which is the whole design:
/// - no id (the run never reached `session-start`, so nothing was written);
/// - a non-TTY stdout (a piped or headless run must gain zero bytes - scripted
///   users parse this output, and a hint that cannot be clicked is noise).
///
/// The caller has already closed the session before asking, so an id here means
/// the file exists. Kept pure so the decision is testable without a terminal.
pub fn exit_resume_hint(
    session_id: &str,
    session_dir: Option<&std::path::Path>,
    stdout_is_tty: bool,
) -> Option<String> {
    if session_id.is_empty() || !stdout_is_tty {
        return None;
    }
    Some(format!(
        "resume with: {}",
        resume_command(session_id, session_dir)
    ))
}
