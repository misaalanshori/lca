//! Forward session migration (gh #98): version detection, linkage
//! backfill with backup, and lossless verification. Split from
//! `store.rs` under the workspace's 1,200-line file ceiling.
//!
//! The one rewrite this performs is linkage backfill: pre-linkage
//! records (no `parent`, written before gh #37) chain to their log
//! predecessor, so the entry tree and the ancestry walk see them.
//! Everything else - unknown-future lines, junction records, already
//! linked records - round-trips byte-identical. What cannot be
//! verified (a meta/header mismatch, a truncated log) refuses with
//! a message instead of rewriting. Fork-chain collapse stays a
//! documented non-goal (`docs/session-log-format.md` § Migration
//! note): this migrates one log, and says so when the session has
//! fork relatives.

use super::store::{Session, SessionStore};
use super::view::ViewMode;

/// A log's version state (gh #98): every stamp the format carries,
/// read without rewriting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionState {
    /// `meta.json`'s version (`None` when the meta is unreadable).
    pub meta_version: Option<u32>,
    /// The `session-start` record's version (`None` when missing).
    pub header_version: Option<u32>,
    /// The highest per-record `v` in the log.
    pub max_record_version: u32,
    /// Linkable records without parents that have an id-predecessor
    /// (the backfill set; a leading record stays parentless - there
    /// is no id to point it at, exactly as `append` leaves it).
    pub parentless: usize,
    /// Records newer than this reader (kept verbatim, warned).
    pub unknown_future: usize,
    /// Whether the read stopped early (unverifiable tail).
    pub truncated: bool,
    /// Whether the stamps disagree (corruption signal, never picked).
    pub mismatched: bool,
}

impl VersionState {
    /// Nothing to do: stamps agree and every linkable record links.
    /// Future records do not block currentness - they keep and warn.
    pub fn is_current(&self) -> bool {
        !self.mismatched && !self.truncated && self.parentless == 0
    }
}

/// What a migration did (gh #98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateReport {
    /// Linkage stamps written.
    pub stamped: usize,
    /// True when the log was already current (nothing written).
    pub already_current: bool,
    /// Where the original log survives (`None` on a no-op).
    pub backup: Option<std::path::PathBuf>,
    /// Non-fatal notes (kept future records, fork relatives).
    pub warnings: Vec<String>,
}

