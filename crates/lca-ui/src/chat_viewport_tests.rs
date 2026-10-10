//! The viewport rows (gh #35): the fullscreen dock split, pi's scrollbar
//! geometry and where it paints, the jump indicator, and the scroll hold.
//! Split from `chat_tests.rs`, which crossed the workspace's 1,200-line
//! ceiling (gate 11) with these rows in it.

use super::chat;
use crate::chat_mouse::ScrollbarHit;

// Verifies: gh #35 (the headline) - in fullscreen the dock pins: the
// editor and footer occupy the same rows whatever the transcript's
// scroll, while the transcript window above them shows a different
// slice. The old viewport sliced the whole document, so scrolling up
// walked the editor off the bottom.
#[test]
fn the_dock_stays_pinned_while_the_transcript_scrolls() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let at_bottom = chat.viewport(w, h, 0);
    let scrolled = chat.viewport(w, h, 20);

    assert_eq!(at_bottom.len(), h as usize, "the frame fills the viewport");
    assert_eq!(scrolled.len(), h as usize, "and still fills it scrolled");

    let strip = |row: &str| lca_tui::engine::text::strip_terminal_sequences(row);
    let editor_row = |rows: &[String]| {
        rows.iter()
            .rposition(|row| strip(row).trim_start().starts_with("> "))
    };
    let bottom_editor = editor_row(&at_bottom).expect("the editor is on screen");
    let scrolled_editor = editor_row(&scrolled).expect("...and stays on screen");
    assert_eq!(
        &at_bottom[bottom_editor..],
        &scrolled[scrolled_editor..],
        "the dock rows are byte-identical: the editor and footer did not move"
    );
    // The separator above the editor survives too.
    let separator = &at_bottom[bottom_editor - 1];
    assert!(
        scrolled.iter().any(|row| row == separator),
        "the separator is still there, unchanged: {separator:?}"
    );
    // And the transcript really scrolled: a different slice on top.
    assert_ne!(
        &at_bottom[..bottom_editor],
        &scrolled[..scrolled_editor],
        "the transcript window shows different content"
    );
    let first_content = |rows: &[String]| {
        rows.iter()
            .map(|row| strip(row))
            .find(|row| row.trim().contains("question"))
            .unwrap_or_default()
    };
    assert_ne!(
        first_content(&at_bottom),
        first_content(&scrolled),
        "an earlier line is what the window shows now"
    );
}

// Verifies: gh #35 - pi's scrollbar thumb math (`getScrollbarGeometry`):
// height `round(track^2/content)` floored at 2, offset mapping the
// top-anchored scroll into the thumb's range, hidden when the
// transcript fits its window.
#[test]
fn the_scrollbar_geometry_follows_pis_thumb_math() {
    use crate::chat_render::scrollbar_geometry;

    assert_eq!(scrollbar_geometry(10, 10, 0, 80), None, "it fits");
    assert_eq!(scrollbar_geometry(9, 10, 0, 80), None, "it still fits");
    assert_eq!(scrollbar_geometry(100, 0, 0, 80), None, "no window, no bar");

    // track 10, content 100: thumb = round(100/100) = 1, floored to 2;
    // at the live bottom (scrollTop 90/90) the thumb sits at the end.
    let at_bottom = scrollbar_geometry(100, 10, 0, 80).expect("visible");
    assert_eq!(at_bottom.column, 79, "the rightmost column");
    assert_eq!(at_bottom.rows, 10, "the track is the window");
    assert_eq!(at_bottom.thumb_height, 2, "the floor is 2");
    assert_eq!(at_bottom.thumb_top, 8, "10 - 2 at the bottom");

    // At the top of the content the thumb is at the top of the track.
    let at_top = scrollbar_geometry(100, 10, 90, 80).expect("visible");
    assert_eq!(at_top.thumb_top, 0);

    // Halfway: scrollTop 45 of 90 over a range of 8 rows -> 4.
    let halfway = scrollbar_geometry(100, 10, 45, 80).expect("visible");
    assert_eq!(halfway.thumb_top, 4);

    // A shorter transcript gets a taller thumb: round(100/20) = 5.
    let tall = scrollbar_geometry(20, 10, 0, 80).expect("visible");
    assert_eq!(tall.thumb_height, 5);
}

