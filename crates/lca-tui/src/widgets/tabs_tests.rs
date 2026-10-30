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

// Verifies: gh #326 - the close affordance survives a narrow rail.
//
// The old vertical layout formatted the whole row (`▶ [1] title * ×`), truncated
// **that** to the width, and then placed the close region on the truncated
// string's last column. Past a certain width the `×` itself was cut, so the user
// saw no close button and the host still reported one - a phantom region on the
// last column, which is why clicks "missed" rather than doing nothing.
//
// The fix is an ordering one: reserve the `×` first, then truncate the title into
// what remains. Measured before the fix at width 10: the row read
// `▶ [1] ses…`, with no `×` at all.
#[test]
fn a_narrow_vertical_tab_keeps_its_close_button() {
    let tabs = vertical_rail(3, true);
    for width in [40u16, 24, 16, 14, 12, 10, 8, 6, 4] {
        let rows = tabs.render(width);
        for (index, row) in rows.iter().take(3).enumerate() {
            assert!(
                row.contains('×'),
                "tab {index} keeps its close glyph at width {width}: {row:?}"
            );
            assert!(
                visible_width(row) <= width as usize,
                "tab {index} still fits its width {width}: {row:?}"
            );
        }
    }
}

// Verifies: gh #326 - the close hit region follows the rendered glyph.
//
// The ordering defect's second half: `tab_at` reported `Close(n)` at the last
// column of the *truncated* cell, which after the glyph was cut was the `…` or a
// title character. So a click on the ellipsis closed the tab, and a click on the
// `×` (when it did fit) sometimes did not. The region must be the column the `×`
// actually landed on.
#[test]
fn the_close_region_is_the_column_the_glyph_landed_on() {
    let tabs = vertical_rail(3, true);
    for width in [40u16, 24, 16, 12, 10, 8, 6] {
        let rows = tabs.render(width);
        for (row_index, row) in rows.iter().take(3).enumerate() {
            let glyph_col = char_column_of(row, '×')
                .unwrap_or_else(|| panic!("a `×` at width {width}: {row:?}"));
            assert_eq!(
                tabs.tab_at(glyph_col as u16, row_index as u16, width),
                Some(TabHit::Close(row_index)),
                "the `×` column itself closes tab {row_index} at width {width}: {row:?}"
            );
            // And the region is exactly the glyph: the column before it is tab body.
            if glyph_col > 0 {
                assert_eq!(
                    tabs.tab_at((glyph_col - 1) as u16, row_index as u16, width),
                    Some(TabHit::Tab(row_index)),
                    "the cell before the `×` is the tab body at width {width}: {row:?}"
                );
            }
        }
    }
}

// Verifies: gh #326 - flags are dropped before the close glyph.
//
// The priority order for a narrowing rail, from the outside in: the `×` is an
// affordance and never goes; the flags carry state the title cannot; the title is
// the only thing that can shrink. So a rail one cell too narrow loses a title
// character, and one far too narrow loses its flags, but the `×` stays until
// there is room for nothing but it.
#[test]
fn flags_are_dropped_before_the_close_glyph() {
    let mut tabs = vertical_rail(2, true);
    tabs.items[0].badge = Some("42k".to_string());
    tabs.items[0].is_dirty = true;

    let wide = tabs.render(40);
    assert!(wide[0].contains("(42k)"), "the badge fits: {:?}", wide[0]);
    assert!(wide[0].contains('●'), "the dirty flag fits: {:?}", wide[0]);
    assert!(wide[0].contains('×'), "and the close glyph: {:?}", wide[0]);

    // Squeeze until the badge can no longer fit: the `×` outlives it.
    for width in (6u16..40).rev() {
        let row = &tabs.render(width)[0];
        if !row.contains("(42k)") {
            assert!(
                row.contains('×'),
                "the close glyph outlives the badge at width {width}: {row:?}"
            );
            break;
        }
    }
    // At the narrowest widths the flags are gone and the `×` remains.
    let narrow = tabs.render(4);
    assert!(
        narrow[0].contains('×'),
        "the close glyph is the last thing to go: {:?}",
        narrow[0]
    );
    assert!(
        !narrow[0].contains('●') && !narrow[0].contains("(42k)"),
        "the flags went first: {:?}",
        narrow[0]
    );
}

