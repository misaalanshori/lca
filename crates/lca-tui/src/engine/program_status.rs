//! Program Status Protocol (OSC 7501, gh #213, pi 1.1.0 #10607): the
//! agent tells the terminal whether it is idle, working, blocked on the
//! user, done, or failed, so tab bars and dashboards can badge it.
//!
//! Privacy law: reports carry only the state enum and `app=lca` —
//! never prompt text, file contents, or model output.

/// The lifecycle states a tab badge can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    /// The editor waits for input.
    Idle,
    /// A turn runs (generation or a tool).
    Working,
    /// A modal waits for the user (permission, login, question).
    Blocked,
    /// The turn finished cleanly.
    Done,
    /// The turn failed without retry.
    Error,
    /// Shutdown: remove the badge.
    Clear,
}

impl ProgramState {
    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Blocked => "blocked",
            ProgramState::Done => "done",
            ProgramState::Error => "error",
            ProgramState::Clear => "clear",
        }
    }
}

/// Encode one status report. `clear` carries no app pair (the issue's
/// syntax verbatim); every other state names `app=lca` and nothing else.
pub fn format_program_status(state: ProgramState) -> String {
    match state {
        ProgramState::Clear => "\x1b]7501;state=clear\x1b\\".to_string(),
        _ => format!("\x1b]7501;state={}:app=lca\x1b\\", state.name()),
    }
}

/// The support query, asked at startup beside the Kitty negotiation. A
/// terminal that does not know OSC 7501 ignores it.
pub const PROGRAM_STATUS_QUERY: &str = "\x1b]7501;?\x1b\\";

/// Whether an inbound sequence is the support reply (ST or BEL
/// terminated; later spec revisions may append pairs after the `?`).
pub fn is_program_status_reply(sequence: &str) -> bool {
    let Some(body) = sequence.strip_prefix("\x1b]7501;") else {
        return false;
    };
    let body = body
        .strip_suffix("\x1b\\")
        .or_else(|| body.strip_suffix('\x07'));
    body.is_some_and(|inner| inner == "?" || inner.starts_with("?;"))
}

/// Whether an inbound sequence is any OSC 7501 traffic: replies and
/// unknown future revisions are consumed, never forwarded as input.
pub fn is_program_status_traffic(sequence: &str) -> bool {
    sequence.starts_with("\x1b]7501;")
        && (sequence.ends_with("\x1b\\") || sequence.ends_with('\x07'))
}

/// How reporting is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramStatusMode {
    /// `LCA_PROGRAM_STATUS=1`: report without asking.
    ForcedOn,
    /// `LCA_PROGRAM_STATUS=0`: never report.
    ForcedOff,
    /// Otherwise: report only after the terminal confirms support.
    Auto,
}

impl ProgramStatusMode {
    /// Whether the startup support query is worth asking: only auto
    /// mode needs detection (forced modes already decided).
    pub fn needs_query(self) -> bool {
        matches!(self, ProgramStatusMode::Auto)
    }
}

/// Read the `LCA_PROGRAM_STATUS` override over an injected environment
/// (tests pass a map; only `1` and `0` count, like `LCA_HYPERLINKS`).
pub fn program_status_mode(env: impl Fn(&str) -> Option<String>) -> ProgramStatusMode {
    match env("LCA_PROGRAM_STATUS").as_deref() {
        Some("1") => ProgramStatusMode::ForcedOn,
        Some("0") => ProgramStatusMode::ForcedOff,
        _ => ProgramStatusMode::Auto,
    }
}

/// Whether the multiplexer strips OSC 7501 (`screen` does; `tmux` is
/// left to the query/reply detection, which stays silent without a
/// reply). Same injected-environment shape as the hyperlink ladder.
pub fn multiplexer_strips_osc(env: impl Fn(&str) -> Option<String>) -> bool {
    if env("STY").is_some() {
        return true;
    }
    env("TERM")
        .unwrap_or_default()
        .to_lowercase()
        .starts_with("screen")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #213 - exact bytes per lifecycle state.
    #[test]
    fn states_encode_to_the_issue_syntax() {
        assert_eq!(
            format_program_status(ProgramState::Working),
            "\x1b]7501;state=working:app=lca\x1b\\"
        );
        assert_eq!(
            format_program_status(ProgramState::Blocked),
            "\x1b]7501;state=blocked:app=lca\x1b\\"
        );
        assert_eq!(
            format_program_status(ProgramState::Done),
            "\x1b]7501;state=done:app=lca\x1b\\"
        );
        assert_eq!(
            format_program_status(ProgramState::Error),
            "\x1b]7501;state=error:app=lca\x1b\\"
        );
        assert_eq!(
            format_program_status(ProgramState::Idle),
            "\x1b]7501;state=idle:app=lca\x1b\\"
        );
        assert_eq!(
            format_program_status(ProgramState::Clear),
            "\x1b]7501;state=clear\x1b\\"
        );
    }

    // Verifies: gh #213 - the support reply parses (ST and BEL
    // terminators); anything else is not a reply.
    #[test]
    fn replies_parse_and_non_replies_do_not() {
        assert!(is_program_status_reply("\x1b]7501;?\x1b\\"));
        assert!(is_program_status_reply("\x1b]7501;?\x07"));
        assert!(!is_program_status_reply(
            "\x1b]7501;state=working:app=lca\x1b\\"
        ));
        assert!(!is_program_status_reply("\x1b[A"));
        assert!(!is_program_status_reply("\x1b]11;?\x07"));
    }

    // Verifies: gh #213 - unknown 7501 traffic is swallowed, never input.
    #[test]
    fn unknown_traffic_is_consumed() {
        assert!(is_program_status_traffic("\x1b]7501;?;v=2\x1b\\"));
        assert!(!is_program_status_traffic("\x1b]11;?\x07"));
        assert!(!is_program_status_traffic("\x1b]7501;?"));
    }

    // Verifies: gh #213 - `LCA_PROGRAM_STATUS=1` forces on, `=0`
    // forces off, anything else auto-detects.
    #[test]
    fn the_env_override_wins() {
        let env = |key: &str| match key {
            "LCA_PROGRAM_STATUS" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(program_status_mode(env), ProgramStatusMode::ForcedOn);
        let env = |key: &str| match key {
            "LCA_PROGRAM_STATUS" => Some("0".to_string()),
            _ => None,
        };
        assert_eq!(program_status_mode(env), ProgramStatusMode::ForcedOff);
        let env = |_: &str| None;
        assert_eq!(program_status_mode(env), ProgramStatusMode::Auto);
        let env = |key: &str| match key {
            "LCA_PROGRAM_STATUS" => Some("yes".to_string()),
            _ => None,
        };
        assert_eq!(program_status_mode(env), ProgramStatusMode::Auto);
    }

    // Verifies: gh #213 - screen strips OSC (never reported under it).
    #[test]
    fn screen_disables_reporting() {
        let env = |key: &str| match key {
            "TERM" => Some("screen-256color".to_string()),
            _ => None,
        };
        assert!(multiplexer_strips_osc(env));
        let env = |_: &str| None;
        assert!(!multiplexer_strips_osc(env));
    }
}
