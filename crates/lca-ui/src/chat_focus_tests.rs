//! Gh #371 bug 3: exactly one `CURSOR_MARKER` per frame, and it belongs to
//! whoever owns the keyboard.
//!
//! The engine's `extract_cursor_position` takes the **last** marker in the
//! document, so before this the composer's marker always won - a CJK or emoji
//! IME candidate window opened at the inactive bottom editor while the user was
//! typing into a picker, or vanished entirely when the overlay covered the
//! composer's caret column.
//!
//! Every row here counts markers, because that is the fact the fix is: the
//! frames differ in *where* the caret is, never in how many there are. A row
//! that only asserted "a marker exists" would pass on the broken build.
//!
//! Split into its own file at the workspace ceiling, and gated on `gh #371`
//! like the bug 1/2 rows in `tests/regressions/371-*.rs`.

use super::{chat, options};
use crate::chat_focus::{Field, Focus};
use crate::chat_hub::ExtensionHub;
use crate::chat_pickers::{ModelPicker, SettingsPicker, TreeFilter, TreePicker};
use crate::state::{DialogExchange, DialogModal, SecretPrompt, SettingRow};
use lca_protocol::UiDialog;
use lca_tui::engine::core::CURSOR_MARKER;
use lca_tui::engine::keybindings::KeybindingsManager;
use std::sync::Arc;

const W: u16 = 100;
const H: u16 = 30;

/// The marker count in a frame, and the row index carrying it.
///
/// The row is `None` when the count is zero, so a caller asserting "exactly
/// one, on the search row" reads both facts from one place.
fn markers(lines: &[String]) -> (usize, Option<usize>) {
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains(CURSOR_MARKER))
        .map(|(index, _)| index)
        .collect();
    (hits.len(), hits.first().copied())
}

/// A chat on a **colored** theme, so the painted caret is active. The
/// fixtures default to the plain theme, where the caret is the bare marker -
/// both are asserted, in [`plain_theme_keeps_the_bare_marker`].
fn colored_chat() -> crate::chat::Chat {
    let mut opts = options();
    opts.plain = false;
    opts.theme = "dark".to_string();
    crate::chat::Chat::new(opts, Arc::new(KeybindingsManager::new()))
}

fn paint(chat: &crate::chat::Chat) -> Vec<String> {
    chat.viewport(W, H, 0)
}

/// The frame's plain text, for "the caret sits beside this label" assertions.
fn plain(chat: &crate::chat::Chat) -> Vec<String> {
    let (clean, _) = lca_tui::engine::core::extract_cursor_position(&paint(chat));
    clean.iter().map(|row| strip(row)).collect()
}

fn strip(row: &str) -> String {
    lca_tui::engine::text::strip_terminal_sequences(row)
}

/// The row the caret is on, plain, with the marker and everything after it
/// removed - the text the caret is anchored *after*.
fn caret_prefix(lines: &[String]) -> Option<String> {
    let (_, row) = markers(lines);
    let line = lines.get(row?)?;
    let at = line.find(CURSOR_MARKER)?;
    Some(strip(&line[..at]))
}

/// A chat with a `/model` picker open on two rows.
fn model_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    chat.model_picker = Some(ModelPicker::new(vec![
        crate::ModelRow::from(("alpha".into(), "alpha".into())),
        crate::ModelRow::from(("beta".into(), "beta".into())),
    ]));
    chat
}

/// A chat with a `/resume` picker open on two sessions.
fn resume_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    chat.resume_picker = Some(crate::resume::ResumePicker::new(vec![
        crate::resume::SessionEntry {
            id: "s1".into(),
            title: "first session".into(),
            messages: 2,
            age: "today".into(),
            labels: Vec::new(),
            modified_ms: 2,
            parent: None,
        },
        crate::resume::SessionEntry {
            id: "s2".into(),
            title: "second session".into(),
            messages: 4,
            age: "yesterday".into(),
            labels: Vec::new(),
            modified_ms: 1,
            parent: None,
        },
    ]));
    chat
}

