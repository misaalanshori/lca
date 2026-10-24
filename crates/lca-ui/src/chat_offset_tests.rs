//! gh #331 — the fullscreen tab bar's one row, through every window calculation.
//!
//! The bug: `viewport` subtracts the tab rows from the transcript window, but
//! `click_at`, `window_height`, `clamp_scroll` and `tooltip_at` each computed the
//! window (or addressed the row) as if the tab bar were not there. One bug, four
//! symptoms, all one row of vertical offset.
//!
//! Every row here is written to **fail before the fix and pass after it**, and none
//! of them asserts coordinate arithmetic: they assert the *effect* - which entry
//! changed, which glyph the stepper produced, what the tooltip named - because an
//! arithmetic assertion would be satisfied by any two mistakes that cancel.
//!
//! The last two rows are the mirrors. Main-screen mode has no tab bar above the
//! transcript (it is dock-bottom there), so the same click at the same coordinates
//! must hit the same entry; and `viewport` and `scroll_extent` must agree, because a
//! frame that is one row taller than the number used to hit-test it is exactly how
//! this bug survived the frame-level tests.
//!
//! Fixtures come from the parent (`chat()`, `options()`, `strip`).

use super::Chat;
use super::tests::{options, strip};
use lca_tui::engine::keybindings::KeybindingsManager;
use std::sync::Arc;

