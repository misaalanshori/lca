//! The session store: layout on disk, append-only writes, listing, forking,
//! and export.
//!
//! **The contract is `docs/session-log-format.md`.** Records are never rewritten
//! in place: `append` writes one line and flushes, and compaction appends a
//! marker naming the range it replaces. A reader can therefore always
//! reconstruct history, and a crash loses at most the line being written. A
//! flush that crosses a write, or a write that rewrites a record, is a defect
//! rather than a refactor.
//!
//! Gh #564 split this file along the seams it already had, and no path a caller
//! names changed - `lib.rs` re-exports every public item from this module:
//!
//! | file | what it owns |
//! |---|---|
//! | this file | the two handles (`Session`, `SessionStore`), where a session lives (`sessions_dir`, `project_dir`, `session`, `session_ref`), and the edits that touch only `meta.json` or the id (`rename`, `reid`, `record_model_used`, `meta`, `attachment_path`) |
//! | [`rows`] | the shapes a read or a listing hands back: `ReadOutcome`, `ExportOptions`, `SessionMeta`, `SessionSummary`, `IndexFile`, `row_label`, `display_path` |
//! | [`lines`] | the line primitives: the tip scan, the atomic write, the fork-chain project lookup |
//! | [`read`] | reading: `read`, `ancestors`, `tip_id`, `resolved`, the line decoder, and the listing cache |
//! | [`write`] | writing: `create_session*`, `append`, `close`, import and `raw_start` |
//! | [`forks`] | the tree: `fork`, `clone_session`, `fork_tree`, `branch_at`, `summarize_branch`, `trash`, `gc` |
//!
//! `append` is the only writer of `log.jsonl`, and it stays in `write.rs` beside
//! `close` and the creation path: "one record, one line, then flush" is a rule
//! about one function, not a property of the type.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::ids;

mod forks;
mod lines;
mod read;
mod rows;
mod write;

// The names the crate's other modules name through `crate::store`, unchanged by
// the split (gh #564): `super::store::x` resolves exactly as it did.
pub(crate) use lines::{project_dir_of, write_atomic};
pub use read::known_record_types;
pub use rows::{
    DEFAULT_TITLE, ExportOptions, ReadOutcome, SessionMeta, SessionSummary, display_path, row_label,
};

/// A handle to one session directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    id: String,
    dir: PathBuf,
}

impl Session {
    pub(crate) fn new(id: String, dir: PathBuf) -> Self {
        Session { id, dir }
    }

    /// The session identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the append-only log.
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("log.jsonl")
    }

    /// Path of the metadata file.
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }
}

/// One session's store rooted at the user data directory.
#[derive(Clone)]
pub struct SessionStore {
    root: PathBuf,
    pub(crate) pending: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, crate::lazy::PendingSpec>>,
    >,
}

impl SessionStore {
    /// Root the store at the user data directory (`.../sessions` lives below).
    pub fn new(root: PathBuf) -> Self {
        SessionStore {
            root,
            pending: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// The `sessions/` directory.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Path of a project's `index.json`.
    pub fn index_path(&self, project_dir: &Path) -> PathBuf {
        self.project_dir(project_dir).join("index.json")
    }

    fn project_key(&self, project_dir: &Path) -> String {
        // Identity keys on the canonical path - verbatim included - so two
        // spellings of one directory are one project (ADR-0006). Only the
        // *stored display* form is stripped (R7b, `display_path`).
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        ids::project_key(&canonical.to_string_lossy())
    }

    pub(crate) fn project_dir(&self, project_dir: &Path) -> PathBuf {
        self.sessions_dir().join(self.project_key(project_dir))
    }

    /// Look one session up by id inside this project. Errors when the id is
    /// unknown (the CLI maps this to exit code 6, `docs/headless.md`).
    pub fn session(&self, project_dir: &Path, id: &str) -> Result<Session> {
        let dir = self.project_dir(project_dir).join(id);
        if !dir.join("meta.json").is_file() {
            return Err(crate::Error::UnknownSession { id: id.to_string() });
        }
        Ok(Session::new(id.to_string(), dir))
    }

    /// Open a session by id or by session-directory path (gh #110:
    /// `--session <id|path>`). A reference naming an existing directory
    /// holding `meta.json` opens by path (its directory name is the
    /// id); anything else opens by id, with the usual unknown error.
    pub fn session_ref(&self, project_dir: &Path, reference: &str) -> Result<Session> {
        let path = PathBuf::from(reference);
        if path.is_absolute() || reference.contains('/') || reference.contains('\\') {
            if path.join("meta.json").is_file() {
                let id = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(reference);
                return Ok(Session::new(id.to_string(), path));
            }
            return Err(crate::Error::UnknownSession {
                id: reference.to_string(),
            });
        }
        self.session(project_dir, reference)
    }

    /// Rename a session's id (gh #69: `--fork X --session-id Y`
    /// chooses the fork's id): meta carries no self-id and records
    /// reference parents, so moving the directory and rebuilding the
    /// index is the whole move.
    pub fn reid(&self, session: &Session, new_id: &str) -> Result<Session> {
        if !crate::valid_session_id(new_id) {
            return Err(crate::Error::InvalidId {
                id: new_id.to_string(),
            });
        }
        let dir = session.dir().to_path_buf();
        let target = dir
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .join(new_id);
        if target.exists() {
            return Err(crate::Error::UnknownSession {
                id: new_id.to_string(),
            });
        }
        std::fs::rename(&dir, &target)?;
        let project = target
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""))
            .to_path_buf();
        self.rebuild_index_from_key(&project)?;
        Ok(Session::new(new_id.to_string(), target))
    }

