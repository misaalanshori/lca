//! `Chat`'s picker rolling window (gh #226): tall-list viewports,
//! paging keys, wheel steps, and scroll-offset clicks.
//!
//! Split from `chat_overlay_tests.rs` under the workspace's 1,200-line
//! file ceiling. The fixtures (`options`, `chat`, `strip`) stay in
//! `chat_tests.rs` and are imported from it.

use super::*;
use super::{chat, strip};

// Verifies: gh #226 - a 50-model picker on an 80x24 pane keeps a
// closed frame and visible selection while paging: End lands on the
// last row with an upward count, and the hint row never scrolls away.
#[test]
fn a_tall_model_picker_pages_with_frame_and_selection_visible() {
    use crate::chat_pickers::ModelPicker;
    let mut chat = chat();
    chat.screen_mode = true;
    let models: Vec<(String, String)> = (0..50)
        .map(|index| (format!("model-{index:02}"), format!("model-{index:02}")))
        .collect();
    chat.model_picker = Some(ModelPicker::new(models));
    let (w, h) = (80u16, 24u16);
    let paint = |chat: &Chat| strip(&chat.viewport(w, h, 0)).join("\n");
    let text = paint(&chat);
    assert!(text.contains("╭─"), "top border draws");
    assert!(text.contains("╰─"), "bottom border draws");
    assert!(text.contains("▼"), "more below at the top");
    assert!(text.contains("model-00"), "starts at the top");

    for _ in 0..3 {
        let _ = chat.handle_picker_key("", Some("pagedown"));
    }
    assert_eq!(chat.model_picker.as_ref().unwrap().selected, 30);
    let text = paint(&chat);
    assert!(text.contains("model-30"), "the paged-to row shows");
    assert!(text.contains("▲"), "more above now");
    assert!(text.contains("▼"), "more below still");
    assert!(text.contains("enter apply"), "the hint row stays");

    let _ = chat.handle_picker_key("", Some("end"));
    assert_eq!(chat.model_picker.as_ref().unwrap().selected, 49);
    let text = paint(&chat);
    assert!(text.contains("model-49"), "the last row shows");
    assert!(!text.contains("▼"), "nothing below at the end");
}

// Verifies: gh #226 - page/home/end keys step selection (ten rows a
// page), and the wheel rolls one row a tick.
#[test]
fn paging_keys_and_wheel_step_selection() {
    use crate::chat_pickers::ModelPicker;
    let mut chat = chat();
    let models: Vec<(String, String)> = (0..50)
        .map(|index| (format!("m{index:02}"), format!("m{index:02}")))
        .collect();
    chat.model_picker = Some(ModelPicker::new(models));
    let selected = |chat: &Chat| chat.model_picker.as_ref().unwrap().selected;
    let _ = chat.handle_picker_key("", Some("pagedown"));
    assert_eq!(selected(&chat), 10);
    let _ = chat.handle_picker_key("", Some("pageup"));
    assert_eq!(selected(&chat), 0);
    let _ = chat.handle_picker_key("", Some("end"));
    assert_eq!(selected(&chat), 49);
    let _ = chat.handle_picker_key("", Some("home"));
    assert_eq!(selected(&chat), 0);
    chat.wheel_picker(1);
    assert_eq!(selected(&chat), 1);
    chat.wheel_picker(-1);
    assert_eq!(selected(&chat), 0);
    chat.wheel_picker(-1);
    assert_eq!(selected(&chat), 0, "clamped at the top");
}

// Verifies: gh #226 - a click lands on the scrolled row (`start +
// row`), not the unwindowed one: the hit test reads back the painted
// window.
#[test]
fn a_click_maps_through_the_scroll_offset() {
    use crate::chat_mouse::PickerHit;
    use crate::chat_pickers::ModelPicker;
    let mut chat = chat();
    chat.screen_mode = true;
    let models: Vec<(String, String)> = (0..50)
        .map(|index| (format!("m{index:02}"), format!("m{index:02}")))
        .collect();
    chat.model_picker = Some(ModelPicker::new(models));
    chat.model_picker.as_mut().unwrap().selected = 30;
    let (w, h) = (80u16, 24u16);
    let _ = chat.viewport(w, h, 0);
    let (start, end, _) = chat.picker_window.get();
    assert!(start > 0 && end < 52, "paint windowed: {start}..{end}");
    let (rect, _) = chat.picker_layout(w, h).expect("the box maps");
    let col = rect.col + 4;
    // The third visible item row below the title: body `start + 2`,
    // item `start` (the model body opens with two header rows).
    let row = rect.row + 1 + 2;
    assert_eq!(
        chat.picker_hit(col, row, w, h),
        Some(PickerHit::Item(start)),
        "the scrolled row maps, not row two"
    );
}

// Verifies: gh #226 - the settings selector's paging keys jump the
// full row range (end/home) and step ten rows (pagedown).
#[test]
fn settings_paging_keys_jump_and_step_selection() {
    let mut chat = chat();
    chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
        rows: (0..30)
            .map(|i| crate::state::SettingRow {
                key: format!("k{i:02}"),
                value: "v".to_string(),
                source: "default".to_string(),
                section: String::new(),
                values: Vec::new(),
            })
            .collect(),
        selected: 0,
        editing: None,
    });
    let _ = chat.handle_picker_key("", Some("end"));
    assert_eq!(chat.settings_picker.as_ref().unwrap().selected, 29);
    let _ = chat.handle_picker_key("", Some("home"));
    assert_eq!(chat.settings_picker.as_ref().unwrap().selected, 0);
    let _ = chat.handle_picker_key("", Some("pagedown"));
    assert_eq!(chat.settings_picker.as_ref().unwrap().selected, 10);
}

// Verifies: gh #230 - hovering a visible item highlights without
// moving the rolling window: the stored window survives a hover
// that stays inside it.
#[test]
fn hover_inside_the_window_highlights_without_scrolling() {
    use crate::chat_pickers::ModelPicker;
    let models: Vec<(String, String)> =
        (0..50).map(|i| (format!("m{i:02}"), format!("model {i:02}"))).collect();
    let mut chat = chat();
    chat.screen_mode = true;
    chat.model_picker = Some(ModelPicker::new(models));
    // Keyboard to the bottom: the window sits at the end.
    let (w, h) = (80u16, 24u16);
    let _ = chat.viewport(w, h, 0);
    let _ = chat.handle_picker_key("", Some("end"));
    let _ = chat.viewport(w, h, 0);
    let (start, end, _) = chat.picker_window.get();
    assert!(start > 0, "the window rolled: ({start}, {end})");
    let selected_before = chat.model_picker.as_ref().expect("open").selected;
    // Hover the first visible row: selection follows, window holds.
    chat.hover_picker_item(start);
    let picker = chat.model_picker.as_ref().expect("open");
    assert_eq!(picker.selected, start, "the highlight follows");
    // One frame with the hovered selection: the window must hold.
    let _ = chat.viewport(w, h, 0);
    assert_eq!(
        chat.picker_window.get(),
        (start, end, chat.picker_window.get().2),
        "hover never rescrolls"
    );
    assert_ne!(selected_before, start, "the key walk moved first");
}
