//! gh #371 bug 4: the caret's form is a display tuning, so the interface is the
//! path a config value reaches the editor through.
//!
//! Split into its own file at the workspace's 1,200-line ceiling; the fixtures
//! (`options`, `chat`) stay in `chat_tests.rs` and are imported from it.
//!
//! **Why these rows and not only the CLI's.** The CLI rows prove the config
//! reaches a `DisplayTuning`; these prove the *interface* acts on it. A tuning
//! field nobody applies is exactly the shape of gh #538 (a key that parsed, a
//! renderer that implemented, and nothing calling the setter), so both halves
//! are pinned: the value travels, and the value is used.

use super::options;
use lca_tui::engine::keybindings::KeybindingsManager;
use std::sync::Arc;

/// A themed chat whose display tuning reports `show_hardware_cursor`.
///
/// The plain theme would confound the row - it never paints for its own
/// reasons - so the theme is `dark`, where the toggle is the only variable.
fn themed_chat(show_hardware_cursor: bool) -> crate::chat::Chat {
    let mut opts = options();
    opts.plain = false;
    opts.theme = "dark".to_string();
    opts.hooks.display_tuning = Some(Arc::new(move || crate::state::DisplayTuning {
        show_hardware_cursor,
        ..Default::default()
    }));
    crate::chat::Chat::new(opts, Arc::new(KeybindingsManager::new()))
}

/// The frame as the terminal receives it.
///
/// `viewport`, not `render`: an overlay is composited by `viewport` (the
/// document `render` returns has no overlay on it), and bug 3's rows learned the
/// same lesson the hard way.
fn frame(chat: &crate::chat::Chat) -> Vec<String> {
    chat.viewport(80, 30, 0)
}

/// Whether the chat's rendered frame paints its caret.
fn paints(chat: &crate::chat::Chat) -> bool {
    let (_, position) = lca_tui::engine::core::extract_cursor_position(&frame(chat));
    position.expect("the composer always carries a marker").2
}

// Verifies: gh #371 bug 4 - the tuning reaches the editor. This is the row that
// fails if `sync_display_tuning` stops applying it, which is the #538 gap.
#[test]
fn the_tuning_reaches_the_editors_caret() {
    assert!(
        paints(&themed_chat(false)),
        "off: a coloured theme paints the caret, the product default"
    );
    assert!(
        !paints(&themed_chat(true)),
        "on: the accessibility opt-in reaches the editor and stops the painting"
    );
}

// Verifies: gh #371 bug 4 - a `/settings` cycle applies without a restart, like
// every other tuning (gh #82). The hook is read fresh, so a chat whose tuning
// flips between frames flips its caret between frames.
#[test]
fn flipping_the_tuning_at_runtime_flips_the_caret() {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip = flag.clone();
    let mut opts = options();
    opts.plain = false;
    opts.theme = "dark".to_string();
    opts.hooks.display_tuning = Some(Arc::new(move || crate::state::DisplayTuning {
        show_hardware_cursor: flip.load(std::sync::atomic::Ordering::SeqCst),
        ..Default::default()
    }));
    let mut chat = crate::chat::Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert!(paints(&chat), "starts painted");

    flag.store(true, std::sync::atomic::Ordering::SeqCst);
    chat.tick();
    assert!(
        !paints(&chat),
        "the tick re-applies the tuning, so the change lands without a restart"
    );

    flag.store(false, std::sync::atomic::Ordering::SeqCst);
    chat.tick();
    assert!(paints(&chat), "and back again");
}

// Verifies: gh #371 bug 4 - the plain theme's no-escape contract survives the
// toggle, and so does the pre-existing defect this cycle fixed on the way.
//
// Switching to `plain` at runtime used to keep painting, because
// `set_paint_caret` ran only in `Chat::new`. The tuning sync is now the one
// place the decision is made, which makes the theme switch correct as a
// consequence rather than as a second special case.
#[test]
fn switching_to_the_plain_theme_stops_the_painting() {
    let mut chat = themed_chat(false);
    assert!(paints(&chat), "a coloured theme paints");

    chat.set_theme("plain");
    chat.tick();
    assert!(
        !paints(&chat),
        "the plain theme renders no escapes at all (FR-UI-5), so the caret is the \
         terminal's - and the tuning sync is what notices the theme changed"
    );

    chat.set_theme("dark");
    chat.tick();
    assert!(paints(&chat), "and switching back restores it");
}

