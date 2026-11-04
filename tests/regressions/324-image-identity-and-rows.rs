//! Gh #324 gap 1: every inline image used id 1 and claimed one row, so images
//! corrupted each other and the row after each one painted over it.
//!
//! Two defects in one function, and the second is why the first was not simply an
//! id bug: `render_image` hardcoded `i=1` (so a second image overwrote the first's
//! texture) and returned **one** line for an image `rows` tall (so the transcript
//! entry that followed was painted straight over the image's own rows). Fixing
//! only the id would have left every multi-row image sliced by its successor.
//!
//! The behavioural rows live beside the code they pin:
//!
//! * `crates/lca-tui/src/widgets/image.rs` - id derivation (unique per payload,
//!   stable across renders, different at a new scale), the kitty and iTerm2 block
//!   shapes, and the placement form.
//! * `crates/lca-ui/src/placements.rs` - the first paint transmits, a repaint
//!   places, two images are tracked separately, a rescaled image re-uploads, and
//!   the set is bounded.
//! * `crates/lca-ui/src/transcript/tests.rs` - two transcript images with distinct
//!   ids and both placements present, the reservation count, the clamp, and the
//!   placeholder arm byte-identical.
//! * `crates/lca-tui/src/engine/main_screen.rs` - the paint loop treats a reserve
//!   as one block, and the clear pass runs before the sequence is placed.
//!
//! This file is the NFR-24 named guard: the rows above could all pass while the
//! *shape* that produced the defect came back - a literal `i=1` at a call site, a
//! single-row return, an image id allocated from a counter rather than derived, or
//! a renderer that walks into a reserve.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The body of one function, from its `fn` line to the first closing brace at the
/// start of a line.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #324 gap 1 - **no call site hardcodes an image id.** The filed bug
// was `encode_kitty(bytes, cols, rows, 1)`. A literal at a call site is the exact
// shape that produced it, so this pins that every `encode_kitty` call inside
// `render_image` passes a derived value, and that the derivation is the only place
// an id is invented.
#[test]
fn no_call_site_hardcodes_a_kitty_image_id() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let render = function_body(&image, "pub fn render_image(");
    for call in render.split("encode_kitty(").skip(1) {
        let args = &call[..call.find(')').unwrap_or(call.len())];
        assert!(
            !args
                .split(',')
                .map(str::trim)
                .any(|arg| arg.parse::<u32>().is_ok()),
            "the image id is derived, never a literal: encode_kitty({args})"
        );
    }
    assert!(
        render.contains("kitty_image_id("),
        "the kitty arm derives its id: {render}"
    );
    // And the derivation is content-addressed, not allocated: a counter and a
    // random source are both rejected, because the first re-uploads on every
    // frame and the second cannot be tested at all.
    let derive = function_body(&image, "pub fn kitty_image_id(");
    assert!(
        derive.contains("0x0000_0100_0000_01b3"),
        "the id is a hash of its inputs (FNV-1a): {derive}"
    );
    assert!(
        !derive.contains("AtomicU32") && !derive.contains("random"),
        "not a counter and not random: {derive}"
    );
    // Both inputs are in the hash: the payload and the scale.
    assert!(
        derive.contains("columns") && derive.contains("rows") && derive.contains("bytes"),
        "the id covers the payload and the cell scale: {derive}"
    );
}

// Verifies: gh #324 gap 1 - **the kitty render returns the reservation, not one
// line.** The second half of the filed bug: a single-element vector for a
// multi-row image.
#[test]
fn the_kitty_arm_returns_one_line_per_reserved_row() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let render = function_body(&image, "pub fn render_image(");
    assert!(
        render.contains("lines.resize(rows as usize"),
        "the kitty arm pads its vector to `rows`: {render}"
    );
    // The iTerm2 arm is the mirror image and must reserve first.
    assert!(
        render.contains("rows.saturating_sub(1)"),
        "the iTerm2 arm reserves its rows before the sequence: {render}"
    );
    assert!(
        render.contains("ImageProtocol::Iterm2 =>"),
        "the iTerm2 arm still exists (gh #326's encoder guard covers it)"
    );
    // The placeholder arm is the only text one, and it is untouched.
    assert!(
        render.contains("ImageProtocol::None => render_image_placeholder"),
        "the fallback arm is the placeholder: {render}"
    );
}

