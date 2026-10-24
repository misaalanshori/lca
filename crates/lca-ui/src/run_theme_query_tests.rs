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
