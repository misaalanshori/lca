use super::*;

#[test]
fn overlay_layout_resolves_anchor_and_clamps() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        anchor: Some(Anchor::Center),
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 80, 24, 6);
    assert_eq!(rect.width, 20);
    assert_eq!(rect.height, 6);
    assert_eq!(rect.col, 30);
    assert_eq!(rect.row, 9);
    // Over-large content is clamped into the margin box.
    let opts = OverlayOptions {
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 20, 10, 40);
    assert!(rect.row + rect.height <= 10);
    assert!(rect.col + rect.width <= 20);
}

// Verifies: gh #373 gap 4 - a percentage positions the **overlay**, not its
// corner, so `Percent(50)` centres it in the space between the margins. The
// assertion is the *gaps*, because the gaps are the property: a formula that
// happened to produce the same number from a corner-anchored rule would not
// keep them equal when the box or the terminal changes shape.
#[test]
fn a_percentage_centres_the_overlay_in_the_margin_box() {
    for (term_w, term_h, margin, box_w, box_h) in [
        (80u16, 24u16, 0u16, 20u16, 6u16),
        (80, 24, 2, 20, 6),
        (81, 25, 0, 20, 7),
        (120, 40, 4, 50, 11),
    ] {
        let opts = OverlayOptions {
            width: Some(SizeValue::Abs(box_w)),
            row: Some(SizeValue::Percent(50)),
            col: Some(SizeValue::Percent(50)),
            margin,
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&opts, term_w, term_h, box_h);
        assert_eq!(rect.width, box_w);
        assert_eq!(rect.height, box_h);

        let left = rect.col.saturating_sub(margin);
        let right = (term_w - margin).saturating_sub(rect.col + rect.width);
        assert!(
            left.abs_diff(right) <= 1,
            "horizontal gaps differ by more than a cell at {term_w}x{term_h} margin {margin}: \
             left {left} right {right} (col {})",
            rect.col
        );

        let top = rect.row.saturating_sub(margin);
        let bottom = (term_h - margin).saturating_sub(rect.row + rect.height);
        assert!(
            top.abs_diff(bottom) <= 1,
            "vertical gaps differ by more than a cell at {term_w}x{term_h} margin {margin}: \
             top {top} bottom {bottom} (row {})",
            rect.row
        );
    }
}

// Verifies: gh #373 gap 4 - the endpoints are the values actually in use, and a
// centring formula can break both: `Percent(0)` must stay exactly on the margin
// (not one row above it, not the margin doubled).
#[test]
fn a_zero_percentage_lands_on_the_margin() {
    for margin in [0u16, 1, 2, 3] {
        let opts = OverlayOptions {
            width: Some(SizeValue::Abs(20)),
            row: Some(SizeValue::Percent(0)),
            col: Some(SizeValue::Percent(0)),
            margin,
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&opts, 80, 24, 6);
        assert_eq!(rect.col, margin, "col at margin {margin}");
        assert_eq!(rect.row, margin, "row at margin {margin}");
    }
}

// Verifies: gh #373 gap 4 - `Percent(100)` puts the overlay's **far edge** on the
// opposite margin. This is the half a pure centring fix breaks by resolving the
// corner at `term - margin`, which would push the far edge a whole box past it.
#[test]
fn a_hundred_percentage_puts_the_far_edge_on_the_margin() {
    for margin in [0u16, 1, 2, 3] {
        let opts = OverlayOptions {
            width: Some(SizeValue::Abs(20)),
            row: Some(SizeValue::Percent(100)),
            col: Some(SizeValue::Percent(100)),
            margin,
            ..Default::default()
        };
        let rect = resolve_overlay_layout(&opts, 80, 24, 6);
        assert_eq!(
            rect.col + rect.width,
            80 - margin,
            "right edge at margin {margin}"
        );
        assert_eq!(
            rect.row + rect.height,
            24 - margin,
            "bottom edge at margin {margin}"
        );
    }
}

