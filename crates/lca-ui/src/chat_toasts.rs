//! The transient toast stack (gh #239), split out of `chat.rs` for the
//! 1,200-line ceiling (gate 11).
//!
//! A toast is a confirmation or a host diagnostic that floats top-right and
//! fades: it never steals a transcript row and never lands in history, which is
//! what separates it from [`Chat::say`](crate::chat::Chat::say) (a permanent
//! transcript notice). The stack lives on `Chat` as `toasts`; [`Chat::toast`]
//! pushes and [`Chat::expire_toasts`] reaps.

use crate::chat::{Chat, TOAST_MAX, TOAST_SECS, Toast};

impl Chat {
    /// Float it top-right (gh #239): a transient confirmation or
    /// host diagnostic, fading after [`TOAST_SECS`] seconds. Newest
    /// last; past [`TOAST_MAX`] the oldest drops.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            until: std::time::Instant::now() + std::time::Duration::from_secs(TOAST_SECS),
        });
        while self.toasts.len() > TOAST_MAX {
            self.toasts.remove(0);
        }
    }

    /// Drop faded toasts. Returns `true` when the frame changed.
    pub fn expire_toasts(&mut self) -> bool {
        let now = std::time::Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.until > now);
        self.toasts.len() != before
    }
}
