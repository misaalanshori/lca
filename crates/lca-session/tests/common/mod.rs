//! The three fixtures every `lca-session` test target shares (gh #564 phase 2).
//!
//! Nine targets had each grown their own private copy of `scratch`, `store`, and
//! `user_record` - byte-identical in all nine, which is the shape a drift defect
//! takes: the day one of them changes, only the rows in that file keep passing.
//! They live here once, and every target declares `mod common;` for them.
//!
//! `store` is deliberately `SessionStore::new(scratch(name))` and nothing else: a
//! target that needs a different root (a sandboxed `HOME`, a project subdirectory)
//! builds it from `scratch` in its own row, so this fixture cannot quietly change
//! what a row sets up.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::SessionStore;

/// A fresh scratch directory for one row.
pub fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

/// A store rooted in a fresh scratch directory.
pub fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

/// One plain `user` record: the shape every log row appends first.
pub fn user_record(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}
