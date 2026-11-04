//! The terminal colour query's lifecycle (gh #379 gap 1): the `DA1` sentinel
//! closes the window, a reply is applied only inside it (plus a short grace for a
//! slow link), and a stray `OSC 11` from another program cannot repaint the theme.
//!
//! Split out of `run_tests.rs` when that file crossed gate 11's 1,200-line ceiling
//! with these rows in it. Behaviour unchanged.

use super::*;
use lca_tui::engine::terminal::FakeTerminal;

// ============================================================================
// gh #379 gap 1 - the OSC 11 query lifecycle: a sentinel closes it, a reply only
// counts while it is open, and a stray one never repaints the theme
// ============================================================================

/// A fullscreen chat on the **auto** theme, which is the only one that follows
/// the terminal, plus a screen and a terminal to drive `handle_input` with.
fn autotheme_chat() -> (crate::chat::Chat, Screen, FakeTerminal) {
    let mut options = super::tests::options();
    options.plain = false;
    let mut chat = Chat::new(
        options,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.screen_mode = true;
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    (chat, screen, term)
}

/// The user band's colour, which is what a reader actually sees change when the
/// theme follows the terminal.
fn band(chat: &crate::chat::Chat) -> String {
    format!(
        "{:?}",
        chat.theme.palette.get(crate::theme::Role::UserMessageBg)
    )
}

// Verifies: gh #379 gap 1 - the startup query is **opened**, so a reply is
// expected. Without this flag the very first (correct) reply would be treated as
// stray and thrown away, which is exactly how "fix the stray case" turns into a
// regression on every normal terminal.
#[test]
fn the_startup_query_is_open_when_the_loop_begins() {
    let (chat, _screen, _term) = autotheme_chat();
    assert!(
        chat.theme_query_open(),
        "the loop writes the query at startup, so the window must be open"
    );
}

// Verifies: gh #379 gap 1 - the **DA1 sentinel closes** the query without
// changing the palette. That is the whole point of the sentinel: it proves the
// replies are done or absent, so the theme stops listening. It must resolve even
// when the terminal never answered a colour at all.
#[test]
fn the_sentinel_closes_the_query_and_leaves_the_palette_alone() {
    let (mut chat, mut screen, mut term) = autotheme_chat();
    let before = band(&chat);
    super::viewport_keys_tests::press("\x1b[?1;2c", &mut chat, &mut screen, &mut term);
    assert!(
        !chat.theme_query_open(),
        "DA1 proves the replies are done: the window closes"
    );
    assert_eq!(
        band(&chat),
        before,
        "a sentinel carries no colour, so the palette must not change"
    );
}

// Verifies: gh #379 gap 1 - while the query is open, a reply **does** reach the
// palette. This is the "zero cost when the terminal answers" half: a fast
// terminal's answer must land, not be swallowed by the new stray guard.
#[test]
fn a_reply_while_the_query_is_open_updates_the_palette() {
    let (mut chat, mut screen, mut term) = autotheme_chat();
    let before = band(&chat);
    // A clean, unambiguous light-terminal background.
    super::viewport_keys_tests::press("\x1b]11;#ffffff\x07", &mut chat, &mut screen, &mut term);
    assert_ne!(
        band(&chat),
        before,
        "an OSC 11 answer during the query window must reach the palette"
    );
}

// Verifies: gh #379 gap 1 (**the defect this cycle found**) - once the query is
// closed, an `OSC 11` arriving from anywhere else must **not** repaint the theme.
// Any child process that prints one - `ls`, `git`, a colour-picking prompt - would
// otherwise hijack the palette mid-session. pi guards this by consuming a colour
// reply only while a query is pending; before this fix LCA had no lifecycle at
// all, so every stray reply was applied.
#[test]
fn an_osc11_after_the_sentinel_does_not_repaint_the_theme() {
    let (mut chat, mut screen, mut term) = autotheme_chat();
    // Close the query the way a normal terminal does.
    super::viewport_keys_tests::press("\x1b[?1;2c", &mut chat, &mut screen, &mut term);
    let after_close = band(&chat);
    let theme_after_close = chat.theme.name.clone();

    // A subprocess emits its own OSC 11 well after our query finished. The wait
    // matters: a real stray arrives later than the in-flight-late-reply grace,
    // which is exactly what distinguishes it from the SSH case pinned above.
    std::thread::sleep(crate::theme_query::LATE_REPLY_GRACE + std::time::Duration::from_millis(60));
    super::viewport_keys_tests::press("\x1b]11;#ff00ff\x07", &mut chat, &mut screen, &mut term);

    assert_eq!(
        band(&chat),
        after_close,
        "a stray OSC 11 must not repaint the palette"
    );
    assert_eq!(
        chat.theme.name, theme_after_close,
        "...nor switch the theme by name"
    );
}

// Verifies: gh #379 gap 1 - a stray reply is not *typed* either. It is a reply,
// not keystrokes, so it must be consumed silently - the guard and the palette
// rule have to agree, which is why both go through one recogniser.
#[test]
fn a_stray_osc11_is_swallowed_rather_than_typed() {
    let (mut chat, mut screen, mut term) = autotheme_chat();
    super::viewport_keys_tests::press("\x1b[?1;2c", &mut chat, &mut screen, &mut term);
    super::viewport_keys_tests::press("\x1b]11;#ff00ff\x07", &mut chat, &mut screen, &mut term);
    assert_eq!(
        chat.editor.text(),
        "",
        "a reply must never land in the composer"
    );
}

// Verifies: gh #379 gap 1 - **a late answer still lands.** This is the SSH case
// the issue names: the reply arrives after the sentinel closed the window (a slow
// link, or a terminal that answered DA1 first). It must hot-update the palette
// rather than be discarded, because "wait longer" is what the sentinel exists to
// avoid.
#[test]
fn a_late_reply_after_the_sentinel_still_updates_the_palette() {
    let (mut chat, mut screen, mut term) = autotheme_chat();
    super::viewport_keys_tests::press("\x1b[?1;2c", &mut chat, &mut screen, &mut term);
    let before = band(&chat);
    super::viewport_keys_tests::press("\x1b]11;#ffffff\x07", &mut chat, &mut screen, &mut term);
    assert_ne!(
        band(&chat),
        before,
        "a late OSC 11 answer must still reach the palette"
    );
}

// Verifies: gh #379 gap 1 - a terminal that answers **nothing** still gets a
// theme, and gets it without waiting. The floor is the detected/`COLORFGBG`
// scheme, which is resolved synchronously at startup; the loop must be painting
// frames before any reply could arrive. Asserted as "the theme is a real one with
// a palette, before any reply is fed".
#[test]
fn a_silent_terminal_still_gets_a_theme_without_waiting() {
    let (chat, _screen, _term) = autotheme_chat();
    // The theme is resolved from `COLORFGBG`/defaults at construction, so it
    // exists before the loop reads a single byte.
    assert!(!chat.theme.name.is_empty(), "a theme is chosen up front");
    let band_color = chat.theme.palette.get(crate::theme::Role::UserMessageBg);
    assert!(
        !matches!(band_color, crate::theme::Color::Default),
        "and it is a real palette, not an empty one: {band_color:?}"
    );
    // The query window is open but nothing is blocked on it: the chat is usable.
    assert!(
        chat.theme_query_open(),
        "listening, but not waiting - a reply would refine the theme"
    );
}

// ============================================================================
// gh #324 gap 2 - the cell-size probe reaches the renderer
// ============================================================================

// Verifies: gh #324 gap 2 - **a `CellSize` reply changes the scale, and the frames
// laid out against the estimate are dropped.** The second half is the point: an
// image's kitty id hashes its cell scale (gap 1), so a probe that moved the grid
// without invalidating the cache would leave the terminal displaying a texture whose
// geometry no longer matches its placement. The row asserts the scale *and* that the
// transcript re-renders, because a wiring that only stored the value would pass a
// scale assertion and still show a distorted image.
#[test]
fn a_cell_size_reply_installs_the_scale_and_drops_the_estimate_frames() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (mut chat, mut screen, mut term) = autotheme_chat();
    assert_eq!(
        lca_tui::widgets::image::current_cell_pixels(),
        lca_tui::widgets::image::DEFAULT_CELL_PIXELS,
        "nothing has probed yet, so the estimate is in force"
    );

    // Render once at the estimate, so there is a cached frame to invalidate.
    let _ = chat.render(80);
    let epoch_before = chat.transcript.cache_epoch_for_test();

    // A 22-pixel-tall cell: no terminal this box has, which is the point.
    super::viewport_keys_tests::press("\x1b[6;22;9t", &mut chat, &mut screen, &mut term);

    assert_eq!(
        lca_tui::widgets::image::current_cell_pixels(),
        (9.0, 22.0),
        "the reply's own dimensions are installed (width, height)"
    );
    assert_ne!(
        chat.transcript.cache_epoch_for_test(),
        epoch_before,
        "and the estimate's frames are invalidated, so the image is re-rendered \
         with a new id rather than silently re-scaled"
    );
    lca_tui::widgets::image::clear_cell_pixels();
}

