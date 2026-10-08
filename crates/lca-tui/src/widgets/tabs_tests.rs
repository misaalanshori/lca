//! The tabs widget's tests (gh #208).

use super::*;
use crate::engine::core::{Modifiers, MouseButton, MouseEvent};
use crate::engine::text::visible_width;

// Verifies: gh #208 - a horizontal bar marks the active tab and its
// badge, dirty, and closable states in plain markers (the host styles
// the spans; the widget stays theme-free per the engine boundary).
#[test]
fn horizontal_marks_active_badge_dirty_and_close() {
    let tabs = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![
            TabItem {
                id: "a".to_string(),
                title: "Fix Auth Bug".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
            TabItem {
                id: "b".to_string(),
                title: "Refactor Parser".to_string(),
                badge: Some("3".to_string()),
                is_generating: false,
                is_dirty: true,
                closable: true,
            },
        ],
        active_index: 0,
        show_add_button: true,
        hovered: None,
    };
    let rows = tabs.render(80);
    assert_eq!(rows.len(), 1, "one bar row");
    assert!(
        rows[0].contains("[ 1. Fix Auth Bug* ]"),
        "active: {row}",
        row = rows[0]
    );
    assert!(rows[0].contains("(3)"), "badge: {row}", row = rows[0]);
    assert!(rows[0].contains('●'), "dirty: {row}", row = rows[0]);
    assert!(rows[0].contains('×'), "closable: {row}", row = rows[0]);
    assert!(rows[0].contains("[+]"), "add: {row}", row = rows[0]);
}

// Verifies: gh #208 - titles truncate with an ellipsis past the width.
#[test]
fn horizontal_truncates_with_an_ellipsis() {
    let tabs = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![TabItem {
            id: "a".to_string(),
            title: "a very long tab title that cannot fit".to_string(),
            badge: None,
            is_generating: false,
            is_dirty: false,
            closable: false,
        }],
        active_index: 0,
        show_add_button: false,
        hovered: None,
    };
    let rows = tabs.render(20);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains('…'), "truncated: {row}", row = rows[0]);
    assert!(visible_width(&rows[0]) <= 20, "fits: {row}", row = rows[0]);
}

