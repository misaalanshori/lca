//! Session hook builders (ceiling split from `hooks.rs`):
//! the session list, switch, fork, and clone closures the interface
//! calls. Behavior unchanged.

use std::sync::Arc;

use super::Ui;
use super::display::age_label;
use lca_protocol::Record;
use lca_session::ViewMode;

impl Ui {
    /// The `/resume` session list (R2), newest first with ages.
    pub(super) fn session_list(&self) -> lca_ui::state::SessionList {
        let store = self.store.clone();
        let cwd = self.cwd.clone();
        Arc::new(move || {
            let now = lca_session::now_ms();
            store
                .list_sessions(&cwd)
                .unwrap_or_default()
                .into_iter()
                .map(|summary| {
                    let title = summary.display_title();
                    // gh #75: bookmark names ride the row so the
                    // picker filter finds them (one resolved read per
                    // session - the picker opens rarely, the list is
                    // short, and listing already reads every log).
                    let labels = store
                        .session(&cwd, &summary.id)
                        .ok()
                        .and_then(|session| store.labels(&session).ok())
                        .map(|marks| {
                            let mut names: Vec<String> = marks.into_values().collect();
                            names.sort();
                            names.dedup();
                            names
                        })
                        .unwrap_or_default();
                    lca_ui::resume::SessionEntry {
                        id: summary.id,
                        title,
                        messages: summary.message_count,
                        age: age_label(now, summary.modified_ms),
                        labels,
                    }
                })
                .collect()
        })
    }

    pub(super) fn switch_session(&self) -> lca_ui::state::SwitchSession {
        let store = self.store.clone();
        let cwd = self.cwd.clone();
        let data = self.data.clone();
        let session_cell = self.current_session.clone();
        let opened = self.opened_sessions.clone();
        Arc::new(move |id: &str| -> Option<Vec<lca_protocol::Record>> {
            // gh #122: a pending session has no directory yet -
            // entering it replays nothing instead of refusing.
            let session = store.open_or_pending(&cwd, id).ok()?;
            let read = store.read_with(&session, ViewMode::Display).ok()?;
            if let Err(err) = crate::ensure_session_temp(&data, session.id()) {
                eprintln!("error: cannot create the session temp dir: {err}");
                return None;
            }
            *session_cell.lock().unwrap_or_else(|p| p.into_inner()) = session;
            // gh #209: every opened session ends cleanly at close.
            let mut opened = opened.lock().unwrap_or_else(|p| p.into_inner());
            if !opened.iter().any(|open| open == id) {
                opened.push(id.to_string());
            }
            Some(read.records)
        })
    }

    pub(super) fn fork_at(&self) -> lca_ui::state::ForkAt {
        use lca_ui::state::ForkReport;
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |index: usize| -> ForkReport {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let outcome = match store.read_with(&session, ViewMode::Display) {
                Ok(outcome) => outcome,
                Err(err) => {
                    return ForkReport {
                        id: None,
                        notice: format!("cannot read the session: {err}"),
                    };
                }
            };
            let record_id = outcome
                .records
                .iter()
                .filter(|record| matches!(record, Record::User { .. }))
                .nth(index)
                .and_then(|record| record.id().map(str::to_string));
            let Some(record_id) = record_id else {
                return ForkReport {
                    id: None,
                    notice: format!("no user message at index {index}"),
                };
            };
            super::label_hooks::fork_report(
                store.as_ref(),
                &session,
                &record_id,
                &format!("turn {index}"),
            )
        })
    }

    pub(super) fn clone_session(&self) -> lca_ui::state::CloneSession {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(
            move |name: Option<String>| -> Result<(String, String), String> {
                let session = session_cell
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                match store.clone_session(&session, name.as_deref()) {
                    Ok(child) => {
                        let title = store
                            .meta(&child)
                            .map(|meta| meta.title)
                            .unwrap_or_default();
                        Ok((child.id().to_string(), title))
                    }
                    Err(err) => Err(format!("clone failed: {err}")),
                }
            },
        )
    }
}