// Verifies: gh #35 - the scrollbar is painted onto the transcript
// window's right margin and nowhere else: the dock rows keep their full
// width, which is what makes excluding it from a copy a question about
// rows rather than about luck.
#[test]
fn the_scrollbar_paints_only_the_transcript_window() {
    let mut long = chat();
    long.screen_mode = true;
    for i in 0..40 {
        long.transcript.push_user(format!("question {i}"));
        long.transcript.append_text(&format!("answer {i}"));
        long.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let geometry = long
        .scrollbar_for_frame(w, h, 0)
        .expect("40+ transcript lines in a 20-odd row window");
    let rows = long.viewport(w, h, 0);
    for row in rows.iter().take(geometry.rows as usize) {
        let strip = lca_tui::engine::text::strip_terminal_sequences(row);
        assert!(
            strip.ends_with('│')
                || strip.ends_with('┃')
                || strip.ends_with('▲')
                || strip.ends_with('▼'),
            "every transcript-window row carries the bar or a stepper: {strip:?}"
        );
    }
    for row in rows.iter().skip(geometry.rows as usize) {
        let strip = lca_tui::engine::text::strip_terminal_sequences(row);
        assert!(
            !strip.ends_with('│')
                && !strip.ends_with('┃')
                && !strip.ends_with('▲')
                && !strip.ends_with('▼'),
            "no dock row carries it: {strip:?}"
        );
    }
    // A short transcript fits: no scrollbar at all.
    let mut short = chat();
    short.screen_mode = true;
    short.transcript.push_user("one line");
    assert!(
        short.scrollbar_for_frame(w, h, 0).is_none(),
        "hidden when it fits"
    );
}

// Verifies: gh #35 - the jump indicator shows exactly while scrolled
// away from the live bottom, and it names the key that returns there
// (pi's `↓ Jump to latest message · <key>` shape).
#[test]
fn the_jump_indicator_shows_while_scrolled_and_names_its_key() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let strip = |rows: &[String]| {
        rows.iter()
            .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        !strip(&chat.viewport(w, h, 0)).contains("Jump to latest message"),
        "no indicator at the live bottom"
    );
    let scrolled = chat.viewport(w, h, 12);
    let text = strip(&scrolled);
    assert!(
        text.contains("Jump to latest message"),
        "the indicator appears while scrolled up:\n{text}"
    );
    let key = lca_tui::engine::keybindings::key_text("tui.altScreen.bottom");
    assert!(text.contains(&key), "and names the key {key}: {text}");
}

// Verifies: gh #35 - scroll holds the reader's place across new output
// the way pi's ScrollView does (follow-end at the bottom, a fixed view
// away from it), and clamps to what the transcript can show.
#[test]
fn scroll_holds_the_readers_place_across_new_output() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);

    // At the live bottom, growth keeps following it: scroll stays 0.
    assert_eq!(chat.clamp_scroll(0, w, h), 0);
    for i in 40..45 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.finish_assistant();
    }
    assert_eq!(
        chat.clamp_scroll(0, w, h),
        0,
        "follow-end never fights growth"
    );

    // Scrolled up, the same line stays at the top of the window while
    // the transcript grows under it - pi's top-anchored hold, expressed
    // in bottom coordinates. Without the hold, arriving lines drag the
    // reader's place down the document.
    let up = chat.clamp_scroll(5, w, h);
    assert_eq!(
        up, 5,
        "the first frame after scrolling records the position"
    );
    let top_before = chat.viewport(w, h, up);
    for i in 45..48 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.finish_assistant();
    }
    let held = chat.clamp_scroll(5, w, h);
    let top_after = chat.viewport(w, h, held);
    assert_eq!(
        top_before.first(),
        top_after.first(),
        "the same line is still at the top of the window while content grows"
    );

    // And it never scrolls past the start: clamped, the window opens on
    // the very first line of the transcript.
    let clamped = chat.clamp_scroll(9_999, w, h);
    let at_top = chat.viewport(w, h, clamped);
    assert!(
        at_top.iter().any(|row| {
            lca_tui::engine::text::strip_terminal_sequences(row).contains("question 0")
        }),
        "clamped to the start of the transcript"
    );
}

