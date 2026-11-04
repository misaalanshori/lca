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
        main.contains("kitty_block_rows("),
        "the main-screen paint loop asks how many rows an image claims - and gap 4          narrowed that to the rows the reserve can actually own (it stops at text)"
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

// ===========================================================================
// Gap 3 - a scrolled-past image is cropped to the band the window shows
// ===========================================================================

// Verifies: gh #324 gap 3 - **the crop is a placement, not a re-encode.** The row that
// matters most in this gap: the id is unchanged (gap 1 hashes payload and cell scale,
// deliberately not the crop) and no base64 comes back, so scrolling past an image costs
// bytes rather than a re-upload. A fix that cropped by re-encoding would pass the band
// rows and fail this one.
#[test]
fn the_crop_rewrites_the_placement_and_not_the_identity() {
    let image = read("crates/lca-tui/src/widgets/image/crop.rs");
    let crop = function_body(&image, "pub fn crop_kitty_line_px(");
    assert!(
        crop.contains("\"\\x1b_Ga=p,q=2,"),
        "the crop emits a placement (`a=p`), not a transmission: {crop}"
    );
    assert!(
        crop.contains("i={id}") && !crop.contains("encode_kitty"),
        "with the same id and no encoder call: {crop}"
    );
    assert!(
        !crop.contains("base64"),
        "and no payload path at all: {crop}"
    );
    // The id must not be derived here: it comes from the line it rewrites.
    assert!(
        !crop.contains("kitty_image_id"),
        "the id is read, never recomputed - a recompute is how a crop would change it: {crop}"
    );
}

// Verifies: gh #324 gap 3 - **both edges are computed from different variables.** The
// brief names this specifically: top-clipped uses `sourceY`, bottom-clipped uses
// `sourceEnd`, and an implementation that derived one from the other would still pass
// single-edge rows.
#[test]
fn the_two_edges_use_separately_named_extents() {
    let image = read("crates/lca-tui/src/widgets/image/crop.rs");
    let crop = function_body(&image, "pub fn crop_kitty_line_px(");
    assert!(crop.contains("source_y"), "the top edge: {crop}");
    assert!(crop.contains("source_end"), "the bottom edge: {crop}");
    assert!(
        crop.contains("div_ceil"),
        "the far edge rounds up, so consecutive bands tile instead of gapping: {crop}"
    );
    assert!(
        crop.contains("u64::from(px_h) * u64::from(hidden_rows)) / u64::from(rows)"),
        "the near edge floors: {crop}"
    );
}

// Verifies: gh #324 gap 3 - **the window tells the crop what it can see**, and the
// arithmetic lives in one function both edges share. Without this the renderer sees a
// fragment and cannot recover the band.
#[test]
fn the_window_reports_the_band_it_shows() {
    let image = read("crates/lca-tui/src/widgets/image/crop.rs");
    let band = function_body(&image, "pub fn clip_band(");
    assert!(
        band.contains("window_start") && band.contains("window_end"),
        "the band is derived from both window edges: {band}"
    );
    assert!(
        band.contains("saturating_sub(block_start)"),
        "hidden rows are the distance from the window's start: {band}"
    );
    assert!(
        band.contains("return None"),
        "a block outside the window reports nothing, so no zero-row placement is emitted: {band}"
    );
}

// Verifies: gh #324 gap 3 - **the crop runs before the placement cache.** Gap 1's cache
// rewrites a header into a no-payload `a=p` form, and a cropped header *is* a placement;
// cropping afterwards would have nothing to parse. Measured: getting this order wrong
// made every image vanish from the frame for every scroll position, which is why the
// render exposes a stage instead of a boolean.
#[test]
fn the_crop_runs_before_the_placement_cache() {
    let render = read("crates/lca-ui/src/chat_render.rs");
    assert!(
        render.contains("RowStage::BeforePlacements"),
        "the sections render stops before the cache so the header is still an a=T block"
    );
    assert!(
        render.contains("crop_image_blocks(&transcript"),
        "the crop is applied to that stage"
    );
    let after_crop = render
        .find("crop_image_blocks(&transcript")
        .expect("the crop call");
    let placements = render
        .find("apply_placements(&mut transcript)")
        .expect("the cache call");
    assert!(
        after_crop < placements,
        "and the cache runs afterwards, on whatever the crop left"
    );
}

// Verifies: gh #324 gap 3 - **a top-clipped block's placement moves onto its first
// visible row.** The sequence lives on the header, so if the header is above the window
// the slice cuts it away and the band shows nothing - the image vanishes instead of
// being cropped. This is the bug that measurement found, and pi solves it the same way
// (`paintBox` places the cropped sequence on the scroll view's first row).
#[test]
fn a_top_clipped_placement_moves_onto_the_first_visible_row() {
    let render = read("crates/lca-ui/src/chat_render.rs");
    let crop = function_body(&render, "fn crop_image_blocks(");
    assert!(
        crop.contains("row < start"),
        "the header above the window is recognised: {crop}"
    );
    assert!(
        crop.contains("start.min(transcript.len()"),
        "and the placement is written on the window's first row instead: {crop}"
    );
}