// Verifies: gh #324 gap 1 - **the renderer never walks into an image's reserved
// rows as text.** A future edit that drops the block walk would leave the next
// entry painting over the image, and every behavioural row above would still pass
// because they assert the *rendered lines*, not how they were painted.
#[test]
fn both_renderers_treat_an_image_reserve_as_one_block() {
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    assert!(
        main.contains("kitty_reserved_rows("),
        "the main-screen paint loop asks how many rows an image claims"
    );
    assert!(
        main.contains("i += ahead"),
        "and advances past the whole reserve, so a blank row inside one is \
         unreachable from the text path"
    );
    let alt = read("crates/lca-tui/src/engine/alt_screen.rs");
    assert!(
        alt.contains("expand_for_image_blocks(&lines, &touched)"),
        "the alt-screen render expands a touched row to its whole block"
    );
    let alt_module = read("crates/lca-tui/src/engine/alt_screen/image_blocks.rs");
    assert!(
        alt_module.contains("fn expand_for_image_blocks("),
        "and the expansion itself lives in its own module"
    );
}

// Verifies: gh #324 gap 1 - **a repaint places.** The performance half of the
// design call: the transcript must consult a cache before sending bytes, or a
// scroll over an image re-uploads it. A naive "unique id every frame" fix passes
// the collision row and fails this one.
#[test]
fn the_transcript_holds_a_placement_cache() {
    let transcript = read("crates/lca-ui/src/transcript/mod.rs");
    assert!(
        transcript.contains("PlacementCache"),
        "the transcript owns the cache"
    );
    assert!(
        transcript.contains(".substitute(&mut out)"),
        "a kitty frame goes through it before being returned"
    );
    let placements = read("crates/lca-ui/src/placements.rs");
    assert!(
        placements.contains("kitty_placement("),
        "a known image is rewritten into the placement form"
    );
    assert!(placements.contains("MAX_TRACKED"), "the set is bounded");
    // The bound is a constant, not an unbounded map: a session must not grow ids
    // forever.
    assert!(
        !placements.contains("HashMap::new()"),
        "the cache is the shared LRU, not a hand-rolled map"
    );
    assert!(
        placements.contains("LruCache"),
        "gh #323's one cache policy is reused"
    );
}

// Verifies: gh #324 gap 1 - the **cell movements are in the width vocabulary.**
// The multi-row iTerm2 shape opens with `CSI {n} A`; the measurement layer used to
// count that as three columns, which would have made the very row this fix
// introduces unmeasurable.
#[test]
fn the_width_vocabulary_covers_the_cursor_movements_a_reserve_needs() {
    let text = read("crates/lca-tui/src/engine/text.rs");
    let length = function_body(&text, "fn ansi_code_length(");
    for final_byte in ["b'A'", "b'B'", "b'C'", "b'D'"] {
        assert!(
            length.contains(final_byte),
            "ansi_code_length accepts {final_byte}: {length}"
        );
    }
    let extract = function_body(&text, "pub fn extract_ansi_code(");
    for final_byte in ["b'A'", "b'B'", "b'C'", "b'D'"] {
        assert!(
            extract.contains(final_byte),
            "the two vocabularies agree on {final_byte} (a mismatched pair \
             measures and strips differently): {extract}"
        );
    }
}

// ===========================================================================
// Gap 2 - the cell-size probe: sent, recognised, consumed, and it re-renders
// ===========================================================================

// Verifies: gh #324 gap 2 - **the query exists.** The gap's premise was a parser
// with no producer: `CellSize` had been parsed since `core.rs` grew it and nothing
// ever sent `CSI 16 t`, so no terminal could answer. A consumer wired to a reply
// nobody asks for is dead code, which is the defect class this guard exists for.
#[test]
fn the_cell_size_query_is_actually_sent() {
    let colors = read("crates/lca-tui/src/engine/colors.rs");
    assert!(
        colors.contains("\"\\x1b[16t\""),
        "the query constant is the real sequence"
    );
    assert!(
        colors.contains("fn startup_query()")
            && colors.contains("{TERMINAL_CELL_QUERY}{TERMINAL_COLOR_QUERY}"),
        "and the startup query puts the cell query first, the sentinel last"
    );
    let run = read("crates/lca-ui/src/run.rs");
    assert!(
        run.contains("colors::startup_query()"),
        "the loop writes it"
    );
    assert!(
        !run.contains("\"\\x1b[16t\""),
        "and not as an inline literal, so the ordering cannot drift at the call site"
    );
}

