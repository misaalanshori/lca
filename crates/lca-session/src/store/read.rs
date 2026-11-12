//! Reading the log (gh #564, FR-SESS-2/6/7): the raw read and its line decoder,
//! the ancestor walk a fork needs, and the listing cache rebuilt from the
//! directories.
//!
//! Every function here rests on the append-only assumption: a record read once
//! never changes, so `rebuild_index` may cache and `read` may stop at the first
//! corrupt line (FR-SESS-6). `docs/session-log-format.md` is that contract.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::Result;
use crate::view::ViewMode;

use super::lines::{last_id_in, project_dir_of, write_atomic};
use super::rows::{IndexFile, ReadOutcome, SessionSummary};
use super::{Session, SessionStore};

// The record variant is large by nature (it mirrors the log schema);
// boxing it would tax every parse for a private type's lint comfort.
#[allow(clippy::large_enum_variant)]
pub(super) enum Line {
    Record(lca_protocol::Record),
    Skipped { reason: String },
    Corrupt { reason: String },
}

pub(super) fn parse_line(line: &[u8]) -> Line {
    let text = match std::str::from_utf8(line) {
        Ok(text) => text,
        Err(err) => {
            return Line::Corrupt {
                reason: format!("invalid utf-8: {err}"),
            };
        }
    };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return Line::Corrupt {
                reason: format!("unparseable record: {err}"),
            };
        }
    };
    let Some(version) = value.get("v").and_then(serde_json::Value::as_u64) else {
        return Line::Corrupt {
            reason: "record has no version field".to_string(),
        };
    };
    if version > u64::from(lca_protocol::FORMAT_VERSION) {
        return Line::Skipped {
            reason: format!("record version {version} is newer than this reader"),
        };
    }
    // The type tag comes from the parsed value, not a fragile split of the
    // raw text: a known record that fails to deserialize is corruption the
    // reader must stop at (FR-SESS-6), while a genuinely unknown type is
    // skipped so a newer agent's log stays readable.
    let tag = value
        .get("t")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    match serde_json::from_value::<lca_protocol::Record>(value) {
        Ok(record) => Line::Record(record),
        Err(err) => {
            if KNOWN_TYPES.contains(&tag.as_str()) {
                Line::Corrupt {
                    reason: format!("known record type `{tag}` failed to parse: {err}"),
                }
            } else {
                Line::Skipped {
                    reason: format!("unknown record type `{tag}`"),
                }
            }
        }
    }
}

pub(super) const KNOWN_TYPES: &[&str] = &[
    "session-start",
    "user",
    "assistant",
    "tool-call",
    "tool-result",
    "permission",
    "extension-event",
    "compaction",
    "fork-point",
    "model-change",
    "thinking-level-change",
    "usage",
    "label",
    "session-info",
    "custom",
    "custom-message",
    "context-edit",
    "session-end",
    "branch-point",
    "branch-summary",
];

/// The record `t` tags the reader treats as known (gh #108): a
/// malformed line of one of these stops the reader as corruption
/// (FR-SESS-6); any other tag skips as a newer agent's record. The
/// vocabulary test pins this list against every tag the writer emits.
pub fn known_record_types() -> &'static [&'static str] {
    KNOWN_TYPES
}

impl SessionStore {
    /// Read this session's raw log: records until the first failure, then a
    /// truncation report (FR-SESS-6).
    pub fn read(&self, session: &Session) -> Result<ReadOutcome> {
        let path = session.log_path();
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let mut outcome = ReadOutcome::default();
        for line in reader.split(b'\n') {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            match parse_line(&line) {
                Line::Record(record) => outcome.records.push(record),
                Line::Skipped { reason } => {
                    outcome.skipped_unknown += 1;
                    outcome.warnings.push(reason);
                }
                Line::Corrupt { reason } => {
                    outcome.truncated = true;
                    outcome.warnings.push(reason);
                    break;
                }
            }
        }
        Ok(outcome)
    }

    /// The ancestor chain of `session`, nearest first: the session itself,
    /// then its parent, and so on to the root. A missing parent ends the
    /// chain (the reader reports truncation for that same case); a cycle is
    /// an error (`docs/session-log-format.md` § Fork).
    pub(super) fn ancestors(&self, session: &Session) -> Result<Vec<Session>> {
        let mut chain = vec![session.clone()];
        // gh #122: a session with no directory yet has no parents
        // (a missing dir means never materialized, never corrupt -
        // a present dir without meta still errors below).
        if !session.dir().is_dir() {
            return Ok(chain);
        }
        let mut visited = BTreeSet::new();
        visited.insert(session.id().to_string());
        let mut current = session.clone();
        while let Some(parent_id) = self.meta(&current)?.parent_session {
            if !visited.insert(parent_id.clone()) {
                return Err(crate::Error::ForkCycle { session: parent_id });
            }
            let dir = project_dir_of(&current)?.join(&parent_id);
            if !dir.is_dir() {
                break;
            }
            let parent = Session::new(parent_id, dir);
            chain.push(parent.clone());
            current = parent;
        }
        Ok(chain)
    }