// Verifies: gh #373 gap 4 - an out-of-range percentage is clamped to the far
// margin rather than producing a position outside the box (a config-shaped number
// reaches this function, so the range is enforced here rather than trusted).
//
// The observable rule is "anything above 100 resolves as 100"; the clamp is what
// makes that true, which is why `share`'s own saturation is not the half this row
// fails against (see the report's mutation table).
#[test]
fn an_out_of_range_percentage_is_clamped_to_the_far_margin() {
    let at_hundred = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        row: Some(SizeValue::Percent(100)),
        col: Some(SizeValue::Percent(100)),
        margin: 2,
        ..Default::default()
    };
    let beyond = OverlayOptions {
        row: Some(SizeValue::Percent(250)),
        col: Some(SizeValue::Percent(u16::MAX)),
        ..at_hundred.clone()
    };
    let at = resolve_overlay_layout(&at_hundred, 80, 24, 6);
    let past = resolve_overlay_layout(&beyond, 80, 24, 6);
    assert_eq!(past, at, "anything above 100 resolves the same as 100");
    assert!(past.col + past.width <= 80 - 2);
    assert!(past.row + past.height <= 24 - 2);
}

// Verifies: gh #373 gap 4 - the anchor does not silently override a percentage.
// Two controls answering the same question is how this bug gets filed again, so
// the precedence is pinned: the explicit position wins, and the axis it does not
// set still falls back to the anchor.
#[test]
fn an_anchor_does_not_override_the_percentage() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        anchor: Some(Anchor::TopLeft),
        row: Some(SizeValue::Percent(50)),
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 80, 24, 6);
    assert_eq!(rect.row, 9, "the percentage sets the row, not the anchor");
    assert_eq!(rect.col, 0, "the unset axis still obeys the anchor");
}

// Verifies: gh #373 gap 4 - the clamp sits **after** the position is resolved, and
// it is what keeps an overlay inside its box.
//
// Under the corrected percentage form the clamp is no longer load-bearing *for a
// percentage on its own*: `share(avail - size)` cannot exceed the slack, so the far
// edge lands on the margin by construction. What still needs it is a position plus a
// displacement - an offset, or an absolute row/col - which is what this row drives,
// because a clamp with nothing reaching it is a clamp nobody would notice losing.
#[test]
fn a_displaced_overlay_is_clamped_inside_its_box() {
    let pushed_right_and_down = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        row: Some(SizeValue::Percent(100)),
        col: Some(SizeValue::Percent(100)),
        offset_x: 40,
        offset_y: 40,
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&pushed_right_and_down, 80, 24, 6);
    assert_eq!(
        (rect.col, rect.row),
        (60, 18),
        "the far edge stops at the screen edge, the reach this clamp always had"
    );
    assert_eq!(rect.col + rect.width, 80);
    assert_eq!(rect.row + rect.height, 24);

    let pushed_left_and_up = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        row: Some(SizeValue::Percent(0)),
        col: Some(SizeValue::Percent(0)),
        offset_x: -40,
        offset_y: -40,
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&pushed_left_and_up, 80, 24, 6);
    assert_eq!((rect.col, rect.row), (2, 2), "clamped to the near margin");

    // An absolute position past the edge is the third way in.
    let far = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        row: Some(SizeValue::Abs(500)),
        col: Some(SizeValue::Abs(500)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&far, 80, 24, 6);
    assert_eq!((rect.col + rect.width, rect.row + rect.height), (80, 24));
}

// Verifies: gh #373 gap 4 - an offset applies on top of a percentage, as in pi's
// `resolveOverlayLayout` (which adds `offsetY`/`offsetX` after either form).
#[test]
fn an_offset_applies_on_top_of_a_percentage() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(20)),
        row: Some(SizeValue::Percent(50)),
        col: Some(SizeValue::Percent(50)),
        offset_y: 1,
        offset_x: -5,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 80, 24, 6);
    assert_eq!(rect.row, 10, "9 from the percentage plus 1");
    assert_eq!(rect.col, 25, "30 from the percentage minus 5");
}