// Verifies: gh #208 - the vertical rail marks the active row with ▶.
#[test]
fn vertical_marks_the_active_row() {
    let tabs = TabsWidget {
        orientation: TabOrientation::Vertical,
        items: vec![
            TabItem {
                id: "a".to_string(),
                title: "Context".to_string(),
                badge: Some("42k".to_string()),
                is_generating: true,
                is_dirty: false,
                closable: false,
            },
            TabItem {
                id: "b".to_string(),
                title: "Git Status".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
        ],
        active_index: 1,
        show_add_button: true,
        hovered: None,
    };
    let rows = tabs.render(40);
    assert!(rows[0].contains("(42k)"), "badge: {rows:?}");
    assert!(rows[0].contains('◌'), "generating: {rows:?}");
    assert!(rows[1].starts_with("▶"), "active: {rows:?}");
    assert!(rows[2].contains("[+]"), "add: {rows:?}");
}

// Verifies: gh #208 - arrows walk (wrapping), digits jump, Enter
// confirms, Ctrl+W closes a closable tab.
#[test]
fn keyboard_walks_jumps_confirms_and_closes() {
    let mut tabs = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![
            TabItem {
                id: "a".to_string(),
                title: "a".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: true,
            },
            TabItem {
                id: "b".to_string(),
                title: "b".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
        ],
        active_index: 0,
        show_add_button: false,
        hovered: None,
    };
    assert_eq!(tabs.handle_key("right"), None);
    assert_eq!(tabs.active_index, 1);
    assert_eq!(tabs.handle_key("right"), None);
    assert_eq!(tabs.active_index, 0, "wraps");
    assert_eq!(tabs.handle_key("2"), Some(TabEvent::Select(1)));
    assert_eq!(tabs.handle_key("enter"), Some(TabEvent::Select(1)));
    assert_eq!(tabs.handle_key("ctrl+w"), None, "not closable: no event");
    assert_eq!(tabs.handle_key("1"), Some(TabEvent::Select(0)));
    assert_eq!(
        tabs.handle_key("ctrl+w"),
        Some(TabEvent::Close(0)),
        "closable: close event"
    );
}

// Verifies: gh #208 - clicks hit tabs, ×, and +; a press previews, a
// release on the same cell confirms.
#[test]
fn mouse_clicks_switch_tabs_and_emit_add_and_close() {
    let mut tabs = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![
            TabItem {
                id: "a".to_string(),
                title: "a".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: true,
            },
            TabItem {
                id: "b".to_string(),
                title: "b".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
        ],
        active_index: 0,
        show_add_button: true,
        hovered: None,
    };
    let plain = || MouseEvent::Move {
        col: 0,
        row: 0,
        button: None,
        modifiers: Modifiers::default(),
    };
    // Hover the second tab: lift follows.
    let second = tabs.tab_at(16, 0, 80).expect("second tab hits");
    assert_eq!(second, TabHit::Tab(1));
    let down = |col: u16| MouseEvent::Down {
        col,
        row: 0,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
    };
    let up = |col: u16| MouseEvent::Up {
        col,
        row: 0,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
    };
    assert_eq!(tabs.handle_mouse(down(16), 80), None, "press previews");
    assert_eq!(tabs.active_index, 1, "press switches");
    assert_eq!(
        tabs.handle_mouse(up(16), 80),
        Some(TabEvent::Select(1)),
        "release confirms"
    );
    assert_eq!(tabs.hovered, Some(1), "hover lifts: {:?}", tabs.hovered);
    let _ = plain;
    // Sweep the bar: the × cell and the [+] cell hit distinctly.
    let mut hits: Vec<TabHit> = (0..60u16)
        .filter_map(|col| tabs.tab_at(col, 0, 80))
        .collect();
    hits.dedup();
    assert!(hits.contains(&TabHit::Close(0)), "× hits: {hits:?}");
    assert!(hits.contains(&TabHit::Add), "+ hits: {hits:?}");
    // The × cell closes on release; [+] adds.
    let close_col = (0..60u16)
        .find(|col| tabs.tab_at(*col, 0, 80) == Some(TabHit::Close(0)))
        .expect("a close cell");
    assert_eq!(tabs.handle_mouse(down(close_col), 80), None);
    assert_eq!(
        tabs.handle_mouse(up(close_col), 80),
        Some(TabEvent::Close(0))
    );
    let add_col = (0..60u16)
        .find(|col| tabs.tab_at(*col, 0, 80) == Some(TabHit::Add))
        .expect("an add cell");
    assert_eq!(tabs.handle_mouse(down(add_col), 80), None);
    assert_eq!(tabs.handle_mouse(up(add_col), 80), Some(TabEvent::Add));
}

// Verifies: gh #208 - the rail hit-tests by row, and an empty widget
// neither renders nor answers.
#[test]
fn vertical_hits_by_row_and_empty_is_quiet() {
    let mut tabs = TabsWidget {
        orientation: TabOrientation::Vertical,
        items: vec![
            TabItem {
                id: "a".to_string(),
                title: "a".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
            TabItem {
                id: "b".to_string(),
                title: "b".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: false,
            },
        ],
        active_index: 0,
        show_add_button: true,
        hovered: None,
    };
    assert_eq!(tabs.tab_at(0, 1, 40), Some(TabHit::Tab(1)));
    assert_eq!(tabs.tab_at(0, 2, 40), Some(TabHit::Add));
    let down = MouseEvent::Down {
        col: 0,
        row: 1,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
    };
    let up = MouseEvent::Up {
        col: 0,
        row: 1,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
    };
    assert_eq!(tabs.handle_mouse(down, 40), None);
    assert_eq!(tabs.handle_mouse(up, 40), Some(TabEvent::Select(1)));
    assert_eq!(tabs.handle_key("down"), None);
    assert_eq!(tabs.active_index, 0, "vertical wraps down to the top");

    let mut empty = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![],
        active_index: 0,
        show_add_button: false,
        hovered: None,
    };
    assert_eq!(empty.render(80).len(), 1, "an empty bar is one empty row");
    assert_eq!(empty.handle_key("right"), None);
    assert_eq!(empty.tab_at(0, 0, 80), None);
}
