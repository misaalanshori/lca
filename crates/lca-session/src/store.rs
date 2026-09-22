//! The session store: layout on disk, append-only writes, listing, forking,
//! and export.

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

    /// Read this session's records in the requested view.
    pub fn resolved(&self, session: &Session, mode: ViewMode) -> Result<ReadOutcome> {
        self.read_with(session, mode)
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
    pub fn list_sessions(&self, project_dir: &Path) -> Result<Vec<SessionSummary>> {
        if let Ok(text) = std::fs::read_to_string(self.index_path(project_dir))
            && let Ok(index) = serde_json::from_str::<IndexFile>(&text)
            && index.version == 1
        {
            return Ok(index.sessions);
        }
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
                let message_count = outcome
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
        if let Some(parent) = project_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(
            &project_dir.join("index.json"),
            &serde_json::to_vec_pretty(&index)?,
        )?;
        Ok(sessions)
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
        let document = serde_json::json!({
            "meta": meta,
            "records": records,
            "attachments": serde_json::Map::new(),
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

    /// Raw records of this session only (no fork chain).
    pub(crate) fn raw(&self, session: &Session) -> Result<Vec<lca_protocol::Record>> {
        Ok(self.read(session)?.records)
    }
}

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
    match serde_json::from_value::<lca_protocol::Record>(value) {
        Ok(record) => Line::Record(record),
        Err(err) => {
            let tag = text.split('"').nth(3).unwrap_or("").to_string();
            if KNOWN_TYPES.contains(&tag.as_str()) {
                Line::Corrupt {
                    reason: format!("known record type failed to parse: {err}"),
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