// Verifies: gh #324 gap 2 - **the reply is classified as a reply, and is consumed
// before any component sees it.** A query whose answer reaches the composer is
// worse than no query, and the recogniser is the half a "wire the consumer" fix
// misses.
//
// The assertion is deliberately about **what the composer receives**, not about it
// being empty: an unknown CSI is also swallowed by the editor, so
// `editor.text() == ""` passes whether or not the reply was classified. I measured
// that - removing `is_cell_size_report` from `is_startup_query_reply` left the empty
// assertion green. Typing a character after the sequence is what tells the two
// apart: consumed-as-reply means the composer got only the character.
#[test]
fn a_cell_size_reply_is_consumed_before_it_reaches_the_composer() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (mut chat, mut screen, mut term) = autotheme_chat();
    let result =
        super::viewport_keys_tests::press("\x1b[6;22;9t", &mut chat, &mut screen, &mut term);
    assert!(
        matches!(result, InputResult::Continue),
        "the reply is consumed as a reply, not dispatched as input"
    );
    super::viewport_keys_tests::press("x", &mut chat, &mut screen, &mut term);
    assert_eq!(
        chat.editor.text(),
        "x",
        "the composer received the keystroke and not the reply bytes: {:?}",
        chat.editor.text()
    );
    assert_eq!(
        lca_tui::widgets::image::current_cell_pixels(),
        (9.0, 22.0),
        "and the reply was applied, not merely dropped"
    );
    lca_tui::widgets::image::clear_cell_pixels();
}

