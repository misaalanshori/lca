//! Label and record-id fork hooks (gh #37): bookmarking, listing,
//! and fork-at-record for `/label`, `/labels`, `/jump`. Split from
//! `hooks.rs` under the workspace's 1,200-line file ceiling. The shared
//! `fork_report` tail lives here so both fork hooks word alike.

use std::sync::Arc;

use lca_protocol::Record;
use lca_session::ViewMode;

use super::Ui;

impl Ui {
    /// Fork at a bookmark's record id (gh #37): `/jump` resolves the
    /// name, then branches exactly where the mark points.
    pub(super) fn fork_record(&self) -> lca_ui::state::ForkAtRecord {
        use lca_ui::state::ForkReport;
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |record_id: &str| -> ForkReport {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            fork_report(
                store.as_ref(),
                &session,
                record_id,
                &format!("record {record_id}"),
            )
        })
    }

    /// Bookmark the nth user message under a name (gh #37, FR-SESS-10).
    pub(super) fn set_label(&self) -> lca_ui::state::SetLabel {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |index: usize, name: &str| -> Result<String, String> {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let outcome = store
                .read_with(&session, ViewMode::Display)
                .map_err(|err| format!("cannot read the session: {err}"))?;
            let record_id = outcome
                .records
                .iter()
                .filter(|record| matches!(record, Record::User { .. }))
                .nth(index)
                .and_then(|record| record.id().map(str::to_string))
                .ok_or_else(|| format!("no user message at index {index}"))?;
            store
                .set_label(&session, &record_id, Some(name))
                .map_err(|err| format!("cannot bookmark: {err}"))?;
            Ok(record_id)
        })
    }

    /// The configured tree filter mode for `/tree` (gh #231): the
    /// config value at options-build time; `f` cycles from it.
    pub(super) fn tree_filter_mode(&self) -> lca_ui::state::TreeFilterMode {
        let mode = crate::lock(&self.config).ui_tree_filter_mode().to_string();
        Arc::new(move || mode.clone())
    }

    /// The live session's entry tree for `/tree` (gh #37, FR-UI-16,
    /// gh #231): structured rows oldest-first; the navigator paints
    /// connectors, markers, and filters from these.
    pub(super) fn entry_tree(&self) -> lca_ui::state::SessionTree {
        use lca_ui::state::{TreeRow, TreeRowKind};
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move || {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            store
                .entry_tree(&session)
                .unwrap_or_default()
                .into_iter()
                .map(|row| {
                    let kind = match row.kind {
                        lca_session::EntryKind::User => TreeRowKind::User,
                        lca_session::EntryKind::Assistant => TreeRowKind::Assistant,
                        lca_session::EntryKind::Tool => TreeRowKind::Tool,
                        lca_session::EntryKind::Summary => TreeRowKind::Summary,
                        lca_session::EntryKind::Compaction => TreeRowKind::Compaction,
                    };
                    TreeRow {
                        id: row.id,
                        depth: row.depth,
                        kind,
                        text: row.text,
                        label: row.label,
                        live: row.live,
                    }
                })
                .collect()
        })
    }

    /// Bookmark a tree row by record id (gh #231, FR-SESS-10): an
    /// empty name clears the bookmark. Errors name the record, never
    /// the store's vocabulary.
    pub(super) fn label_record(&self) -> lca_ui::state::LabelRecord {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(
            move |record_id: &str, name: &str| -> Result<String, String> {
                let session = session_cell
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let label = (!name.is_empty()).then(|| name.to_string());
                store
                    .set_label(&session, record_id, label.as_deref())
                    .map_err(|_| format!("no tree row points at record {record_id}"))?;
                Ok(if name.is_empty() {
                    "bookmark cleared".to_string()
                } else {
                    format!("bookmarked '{name}'")
                })
            },
        )
    }

    /// Branch at a tree row and return the replayed chain (gh #37,
    /// FR-UI-16): the interface replays these records like a session
    /// switch, but the log kept one file.
    pub(super) fn branch_here(&self) -> lca_ui::state::BranchHere {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |record_id: &str| {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if store.branch_at(&session, record_id).is_err() {
                return None;
            }
            store
                .read_with(&session, lca_session::ViewMode::Display)
                .map(|outcome| outcome.records)
                .ok()
        })
    }

    /// Rename the live session (gh #37): the entry trails in the log,
    /// the title resolves as before.
    pub(super) fn rename_session(&self) -> lca_ui::state::RenameSession {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |name: &str| {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            match store.rename(&session, name) {
                Ok(()) => Ok(format!("renamed to '{name}'")),
                Err(err) => Err(format!("cannot rename: {err}")),
            }
        })
    }

    /// Every live bookmark as `(name, record id)` (gh #37, FR-SESS-10).
    pub(super) fn list_labels(&self) -> lca_ui::state::ListLabels {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move || {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let mut pairs = store
                .labels(&session)
                .unwrap_or_default()
                .into_iter()
                // The store maps record -> name; the verbs read name
                // -> record, sorted for a stable listing.
                .map(|(target, name)| (name, target))
                .collect::<Vec<_>>();
            pairs.sort();
            pairs
        })
    }
}

/// Fork at `record_id`, reporting the new branch (gh #203's tail,
/// shared by the index and record-id hooks so both word the same).
pub(super) fn fork_report(
    store: &lca_session::SessionStore,
    session: &lca_session::Session,
    record_id: &str,
    origin: &str,
) -> lca_ui::state::ForkReport {
    match store.fork(session, record_id) {
        Ok(branch) => lca_ui::state::ForkReport {
            id: Some(branch.id().to_string()),
            notice: format!("✓ Forked from {origin} (session: {})", branch.id()),
        },
        Err(err) => lca_ui::state::ForkReport {
            id: None,
            notice: format!("fork failed: {err}"),
        },
    }
}
