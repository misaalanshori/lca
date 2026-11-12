//! Writing the log (gh #564): session creation, the one append path, the
//! clean-exit marker, and the import / `raw_start` helpers.
//!
//! **`append` writes one line and flushes it** - `docs/session-log-format.md`.
//! A record is never rewritten in place; compaction appends a marker that names
//! the range it replaces. If you touch this file, that is the rule you are
//! touching.

use std::io::Write;
use std::path::Path;

use crate::ids;
use crate::{ABI_VERSION, Result};

use super::lines::write_atomic;
use super::rows::{SessionMeta, display_path};
use super::{Session, SessionStore};

impl SessionStore {
    /// Create a session for `project_dir` and write its `session-start`.
    pub fn create_session(&self, project_dir: &Path, title: &str) -> Result<Session> {
        self.create_session_with_id(project_dir, &ids::session_id(ids::now_ms()), title)
    }

    /// Create a session with a chosen id (gh #69: `--session-id`
    /// creates it when absent). The id validates first, so it can
    /// never escape its project directory.
    pub fn create_session_with_id(
        &self,
        project_dir: &Path,
        id: &str,
        title: &str,
    ) -> Result<Session> {
        if !crate::valid_session_id(id) {
            return Err(crate::Error::InvalidId { id: id.to_string() });
        }
        let now = ids::now_ms();
        let id = id.to_string();
        let project = self.project_dir(project_dir);
        std::fs::create_dir_all(&project)?;
        let dir = project.join(&id);
        std::fs::create_dir_all(&dir)?;

        // R7b: identity and project keys use the canonical (possibly
        // verbatim) path; what lands in `meta.json` and the session-start
        // record is the display form. A `\\?\C:\Users\me` in the stored
        // `working_dir` is what the owner's log showed, and it is a
        // Windows implementation detail, not a path a human or a model
        // should ever read.
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let display = display_path(&canonical);
        let meta = SessionMeta {
            format_version: lca_protocol::FORMAT_VERSION,
            created_ms: now,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            model: None,
            provider: None,
            working_dir: display.clone(),
            title: title.to_string(),
            parent_session: None,
            parent_record: None,
        };
        write_atomic(&dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;

        let session = Session::new(id, dir);
        self.append(
            &session,
            lca_protocol::Record::SessionStart {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                abi_version: ABI_VERSION.to_string(),
                working_dir: meta.working_dir.clone(),
            },
        )?;
        self.rebuild_index(project_dir)?;
        Ok(session)
    }

    /// Append one record: a single write of the full line, then flush.
    /// A pending session materializes first (gh #122): the directory,
    /// meta, and session-start land ahead of the record.
    pub fn append(&self, session: &Session, mut record: lca_protocol::Record) -> Result<()> {
        self.materialize(session)?;
        // gh #37 (FR-SESS-11): stamp the ancestry link from the log tip
        // when the writer left it empty. Junction types (`BranchPoint`
        // and friends) ignore the stamp; the walk never follows theirs.
        // The tip is re-read per append (a short backward scan), so a
        // second process appending the same session still links truly.
        if record.parent().is_none()
            && let Some(tip) = self.tip_id(session)?
        {
            record.set_parent(&tip);
        }
        let line = serde_json::to_string(&record)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(session.log_path())?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        // Gh #258: the write (type + id + bytes, never content).
        crate::debug::trace_append(&record, line.len());
        Ok(())
    }

    /// Write the clean-exit `session-end` marker (`docs/session-log-format.md`:
    /// its absence means the session ended without one, normal after a crash).
    /// The CLI calls this when a session ends. A session that never
    /// materialized has no records to end (gh #122): skip it, so
    /// launch-and-quit leaves no directory.
    pub fn close(&self, session: &Session) -> Result<()> {
        if !session.log_path().is_file() {
            return Ok(());
        }
        let now = ids::now_ms();
        self.append(
            session,
            lca_protocol::Record::SessionEnd {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                id: ids::record_id(now),
                parent: None,
            },
        )
    }

    /// Parse an import file's records in either accepted shape (gh #255):
    /// Parse an import file's records in either accepted shape (gh #255):
    /// the export document (`{"records": [...]}`) or raw JSONL lines.
    /// Returns the records plus the position word error messages use
    /// (`"line"` for JSONL, `"record"` for the document, whose pretty
    /// printing makes line numbers meaningless). Detection is strict:
    /// the whole file must parse as an object with a `records` array to
    /// take the document path, else every line must parse as a record —
    /// a half-document shape fails loudly rather than half-importing.
    fn parse_import_records(
        lines: &[&str],
    ) -> std::result::Result<(Vec<lca_protocol::Record>, &'static str), String> {
        let joined = lines.join("\n");
        if let Ok(document) = serde_json::from_str::<serde_json::Value>(&joined)
            && let Some(array) = document.get("records").and_then(|value| value.as_array())
        {
            let mut records = Vec::with_capacity(array.len());
            for (index, entry) in array.iter().enumerate() {
                match serde_json::from_value::<lca_protocol::Record>(entry.clone()) {
                    Ok(record) => records.push(record),
                    Err(err) => {
                        return Err(format!("record {}: {err}", index + 1));
                    }
                }
            }
            return Ok((records, "record"));
        }
        let mut records = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            serde_json::from_str::<lca_protocol::Record>(line)
                .map(|record| records.push(record))
                .map_err(|err| format!("line {}: {err}", index + 1))?;
        }
        Ok((records, "line"))
    }

    /// Import sessions from a file (gh #57's `/import`, gh #255): raw
    /// JSONL lines (one record per line, the log's own shape) or the
    /// export document this store writes. Every record parses before
    /// anything is created, so a corrupt entry aborts with its
    /// position and no half-imported session exists. The first
    /// record must open the session (`session-start`).
    ///
    /// Attachments travel as path references, not bytes, so hashes
    /// the new session cannot resolve stay dangling exactly like a
    /// raw-JSONL import of the same records; restoring bytes is out
    /// of scope.
    pub fn import_jsonl(
        &self,
        project_dir: &Path,
        title: &str,
        lines: &[&str],
    ) -> std::result::Result<Session, String> {
        let (records, shape) = Self::parse_import_records(lines)?;
        match records.first() {
            Some(lca_protocol::Record::SessionStart { .. }) => {}
            _ => {
                return Err(format!("{shape} 1: a session must open with session-start"));
            }
        }
        let session = self
            .create_session(project_dir, title)
            .map_err(|err| format!("cannot create the session: {err}"))?;
        // The created session stamps its own start; the file's opener
        // only proved the shape.
        for record in records.into_iter().skip(1) {
            self.append(&session, record)
                .map_err(|err| format!("cannot write the session: {err}"))?;
        }
        Ok(session)
    }

    /// The session's own `session-start` record, for tests that need the
    /// exact stored form.
    pub fn raw_start(&self, session: &Session) -> Result<lca_protocol::Record> {
        let Some(record) = self.read(session)?.records.into_iter().next() else {
            return Err(crate::Error::Io(std::io::Error::other(
                "the session log has no records",
            )));
        };
        Ok(record)
    }
}