// Verifies: gh #326 - a tab with no room for a title still renders and closes.
//
// The degenerate case the old layout reached by accident: it printed `▶…` and
// claimed the last column was the close button. It must stay *honest* - the `×`
// alone is the right answer, and clicking it must close the tab.
#[test]
fn a_tab_too_narrow_for_a_title_still_renders_and_closes() {
    let tabs = vertical_rail(2, true);
    for width in [1u16, 2, 3, 4, 5] {
        let rows = tabs.render(width);
        assert_eq!(
            rows.len(),
            3,
            "still one row per tab plus add at width {width}"
        );
        for (row_index, row) in rows.iter().take(2).enumerate() {
            assert!(
                !row.is_empty(),
                "tab {row_index} draws something honest at width {width}"
            );
            assert!(
                visible_width(row) <= width as usize,
                "tab {row_index} fits at width {width}: {row:?}"
            );
        }
        // Wherever the glyph landed it must be hittable; and at these widths the
        // glyph is the only thing that can be there.
        let row = &rows[0];
        if let Some(col) = char_column_of(row, '×') {
            assert_eq!(
                tabs.tab_at(col as u16, 0, width),
                Some(TabHit::Close(0)),
                "the `×` is hittable at width {width}: {row:?}"
            );
        } else {
            // No room even for the glyph: the click must not claim a phantom close.
            let hit = tabs.tab_at((visible_width(row).saturating_sub(1)) as u16, 0, width);
            assert_ne!(
                hit,
                Some(TabHit::Close(0)),
                "no `×` drawn means no close region at width {width}: {row:?}"
            );
        }
    }
}

// Verifies: gh #326 - `0` is a no-op, not a jump to the first tab.
//
// `0.saturating_sub(1) == 0` meant the digit that should select nothing selected
// tab 1. Tabs are numbered from 1, so `0` names no tab; the guard is the digit
// itself.
#[test]
fn the_zero_digit_selects_no_tab() {
    for orientation in [TabOrientation::Horizontal, TabOrientation::Vertical] {
        let mut tabs = vertical_rail(4, true);
        tabs.orientation = orientation;
        tabs.active_index = 2;
        assert_eq!(
            tabs.handle_key("0"),
            None,
            "{orientation:?}: `0` emits no event"
        );
        assert_eq!(
            tabs.active_index, 2,
            "{orientation:?}: `0` does not move the active tab"
        );
        // And the digits that do name tabs still work.
        assert_eq!(
            tabs.handle_key("1"),
            Some(TabEvent::Select(0)),
            "{orientation:?}: `1` still selects the first tab"
        );
        assert_eq!(
            tabs.handle_key("0"),
            None,
            "{orientation:?}: `0` stays quiet after a real jump"
        );
        assert_eq!(tabs.active_index, 0, "{orientation:?}: still on tab 1");
    }
}

/// A vertical rail of `count` closable tabs named `session-N`.
fn vertical_rail(count: usize, add: bool) -> TabsWidget {
    TabsWidget {
        orientation: TabOrientation::Vertical,
        items: (1..=count)
            .map(|n| TabItem {
                id: format!("t{n}"),
                title: format!("session-{n}"),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: true,
            })
            .collect(),
        active_index: 0,
        show_add_button: add,
        hovered: None,
    }
}

/// The cell column where `glyph` is painted in `row`, in the widget's own
/// coordinate space.
///
/// Measured as the visible width of the **prefix** up to the glyph, not by
/// walking characters: `truncate_to_width` emits `\x1b[0m` resets around the
/// ellipsis, and a per-character walk counts each of those four bytes as one
/// cell, so it reports a column three cells past the truth. (My first version did
/// exactly that, and the row "failed" at a column the glyph had never occupied.)
fn char_column_of(row: &str, glyph: char) -> Option<usize> {
    let bytes_before = row.find(glyph)?;
    Some(visible_width(&row[..bytes_before]))
}

