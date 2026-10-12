//! Image staging commands: `/attach <path>`, opener `@file` args, and
//! `/paste` (gh #126). Split from `commands.rs` for the line ceiling;
//! behavior unchanged.

use std::sync::Arc;

use lca_protocol::CommandEffect;

use super::Ui;

/// Stage opener `@file` images into the pending attachments (gh
/// #71): they ride with the first submission exactly like `/attach`
/// does. A file that will not stage warns and drops - the interface
/// stays usable, and the warning names the path.
pub(crate) fn stage_initial_attachments(
    store: &lca_session::SessionStore,
    session: &Arc<std::sync::Mutex<lca_session::Session>>,
    paths: &[std::path::PathBuf],
) -> Vec<lca_core::StagedAttachment> {
    let Ok(session) = session.lock() else {
        return Vec::new();
    };
    // gh #122: staging is content - it materializes a pending
    // session (meta and session-start land with the attachments).
    if !paths.is_empty() {
        let _ = store.ensure_materialized(&session);
    }
    let mut staged = Vec::new();
    for path in paths {
        match lca_core::stage_image(&session, path) {
            Ok(attachment) => staged.push(attachment),
            Err(err) => eprintln!("warning: @{}: {err}", path.display()),
        }
    }
    staged
}

impl Ui {
    /// Resolve an attachment hash against the *current* session (a fork's
    /// images live in its ancestor's directory, store.attachment_path walks
    /// the chain), so a replayed message shows its image (FR-UI-13).
    pub(super) fn load_attachment(&self) -> lca_ui::state::LoadAttachment {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |hash: &str| {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let path = store.attachment_path(&session, hash)?;
            let bytes = std::fs::read(path).ok()?;
            let media = lca_protocol::sniff_image_media_type(&bytes)?.to_string();
            Some((media, bytes))
        })
    }
    /// `/attach <path>`: stage an image for the next turn (ADR-0029). The
    /// bytes land in the session's attachment store; the stub text rides
    /// the next submission, and the placeholder renders at once.
    pub(super) fn command_attach(&self, argument: &str) -> CommandEffect {
        let session = self.session();
        let path = std::path::Path::new(argument.trim());
        match lca_core::stage_image(&session, path) {
            Ok(staged) => {
                let note = format!(
                    "attached {} - it goes with your next message",
                    &staged.hash[..8]
                );
                self.pending_attachments
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(staged);
                // Show the image placeholder at once (FR-UI-13); fall back to
                // the note when the bytes are gone.
                match std::fs::read(path).ok().and_then(|bytes| {
                    lca_protocol::sniff_image_media_type(&bytes)
                        .map(|media_type| (media_type.to_string(), bytes))
                }) {
                    Some((media_type, bytes)) => CommandEffect::AttachImage {
                        media_type,
                        bytes,
                        note,
                    },
                    None => CommandEffect::ShowWidget(note),
                }
            }
            Err(err) => CommandEffect::ShowWidget(format!("attach failed: {err}")),
        }
    }

    /// `/paste`: stage the clipboard image for the next turn (gh #126,
    /// pi's `app.clipboard.pasteImage`). An empty clipboard is a
    /// notice, never an error: there is nothing to fix, only nothing
    /// to attach. Drag-and-drop needs no separate path: terminals
    /// paste the dropped file's path as text, which `/attach <path>`
    /// (or an `@path` opener argument) already stages.
    pub(super) fn command_paste(&self) -> CommandEffect {
        let session = self.session();
        let clipboard = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image());
        let image = match clipboard {
            Ok(image) => image,
            Err(_) => {
                return CommandEffect::ShowWidget(
                    "the clipboard holds no image - copy one first".to_string(),
                );
            }
        };
        let png = match lca_tools::clipboard_png(
            u32::try_from(image.width).unwrap_or(0),
            u32::try_from(image.height).unwrap_or(0),
            &image.bytes,
        ) {
            Ok(png) => png,
            Err(err) => return CommandEffect::ShowWidget(format!("paste failed: {err}")),
        };
        let hash = match lca_core::store_image_bytes(&session, &png) {
            Ok(hash) => hash,
            Err(err) => return CommandEffect::ShowWidget(format!("paste failed: {err}")),
        };
        let note = format!(
            "pasted clipboard image {} - it goes with your next message",
            &hash[..8]
        );
        self.pending_attachments
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(lca_core::StagedAttachment {
                hash,
                stub: format!(
                    "[image attachment clipboard, image/png, {} bytes]",
                    png.len()
                ),
            });
        CommandEffect::AttachImage {
            media_type: "image/png".to_string(),
            bytes: png,
            note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Verifies: gh #71/#126 - an opener `@shot.png` stages like
    // `/attach` does; a non-image warns and drops, never stages.
    #[test]
    fn opener_image_args_stage_and_non_images_drop() {
        let dir = tempfile::tempdir().expect("scratch");
        let store = lca_session::SessionStore::new(dir.path().join("store"));
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let session = store.create_session(&project, "test").expect("session");
        let session = Arc::new(std::sync::Mutex::new(session));
        let png = project.join("shot.png");
        std::fs::write(
            &png,
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4],
        )
        .expect("write png");
        let txt = project.join("notes.txt");
        std::fs::write(&txt, b"just text").expect("write txt");

        let staged = super::stage_initial_attachments(
            &store,
            &session,
            &[png.clone(), txt.clone(), project.join("missing.png")],
        );
        assert_eq!(staged.len(), 1, "only the png stages");
        assert!(staged[0].stub.contains("image/png"));
        let stored = session
            .lock()
            .expect("session")
            .dir()
            .join("attachments")
            .join(&staged[0].hash);
        assert!(stored.exists(), "bytes land in the store");
    }
}

