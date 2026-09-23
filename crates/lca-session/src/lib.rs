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
mod view;

use std::path::PathBuf;

pub use cache::{CacheMiss, CacheWasteTotals, collect_cache_misses, compute_cache_waste};
pub use lca_protocol::{PermissionDecision, ToolResultStatus, ToolSource};
pub use store::{ExportOptions, ReadOutcome, Session, SessionMeta, SessionStore, SessionSummary};
pub use view::ViewMode;

/// The extension ABI version recorded in new session logs until `lca-ext-abi`
/// owns the constant (Phase 2 moves this into the contract crate).
pub const ABI_VERSION: &str = "0.1";

/// Resolve the user data directory the way each platform documents it
/// (`docs/platform-notes.md`): `$XDG_DATA_HOME` or `~/.local/share` on
/// Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows.
pub fn default_data_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home_dir().join(".local/share"))
            .join("lca")
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library/Application Support/lca")
    } else {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
            .join("lca")
    }
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
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