// Verifies: gh #326 - a non-closable rail row has no close region at all.
//
// The old layout already guarded this (`item.closable.then(...)`), so it is a
// **regression** row rather than a red-before one: a non-closable tab paints no
// `×`, and the region must stay absent rather than falling back to "the last
// column", which is what a width-derived rule would do.
#[test]
fn a_rail_row_without_a_close_glyph_has_no_close_region() {
    let mut tabs = vertical_rail(2, true);
    tabs.items[0].closable = false;
    for width in [40u16, 20, 12, 8, 4, 2] {
        let row = &tabs.render(width)[0];
        assert!(
            !row.contains('×'),
            "a non-closable tab paints no glyph at width {width}: {row:?}"
        );
        let last = visible_width(row).saturating_sub(1) as u16;
        assert_eq!(
            tabs.tab_at(last, 0, width),
            Some(TabHit::Tab(0)),
            "the last column is the tab body, not a phantom close, at width {width}: {row:?}"
        );
    }
    // The closable sibling still answers, so this is not the region being off.
    let sibling = &tabs.render(40)[1];
    let col = char_column_of(sibling, '×').expect("tab 1 is closable");
    assert_eq!(tabs.tab_at(col as u16, 1, 40), Some(TabHit::Close(1)));
}

// Verifies: gh #326 - the region is measured by finding the glyph, not by
// taking the row's last column.
//
// This is the row that separates the two rules. After the ordering fix the glyph
// *is* always painted last, so the two rules agree for every row the widget
// produces today - which is exactly why the derivation is pinned here, at the
// function that owns it, instead of only through `tab_at`. A future change that
// paints anything after the `×` (a badge, a modified marker) would silently make
// "the last column" wrong again, and this row is what notices.
#[test]
fn the_region_is_measured_by_finding_the_glyph_not_the_row_end() {
    // A row whose glyph is followed by more cells: the width-derived rule reports
    // the trailing cells' column, the glyph-derived rule reports the glyph's.
    // `▶ [1] se ` is 9 cells, so the `×` owns cell 9.
    let observed = rendered_close_region("▶ [1] se × ··", true);
    assert_eq!(
        observed,
        Some((9, 10)),
        "the region is the glyph's own cell, wherever it sits: {observed:?}"
    );
    // No glyph painted, nothing to close - even though the row has a last column.
    assert_eq!(rendered_close_region("▶ [1] session", true), None);
    // And a non-closable row never has one, whatever it paints.
    assert_eq!(rendered_close_region("▶ [1] se ×", false), None);
}

// Verifies: gh #326 - the horizontal bar never paints wider than its width.
//
// Found while checking the vertical fix against the surface the product actually
// uses (`chat_tabs.rs` only ever builds a `Horizontal` bar). Measured on the
// pre-fix build: **4 tabs at width 80 painted 82 cells**, and the overflow grew
// with the tab count (2 tabs at 40 → 48, 1 tab at 10 → 16). The narrowing loop
// shaves titles against a `total` it computes from `chrome()` + title widths, but
// `chrome()` under-counted the flags/badge they actually add, so the loop
// believed the bar fitted when it did not; the extra cells then simply painted
// past the pane.
//
// Same defect class as this issue - a row that does not fit - one surface over,
// so it is pinned here rather than filed blind.
#[test]
fn a_horizontal_bar_never_paints_wider_than_its_width() {
    for tabs_count in [1usize, 2, 3, 4, 6] {
        for badge in [None, Some("*".to_string()), Some("42k".to_string())] {
            for add in [false, true] {
                let mut widget = TabsWidget {
                    orientation: TabOrientation::Horizontal,
                    items: (1..=tabs_count)
                        .map(|n| TabItem {
                            id: format!("t{n}"),
                            title: format!("session-number-{n}"),
                            badge: badge.clone(),
                            is_generating: false,
                            is_dirty: true,
                            closable: true,
                        })
                        .collect(),
                    active_index: 0,
                    show_add_button: add,
                    hovered: None,
                };
                widget.items[0].is_generating = true;
                for width in [80u16, 60, 40, 20, 10] {
                    let row = &widget.render(width)[0];
                    assert!(
                        visible_width(row) <= width as usize,
                        "{tabs_count} tabs, badge {badge:?}, add {add} at width {width}: \
                         the bar painted {} cells: {row:?}",
                        visible_width(row)
                    );
                }
            }
        }
    }
}

