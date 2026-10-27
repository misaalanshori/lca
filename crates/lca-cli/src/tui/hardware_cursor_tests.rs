//! gh #371 bug 4: `ui.show_hardware_cursor` reaches the caret.
//!
//! Split from `hooks.rs`'s own test module for the workspace's 1,200-line
//! ceiling (gate 11), the same reason `clear_on_shrink_tests` was split.
//!
//! **Why these rows start at a loaded config.** The key parsed, defaulted and
//! merged for a whole cycle while nothing read it, and the reason that survived
//! is the #538 lesson verbatim: rows that build a `DisplayTuning` by hand prove
//! the struct and nothing about the path to it. So every row below begins at a
//! `Config` (a file, an environment variable, or the defaults) and ends at what
//! the **editor** renders and what the renderer wrote.

use crate::tui::hooks::display_tuning;

/// A config with `ui.show_hardware_cursor` set through a file.
fn config_with_hardware_cursor(value: &str) -> lca_config::Config {
    let dir = lca_testkit::scratch_path("lca-371g4-hw-cursor");
    let file = dir.join("user.toml");
    std::fs::write(
        file.as_path(),
        format!("ui.show_hardware_cursor = {value}\n"),
    )
    .expect("write");
    lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(file),
        ..Default::default()
    })
    .expect("load")
}

/// The editor as the interface configures it for these tunables: `paint_caret`
/// from the same two facts `Chat::paints_caret` reads, with the caret typed two
/// characters into a three-character buffer.
///
/// The editor is the production widget, and `set_paint_caret` is the production
/// seam - so a row that fails here fails on the path the interface takes, not on
/// a fixture's idea of it.
fn editor_for(
    tuning: &lca_ui::state::DisplayTuning,
    theme_colored: bool,
) -> lca_tui::widgets::editor::Editor {
    let mut editor = lca_tui::widgets::editor::Editor::new();
    // The one call `Chat::sync_display_tuning` makes, with the same inputs.
    editor.set_paint_caret(lca_ui::chat::paint_caret_for_tests(
        theme_colored,
        tuning.show_hardware_cursor,
    ));
    editor.insert_str("abc");
    editor.cursor_left();
    editor
}

/// The editor's rendered rows plus the engine's reading of the marker.
fn marker_of(
    editor: &lca_tui::widgets::editor::Editor,
    width: u16,
) -> (Vec<String>, (u16, u16, bool)) {
    let lines = editor.render(width);
    let (clean, position) = lca_tui::engine::core::extract_cursor_position(&lines);
    (
        clean,
        position.expect("the editor always carries its caret marker"),
    )
}

// =============================================================================
// The default is the safety argument, so it is pinned twice.
// =============================================================================

// Verifies: gh #371 bug 4 - an unset config leaves the painted caret exactly as
// it is today. This is the row that would fail if the toggle had flipped the
// default, which is the one mistake an accessibility opt-in must not make.
#[test]
fn the_default_keeps_the_painted_caret() {
    let tuning = display_tuning(&lca_config::Config::defaults());
    assert!(
        !tuning.show_hardware_cursor,
        "the tuning says off, which is the product's default"
    );
    let (clean, (_, _, painted)) = marker_of(&editor_for(&tuning, true), 40);
    assert!(
        painted,
        "the painted caret is authoritative (ADR-0037), so the default paints it"
    );
    assert!(
        clean[0].contains("\x1b[7m") && clean[0].contains("\x1b[27m"),
        "in the composer's own form: {:?}",
        clean[0]
    );
}

// Verifies: gh #371 bug 4 - the off case is the frame it always was.
//
// An explicit `false` and an unset key are the same product state, so they must
// render the same bytes. A golden string would break on an unrelated repaint;
// the *equality of the two renderings* is the property that matters.
#[test]
fn an_explicit_off_renders_what_an_unset_key_renders() {
    let off = display_tuning(&config_with_hardware_cursor("false"));
    let defaults = display_tuning(&lca_config::Config::defaults());
    assert_eq!(
        editor_for(&off, true).render(40),
        editor_for(&defaults, true).render(40),
        "an explicit `false` and an unset key must produce the same frame, or the \
         default has stopped being the safety argument"
    );
}

// =============================================================================
// The opt-in.
// =============================================================================