// Verifies: gh #35 - the prompt jump's scroll is a transcript
// coordinate: the dock below the transcript is in neither count, so the
// jump lands where the old whole-document arithmetic (now used by
// nothing) would have overshot by the dock's height.
#[test]
fn the_prompt_jump_measures_scrolls_against_the_transcript() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    chat.jump_target = Some(0);
    let scroll = chat
        .take_jump_scroll(w, h)
        .expect("a pending jump returns a target");
    let total = chat.transcript_len(w) as u16;
    let window = chat.window_height(w, h) as u16;
    assert_eq!(
        scroll,
        total.saturating_sub(window),
        "target 0 pins the transcript's start to the top row, in transcript rows"
    );
}

// Verifies: gh #175 - the popup is bounded: with 30 commands `/` offers
// 30, but the dock renders at most five suggestion rows, and on an
// 80x24 frame the editor and footer stay on screen.
#[test]
fn the_popup_never_pushes_the_editor_offscreen() {
    use super::Chat;
    use super::options;
    use std::sync::Arc;
    let mut opts = options();
    opts.slash_commands = (0..30).map(|i| format!("/cmd{i:02}")).collect();
    let mut chat = Chat::new(
        opts,
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.screen_mode = true;
    chat.handle_key("/");
    let total = chat
        .editor
        .suggestions()
        .map(|s| s.items.len())
        .unwrap_or(0);
    assert!(total > 5, "the test needs more offers than fit: {total}");
    let rows = chat.editor.render_popup(80);
    assert!(
        rows.len() <= 5,
        "at most five suggestion rows: {}",
        rows.len()
    );
    let frame = chat.viewport(80, 24, 0);
    assert_eq!(frame.len(), 24, "the frame fills the viewport");
    let strip = |row: &String| lca_tui::engine::text::strip_terminal_sequences(row);
    assert!(
        frame
            .iter()
            .any(|row| strip(row).trim_start().starts_with("> /")),
        "the prompt editor is on screen"
    );
    assert!(
        frame.iter().any(|row| strip(row).contains("interrupt")),
        "the footer is on screen"
    );
}

// Verifies: gh #175 - the rolling window centers on the selection: the
// selected row always renders, and the window slides as it moves.
#[test]
fn the_popup_window_follows_the_selection() {
    use super::Chat;
    use super::options;
    use std::sync::Arc;
    let mut opts = options();
    opts.slash_commands = (0..30).map(|i| format!("/cmd{i:02}")).collect();
    let mut chat = Chat::new(
        opts,
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.handle_key("/");
    assert_eq!(chat.editor.popup_window(), Some((0, 5)));
    chat.editor.move_suggestion(4);
    assert_eq!(chat.editor.popup_window(), Some((2, 7)));
    chat.editor.move_suggestion(25);
    let (start, end) = chat.editor.popup_window().expect("a window");
    assert_eq!(end - start, 5, "still five rows at the bottom");
    assert!(
        (start..end).contains(&chat.editor.suggestion_index()),
        "the selection stays inside"
    );
    let rows = chat.editor.render_popup(80);
    assert!(
        rows.iter().any(|row| row.starts_with("▸ ")),
        "one row is marked"
    );
}

// Verifies: gh #175 - the selected popup row carries a background SGR
// (the `SelectedBg` role); unselected rows carry none.
#[test]
fn the_selected_popup_row_is_highlighted() {
    use super::Chat;
    use super::options;
    use std::sync::Arc;
    let mut opts = options();
    opts.slash_commands = (0..30).map(|i| format!("/cmd{i:02}")).collect();
    // Styling on: the plain theme renders no escapes at all (FR-UI-5),
    // so the highlight has nothing to assert under it.
    opts.plain = false;
    let mut chat = Chat::new(
        opts,
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.screen_mode = true;
    chat.handle_key("/");
    chat.editor.move_suggestion(1);
    let frame = chat.viewport(80, 24, 0);
    let strip = |row: &String| lca_tui::engine::text::strip_terminal_sequences(row);
    let popup: Vec<&String> = frame
        .iter()
        .filter(|row| {
            let plain = strip(row);
            plain.starts_with("▸ ") || plain.starts_with("  /cmd")
        })
        .collect();
    assert!(!popup.is_empty(), "popup rows render");
    let (selected, plain): (Vec<&&String>, Vec<&&String>) =
        popup.iter().partition(|row| strip(row).starts_with("▸ "));
    assert_eq!(selected.len(), 1, "exactly one row is selected");
    assert!(
        selected[0].contains("48;"),
        "the selected row carries a background SGR: {:?}",
        selected[0]
    );
    assert!(
        plain.iter().all(|row| !row.contains("48;")),
        "unselected rows carry no background"
    );
}

// Verifies: gh #175 - clicking a popup row applies that completion.
#[test]
fn clicking_a_popup_row_applies_it() {
    use super::Chat;
    use super::options;
    use crate::chat_render::ClickOutcome;
    use std::sync::Arc;
    let mut opts = options();
    opts.slash_commands = (0..30).map(|i| format!("/cmd{i:02}")).collect();
    let mut chat = Chat::new(
        opts,
        Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.screen_mode = true;
    chat.handle_key("/");
    let (w, h) = (80u16, 24u16);
    let frame = chat.viewport(w, h, 0);
    let strip = |row: &String| lca_tui::engine::text::strip_terminal_sequences(row);
    let target = chat.editor.suggestions().expect("offers").items[2]
        .label
        .clone();
    let row = frame
        .iter()
        .position(|line| strip(line).contains(&target))
        .expect("the third offer renders") as u16;
    assert!(matches!(
        chat.click_at(4, row, 0, w, h),
        ClickOutcome::SuggestionAccepted
    ));
    assert!(
        chat.editor.text().starts_with(&format!("{target} ")),
        "the offer applied: {:?}",
        chat.editor.text()
    );
}

// Verifies: gh #173 - jumping pins the prompt's first line to the
// viewport's top row (not the middle): scroll leaves exactly
// `target + window` rows below the cut.
#[test]
fn jump_pins_the_prompt_to_the_top_row() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let offsets = chat.transcript.user_offsets(80, &chat.theme);
    let target = offsets[10];
    chat.jump_target = Some(target);
    let scroll = chat
        .take_jump_scroll(w, h)
        .expect("a pending jump returns a target");
    let total = chat.transcript_len(w);
    let window = chat.window_height(w, h);
    assert_eq!(
        scroll as usize,
        total.saturating_sub(target + window),
        "the prompt lands on row 0"
    );
}

// Verifies: gh #173 - the scrollbar's end cells are steppers (▲ top,
// ▼ bottom) while it is active.
#[test]
fn scrollbar_end_cells_are_steppers() {
    use super::strip;
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let window = chat.window_height(w, h);
    assert!(window > 2, "a real window to step in");
    let frame = strip(&chat.viewport(w, h, 0));
    let top = frame[0].trim_end();
    assert!(
        top.ends_with('▲'),
        "the track opens with a stepper: {top:?}"
    );
    let bottom = frame[window - 1].trim_end();
    assert!(
        bottom.ends_with('▼'),
        "the track closes with a stepper: {bottom:?}"
    );
}

// Verifies: gh #173 - clicking the steppers jumps between prompts
// (the bottom stepper wins its cell over the jump indicator).
#[test]
fn stepper_clicks_jump_between_prompts() {
    use crate::chat_render::ClickOutcome;
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let window = chat.window_height(w, h) as u16;
    assert_eq!(
        chat.click_at(w - 1, 0, 0, w, h),
        ClickOutcome::PreviousPrompt,
        "▲ steps back"
    );
    assert_eq!(
        chat.click_at(w - 1, window - 1, 5, w, h),
        ClickOutcome::NextPrompt,
        "▼ steps forward, even scrolled"
    );
    // Off the steppers, the map is unchanged: mid-track ignores, the
    // last row off-track still returns to live.
    assert_eq!(
        chat.click_at(w - 1, 3, 0, w, h),
        ClickOutcome::Ignored,
        "mid-track is not a stepper"
    );
    assert_eq!(
        chat.click_at(0, window - 1, 5, w, h),
        ClickOutcome::JumpBottom,
        "the indicator keeps its row"
    );
}

// Verifies: gh #173 - Shift+Up/Down step through prompts like Alt+Up/Down.
#[test]
fn shift_up_and_down_jump_between_prompts() {
    use crate::state::Action;
    let mut chat = chat();
    chat.world.resize(80, 24);
    chat.transcript.push_user("first question");
    chat.transcript.append_text("an answer");
    chat.transcript.finish_assistant();
    chat.transcript.push_user("second question");
    assert_eq!(chat.handle_key("\x1b[1;2A"), Action::Continue); // Shift+Up
    assert!(chat.jump_target.is_some(), "shift+up steps back");
    assert_eq!(chat.handle_key("\x1b[1;2B"), Action::Continue); // Shift+Down
    assert!(chat.jump_target.is_some(), "shift+down steps forward");
}

// Verifies: gh #82 - the scrollbar mode bites: hidden paints nothing,
// always paints a full bar even when the transcript fits.
#[test]
fn scrollbar_mode_hides_or_forces_the_bar() {
    use super::Chat;
    use super::options;
    use super::strip;
    use crate::state::DisplayTuning;
    use lca_tui::engine::keybindings::KeybindingsManager;
    use std::sync::Arc;
    let tuned = |scrollbar: &str| {
        let mut opts = options();
        let mode = scrollbar.to_string();
        opts.hooks.display_tuning = Some(Arc::new(move || DisplayTuning {
            fullscreen_scrollbar: mode.clone(),
            ..DisplayTuning::default()
        }));
        let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
        chat.screen_mode = true;
        chat.transcript.push_user("one line");
        chat
    };
    let (w, h) = (80u16, 24u16);
    let plain = tuned("auto");
    assert!(
        plain.scrollbar_for_frame(w, h, 0).is_none(),
        "a fitting transcript hides the auto bar"
    );
    let hidden = tuned("hidden");
    assert!(
        hidden.scrollbar_for_frame(w, h, 0).is_none(),
        "hidden paints nothing"
    );
    let mut forced = tuned("always");
    forced.transcript.push_user("one line");
    let frame = strip(&forced.viewport(w, h, 0));
    assert!(
        frame.iter().any(|line| line.trim_end().ends_with('▲')),
        "always paints the steppers: {frame:?}"
    );
    assert!(
        frame.iter().any(|line| line.trim_end().ends_with('▼')),
        "always paints both ends: {frame:?}"
    );
}

// Verifies: gh #164 - a Down on the scrollbar track (not the stepper
// cells) starts a drag; the steppers keep their prompt-jump clicks,
// and other columns miss.
#[test]
fn scrollbar_track_and_stepper_hit_test() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    assert_eq!(
        chat.scrollbar_hit(79, 5, w, h, 0),
        ScrollbarHit::Track,
        "a track row starts a drag"
    );
    assert_eq!(
        chat.scrollbar_hit(79, 0, w, h, 0),
        ScrollbarHit::Stepper,
        "the ▲ cell keeps its jump click"
    );
    assert_eq!(
        chat.scrollbar_hit(5, 5, w, h, 0),
        ScrollbarHit::Miss,
        "content columns miss"
    );
}

// Verifies: gh #164 - a drag maps pointer Y to scroll (pi's
// `scrollScrollbarToPointer`): the track top holds the oldest lines,
// the track bottom returns to live.
#[test]
fn scrollbar_drag_maps_pointer_y_to_scroll() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let top = chat
        .scrollbar_drag_scroll(w, h, 0, 0, 0)
        .expect("a drag maps");
    assert!(top > 0, "the track top scrolls away from live");
    let frame = chat.viewport(w, h, top);
    let plain: Vec<String> = frame
        .iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect();
    assert!(
        plain.iter().any(|row| row.contains("question 0")),
        "the oldest exchange is on screen: {plain:?}"
    );
    let bottom = chat
        .scrollbar_drag_scroll(w, h, top, h - 1, 0)
        .expect("a drag maps");
    assert_eq!(bottom, 0, "the track bottom returns to live");
}

// Verifies: gh #164 - a hovered or dragged thumb paints solid (pi's
// active `"█"` over the idle `"┃"`).
#[test]
fn a_hovered_thumb_paints_solid() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let idle: String = chat.viewport(w, h, 0).join("\n");
    assert!(idle.contains('┃'), "idle thumbs paint thin");
    chat.scrollbar_hover = true;
    let hovered: String = chat.viewport(w, h, 0).join("\n");
    assert!(hovered.contains('█'), "a hovered thumb paints solid");
}

