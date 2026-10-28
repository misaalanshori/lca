//! The transient toast stack (gh #239), split out of `chat.rs` for the
//! 1,200-line ceiling (gate 11).
//!
//! A toast is a confirmation or a host diagnostic that floats top-right and
//! fades: it never steals a transcript row and never lands in history, which is
//! what separates it from [`Chat::say`](crate::chat::Chat::say) (a permanent
//! transcript notice). The stack lives on `Chat` as `toasts`; [`Chat::toast`]
//! pushes and [`Chat::expire_toasts`] reaps.

use crate::chat::{Chat, TOAST_MAX, TOAST_SECS, TOAST_SUCCESS_SECS, Toast};

impl Chat {
    /// Float it top-right (gh #239): a transient confirmation or
    /// host diagnostic, fading after one of the two windows.
    ///
    /// [`Self::toast`] is the **long** (error/notice) arm and keeps the
    /// pre-gh-#374-gap-4 behaviour every existing caller relies on;
    /// [`Self::toast_success`] is the short one. Newest last; past
    /// [`TOAST_MAX`] the oldest drops.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.push_toast(text, TOAST_SECS);
    }

    /// Float a **success**, which fades sooner than a caveat or a failure
    /// (gh #374 gap 4).
    ///
    /// The duration is a claim about reading speed, not a style choice: a
    /// confirmation needs only to be noticed, while a caveat has to be read. pi
    /// draws the same line (`COPY_ERROR_FLASH_DURATION_MS`).
    pub fn toast_success(&mut self, text: impl Into<String>) {
        self.push_toast(text, TOAST_SUCCESS_SECS);
    }

    /// Float a **caveat or failure** (gh #374 gap 4).
    ///
    /// Named rather than folded into [`Self::toast`] so a call site has to say
    /// which of the two it is showing - the whole point of the gap is that a copy
    /// result cannot be reported as a bare confirmation.
    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.push_toast(text, TOAST_SECS);
    }

    fn push_toast(&mut self, text: impl Into<String>, secs: u64) {
        self.toasts.push(Toast {
            text: text.into(),
            until: std::time::Instant::now() + std::time::Duration::from_secs(secs),
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
