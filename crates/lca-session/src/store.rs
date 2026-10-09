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
struct IndexFile {
    version: u32,
    sessions: Vec<SessionSummary>,
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

/// The newest record id in a log buffer (gh #37): newest line
/// first, skipping blanks and a partial crash tail. Empty when no
/// complete line names one.
fn last_id_in(buf: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buf);
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
            return Some(id.to_string());
        }
    }
    None
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
        // Identity keys on the canonical path - verbatim included - so two
        // spellings of one directory are one project (ADR-0006). Only the
        // *stored display* form is stripped (R7b, `display_path`).
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        ids::project_key(&canonical.to_string_lossy())
    }

    fn project_dir(&self, project_dir: &Path) -> PathBuf {
        self.sessions_dir().join(self.project_key(project_dir))
    }

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
    pub fn append(&self, session: &Session, mut record: lca_protocol::Record) -> Result<()> {
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
                parent: None,
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
        let Some(root) = ancestors.last() else {
            return Err(crate::Error::Io(std::io::Error::other(
                "the fork chain is empty",
            )));
        };
        let project_dir = project_dir_of(root)?.to_path_buf();
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
            // The full file minus compacted-away records (gh #37): an
            // abandoned branch's attachments are still in the log and
            // must survive, while a compaction orphan is truly gone.
            let outcome = self.read_with(member, ViewMode::Audit)?;
            collect_referenced(
                &crate::view::reachable_records(outcome.records),
                &mut reachable,
            );
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
        let mut meta = self.meta(session)?;
        meta.title = title.to_string();
        write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)?;
        if let Some(project_dir) = session.dir().parent() {
            self.rebuild_index_from_key(project_dir)?;
        }
        Ok(())
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
        let mut meta = self.meta(session)?;
        if meta.model.as_deref() == Some(model) && meta.provider.as_deref() == Some(provider) {
            return Ok(());
        }
        meta.model = Some(model.to_string());
        meta.provider = Some(provider.to_string());
        write_atomic(&session.meta_path(), &serde_json::to_vec_pretty(&meta)?)
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
        let project_dir = project_dir_of(parent)?;
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

    /// Clone at the tip (gh #205): fork at the parent's latest record
    /// and title the child, defaulting to `Clone of <parent-title>`.
    /// Meta (model, provider) rides the fork, so settings inherit; the
    /// parent's log is untouched, as with any fork.
    pub fn clone_session(&self, parent: &Session, title: Option<&str>) -> Result<Session> {
        let outcome = self.read(parent)?;
        let Some(tip) = outcome.records.iter().rev().find_map(|record| record.id()) else {
            return Err(crate::Error::ForkPointMissing {
                session: parent.id().to_string(),
                record: "tip".to_string(),
            });
        };
        let tip = tip.to_string();
        let session = self.fork(parent, &tip)?;
        let title = match title.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => name.to_string(),
            None => format!("Clone of {}", self.meta(parent)?.title),
        };
        self.rename(&session, &title)?;
        Ok(session)
    }

    /// The last id-bearing record in this session's own log (gh #37):
    /// a short backward scan, so appends link without re-reading the
    /// whole file. A partial crash tail is skipped like the reader
    /// skips it; a missing log means a first write (`None`).
    fn tip_id(&self, session: &Session) -> Result<Option<String>> {
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

    /// Navigate to `record_id` and continue in this same log (gh #37,
    /// FR-SESS-11): appends a `branch-point` naming it. Later appends
    /// chain through the jump, so the live view and the model see the
    /// target's ancestry plus the new records - no new session
    /// directory, the parent's log untouched otherwise.
    pub fn branch_at(&self, session: &Session, record_id: &str) -> Result<lca_protocol::Record> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        if !resolved.records.iter().any(|r| r.id() == Some(record_id)) {
            return Err(crate::Error::BranchTargetMissing {
                session: session.id().to_string(),
                record: record_id.to_string(),
            });
        }
        let now = ids::now_ms();
        let record = lca_protocol::branch_point_record(now, ids::record_id(now), record_id);
        self.append(session, record.clone())?;
        Ok(record)
    }

    /// Navigate like [`SessionStore::branch_at`] and record what the
    /// abandoned path learned (gh #37, FR-SESS-11, pi's
    /// `branchWithSummary`): the summary's parent names the navigation
    /// point explicitly and `from_id` the abandoned tip, so assembly
    /// injects it where the new branch continues.
    pub fn summarize_branch(
        &self,
        session: &Session,
        record_id: &str,
        summary: &str,
    ) -> Result<lca_protocol::Record> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        if !resolved.records.iter().any(|r| r.id() == Some(record_id)) {
            return Err(crate::Error::BranchTargetMissing {
                session: session.id().to_string(),
                record: record_id.to_string(),
            });
        }
        let from = resolved.records.iter().rev().find_map(|r| r.id());
        self.branch_at(session, record_id)?;
        let now = ids::now_ms();
        let record =
            lca_protocol::branch_summary_record(now, ids::record_id(now), record_id, from, summary);
        self.append(session, record.clone())?;
        Ok(record)
    }

    /// Bookmark `record_id` as `label` (gh #37, ADR-0046): appends a
    /// `label` record to the live session's own log. `None` clears the
    /// bookmark. The target is resolved through the fork chain, so a
    /// fork can bookmark (and see) its ancestors' records (FR-SESS-10).
    pub fn set_label(&self, session: &Session, record_id: &str, label: Option<&str>) -> Result<()> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        if !resolved.records.iter().any(|r| r.id() == Some(record_id)) {
            return Err(crate::Error::LabelTargetMissing {
                session: session.id().to_string(),
                record: record_id.to_string(),
            });
        }
        let now = ids::now_ms();
        self.append(
            session,
            lca_protocol::Record::Label {
                v: lca_protocol::FORMAT_VERSION,
                ts: now,
                id: ids::record_id(now),
                parent: None,
                target_id: record_id.to_string(),
                label: label.map(str::to_string),
            },
        )
    }

    /// Every live bookmark as `record id -> label` (gh #37): walks the
    /// resolved history latest-wins, so a clear (`None`) erases an
    /// earlier name and a fork sees its ancestors' marks (FR-SESS-10).
    pub fn labels(&self, session: &Session) -> Result<std::collections::BTreeMap<String, String>> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        let mut marks = std::collections::BTreeMap::new();
        for record in &resolved.records {
            if let lca_protocol::Record::Label {
                target_id, label, ..
            } = record
            {
                match label {
                    Some(name) => {
                        marks.insert(target_id.clone(), name.clone());
                    }
                    None => {
                        marks.remove(target_id);
                    }
                }
            }
        }
        Ok(marks)
    }

    /// The record a bookmark name points at (gh #37): latest set wins
    /// across the resolved history; `None` when no live bookmark
    /// carries the name (FR-SESS-10).
    pub fn resolve_label(&self, session: &Session, name: &str) -> Result<Option<String>> {
        let resolved = self.resolved(session, ViewMode::Audit)?;
        // Latest set wins per name; a clear erases only names pointing
        // at its own target, so a sibling keeping the name resolves.
        let mut by_name = std::collections::BTreeMap::new();
        for record in &resolved.records {
            if let lca_protocol::Record::Label {
                target_id, label, ..
            } = record
            {
                match label {
                    Some(mark) => {
                        by_name.insert(mark.clone(), target_id.clone());
                    }
                    None => {
                        by_name.retain(|_, held| held != target_id);
                    }
                }
            }
        }
        Ok(by_name.remove(name))
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
    /// (`docs/session-log-format.md`); `permission`, `extension-event`,
    /// `usage`, and `custom` records are stripped unless `audit` is set
    /// (FR-SESS-7).
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
                            | lca_protocol::Record::Usage { .. }
                            | lca_protocol::Record::Custom { .. }
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
        let Some(record) = self.read(session)?.records.into_iter().next() else {
            return Err(crate::Error::Io(std::io::Error::other(
                "the session log has no records",
            )));
        };
        Ok(record)
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

/// The project directory a session lives under. A session directory is
/// always `<data>/sessions/<project>/<id>`, so its parent is the project
/// key; a session whose directory has no parent is a broken handle, reported
/// as an I/O error rather than a panic.
pub(crate) fn project_dir_of(session: &Session) -> Result<&Path> {
    session.dir().parent().ok_or_else(|| {
        crate::Error::Io(std::io::Error::other(format!(
            "session directory {} has no project parent",
            session.dir().display()
        )))
    })
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
    "model-change",
    "thinking-level-change",
    "usage",
    "label",
    "session-info",
    "custom",
    "custom-message",
    "context-edit",
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
