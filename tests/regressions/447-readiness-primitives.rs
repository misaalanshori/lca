//! gh #447 - the harness's readiness primitives exist and keep their negative
//! behaviour.
//!
//! `main` was red on most pushes since batch-13, and the shape was one class, not
//! five bugs: a row asked the pane one question and treated the answer as
//! complete. `wait_for(head)` returns the frame the head was painted in, so a row
//! asserting a *second* glyph read two moments as one; and `[session in` is
//! painted by the first render, which says nothing about the input thread, so a
//! scenario typed in that gap was lost with no trace to poll for.
//!
//! The primitives live in `crates/lca-cli/tests/common/readiness.rs`, because a
//! fix per row is the thing this cycle removes. This guard exists so a future
//! "simplification" back to `sleep`/`wait_for(glyph)` fails loudly: it pins that
//! the primitives exist, that the classes they close are actually routed through
//! them, and that their negative rows are still negative — a fix that turns a
//! flake into a hang or a silent pass is worse than the flake.
//!
//! The behaviour rows are in `crates/lca-cli/tests/harness_contract.rs` (settling,
//! never settling, echoing, never echoing, retry-once's laziness and its
//! two-miss ceiling, and the documented settle ceiling) and
//! `crates/lca-mcp/tests/oauth_multihost.rs` (the in-flow observation). This is
//! the NFR-24 pin that the mechanism stays in one place.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #447 - the primitives exist, in one place, with the names the
// contract rows and the adopted rows use.
#[test]
fn the_readiness_primitives_live_in_one_place() {
    let readiness = read("crates/lca-cli/tests/common/readiness.rs");
    for fact in [
        "pub fn wait_until_painted(",
        "pub fn wait_until_painted_e(",
        "pub fn wait_settled_for(",
        "pub fn wait_settled_for_e(",
        "pub fn wait_for_input_ready(",
        "pub fn retry_once<T>(",
        // The settle budget rides the same load factor as `wait_for`, so two
        // budget rules cannot disagree (gh #413 is not renegotiated).
        "timeout * Self::load_factor()",
    ] {
        assert!(readiness.contains(fact), "readiness.rs must define {fact}");
    }
    // The module is wired into the harness, not merely present.
    let common = read("crates/lca-cli/tests/common/mod.rs");
    assert!(
        common.contains("mod readiness;"),
        "the harness declares the readiness module"
    );
    assert!(
        common.contains("pub use readiness::{retry_once, settle_stats};"),
        "and re-exports what the rows and the contract rows use"
    );
}

// Verifies: gh #447 - the five named rows route their timing through the
// primitives, so the class cannot come back row by row.
//
// Each row is checked for the primitive it needs, not for a line number: a
// `wait_for` that was replaced by a `sleep` again would pass a "the file changed"
// check and fail this one.
#[test]
fn the_named_rows_wait_through_the_primitives() {
    // Two-frame class: read a settled frame before asserting a second glyph.
    let picker = read("crates/lca-cli/tests/e2e_picker_scroll.rs");
    assert!(
        picker.contains("wait_settled_for(\"key = value [source]\""),
        "the settings picker settles before asserting its border"
    );
    assert!(
        picker.contains("wait_for_input_ready("),
        "and proves input is attached before typing at it"
    );

    let navigator = read("crates/lca-cli/tests/e2e_tree_navigator.rs");
    assert!(
        navigator.contains("wait_settled_for(\"Message branches\""),
        "gh #452's row settles before asserting two connectors"
    );
    assert!(
        navigator.contains("wait_settled_for(\"\\u{2514}\""),
        "and the unfold restore settles too"
    );

    let color = read("crates/lca-cli/tests/e2e_color.rs");
    assert!(
        color.contains("wait_until_painted_e("),
        "the post-turn colour receipts settle in the SGR capture kind"
    );

    // Sent-before-attached class: the input path is a fact before a scenario.
    //
    // Checked **per row**, not per file: `e2e_slash_output.rs` holds two rows, so
    // a file-level `contains` would still pass with this fix removed from the
    // one that needs it. (The first version of this guard did exactly that.)
    let slash = read("crates/lca-cli/tests/e2e_slash_output.rs");
    let hotkeys = row(
        &slash,
        "fn hotkeys_lands_in_scrollback_while_the_dock_stays_bounded",
    );
    assert!(
        hotkeys.contains("wait_for_input_ready("),
        "gh #444's row waits for the reader before `/hotkeys`"
    );
    assert!(
        hotkeys.find("wait_for_input_ready(") < hotkeys.find("\"/hotkeys\""),
        "and the wait comes before the keys are sent, or it proves nothing"
    );

    let consent = read("crates/lca-cli/tests/e2e_model_consent.rs");
    let allow = row(
        &consent,
        "fn an_ungranted_endpoint_prompts_before_the_model_list_and_lists_after",
    );
    assert!(
        allow.contains("wait_for_input_ready("),
        "gh #446's row waits for the reader before `/model`"
    );
    assert!(
        allow.find("wait_for_input_ready(") < allow.find("\"/model\""),
        "and the wait comes before the keys are sent"
    );
}

