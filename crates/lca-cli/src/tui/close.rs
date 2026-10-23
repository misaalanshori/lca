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

    /// The resume command for the session this run ended in (gh #403 gap 2).
    ///
    /// `None` unless all three honesty gates hold: stdout is a TTY, the session
    /// has an id, and - because `close` has already run - that id was written.
    /// The command string itself comes from `lca_ui::resume_command`, the same
    /// formatter the fullscreen-exit hint uses.
    ///
    /// The session is the one the run *ended* in, not the one it launched with: a
    /// `/tree` jump or an in-session `/resume` switches `current_session`, and
    /// naming the launch argument would point the user at a transcript they had
    /// already left.
    pub(super) fn exit_resume_hint(&self) -> Option<String> {
        use std::io::IsTerminal as _;
        let session = self
            .current_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Gate 2, and the reason it is a filesystem question rather than "does the
        // id exist": a lazy session (gh #122) is *pending* - it has an id from the
        // moment the interface opens, but no directory until the first record is
        // written. `SessionStore::close` uses this same test to decide whether
        // there is anything to close, so asking it here keeps the hint honest by
        // construction: a launch-and-quit run wrote nothing, so it advertises
        // nothing.
        if !session.log_path().is_file() {
            return None;
        }
        let id = session.id().to_string();
        let dir = session.dir().to_path_buf();
        // A session under the default data home needs only `--resume <id>`;
        // anywhere else needs the path too, because `--resume` resolves against
        // the current home.
        let relocated = !dir.starts_with(crate::data_dir());
        let session_dir = relocated.then_some(dir.as_path());
        lca_ui::resume_command::exit_resume_hint(&id, session_dir, std::io::stdout().is_terminal())
    }

    /// The close-out: stop this session's inbound HTTP listener, let hooks
    /// flush state, then close every session this run opened (gh #209 -
    /// background tabs end cleanly too, never crash-shaped).
    ///
    /// The listener stops **first** (gh #363, ADR-0049): a request arriving
    /// while hooks flush would be answered by an extension whose session is
    /// already closing, and the port must be free before this returns so a
    /// restarted session can take the same one.
    /// Close out, then say how to come back (gh #403 gap 2).
    ///
    /// The print lives here rather than at the call site so no exit path can
    /// close a session and forget the hint - the same "one seam" rule the
    /// fullscreen notice and #403 gap 1's selection reset follow.
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
        // Every session is closed, so the hint's second gate is now answerable.
        if let Some(hint) = self.exit_resume_hint() {
            println!("{hint}");
        }
    }
}