// Verifies: gh #164 - hover refreshes on every event, so a release off
// the track clears a stale solid thumb.
#[test]
fn hover_clears_when_the_pointer_leaves_the_track() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript.push_user(format!("question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    chat.refresh_scrollbar_hover(79, 5, w, h, 0);
    assert!(chat.scrollbar_hover, "on the track hovers");
    chat.refresh_scrollbar_hover(5, 5, w, h, 0);
    assert!(!chat.scrollbar_hover, "off the track clears");
}

// Verifies: gh #165 - the editor owns the rows above the footer, full
// width, so clicks there focus the composer.
#[test]
fn editor_rect_pins_the_composer_rows() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.editor.set_text("hello");
    let (w, h) = (80u16, 24u16);
    let (top, len) = chat.editor_rect(w, h).expect("the editor is on screen");
    let frame = chat.viewport(w, h, 0);
    let plain: Vec<String> = frame
        .iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect();
    assert!(
        plain[top as usize].trim_start().starts_with("> hello"),
        "the rect opens on the prompt row: {plain:?}"
    );
    assert_eq!(len, 1, "one buffer row paints as one editor row");
    assert!(!chat.editor_rect(w, h).is_none());
}

// Verifies: gh #165 - clicking the editor rows places the caret under
// the pointer (the zero-width cursor marker names the column).
#[test]
fn clicking_the_editor_places_the_caret() {
    use lca_tui::engine::core::CURSOR_MARKER;
    let mut chat = chat();
    chat.screen_mode = true;
    chat.editor.set_text("hello world");
    let (w, h) = (80u16, 24u16);
    let (top, _) = chat.editor_rect(w, h).expect("the editor is on screen");
    assert!(chat.place_editor_caret(2 + 5, top, w, h), "the click lands");
    let rows = chat.editor.render(w - 2);
    let marked = rows
        .iter()
        .find(|row| row.contains(CURSOR_MARKER))
        .expect("the caret paints");
    let column = marked.find(CURSOR_MARKER).expect("marker present");
    assert_eq!(column, 5, "the caret sits under the click: {marked:?}");
}

