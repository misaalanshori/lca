//! The drawer tab's rows (gh #207): the margin handle that toggles
//! the extension panel. Split from `chat_overlay_tests.rs` for the
//! workspace's 1,200-line file ceiling.

use super::*;
use super::{chat, options};

// Verifies: gh #207 - with no panel region registered, no drawer tab
// paints on the margin.
#[test]
fn no_panel_registered_draws_no_tab() {
    let mut chat = chat();
    chat.screen_mode = true;
    let (w, h) = (80u16, 24u16);
    assert_eq!(chat.drawer_rect(w, h), None, "no region, no tab");
    let frame = chat.viewport(w, h, 0).join("\n");
    assert!(
        !frame.contains('◀') && !frame.contains('▶'),
        "the margin stays clean"
    );
}

// Verifies: gh #207 - a registered panel paints ◀ on the right margin;
// clicking toggles open (▶ on the panel edge) and back.
#[test]
fn drawer_tab_toggles_the_panel() {
    let mut opts = options();
    opts.render_regions = Some(std::sync::Arc::new(|region: &str| {
        if region != "panel" {
            return Vec::new();
        }
        vec![(
            "drawer-demo".to_string(),
            lca_protocol::WidgetTree {
                nodes: vec![lca_protocol::Widget::Text {
                    content: "panel body".to_string(),
                    role: "default".to_string(),
                }],
            },
        )]
    }));
    let mut chat = Chat::new(opts, std::sync::Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    let (w, h) = (80u16, 24u16);
    let (col, row) = chat.drawer_rect(w, h).expect("the tab shows");
    assert_eq!((col, row), (w - 1, h / 2), "right margin, centered");
    let frame = chat.viewport(w, h, 0).join("\n");
    assert!(frame.contains('◀'), "collapsed handle paints");
    assert!(chat.click_drawer(col, row, w, h), "the click lands");
    assert!(chat.world.panel_open, "open");
    let (col, _) = chat.drawer_rect(w, h).expect("the tab shows");
    assert_eq!(col, w - 40, "on the panel edge when open");
    assert!(chat.click_drawer(col, row, w, h), "the click lands");
    assert!(!chat.world.panel_open, "closed again");
}

// Verifies: gh #238 - the drawer glyph splices at the seam: over
// styled transcript rows it opens with a reset (no bleed in) and
// closes with one (no bleed out).
#[test]
fn drawer_glyph_isolates_sgr_state_over_styled_rows() {
    let mut opts = options();
    opts.render_regions = Some(std::sync::Arc::new(|region: &str| {
        if region != "panel" {
            return Vec::new();
        }
        vec![(
            "drawer-demo".to_string(),
            lca_protocol::WidgetTree {
                nodes: vec![lca_protocol::Widget::Text {
                    content: "panel body".to_string(),
                    role: "default".to_string(),
                }],
            },
        )]
    }));
    let mut chat = Chat::new(opts, std::sync::Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    for i in 0..40 {
        chat.transcript
            .push_user(format!("*thinking* question {i}"));
        chat.transcript.append_text(&format!("answer {i}"));
        chat.transcript.finish_assistant();
    }
    let (w, h) = (80u16, 24u16);
    let (_, tab_row) = chat.drawer_rect(w, h).expect("the tab shows");
    let frame = chat.viewport(w, h, 0);
    let row = &frame[tab_row as usize];
    assert!(row.contains('◀'), "the tab paints: {row:?}");
    let reset = lca_tui::engine::core::SEGMENT_RESET;
    let glyph_at = row.find('◀').expect("glyph");
    let entry = row[..glyph_at].rfind(reset).expect("entry reset");
    assert!(
        !row[entry + reset.len()..glyph_at].contains("\x1b[3m"),
        "no italic reaches the glyph: {row:?}"
    );
    let after = glyph_at + '◀'.len_utf8();
    assert!(row[after..].contains(reset), "the glyph closes: {row:?}");
}