/// A chat with the `/settings` picker open, one free-text row.
fn settings_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    chat.settings_picker = Some(SettingsPicker {
        rows: vec![SettingRow {
            section: "Test".to_string(),
            key: "model.default".to_string(),
            label: "Default model".to_string(),
            description: "The model a new session starts on".to_string(),
            value: "alpha".to_string(),
            source: "default".to_string(),
            masked: false,
            values: Vec::new(),
            project: None,
        }],
        selected: 0,
        editing: None,
        query: String::new(),
        remembered: std::collections::BTreeMap::new(),
    });
    chat
}

/// A chat with the `/tree` navigator open on two rows.
fn tree_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    chat.tree_picker = Some(TreePicker::new(
        vec![
            super::tree_row("u1", 0, "first prompt"),
            super::tree_row("u2", 1, "second prompt"),
        ],
        TreeFilter::Default,
    ));
    chat
}

/// A chat with the `/extensions` hub open over the fixture rows.
fn hub_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    let rows = crate::chat_hub::fixture_rows();
    chat.extension_hub = Some(ExtensionHub::loading(rows));
    chat
}

/// A chat with a host `Input` dialog open.
fn input_dialog_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    let (respond, _rx) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(DialogModal {
        exchange: DialogExchange {
            dialog: UiDialog::Input {
                label: "Your name".into(),
                placeholder: None,
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
    });
    chat
}

/// A chat with a host `Select` dialog open.
fn select_dialog_chat() -> crate::chat::Chat {
    let mut chat = colored_chat();
    let (respond, _rx) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(DialogModal {
        exchange: DialogExchange {
            dialog: UiDialog::Select {
                title: "Pick one".into(),
                options: vec!["alpha".into(), "beta".into()],
            },
            respond,
        },
        query: String::new(),
        matches: vec![0, 1],
        selected: 0,
        input: String::new(),
    });
    chat
}

/// Type into whichever field owns the keyboard, by driving the real key path,
/// so the fixtures exercise what a user presses rather than assigning state.
fn type_keys(chat: &mut crate::chat::Chat, text: &str) {
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        chat.handle_key(ch.encode_utf8(&mut buf));
    }
}

// =============================================================================
// The eight surfaces the issue names, plus the two a field-level sweep found.
// =============================================================================

// Verifies: gh #371 bug 3 - `/model` search takes the anchor from the composer.
#[test]
fn the_model_search_field_owns_the_single_marker() {
    let mut chat = model_chat();
    type_keys(&mut chat, "alp");
    let frame = paint(&chat);
    let (count, row) = markers(&frame);
    assert_eq!(count, 1, "exactly one marker:\n{}", plain(&chat).join("\n"));
    let prefix = caret_prefix(&frame).expect("the marker carries a prefix");
    assert!(
        prefix.contains("search: alp"),
        "the marker rides the search field, after what was typed: {prefix:?}"
    );
    assert!(
        row.is_some_and(|row| plain(&chat)[row].contains("search:")),
        "and it is on the search row"
    );
}

