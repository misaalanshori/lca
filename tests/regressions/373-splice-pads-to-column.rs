//! GitHub #373 bug 1: a spliced overlay lands at its column, not at the end of the
//! row's text.
//!
//! `splice_segment` never padded `before` to `x`, so on any base row shorter than the
//! column — which is most rows, since a row's *text* is usually shorter than the
//! column an overlay is placed at — the drawer tab `◀` and the `↓ Jump to latest
//! message` indicator were drawn right after the text instead of where they were
//! placed. The wide-character half is the same defect horizontally: a 2-cell glyph
//! straddling a seam shifted the segment (or the tail) by a column.
//!
//! The behavioural rows live in `crates/lca-ui/src/render_tests.rs`. This file is the
//! NFR-24 named guard: it pins the *shape* from outside, so a future edit cannot
//! quietly drop the pad, split the seam arithmetic again, or hand back a
//! `slice_by_column` tail that resurrects closed styles.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The body of one function, from its `fn` line to the next line that closes it at
/// four-space indent.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #373 bug 1 - the seam both measures and pads.
//
// The pad is the whole fix. Without it the segment lands wherever the base row's text
// stopped. This is the one line the bug was missing, and it must be measured with
// `visible_width` (a byte count would lie about a row holding a wide glyph).
#[test]
fn the_seam_pads_before_to_its_column() {
    let source = read("crates/lca-ui/src/render.rs");
    let body = function_body(&source, "pub fn splice_segment(");
    assert!(
        body.contains("visible_width(&before)"),
        "the pad must measure the surviving slice in cells: {body}"
    );
    assert!(
        body.contains("saturating_sub(visible_width(&before))"),
        "and subtract it from `x` to pad the remainder: {body}"
    );
    assert!(
        !body.contains("before.trim_end()"),
        "trimming `before` would delete the row's own trailing spaces"
    );
}

// Verifies: gh #373 bugs 1 and 2 - one instrument owns both seams.
//
// `slice_by_column(strict = false)` keeps a grapheme straddling the left seam, so
// `before` came out one cell wider than `x` and the segment shifted right; and its
// `pending_ansi` path re-emits styles the base row had already closed. Both slices
// therefore go through `extract_segments`, which is strict on `before` and tracks net
// style. A revert to `slice_by_column` for either slice re-opens one of the two.
#[test]
fn both_seams_go_through_the_style_tracking_instrument() {
    let source = read("crates/lca-ui/src/render.rs");
    let body = function_body(&source, "pub fn splice_segment(");
    assert!(
        body.contains("extract_segments("),
        "the seam must use extract_segments, whose tracker emits net style: {body}"
    );
    // Code lines only: the doc comment above the call *names* `slice_by_column` to
    // explain what it replaced, and a guard that cannot tell prose from code fires
    // on the documentation of its own fix (the trap batch-57 hit).
    let code: String = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("slice_by_column("),
        "and must not slice either side with the non-net path: {code}"
    );
    // The `after` length must be a real bound: `0` means "nothing after" to
    // `extract_segments`, which silently drops the whole tail.
    assert!(
        body.contains("usize::MAX / 2"),
        "the after slice needs a real length, not 0: {body}"
    );
}

// Verifies: gh #373 bug 1 - the two neighbours that always padded still do.
//
// They are the reference shape this fix converged on. If one of them stops padding,
// the convergence is broken and `splice_segment` is again the odd one out.
#[test]
fn the_two_reference_neighbours_still_pad() {
    let source = read("crates/lca-ui/src/render.rs");
    for signature in ["fn side_panel(", "fn overlay_box_placed_windowed("] {
        let body = function_body(&source, signature);
        if signature == "fn side_panel(" {
            // `side_panel` pads, then hands a padded row to the seam.
            assert!(
                body.contains("saturating_sub(visible_width(&before))"),
                "{signature} must keep padding its base side: {body}"
            );
        }
    }
    // The tooltip placement is the third padder; it lives in chat_render.
    let chat = read("crates/lca-ui/src/chat_render.rs");
    assert!(
        chat.contains("saturating_sub(visible_width(&before))"),
        "the tooltip placement must keep padding its base side"
    );
}

