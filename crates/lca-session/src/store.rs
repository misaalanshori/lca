//! The session store: layout on disk, append-only writes, listing, forking,
//! and export.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids;
use crate::view::ViewMode;
use crate::{ABI_VERSION, Result};

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
pub struct SessionSummary {
    /// Session identifier (its directory name).
    pub id: String,
    /// Title.
    pub title: String,
    /// Creation time, epoch milliseconds.
    pub created_ms: u64,
    /// Last modification, epoch milliseconds.
    pub modified_ms: u64,
    /// Number of user and assistant messages in this session's own log.
    pub message_count: usize,
    /// Parent session, when forked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    sessions: Vec<SessionSummary>,
}

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
}

impl SessionStore {
    /// Root the store at the user data directory (`.../sessions` lives below).
    pub fn new(root: PathBuf) -> Self {
        SessionStore { root }
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
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        ids::project_key(&canonical.to_string_lossy())
    }

    fn project_dir(&self, project_dir: &Path) -> PathBuf {
        self.sessions_dir().join(self.project_key(project_dir))
    }

    /// Create a session for `project_dir` and write its `session-start`.
    pub fn create_session(&self, project_dir: &Path, title: &str) -> Result<Session> {
        let now = ids::now_ms();
        let id = ids::session_id(now);
        let project = self.project_dir(project_dir);
        std::fs::create_dir_all(&project)?;
        let dir = project.join(&id);
        std::fs::create_dir_all(&dir)?;

        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let meta = SessionMeta {
            format_version: lca_protocol::FORMAT_VERSION,
            created_ms: now,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            model: None,
            provider: None,
            working_dir: canonical.to_string_lossy().into_owned(),
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
    pub fn append(&self, session: &Session, record: lca_protocol::Record) -> Result<()> {
        let line = serde_json::to_string(&record)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(session.log_path())?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    /// Write the clean-exit `session-end` marker (`docs/session-log-format.md`:
    /// its absence means the session ended without one, normal after a crash).
    /// The CLI calls this when a session ends.
    pub fn close(&self, session: &Session) -> Result<()> {
        let now = ids::now_ms();
        self.append(
            session,
            lca_protocol::Record::SessionEnd {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                id: ids::record_id(now),
            },
        )
    }

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
    fn ancestors(&self, session: &Session) -> Result<Vec<Session>> {
        let mut chain = vec![session.clone()];
        let mut visited = BTreeSet::new();
        visited.insert(session.id().to_string());
        let mut current = session.clone();
        while let Some(parent_id) = self.meta(&current)?.parent_session {
            if !visited.insert(parent_id.clone()) {
                return Err(crate::Error::ForkCycle { session: parent_id });
            }
            let dir = current
                .dir()
                .parent()
                .expect("session lives under a project dir")
                .join(&parent_id);
            if !dir.is_dir() {
                break;
            }
            let parent = Session::new(parent_id, dir);
            chain.push(parent.clone());
            current = parent;
        }
        Ok(chain)
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

    /// Every session sharing `session`'s fork root: the ancestors, the
    /// session, and every descendant in the project. The GC and the export
    /// both need the whole tree, because an attachment in one member's
    /// directory can be referenced by any member's resolved records.
    pub fn fork_tree(&self, session: &Session) -> Result<Vec<Session>> {
        let ancestors = self.ancestors(session)?;
        let root = ancestors
            .last()
            .expect("the session itself is in the chain");
        let project_dir = root
            .dir()
            .parent()
            .expect("session lives under a project dir")
            .to_path_buf();
        let summaries = self.rebuild_index_from_key(&project_dir)?;
        let mut tree = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = vec![root.id().to_string()];
        while let Some(id) = queue.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let dir = project_dir.join(&id);
            if !dir.is_dir() {
                continue;
            }
            for summary in &summaries {
                if summary.parent_session.as_deref() == Some(id.as_str()) {
                    queue.push(summary.id.clone());
                }
            }
            tree.push(Session::new(id, dir));
        }
        Ok(tree)
    }

    /// Delete attachment files that no resolved record list in `session`'s
    /// fork tree references (D5, `docs/session-log-format.md`). Returns the
    /// deleted hashes, sorted.
    ///
    /// The mark is the **display** view: a record a compaction replaced is
    /// gone from what a reader sees, so its attachment is an orphan. As the
    /// sweep touches every member's directory, asking for any member cleans
    /// the whole tree (the cascade the plan calls for when children exist).
    /// The tree's reachable set is computed first and deletion second, so
    /// the sweep never mistakes its own deletions for reachability.
    // ponytail: manual, whole-tree sweep; an automatic sweep at compaction
    // and fork-prune can replace the explicit command once disk use matters.
    pub fn gc(&self, session: &Session) -> Result<Vec<String>> {
        let tree = self.fork_tree(session)?;
        let mut reachable = BTreeSet::new();
        for member in &tree {
            let outcome = self.read_with(member, ViewMode::Display)?;
            collect_referenced(&outcome.records, &mut reachable);
        }
        let mut deleted = Vec::new();
        for member in &tree {
            let dir = member.dir().join("attachments");
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue; // no attachments directory: nothing to sweep
            };
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !reachable.contains(&name) {
                    std::fs::remove_file(entry.path())?;
                    deleted.push(name);
                }
            }
        }
        deleted.sort();
        Ok(deleted)
    }

    /// Read this session's records in the requested view.
    pub fn resolved(&self, session: &Session, mode: ViewMode) -> Result<ReadOutcome> {
        self.read_with(session, mode)
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

    /// Rename a session: `meta.json` atomically, then the index cache.
    pub fn rename(&self, session: &Session, title: &str) -> Result<()> {
        let mut meta = self.meta(session)?;
        meta.title = title.to_string();
        write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)?;
        if let Some(project_dir) = session.dir().parent() {
            self.rebuild_index_from_key(project_dir)?;
        }
        Ok(())
    }

    /// Session metadata (`meta.json`).
    pub fn meta(&self, session: &Session) -> Result<SessionMeta> {
        let text = std::fs::read_to_string(session.meta_path())?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Fork at `record_id`: a new session with `session-start` and
    /// `fork-point`; the parent's log is untouched (FR-SESS-3).
    pub fn fork(&self, parent: &Session, record_id: &str) -> Result<Session> {
        let outcome = self.read(parent)?;
        if !outcome.records.iter().any(|r| r.id() == Some(record_id)) {
            return Err(crate::Error::ForkPointMissing {
                session: parent.id().to_string(),
                record: record_id.to_string(),
            });
        }
        let now = ids::now_ms();
        let id = ids::session_id(now);
        let project_dir = parent.dir().parent().expect("project dir");
        let dir = project_dir.join(&id);
        std::fs::create_dir_all(&dir)?;

        let mut meta = self.meta(parent)?;
        meta.created_ms = now;
        meta.parent_session = Some(parent.id().to_string());
        meta.parent_record = Some(record_id.to_string());
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
        self.append(
            &session,
            lca_protocol::Record::ForkPoint {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                id: ids::record_id(now),
                parent_session: parent.id().to_string(),
                record_id: record_id.to_string(),
            },
        )?;
        // The parent's project entry now holds a child; refresh the cache.
        let project = project_dir.to_path_buf();
        self.rebuild_index_from_key(&project)?;
        Ok(session)
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

    fn rebuild_index(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
        self.rebuild_index_from_key(&self.project_dir(project_dir))
    }

    fn rebuild_index_from_key(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
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
                        )
                    })
                    .count();
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

    /// The export's `attachments` map: every hash a record references,
    /// mapped to its sidecar path. A fork's resolved records reference the
    /// ancestor's files, so the search walks the fork chain and the path is
    /// relative to the export file: `attachments/<hash>` when the session
    /// owns the file, `../<owner>/attachments/<hash>` when an ancestor does.
    /// Only files that exist are listed; the format allows inline base64
    /// too, but the sidecar keeps large tool output out of the JSON
    /// (`docs/session-log-format.md`).
    fn attachment_map(
        &self,
        session: &Session,
        records: &[lca_protocol::Record],
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        for record in records {
            let hashes: Vec<&String> = match record {
                lca_protocol::Record::User { attachments, .. } => attachments.iter().collect(),
                lca_protocol::Record::ToolResult {
                    attachment: Some(hash),
                    ..
                } => vec![hash],
                _ => Vec::new(),
            };
            for hash in hashes {
                if map.contains_key(hash) {
                    continue;
                }
                let Some(path) = self.attachment_path(session, hash) else {
                    continue;
                };
                let own = session.dir().join("attachments").join(hash);
                let value = if path == own {
                    format!("attachments/{hash}")
                } else {
                    let owner = path
                        .parent()
                        .and_then(Path::parent)
                        .and_then(Path::file_name)
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    format!("../{owner}/attachments/{hash}")
                };
                map.insert(hash.clone(), serde_json::Value::String(value));
            }
        }
        map
    }

    /// Export the resolved record list with metadata
    /// (`docs/session-log-format.md`); `permission` and `extension-event`
    /// records are stripped unless `audit` is set (FR-SESS-7).
    pub fn export(&self, session: &Session, options: ExportOptions) -> Result<PathBuf> {
        let resolved = self.read_with(session, ViewMode::Audit)?;
        let records: Vec<_> = if options.audit {
            resolved.records
        } else {
            resolved
                .records
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r,
                        lca_protocol::Record::Permission { .. }
                            | lca_protocol::Record::ExtensionEvent { .. }
                    )
                })
                .collect()
        };
        let meta = self.meta(session)?;
        let attachments = self.attachment_map(session, &records);
        let document = serde_json::json!({
            "meta": meta,
            "records": records,
            "attachments": attachments,
        });
        let out = session.dir().join(if options.audit {
            "export-audit.json"
        } else {
            "export.json"
        });
        write_atomic(&out, &serde_json::to_vec_pretty(&document)?)?;
        Ok(out)
    }

    /// The session's own `session-start` record, for tests that need the
    /// exact stored form.
    pub fn raw_start(&self, session: &Session) -> Result<lca_protocol::Record> {
        Ok(self
            .read(session)?
            .records
            .into_iter()
            .next()
            .expect("session-start is always the first record"))
    }
}

// The record variant is large by nature (it mirrors the log schema);
// boxing it would tax every parse for a private type's lint comfort.
#[allow(clippy::large_enum_variant)]
enum Line {
    Record(lca_protocol::Record),
    Skipped { reason: String },
    Corrupt { reason: String },
}

fn parse_line(line: &[u8]) -> Line {
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

const KNOWN_TYPES: &[&str] = &[
    "session-start",
    "user",
    "assistant",
    "tool-call",
    "tool-result",
    "permission",
    "extension-event",
    "compaction",
    "fork-point",
    "session-end",
];

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// Every attachment hash a resolved record list references: the GC's mark
/// set. A `user` message carries a list, a `tool-result` carries at most one.
fn collect_referenced(records: &[lca_protocol::Record], out: &mut BTreeSet<String>) {
    for record in records {
        match record {
            lca_protocol::Record::User { attachments, .. } => {
                out.extend(attachments.iter().cloned());
            }
            lca_protocol::Record::ToolResult {
                attachment: Some(hash),
                ..
            } => {
                out.insert(hash.clone());
            }
            _ => {}
        }
    }
}