// Verifies: gh #371 bug 3 - `/resume` search takes the anchor from the composer.
#[test]
fn the_resume_search_field_owns_the_single_marker() {
    let mut chat = resume_chat();
    type_keys(&mut chat, "fir");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("search: fir"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - the `/extensions` filter takes the anchor.
#[test]
fn the_extension_filter_owns_the_single_marker() {
    let mut chat = hub_chat();
    type_keys(&mut chat, "open");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("filter: open"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - `/settings` type-to-search takes the anchor.
#[test]
fn the_settings_search_field_owns_the_single_marker() {
    let mut chat = settings_chat();
    type_keys(&mut chat, "model");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("search: model"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - `/settings` inline editing takes the anchor, at the
// end of the buffer being edited.
#[test]
fn the_settings_inline_edit_owns_the_single_marker() {
    let mut chat = settings_chat();
    // Enter opens the inline editor on a free-text row.
    chat.handle_key("\r");
    assert!(
        chat.settings_picker
            .as_ref()
            .is_some_and(|picker| picker.editing.is_some()),
        "the row is being edited"
    );
    type_keys(&mut chat, "-2");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(
        prefix.contains("Default model = alpha-2"),
        "the caret sits after the edited buffer: {prefix:?}"
    );
    assert!(
        !prefix.contains('█'),
        "and no second caret glyph stands beside it: {prefix:?}"
    );
}

// Verifies: gh #371 bug 3 - `/tree` bookmark editing takes the anchor.
#[test]
fn the_tree_bookmark_field_owns_the_single_marker() {
    let mut chat = tree_chat();
    chat.handle_picker_key("", Some("e"));
    assert!(
        chat.tree_picker
            .as_ref()
            .is_some_and(|picker| picker.editing.is_some()),
        "the label field is open"
    );
    type_keys(&mut chat, "chk");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("chk"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - the masked `/login` field takes the anchor, at the
// insertion point of the *masked* value, and never leaks the secret.
#[test]
fn the_masked_secret_field_owns_the_single_marker_without_leaking_it() {
    let mut chat = colored_chat();
    chat.world.secret = Some(SecretPrompt {
        provider: "example".into(),
        label: "Paste the API key".into(),
        input: String::new(),
        masked: true,
    });
    chat.handle_key("s");
    chat.handle_key("k");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let text = plain(&chat).join("\n");
    // The field's own row, not the whole frame: a transcript or hint line can
    // contain any two letters by accident, and an assertion that scans all of
    // it would fail on the startup header rather than on a leak.
    let field_row = plain(&chat)
        .into_iter()
        .find(|row| row.contains("**"))
        .unwrap_or_else(|| panic!("the mask is painted:\n{text}"));
    assert!(
        !field_row.contains("sk"),
        "the secret's characters never reach the field's row: {field_row:?}"
    );
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(
        prefix.ends_with("  **"),
        "the caret sits after the mask, not after the value: {prefix:?}"
    );
}

// Verifies: gh #371 bug 3 - a host `Input` dialog takes the anchor.
#[test]
fn the_host_input_dialog_owns_the_single_marker() {
    let mut chat = input_dialog_chat();
    type_keys(&mut chat, "abc");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("abc"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - a host `Select` dialog's filter takes the anchor.
#[test]
fn the_host_select_filter_owns_the_single_marker() {
    let mut chat = select_dialog_chat();
    type_keys(&mut chat, "be");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("> be"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - `Ctrl+R` transcript search takes the anchor.
#[test]
fn the_transcript_search_owns_the_single_marker() {
    let mut chat = colored_chat();
    chat.transcript.push_user("a question about Rust");
    chat.handle_key("\x12");
    assert!(chat.search.is_some(), "Ctrl+R opens the search readout");
    type_keys(&mut chat, "rus");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("/rus"), "{prefix:?}");
}

// Verifies: gh #371 bug 3 - the `/mcp` sign-in field takes the anchor.
#[test]
fn the_mcp_redirect_field_owns_the_single_marker() {
    let mut chat = colored_chat();
    chat.mcp_manager = Some(crate::chat_mcp::McpManager::ready(
        vec![crate::McpServerRow {
            name: "idp".into(),
            state: "needs sign-in".into(),
            detail: "fixture".into(),
            effective_exposure: "direct".into(),
            scope: "user".into(),
            project_override: false,
        }],
        Vec::new(),
    ));
    // The sign-in screen is the one manager screen with a text field; open it
    // directly so the row does not depend on the detail-then-enter journey.
    if let Some(manager) = chat.mcp_manager.as_mut() {
        manager.screen = crate::chat_mcp::McpScreen::SignIn {
            server: "idp".into(),
            url: Some("https://example.invalid/authorize".into()),
            pasted: String::new(),
            status: String::new(),
        };
    }
    chat.handle_mcp_manager_key("a", None);
    chat.handle_mcp_manager_key("b", None);
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1, "{}", plain(&chat).join("\n"));
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(prefix.contains("> ab"), "{prefix:?}");
}

// =============================================================================
// The properties that make "ownership" a fact rather than a description.
// =============================================================================

// Verifies: gh #371 bug 3 - with no overlay the composer keeps exactly the
// marker it always had (the no-regression row).
#[test]
fn the_composer_keeps_its_marker_when_nothing_is_open() {
    let mut chat = colored_chat();
    type_keys(&mut chat, "hello");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(
        prefix.trim_end().ends_with("hello"),
        "the caret is the composer's, after the typed text: {prefix:?}"
    );
}

// Verifies: gh #371 bug 3 - a frame where the overlay covers the composer's
// caret column still anchors to the overlay.
//
// The picker is bottom-anchored above the composer, so at a short terminal it
// covers the editor's rows outright - which is the state where the old
// behaviour anchored to nothing at all.
#[test]
fn a_short_frame_anchors_to_the_overlay_that_covers_the_composer() {
    let mut chat = model_chat();
    type_keys(&mut chat, "al");
    let (w, h) = (60u16, 14u16);
    let frame = chat.viewport(w, h, 0);
    let (count, _) = markers(&frame);
    assert_eq!(count, 1, "exactly one marker on a short frame");
    let prefix = caret_prefix(&frame).expect("prefix");
    assert!(
        prefix.contains("search: al"),
        "and it is the overlay's, though the overlay covers the composer: {prefix:?}"
    );
}

// Verifies: gh #371 bug 3 - the active field paints the *composer's* caret
// form, `7m` … `27m`, and closes only reverse video (bug 2's rule).
#[test]
fn the_active_field_paints_the_composers_caret_form() {
    let mut chat = model_chat();
    type_keys(&mut chat, "a");
    let frame = paint(&chat);
    let row = frame
        .iter()
        .find(|row| row.contains(CURSOR_MARKER))
        .expect("the marker's row");
    let caret = format!("{CURSOR_MARKER}\x1b[7m \x1b[27m");
    let at = row.find(&caret).unwrap_or_else(|| {
        panic!("the painted caret follows the marker and closes with 27m: {row:?}")
    });
    // Gh #371 bug 2's rule, asserted as the sequence that follows the marker
    // (the box painter's own role resets sit elsewhere in the row): the caret
    // opens reverse video and closes exactly that, so nothing after it in the
    // row loses its colour or decoration.
    assert_eq!(
        &row[at..at + caret.len()],
        caret,
        "the caret's close is channel-scoped, never a full reset: {row:?}"
    );
}

// Verifies: gh #371 bug 3 - on the plain theme (which renders no escapes at
// all, FR-UI-5) every field emits the bare marker and no paint.
#[test]
fn plain_theme_keeps_the_bare_marker() {
    let mut chat = chat();
    type_keys(&mut chat, "pl");
    chat.model_picker = Some(ModelPicker::new(vec![crate::ModelRow::from((
        "alpha".into(),
        "alpha".into(),
    ))]));
    type_keys(&mut chat, "al");
    let frame = paint(&chat);
    assert_eq!(markers(&frame).0, 1);
    let row = frame
        .iter()
        .find(|row| row.contains(CURSOR_MARKER))
        .expect("the marker's row");
    assert!(
        !row.contains("\x1b[7m"),
        "plain mode never paints a caret: {row:?}"
    );
}

// Verifies: gh #371 bug 3 - the ownership predicate is one answer, and the
// render and the caret helper agree on it.
//
// This is the seam rule the brief names: if both sides asked their own
// question they would eventually disagree, and a frame would carry two markers
// or none. Driving every surface through the same fixture and asserting the
// count is what pins it.
#[test]
fn every_keyboard_owning_surface_keeps_the_marker_count_at_one() {
    let mut surfaces: Vec<(&str, crate::chat::Chat)> = Vec::new();

    let mut model = model_chat();
    type_keys(&mut model, "x");
    surfaces.push(("model search", model));

    let mut resume = resume_chat();
    type_keys(&mut resume, "x");
    surfaces.push(("resume search", resume));

    let mut hub = hub_chat();
    type_keys(&mut hub, "x");
    surfaces.push(("extension filter", hub));

    let mut settings = settings_chat();
    type_keys(&mut settings, "x");
    surfaces.push(("settings search", settings));

    let mut editing = settings_chat();
    editing.handle_key("\r");
    type_keys(&mut editing, "x");
    surfaces.push(("settings edit", editing));

    let mut tree = tree_chat();
    tree.handle_picker_key("", Some("e"));
    type_keys(&mut tree, "x");
    surfaces.push(("tree label", tree));

    let mut secret = colored_chat();
    secret.world.secret = Some(SecretPrompt {
        provider: "example".into(),
        label: "API key".into(),
        input: String::new(),
        masked: true,
    });
    type_keys(&mut secret, "x");
    surfaces.push(("secret", secret));

    let mut input = input_dialog_chat();
    type_keys(&mut input, "x");
    surfaces.push(("dialog input", input));

    let mut select = select_dialog_chat();
    type_keys(&mut select, "x");
    surfaces.push(("dialog select", select));

    let mut search = colored_chat();
    search.transcript.push_user("a question");
    search.handle_key("\x12");
    type_keys(&mut search, "x");
    surfaces.push(("transcript search", search));

    for (name, chat) in &surfaces {
        let frame = paint(chat);
        assert_eq!(
            markers(&frame).0,
            1,
            "{name}: exactly one marker\n{}",
            plain(chat).join("\n")
        );
    }
}

// Verifies: gh #371 bug 3 - a keyboard-owning surface with **no text field**
// anchors to nothing rather than to the composer behind it.
//
// The composer is the inactive field in that frame, so leaving its marker
// would put the candidate window at the bottom of the screen while the user
// answers a `[y/n]` - the exact mis-anchor the bug is about.
#[test]
fn a_fieldless_surface_leaves_the_frame_without_a_marker() {
    let mut chat = colored_chat();
    let (respond, _rx) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(DialogModal {
        exchange: DialogExchange {
            dialog: UiDialog::Confirm {
                title: "Continue?".into(),
                message: "This runs the thing.".into(),
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
    });
    let frame = paint(&chat);
    assert_eq!(
        markers(&frame).0,
        0,
        "no field is on screen, so no marker is:\n{}",
        plain(&chat).join("\n")
    );
}

// Verifies: gh #371 bug 3 - `/settings` with an empty query paints no search
// row, so it does not claim the caret either.
#[test]
fn an_empty_settings_query_claims_no_caret() {
    let chat = settings_chat();
    assert_eq!(
        chat.keyboard_owner(),
        Focus::Surface,
        "the surface owns the keyboard and paints no field"
    );
    let frame = paint(&chat);
    assert_eq!(
        markers(&frame).0,
        0,
        "so the frame carries no marker:\n{}",
        plain(&chat).join("\n")
    );
}

// Verifies: gh #371 bug 3 - a canvas with content does not shift when the
// caret appears: the marker is zero-width and the painted cell lands inside
// the field's own prefix.
#[test]
fn the_caret_costs_no_layout() {
    let mut chat = model_chat();
    type_keys(&mut chat, "al");
    let widths: Vec<usize> = paint(&chat)
        .iter()
        .map(|row| lca_tui::engine::text::visible_width(&row.replace(CURSOR_MARKER, "")))
        .collect();
    assert!(
        widths.iter().all(|width| *width <= W as usize),
        "no row exceeds the terminal width: {widths:?}"
    );
}

// Verifies: gh #371 bug 3 - the field helper answers `None`'s field without a
// caret, so the predicate is the only input to the decision.
#[test]
fn the_caret_helpers_follow_the_predicate() {
    let chat = model_chat();
    assert_eq!(chat.keyboard_owner(), Focus::Field(Field::ModelSearch));
    assert!(
        chat.caret_field("abc", Field::ModelSearch)
            .contains(CURSOR_MARKER)
    );
    assert_eq!(
        chat.caret_field("abc", Field::Secret),
        "abc",
        "a field that does not own the keyboard carries no caret"
    );
    assert_eq!(
        chat.caret_row("  > ", "abc", "", Field::Secret),
        "  > abc",
        "and the row form agrees"
    );
    assert!(
        chat.caret_row("  > ", "abc", "", Field::ModelSearch)
            .contains(CURSOR_MARKER)
    );
}
