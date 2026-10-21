//! Gh #283 (NFR-24): Copilot's live model discovery used to stamp
//! `context_window: 0` on every row, discarding the curated table's real
//! windows.
//!
//! `0` is not "small" here: the footer reads it as `ctx ?`, and FR-SESS-4's
//! threshold falls back to `docs/compaction.md`'s 128k estimate - correct
//! only for a model whose real window is 128k. Copilot's curated figures are
//! 200k, so the defect gave every Claude and o-series session a threshold far
//! below the real one (early compaction) while the footer said nothing.
//!
//! This file guards the two halves that can silently drift apart: the
//! numbers the extension resolves (behaviour, from its own suite) and the
//! hand-off that must carry them across the sandbox boundary - Copilot
//! ships as a WASM component, so a native-only fix would be dead code in
//! the binary users actually run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The source of the shipped guest's `list_models` must forward the
/// resolved extras rather than replacing them with an empty list.
///
/// A behavioural test cannot reach this: `wasm_mode`'s `list_models` runs
/// against a `GUEST_CAP` static that only exists inside the component, so
/// the truncation that shipped is invisible to the extension's native
/// suite. It was invisible to it for exactly that reason.
#[test]
fn the_shipped_component_forwards_a_rows_extras() {
    let source = include_str!("../../extensions/github-copilot/src/wasm_mode.rs");
    let start = source
        .find("impl ModelsGuest for GitHubCopilotWasm")
        .expect("the guest's model export exists");
    let body = &source[start..];
    let end = body.find("\nimpl ").unwrap_or(body.len());
    let export = &body[..end];
    assert!(
        !export.contains("extras: Vec::new()"),
        "the guest drops the resolved extras again: the endpoint's capability \
         hints would never reach the host in the shipped component"
    );
    assert!(
        export.contains("extras: model"),
        "the guest must forward `model.extras` across the WIT boundary: {export}"
    );
}

/// The window resolution must consult the curated table and the endpoint's
/// declared limit, and must never invent a number.
///
/// Pinned at the source level because the *shape* is the contract here:
/// the bug was a hardcoded literal sitting where a lookup belonged, and a
/// future edit could reintroduce one without failing any behavioural row
/// (an all-curated fixture would still read correctly).
#[test]
fn the_resolver_keeps_the_curated_table_and_the_endpoint_fields() {
    let source = include_str!("../../extensions/github-copilot/src/lib.rs");
    let start = source
        .find("fn resolve_row(row: LiveRow)")
        .expect("the resolver exists");
    let body = &source[start..];
    let end = body.find("\n/// ").unwrap_or(body.len());
    let resolver = &body[..end];
    assert!(
        resolver.contains("CURATED_MODELS"),
        "the curated table is a source of windows: {resolver}"
    );
    assert!(
        resolver.contains("declared_window()"),
        "the endpoint's declared limit is the second source: {resolver}"
    );
    assert!(
        !resolver.contains("128_000") && !resolver.contains("128000"),
        "no fabricated fallback window: an unknown model stays 0 (unknown), \
         because a wrong denominator silently mis-triggers compaction"
    );
    // The endpoint field names are GitHub's, not guesses; they live in one
    // place so a rename cannot be half-applied.
    assert!(
        source.contains("max_context_window_tokens"),
        "the endpoint's window field is read by name"
    );
    assert!(
        source.contains("max_prompt_tokens"),
        "the documented prompt-cap fallback is read by name"
    );
}
