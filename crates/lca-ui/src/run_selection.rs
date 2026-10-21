//! Selection lifetime across a transcript replacement (gh #403 gap 1).
//!
//! Split from `run.rs` for the 1,200-line ceiling (gate 11); behaviour
//! unchanged by the split.
//!
//! **The rule.** A selection lives in *content* coordinates - row and column
//! in the rendered document - which is what lets it survive scrolling. That
//! also means it does not survive *replacement*: after `/resume`, a `/tree`
//! jump, a fork, a clone, or `/import`, the same coordinates address
//! different text, so the highlight paints over rows the user never selected
//! and a copy hands back bytes from a transcript that no longer exists.
//!
//! **Where the reset belongs.** At the one seam every replacement path shares,
//! not at the call sites. `Chat::replay_records` is the only thing that clears
//! the transcript, so it is the only thing that bumps
//! [`Transcript::generation`]; the run loop compares that against the value it
//! last saw. Four call sites is how the fifth replacement path gets missed.
//!
//! **The mirror-image bug.** A redraw that does *not* replace content - a
//! resize, a streaming delta, a marker repaint - leaves earlier rows exactly
//! where they were, so the selection over them is still valid and must
//! survive. A reset that fires on every one of those would train users out of
//! trusting selection altogether, so both directions are pinned.

use crate::chat::Chat;
use crate::run::Screen;

impl Screen {
    /// Drop the selection and the multi-click run because the transcript was
    /// replaced (gh #403 gap 1).
    ///
    /// Distinct from [`Screen::clear_selection`], which a picker press uses:
    /// that one clears the rectangles because a release must not copy what the
    /// picker ate, while the click run still belongs to the same screen.
    pub(super) fn reset_selection_for_replacement(&mut self) {
        if let Screen::Alt(r) = self {
            r.reset_text_selection();
        }
    }
}

/// The gh #403 gap 1 decision: has the transcript been *replaced* since we
/// last looked, and if so drop the selection and the multi-click run?
///
/// Extracted rather than inlined into the run loop because the interesting
/// part is the comparison, and a comparison buried in a `loop` can only be
/// tested by driving a whole terminal. `seen` is the generation the caller
/// last accounted for; the return value is what to store next.
pub(super) fn reset_selection_if_replaced(screen: &mut Screen, chat: &Chat, seen: u64) -> u64 {
    let generation = chat.transcript.generation();
    if generation != seen {
        screen.reset_selection_for_replacement();
    }
    generation
}

/// A Shift+wheel gesture's prompt step (gh #173): wheel-up steps back,
/// wheel-down steps forward. `None` for anything missing either half -
/// a plain wheel keeps scrolling, an unmodified press keeps selecting.
pub(super) fn shift_wheel_jump(mouse: &lca_tui::engine::alt_screen::SgrMouse) -> Option<i32> {
    if mouse.bits & 64 == 0 || mouse.bits & 4 == 0 {
        return None;
    }
    Some(if mouse.bits & 3 == 0 { -1 } else { 1 })
}

/// Wheel lines for a numeric setting (gh #82, gh #206): `auto` resolves
/// to pi's own one-line baseline - the velocity curve scales it from
/// there, so no platform sniff is needed.
fn wheel_lines_for(tuning: &crate::state::DisplayTuning) -> u8 {
    match tuning.fullscreen_wheel_lines.as_str() {
        "auto" => 1,
        text => text.parse().unwrap_or(1),
    }
}

/// Sync the fullscreen tunables onto a screen (gh #82): copy and wheel
/// behavior follow config, so a `/settings` cycle applies on the next
/// switch. Lives beside the selection reset because `copy_on_select` is what
/// decides whether a release copies the selection at all.
pub(super) fn apply_screen_tuning(screen: &mut Screen, tuning: &crate::state::DisplayTuning) {
    if let Screen::Alt(alt) = screen {
        alt.copy_on_select = tuning.fullscreen_copy_on_select;
        alt.wheel_lines = wheel_lines_for(tuning);
        // Gh #206: `auto` is the velocity curve now (pi's shape), so a
        // mode flip restarts the gesture.
        let auto = tuning.fullscreen_wheel_lines.as_str() == "auto";
        if alt.wheel_auto != auto {
            alt.wheel_auto = auto;
            alt.reset_wheel();
        }
    }
}

#[cfg(test)]
#[path = "run_selection_tests.rs"]
mod tests;