// Verifies: gh #324 gap 3 - **the halfblock/placeholder path keeps its own clipping.**
// It is text rows; routing it through pixel arithmetic would be a category error, and
// most terminals get exactly this path.
#[test]
fn the_placeholder_path_stays_text() {
    let crop_src = read("crates/lca-tui/src/widgets/image/crop.rs");
    let crop = function_body(&crop_src, "pub fn crop_kitty_line_px(");
    assert!(
        crop.contains("kitty_image_control(line)?"),
        "the crop is a kitty-only operation: {crop}"
    );
    // The placeholder lives in `image.rs`; the crop in its own module (gate 11 split).
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let placeholder = function_body(&image, "pub fn render_image_placeholder(");
    assert!(
        !placeholder.contains("source_y") && !placeholder.contains("\x1b_G"),
        "the placeholder has no pixel geometry and no graphics sequence: {placeholder}"
    );
    // And `render_image`'s None arm is still the placeholder.
    let render_fn = function_body(&image, "pub fn render_image(");
    assert!(
        render_fn.contains("ImageProtocol::None => render_image_placeholder"),
        "the None arm is untouched: {render_fn}"
    );
}

// ===========================================================================
// Gap 4 - the changed range is image-aware, and orphaned textures are deleted
// ===========================================================================

// Verifies: gh #324 gap 4 - **the range expansion runs in `render`, upstream of every
// arm that consumes the range.** A test-only accessor that calls the expansion directly
// passes with the wiring removed - I measured that with my first version of the rows,
// which stayed green against a broken renderer. So the guard pins the *call site*, not
// the helper.
#[test]
fn the_changed_range_is_expanded_inside_render() {
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    let render = function_body(&main, "pub fn render(");
    assert!(
        render.contains("expand_range_for_images(&self.previous_lines, &new_lines"),
        "the range is expanded, from both frames, inside render: {render}"
    );
    // And there is no test-only accessor that could be mistaken for the real path.
    assert!(
        !main.contains("fn expanded_changed_range("),
        "no accessor that bypasses the wiring it would be testing"
    );
}

// Verifies: gh #324 gap 4 - **a block is bounded by the rows it can own.** `r=` is the
// claimed height, but the reserve is a run of *blank* rows: the document may end, or
// another entry's text may start, before the claim is reached. Treating the claim as
// authoritative repaints a neighbour's text as part of the image - which is how the block
// walk's own assertion fired the first time I wrote the full-viewport row.
#[test]
fn a_block_stops_at_the_first_row_that_holds_text() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let block = function_body(&image, "pub fn kitty_block_rows(");
    assert!(
        block.contains("!next.is_empty()"),
        "a non-blank row ends the reserve: {block}"
    );
    assert!(
        block.contains("None => break"),
        "and so does the end of the document: {block}"
    );
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    assert!(
        main.contains("kitty_block_rows(&new_lines, i)"),
        "the paint loop uses the bounded answer"
    );
}

// Verifies: gh #324 gap 4 - **deleting a texture is an id diff, not a range scan.** The
// distinction the brief calls out: a *replaced* image needs `a=d,d=I,i=<old>`; a *moved*
// one (gap 3's crop, same id, new placement) must not be deleted. A range-based delete
// would undo gap 3, so the guard pins the diff.
#[test]
fn the_delete_is_an_id_diff_not_a_range_scan() {
    // The gap-4 helpers live in their own module (gate 11 split).
    let ranges = read("crates/lca-tui/src/engine/main_screen/image_ranges.rs");
    let orphan = function_body(&ranges, "fn orphaned_image_ids(");
    assert!(
        orphan.contains("previous") && orphan.contains("next"),
        "both frames are consulted, so an id that survives is not deleted: {orphan}"
    );
    assert!(
        orphan.contains("!live.contains(id)"),
        "an id is orphaned only when nothing in the new frame names it: {orphan}"
    );
    assert!(
        !orphan.contains("first") && !orphan.contains("last"),
        "and no range is involved, which is what keeps a crop alive: {orphan}"
    );
    // The delete command itself is kitty's `d=I`, which frees the data.
    let delete = function_body(&ranges, "fn delete_kitty_image(");
    assert!(
        delete.contains("a=d,d=I,i={id},q=2"),
        "the data is freed, not just the placement: {delete}"
    );
}

// Verifies: gh #324 gap 4 - **the delete rides the same synchronized block as the
// repaint.** Emitting it before the frame would make the old image vanish for a frame
// before the new drawing arrived - a flicker where the fix was meant to remove one.
#[test]
fn the_delete_is_emitted_inside_the_frame() {
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    let render = function_body(&main, "pub fn render(");
    let open = render
        .find("out.push_str(\"\\x1b[?2026h\")")
        .expect("the frame opens");
    let delete = render
        .find("delete_kitty_image(*id)")
        .expect("the deletes are emitted");
    assert!(
        delete > open,
        "the deletes come after the frame opens: {render}"
    );
}

// ===========================================================================
// Gap 5 - the fallback is a link when it can be, a label when it cannot
// ===========================================================================