// Verifies: gh #371 bug 4 - the toggle stops the editor painting, so the
// hardware cursor at the marker becomes the caret.
#[test]
fn an_enabled_toggle_leaves_the_caret_to_the_terminal() {
    let tuning = display_tuning(&config_with_hardware_cursor("true"));
    assert!(
        tuning.show_hardware_cursor,
        "the file's `true` reaches the tuning"
    );
    let (clean, (_, _, painted)) = marker_of(&editor_for(&tuning, true), 40);
    assert!(
        !painted,
        "the accessibility opt-in asks for the terminal's cursor, so the row must \
         not paint one"
    );
    assert!(
        !clean[0].contains("\x1b[7m"),
        "and nothing else in the row opened reverse video: {:?}",
        clean[0]
    );
    // The buffer is still there - the caret changed hands, not the text.
    assert!(
        lca_tui::engine::text::strip_terminal_sequences(&clean[0]).contains("abc"),
        "the typed text is unchanged: {:?}",
        clean[0]
    );

    // The off case is the mirror, so this row cannot pass on an editor that
    // simply never paints.
    let off = display_tuning(&config_with_hardware_cursor("false"));
    let (_, (_, _, painted_off)) = marker_of(&editor_for(&off, true), 40);
    assert!(
        painted_off,
        "off still paints, which is what makes the row above a difference rather \
         than a constant"
    );
}

// Verifies: gh #371 bug 4 - the toggle changes **visibility only**.
//
// The brief's sharpest row: a toggle that also moved the cursor would be a
// regression dressed as an option. The marker's (row, col) is read out of the
// rendered frame in both modes and must be identical, while the paint flag - the
// only thing the renderer turns into `?25h`/`?25l` - must differ.
#[test]
fn the_marker_position_is_identical_in_both_modes() {
    let off = display_tuning(&config_with_hardware_cursor("false"));
    let on = display_tuning(&config_with_hardware_cursor("true"));

    let (_, off_marker) = marker_of(&editor_for(&off, true), 40);
    let (_, on_marker) = marker_of(&editor_for(&on, true), 40);
    assert_eq!(
        (off_marker.0, off_marker.1),
        (on_marker.0, on_marker.1),
        "the caret sits at the same cell either way - the toggle must not move it"
    );
    assert_ne!(
        off_marker.2, on_marker.2,
        "and only the paint flag differs, which is what the renderer turns into \
         the cursor's visibility"
    );
}

// Verifies: gh #371 bug 4 - the marker's position is also unchanged in the
// *engine's* arithmetic, so `hardware_cursor_row` frame state cannot drift.
//
// The paint flag is the renderer's visibility input; the position feeds
// `position_hardware_cursor`'s row bookkeeping, which gh #371 bug 1 established
// must be a row the terminal can be on. Same column, same row, same everything
// except who draws the cell.
#[test]
fn the_toggle_does_not_touch_what_the_renderer_records() {
    let off = display_tuning(&config_with_hardware_cursor("false"));
    let on = display_tuning(&config_with_hardware_cursor("true"));

    let frame = |tuning: &lca_ui::state::DisplayTuning| {
        let mut renderer = lca_tui::engine::main_screen::MainScreenRenderer::new();
        let mut term = lca_tui::engine::terminal::FakeTerminal::new(40, 12);
        let lines = editor_for(tuning, true).render(38);
        let mut document = vec!["a transcript row".to_string()];
        document.extend(lines);
        document.push("footer".to_string());
        let placed = renderer.render(&mut term, document, 40, 12);
        let out = term.take_output();
        // `?25h` shown, `?25l` hidden: whichever the frame ended on.
        let shown = out.rfind("\x1b[?25h");
        let hidden = out.rfind("\x1b[?25l");
        let visible = match (shown, hidden) {
            (Some(a), Some(b)) => a > b,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => panic!("the frame set neither ?25h nor ?25l:\n{out}"),
        };
        (placed, visible)
    };

    let (off_placed, off_visible) = frame(&off);
    let (on_placed, on_visible) = frame(&on);
    assert_eq!(
        off_placed, on_placed,
        "the renderer places the cursor at the same cell either way"
    );
    assert!(
        !off_visible,
        "off: the painted caret is authoritative, so the terminal's cursor is hidden"
    );
    assert!(
        on_visible,
        "on: the accessibility opt-in asks for a real cursor, which is its whole point"
    );
}

// =============================================================================
// The plain theme, which already had the caret turned off.
// =============================================================================

