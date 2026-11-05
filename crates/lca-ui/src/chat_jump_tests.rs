//! The gh #375 gap 3 rows: a jump target is an identity, so a resize or a re-theme
//! cannot move it. Split from `chat_viewport_tests.rs` at the 1,200-line ceiling.
//!
//! Every row here drives the **production order** - `jump_prompt`, then
//! `take_jump_scroll`, then `clamp_scroll` - because that order is where the bug lived:
//! each half alone is defensible and the three together drifted.

use super::Chat;
use super::chat;
use super::tests::strip;

// ===========================================================================
// gh #375 gap 3: a jump target is an identity, so the row is resolved at paint
// time and a resize or a re-theme cannot move it.
// ===========================================================================

/// A chat in fullscreen with `turns` user prompts, each with an answer, so the
/// transcript overflows a 24-row pane.
fn turns_chat(turns: usize) -> Chat {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..turns {
        chat.transcript.push_user(format!("prompt {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    chat
}

/// Jump to prompt `index` and return the scroll the loop would apply.
fn jump_to(chat: &mut Chat, index: i32, w: u16, h: u16) -> u16 {
    // `jump_prompt` walks from the current position, so start at the top.
    chat.jump_target = Some(crate::chat_jump::JumpTarget::Prompt(0));
    let _ = chat.take_jump_scroll(w, h);
    for _ in 0..index {
        chat.jump_prompt(1);
        let _ = chat.take_jump_scroll(w, h);
    }
    chat.pinned_jump
        .map(|pin| pin.target)
        .and_then(|target| chat.resolve_jump_row(target, w))
        .map(|row| chat.scroll_pinning(row, w, h))
        .expect("the jump pinned something")
}

// Verifies: gh #375 gap 3 - **a jump, then a resize, leaves the target at the top.**
//
// This is the row that fails today. The target used to be a document row computed at
// the width the key was pressed at; narrowing the pane rewraps every line, so the same
// number pointed at a different prompt. Now the target is the message's **ordinal** and
// the row comes from the rows the frame is about to paint.
#[test]
fn a_jump_survives_a_resize_with_the_prompt_still_at_the_top() {
    let mut chat = turns_chat(40);
    let (w, h) = (100u16, 24u16);
    let scroll = jump_to(&mut chat, 10, w, h);
    let before = strip(&chat.viewport(w, h, scroll));
    assert!(
        before.first().is_some_and(|row| row.contains("prompt 10")),
        "the jump lands on prompt 10 at the original width:\n{}",
        before.join("\n")
    );

    // Narrow the pane - the rewrap changes every row index - and let the loop's
    // re-resolve run exactly as it does on the next frame.
    let (w2, h2) = (48u16, 24u16);
    chat.world.resize(w2, h2);
    let scroll2 = chat
        .take_jump_scroll(w2, h2)
        .expect("a pinned jump re-resolves at the new size");
    let after = strip(&chat.viewport(w2, h2, scroll2));
    assert!(
        after.first().is_some_and(|row| row.contains("prompt 10")),
        "the same prompt is still on the top row after a resize:\n{}",
        after.join("\n")
    );
}

// Verifies: gh #375 gap 3 - **a jump, then a theme change**, likewise.
//
// A theme can change a row's height (padding, code-block borders), so the row must be
// resolved *after* the theme is in place. `set_theme` invalidates the render cache and
// bumps the epoch, and the pin re-resolves on that alone - the pane size does not move,
// which is why geometry by itself is not enough to notice a re-theme.
#[test]
fn a_jump_survives_a_theme_change_with_the_prompt_still_at_the_top() {
    let mut chat = turns_chat(40);
    let (w, h) = (80u16, 24u16);
    let scroll = jump_to(&mut chat, 7, w, h);
    assert!(
        strip(&chat.viewport(w, h, scroll))
            .first()
            .is_some_and(|row| row.contains("prompt 7")),
        "the jump landed before the theme change"
    );

    chat.set_theme("light");
    let scroll2 = chat
        .take_jump_scroll(w, h)
        .expect("a theme change re-resolves the pin");
    assert!(
        strip(&chat.viewport(w, h, scroll2))
            .first()
            .is_some_and(|row| row.contains("prompt 7")),
        "and it is still on the top row after the theme changed"
    );
}

// Verifies: gh #375 gap 3 (endpoints) - the **first** prompt pins the transcript's
// start, and the **last** one is bottom-anchored rather than pushed past the end.
#[test]
fn the_first_and_last_prompts_are_the_endpoints() {
    let mut chat = turns_chat(40);
    let (w, h) = (80u16, 24u16);

    // The first prompt is not the document's first row: the startup header, the session
    // line and two separator rows sit above it (the transcript owns those). So "the
    // first prompt pins the top" means **the prompt is on row 0**, not that the scroll is
    // `content - window` - which would put *whatever happens to be the first document
    // row* at the top, which is a different (and wrong) promise. That distinction is
    // itself a gap-3 lesson: the row must come from the identity.
    let first = jump_to(&mut chat, 0, w, h);
    assert!(
        strip(&chat.viewport(w, h, first))
            .first()
            .is_some_and(|row| row.contains("prompt 0")),
        "the first prompt is on the top row"
    );
    assert!(
        first as usize
            <= chat
                .transcript_len(w)
                .saturating_sub(chat.window_height(w, h)),
        "and the resolved scroll is inside the viewport's own range"
    );

    let last = jump_to(&mut chat, 39, w, h);
    assert_eq!(last, 0, "the last prompt is at the live bottom");
    assert!(
        strip(&chat.viewport(w, h, last))
            .iter()
            .any(|row| row.contains("prompt 39")),
        "and it is visible there"
    );
}

// Verifies: gh #375 gap 3 - **a transcript with no user messages jumps nowhere.**
//
// The empty case must not panic and must not store a target: `jump_prompt` returns
// before the ordinal walk, so a Shift+Up on a fresh session leaves `None`.
#[test]
fn a_transcript_with_no_prompts_does_not_jump() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.transcript.append_text("an answer with no question");
    chat.transcript.finish_assistant();
    assert_eq!(chat.prompt_count(), 0);
    chat.jump_prompt(1);
    assert!(chat.jump_target.is_none(), "nothing to jump to");
    assert!(chat.pinned_jump.is_none());
    assert!(chat.take_jump_scroll(80, 24).is_none());
}

// Verifies: gh #375 gap 3 - **the mouse stepper, the Shift+wheel gesture and the
// keyboard all land on the same prompt**, from the same starting position.
//
// The sharing the brief asks for: the steppers and the wheel call the same
// `jump_prompt`, so they inherit the identity target instead of keeping their own
// offset arithmetic. Asserted as the resolved row, not as "a target was set" - the
// whole point is that the three paths agree about *where*.
#[test]
fn the_keyboard_stepper_and_wheel_paths_land_on_the_same_row() {
    use crate::chat_jump::JumpTarget;
    use crate::state::Action;
    let (w, h) = (80u16, 24u16);

    // One starting position for all three: the last prompt, pinned. Two steps back
    // from there, by each input path.
    let start = |chat: &mut Chat| {
        chat.world.resize(w, h);
        chat.jump_target = Some(JumpTarget::Prompt(39));
        let _ = chat.take_jump_scroll(w, h);
    };

    let mut keyboard = turns_chat(40);
    start(&mut keyboard);
    assert_eq!(keyboard.handle_key("\x1b[1;2A"), Action::Continue); // Shift+Up
    let _ = keyboard.take_jump_scroll(w, h);
    assert_eq!(keyboard.handle_key("\x1b[1;2A"), Action::Continue);
    let _ = keyboard.take_jump_scroll(w, h);

    // The stepper: the run loop's `PreviousPrompt` arm is exactly this call.
    let mut stepper = turns_chat(40);
    start(&mut stepper);
    stepper.jump_prompt(-1);
    let _ = stepper.take_jump_scroll(w, h);
    stepper.jump_prompt(-1);
    let _ = stepper.take_jump_scroll(w, h);

    // The Shift+wheel gesture: `shift_wheel_jump` returns the delta the loop feeds to
    // the same `jump_prompt`.
    let mut wheel = turns_chat(40);
    start(&mut wheel);
    for _ in 0..2 {
        if let Some(delta) = crate::run_selection::shift_wheel_jump(&gesture(64 + 4)) {
            wheel.jump_prompt(delta);
            let _ = wheel.take_jump_scroll(w, h);
        }
    }

    let landed = |chat: &Chat| {
        chat.pinned_jump
            .and_then(|pin| chat.resolve_jump_row(pin.target, w))
    };
    assert_eq!(
        landed(&keyboard),
        landed(&stepper),
        "the stepper lands where the keyboard does"
    );
    assert_eq!(
        landed(&keyboard),
        landed(&wheel),
        "and so does the Shift+wheel gesture"
    );
    assert_eq!(
        keyboard.pinned_jump.map(|pin| pin.target),
        Some(JumpTarget::Prompt(37)),
        "two steps back from the last prompt"
    );
}

/// A wheel gesture in the shape the parser produces.
fn gesture(bits: u16) -> lca_tui::engine::alt_screen::SgrMouse {
    lca_tui::engine::alt_screen::parse_sgr_mouse(&format!("\x1b[<{bits};40;12M"))
        .expect("a valid SGR mouse sequence")
}

// Verifies: gh #375 gap 3 - **no painted row carries the marker.**
//
// The marker is in-band so a jump can find its row after any reflow, but it is a
// control sequence: it must be gone by the time a row reaches the screen, a copy
// destination or the scrollback. Asserted on the whole frame including the dock, so a
// path that composes rows another way is covered too.
#[test]
fn no_painted_row_carries_the_prompt_marker() {
    let mut chat = turns_chat(6);
    chat.screen_mode = true;
    for (w, h) in [(80u16, 24u16), (40, 20), (200, 50)] {
        for row in chat.viewport(w, h, 0) {
            assert!(
                !row.contains("\x1b]133;"),
                "a painted row leaked the marker at {w}x{h}: {row:?}"
            );
        }
        for row in chat.render(w) {
            assert!(
                !row.contains("\x1b]133;"),
                "the document leaked the marker at {w}: {row:?}"
            );
        }
    }
    // And the marker is still there to find, at the stage the jump reads.
    assert_eq!(
        chat.transcript.user_offsets(80, &chat.theme).len(),
        6,
        "the marker survives for the jump scan"
    );
}

// Verifies: gh #375 gap 3 - a **re-theme does not move a pinned jump**, even though it
// changes every cached row.
//
// `set_theme` invalidates the render cache and can change row heights; the pin's
// geometry is the width and height, so the re-resolve has to happen on the next frame
// rather than being skipped as "nothing changed".
#[test]
fn a_retheme_reresolves_a_pinned_jump() {
    let mut chat = turns_chat(40);
    let (w, h) = (80u16, 24u16);
    let scroll = jump_to(&mut chat, 12, w, h);
    assert!(
        strip(&chat.viewport(w, h, scroll))
            .first()
            .is_some_and(|row| row.contains("prompt 12"))
    );

    chat.set_theme("plain");
    // The loop's order: take the jump (re-resolve), then clamp.
    let reresolved = chat.take_jump_scroll(w, h);
    assert!(
        reresolved.is_some(),
        "a theme change must re-resolve, not be mistaken for an unchanged pane"
    );
    let scroll2 = chat.clamp_scroll(reresolved.unwrap_or(scroll), w, h);
    assert!(
        strip(&chat.viewport(w, h, scroll2))
            .first()
            .is_some_and(|row| row.contains("prompt 12")),
        "the prompt is still on the top row after the theme change"
    );
}

// Verifies: gh #375 gap 3 - **the loop's exact order keeps the prompt on the top row
// across a resize**: `take_jump_scroll`, then `clamp_scroll`.
//
// This is the row the production path actually runs (see `run.rs`'s render arm), and it
// is not the same as either half alone: `clamp_scroll`'s growth-hold adds the re-wrap's
// line delta to a held position, which for a *pinned* jump would drag the prompt off the
// top row by exactly the lines the rewrap added. The pin owns the position across a
// geometry change, so the hold does not apply to it.
#[test]
fn the_loop_order_keeps_a_pinned_prompt_on_top_across_a_resize() {
    let mut chat = turns_chat(40);
    let (w, h) = (120u16, 30u16);
    let jump = jump_to(&mut chat, 8, w, h);
    // The first frame, exactly as the loop does it.
    chat.world.resize(w, h);
    let pinned = chat.take_jump_scroll(w, h).unwrap_or(jump);
    let scroll = chat.clamp_scroll(pinned, w, h);
    assert!(
        strip(&chat.viewport(w, h, scroll))
            .first()
            .is_some_and(|row| row.contains("prompt 8")),
        "the jump lands on prompt 8 at the first size"
    );

    // Narrow the pane: many more rows, so the growth-hold would move the position.
    let (w2, h2) = (56u16, 30u16);
    chat.world.resize(w2, h2);
    let pinned = chat
        .take_jump_scroll(w2, h2)
        .expect("a pinned jump re-resolves at the new width");
    let scroll2 = chat.clamp_scroll(pinned, w2, h2);
    assert_eq!(
        scroll2, pinned,
        "the pin survives the clamp: the growth-hold must not move a freshly pinned jump"
    );
    assert!(
        strip(&chat.viewport(w2, h2, scroll2))
            .first()
            .is_some_and(|row| row.contains("prompt 8")),
        "and the prompt is still on the top row:\n{}",
        strip(&chat.viewport(w2, h2, scroll2)).join("\n")
    );
}
