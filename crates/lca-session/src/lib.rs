//! Session storage: the append-only log, fork, resume, export, and the
//! durable compaction records, per `docs/session-log-format.md`.
//!
//! Records are never rewritten in place. Compaction appends a marker that
//! names the range it replaces; readers substitute the summary without
//! touching the stored records.

#![forbid(unsafe_code)]

mod cache;
mod ids;
mod store;
mod tree;
pub(crate) mod view;

use std::path::PathBuf;

pub use cache::{CacheMiss, CacheWasteTotals, collect_cache_misses, compute_cache_waste};
pub use lca_protocol::{PermissionDecision, ToolResultStatus, ToolSource};
pub use store::{
    DEFAULT_TITLE, ExportOptions, ReadOutcome, Session, SessionMeta, SessionStore, SessionSummary,
    display_path, row_label,
};
pub use tree::{EntryKind, EntryRow};
pub use view::ViewMode;

/// The extension ABI version recorded in a `session-start` record, sourced
/// from the contract crate so it cannot drift from the frozen `lca:ext`
/// version (`docs/session-log-format.md`: "the ABI version").
pub use lca_ext_abi::ABI_VERSION;

/// Resolve the user data directory: `$HOME/.lca` on every platform
/// (`USERPROFILE` on Windows when `HOME` is unset).
///
/// One predictable home dot-directory, pi's `~/.pi` shape (R7,
/// 2026-10-01). The platform-conventional split - `~/.local/share` vs
/// `~/Library/Application Support` vs `%APPDATA%` - meant the owner's own
/// agent could not find its state on Windows, and "where is my data" is not
/// a question the product should leave to a specification. The whole tree
/// lives here: `sessions/`, `extensions/`, `credentials/`, `grants.json`,
/// `state/`, `tmp/`, `themes/`, `ui.json`, and `config.toml`.
///
/// **No migration.** The old platform directories are left untouched; the
/// CHANGELOG and `docs/platform-notes.md` name them and say to copy the
/// directory across if the old sessions are wanted.
pub fn default_data_dir() -> PathBuf {
    home_dir().join(".lca")
}

/// A fresh record identifier: sortable by creation order, unique within the
/// process. The core loop stamps user and assistant records with these.
pub fn new_record_id() -> String {
    ids::record_id(ids::now_ms())
}

/// Milliseconds since the Unix epoch, for record timestamps.
pub fn now_ms() -> u64 {
    ids::now_ms()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Errors this crate returns.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem failure, with the path involved.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// JSON failure while reading or writing a session file.
    #[error("invalid JSON in a session file: {0}")]
    Json(#[from] serde_json::Error),
    /// The named fork point does not exist in the parent session.
    #[error("fork point `{record}` not found in session {session}")]
    ForkPointMissing {
        /// The session searched.
        session: String,
        /// The record identifier sought.
        record: String,
    },
    /// A fork's parent directory is gone; the reader reports truncation.
    #[error("parent session {session} is missing; history is truncated")]
    MissingParent {
        /// The vanished session.
        session: String,
    },
    /// The named session does not exist in this project (exit code 6).
    #[error("session {id} is missing, malformed, or belongs to another project")]
    UnknownSession {
        /// The id that was sought.
        id: String,
    },
    /// A fork chain loops back on itself.
    #[error("fork cycle detected at session {session}")]
    ForkCycle {
        /// Where the cycle closed.
        session: String,
    },
    /// A session id breaks pi's rules (gh #69): letters, numbers, `.`,
    /// `_`, and `-`, starting and ending with a letter or number.
    #[error("invalid session id `{id}`: use letters, numbers, `.`, `_`, `-`")]
    InvalidId {
        /// The rejected id.
        id: String,
    },
    /// A branch names a record no resolved history holds (gh #37).
    #[error("branch target `{record}` not found in session {session}")]
    BranchTargetMissing {
        /// The session searched.
        session: String,
        /// The record identifier sought.
        record: String,
    },
    /// A label names a record no resolved history holds (gh #37).
    #[error("label target `{record}` not found in session {session}")]
    LabelTargetMissing {
        /// The session searched.
        session: String,
        /// The record identifier sought.
        record: String,
    },
}

/// Whether a session id is usable (gh #69, pi's constraints): letters,
/// numbers, `.`, `_`, and `-`, starting and ending alphanumerically.
/// Generated ids always pass; anything else is refused before touching
/// the store, so ids can never escape their directory.
pub fn valid_session_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let edge = |byte: &u8| byte.is_ascii_alphanumeric();
    if !edge(&bytes[0]) || !edge(&bytes[bytes.len() - 1]) {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