// Verifies: gh #326 - every tab that survives on the bar keeps its close glyph.
//
// The affordance rule applies to the bars the user can see: if a tab is painted,
// its `×` is painted, because the host hit-tests those cells. A bar too narrow for
// all its tabs drops whole tabs from the right (never the active one) rather than
// painting truncated ones - so the assertion is per *painted* tab, not "every tab
// in the list".
#[test]
fn every_painted_horizontal_tab_keeps_its_close_glyph() {
    for tabs_count in [1usize, 2, 3, 4] {
        for width in [80u16, 60, 40, 20, 12] {
            let widget = TabsWidget {
                orientation: TabOrientation::Horizontal,
                items: (1..=tabs_count)
                    .map(|n| TabItem {
                        id: format!("t{n}"),
                        title: format!("session-number-{n}"),
                        badge: None,
                        is_generating: false,
                        is_dirty: false,
                        closable: true,
                    })
                    .collect(),
                active_index: 0,
                show_add_button: false,
                hovered: None,
            };
            let row = &widget.render(width)[0];
            // Tabs are painted as `[ n. `, so the index marker is what counts them
            // (a `[` count would also catch ANSI resets and the `[+]` cell).
            let painted = row.matches(". ").count() - row.matches("[+]").count();
            assert!(painted >= 1, "at least the active tab is painted: {row:?}");
            assert_eq!(
                row.matches('×').count(),
                painted,
                "{tabs_count} tabs at width {width}: every painted tab keeps its glyph: {row:?}"
            );
            // And the active tab is never the one dropped: it is first on the bar.
            assert!(
                row.contains("[ 1. ") || row.contains("[1."),
                "the active (first) tab survives at width {width}: {row:?}"
            );
        }
    }
}

// Verifies: gh #326 - dropping a tab is the last resort, and it drops from the right.
//
// The bar gives up, in order: flags, then `[+]`, then whole tabs from the right.
// A tab that does fit is never truncated in place of dropping the one that does
// not, and the active tab is never the victim.
#[test]
fn a_bar_too_narrow_drops_tabs_rather_than_truncating_them_all() {
    let widget = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: (1..=4)
            .map(|n| TabItem {
                id: format!("t{n}"),
                title: "x".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: true,
            })
            .collect(),
        active_index: 0,
        show_add_button: true,
        hovered: None,
    };
    let wide = &widget.render(200)[0];
    assert_eq!(
        wide.matches('×').count(),
        4,
        "all four fit when there is room"
    );
    assert!(wide.contains("[+]"), "and `[+]` is there");
    let narrow = &widget.render(24)[0];
    assert!(
        narrow.matches('×').count() < 4,
        "a narrow bar drops tabs instead of overflowing: {narrow:?}"
    );
    assert!(visible_width(narrow) <= 24, "and still fits: {narrow:?}");
}

