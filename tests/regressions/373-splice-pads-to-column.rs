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