#[cfg(test)]
#[cfg(test)]
mod ui_tests {
    use super::super::{BUILTIN_SLOTS, adopt_login_identity, live_cells};

    // Verifies: E5 - the status label names the preset identity, not the
    // extension, when the login stored one.
    #[test]
    fn live_cells_label_the_preset_identity() {
        let cells = live_cells("opencode-go", "deepseek-v4.1-flash", 0, None);
        assert_eq!(
            *cells.label.lock().unwrap(),
            "opencode-go/deepseek-v4.1-flash"
        );
    }

    // Cycle-6 drive regression: a mid-session login must move the identity
    // cell with the preset, or a later `/model` switch reads the stale
    // startup identity and the footer names the extension instead of the
    // preset. The drive reproduced this after logging in on a session that
    // started logged out.
    #[test]
    fn a_mid_session_login_adopts_the_preset_into_the_identity_cell() {
        // Startup had no stored preset, so the cell held the extension.
        let cells = live_cells("openai-compatible", "deepseek-v4.1-flash", 0, None);
        // The login then completes on the opencode-go preset.
        adopt_login_identity(&cells.identity, "opencode-go");
        assert_eq!(*cells.identity.lock().unwrap(), "opencode-go");
        // A `/model` switch keeps the preset in the footer label.
        let label = format!(
            "{}/{}",
            *cells.identity.lock().unwrap(),
            "deepseek-v4.1-flash"
        );
        assert_eq!(label, "opencode-go/deepseek-v4.1-flash");
    }

    // SRDD's interface section: the built-in slots, of which the five
    // spec-named ones live in this list alongside `/attach`, `/paste`,
    // and /stats arrives from the native hooks extension that holds
    // the stats source.
    #[test]
    fn the_spec_named_builtins_are_claimed() {
        for slot in [
            "login", "logout", "usage", "model", "compact", "attach", "paste",
        ] {
            assert!(
                BUILTIN_SLOTS.contains(&slot),
                "/{slot} is a built-in the interface claims"
            );
        }
    }
}