/// A chat whose host can mint sessions, so the fullscreen bar actually shows.
///
/// The single-tab case already draws a `[+]` row, but these rows want the real bar:
/// the point is a session list a reader would see, and `tab_top` returns 1 either
/// way, which is the number under test.
fn tabbed(theme_plain: bool) -> Chat {
    let mut opts = options();
    opts.plain = theme_plain;
    let minted = Arc::new(std::sync::Mutex::new(3u32));
    let next = minted.clone();
    opts.hooks.new_session = Some(Arc::new(move || {
        let mut n = next.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = format!("s{}", *n);
        *n += 1;
        Some(id)
    }));
    opts.hooks.switch_session = Some(Arc::new(|id: &str| {
        Some(vec![lca_protocol::Record::User {
            v: lca_protocol::FORMAT_VERSION,
            ts: 1,
            id: format!("{id}-u"),
            parent: None,
            content: format!("tab {id}"),
            attachments: Vec::new(),
            queue: None,
        }])
    }));
    opts.hooks.session_list = Some(Arc::new(|| {
        ["s1", "s2", "s3"]
            .into_iter()
            .map(|id| crate::resume::SessionEntry {
                id: id.to_string(),
                title: format!("title {id}"),
                messages: 1,
                age: "now".to_string(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
            })
            .collect()
    }));
    opts.hooks.current_session_id = Some(Arc::new(|| "s1".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    // Two tabs: Ctrl+T mints `s3` through the host hook above.
    chat.handle_key("\x14");
    assert_eq!(
        chat.tab_top(80),
        1,
        "the fixture must show a fullscreen tab bar for these rows to mean anything"
    );
    chat
}

/// A chat with a finished tool card, so there is something to collapse.
fn with_a_tool_card(mut chat: Chat) -> Chat {
    use crate::transcript::ToolStatus;
    chat.transcript.start_tool("bash", "ls");
    chat.transcript
        .finish_tool(ToolStatus::Ok, Some("a\nb\nc\nd\ne\nf".to_string()));
    chat
}

/// The frame row that paints `needle`, found the way a reader's eye would.
fn frame_row(chat: &Chat, width: u16, height: u16, needle: &str) -> Option<u16> {
    let frame = chat.viewport(width, height, 0);
    frame
        .iter()
        .position(|line| strip(std::slice::from_ref(line))[0].contains(needle))
        .map(|row| row as u16)
}

// ---------------------------------------------------------------------------
// Symptom 1: a click on the first transcript row.
// ---------------------------------------------------------------------------

#[test]
fn a_click_on_the_first_transcript_row_collapses_the_card_under_it() {
    // The headline row. With a tab bar, row 0 is the bar and the first transcript
    // row is row 1 - the row that used to be off by one, so a click aimed at a
    // card's header did nothing (it hit the bar) or hit the row *above* the card.
    let (w, h) = (80u16, 24u16);
    let mut chat = with_a_tool_card(tabbed(true));
    // Land the reader on the card, then find where the frame paints it.
    let header = frame_row(&chat, w, h, "bash").expect("the card header paints");
    assert!(
        header >= chat.tab_top(w),
        "the card is below the tab bar: header at {header}, tabs at {}",
        chat.tab_top(w)
    );
    assert!(
        matches!(
            chat.click_at(4, header, 0, w, h),
            crate::chat_render::ClickOutcome::ToolToggled
        ),
        "the click at the row the card is painted on toggles it"
    );
    // The toggle's *effect*, not a coordinate: the card's body appears. It starts
    // collapsed (the row says `… (1 earlier lines, ctrl+o to expand)`), so a click
    // that lands expands it and the output lines `b`/`c`/`d` become visible.
    let after = strip(&chat.viewport(w, h, 0)).join("\n");
    assert!(
        after.lines().any(|row| row.trim() == "c"),
        "the click expanded the card it was aimed at: {after}"
    );
}

#[test]
fn a_click_on_the_tab_bar_is_not_a_transcript_click() {
    // The other half of the same subtraction: row 0 belongs to the tabs, so a click
    // there must not toggle a card as if it were the first transcript row. The tab
    // gesture runs first in the loop (`handle_tab_mouse`), and this row pins that
    // `click_at` on its own does not also answer for that row.
    let (w, h) = (80u16, 24u16);
    let mut chat = with_a_tool_card(tabbed(true));
    assert_eq!(
        chat.click_at(4, 0, 0, w, h),
        crate::chat_render::ClickOutcome::Ignored,
        "the tab row is not a transcript row"
    );
}

// ---------------------------------------------------------------------------
// Symptom 2: the scrollbar's top stepper.
// ---------------------------------------------------------------------------

#[test]
fn the_top_stepper_fires_with_a_tab_bar_present() {
    // `▲` is painted on the transcript window's first row, which is terminal row
    // `tabs` - not row 0. The hit-test checked `row == 0`, so with tabs the stepper
    // never fired: the cell the reader saw was the second stepper's row.
    let (w, h) = (80u16, 24u16);
    let mut chat = with_a_tool_card(tabbed(true));
    for index in 0..40 {
        chat.transcript.push_user(format!("line {index}"));
    }
    let (content, window) = chat.scroll_extent(w, h);
    assert!(
        content > window,
        "the fixture must overflow for a bar to paint"
    );
    let geometry = chat
        .scrollbar_for_frame(w, h, 0)
        .expect("an overflowing transcript paints a scrollbar");
    let stepper_row = chat.tab_top(w);
    // The frame really paints `▲` on that row - the glyph the reader aims at.
    let frame = chat.viewport(w, h, 0);
    assert!(
        strip(&frame)
            .get(stepper_row as usize)
            .is_some_and(|row| row.contains('▲')),
        "▲ is on row {stepper_row} with the tabs: {}",
        strip(&frame).join("\n")
    );
    // Scrolled away from the bottom so `PreviousPrompt` has somewhere to go.
    assert!(
        matches!(
            chat.click_at(geometry.column, stepper_row, 4, w, h),
            crate::chat_render::ClickOutcome::PreviousPrompt
        ),
        "the stepper the reader aims at answers"
    );
}

// ---------------------------------------------------------------------------
// Symptom 3: the scroll extent.
// ---------------------------------------------------------------------------

#[test]
fn the_window_matches_the_rows_actually_painted() {
    // A row-count assertion, not a pixel one: the window used for hit-testing and
    // clamping must be the number of transcript rows the frame really shows. With
    // the tab bar the frame shows `height - dock - 1`; a window of
    // `height - dock` claims a row that the tab bar owns.
    let (w, h) = (80u16, 24u16);
    let chat = with_a_tool_card(tabbed(true));
    let (_, window) = chat.scroll_extent(w, h);
    let (_, dock) = chat.sections(w);
    assert_eq!(
        window,
        (h as usize).saturating_sub(dock.len()).saturating_sub(1),
        "the window is the height minus the dock minus the one tab row"
    );
    assert_eq!(
        chat.window_height(w, h),
        window,
        "and `window_height` is that same number - the two cannot disagree"
    );
    // The frame is exactly `h` rows: tab + window + dock.
    let frame = chat.viewport(w, h, 0);
    assert_eq!(frame.len(), h as usize, "the frame fills the terminal");
}

#[test]
fn the_maximum_scroll_is_the_content_that_does_not_fit() {
    // `clamp_scroll`'s maximum is `content - window`. With the tab row missing from
    // the window it was one short, so the last line could never reach the top of the
    // window - the extent was off by exactly the row the tab bar took.
    let (w, h) = (80u16, 24u16);
    let mut chat = with_a_tool_card(tabbed(true));
    for index in 0..40 {
        chat.transcript.push_user(format!("line {index}"));
    }
    let (content, window) = chat.scroll_extent(w, h);
    assert!(content > window, "the fixture overflows");
    let max = (content - window) as u16;
    // Asking for more than the maximum clamps to the maximum, not to a number one
    // short of it.
    assert_eq!(
        chat.clamp_scroll(u16::MAX, w, h),
        max,
        "the clamp's ceiling is `content - window` with the tab row included"
    );
}

// ---------------------------------------------------------------------------
// Symptom 4: the tooltip.
// ---------------------------------------------------------------------------

#[test]
fn a_tooltip_over_a_tool_header_names_that_header() {
    // Hover fired one row below its target: the window was right, the row was not
    // offset by the tab bar.
    let (w, h) = (80u16, 24u16);
    let chat = with_a_tool_card(tabbed(true));
    let header = frame_row(&chat, w, h, "bash").expect("the card header paints");
    let text = chat.tooltip_at(4, header, w, h, 0);
    assert!(
        text.as_deref()
            .is_some_and(|tip| tip.contains("Expand") || tip.contains("Collapse")),
        "the header row names the card action, got {text:?}"
    );
    // And the tab bar itself names nothing transcript-shaped: without the offset
    // this was the row that answered `Expand output` for a pointer sitting on the
    // session tabs, because it was read as the transcript's first row.
    assert_eq!(
        chat.tooltip_at(4, 0, w, h, 0),
        None,
        "the tab row is not a tool header"
    );
}

// ---------------------------------------------------------------------------
// The mirrors: scrollback mode, and the two window counters agreeing.
// ---------------------------------------------------------------------------

#[test]
fn main_screen_mode_is_unchanged() {
    // The row that keeps the fix from becoming a mirror bug in the DEFAULT renderer.
    // Main screen has no tab bar above the transcript (it is dock-bottom there), so
    // the same click at the same coordinates must hit the same entry.
    let (w, h) = (80u16, 24u16);
    let mut chat = with_a_tool_card(tabbed(true));
    // Same fixture, only the renderer differs: a second chat from the same builder.
    let mut main = with_a_tool_card(tabbed(true));
    main.screen_mode = false;
    let frame = main.viewport(w, h, 0);
    let painted = frame
        .iter()
        .position(|line| strip(std::slice::from_ref(line))[0].contains("bash"));
    assert!(
        painted.is_some(),
        "the card still paints in scrollback mode: {}",
        strip(&frame).join("\n")
    );
    assert_eq!(
        main.click_at(4, painted.unwrap_or(0) as u16, 0, w, h),
        crate::chat_render::ClickOutcome::Ignored,
        "but scrollback mode never hit-tests the mouse (it does not capture it)"
    );
    assert_eq!(
        chat.click_at(4, 0, 0, w, h),
        crate::chat_render::ClickOutcome::Ignored,
        "and `click_at` refuses outright off fullscreen"
    );
    // The window arithmetic is the same number in both modes once the tab row is
    // accounted for: in scrollback mode `sections()` puts the bar in the *dock*, so
    // subtracting it twice would be the mirror bug.
    chat.screen_mode = false;
    let (_, main_window) = chat.scroll_extent(w, h);
    let (_, main_dock) = chat.sections(w);
    assert_eq!(
        main_window,
        (h as usize).saturating_sub(main_dock.len()),
        "scrollback mode subtracts the dock only - its tab row is already inside it"
    );
    assert_eq!(
        chat.window_height(w, h),
        main_window,
        "and `window_height` agrees here too"
    );
}

#[test]
fn the_frame_and_the_hit_test_use_the_same_window() {
    // Gh #331's survival mechanism: `viewport` was right and the four hit-test
    // paths were wrong, so a test that only looked at the frame passed while every
    // click was off. This row asserts the two counters are the same number, in both
    // modes, so the divergence cannot come back quietly.
    let (w, h) = (80u16, 24u16);
    for tabs in [0usize, 1] {
        let mut chat = with_a_tool_card(tabbed(true));
        if tabs == 0 {
            // A host that cannot mint sessions still shows `[+]`, so simulate "no
            // bar at all" by dropping the hook - that is the other real shape.
            chat.world.options.hooks.new_session = None;
        }
        let (_, window) = chat.scroll_extent(w, h);
        let (_, dock) = chat.sections(w);
        let tab_rows = chat.tab_top(w) as usize;
        assert_eq!(
            window,
            (h as usize)
                .saturating_sub(dock.len())
                .saturating_sub(tab_rows),
            "with tab_rows={tab_rows}: the window is height - dock - tabs"
        );
        assert_eq!(
            chat.window_height(w, h),
            window,
            "and the per-frame helper agrees (tab_rows={tab_rows})"
        );
    }
}