/// One row's source, from its `fn` line to the next top-level `fn`.
///
/// A row is the unit of adoption: a file may hold several, so a guard that only
/// asks "does this file mention the primitive" cannot tell which row uses it.
fn row<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("the row `{signature}` is present"));
    let rest = &source[start..];
    // The next `}` at column 0 ends the function body.
    match rest.find("\n}\n") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

// Verifies: gh #447 - a live animation is not "settled", and the row that reads
// one says so instead of quietly timing out.
//
// `e2e_color`'s in-flight read keeps `wait_for_e`: the spinner animates
// continuously, so a settled frame is a frame that never arrives. The row's
// comment records why, so the next person does not "finish the job" by settling
// it and turning a passing row into a 25 s timeout (which is what happened while
// this cycle was being written).
#[test]
fn an_animating_frame_is_read_live_and_says_why() {
    let color = read("crates/lca-cli/tests/e2e_color.rs");
    assert!(
        color.contains("let working = session.wait_for_e(TOOL_PENDING"),
        "the in-flight read stays a live read, because the spinner animates"
    );
    assert!(
        color.contains("the spinner animates"),
        "and the reason is in the file, not only in a report"
    );
}

// Verifies: gh #447 - the negatives are still negative.
//
// The contract rows are the difference between a fix and a cover-up: a fake that
// never settles must fail (not hang, not pass), and a pane that never echoes must
// fail with a message about what a lost scenario means.
#[test]
fn the_contract_rows_keep_their_negatives() {
    let contract = read("crates/lca-cli/tests/harness_contract.rs");
    for row in [
        "fn wait_until_painted_returns_the_frame_after_the_screen_stops_changing",
        "fn a_frame_that_never_settles_fails_and_shows_both_frames",
        "fn wait_for_input_ready_proves_the_reader_and_leaves_no_probe",
        "fn a_pane_that_never_echoes_fails_with_a_useful_message",
        "fn retry_once_is_lazy_and_gives_up_after_the_second_miss",
        "fn a_pane_that_pauses_past_the_poll_interval_looks_settled",
    ] {
        assert!(
            contract.contains(row),
            "the contract row `{row}` is present"
        );
    }
    // The never-settling fake must assert a *failure*, with both frames, and no
    // re-spawn: a painted pane is a fact (gh #413's rule, unchanged).
    assert!(
        contract.contains("a pane that never settles must fail, not hang"),
        "the never-settling row demands a failure"
    );
    assert!(
        contract.contains("--- previous ---") && contract.contains("--- current ---"),
        "and the failure carries both frames"
    );
    // The deaf pane is *really* deaf: a pty echoes typed bytes even with no
    // reader, so a bare `sleep` would echo the probe and pass.
    assert!(
        contract.contains("stty -echo"),
        "the deaf fake turns echo off, or it would echo the probe and pass"
    );
}

// Verifies: gh #447 - the settle heuristic's ceiling is named where the next
// person will read it, and the doc says what would catch it if it bites.
#[test]
fn the_settle_ceiling_is_documented() {
    let readiness = read("crates/lca-cli/tests/common/readiness.rs");
    assert!(
        readiness.contains("Two identical frames is a **heuristic**, not a guarantee"),
        "the ceiling is stated as a limit, not sold as a guarantee"
    );
    for remedy in ["a longer quiet window", "a state marker the app emits"] {
        assert!(
            readiness.contains(remedy),
            "the doc names the remedy `{remedy}` so nobody has to guess"
        );
    }
    // And the testing plan carries it, so it is part of the documented contract
    // rather than a code comment only.
    let plan = read("docs/testing-plan.md");
    assert!(
        plan.contains("two identical frames is a heuristic, not a guarantee"),
        "docs/testing-plan.md section 14 states the ceiling"
    );
    assert!(
        plan.contains("wait_for_input_ready") && plan.contains("retry_once"),
        "and documents the three primitives"
    );
}

// Verifies: gh #447 - `retry_once` is lazy and bounded, which is the whole
// reason it is promoted rather than copy-pasted.
//
// The pattern's subtlety is that `(attempt(), attempt())` evaluates both eagerly,
// running the journey twice even when the first is fine; and a retry that is not
// bounded turns a broken row into a hang.
#[test]
fn retry_once_is_lazy_by_construction() {
    let readiness = read("crates/lca-cli/tests/common/readiness.rs");
    let body = readiness
        .split("pub fn retry_once<T>")
        .nth(1)
        .expect("the promoted helper is present");
    let body = body.split("\n}").next().unwrap_or(body);
    assert!(
        body.contains("match attempt()"),
        "the helper calls the first attempt before deciding: {body}"
    );
    assert!(
        !body.contains(',') || !body.contains("attempt(), attempt()"),
        "it must not evaluate both attempts eagerly"
    );
    // Structure, not prose: at most two calls to `attempt()`, so it cannot loop.
    assert!(
        body.matches("attempt()").count() <= 2,
        "at most two attempts, so a persistent miss fails instead of spinning"
    );
}