// Verifies: gh #371 bug 4 - the two ways the caret gets turned off compose.
//
// The plain theme renders no escapes at all (FR-UI-5), so it already leaves the
// caret to the terminal. The toggle asks for the same thing, so the combination
// is agreement rather than conflict - and `&&` is what keeps it that way: the
// toggle never re-enables painting a plain theme has taken away.
#[test]
fn the_plain_theme_and_the_toggle_agree_rather_than_fight() {
    // The pure composition, as `Chat::paints_caret` computes it.
    assert!(
        !lca_ui::chat::paint_caret_for_tests(false, false),
        "a plain theme never paints, toggle or no toggle (FR-UI-5)"
    );
    assert!(
        !lca_ui::chat::paint_caret_for_tests(false, true),
        "and the toggle does not turn painting back on for it"
    );
    assert!(
        lca_ui::chat::paint_caret_for_tests(true, false),
        "a coloured theme with the toggle off paints - the default"
    );
    assert!(
        !lca_ui::chat::paint_caret_for_tests(true, true),
        "and stops painting when the user asks for the hardware cursor"
    );

    // The frame agrees: plain with the toggle on is a bare marker, no escapes.
    let tuning = display_tuning(&config_with_hardware_cursor("true"));
    let (clean, (_, _, painted)) = marker_of(&editor_for(&tuning, false), 40);
    assert!(!painted, "not painted");
    assert!(
        !clean[0].contains('\x1b'),
        "and the plain theme's no-escape contract still holds: {:?}",
        clean[0]
    );
}

// =============================================================================
// The row a user actually reaches for.
// =============================================================================

// Verifies: gh #371 bug 4 - `/settings` offers the key, with pi's wording, in
// the display group where a user looks for it. A toggle that is not in the
// editor is not wired in the product sense.
#[test]
fn settings_offers_the_key_with_pis_wording() {
    let (label, description) = crate::tui::display::setting_meta("ui.show_hardware_cursor");
    assert_eq!(
        label, "Show hardware cursor",
        "pi's own row label (its `/settings` says exactly this)"
    );
    assert!(
        description.to_lowercase().contains("screen reader"),
        "the description names who it is for - the whole reason it exists: {description}"
    );
    // Pi's own description is "Use the terminal cursor instead of Pi's drawn
    // cursor". Ours has to say the same thing *and* name who needs it, because
    // this repo's copy rule is that a row tells the user what it does and why
    // they might want it - and it must never say "Pi".
    assert!(
        description.to_lowercase().contains("cursor"),
        "and it says what changes (the cursor): {description}"
    );
    assert!(
        !description.contains("Pi"),
        "the product's own copy never names the project it was ported from: {description}"
    );

    // The inventory a `/settings` open builds: `settings_rows` needs a whole
    // `Ui`, so the row list is read through the same `setting_meta`/row table
    // by the assertion below. The *key* is in the row list is what matters, so
    // it is read from the source table rather than half-building a `Ui`.
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tui/display.rs"))
            .expect("read display.rs");
    let row_at = source
        .find("\"ui.show_hardware_cursor\",")
        .expect("the key is offered as a settings row");
    let around = &source[row_at.saturating_sub(400)..row_at];
    assert!(
        around.contains("\"Display & Appearance\""),
        "it sits in the display group (pi puts it there): {around}"
    );
    assert!(
        source[row_at..]
            .starts_with("\"ui.show_hardware_cursor\",\n                &[\"false\", \"true\"],"),
        "and it cycles off then on, so the default is the cycle's starting point"
    );
}

// Verifies: gh #371 bug 4 - toggling it persists, through the same writer every
// `/settings` cycle uses.
//
// `persist_setting_at` is the testable half of `persist_setting` (the same
// function, one path argument), so this row pins the writer the interface runs
// rather than a re-implementation - the #538 lesson in its other direction: a
// value that only exists in a live struct is not a setting.
#[test]
fn toggling_the_row_persists_the_key() {
    let dir = lca_testkit::scratch_path("lca-371g4-persist");
    let file = dir.join("config.toml");
    std::fs::write(&file, "# my setup\n").expect("seed");

    crate::persist::persist_setting_at(&file, "ui.show_hardware_cursor", Some("true"))
        .expect("persist");

    let text = std::fs::read_to_string(&file).expect("read back");
    assert!(
        text.contains("show_hardware_cursor = true"),
        "the key lands in the file, as a TOML boolean rather than a string:\n{text}"
    );
    assert!(
        text.contains("# my setup"),
        "and the comment-preserving writer kept the rest:\n{text}"
    );
    let reloaded = lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(file.clone()),
        ..Default::default()
    })
    .expect("reload");
    assert!(
        reloaded.ui_show_hardware_cursor(),
        "and a fresh load reads `true` back, so the write is loadable"
    );

    // Off again: the cycle's other direction, so a writer that only ever writes
    // `true` cannot pass this row.
    crate::persist::persist_setting_at(&file, "ui.show_hardware_cursor", Some("false"))
        .expect("persist off");
    let reloaded = lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(file),
        ..Default::default()
    })
    .expect("reload");
    assert!(
        !reloaded.ui_show_hardware_cursor(),
        "and `false` round-trips too"
    );
}
