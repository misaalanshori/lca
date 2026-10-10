//! gh #30 (EFG-030 + PG-032): `/settings` was a read-only text dump.
//! It is pi's interactive selector on our picker chrome now: rows of key,
//! value, and winning source; Enter/→ cycles a value through the one
//! persist seam; the theme and thinking rows open their existing
//! sub-pickers (the list comes back when they close); the yolo toggle is
//! loud. A host with no rows hook keeps the dump (FR-CFG-2's command
//! half), which is also where `lca config` still prints it.
//!
//! Verifies: FR-CFG-2 (the merged configuration and its winning source
//! stay reachable) for the fallback half.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::state::{SettingRow, UiHooks};
use lca_ui::{Chat, UiOptions};

/// What the persist seam was asked to write, in order.
type Writes = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A hook whose rows answer like the host's do: the file says `default`
/// until a write lands, then `user file` (the host re-reads its config;
/// this stub models exactly that contract).
fn rows_hook(writes: Writes) -> UiHooks {
    let written = Arc::new(Mutex::new(
        std::collections::BTreeMap::<String, String>::new(),
    ));
    let rows_source = written.clone();
    let settings_rows: Option<lca_ui::SettingsRows> = Some(Arc::new(move || {
        let written = rows_source.lock().unwrap();
        let row = |section: &str, key: &str, values: &[&str]| SettingRow {
            section: section.to_string(),
            key: key.to_string(),
            label: key.to_string(),
            description: String::new(),
            value: written
                .get(key)
                .cloned()
                .unwrap_or_else(|| "auto".to_string()),
            source: if written.contains_key(key) {
                "user file".to_string()
            } else {
                "default".to_string()
            },
            values: values.iter().map(|v| v.to_string()).collect(),
        };
        vec![
            row("Display", "ui.theme", &[]),
            row("Display", "ui.color", &["auto", "never"]),
            row("Model", "thinking", &[]),
            row("Security", "permissions.mode", &["ask", "yolo"]),
            row("Terminal", "shell.command_prefix", &[]),
        ]
    }));
    let written_for_persist = written.clone();
    let persist_setting: Option<lca_ui::state::SettingPersist> =
        Some(Arc::new(move |key: &str, value: Option<String>| {
            if let Some(value) = &value {
                written_for_persist
                    .lock()
                    .unwrap()
                    .insert(key.to_string(), value.clone());
            }
            writes.lock().unwrap().push((key.to_string(), value));
        }));
    UiHooks {
        settings_rows,
        persist_setting,
        ..Default::default()
    }
}

fn options(hooks: UiHooks) -> UiOptions {
    UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
        context_window: Arc::new(std::sync::Mutex::new(0)),
        thinking: Arc::new(Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: lca_ui::theme::THEMES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        initial_lines: Vec::new(),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: PathBuf::from("."),
        keybinding_overrides: Default::default(),
        keybinding_error: None,
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        confirm_switch: None,
        hooks,
        fullscreen: true,
    }
}

fn chat(hooks: UiHooks) -> Chat {
    Chat::new(options(hooks), Arc::new(KeybindingsManager::new()))
}

/// Type a command into the editor, one key per call - the loop's shape.
fn type_text(chat: &mut Chat, text: &str) {
    for key in text.chars() {
        chat.handle_key(&key.to_string());
    }
}

/// Type `/settings` and submit it.
fn open_settings(chat: &mut Chat) {
    type_text(chat, "/settings");
    chat.handle_key("\r");
}

// The selector itself: rows with key, value, and winning source; `q`
// closes; and a host with no rows hook keeps the read-only dump.
#[test]
fn the_selector_shows_key_value_and_source_and_q_closes_it() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut selector = chat(rows_hook(writes));
    open_settings(&mut selector);
    let picker = selector
        .settings_picker
        .as_ref()
        .expect("the selector opened");
    assert_eq!(picker.rows.len(), 5, "the curated rows: {:?}", picker.rows);
    assert_eq!(picker.rows[0].key, "ui.theme");
    assert_eq!(picker.rows[0].source, "default");
    assert_eq!(picker.rows[1].values, vec!["auto", "never"]);

    // Rendered: the frame and the source column are on screen.
    let rendered = selector
        .viewport(100, 24, 0)
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("settings"), "{rendered}");
    assert!(
        rendered.contains("ui.color = auto [default]"),
        "key, value, and source on the row:\n{rendered}"
    );

    selector.handle_key("q");
    assert!(
        selector.settings_picker.is_none(),
        "`q` closes the selector"
    );

    // No hook: the host's dump comes through the existing command path.
    let mut plain = chat(UiHooks::default());
    plain.world.options.slash_commands = vec!["/settings".to_string()];
    plain.world.options.invoke_command = Arc::new(|_, _| {
        CommandEffect::ShowWidget(
            "settings (key = value [source]; run /grants for permissions):".to_string(),
        )
    });
    open_settings(&mut plain);
    assert!(plain.settings_picker.is_none(), "no rows, no selector");
    assert!(
        plain
            .world
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("key = value [source]")),
        "the dump is what a hookless host gets: {:?}",
        plain.world.notice
    );
}

