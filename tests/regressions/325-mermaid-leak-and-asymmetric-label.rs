//! Gh #325: the Mermaid renderer must not leak, must not re-parse a repaint, and
//! every node shape must draw its label.
//!
//! The behavioural rows live beside the code
//! (`crates/lca-tui/src/widgets/mermaid/tests.rs`) and in
//! `crates/lca-tui/tests/mermaid_heap.rs`, which carries its own
//! `#[global_allocator]` so the counter cannot perturb another binary's tests.
//! This file is the NFR-24 guard, and it covers the things a behaviour row cannot
//! see because it measures the *current* code:
//!
//! 1. **no `leak` comes back.** The measured rows prove this build does not retain
//!    per parse; this one proves the mechanism that caused it is not merely
//!    dormant. `#325`'s own fix must not trade one for another, so the ban is on
//!    the call, not on a variable name.
//! 2. **the cache is bounded and the parse is width-independent.** A bound is the
//!    decision the brief asked for, and an unbounded cache is the leak with a
//!    longer lifetime.
//! 3. **every shape places its label through one helper.** The defect was one
//!    shape being written differently from the rest; a shape with its own interior
//!    loop is how the next one gets it wrong too.

use std::path::PathBuf;

/// The renderer's source.
fn source() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/lca-tui/src/widgets/mermaid.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The body of `fn <name>` in `src`, from its `{` to the matching `}`.
///
/// `name` must end where the declaration does (a `(` or `<` next), so `parse`
/// cannot match `parse_node` and the guard cannot silently inspect the wrong
/// function.
fn function_body(src: &str, name: &str) -> String {
    let needle = format!("fn {name}");
    let start = src
        .match_indices(&needle)
        .find(|(at, _)| matches!(src[at + needle.len()..].chars().next(), Some('(' | '<')))
        .map(|(at, _)| at)
        .unwrap_or_else(|| panic!("`{name}` is declared"));
    let open = start
        + src[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{name}` has a body"));
    let mut depth = 0usize;
    let mut end = src.len();
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > open, "`{name}`'s body is closed");
    src[open..=end].to_string()
}

// Verifies: gh #325 - `String::leak` does not come back.
//
// The measured rows in `tests/mermaid_heap.rs` are the real proof, but they test
// the *observable* consequence: retention. This one bans the mechanism, because
// "no heap growth under the paths we happened to exercise" is a weaker claim than
// "nothing in this file leaks", and a future parse path that is not covered by a
// row would slip past the first. Counting the call sites keeps the claim honest:
// the issue names exactly one, and it is zero now.
#[test]
fn the_renderer_does_not_leak_strings() {
    let src = source();
    // Comments may name the defect (this file's own docs do, and so does the
    // `split_edge` doc comment explaining why the leak was pointless). Only code
    // counts.
    let code: String = src
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [".leak()", "Box::leak", "String::leak"] {
        assert!(
            !code.contains(banned),
            "gh #325: `{banned}` must not appear in the renderer - a leaked \
             allocation is never reclaimed, and a diagram re-renders on every \
             resize and repaint"
        );
    }
}

// Verifies: gh #325 - the cache exists, is consulted, and is **bounded**.
//
// All three parts matter: a cache that is never read is dead weight, and one with
// no bound is the same bug with a longer lifetime. The bound is a named constant
// so the row can check the cache is built from the same number the docs quote.
//
// **Corrected by gh #579.** This row used to require `cache.remove(` inside `render`,
// which was true while `render` hand-evicted. The eviction now lives in the shared
// `LruCache` (one policy for the crate, #323/#579), so `render` correctly contains no
// eviction at all - keeping the old assertion would have forced the second policy back
// in. The bound is now asserted at the *construction* site, which is where it is applied.
#[test]
fn the_parse_cache_is_bounded_and_consulted() {
    let src = source();
    assert!(
        src.contains("const PARSE_CACHE_SIZE: usize =")
            || src.contains("pub const PARSE_CACHE_SIZE: usize ="),
        "the cache bound is a named constant, not a literal at the eviction site"
    );
    assert!(
        src.contains("LruCache::new(PARSE_CACHE_SIZE)"),
        "and the cache is built from it, so the bound is applied by the shared policy"
    );
    let render = function_body(&src, "render");
    assert!(
        render.contains("PARSE_CACHE"),
        "`render` consults the cache:\n{render}"
    );
    // Eviction is the policy's job now, so the assertion is that `render` does **not**
    // pick a victim - the opposite of what this row used to require.
    assert!(
        !render.contains("cache.remove(") && !render.contains("keys().next()"),
        "and leaves eviction to `LruCache` rather than picking its own victim:\n{render}"
    );
}

// Verifies: gh #325 - the cached parse does not depend on width.
//
// This is what makes one entry serve every resize: `render` takes only the source,
// so the `Art` it returns (spans plus a width) is reusable at any pane width. If a
// future change threaded a width into the parse, the cache key would silently
// become wrong and a resize would serve a stale layout.
#[test]
fn the_cached_parse_is_width_independent() {
    let src = source();
    assert!(
        src.contains("pub fn render(source: &str) -> Option<Art>"),
        "`render` takes only the source, so its result is width-independent"
    );
    let parse = function_body(&src, "parse");
    assert!(
        !parse.contains("width: usize") && !parse.contains("inner_width"),
        "the parse does not take a width:\n{parse}"
    );
}

// Verifies: gh #325 - **one** label placement serves every shape.
//
// The defect was `Shape::Asymmetric` hand-building its middle row, so the label
// vanished. The structural fix is that no shape builds its own interior: each calls
// `label_row`, and the guard asserts that per shape rather than trusting a
// behavioural row that would pass for four of the five.
#[test]
fn every_shape_places_its_label_through_one_helper() {
    let src = source();
    let make_box = function_body(&src, "make_box");
    assert!(
        src.contains("fn label_row("),
        "there is one label-placement helper"
    );
    // Every shape arm in `make_box` routes its middle row through the helper.
    for shape in ["Rect", "Round", "Rhombus", "DoubleRound", "Asymmetric"] {
        let mentions = make_box.matches(&format!("Shape::{shape}")).count();
        assert!(
            mentions > 0,
            "`{shape}` is handled by `make_box`: {mentions}"
        );
    }
    // Four call sites for five shapes: `Rect` and `Round` share one arm, because
    // they differ only in their corner glyphs. The count is per *arm*, not per
    // variant - asserting five would have been sloppier than the code.
    let calls = make_box.matches("label_row(").count();
    assert!(
        calls >= 4,
        "every shape arm calls the one helper (found {calls}): the bug was a shape \
         that built its own interior"
    );
    // And no shape arm builds a bare all-spaces interior any more.
    assert!(
        !make_box.contains("(' ', Class::None)\n                        })"),
        "no shape hand-builds a spaces-only middle row"
    );
}

// Verifies: gh #325 - the asymmetric opener is distinguished from an arrow head.
//
// `>` does two jobs in this syntax, and telling them apart is what lets an
// asymmetric node sit on the *left* of an edge. Without the distinction the `]`
// closes a node that was never opened and the arrow is scanned as node text.
#[test]
fn the_asymmetric_opener_is_told_apart_from_an_arrow() {
    let src = source();
    assert!(
        src.contains("fn is_asymmetric_open("),
        "the `>` disambiguation is its own predicate"
    );
    let split = function_body(&src, "split_edge");
    assert!(
        split.contains("is_asymmetric_open"),
        "and the edge scanner uses it:\n{split}"
    );
    assert!(
        split.contains("max(0)"),
        "while the depth decrement cannot go negative, so an unmatched `]` cannot \
         make the rest of the statement look like node text:\n{split}"
    );
}