    /// The last id-bearing record in this session's own log (gh #37):
    /// a short backward scan, so appends link without re-reading the
    /// whole file. A partial crash tail is skipped like the reader
    /// skips it; a missing log means a first write (`None`).
    pub(super) fn tip_id(&self, session: &Session) -> Result<Option<String>> {
        use std::io::{Read, Seek, SeekFrom};
        const TAIL: u64 = 1_048_576;
        let path = session.log_path();
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(crate::Error::Io(source));
            }
        };
        let len = file.metadata()?.len();
        let start = len.saturating_sub(TAIL);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        if let Some(id) = last_id_in(&buf) {
            return Ok(Some(id));
        }
        if start == 0 {
            return Ok(None);
        }
        // A single line over a megabyte (pathological: spills cap far
        // below it): correctness first, forward scan.
        let full = std::fs::read(&path)?;
        Ok(last_id_in(&full))
    }

    /// Read this session's records in the requested view.
    pub fn resolved(&self, session: &Session, mode: ViewMode) -> Result<ReadOutcome> {
        self.read_with(session, mode)
    }

    /// List this project's sessions, newest first (FR-SESS-2). Rebuilds
    /// `index.json` when it is missing or unreadable.
    /// List a project's sessions, newest first. Rebuilt from the session
    /// directories on every call: the index cache is only refreshed on
    /// create, rename, and gc, so trusting it showed a session's message
    /// count frozen at creation (`lca resume` said `0 messages` for a
    /// session that had grown). ponytail: O(sessions) directory scan; this
    /// is a human-facing listing, so add an mtime-guarded cache only if a
    /// huge project ever makes it slow.
    pub fn list_sessions(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
        self.rebuild_index(project_dir)
    }

    pub(super) fn rebuild_index(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
        self.rebuild_index_from_key(&self.project_dir(project_dir))
    }

    pub(super) fn rebuild_index_from_key(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();
        if project_dir.is_dir() {
            for entry in std::fs::read_dir(project_dir)? {
                let entry = entry?;
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                let handle = Session::new(id, dir);
                let Ok(meta) = self.meta(&handle) else {
                    continue;
                };
                let outcome = self.read(&handle).unwrap_or_default();
                // A fork's own log holds only its framing records; its
                // history lives in the ancestor chain, so count the resolved
                // view (FR-SESS-3), the same one `lca resume` shows.
                let resolved = self
                    .read_with(&handle, ViewMode::Display)
                    .unwrap_or_default();
                let message_count = resolved
                    .records
                    .iter()
                    .filter(|r| {
                        matches!(
                            r,
                            lca_protocol::Record::User { .. }
                                | lca_protocol::Record::Assistant { .. }
                                // gh #253: a compaction summary is resumable
                                // content — continuing the session appends
                                // after it. Counting only User/Assistant
                                // skipped compacted sessions with nothing
                                // after the compaction (count 0) even though
                                // their summary lists and resumes.
                                | lca_protocol::Record::Compaction { .. }
                        )
                    })
                    .count();
                // A session with no messages has nothing to resume. The
                // interface creates one at every launch, so listing them
                // buries real sessions under a wall of `0 messages` rows -
                // the manual side-by-side against pi's picker (which never
                // shows an empty session) is what surfaced it, 2026-10-01.
                // Hidden from the list only: `lca resume <id>` still opens
                // one directly (FR-SESS-2).
                if message_count == 0 {
                    continue;
                }
                let first_user = resolved.records.iter().find_map(|record| match record {
                    lca_protocol::Record::User { content, .. } => Some(content.clone()),
                    _ => None,
                });
                let modified_ms = outcome
                    .records
                    .iter()
                    .filter_map(|r| match r {
                        lca_protocol::Record::User { ts, .. }
                        | lca_protocol::Record::Assistant { ts, .. }
                        | lca_protocol::Record::ToolCall { ts, .. }
                        | lca_protocol::Record::ToolResult { ts, .. } => Some(*ts),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(meta.created_ms);
                sessions.push(SessionSummary {
                    id: handle.id.clone(),
                    title: meta.title,
                    created_ms: meta.created_ms,
                    modified_ms,
                    message_count,
                    parent_session: meta.parent_session,
                    first_user,
                });
            }
        }
        sessions.sort_by(|a, b| b.id.cmp(&a.id));
        let index = IndexFile {
            version: 1,
            sessions: sessions.clone(),
        };
        // The project directory may not exist yet - a first `lca resume` on
        // a project with no sessions must list an empty set, not fail with
        // ENOENT while writing the index cache (FR-SESS-2).
        std::fs::create_dir_all(project_dir)?;
        write_atomic(
            &project_dir.join("index.json"),
            &serde_json::to_vec_pretty(&index)?,
        )?;
        Ok(sessions)
    }
}
