//! GitHub issue #33 (released 0.5.3): exiting LCA left the cursor wherever
//! the terminal happened to put it, so the returning shell prompt landed on
//! top of LCA's own content.
//!
//! Two shapes, one contract:
//!
//! - The alt-screen teardown restored the pre-alt document with no `\r\n`
//!   after its final line. `?1049l` puts the cursor back at its pre-alt
//!   position (commonly row 1 col 1), so the prompt overwrote the top of
//!   the restored transcript.
//! - `ProcessTerminal::drop` then emitted a *second* `?1049l` for every
//!   exit, main screen included. Once the screen switch has happened, that
//!   one restores the cursor the switch *saved* - the top-left - and undoes
//!   the park, which is what issue #33 actually showed. tmux proves it: the
//!   same byte stream with and without the second `?1049l` lands the cursor
//!   at (0,0) versus below the document.
//! - The main-screen renderer's exit write is reached only through
//!   `Screen::leave`, which was a no-op for `Screen::Main`, so nothing at
//!   all was emitted. It must also descend from the caret row to below the
//!   last rendered row first: the document ends with the footer, while the
//!   caret sits in the editor line *above* it, so a bare `\r\n` from the
//!   caret would park the prompt on the footer.
//!
//! Both renderers must leave the cursor on a line of its own below the last
//! line they rendered, visible, with wrapping re-enabled - and nothing may
//! move the cursor after them.
//!
//! The `Screen::leave` dispatch half is guarded by the unit rows
//! `screen_leave_parks_the_cursor_in_main_screen_mode` and
//! `a_preserving_leave_in_main_screen_mode_writes_nothing` beside
//! `crates/lca-ui/src/run.rs`: `Screen` is private to that module, so the
//! dispatch cannot move here (the same reason `07`'s pinning test stays
//! beside the private resolver). The flag that gates the drop's restore is
//! guarded beside `crates/lca-tui/src/engine/terminal.rs`.
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #33.

use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::FakeTerminal;

// Verifies: gh #33 (alt screen) - the restored document ends with a
// newline, so the cursor is left below it rather than at the terminal's
// restored position, and the cursor is visible with wrapping on.
#[test]
fn leaving_the_alt_screen_parks_the_cursor_below_the_restored_document() {
    let mut term = FakeTerminal::new(40, 3);
    let mut renderer = AltScreenRenderer::new();
    renderer.render_lines(
        &mut term,
        vec!["line one".into(), "line two".into(), "line three".into()],
        40,
        3,
    );
    let _ = term.take_output();

    renderer.leave(&mut term, false);
    let out = term.take_output();

    assert!(
        out.contains("\x1b[?1049l"),
        "leaves the alt screen: {out:?}"
    );
    assert!(
        out.ends_with("line three\r\n\x1b[?7h\x1b[?25h"),
        "the park comes after the final line, then wrap+cursor: {out:?}"
    );
}

// Verifies: gh #33 (main screen) - the exit write descends from the caret
// row to below the last rendered row before the newline, so the prompt
// lands below the footer instead of on it, and it ends wrap+cursor on.
#[test]
fn finishing_the_main_screen_parks_the_cursor_below_the_last_rendered_line() {
    let mut term = FakeTerminal::new(40, 10);
    let mut renderer = MainScreenRenderer::new();
    // Four document rows with the caret on row 1: `cursor_row` (3) is two
    // rows below the hardware cursor (1), which is the footer-overwrite
    // shape issue #33 describes.
    renderer.render(
        &mut term,
        vec![
            "transcript".into(),
            format!("{}> prompt", lca_tui::engine::core::CURSOR_MARKER),
            "footer one".into(),
            "footer two".into(),
        ],
        40,
        10,
    );
    let _ = term.take_output();

    renderer.finish(&mut term);
    let out = term.take_output();

    assert!(
        out.starts_with("\x1b[2B\r\n"),
        "descends two rows to below the last line, then a fresh line: {out:?}"
    );
    assert!(
        out.ends_with("\x1b[?7h\x1b[?25h"),
        "wrap re-enabled and cursor visible: {out:?}"
    );
}

// Verifies: gh #33 - a renderer whose hardware cursor already rests on the
// last rendered row needs no descent, only the newline.
#[test]
fn finishing_the_main_screen_with_the_cursor_already_at_the_bottom_only_advances() {
    let mut term = FakeTerminal::new(40, 10);
    let mut renderer = MainScreenRenderer::new();
    // No caret marker: the render leaves the hardware cursor on the final
    // row it wrote.
    renderer.render(
        &mut term,
        vec!["transcript".into(), "footer".into()],
        40,
        10,
    );
    let _ = term.take_output();

    renderer.finish(&mut term);
    let out = term.take_output();

    assert!(
        out.ends_with("\r\n\x1b[?7h\x1b[?25h"),
        "a fresh line below the last row: {out:?}"
    );
    assert!(!out.contains('B'), "no descent is needed: {out:?}");
}

// Verifies: gh #33 (the whole exit byte stream, through the real terminal
// and the real renderer) - a full enter -> render -> leave cycle writes
// *one* `?1049l`, and the restored document ends with the newline that
// parks the cursor. The second `?1049l` came from `ProcessTerminal::drop`,
// which restores the cursor saved by the switch: tmux puts that cursor at
// (0,0), straight back onto the transcript.
//
// `PI_TUI_WRITE_LOG` is the engine's own capture hook (the testing plan's
// "record what the terminal received"), so this asserts the bytes rather
// than a renderer's opinion of them.
#[test]
#[allow(unsafe_code)] // SAFETY: `env_lock` is held for the whole test, and nextest runs each test in its own process.
fn the_exit_byte_stream_switches_the_screen_once_and_parks_after_the_document() {
    use lca_tui::engine::terminal::ProcessTerminal;

    let _lock = lca_testkit::fixture::env_lock();
    let path = std::env::temp_dir().join(format!("lca-gh33-writes-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let previous = std::env::var_os("PI_TUI_WRITE_LOG");
    // SAFETY: `env_lock` is held for the whole test; see the comment above.
    unsafe { std::env::set_var("PI_TUI_WRITE_LOG", &path) };
    {
        let mut term = ProcessTerminal::new();
        let mut alt = AltScreenRenderer::new();
        alt.enter(&mut term);
        alt.render_lines(&mut term, vec!["transcript".into(), "footer".into()], 40, 2);
        alt.leave(&mut term, false);
    } // `term` drops here: the defensive teardown runs after the renderer's own.
    // SAFETY: same lock discipline as above.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("PI_TUI_WRITE_LOG", value),
            None => std::env::remove_var("PI_TUI_WRITE_LOG"),
        }
    };

    let log = std::fs::read_to_string(&path).expect("the engine logged every write");
    let _ = std::fs::remove_file(&path);
    assert!(
        log.contains("footer\r\n\x1b[?7h\x1b[?25h"),
        "the document ends with the park: {log:?}"
    );
    assert_eq!(
        log.matches("\x1b[?1049l").count(),
        1,
        "only the renderer's own leave may switch screens: {log:?}"
    );
}