// Verifies: gh #326 - the bar never hides the active tab to make room.
//
// Found while mutation-testing the drop rule: with the active tab last, dropping
// "from the right" removed exactly it, so a narrow bar showed tabs 1 and 2 while
// the session the user was in vanished from the bar entirely. The bar's job is to
// say where they are, so the active tab is never the victim.
#[test]
fn a_narrow_bar_never_drops_the_active_tab() {
    for active in [0usize, 1, 2, 3] {
        for width in [80u16, 40, 30, 24, 20, 14] {
            let widget = TabsWidget {
                orientation: TabOrientation::Horizontal,
                items: (1..=4)
                    .map(|n| TabItem {
                        id: format!("t{n}"),
                        title: format!("session-number-{n}"),
                        badge: None,
                        is_generating: false,
                        is_dirty: false,
                        closable: true,
                    })
                    .collect(),
                active_index: active,
                show_add_button: false,
                hovered: None,
            };
            let row = &widget.render(width)[0];
            let painted = row.matches(". ").count();
            assert!(
                (1..=4).contains(&painted),
                "the bar paints at least one tab: {row:?}"
            );
            // The active tab is the one carrying the `*` marker; it must be on the bar.
            assert!(
                row.contains('*'),
                "the active tab (index {active}) is still on the bar at width {width}: {row:?}"
            );
        }
    }
    // And the active tab is the *only* one that may carry `*`, so the assertion
    // above cannot be satisfied by a stale flag on some other tab.
    let widget = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: (1..=4)
            .map(|n| TabItem {
                id: format!("t{n}"),
                title: "s".to_string(),
                badge: None,
                is_generating: false,
                is_dirty: false,
                closable: true,
            })
            .collect(),
        active_index: 2,
        show_add_button: false,
        hovered: None,
    };
    assert_eq!(widget.render(200)[0].matches('*').count(), 1);
}

// Verifies: gh #326 - flag-dropping never removes the `*` active marker.
//
// The active marker is not a state flag like `●`/`◌`/(badge): it is the bar's one
// job, saying which session the user is in. It is the last thing a narrowing bar
// may give up, and at any width that can fit a tab at all it must be present.
#[test]
fn the_active_marker_survives_every_width_that_can_show_a_tab() {
    for active in [0usize, 1, 2, 3] {
        for width in [80u16, 60, 40, 30, 24, 20, 14, 10] {
            let widget = TabsWidget {
                orientation: TabOrientation::Horizontal,
                items: (1..=4)
                    .map(|n| TabItem {
                        id: format!("t{n}"),
                        title: format!("session-number-{n}"),
                        badge: Some("42k".to_string()),
                        is_generating: true,
                        is_dirty: true,
                        closable: true,
                    })
                    .collect(),
                active_index: active,
                show_add_button: true,
                hovered: None,
            };
            let row = &widget.render(width)[0];
            assert_eq!(
                row.matches('*').count(),
                1,
                "exactly one active marker, on the active tab, at width {width}: {row:?}"
            );
            assert!(
                visible_width(row) <= width as usize,
                "and the bar still fits at width {width}: {row:?}"
            );
        }
    }
}

// Verifies: gh #326 - flag priority: (badge) goes before `●`/`◌`, and `*` last.
//
// Stated as the order the bar gives things up, because "which flag disappeared
// first" is not observable from a single width - it needs a sweep from wide to
// narrow, recording the width at which each one vanishes.
#[test]
fn flags_are_given_up_badge_first_then_markers_never_the_active_star() {
    let widget = TabsWidget {
        orientation: TabOrientation::Horizontal,
        items: vec![TabItem {
            id: "a".to_string(),
            title: "x".to_string(),
            badge: Some("42k".to_string()),
            is_generating: true,
            is_dirty: true,
            closable: true,
        }],
        active_index: 0,
        show_add_button: false,
        hovered: None,
    };
    let mut lost_badge = None;
    let mut lost_markers = None;
    let mut lost_star = None;
    for width in (4u16..40).rev() {
        let row = &widget.render(width)[0];
        if lost_badge.is_none() && !row.contains("42k") {
            lost_badge = Some(width);
        }
        if lost_markers.is_none() && !row.contains('●') && !row.contains('◌') {
            lost_markers = Some(width);
        }
        if lost_star.is_none() && !row.contains('*') {
            lost_star = Some(width);
        }
    }
    let (Some(badge), Some(markers)) = (lost_badge, lost_markers) else {
        panic!(
            "the badge and the markers must be given up somewhere below 40: {lost_badge:?} {lost_markers:?}"
        );
    };
    assert!(
        badge > markers,
        "the badge goes before the markers (badge at {badge}, markers at {markers})"
    );
    assert!(
        lost_star.map(|star| star < markers).unwrap_or(true),
        "and the `*` only after them, if ever (star at {lost_star:?}, markers at {markers})"
    );
}
