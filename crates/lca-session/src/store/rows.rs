//! The shapes a session read or a listing hands back (gh #564).
//!
//! `ReadOutcome` is what every reader returns - the records it got, whether the
//! read stopped early (FR-SESS-6), and the notes a skip produced.
//! `SessionMeta` and `SessionSummary` are the two on-disk JSON shapes
//! (`meta.json`, `index.json`); `row_label` and `display_path` are the two
//! formatting rules a picker row or a footer needs. No I/O lives here: the
//! contract these shapes describe is `docs/session-log-format.md`.

use serde::{Deserialize, Serialize};

/// How a session log read ended.
#[derive(Debug, Clone, Default)]
pub struct ReadOutcome {
    /// Everything loaded before any failure.
    pub records: Vec<lca_protocol::Record>,
    /// Whether the read stopped early (FR-SESS-6).
    pub truncated: bool,
    /// Lines skipped because their type or version is beyond this reader.
    pub skipped_unknown: usize,
    /// Non-fatal notes, one per skipped line.
    pub warnings: Vec<String>,
}

/// Export shape (`docs/session-log-format.md`).
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Keep `permission` and `extension-event` records (FR-SESS-7).
    pub audit: bool,
}

/// Session metadata, stored as `meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionMeta {
    /// Record format version this session was created with.
    pub format_version: u32,
    /// Creation time, epoch milliseconds.
    pub created_ms: u64,
    /// Agent version that created the session.
    pub agent_version: String,
    /// Model last used, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider last used, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Canonical working directory.
    pub working_dir: String,
    /// Human title.
    pub title: String,
    /// Parent session, when this session is a fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    /// Record in the parent the fork was taken at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_record: Option<String>,
}

/// One line of `index.json`; a cache rebuilt from the directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionSummary {
    /// Session identifier (its directory name).
    pub id: String,
    /// Title.
    pub title: String,
    /// Creation time, epoch milliseconds.
    pub created_ms: u64,
    /// Last modification, epoch milliseconds.
    pub modified_ms: u64,
    /// Number of user and assistant messages in this session's own log
    /// (compaction summaries count too: they are resumable content,
    /// gh #253).
    pub message_count: usize,
    /// Parent session, when forked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    /// The first user prompt, when the session has one (a listing label;
    /// see [`SessionSummary::display_title`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_user: Option<String>,
}

/// The title a fresh session gets when its creator has nothing better to
/// give it. The listing falls back to the first prompt while a session
/// still carries this (see [`row_label`]).
pub const DEFAULT_TITLE: &str = "session";

/// The label a session listing shows: its title when it has one, its first
/// prompt when it still carries [`DEFAULT_TITLE`]. Pi's session rows are
/// "name-or-first-message" (`pi-tui-re/src_re/agent-components/
/// selectors-large.md`), and a picker full of identically-titled rows tells
/// its user nothing - which is what a manual drive against pi showed on
/// 2026-10-01. Control characters are stripped and the line is cut to 80
/// columns, because a prompt can carry an escape sequence and a picker row
/// must not render one.
pub fn row_label(title: &str, first_user: Option<&str>) -> String {
    if title != DEFAULT_TITLE {
        return title.to_string();
    }
    let Some(text) = first_user else {
        return title.to_string();
    };
    // Strip escape sequences whole before shaping: a prompt can paste an
    // ANSI snippet, and dropping only the ESC byte would leave `[31m` as
    // row text. States: 0 normal, 1 after ESC, 2 CSI, 3 OSC, 4 OSC's ESC.
    let mut visible = String::with_capacity(text.len());
    let mut state = 0u8;
    for ch in text.chars() {
        match state {
            1 => {
                state = match ch {
                    '[' => 2,
                    ']' => 3,
                    _ => 0,
                };
            }
            2 => {
                if ('\x40'..='\x7e').contains(&ch) {
                    state = 0;
                }
            }
            3 => {
                if ch == '\x07' {
                    state = 0;
                } else if ch == '\x1b' {
                    state = 4;
                }
            }
            4 => {
                state = 0;
            }
            _ => {
                if ch == '\x1b' {
                    state = 1;
                } else {
                    visible.push(ch);
                }
            }
        }
    }
    // The row itself: runs of control bytes and spaces collapse to one
    // space, and the label stops at 80 columns with an ellipsis.
    let mut label = String::with_capacity(80);
    let mut last_space = false;
    let mut width = 0;
    for ch in visible.chars() {
        if width >= 80 {
            label.push('…');
            break;
        }
        if ch.is_control() || ch == ' ' {
            if !last_space {
                label.push(' ');
                last_space = true;
                width += 1;
            }
        } else {
            label.push(ch);
            last_space = false;
            width += 1;
        }
    }
    let trimmed = label.trim().to_string();
    if trimmed.is_empty() {
        title.to_string()
    } else {
        trimmed
    }
}

impl SessionSummary {
    /// This session's listing label (see [`row_label`]).
    pub fn display_title(&self) -> String {
        row_label(&self.title, self.first_user.as_deref())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct IndexFile {
    pub(super) version: u32,
    pub(super) sessions: Vec<SessionSummary>,
}

/// The path as a human (and a model) should read it: Windows' verbatim
/// `\\?\` prefix stripped, and `\\?\UNC\server\share` restored to
/// `\\server\share`. Canonicalization stays load-bearing for identity and
/// project-key derivation; this is the display form only (R7b).
pub fn display_path(path: &std::path::Path) -> String {
    // One implementation, applied on every platform: the prefix can only be
    // produced on Windows, so the branches are inert elsewhere and the rule
    // stays unit-testable where it is written.
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    text.into_owned()
}