impl SessionStore {
    /// Read a session's version stamps without rewriting (gh #98).
    /// Value-level throughout: no record-enum instantiation, so the
    /// reader's size budget never notices this (NFR-1).
    /// Never inlined: a cold one-shot path must not drag
    /// its callees into every caller (NFR-1).
    #[inline(never)]
    pub fn version_state(&self, session: &Session) -> super::Result<VersionState> {
        let meta_version = self.meta(session).ok().map(|meta| meta.format_version);
        let outcome = self.read(session)?;
        let mut header_version = None;
        let mut max_record_version = 0;
        let mut parentless = 0;
        let mut unknown_future = 0;
        let mut predecessor: Option<String> = None;
        let text = std::fs::read_to_string(session.log_path()).unwrap_or_default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let version = value.get("v").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            max_record_version = max_record_version.max(version);
            if version > lca_protocol::FORMAT_VERSION {
                unknown_future += 1;
                continue;
            }
            let tag = value.get("t").and_then(|t| t.as_str()).unwrap_or("");
            if tag == "session-start" && header_version.is_none() {
                header_version = value.get("v").and_then(|v| v.as_u64()).map(|v| v as u32);
            }
            if value.get("parent").is_none() && is_linkable_tag(tag) {
                // A leading record has no id to point at (append
                // leaves it parentless too); only a record with an
                // id-predecessor wants a stamp.
                if predecessor.is_some() {
                    parentless += 1;
                }
            }
            if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                predecessor = Some(id.to_string());
            }
        }
        let mismatched = match (meta_version, header_version) {
            (Some(meta), Some(header)) => meta != header,
            _ => false,
        };
        Ok(VersionState {
            meta_version,
            header_version,
            max_record_version,
            parentless,
            unknown_future,
            truncated: outcome.truncated,
            mismatched,
        })
    }

    /// Forward-migrate one session log (gh #98): linkage backfill
    /// with backup and lossless verification. Refuses what cannot be
    /// verified (a stamp mismatch, a truncated log) with a message.
    /// Never inlined: a cold one-shot path must not drag
    /// its callees into every caller (NFR-1).
    #[inline(never)]
    pub fn migrate(&self, session: &Session) -> super::Result<MigrateReport> {
        let state = self.version_state(session)?;
        if state.mismatched {
            return Err(super::Error::CannotMigrate {
                session: session.id().to_string(),
                reason: format!(
                    "version mismatch: meta.json says v{} but session-start says v{}; pick the true version by hand first",
                    state.meta_version.unwrap_or(0),
                    state.header_version.unwrap_or(0),
                ),
            });
        }
        if state.truncated {
            return Err(super::Error::CannotMigrate {
                session: session.id().to_string(),
                reason: "the log is truncated past a corrupt line: repair it first".to_string(),
            });
        }
        if state.parentless == 0 {
            let mut warnings = self.read(session)?.warnings;
            if state.unknown_future > 0 {
                warnings.push(format!(
                    "{} future record{} kept as-is",
                    state.unknown_future,
                    if state.unknown_future == 1 { "" } else { "s" }
                ));
            }
            warnings.extend(self.fork_note(session));
            return Ok(MigrateReport {
                stamped: 0,
                already_current: true,
                backup: None,
                warnings,
            });
        }
        // Transcript before: the lossless check compares it after
        // (the audit-view check rides the raw contents below:
        // ancestors never rewrite, so the own log covers it).
        let before_display = self.view_ids(session, ViewMode::Display)?;
        let before_values = self.audit_values(session)?;

        let backup = self.backup_log(session)?;
        let text = std::fs::read_to_string(session.log_path())?;
        let (out, stamped) = stamp_log(&text);
        std::fs::write(session.log_path(), out)?;

        // Lossless verification: the same transcript, the same
        // contents modulo the stamped links.
        let after_display = self.view_ids(session, ViewMode::Display)?;
        let after_values = self.audit_values(session)?;
        if before_display != after_display {
            return Err(super::Error::CannotMigrate {
                session: session.id().to_string(),
                reason: "verification failed: transcript changed; the backup holds the original"
                    .to_string(),
            });
        }
        if before_values != after_values {
            return Err(super::Error::CannotMigrate {
                session: session.id().to_string(),
                reason: "verification failed: contents changed; the backup holds the original"
                    .to_string(),
            });
        }

        let mut warnings = self.read(session)?.warnings;
        if state.unknown_future > 0 {
            warnings.push(format!(
                "{} future record{} kept verbatim",
                state.unknown_future,
                if state.unknown_future == 1 { "" } else { "s" }
            ));
        }
        warnings.extend(self.fork_note(session));
        Ok(MigrateReport {
            stamped,
            already_current: false,
            backup: Some(backup),
            warnings,
        })
    }

    /// Record ids in a resolved view (the lossless check's shape).
    #[inline(never)]
    fn view_ids(&self, session: &Session, mode: ViewMode) -> super::Result<Vec<String>> {
        Ok(self
            .resolved(session, mode)?
            .records
            .iter()
            .filter_map(|record| record.id().map(str::to_string))
            .collect())
    }

    /// Audit contents minus `parent` (the one field migration
    /// stamps), read as values so unknown fields compare too.
    #[inline(never)]
    fn audit_values(&self, session: &Session) -> super::Result<Vec<serde_json::Value>> {
        let text = std::fs::read_to_string(session.log_path())?;
        let mut values = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            // The caller refused truncated logs, so every line parses.
            let mut value: serde_json::Value =
                serde_json::from_str(line).map_err(|err| super::Error::CannotMigrate {
                    session: session.id().to_string(),
                    reason: format!("verification failed: unreadable line: {err}"),
                })?;
            if let Some(object) = value.as_object_mut() {
                object.remove("parent");
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Copy the log into a backup directory (the format's rule: the
    /// original survives until the user removes it).
    #[inline(never)]
    fn backup_log(&self, session: &Session) -> super::Result<std::path::PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        let dir = session
            .dir()
            .join("backups")
            .join(format!("migrate-{stamp}"));
        std::fs::create_dir_all(&dir)?;
        let backup = dir.join("log.jsonl");
        std::fs::copy(session.log_path(), &backup)?;
        Ok(backup)
    }

    /// The fork-collapse deferral note (gh #98): this migrates one
    /// log; collapsing a fork tree stays designed-not-built.
    #[inline(never)]
    fn fork_note(&self, session: &Session) -> Vec<String> {
        let relatives = self.fork_tree(session).map(|tree| tree.len()).unwrap_or(1);
        if relatives > 1 {
            vec!["this session has fork relatives: one log migrated, tree collapse stays designed-not-built (docs/session-log-format.md)".to_string()]
        } else {
            Vec::new()
        }
    }
}

/// Whether a record type takes an ancestry link (gh #98): junction
/// records never do - the walk jumps through them. Tag-level, so the
/// value path never instantiates the record enum (NFR-1).
fn is_linkable_tag(tag: &str) -> bool {
    !matches!(tag, "session-start" | "fork-point" | "branch-point")
}

/// Chain parentless linkable lines to their id-predecessor (gh #98):
/// pure string transform, infallible, so the caller keeps the only
/// error paths. Only stamped lines change, and only by surgery on
/// the parsed value (unknown fields survive); everything else rides
/// through byte-identical.
fn stamp_log(text: &str) -> (String, usize) {
    let mut stamped = 0;
    let mut previous: Option<String> = None;
    let mut out = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut rewritten = line.to_string();
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) {
            let version = value.get("v").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let tag = value.get("t").and_then(|t| t.as_str()).unwrap_or("");
            let linkable = version <= lca_protocol::FORMAT_VERSION
                && value.get("parent").is_none()
                && is_linkable_tag(tag);
            if linkable
                && let Some(prev) = previous.clone()
                && value.is_object()
            {
                value["parent"] = serde_json::Value::String(prev);
                rewritten = serde_json::to_string(&value).unwrap_or_else(|_| line.to_string());
                stamped += 1;
            }
            if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                previous = Some(id.to_string());
            }
        }
        out.push_str(&rewritten);
        out.push('\n');
    }
    (out, stamped)
}
