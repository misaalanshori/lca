//! `/settings` type-to-search (gh #272 parity pass): pi's `ConfigSelector`
//! filters its row list as you type, and LCA's selector did not.
//!
//! The filter matches the key, the label, and the description, so a user can
//! type what they remember ("retry", "steering", or `ui.color`) rather than
//! scrolling a 40-row sectioned list. The cursor indexes the *visible* set,
//! which is the property that matters: a filter that hides the selected row
//! must never leave the cursor on it, because Enter would then change a
//! setting the user cannot see.
//!
//! Split from `chat_overlay_tests.rs` under the file ceiling.

use super::*;
use super::{chat, strip};

/// A picker over `(key, label, description)` triples.
fn picker(rows: &[(&str, &str, &str)]) -> crate::chat_pickers::SettingsPicker {
    crate::chat_pickers::SettingsPicker {
        rows: rows
            .iter()
            .map(|(key, label, description)| crate::state::SettingRow {
                section: "Test".to_string(),
                key: key.to_string(),
                label: label.to_string(),
                description: description.to_string(),
                value: "v".to_string(),
                source: "default".to_string(),
                masked: false,
                values: Vec::new(),
                project: None,
            })
            .collect(),
        selected: 0,
        editing: None,
        query: String::new(),
        remembered: std::collections::BTreeMap::new(),
    }
}

fn chat_with(rows: &[(&str, &str, &str)]) -> Chat {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.settings_picker = Some(picker(rows));
    chat
}

fn paint(chat: &Chat) -> String {
    strip(&chat.viewport(100, 30, 0)).join("\n")
}

fn type_text(chat: &mut Chat, text: &str) {
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let bytes = ch.encode_utf8(&mut buf).as_bytes();
        let _ = chat.handle_picker_key(std::str::from_utf8(bytes).unwrap_or(""), None);
    }
}

// Verifies: gh #272 parity pass - typing filters the list by key, and the
// non-matching rows disappear from the painted pane.
#[test]
fn typing_filters_the_settings_list_by_key() {
    let mut chat = chat_with(&[
        ("ui.color", "Color", "Terminal color policy"),
        ("ui.theme", "Theme", "The color palette"),
        ("provider.retry_limit", "Retry limit", "Retry attempts"),
    ]);
    let before = paint(&chat);
    assert!(before.contains("Color = v"), "{before}");
    assert!(before.contains("Retry limit = v"), "{before}");

    type_text(&mut chat, "retry");
    let after = paint(&chat);
    assert!(
        after.contains("Retry limit = v"),
        "the match stays:\n{after}"
    );
    assert!(
        !after.contains("Color = v"),
        "the non-match is filtered out:\n{after}"
    );
    assert!(
        after.contains("search: retry"),
        "the query is shown:\n{after}"
    );
}

// Verifies: gh #272 parity pass - the label matches too, not only the key, so
// someone who remembers "Retry limit" but not `provider.retry_limit` finds it.
#[test]
fn typing_matches_the_label_and_the_description() {
    for needle in ["retry limit", "attempts"] {
        let mut chat = chat_with(&[
            ("ui.color", "Color", "Terminal color policy"),
            ("provider.retry_limit", "Retry limit", "Retry attempts"),
        ]);
        type_text(&mut chat, needle);
        let text = paint(&chat);
        assert!(
            text.contains("Retry limit = v"),
            "`{needle}` finds the row:\n{text}"
        );
        assert!(!text.contains("Color = v"), "`{needle}` filters:\n{text}");
    }
}

// Verifies: gh #272 parity pass - the case-insensitive property, so `RETRY`
// and `retry` find the same rows (pi's filter is case-insensitive).
#[test]
fn the_filter_is_case_insensitive() {
    let mut chat = chat_with(&[("provider.retry_limit", "Retry limit", "Retry attempts")]);
    type_text(&mut chat, "RETRY");
    assert!(paint(&chat).contains("Retry limit = v"));
}

// Verifies: gh #272 parity pass - THE property that makes filtering safe:
// the cursor indexes the visible set, so a query that hides the selected row
// moves the cursor onto a row that is actually on screen. Without this, Enter
// would change a setting the user cannot see.
#[test]
fn the_cursor_follows_the_filtered_set_not_the_full_row_list() {
    let mut chat = chat_with(&[
        ("ui.color", "Color", "Terminal color policy"),
        ("ui.theme", "Theme", "The color palette"),
        ("provider.retry_limit", "Retry limit", "Retry attempts"),
    ]);
    // Move the cursor to the last row (retry), then filter to something that
    // excludes it.
    let _ = chat.handle_picker_key("", Some("down"));
    let _ = chat.handle_picker_key("", Some("down"));
    assert_eq!(chat.settings_picker.as_ref().expect("open").selected, 2);

    type_text(&mut chat, "color");
    let picker = chat.settings_picker.as_ref().expect("open");
    let visible = picker.visible();
    assert!(
        picker.selected < visible.len(),
        "the cursor must land inside the filtered set (selected {}, visible {})",
        picker.selected,
        visible.len()
    );
    // And the row it now names is one the query admits.
    let row = picker.selected_row().expect("a row is selected");
    assert!(
        row.key.contains("color") || row.description.to_lowercase().contains("color"),
        "the selected row matches the filter: {}",
        row.key
    );
}