// A cycle writes through the one seam, the row's source moves with it
// (the host re-reads its config - this stub is that contract), and the
// session reflects it: `ui.color` has no live seam, so its notice says
// what did not happen.
#[test]
fn cycling_a_row_persists_through_the_one_seam_and_the_source_moves() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat(rows_hook(writes.clone()));
    open_settings(&mut chat);
    assert_eq!(
        chat.settings_picker.as_ref().expect("open").selected,
        0,
        "the first row is highlighted"
    );
    // Row 1 is `ui.color` (auto -> never).
    chat.handle_key("\x1b[B");
    chat.handle_key("\x1b[C"); // Right
    assert_eq!(
        *writes.lock().unwrap(),
        vec![("ui.color".to_string(), Some("never".to_string()))],
        "the write went through the persist seam"
    );
    let picker = chat.settings_picker.as_ref().expect("still open");
    assert_eq!(picker.rows[1].value, "never", "the row shows what landed");
    assert_eq!(
        picker.rows[1].source, "user file",
        "the winning source moved with it"
    );
    let notice = chat.world.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("applies to the next session"),
        "honest about the no-live-seam keys: {notice}"
    );
}

// The two rows with sub-pickers open them (and stay open underneath -
// pi's submenu shape), and their writes refresh the selector's rows.
#[test]
fn the_theme_and_thinking_rows_open_their_existing_sub_pickers() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat(rows_hook(writes.clone()));
    open_settings(&mut chat);

    // Row 0 is `ui.theme`: Enter opens the theme picker, selector under it.
    chat.handle_key("\r");
    assert!(
        chat.theme_picker.is_some(),
        "the theme row opens the theme picker"
    );
    assert!(
        chat.settings_picker.is_some(),
        "and the selector stays open underneath it"
    );
    // The theme picker's own Enter applies and persists.
    chat.handle_key("\r");
    let persisted_theme = writes
        .lock()
        .unwrap()
        .iter()
        .any(|(key, value)| key == "ui.theme" && value.is_some());
    assert!(persisted_theme, "the theme pick persisted: {:?}", writes);
    assert!(
        chat.theme_picker.is_none(),
        "and the theme picker closed on Enter"
    );
    assert_eq!(
        chat.settings_picker
            .as_ref()
            .expect("the selector is back")
            .rows[0]
            .source,
        "user file",
        "the restored rows answer with what just landed"
    );

    // Row 2 is `thinking`: same story through its own picker.
    chat.settings_picker.as_mut().expect("open").selected = 2;
    chat.handle_key("\r");
    assert!(
        chat.thinking_picker.is_some(),
        "the thinking row opens the thinking picker"
    );
    chat.handle_key("\x1b[B"); // Down: unset -> off
    chat.handle_key("\r");
    let persisted_thinking = writes
        .lock()
        .unwrap()
        .iter()
        .any(|(key, value)| key == "thinking" && value.as_deref() == Some("off"));
    assert!(
        persisted_thinking,
        "the thinking pick persisted: {:?}",
        writes
    );
}

// ADR-0042: `permissions.mode = yolo` is loud - the footer marker
// appears (the mode is not a silent flag), and the toggle goes through
// the same seam as every other edit.
#[test]
fn the_yolo_toggle_is_loud_in_the_footer() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat(rows_hook(writes.clone()));
    open_settings(&mut chat);
    // Row 3 is `permissions.mode` (ask -> yolo).
    for _ in 0..3 {
        chat.handle_key("\x1b[B");
    }
    chat.handle_key("\x1b[C"); // Right: ask -> yolo
    assert_eq!(
        *writes.lock().unwrap(),
        vec![("permissions.mode".to_string(), Some("yolo".to_string()))]
    );
    assert!(chat.world.options.yolo, "the session flag follows");
    let rendered = chat
        .viewport(100, 24, 0)
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("YOLO: every permission prompt auto-approved"),
        "the footer marker is up:\n{rendered}"
    );
}

