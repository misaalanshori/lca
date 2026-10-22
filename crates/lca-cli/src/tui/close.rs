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

    /// The close-out: stop this session's inbound HTTP listener, let hooks
    /// flush state, then close every session this run opened (gh #209 -
    /// background tabs end cleanly too, never crash-shaped).
    ///
    /// The listener stops **first** (gh #363, ADR-0049): a request arriving
    /// while hooks flush would be answered by an extension whose session is
    /// already closing, and the port must be free before this returns so a
    /// restarted session can take the same one.
    pub(super) fn close(&self) {
        crate::lock(&self.session_http).stop();
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
