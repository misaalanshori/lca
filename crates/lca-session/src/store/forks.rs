//! The tree operations (gh #564): fork, clone, branch, the summary a branch
//! carries, and the two cleanups (`trash`, `gc`).
//!
//! One log per session plus a `fork-point` / `branch-point` record is the whole
//! shape; `resolved` and the ancestor walk are in `read.rs`. The stored form is
//! `docs/session-log-format.md`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::ids;
use crate::view::ViewMode;
use crate::{ABI_VERSION, Result};

use super::lines::{project_dir_of, write_atomic};
use super::{Session, SessionStore};

impl SessionStore {
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
    /// Move a session directory under a timestamped trash directory
    /// (gh #75): recoverable by moving it back, never `rm`. The
    /// project index rebuilds without it on the next listing.
    pub fn trash(&self, session: &Session) -> Result<PathBuf> {
        if !session.dir().is_dir() {
            return Err(crate::Error::UnknownSession {
                id: session.id().to_string(),
            });
        }
        let now = ids::now_ms();
        let target = self
            .root
            .join("trash")
            .join(format!("{}-{now}", session.id()));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(session.dir(), &target)?;
        Ok(target)
    }

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
            crate::attachments::referenced_attachments(
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
}