// Verifies: gh #324 gap 5 - **one formatter, two wrappings.** The linked and unlinked
// forms must differ by the OSC 8 wrapper and nothing else; a second sentence for the
// unlinked case would be a second code path, and the two would drift until a user on
// `screen` read a different label from a user on kitty.
#[test]
fn the_fallback_has_one_text_builder_and_two_wrappings() {
    // The fallback lives in its own module (gate 11 split); the `None` arm stays in
    // `image.rs` and delegates to it.
    let image = read("crates/lca-tui/src/widgets/image/fallback.rs");
    let text = function_body(&image, "pub fn fallback_text(");
    assert!(
        !text.contains("\\x1b]8"),
        "the text builder never emits a link itself: {text}"
    );
    let render = function_body(&image, "pub fn render_fallback_with(");
    assert!(
        render.contains("osc8_link(&text, &url)") && render.contains("None => fallback_text(info)"),
        "the wrapping happens in one place, over one text: {render}"
    );
    // And the link wrapper is the markdown renderer's shape, not a second dialect.
    let link = function_body(&image, "fn osc8_link(");
    assert!(
        link.contains("\\x1b]8;;{url}\\x1b\\\\"),
        "ST-terminated, like `LinkMode::Hyperlink`: {link}"
    );
}

// Verifies: gh #324 gap 5 - **the hyperlink decision reuses the existing ladder.** Adding
// a second detection is how two answers to "can this terminal link" appear; the brief is
// explicit that the ladder in `terminal.rs` is the one.
#[test]
fn the_link_decision_uses_the_one_hyperlink_ladder() {
    let fallback = read("crates/lca-tui/src/widgets/image/fallback.rs");
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let placeholder = function_body(&image, "pub fn render_image_placeholder(");
    assert!(
        placeholder.contains("crate::engine::terminal::supports_hyperlinks()"),
        "the product path asks the existing ladder: {placeholder}"
    );
    // No second **hyperlink** detection. (`env("TERM")` does appear in this file, but it
    // belongs to `detect_image_protocol_with` - the *graphics protocol* ladder, which is a
    // different question from "can this terminal link". Asserting on `TERM` alone would
    // fail on a legitimate, unrelated reader.)
    assert!(
        !fallback.contains("LCA_HYPERLINKS") && !image.contains("LCA_HYPERLINKS"),
        "the fallback does not re-detect hyperlinks on its own"
    );
    for body in [
        function_body(&fallback, "pub fn render_fallback_with("),
        function_body(&fallback, "fn fallback_link("),
    ] {
        assert!(
            !body.contains("supports_hyperlinks()"),
            "the decision is passed in, so a row can drive both answers: {body}"
        );
    }
}

// Verifies: gh #324 gap 5 - **a link is a claim the file exists.** The existence check is
// what keeps the fallback honest after `lca session gc` prunes an attachment; without it
// the label offers a link to something gone, which the issue's design notes call out as
// worse than the label it replaced.
#[test]
fn a_link_is_only_offered_for_a_file_that_exists() {
    let image = read("crates/lca-tui/src/widgets/image/fallback.rs");
    let link = function_body(&image, "fn fallback_link(");
    assert!(
        link.contains("if !file_exists(path)"),
        "the existence check is load-bearing, not decorative: {link}"
    );
    assert!(
        link.contains("if !hyperlinks"),
        "and so is the capability: {link}"
    );
    assert!(
        link.contains("path.as_deref()?"),
        "and a path, which a clipboard image does not have: {link}"
    );
}

// Verifies: gh #324 gap 5 - **the `file://` URL is a real URL.** `/` stays literal and
// everything outside the unreserved set is percent-encoded with uppercase hex, which is
// what `pathToFileURL` produces; a raw space or a raw non-ASCII byte makes the link
// unopenable.
#[test]
fn the_file_url_is_a_real_url() {
    let image = read("crates/lca-tui/src/widgets/image/fallback.rs");
    let url = function_body(&image, "pub fn file_url(");
    assert!(
        url.contains("b'/' | b'-' | b'_' | b'.' | b'~'"),
        "the separators and unreserved marks stay literal: {url}"
    );
    assert!(
        url.contains("%{byte:02X}"),
        "and everything else is uppercase-hex percent-encoded: {url}"
    );
    assert!(
        url.contains("if !path.is_absolute()"),
        "a relative path has no file:// location: {url}"
    );
    assert!(url.contains("file://"), "and the scheme is present");
}

// Verifies: #326 + gh #324 gap 5 - **the grammar a user reads is unchanged for an image
// with no file.** The `None` protocol is what most terminals get, so the text must not
// shift under them: this cycle adds a link and a path, never a reworded sentence.
#[test]
fn the_path_less_fallback_keeps_the_326_grammar() {
    let image = read("crates/lca-tui/src/widgets/image.rs");
    let placeholder = function_body(&image, "pub fn render_image_placeholder(");
    assert!(
        placeholder.contains("if info.path.is_some()"),
        "the richer form is only for an image that has a file: {placeholder}"
    );
    assert!(
        placeholder.contains("[image {} · {} · {}]"),
        "and the path-less grammar is the #326 one, verbatim: {placeholder}"
    );
}