// Verifies: gh #373 gap 4 - a box that fills the available space has no slack for
// a percentage to distribute, so both endpoints collapse onto the margin instead of
// dividing by a zero span.
#[test]
fn a_full_span_percentage_pins_the_overlay_to_the_margin() {
    let opts = OverlayOptions {
        row: Some(SizeValue::Percent(100)),
        col: Some(SizeValue::Percent(100)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 80, 24, 99);
    assert_eq!((rect.col, rect.row), (2, 2));
    assert_eq!((rect.width, rect.height), (76, 20));
}

// Verifies: gh #373 gap 6 - a percentage width has no ceiling, so a `Percent(60)`
// modal on a 200-column terminal blows out to 120 columns. `max_width` gives it one.
#[test]
fn a_max_width_ceiling_clamps_a_percentage_width() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Percent(60)),
        max_width: Some(SizeValue::Abs(80)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 200, 40, 6);
    assert_eq!(rect.width, 80, "the ceiling wins over 60% of 196");

    // Under the ceiling the percentage is untouched, so the option is a
    // ceiling and not a fixed width.
    let narrow = OverlayOptions {
        width: Some(SizeValue::Percent(60)),
        max_width: Some(SizeValue::Abs(80)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&narrow, 100, 40, 6);
    assert_eq!(rect.width, 57, "60% of the 96-column margin box");
}

// Verifies: gh #373 gap 6 - a `max_width` percentage is a share of the **margin
// box**, the same reference every other width percentage uses, so it shrinks with
// the terminal instead of pinning a column count to a wide screen.
#[test]
fn a_percentage_max_width_tracks_the_margin_box() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(500)),
        max_width: Some(SizeValue::Percent(50)),
        margin: 2,
        ..Default::default()
    };
    let wide = resolve_overlay_layout(&opts, 200, 40, 6);
    assert_eq!(wide.width, 98, "half of the 196-column margin box");
    let narrow = resolve_overlay_layout(&opts, 100, 40, 6);
    assert_eq!(narrow.width, 48, "half of 96");
}

// Verifies: gh #373 gap 6 - the ceiling is applied **before** the position chain
// resolves, so a narrower width moves a centred overlay's column.
//
// This is the row that distinguishes the order. With the ceiling applied after the
// position, the column would still be computed from the unclamped width and the
// catalogue's own bug - "a `Percent` computes an out-of-range value and the clamp
// silently truncates it" - would show as a box off-centre by half the overhang.
#[test]
fn the_width_ceiling_is_applied_before_the_position_resolves() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Percent(60)),
        max_width: Some(SizeValue::Abs(80)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 200, 40, 6);
    assert_eq!(rect.width, 80);
    assert_eq!(
        rect.col, 60,
        "the centred column follows the clamped width: (200 - 80) / 2"
    );
    // A ceiling applied after positioning would have left col at 40 (the centre
    // of the 120-column percentage width) and painted an 80-wide box there.
    assert_ne!(rect.col, 40);
}

// Verifies: gh #373 gap 6 - `max_width` and `min_width` cannot both win, and the
// answer is that **the ceiling loses**: a minimum is a floor the box cannot go
// below to stay readable, and a caller that sets an impossible pair meant the
// minimum. What must not happen is the pair exceeding the terminal, so the margin
// box is the real bound either way.
#[test]
fn a_min_width_above_the_ceiling_keeps_the_minimum_but_not_the_screen() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(30)),
        min_width: Some(80),
        max_width: Some(SizeValue::Abs(60)),
        margin: 2,
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&opts, 200, 40, 6);
    assert_eq!(rect.width, 80, "the floor wins over the ceiling");
    assert!(rect.width <= 196, "and still fits the margin box");

    // On a terminal narrower than the floor, the margin box is the bound: a
    // minimum that exceeds the screen is a narrow-pane regression, not a right.
    let tight = resolve_overlay_layout(&opts, 60, 20, 6);
    assert_eq!(tight.width, 56, "the 56-column margin box, not 80");
}

