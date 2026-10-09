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