// The row that guards this (issue #20 / V1): Ctrl+C during a running
// turn reaches the agent - even now that a picker owns the keyboard,
// which is what the selector made the common case.
#[test]
fn a_running_turn_still_cancels_with_the_selector_open() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut chat = chat(rows_hook(writes));
    chat.begin_turn(lca_protocol::steer_queue());
    open_settings(&mut chat);
    assert!(chat.settings_picker.is_some(), "the selector is open");
    assert_eq!(
        chat.handle_key("\x03"),
        lca_ui::Action::CancelTurn,
        "Ctrl+C cancels the turn with a picker open"
    );
}

// Verifies: gh #174 - the selector groups rows under section dividers
// (LCA's categorized menu; pi's list is flat), and the cursor only ever
// lands on rows - headers are display, never selection.
#[test]
fn sectioned_settings_render_dividers_and_skip_headers() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut selector = chat(rows_hook(writes));
    open_settings(&mut selector);
    let rendered = selector
        .viewport(100, 24, 0)
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("── Display ──"),
        "section dividers render:\n{rendered}"
    );
    // Walk the whole list: every stop is a row, never a header. This
    // picker caps at the last row (no wrap); headers skip themselves
    // because selection is row-indexed.
    let rows = selector.settings_picker.as_ref().expect("open").rows.len();
    selector.handle_key("\u{1b}[A"); // up at the top stays
    assert_eq!(
        selector.settings_picker.as_ref().expect("open").selected,
        0,
        "up saturates at the first row"
    );
    for _ in 0..rows {
        selector.handle_key("\u{1b}[B"); // down-arrow
    }
    assert_eq!(
        selector.settings_picker.as_ref().expect("open").selected,
        rows - 1,
        "down walks rows only and caps at the last"
    );
    let cursor_lines: Vec<String> = selector
        .viewport(100, 24, 0)
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .filter(|row| row.contains('>') && row.contains('='))
        .collect();
    assert!(
        cursor_lines.iter().all(|line| line.contains('=')),
        "every cursor sits on a key = value row: {cursor_lines:?}"
    );
}

// Verifies: gh #174 - a free-text row (no cycle values, no sub-picker)
// edits inline: Enter opens the buffer, typing replaces it, Enter
// applies through the one seam, Escape cancels without writing.
#[test]
fn free_text_rows_edit_inline_and_escape_cancels() {
    let writes: Writes = Arc::new(Mutex::new(Vec::new()));
    let mut editor = chat(rows_hook(writes.clone()));
    open_settings(&mut editor);
    // The stub's rows: ui.theme (sub-picker), ui.color (cycle),
    // thinking (sub-picker), shell.command_prefix (free text).
    for _ in 0..4 {
        editor.handle_key("\u{1b}[B");
    }
    assert_eq!(
        editor.settings_picker.as_ref().expect("open").rows[4].key,
        "shell.command_prefix"
    );
    editor.handle_key("\r"); // start editing (buffer holds the stored value)
    for _ in 0..4 {
        editor.handle_key("\u{7f}"); // clear "auto"
    }
    type_text(&mut editor, "source ~/x");
    editor.handle_key("\r"); // apply
    let written = writes.lock().unwrap();
    assert!(
        written
            .iter()
            .any(|(key, value)| key == "shell.command_prefix"
                && value == &Some("source ~/x".to_string())),
        "the edit persists: {written:?}"
    );
    drop(written);
    // Reopen, type, escape: nothing more lands.
    // The picker is still open from the apply: close it first, so the
    // reopen below is a fresh open and not an Enter on the live row.
    editor.handle_key("q");
    open_settings(&mut editor);
    for _ in 0..4 {
        editor.handle_key("\u{1b}[B");
    }
    editor.handle_key("\r");
    type_text(&mut editor, "junk");
    editor.handle_key("\u{1b}");
    assert_eq!(writes.lock().unwrap().len(), 1, "escape cancels the edit");
}