// Verifies: gh #373 gap 6 - a responsive `visible` predicate is **asked every
// resolve**, because the defect it fixes is options that owners cache.
//
// A HUD is expressible as `visible: |w, _| w >= 90`, and the same options value
// gives a different answer at two terminal sizes with no re-construction in
// between - which is exactly what a cached static `bool` cannot do.
#[test]
fn a_visibility_predicate_is_asked_again_on_every_resolve() {
    let opts = OverlayOptions {
        width: Some(SizeValue::Abs(40)),
        visible: Some(&|cols, _rows| cols >= 90),
        ..Default::default()
    };
    assert!(
        resolve_overlay_layout(&opts, 120, 40, 6).visible,
        "above the breakpoint"
    );
    assert!(
        !resolve_overlay_layout(&opts, 60, 40, 6).visible,
        "below it, from the same options value"
    );
    assert!(
        resolve_overlay_layout(&opts, 90, 40, 6).visible,
        "and the breakpoint itself is inclusive"
    );
}

// Verifies: gh #373 gap 6 - the predicate is asked with the **terminal's** size, not
// the margin box's (pi passes `terminal.columns`/`terminal.rows` to `visible`).
//
// A breakpoint written as "hide unless the terminal is at least 90 columns" would
// fire four columns early if it were handed an inset box, which is the kind of
// off-by-a-margin nobody notices until a HUD disappears over one window drag.
#[test]
fn the_visibility_predicate_sees_the_terminal_size_not_the_margin_box() {
    let seen = std::cell::Cell::new((0u16, 0u16));
    let options = OverlayOptions {
        margin: 2,
        visible: Some(&|cols, rows| {
            seen.set((cols, rows));
            true
        }),
        ..Default::default()
    };
    let _ = resolve_overlay_layout(&options, 100, 30, 4);
    assert_eq!(seen.get(), (100, 30), "the terminal's own size, not 96x26");
}

// Verifies: gh #373 gap 6 - a hidden overlay still resolves a rectangle.
//
// The geometry must stay well-defined when the predicate says no: a caller that
// reads the rect for a hit test or a cursor anchor gets a plausible box rather
// than a zeroed one, and the visibility flag is what the caller gates on. This is
// also what keeps `visible` from becoming a sizing option by accident.
#[test]
fn a_hidden_overlay_still_resolves_a_rectangle() {
    let hidden = OverlayOptions {
        width: Some(SizeValue::Abs(40)),
        visible: Some(&|_, _| false),
        ..Default::default()
    };
    let rect = resolve_overlay_layout(&hidden, 100, 40, 6);
    assert_eq!((rect.width, rect.height), (40, 6));
    assert_eq!(rect.col, 30, "still centred, so a caller can anchor to it");
    assert!(!rect.visible);
}

// Verifies: gh #373 gap 6 - an overlay with no predicate is visible, so every
// existing caller keeps painting exactly what it painted before.
#[test]
fn an_overlay_without_a_predicate_is_visible() {
    let opts = OverlayOptions::default();
    assert!(resolve_overlay_layout(&opts, 80, 24, 4).visible);
}

