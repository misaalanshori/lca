//! GitHub #378 gaps 1-2: the width fast path allocates on nothing, and an SGR
//! arriving at the slice seam keeps its order.
//!
//! **Gap 1** — `visible_width`'s fast path was `is_printable_ascii`, which rejects any
//! byte outside `0x20..=0x7e`. `\x1b` is outside it, so *every rendered line* (all of
//! which carry SGR) cloned, replaced tabs, stripped sequences, segmented graphemes and
//! took the width-cache lock. The fast path now walks bytes and skips recognised escape
//! sequences without substringing, declining the moment it meets anything it cannot
//! decide.
//!
//! **Gap 2** — in `slice_with_width`, an SGR arriving *at* `start_col` was pushed to
//! `result` before the buffered pre-`start_col` codes flushed, so `\x1b[31m` followed
//! by `\x1b[0m` at the seam emitted `\x1b[0m\x1b[31m` and **unstyled text turned red**.
//!
//! The behavioural rows live in `crates/lca-tui/src/engine/text_tests.rs`. This file is
//! the NFR-24 named guard: it pins the shape from outside, so the fast path cannot
//! start guessing escape lengths, the ordering cannot reverse again, and the fallback
//! cannot be quietly deleted.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The module's code, without its test module.
///
/// `text.rs` has several `#[cfg(test)]` blocks (`the hit counter is one), so the cut is
/// the **last** one that introduces the test module.
fn text_source() -> String {
    let source = read("crates/lca-tui/src/engine/text.rs");
    match source.rfind("\n#[cfg(test)]\n#[path") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #378 gap 1 - the fast path is byte-driven and allocates nothing.
//
// The whole point is that a coloured row no longer clones, replaces, strips or locks.
// A `to_string()`/`replace`/`to_lowercase` inside this function would restore the old
// cost while every behavioural row still passed.
#[test]
fn the_fast_path_does_not_allocate() {
    let source = text_source();
    let body = function_body(&source, "pub fn ascii_visible_width(");
    for forbidden in [
        ".to_string()",
        ".to_owned()",
        ".replace(",
        ".collect::<",
        ".to_lowercase(",
        "String::from",
        "format!",
    ] {
        assert!(
            !body.contains(forbidden),
            "the fast path must not allocate (`{forbidden}` found): {body}"
        );
    }
    assert!(body.contains("as_bytes()"), "it must walk bytes: {body}");
}

// Verifies: gh #378 gap 1 - an unrecognised escape declines rather than guessing.
//
// **This is the load-bearing property.** A wrong skip length silently mismeasures
// every row after the escape, and a mismeasurement corrupts layout far from its cause;
// a decline merely costs the old path. So the skip helper must answer `0` for anything
// it does not know, and the walker must turn that `0` into `None`.
#[test]
fn an_unknown_escape_declines_instead_of_guessing() {
    let source = text_source();
    let skip = function_body(&source, "fn ansi_code_length(");
    assert!(
        skip.contains("0\n    }") || skip.contains("            0\n"),
        "ansi_code_length must answer 0 when it cannot decide: {skip}"
    );
    // The closed CSI final set, matching `extract_ansi_code` and pi's `ansiCodeLength`.
    for final_byte in ["b'm'", "b'G'", "b'K'", "b'H'", "b'J'"] {
        assert!(
            skip.contains(final_byte),
            "the CSI final-byte set must include {final_byte}: {skip}"
        );
    }
    assert!(
        skip.contains("b'\\\\'"),
        "and OSC/APC must accept the ST (ESC backslash) terminator: {skip}"
    );
    // The walker's decline.
    let walk = function_body(&source, "pub fn ascii_visible_width(");
    assert!(
        walk.contains("return None"),
        "the walker must decline rather than continue: {walk}"
    );
}

// Verifies: gh #378 gap 1 - the slow path still exists and is still the fallback.
//
// An optimisation that deleted the correct path would pass the equivalence table
// trivially (both sides would be the fast path) and then mismeasure every wide line.
#[test]
fn the_slow_path_survives_and_visible_width_falls_back_to_it() {
    let source = text_source();
    assert!(
        source.contains("fn visible_width_slow("),
        "the grapheme-segmentation path must survive"
    );
    let body = function_body(&source, "pub fn visible_width(");
    assert!(
        body.contains("ascii_visible_width("),
        "visible_width must try the fast path first: {body}"
    );
    assert!(
        body.contains("visible_width_slow("),
        "and fall back when it declines: {body}"
    );
}

// Verifies: gh #378 gap 2 - the seam flushes before it emits.
//
// The bug was one reversed pair. The flush must come first, in code order, so a
// reviewer can see the ordering rather than infer it.
#[test]
fn the_seam_flushes_the_buffer_before_the_code() {
    let source = text_source();
    let body = function_body(&source, "pub fn slice_with_width(");
    // The block that handles an escape arriving *inside* the window. Find it, then
    // check the order of the three statements within it - anchored on the branch, so
    // an unrelated `clear()` elsewhere in the function cannot satisfy the check.
    let branch = body
        .find("if current_col >= start_col && current_col < end_col {")
        .expect("the in-window escape branch exists");
    let block_start = branch;
    let block_end = body[block_start..]
        .find("} else if current_col < start_col {")
        .map_or(body.len(), |at| block_start + at);
    let block = &body[block_start..block_end];
    let flush = block
        .find("result.push_str(&pending_ansi);")
        .unwrap_or_else(|| panic!("the seam flushes the buffered codes: {block}"));
    let clear = block
        .find("pending_ansi.clear();")
        .unwrap_or_else(|| panic!("and clears the buffer: {block}"));
    let emit = block
        .find("result.push_str(&code);")
        .unwrap_or_else(|| panic!("and then emits the code: {block}"));
    assert!(
        flush < clear && clear < emit,
        "flush, clear, then emit - in that order: {block}"
    );
}

// Verifies: gh #378 gap 1 - the hit counter is test-only.
//
// The acceptance row needs to know the fast path was *taken*, and the honest way to
// know is a counter. It must not exist in a release build: no atomic, no shared cache
// line, no API surface.
#[test]
fn the_hit_counter_is_gated_on_test() {
    let source = text_source();
    let start = source
        .find("static FAST_PATH_HITS")
        .expect("the hit counter exists");
    // Walk back to the attribute block above it.
    let prefix = &source[start.saturating_sub(400)..start];
    assert!(
        prefix.contains("#[cfg(test)]"),
        "the counter must be `cfg(test)`-gated, not compiled into release builds"
    );
    assert!(
        prefix.contains("thread_local!"),
        "and it is a thread-local, so no cross-thread sharing"
    );
}

// ==== gh #378 gap 3: an APC marker at the seam ====
//
// `extract_segments` copies an escape into `after` only once `after` has started,
// because the tracker re-emits the consolidated style on the first `after`
// grapheme. That is right for SGR and fatal for a zero-width APC marker, which no
// tracker models and whose *position* is the whole payload: an overlay spliced
// exactly at a `CURSOR_MARKER` ate the caret/IME anchor with no error to notice.
//
// The rows below pin the shape from outside, because the failure is silent: the
// behavioural rows say the marker survives, and these say *why* it can only
// survive through the narrow door.

/// Both files the seam spans: `text.rs` holds `extract_segments`, and the
/// predicate lives beside the tracker in `text/ansi.rs` (split at the ceiling).
fn seam_source() -> String {
    format!(
        "{}\n{}",
        text_source(),
        read("crates/lca-tui/src/engine/text/ansi.rs")
    )
}

// Verifies: gh #378 gap 3 - the `after` gate is `after_started` OR an APC marker.
//
// Anchored on the gate itself, not on "the file mentions APC somewhere": a
// pass-through that is computed and never consulted is the shape this bug
// already had once.
#[test]
fn the_after_gate_lets_a_marker_through_before_the_first_grapheme() {
    let source = seam_source();
    let body = function_body(&source, "pub fn extract_segments(");
    let gate_start = body
        .find("} else if current_col >= after_start")
        .expect("the `after` escape branch exists");
    let gate_end = body[gate_start..]
        .find("after.push_str(&code);")
        .map_or(body.len(), |at| gate_start + at);
    let gate = &body[gate_start..gate_end];
    assert!(
        gate.contains("after_started"),
        "the original gate must still be there, so SGR is not duplicated: {gate}"
    );
    assert!(
        gate.contains("is_apc_marker("),
        "and a marker must be able to pass before `after` starts: {gate}"
    );
    assert!(
        gate.contains("||"),
        "the widening is a disjunction with the old gate, not a replacement: {gate}"
    );
}

// Verifies: gh #378 gap 3 - the predicate is APC and nothing wider.
//
// Two ways to get this wrong, and both are the previous bug wearing a new name:
// widening to "any escape" duplicates the SGR the tracker already re-emits
// (gap 2, reversed), and widening to "any zero-width code" replays a CSI erase or
// an OSC title at a seam, which is worse than a lost caret.
#[test]
fn the_pass_through_is_narrow() {
    let source = seam_source();
    let body = function_body(&source, "fn is_apc_marker(");
    assert!(
        body.contains("starts_with(\"\\x1b_\")"),
        "APC is `ESC _`, and that is the whole test: {body}"
    );
    assert!(
        !body.contains("contains("),
        "a substring test would match a marker mid-line, which is not the seam's question: {body}"
    );
    // The CSI and OSC introducers must not appear in the predicate.
    for other in ["b'm'", "\\x1b[", "\\x1b]"] {
        assert!(
            !body.contains(other),
            "the predicate must not widen to {other}: {body}"
        );
    }
}

// Verifies: gh #378 gap 3 - the marker is still stripped before paint, so the
// pass-through cannot put raw APC bytes on the screen.
//
// This is the row that keeps the fix from becoming a leak: the seam's job is to
// carry the marker *to* the renderer, and the renderer's job is to consume it.
#[test]
fn the_engine_still_consumes_the_marker_before_paint() {
    let core = read("crates/lca-tui/src/engine/core.rs");
    let body = function_body(&core, "pub fn extract_cursor_position(");
    assert!(
        body.contains("line.find(CURSOR_MARKER)"),
        "the engine locates the marker: {body}"
    );
    assert!(
        body.contains("replace(CURSOR_MARKER, \"\")"),
        "and removes it from the row it hands the terminal: {body}"
    );
}

// Verifies: gh #378 gap 1's property against gap 3's change - the marker is
// zero-width, so no measurement moved.
//
// A pass-through that measured one cell would re-break every row it survived on,
// which is exactly the optimisation gap 1 bought.
#[test]
fn the_width_path_still_treats_an_apc_as_zero_width() {
    let source = text_source();
    let skip = function_body(&source, "fn ansi_code_length(");
    assert!(
        skip.contains("b'_'"),
        "the skip treats APC as an escape, not as text: {skip}"
    );
    let walk = function_body(&source, "pub fn ascii_visible_width(");
    assert!(
        walk.contains("ansi_code_length(") || walk.contains("extract_ansi_code("),
        "and the fast path walks it by that length: {walk}"
    );
}

// Verifies: gh #378 gap 3 - **the product seam**: an overlay that ends exactly on
// the caret keeps the caret.
//
// The behavioural rows in `text_tests.rs` drive `extract_segments` directly with
// `before_end == after_start` (no gap). The splice the product actually builds is
// `extract_segments(base, x, x + width, ...)`, so the marker sits at a column the
// window *ends* on rather than one it starts at - and that is the shape a tooltip's
// right edge, a picker's frame, or a drawer tab produces. Driving the real painter
// is the difference between "the seam is fixed" and "the seam is fixed where the
// product calls it".
//
// Before the fix this row fails: the marker is skipped because `after` has not
// started, `extract_cursor_position` then finds no marker at all, and the frame has
// no IME anchor.
#[test]
fn an_overlay_ending_on_the_caret_keeps_the_caret() {
    use lca_tui::engine::core::extract_cursor_position;
    use lca_tui::engine::text::visible_width;
    use lca_ui::render::paint_tooltip;
    use lca_ui::theme::Theme;

    const MARKER: &str = "\x1b_pi:c\x07";
    // A composer-like row: 20 cells of text with the caret at column 20.
    let row = format!("{}hello{MARKER}", "x".repeat(15));
    let tip = "TIP-ABCD"; // eight visible cells
    let blank = "        ";
    let mut rows = vec![row];
    // The tooltip's right edge lands on column 20, exactly where the caret is.
    paint_tooltip(&mut rows, 12, 0, &[tip.to_string()], &Theme::plain());

    let painted = &rows[0];
    assert!(
        painted.contains(MARKER),
        "the marker survives a splice that ends on it: {painted:?}"
    );
    assert_eq!(
        visible_width(painted),
        20,
        "and the row keeps its width - the marker is zero cells: {painted:?}"
    );
    assert_eq!(blank.len(), tip.len(), "the fixture's two edges agree");
    let (_, position) = extract_cursor_position(&rows);
    let (row_index, column, _) = position.expect("the frame still has an IME anchor");
    assert_eq!(row_index, 0);
    assert_eq!(
        column, 20,
        "the anchor sits at the seam its marker named: {painted:?}"
    );
}