    /// Rename a session: `meta.json` atomically, then the index cache.
    pub fn rename(&self, session: &Session, title: &str) -> Result<()> {
        // gh #37 (pi's `session_info` on this vocabulary): the log
        // carries the rename as an entry (audit trail + file shape),
        // `meta.json` keeps carrying the display title.
        let now = ids::now_ms();
        self.append(
            session,
            lca_protocol::session_info_record(now, ids::record_id(now), title),
        )?;
        let mut meta = self.meta(session)?;
        meta.title = title.to_string();
        write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)?;
        if let Some(project_dir) = session.dir().parent() {
            self.rebuild_index_from_key(project_dir)?;
        }
        Ok(())
    }

    /// The latest `session-info` name in this session's own log (gh
    /// #37): `None` when nothing renamed it yet. Latest wins, like
    /// every appended marker.
    pub fn session_name(&self, session: &Session) -> Result<Option<String>> {
        let outcome = self.read(session)?;
        let mut name = None;
        for record in &outcome.records {
            if let lca_protocol::Record::SessionInfo { name: next, .. } = record {
                name = Some(next.clone());
            }
        }
        Ok(name)
    }

    /// Session metadata (`meta.json`).
    pub fn meta(&self, session: &Session) -> Result<SessionMeta> {
        let text = std::fs::read_to_string(session.meta_path())?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Record the model and provider last used on this session - the two
    /// fields `docs/session-log-format.md` promises `meta.json` carries
    /// (they were documented but never written). Called where they are
    /// known: the assistant record's persist. Only writes when a value
    /// changed, so a turn costs at most one small atomic write; an empty
    /// model (the zero-provider state) is not "last used" and is ignored.
    pub fn record_model_used(&self, session: &Session, provider: &str, model: &str) -> Result<()> {
        if model.is_empty() {
            return Ok(());
        }
        // gh #122: before materialize, the choice rides the pending
        // spec into the meta written at first append.
        if !session.meta_path().is_file()
            && let Some(spec) = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_mut(session.id())
        {
            spec.model = Some(model.to_string());
            spec.provider = Some(provider.to_string());
            return Ok(());
        }
        let mut meta = self.meta(session)?;
        if meta.model.as_deref() == Some(model) && meta.provider.as_deref() == Some(provider) {
            return Ok(());
        }
        meta.model = Some(model.to_string());
        meta.provider = Some(provider.to_string());
        write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)
    }

    /// The file holding `hash`, walking the fork chain from `session` back
    /// to the root. A fork copies records but not the content they
    /// reference, so a record's attachment lives in the home session that
    /// wrote it (`docs/session-log-format.md` § Fork).
    pub fn attachment_path(&self, session: &Session, hash: &str) -> Option<PathBuf> {
        for handle in self.ancestors(session).ok()? {
            let candidate = handle.dir().join("attachments").join(hash);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}