// Verifies: gh #373 gap 6 (the row whose *absence* let a clamp change through) -
// **the bottom-anchored picker's absolute row.**
//
// The picker counts rows up from the viewport bottom (`offset_y = -above_rows`), so
// its row is a pure arithmetic fact about the terminal, the box height and the
// composer. Nothing pinned it, which is why tightening the clamp's upper bound
// moved it silently. These are the numbers, not a property: a property would have
// caught the clamp change just as well as no test did.
#[test]
fn a_bottom_anchored_picker_lands_on_its_exact_row() {
    // The shape `picker_overlay_options` builds: Percent(80) width, margin 2,
    // BottomCenter, lifted above the composer by `above_rows`.
    let picker = |above_rows: i32| OverlayOptions {
        width: Some(SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(SizeValue::Abs(24)),
        margin: 2,
        anchor: Some(Anchor::BottomCenter),
        offset_y: -above_rows,
        ..Default::default()
    };

    // 100x30, a 10-row box (8 content rows plus 2 frame rows), composer 3 rows tall.
    let rect = resolve_overlay_layout(&picker(3), 100, 30, 10);
    assert_eq!(
        rect.row, 17,
        "30 - 10 - 3: bottom-anchored above the composer"
    );
    // 80% of the 96-column margin box is 76, and the anchor centres that.
    assert_eq!(rect.width, 76);
    assert_eq!(rect.col, 12, "(100 - 76) / 2");

    // A taller box moves up by exactly its extra height.
    let taller = resolve_overlay_layout(&picker(3), 100, 30, 14);
    assert_eq!(taller.row, 13, "30 - 14 - 3");

    // A taller composer moves it up by exactly the composer's growth.
    let lifted = resolve_overlay_layout(&picker(5), 100, 30, 10);
    assert_eq!(lifted.row, 15, "30 - 10 - 5");

    // The bottom edge is the row before the composer starts, which is the
    // property the picker exists for; the absolute rows above are what pin it.
    assert_eq!(rect.row + rect.height, 27);
    assert_eq!(lifted.row + lifted.height, 25);

    // **The case where the clamp actually reaches.** With a small lift the
    // anchor lands inside the bound and nothing tests it; at rest (`above_rows`
    // 0 and 1) the box's bottom edge is the terminal's own, and that is the
    // position a tightened clamp moves. This is the half that fails if the
    // upper bound is ever narrowed to the far margin: the picker would rise by
    // `margin` rows and detach from the composer it is pinned above.
    let at_rest = resolve_overlay_layout(&picker(0), 100, 30, 10);
    assert_eq!(
        at_rest.row, 20,
        "the box's bottom edge is the terminal's own"
    );
    assert_eq!(at_rest.row + at_rest.height, 30);
    let one_up = resolve_overlay_layout(&picker(1), 100, 30, 10);
    assert_eq!(one_up.row, 19, "one row up, not three");
    assert_eq!(one_up.row + one_up.height, 29);
}

#[test]
fn cursor_marker_is_extracted_and_stripped() {
    let lines = vec!["hello".to_string(), format!("ab{CURSOR_MARKER}cd")];
    let (out, pos) = extract_cursor_position(&lines);
    assert_eq!(pos, Some((1, 2, false)), "no painted caret in this row");
    assert_eq!(out[1], "abcd");
    assert!(!out[1].contains("pi:c"));
}

#[test]
fn capability_replies_are_recognized() {
    assert_eq!(
        parse_capability_reply("\x1b]11;#000000\x07"),
        Some(CapabilityReply::BackgroundColor(
            super::super::colors::RgbColor { r: 0, g: 0, b: 0 }
        ))
    );
    assert_eq!(
        parse_capability_reply("\x1b[?997;2n"),
        Some(CapabilityReply::ColorScheme(
            super::super::colors::ColorScheme::Light
        ))
    );
    assert_eq!(
        parse_capability_reply("\x1b[6;16;8t"),
        Some(CapabilityReply::CellSize {
            height: 16,
            width: 8
        })
    );
    assert_eq!(parse_capability_reply("\x1b[A"), None);
}

#[test]
fn width_violation_detects_overlong_lines() {
    let lines = vec!["ok".to_string(), "this is too long".to_string()];
    assert_eq!(width_violation(&lines, 5), Some((1, 16)));
    assert_eq!(width_violation(&lines, 80), None);
}
