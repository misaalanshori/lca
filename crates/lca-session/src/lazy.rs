//! Lazy session creation (gh #122, EFG-034): a fresh session is a
//! handle plus a pending creation spec - no directory, no meta, no
//! log - until the first record lands. Split from `store.rs` under
//! the workspace's 1,200-line file ceiling.
//!
//! The pending registry lives on the store (shared across clones),
//! keyed by session id. `append` consumes the spec and materializes
//! (directory, meta, session-start, then the record); `rename` and
//! `record_model_used` edit the spec when nothing exists yet;
//! readers treat a missing directory as an empty root (never an
//! error); `close` skips what never materialized. Forks and clones
//! always write records at creation, so they stay eager.

use std::path::{Path, PathBuf};

use super::store::{Session, SessionStore};
use crate::{Result, ids};

/// Creation intent for a session with no directory yet (gh #122).
#[derive(Debug, Clone)]
pub(crate) struct PendingSpec {
    /// Display title (renames rewrite this until materialize).
    pub title: String,
    /// Display form of the working directory (R7b).
    pub display_dir: String,
    /// Model last used, when chosen pre-materialize.
    pub model: Option<String>,
    /// Provider last used, when chosen pre-materialize.
    pub provider: Option<String>,
}

impl SessionStore {
    /// A fresh session without touching disk (gh #122): the id is
    /// real, the directory lands on the first record.
    pub fn new_pending(&self, project_dir: &Path, title: &str) -> Session {
        self.new_pending_with_id(
            project_dir,
            &crate::ids::session_id(crate::ids::now_ms()),
            title,
        )
        .unwrap_or_else(|_| Session::new(String::new(), PathBuf::new()))
    }

    /// A fresh session under a chosen id, validating first like the
    /// eager creation (gh #69's rule - an id never escapes its
    /// project directory).
    pub fn new_pending_with_id(
        &self,
        project_dir: &Path,
        id: &str,
        title: &str,
    ) -> Result<Session> {
        if !crate::valid_session_id(id) {
            return Err(crate::Error::InvalidId { id: id.to_string() });
        }
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let display = super::store::display_path(&canonical);
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.insert(
            id.to_string(),
            PendingSpec {
                title: title.to_string(),
                display_dir: display,
                model: None,
                provider: None,
            },
        );
        Ok(Session::new(
            id.to_string(),
            self.project_dir(project_dir).join(id),
        ))
    }

    /// Materialize a pending session for an append (gh #122): the
    /// directory, meta, and session-start land first, in that order.
    /// `Ok(false)` when already real; the record follows.
    pub(crate) fn materialize(&self, session: &Session) -> Result<bool> {
        let spec = match self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session.id())
        {
            Some(spec) => spec,
            None => return Ok(false),
        };
        // A present meta wins over a stale spec, so two creators
        // never clobber each other.
        if session.meta_path().is_file() {
            return Ok(false);
        }
        let now = ids::now_ms();
        let dir = session.dir();
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(dir)?;
        let meta = super::store::SessionMeta {
            format_version: lca_protocol::FORMAT_VERSION,
            created_ms: now,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            model: spec.model,
            provider: spec.provider,
            working_dir: spec.display_dir.clone(),
            title: spec.title,
            parent_session: None,
            parent_record: None,
        };
        super::store::write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)?;
        self.append(
            session,
            lca_protocol::Record::SessionStart {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                abi_version: super::ABI_VERSION.to_string(),
                working_dir: spec.display_dir,
            },
        )?;
        Ok(true)
    }

    /// Open a session that may never have materialized (gh #122): an
    /// existing directory opens strictly (corruption still errors; a
    /// deleted session still refuses); a pending id from this run
    /// yields a bare handle that reads empty until the first record
    /// lands.
    pub fn open_or_pending(&self, project_dir: &Path, id: &str) -> Result<Session> {
        match self.session(project_dir, id) {
            Ok(session) => Ok(session),
            Err(_) => {
                if !crate::valid_session_id(id) {
                    return Err(crate::Error::InvalidId { id: id.to_string() });
                }
                let pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !pending.contains_key(id) {
                    return Err(crate::Error::UnknownSession { id: id.to_string() });
                }
                drop(pending);
                Ok(Session::new(
                    id.to_string(),
                    self.project_dir(project_dir).join(id),
                ))
            }
        }
    }

    /// Ensure a session exists on disk (gh #122): materialize when
    /// pending (attachment staging takes this path - content stages
    /// only into a real directory).
    pub fn ensure_materialized(&self, session: &Session) -> Result<bool> {
        self.materialize(session)
    }
}
