//! The interface close-out: flush hooks, close sessions. Split from
//! `mod.rs` for the workspace file ceiling (gate 11). Behaviour
//! unchanged.

use super::Ui;

impl Ui {
    /// This session's temp dir (gh #160), resolved and validated at
    /// construction; the guard in `run` owns it.
    pub(super) fn temp_dir(&self) -> &std::path::Path {
        &self.temp_dir
    }

    /// The close-out: let hooks flush state, then close every
    /// session this run opened (gh #209 - background tabs end cleanly
    /// too, never crash-shaped).
    pub(super) fn close(&self) {
        let registry = self.registry();
        lca_core::drive_blocking(async move {
            registry.on_session_close().await;
        });
        let opened = self
            .opened_sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for id in opened {
            if let Ok(session) = self.store.session(&self.cwd, &id) {
                let _ = self.store.close(&session);
            }
        }
    }
}