// Verifies: gh #371 bug 4 - the overlay fields follow the composer's caret form
// (the bug 3 rule), so the toggle changes their caret too.
//
// A field that painted while the composer did not would be the second caret
// style bug 3's brief forbids; both read `Chat::paints_caret`, and this row is
// what makes "both" a fact. `/settings` inline edit is the field used here
// because it is reachable without a provider: the model picker needs one to have
// rows, and an empty picker paints no search row at all (bug 3's own rule).
#[test]
fn the_overlay_fields_follow_the_toggle_too() {
    fn settings_edit_frame(show_hardware_cursor: bool) -> (bool, String) {
        let mut opts = options();
        opts.plain = false;
        opts.theme = "dark".to_string();
        opts.hooks.display_tuning = Some(Arc::new(move || crate::state::DisplayTuning {
            show_hardware_cursor,
            ..Default::default()
        }));
        // `/settings` needs a rows hook to open at all.
        opts.hooks.settings_rows = Some(Arc::new(|| {
            vec![crate::state::SettingRow {
                section: "Test".to_string(),
                // A free-text row (`values` empty), so Enter opens the inline
                // editor rather than a sub-picker.
                key: "ui.autocomplete_max_visible".to_string(),
                label: "Completion rows".to_string(),
                description: "How many offers the popup shows".to_string(),
                value: "dark".to_string(),
                source: "default".to_string(),
                masked: false,
                values: Vec::new(),
                project: None,
            }]
        }));
        let mut chat = crate::chat::Chat::new(opts, Arc::new(KeybindingsManager::new()));
        for c in "/settings".chars() {
            chat.handle_key(&c.to_string());
        }
        chat.handle_key("\r");
        assert!(chat.settings_picker.is_some(), "the selector opened");
        // Enter on a free-text row (`values` empty) opens its inline editor.
        chat.handle_key("\r");
        if !chat
            .settings_picker
            .as_ref()
            .is_some_and(|picker| picker.editing.is_some())
        {
            // Diagnostics for the report if this row ever starts failing again.
            let picker = chat.settings_picker.as_ref().expect("picker");
            panic!(
                "the row is being edited, so a field owns the caret; \
                 selected={} visible={} rows={:?}",
                picker.selected,
                picker.visible().len(),
                picker.rows.iter().map(|row| &row.key).collect::<Vec<_>>()
            );
        }
        let painted_frame = frame(&chat);
        let (clean, position) = lca_tui::engine::core::extract_cursor_position(&painted_frame);
        let Some((row, _, painted)) = position else {
            panic!(
                "the edit field carries the marker; owner={:?} editing={:?}\n{}",
                chat.keyboard_owner(),
                chat.settings_picker.as_ref().map(|p| p.editing.is_some()),
                clean.join("\n")
            )
        };
        (painted, clean[row as usize].clone())
    }

    let (painted_on, row_on) = settings_edit_frame(true);
    assert!(
        !painted_on,
        "the edit field inherits the composer's answer: no paint"
    );
    assert!(
        !row_on.contains("\x1b[7m"),
        "and opens no reverse video: {row_on:?}"
    );

    // The mirror, so this row cannot pass on a field that never painted.
    let (painted_off, _) = settings_edit_frame(false);
    assert!(painted_off, "off still paints the field's caret");
}

// Verifies: gh #371 bug 4 - a chat with no tuning hook at all (an embedder that
// installs none) gets the product default, not an accidental on.
#[test]
fn a_host_with_no_tuning_hook_keeps_the_default() {
    let mut opts = options();
    opts.plain = false;
    opts.theme = "dark".to_string();
    opts.hooks.display_tuning = None;
    let chat = crate::chat::Chat::new(opts, Arc::new(KeybindingsManager::new()));
    assert!(
        paints(&chat),
        "no hook means the defaults (off), so the painted caret stays"
    );
}