// Verifies: gh #324 gap 2 - **no probe is not no image.** A terminal that never
// answers keeps the named estimate and still draws, which is the fallback the brief
// requires: the probe improves the picture, it does not gate it.
#[test]
fn without_a_reply_the_estimate_still_renders_an_image() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (mut chat, _screen, _term) = autotheme_chat();
    chat.transcript.push_image(
        lca_tui::widgets::image::ImageInfo::new("image/png", &png_bytes(60, 110)),
        png_bytes(60, 110),
    );
    chat.transcript.invalidate();
    let rows = chat.transcript.render_for_protocol(
        60,
        &chat.theme,
        lca_tui::widgets::image::ImageProtocol::Kitty,
    );
    assert!(
        rows.iter().any(|r| r.contains("\x1b_G")),
        "the image is still placed without a probe: {rows:?}"
    );
    assert_eq!(
        lca_tui::widgets::image::current_cell_pixels(),
        lca_tui::widgets::image::DEFAULT_CELL_PIXELS,
        "and the estimate is what it used"
    );
}

/// A PNG header carrying real dimensions, for the rows above.
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13]);
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[0; 12]);
    png
}

// Verifies: gh #324 gap 2 - **the re-render carries a new id.** The scale is part of
// a kitty image's identity, so a probe must produce a different `i=` in the emitted
// bytes - a new upload - rather than re-placing a texture whose geometry no longer
// matches. Asserting the id rather than "the rows changed" is what makes this the
// pin the brief asks for: the rows can differ for a dozen reasons, the id only for
// the reason under test.
#[test]
fn the_re_render_after_a_probe_emits_a_new_image_id() {
    lca_tui::widgets::image::clear_cell_pixels();
    let (mut chat, mut screen, mut term) = autotheme_chat();
    let bytes = png_bytes(60, 110);
    chat.transcript.push_image(
        lca_tui::widgets::image::ImageInfo::new("image/png", &bytes),
        bytes.clone(),
    );
    chat.transcript.invalidate();

    let id_of = |chat: &crate::chat::Chat| -> Option<u32> {
        let rows = chat.transcript.render_for_protocol(
            60,
            &chat.theme,
            lca_tui::widgets::image::ImageProtocol::Kitty,
        );
        rows.iter().find_map(|row| {
            let at = row.find(",i=")? + 3;
            row[at..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
    };

    let before = id_of(&chat).expect("the estimate frame places the image");
    super::viewport_keys_tests::press("\x1b[6;22;9t", &mut chat, &mut screen, &mut term);
    let after = id_of(&chat).expect("the probed frame places it too");
    assert_ne!(
        before, after,
        "a new cell scale is a new upload: the id moved with the grid"
    );
    // And the grid really moved, so the id change is attributable.
    assert_eq!(lca_tui::widgets::image::current_cell_pixels(), (9.0, 22.0));
    lca_tui::widgets::image::clear_cell_pixels();
}
