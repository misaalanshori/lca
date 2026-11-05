//! Prompt and viewport jumps (FR-UI-11, gh #173, gh #375 gap 3), split from
//! `chat_keys.rs` at the workspace's 1,200-line file ceiling.
//!
//! The subject of this module is one idea: **a jump target is an identity, and the row
//! is resolved when the frame paints it.** A stored row is a derived coordinate -
//! computed at one width and theme, applied under another - which is exactly how a jump
//! came to point at the wrong line after a resize or a re-theme.

use super::chat::Chat;

/// What a viewport jump is aiming at (FR-UI-11, gh #173, gh #375 gap 3).
///
/// **An identity where one exists, and a row only where the subject really is a row.**
///
/// A prompt jump used to store a document row, computed at the width and theme in force
/// when the key was pressed, and apply it on a later frame under whatever width and theme
/// were in force then. Resize the pane and the wrap changes; switch theme and a row's
/// height changes (padding, code-block borders); either way the stored number now names a
/// different line. The prompt is a *fact* — "the third user message" — so that is what is
/// stored, and the row is derived from it when the frame is painted.
///
/// The two row-shaped callers keep a row, and that is deliberate rather than unfinished:
///
/// - **the transcript search** matches against rendered row *text*, so its subject is a
///   row by definition — there is no entry identity to resolve to (`/` finds text, which
///   may be in the middle of an assistant message, a tool card, or a notice);
/// - **a tab restore** records how far the reader had scrolled, which is a position, not
///   an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTarget {
    /// A user message by its ordinal among user messages (0 = the first prompt).
    Prompt(usize),
    /// An absolute document row, for a caller whose subject is a row.
    Row(usize),
}

/// A jump that was applied, kept so a later geometry change can re-resolve it
/// (gh #375 gap 3).
///
/// Taking the target clears the *request*, so without this a resize two frames later has
/// nothing left to re-resolve and the prompt drifts. `width`/`height`/`epoch` are what the
/// pin was last resolved at, which is how the clamp can tell "the reader scrolled away"
/// from "the render changed under them".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PinnedJump {
    pub(crate) target: JumpTarget,
    pub(crate) width: u16,
    pub(crate) height: u16,
    /// The transcript's render epoch when the pin was resolved (gh #375 gap 3): a theme
    /// change invalidates the cache and bumps this without touching the pane size, so it
    /// is the only signal that a row's *height* may have moved.
    pub(crate) epoch: u64,
}

/// Prompt jumps (gh #173), split from `chat.rs` (the 1,200-line ceiling).
impl Chat {
    /// Move the prompt-jump target to the previous/next user message
    /// (FR-UI-11). The scroll is applied by the loop on the next frame.
    ///
    /// Gh #375 gap 3: the target is a **message ordinal**, so the row it names is decided
    /// by the frame that paints it. `delta` walks the ordinals, and "where am I now" is
    /// the pin when one is live, else the top of the viewport.
    pub(crate) fn jump_prompt(&mut self, delta: i32) {
        let count = self.prompt_count();
        if count == 0 {
            return;
        }
        // Where the walk starts: the pinned prompt, else the first prompt at or below
        // the current top row. The second half needs the current geometry, which is
        // why the row is read here rather than stored.
        let width = self.world.size.0.max(1);
        let height = self.world.size.1.max(1);
        let current = match (self.pinned_jump, self.jump_target) {
            (Some(pin), _) => match pin.target {
                JumpTarget::Prompt(index) => index,
                JumpTarget::Row(_) => 0,
            },
            (None, Some(JumpTarget::Prompt(index))) => index,
            (None, _) => {
                let offsets = self.transcript.user_offsets(width, &self.theme);
                let top = self.transcript_len(width).saturating_sub(
                    self.window_height(width, height) + usize::from(self.last_scroll),
                );
                offsets
                    .iter()
                    .position(|&offset| offset >= top)
                    .unwrap_or(0)
            }
        };
        let next = (current as i32 + delta).clamp(0, count as i32 - 1) as usize;
        self.jump_target = Some(JumpTarget::Prompt(next));
    }

    /// How many user messages the transcript holds.
    ///
    /// Read from the rendered rows' markers, so it agrees with what
    /// [`Self::resolve_jump_row`] will find during the frame for the same width and
    /// theme - one measurement, not a second counting pass over the entries.
    pub(crate) fn prompt_count(&self) -> usize {
        let width = self.world.size.0.max(1);
        self.transcript.user_offsets(width, &self.theme).len()
    }

    /// The document row a target names **at this width and theme** (gh #375 gap 3).
    ///
    /// This is the "resolve late" half: it runs during the frame, on the rows the frame
    /// will paint, so a resize or a theme change between the key and the paint cannot
    /// desynchronise them. `None` when the target no longer exists (a prompt count that
    /// shrank), which the caller treats as "no jump".
    pub(crate) fn resolve_jump_row(&self, target: JumpTarget, width: u16) -> Option<usize> {
        match target {
            JumpTarget::Prompt(index) => self
                .transcript
                .user_offsets(width.max(1), &self.theme)
                .get(index)
                .copied(),
            JumpTarget::Row(row) => Some(row),
        }
    }

    /// The scroll value that top-pins a pending jump target, if any
    /// (FR-UI-11, gh #173): the prompt's first line lands on the
    /// viewport's row 0, its answer starting beneath it.
    ///
    /// Gh #375 gap 3: a **re-resolve** for a pin whose geometry has changed. A jump that
    /// was applied records its identity and geometry; when the pane is resized (or the theme
    /// changed under it, which the pin cannot see but the width can) this recomputes the
    /// scroll from the identity and re-pins, so the prompt is still at the top. Whether
    /// the reader has since scrolled away is [`Self::clamp_scroll`]'s question.
    pub fn take_jump_scroll(&mut self, width: u16, height: u16) -> Option<u16> {
        let pending = match self.jump_target.take() {
            Some(target) => target,
            None => {
                let pin = self.pinned_jump?;
                // Gh #375 gap 3: the pin records the render **epoch** alongside the
                // geometry, because a theme change moves rows without moving the pane.
                // A pin whose epoch is stale re-resolves even at an unchanged size, which
                // is the `set_theme` case; one that is current has nothing to do.
                if pin.width == width
                    && pin.height == height
                    && pin.epoch == self.transcript.epoch()
                {
                    return None;
                }
                pin.target
            }
        };
        let row = self.resolve_jump_row(pending, width)?;
        self.pinned_jump = Some(PinnedJump {
            target: pending,
            width,
            height,
            epoch: self.transcript.epoch(),
        });
        Some(self.scroll_pinning(row, width, height))
    }

    /// The scroll that puts document row `row` at the top of the window.
    ///
    /// Gh #35: a transcript coordinate - the dock below is in neither count. The last
    /// prompt of a long session resolves past the end, and the saturating subtraction
    /// lands it at the live bottom, which is the bottom-anchored case.
    pub(crate) fn scroll_pinning(&self, row: usize, width: u16, height: u16) -> u16 {
        let total = self.transcript_len(width);
        let window = self.window_height(width, height);
        let scroll = total.saturating_sub(row.saturating_add(window));
        scroll.min(u16::MAX as usize) as u16
    }
}