// Verifies: gh #272 parity pass - Esc clears the query before it closes the
// list (pi's shape): one Esc undoes the search, a second closes.
#[test]
fn escape_clears_the_query_before_closing() {
    let mut chat = chat_with(&[
        ("ui.color", "Color", "Terminal color policy"),
        ("ui.theme", "Theme", "The color palette"),
    ]);
    type_text(&mut chat, "theme");
    assert!(
        chat.settings_picker
            .as_ref()
            .is_some_and(|picker| !picker.query.is_empty())
    );

    let _ = chat.handle_picker_key("", Some("escape"));
    let picker = chat.settings_picker.as_ref().expect("still open");
    assert!(picker.query.is_empty(), "the query is cleared");
    assert!(paint(&chat).contains("Color = v"), "every row is back");

    let _ = chat.handle_picker_key("", Some("escape"));
    assert!(chat.settings_picker.is_none(), "the second Esc closes");
}

// Verifies: gh #272 parity pass - backspace widens the filter, and an emptied
// query returns the whole list.
#[test]
fn backspace_widens_the_filter_and_an_empty_query_shows_everything() {
    let mut chat = chat_with(&[
        ("ui.color", "Color", "Terminal color policy"),
        ("ui.theme", "Theme", "The color palette"),
    ]);
    type_text(&mut chat, "theme");
    assert!(!paint(&chat).contains("Color = v"));

    for _ in 0..5 {
        let _ = chat.handle_picker_key("", Some("backspace"));
    }
    let picker = chat.settings_picker.as_ref().expect("open");
    assert!(picker.query.is_empty(), "backspaced to empty");
    let text = paint(&chat);
    assert!(text.contains("Color = v"), "{text}");
    assert!(text.contains("Theme = v"), "{text}");
    assert!(
        !text.contains("search:"),
        "no search line while the query is empty:\n{text}"
    );
}

// Verifies: gh #272 parity pass - a query that matches nothing leaves an empty
// list with the cursor at zero, and Enter on it does nothing rather than
// panicking or editing row 0 of the hidden list.
#[test]
fn a_query_matching_nothing_is_safe() {
    let mut chat = chat_with(&[("ui.color", "Color", "Terminal color policy")]);
    type_text(&mut chat, "zzzznomatch");
    let picker = chat.settings_picker.as_ref().expect("open");
    assert!(picker.visible().is_empty(), "nothing matches");
    assert_eq!(picker.selected, 0);
    assert!(picker.selected_row().is_none());

    // Enter must be a no-op, not a panic and not an edit of a hidden row.
    let _ = chat.handle_picker_key("", Some("enter"));
    let picker = chat.settings_picker.as_ref().expect("still open");
    assert!(
        picker.editing.is_none(),
        "Enter on an empty list must not start an edit"
    );
}

// Verifies: gh #272 parity pass - navigation is bounded by the filtered count,
// so Down cannot walk past the last visible row.
#[test]
fn navigation_is_bounded_by_the_filtered_count() {
    let mut chat = chat_with(&[
        ("ui.color", "Color", "Terminal color policy"),
        ("ui.theme", "Theme", "The color palette"),
        ("ui.output_pad", "Output padding", "Transcript margin"),
    ]);
    type_text(&mut chat, "ui.");
    for _ in 0..10 {
        let _ = chat.handle_picker_key("", Some("down"));
    }
    let picker = chat.settings_picker.as_ref().expect("open");
    assert_eq!(
        picker.selected,
        picker.visible().len().saturating_sub(1),
        "the cursor stops at the last visible row"
    );
}

// Verifies: gh #272 parity pass - the hint tells the user the list filters,
// which is how the affordance is discovered at all.
#[test]
fn the_hint_names_the_search_affordance() {
    let chat = chat_with(&[("ui.color", "Color", "Terminal color policy")]);
    let text = paint(&chat);
    assert!(
        text.contains("type to search"),
        "the hint names the affordance:\n{text}"
    );
}

// Verifies: gh #272 parity pass - an empty result says so. Pi prints
// "No matching settings"; a silently blank list reads as a rendering bug.
#[test]
fn an_empty_result_says_no_matching_settings() {
    let mut chat = chat_with(&[("ui.color", "Color", "Terminal color policy")]);
    type_text(&mut chat, "zzzznomatch");
    let text = paint(&chat);
    assert!(
        text.contains("No matching settings"),
        "the empty state is named:\n{text}"
    );
}
