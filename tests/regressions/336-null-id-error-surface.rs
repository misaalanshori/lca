//! gh #336 - a JSON-RPC error carrying `"id": null` surfaces the server's own
//! words on **both** MCP selectors.
//!
//! The defect: `select_response` matched responses strictly by
//! `message.get("id").and_then(as_u64) == Some(id)`. Per JSON-RPC 2.0 a server
//! that cannot parse the request id - or any transport-level failure - answers
//! with `"id": null`, so such an error never matched and the caller reported its
//! own guess ("answered without our response") while the server's `"error"`
//! payload was dropped. "The server refused and said why" and "the server
//! answered something unrelated" are different diagnoses, and only one of them
//! is actionable.
//!
//! Two selectors exist, and the first fix only reached one of them:
//!
//! * `select_response` - the SSE/streamable-HTTP envelope (fixed in `ba7f7fce`,
//!   ADR-0047's Phase C);
//! * `response_for` - the single-outstanding-request stdio path, which is where
//!   the issue was originally filed from. It kept classifying a null-id error as
//!   "an id that is not ours" and reported `answered without an id ... the
//!   stream is desynchronized`, a claim about the stream that was false.
//!
//! The behaviour rows live in `crates/lca-mcp/src/wire.rs`
//! (`null_id_error_tests`): the verbatim payload on both paths, the notification
//! path unchanged, the foreign-id desync unchanged, our own reply winning over a
//! null-id error in the same envelope, and the success path untouched. This is
//! the NFR-24 pin that the mechanism cannot be quietly undone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #336 - the null-id error is a first-class classification, so no
// selector can silently drop it by falling through to "not ours".
//
// The first fix handled it *after* the loop in `select_response`, as a scan. That
// worked for that function and could not be reused by `response_for`, which is
// why the stdio path still swallowed the error. One classification, both callers.
#[test]
fn a_null_id_error_is_classified_not_missed() {
    let wire = read("crates/lca-mcp/src/wire.rs");
    assert!(
        wire.contains("UnattributedError(String)"),
        "the classification exists as its own variant; a null-id error is a \
         protocol-level answer, not a missing reply"
    );
    assert!(
        wire.contains("Classified::UnattributedError("),
        "and `classify` produces it"
    );
    // Both selectors must handle it explicitly rather than letting it fall to a
    // catch-all. `select_response` stores it (so an attributable reply wins);
    // `response_for` returns it (there is only ever one reply on that transport).
    //
    // Counted as "arms that bind and use the variant" rather than by an exact
    // source line: `cargo fmt` reflows a `let`-chain across lines, and a
    // substring match on the unformatted form goes red for no reason (the trap
    // that bit the gh #389/#400/#288 guards).
    let arms = wire
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with("Classified::UnattributedError(error) =>")
        })
        .count();
    assert_eq!(
        arms, 2,
        "both selectors handle the variant - found {arms} arms, and a missing one \
         means that transport silently drops the server's error again"
    );
    // The null id is detected by shape. That an `error` member is required is
    // checked behaviourally by `a_notification_is_still_skipped` and
    // `a_foreign_id_is_still_a_desync`; asserting the syntactic form here would
    // only pin rustfmt's layout.
    assert!(
        wire.contains("is_some_and(serde_json::Value::is_null)"),
        "the null id is detected as a null JSON value"
    );
}

// Verifies: gh #336 design point 1 - the generic text is gone for that case.
//
// The rows assert the server's message is present; this asserts LCA's guess is
// absent, which is the other half of the same claim. Without it a fix could add
// the server's text alongside "answered without our response" and pass.
#[test]
fn the_generic_text_is_not_used_for_a_null_id_error() {
    let wire = read("crates/lca-mcp/src/wire.rs");
    // The generic strings still exist - they are correct for a genuinely
    // unrelated envelope.
    assert!(
        wire.contains("MCP server answered without our response (id {id})"),
        "the generic text is still there for an envelope with nothing for us"
    );
    assert!(
        wire.contains("answered without an id (wanted id {id})"),
        "and the stdio desync text is still there for a genuinely id-less message"
    );
    // What must not happen is a null-id *error* reaching either. The rows in
    // `wire.rs` check the text; this checks the routing that makes it true: the
    // null-id arm returns before the `Foreign` branch.
    let null_arm = wire
        .find("is_some_and(serde_json::Value::is_null)")
        .expect("the null-id detection");
    let foreign = wire
        .find("return Classified::Foreign(found)")
        .expect("the foreign-id branch");
    assert!(
        null_arm < foreign,
        "a null-id error must be classified before the foreign-id branch, or it \
         is reported as desync again"
    );
}

// Verifies: gh #336 design point 4 - the payload is verbatim (#159's precedent).
#[test]
fn the_payload_is_forwarded_not_reinterpreted() {
    let wire = read("crates/lca-mcp/src/wire.rs");
    // The error value is rendered (its own JSON), never decomposed and rebuilt:
    // `{"code":..,"message":..}` reaches the caller as the server sent it.
    assert!(
        wire.contains(r#"format!("MCP server errored: {error}")"#),
        "the server's error object is embedded as-is inside LCA's context"
    );
    // No extraction of just `.message` or just `.code`: either would lose the
    // other half of the server's diagnosis.
    let null_region: String = wire
        .lines()
        .skip_while(|line| !line.contains("is_some_and(serde_json::Value::is_null)"))
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !null_region.contains(".get(\"message\")") && !null_region.contains(".get(\"code\")"),
        "the handler must not pick the payload apart: {null_region}"
    );
}

// Verifies: gh #336 design point 3 - the two neighbouring behaviours are pinned
// by rows, not merely by the code's shape.
#[test]
fn the_neighbours_have_rows() {
    let wire = read("crates/lca-mcp/src/wire.rs");
    for row in [
        "fn the_stdio_path_surfaces_a_null_id_error_verbatim",
        "fn the_error_payload_crosses_verbatim",
        "fn a_notification_is_still_skipped",
        "fn a_foreign_id_is_still_a_desync",
        "fn our_own_reply_wins_over_a_null_id_error",
        "fn the_success_path_is_unchanged",
        "fn an_error_for_our_id_is_still_attributed",
    ] {
        assert!(
            wire.contains(row),
            "the row `{row}` is present: a null-id error must be surfaced without \
             swallowing notifications, foreign ids, or our own reply"
        );
    }
}
