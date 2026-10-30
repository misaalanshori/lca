//! Gh #328: an expanded diff card printed the tool's `result` twice.
//!
//! The defect was two independent `if let`s in a row in `transcript/render.rs`: a diff
//! arm (`if let Some(diff)`) followed by the general output block
//! (`if let Some(result)`) with no `else`. A result carrying a diff was therefore rendered
//! by both — an `edit` printed its one-line summary twice, and a tool whose result *is*
//! the diff printed every line twice.
//!
//! The behavioural rows live in `crates/lca-ui/src/transcript/render_diff_tests.rs`. This
//! file is the NFR-24 guard, and it pins the two structural properties a green suite
//! could otherwise lose:
//!
//! 1. **the choice is one predicate** — a `Body` classification rendered by one `match`,
//!    not two blocks that happen to be adjacent;
//! 2. **the general output block is not also called on the diff path** — the absence, in
//!    the diff arm, of a second `render_tool_output`, which is exactly the shape the bug
//!    comes back in when someone "just adds the trailing note" naively.

use std::path::PathBuf;

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    &source[start..]
}

/// A source file from the workspace root.
fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #328 design point 1 - **one predicate decides diff-versus-output.**
//
// The predicate is `Body::classify`, it returns a typed choice, and `render_tool` renders
// it with one `match`. Two adjacent `if let`s is the shape that produced the bug: nothing
// in the code said the paths were alternatives.
#[test]
fn the_body_is_one_typed_choice_rendered_by_one_match() {
    let text = source("crates/lca-ui/src/transcript/render.rs");
    let render = function_body(&text, "fn render_tool(");

    let classify = render
        .find("Body::classify(")
        .unwrap_or_else(|| panic!("`render_tool` classifies its body:\n{render}"));
    // Relative to `classify`, not absolute: comparing an absolute index against a relative
    // one is how this row first failed against correct code.
    let matched = render[classify..]
        .find("match &body {")
        .unwrap_or_else(|| panic!("and renders the choice with one match:\n{render}"));
    assert!(
        matched > 0,
        "the classification comes before the match that obeys it:\n{render}"
    );

    // The two arms exist and are named for what they are.
    for arm in ["Body::Diff {", "Body::Output =>"] {
        assert!(render.contains(arm), "the {arm:?} arm exists:\n{render}");
    }

    // And the old shape is gone: no second standalone `if let Some(result)` block.
    assert!(
        !render.contains("if let Some(result) = result {"),
        "the general output block is not a second independent `if let` in `render_tool`:\n{render}"
    );
}

// Verifies: gh #328 - the general output block is called from the **output arm only**.
//
// This is the property the bug's return would break. The tempting wrong fix is to keep
// the two blocks and make the second one skip what the first drew - which is a condition
// on the second block, exactly what the brief rules out. So the guard asserts the *count*
// inside the diff arm: zero calls.
#[test]
fn the_diff_arm_never_runs_the_general_output_block() {
    let text = source("crates/lca-ui/src/transcript/render.rs");
    let render = function_body(&text, "fn render_tool(");
    let diff_start = render
        .find("Body::Diff {")
        .unwrap_or_else(|| panic!("the diff arm exists:\n{render}"));
    let output_start = render
        .find("Body::Output =>")
        .unwrap_or_else(|| panic!("the output arm exists:\n{render}"));
    assert!(
        diff_start < output_start,
        "the arms are in the documented order:\n{render}"
    );
    let diff_arm = &render[diff_start..output_start];
    assert!(
        !diff_arm.contains("render_tool_output("),
        "the diff arm does not also draw general output:\n{diff_arm}"
    );
    let output_arm = &render[output_start..];
    assert!(
        output_arm.contains("render_tool_output("),
        "and the output arm is where that block lives:\n{output_arm}"
    );
}

// Verifies: gh #328 design point 3 - **the fix is not lossy**: the classifier splits the
// result rather than discarding it.
//
// The easy over-correction is "drop the general block when there is a diff", which
// silently deletes a trailing note or a truncation marker. `Body::Diff` therefore carries
// a `note`, and `split_summary_and_note` is what fills it. The behavioural counterpart is
// `a_partly_diff_result_keeps_its_trailing_note`; this row pins that the mechanism exists
// so a later simplification to "just the summary" fails here too.
#[test]
fn the_classifier_keeps_a_note_for_what_the_diff_lacks() {
    let text = source("crates/lca-ui/src/transcript/render.rs");
    let body = function_body(&text, "enum Body<'a> {");
    assert!(
        body.contains("note: Option<String>"),
        "the diff variant carries the leftover text:\n{body}"
    );
    let split = function_body(&text, "fn split_summary_and_note(");
    assert!(
        split.contains("diff_lines.contains(&trimmed)"),
        "a line the diff already draws is skipped rather than repeated:\n{split}"
    );
    assert!(
        split.contains("note_lines"),
        "and the remaining prose becomes the note:\n{split}"
    );
    // The redundant early exit that was written first, and removed after mutation-testing
    // showed the per-line skip already answered it. Asserted on **code**, not on the word:
    // the doc comment above names `all_in_diff` precisely to explain why it is gone, so a
    // bare `contains` would fail against the correct file.
    assert!(
        !split.contains("if all_in_diff"),
        "no second rule restating the per-line skip:\n{split}"
    );
}

// Verifies: gh #328 design point 4 - the general block's **behaviour is unchanged**, not
// reimplemented.
//
// `render_tool_output` was extracted, so the risk is that the extraction quietly changed
// the preview rules while moving them. The row names the behaviours that must still be in
// the extracted function: the per-tool preview length, the carriage-return collapse on
// expand, the omission marker's side, and the single-row expand hint.
#[test]
fn the_extracted_output_block_keeps_its_behaviour() {
    let text = source("crates/lca-ui/src/transcript/render.rs");
    let body = function_body(&text, "fn render_tool_output(");
    for needle in [
        "strip_truncation_footer",
        "strip_wrapper_boilerplate",
        "preview_lines(name)",
        "collapse_carriage_returns",
        "build_preview",
        "marker_above",
    ] {
        assert!(
            body.contains(needle),
            "the extracted block still does {needle}:\n{body}"
        );
    }
    // The truncation footer is stripped here, which is what makes a truncated result's
    // marker appear once (the card's own warning) rather than twice.
    assert!(
        body.contains("gh #391 gap 4"),
        "and keeps the footer-stripping rationale with it:\n{body}"
    );
}