// Verifies: gh #373 bug 1 - every production caller passes a column that the base row
// may not reach. This is the sweep the brief asked for: if a caller already pads, the
// seam's pad is a no-op for it; if it does not, the seam now covers it. Either way no
// caller may pre-trim `x` to the row's length to "help".
#[test]
fn no_caller_shortens_its_column_to_the_rows_text() {
    for rel in [
        "crates/lca-ui/src/render.rs",
        "crates/lca-ui/src/chat_overlays.rs",
        "crates/lca-ui/src/chat_render.rs",
    ] {
        let source = read(rel);
        for line in source.lines() {
            let code = line.trim();
            if code.starts_with("//") {
                continue;
            }
            assert!(
                !code.contains(".min(visible_width("),
                "{rel}: a caller must not clamp its column to the row's text width \
                 - the seam owns that: {line}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// gap 3: a modal's height comes from the rows it paints
// ---------------------------------------------------------------------------
//
// `overlay_box_placed` sized the box from `body.len()` — the *unwrapped* line count —
// while painting one row per **wrapped** segment. So a modal with one long line was
// laid out three rows tall and painted seven: the paint ran past its own rect, the
// bottom border fell outside the frame once the body was tall enough to be clipped, and
// `click_modal` — which sizes its hit area from the same unwrapped number — rejected
// clicks on rows that were visibly inside the box.
//
// The behavioural rows live in `crates/lca-ui/src/render_tests.rs`. These pin the shape:

// Verifies: gh #373 gap 3 - **the height is computed from the painted rows, and the
// painter receives them.**
//
// The failure mode this repo has hit repeatedly is "measured one thing, painted another"
// (#384's table cells, #325's mermaid width). The guard is therefore that the height
// expression names the **rows** collection, not `body`, so the two cannot be different
// sets of rows.
#[test]
fn the_modal_height_comes_from_the_painted_rows() {
    let source = read("crates/lca-ui/src/render.rs");
    let body = function_body(&source, "fn overlay_box_placed(");
    assert!(
        body.contains("box_lines.len()"),
        "the height is derived from the rows that will be painted: {body}"
    );
    assert!(
        !body.contains("content_height = u16::try_from(body.len())"),
        "not from the unwrapped body: {body}"
    );
    // And those are the rows handed to the one painter.
    assert!(
        body.contains("overlay_box_placed_windowed("),
        "the same painter the windowed caller uses: {body}"
    );
    assert!(
        body.contains("&box_lines"),
        "and the wrapped rows are what it receives, so the height and the paint agree: \
         {body}"
    );
}

// Verifies: gh #373 gap 3 - **a body taller than the terminal is truncated with a note,
// not silently clipped.**
//
// Once wrapping counts, a long body can exceed the screen. `resolve_overlay_layout`
// clamps the *height*; if nothing else happens the extra rows are dropped wherever the
// clip lands, which is how the box ends up **without a bottom border** — the same defect
// one step out. So the trim is explicit and the frame reports the shortfall.
#[test]
fn a_tall_body_is_truncated_and_the_frame_says_how_much() {
    let source = read("crates/lca-ui/src/render.rs");
    let body = function_body(&source, "fn overlay_box_placed(");
    assert!(
        body.contains("truncate_to_height("),
        "the rows are trimmed to the box's budget: {body}"
    );
    assert!(
        body.contains("bottom_note.as_deref()"),
        "and the shortfall reaches the bottom border: {body}"
    );

    let trim = function_body(&source, "fn truncate_to_height(");
    assert!(
        trim.contains("format!(\"… {hidden} more\")"),
        "the note says how many rows are missing rather than leaving the frame open: \
         {trim}"
    );
    // The budget is read from the layout that owns the clamp, not re-derived.
    assert!(
        trim.contains("resolve_overlay_layout("),
        "the content budget comes from the one layout function: {trim}"
    );
}

// Verifies: gh #373 gap 3 - **the hit-test reads the same wrapped count the paint does.**
//
// `click_modal` sizes its area from `modal_rect(body_len)`. The guard cannot call it
// (it needs a live host), so it pins the seam instead: the hit-test's arithmetic wraps
// each line and counts the segments, which is the same call the painter makes.
#[test]
fn the_hit_test_counts_wrapped_rows_like_the_painter() {
    let mouse = read("crates/lca-ui/src/chat_mouse.rs");
    let body = function_body(&mouse, "fn click_modal(");
    assert!(
        body.contains("wrap_count("),
        "the hit-test counts wrapped rows, not lines: {body}"
    );
    // And it sizes its rect from that count, not from `body.len()`. This is the line
    // that made the hit box smaller than the painted box.
    assert!(
        body.contains("wrapped_rows"),
        "the wrapped count is computed once: {body}"
    );
    assert!(
        body.contains("modal_rect(wrapped_rows"),
        "and the rect is resolved from it, so the hit box matches the paint: {body}"
    );
    assert!(
        !body.contains("modal_rect(body.len()"),
        "not from the unwrapped line count: {body}"
    );
    // The painter's wrapper is the same one.
    let render = read("crates/lca-ui/src/render.rs");
    assert!(
        render.contains("wrap_text_with_ansi(&linkify_urls(line), inner)"),
        "the painter wraps with `wrap_text_with_ansi` over the linkified line"
    );
}

// ---------------------------------------------------------------------------
// gap 4: a percentage positions the overlay, not its corner
// ---------------------------------------------------------------------------
//
// `resolve_overlay_layout` resolved a percentage `row`/`col` as `term * p / 100`
// — a position for the **top-left corner**. So `Percent(50)` put the corner at the
// midpoint instead of centring the overlay, and `Percent(100)` pushed the far edge a
// whole box past the opposite margin (the clamp hid that part, which is why it read as
// "approximately right" on a wide terminal).
//
// The behavioural rows live in `crates/lca-tui/src/engine/core.rs` and
// `crates/lca-ui/src/render_tests.rs`. These pin the shape:

// Verifies: gh #373 gap 4 - **the percentage is a share of the slack, added to the
// margin.**
//
// This is the whole fix, and the one line a future edit could quietly restore: the old
// form resolved the percentage against the terminal dimension (`share(limit)`), which
// positions the corner. Resolving it against `avail - size` and adding the margin is
// what makes the number position the overlay. `avail` and `size` are both in scope at
// that point for exactly this reason.
#[test]
fn a_percentage_is_a_share_of_the_slack_not_of_the_viewport() {
    let source = read("crates/lca-tui/src/engine/core.rs");
    let body = function_body(&source, "fn resolve_axis(");
    assert!(
        body.contains("percent.share(avail - size)"),
        "the percentage distributes the slack between the overlay and the margins: {body}"
    );
    assert!(
        body.contains("i32::from(margin).saturating_add"),
        "starting from the margin, so `Percent(0)` is the margin itself: {body}"
    );
    // The corner-relative reading must not come back.
    for stale in ["share(limit)", "share(term_height)", "share(term_width)"] {
        assert!(
            !body.contains(stale),
            "`{stale}` resolves the corner, which is the defect this gap fixed: {body}"
        );
    }
}

// Verifies: gh #373 gap 4 - **the doc comment states the contract, not the arithmetic.**
//
// The brief's point: the old code's comment accurately described what it did, and what
// it did was not what a reader of `Percent(50)` expects. A formula change alone would
// leave the next reader to infer the meaning from the arithmetic again.
#[test]
fn the_percentage_contract_is_written_down() {
    let source = read("crates/lca-tui/src/engine/core.rs");
    assert!(
        source.contains("A percentage positions the overlay, not its corner"),
        "the contract sentence is the fix's durable half"
    );
    for endpoint in [
        "overlay's near edge on the margin",
        "its far edge on the",
        "opposite margin",
    ] {
        assert!(
            source.contains(endpoint),
            "both endpoints are stated, because they are the values in use: {endpoint}"
        );
    }
    assert!(
        source.contains("an explicit `row`/`col` wins over"),
        "the anchor/percentage precedence is stated where the resolution order is"
    );
}

// Verifies: gh #373 gap 4 - **no production caller asked for a percentage position.**
//
// The brief required the sweep rather than an assumption: every `Percent(` in the tree
// is a `width` or `max_height` (a *size*), and no caller sets `row`/`col` from one. That
// is why the arithmetic change landed without touching a caller - and it is the check
// that would catch a future caller adopting the corrected option and expecting the old
// meaning.
#[test]
fn no_caller_positions_an_overlay_with_a_percentage() {
    for rel in [
        "crates/lca-ui/src/render.rs",
        "crates/lca-ui/src/chat_mouse.rs",
        "crates/lca-ui/src/login_trail.rs",
        "crates/lca-ui/src/chat_settings_footer.rs",
    ] {
        let source = read(rel);
        for (index, line) in source.lines().enumerate() {
            let code = line.trim();
            if code.starts_with("//") {
                continue;
            }
            let positional =
                (code.contains("row:") || code.contains("col:")) && code.contains("Percent(");
            assert!(
                !positional,
                "{rel}:{} positions an overlay with a percentage (row/col); this guard is \
                 where that caller's expectation gets stated: {line}",
                index + 1
            );
        }
    }
}