// Verifies: gh #324 gap 2 - **the reply is classified as a reply.** Sending a query
// whose answer falls through to the composer is worse than not sending it: the
// user's next message would contain the escape bytes.
#[test]
fn the_cell_size_reply_is_recognised_and_consumed() {
    let colors = read("crates/lca-tui/src/engine/colors.rs");
    assert!(
        colors.contains("pub fn is_cell_size_report("),
        "there is a recogniser for the reply shape"
    );
    let startup = function_body(&colors, "pub fn is_startup_query_reply(");
    assert!(
        startup.contains("is_cell_size_report(data)"),
        "and the one recogniser every consumer shares includes it: {startup}"
    );
    let theme_query = read("crates/lca-ui/src/theme_query.rs");
    assert!(
        theme_query.contains("apply_cell_size_report(chat, data)"),
        "the dispatcher applies it"
    );
}

// Verifies: gh #324 gap 2 - **the estimate is named, and the probed value replaces
// it.** A hardcoded 9x18 that nobody calls an estimate is how the gap survived; the
// constant has to be public, `DEFAULT_`-named, documented as an estimate, and
// overridable.
#[test]
fn the_estimate_is_named_and_the_probe_replaces_it() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    assert!(
        image.contains("pub const DEFAULT_CELL_PIXELS"),
        "the fallback is a named constant"
    );
    assert!(
        image.contains("It is an estimate, and it is named as one"),
        "and the doc says what it is, not what it is not"
    );
    assert!(
        image.contains("pub fn current_cell_pixels()"),
        "the renderer reads the live value"
    );
    assert!(
        image.contains("pub fn probe_cell_pixels("),
        "and a probe installs it"
    );
    // The render path must go through the live value, not the constant.
    let cell = function_body(&image, "fn image_cell_size(info: &ImageInfo");
    assert!(
        cell.contains("current_cell_pixels()"),
        "image_cell_size asks for the live scale: {cell}"
    );
    assert!(
        !cell.contains("DEFAULT_CELL_PIXELS"),
        "and does not reach for the estimate behind the probe's back: {cell}"
    );
}

// Verifies: gh #324 gap 2 - **the grid is chosen by least distortion**, and the
// metric is pi's: `max(count/ideal, ideal/count)`, comparing `ceil` against
// `ceil - 1`. A `ceil`-only implementation would pass every other row in this file.
#[test]
fn the_grid_choice_is_a_distortion_comparison() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let helper = function_body(&image, "fn choose_less_distorted_cell_count(");
    assert!(
        helper.contains("/ ideal).max(ideal /"),
        "the metric is symmetric (a ratio and its reciprocal weigh the same): {helper}"
    );
    assert!(
        helper.contains("(upper - 1)"),
        "it compares ceil against ceil - 1: {helper}"
    );
    let cell = function_body(&image, "fn image_cell_size_with(");
    assert!(
        cell.contains("choose_less_distorted_cell_count("),
        "and the cell-size calculation calls it on the binding axis: {cell}"
    );
}

// Verifies: gh #324 gap 2 - **a probe that changes the scale invalidates the cached
// frames.** The scale is part of a kitty image's identity, so storing the new value
// without dropping the old frames leaves the terminal holding a texture stretched to
// the previous geometry. This is the "silent re-scale" the brief names.
#[test]
fn a_probe_invalidates_the_frames_laid_out_against_the_estimate() {
    // The apply path lives in `theme_query.rs` - the module that owns the reply
    // lifecycle. It moved there because `chat.rs` is at gate 11's ceiling.
    let theme_query = read("crates/lca-ui/src/theme_query.rs");
    let apply = function_body(&theme_query, "fn apply_cell_size_report(");
    assert!(
        apply.contains("probe_cell_pixels("),
        "the reply installs the scale: {apply}"
    );
    assert!(
        apply.contains("transcript.invalidate()"),
        "and drops the estimate's frames: {apply}"
    );
    // The lifecycle guard is the caller's: `handle_reply` refuses every reply kind
    // outside the window, so a child process's `CSI 6;...t` cannot reflow.
    let handle = function_body(&theme_query, "pub fn handle_reply(");
    assert!(
        handle.contains("if !is_startup_query_reply(data)"),
        "a non-reply never reaches the apply path: {handle}"
    );
}