// Verifies: gh #165 - clicking the editor while a picker owns the
// keyboard dismisses the picker and still places the caret.
#[test]
fn clicking_the_editor_dismisses_an_open_picker() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.editor.set_text("hello");
    chat.tree_picker = Some(crate::chat_pickers::TreePicker {
        entries: vec![("a".to_string(), "first".to_string())],
        selected: 0,
    });
    let (w, h) = (80u16, 24u16);
    let (top, _) = chat.editor_rect(w, h).expect("the editor is on screen");
    assert!(chat.place_editor_caret(2, top, w, h), "the click lands");
    assert!(chat.tree_picker.is_none(), "the picker dismissed");
}

// Verifies: gh #165 - a click below the editor (footer rows) is not an
// editor click: the picker stays open for its own backdrop path.
#[test]
fn footer_clicks_are_not_editor_clicks() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.editor.set_text("hello");
    chat.tree_picker = Some(crate::chat_pickers::TreePicker {
        entries: vec![("a".to_string(), "first".to_string())],
        selected: 0,
    });
    let (w, h) = (80u16, 24u16);
    assert!(
        !chat.place_editor_caret(2, h - 1, w, h),
        "the footer row misses"
    );
    assert!(chat.tree_picker.is_some(), "the picker stays open");
}

// Verifies: gh #165 - the settings selector counts as an open picker,
// so an editor click dismisses it too.
#[test]
fn clicking_the_editor_dismisses_settings() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.editor.set_text("hello");
    chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
        rows: vec![],
        selected: 0,
        editing: None,
    });
    assert!(chat.picker_open(), "settings owns the keyboard");
    let (w, h) = (80u16, 24u16);
    let (top, _) = chat.editor_rect(w, h).expect("the editor is on screen");
    assert!(chat.place_editor_caret(2, top, w, h), "the click lands");
    assert!(chat.settings_picker.is_none(), "settings dismissed");
}

// Verifies: gh #166 - clicking a tool card's header row toggles that
// card's expansion (the jump indicator and thinking rows keep theirs).
#[test]
fn clicking_a_tool_header_expands_that_card() {
    use crate::chat_render::ClickOutcome;
    use crate::transcript::ToolStatus;
    let mut chat = chat();
    chat.screen_mode = true;
    chat.transcript.start_tool("bash", "ls");
    chat.transcript
        .finish_tool(ToolStatus::Ok, Some("a\nb\nc\nd\ne\nf".to_string()));
    let (w, h) = (80u16, 24u16);
    let frame = chat.viewport(w, h, 0);
    let strip = |row: &String| lca_tui::engine::text::strip_terminal_sequences(row);
    let row = frame
        .iter()
        .position(|line| strip(line).contains("bash"))
        .expect("the card header renders") as u16;
    assert!(matches!(
        chat.click_at(4, row, 0, w, h),
        ClickOutcome::ToolToggled
    ));
    let expanded = strip(&chat.viewport(w, h, 0).join("\n"));
    assert!(
        expanded.lines().any(|line| line.trim() == "f"),
        "the full result shows: {expanded:?}"
    );
}

// Verifies: gh #238 - the jump indicator splices at the seam: over
// italic transcript rows the label still opens with a reset (no
// bleed in) and closes with one (no bleed out).
#[test]
fn the_jump_indicator_isolates_sgr_state_over_italic_rows() {
    let mut chat = chat();
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript
            .push_user(format!("*thinking* question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let scrolled = chat.viewport(w, h, 12);
    let row = scrolled
        .iter()
        .find(|row| row.contains("Jump to latest message"))
        .expect("the indicator paints while scrolled");
    let reset = lca_tui::engine::core::SEGMENT_RESET;
    let label_at = row.find("Jump to latest message").expect("label");
    // Entry reset before the label: everything left of it may carry
    // the transcript's italic, the label itself starts clean.
    let entry = row[..label_at].rfind(reset).expect("entry reset");
    assert!(
        !row[entry + reset.len()..label_at].contains("\x1b[3m"),
        "no italic reaches the label: {row:?}"
    );
    // Exit reset after the label's band: the band cannot leak right.
    let band_end = label_at + "Jump to latest message".len();
    assert!(
        row[band_end..].starts_with(reset) || row[band_end..].contains(reset),
        "the band closes: {row:?}"
    );
}
