//! Gh #594: the interactive `/compact` summarizes on the budget derived from the
//! model's window, not on the 4096-token stopgap.
//!
//! The defect was a headless/TUI parity gap: `headless.rs` derives the budget
//! from the compaction reserve (`0.8 × reserve`, once the model window is known)
//! and `crates/lca-cli/src/tui/**` never called `set_summarization_budget` at
//! all, so `ProviderBackend::complete` fell back to `SUMMARIZATION_MAX_TOKENS`
//! (4096) for every interactive compaction - whatever the model's window, and
//! whatever the size of the transcript.
//!
//! This row is the pane-level receipt the brief asks for, and it is the one that
//! reaches the backend the interface actually builds: it runs the real binary in
//! a tmux pane against the loopback mock, with a **1M-token** model window, types
//! a turn, runs `/compact`, and reads the `max_tokens` the summarization request
//! carried out of the mock's recorded bodies. Pre-fix that number is 4096; with
//! the fix it is the derived one (a 1M window at the default 0.8 threshold
//! reserves 200,000 tokens, so the budget is 160,000).
//!
//! Verifies: gh #594, FR-SESS-4 (compaction is triggered and summarized through
//! the session's own model), and the headless/TUI parity requirement
//! `docs/compaction.md` states.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// The model window the mock-backed session advertises: a 1M-token model, the
/// shape the real failure hit (a long session compacts to an excerpt because the
/// summary was cut at 4096 tokens).
#[cfg(unix)]
const MILLION: &str = "1000000";
/// `floor(0.8 × reserve)` for the window above at the default 0.8 threshold:
/// `reserve = (1 − 0.8) × 1,000,000 = 200,000`, `budget = 160,000`.
#[cfg(unix)]
const DERIVED: &str = "160000";
/// The stopgap `ProviderBackend` falls back to when nobody set a budget.
#[cfg(unix)]
const STOPGAP: &str = "4096";

/// The `max_tokens` a recorded request body carried, when it carried one.
/// A `GET /models` records an empty body, so a parse failure is "carried none".
#[cfg(unix)]
fn max_tokens(body: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.get("max_tokens").and_then(serde_json::Value::as_u64)
}

// Verifies: gh #594 - **an interactive `/compact` carries the budget derived from
// the model's window.** Mutation: dropping the `set_summarization_budget` call at
// the interface's assembly site reddens this row with `4096`, which is exactly
// how the defect shipped.
#[cfg(unix)]
#[test]
fn an_interactive_compaction_carries_the_budget_derived_from_the_model_window() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        // The turn that seeds the transcript a compaction can summarize.
        Reply::Sse(sse_text("first reply")),
        // The summarization itself.
        Reply::Sse(sse_text("the summary")),
    ]));
    let sandbox = sandbox("compact-budget-594");
    // `OPENAI_CONTEXT_WINDOW` is the documented window knob
    // (docs/providers/openai-compatible.md); the model window is what the
    // derivation scales with, so it is the one input this row moves.
    let env = [("OPENAI_CONTEXT_WINDOW", MILLION)];
    let session = Tmux::new("compact-budget");
    session.spawn(&sandbox, Some(&mock), true, &env, &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(30),
    );

    // Gh #444/#446: the model label painting does not prove the input thread is
    // reading, and a prompt sent into that gap is lost invisibly - the row then
    // waits 30 s for a reply to a turn that never started (measured: 2 of 5
    // runs failed that way on an idle box). The echoed-probe primitive is the
    // documented close for this gap.
    session.wait_for_input_ready(std::time::Duration::from_secs(15));
    // Gh #444: the model label painting does not prove the input thread is
    // reading; a prompt sent into that gap is lost invisibly.
    session.wait_for_input_ready(std::time::Duration::from_secs(15));
    session.send(&["say a thing", "Enter"]);
    // **45 s, not 30, and the reason is the product's own countdown.** The
    // endpoint is unconsented, so the first turn raises the permission modal,
    // whose auto-approve deadline is 30 s (`run_turns.rs`: `now() + 30 s`,
    // FR-UI-18). Measured: the reply paints at **30.09-30.35 s** on every run -
    // i.e. exactly when the countdown fires - so a 30 s wait raced the
    // countdown's own timer and failed 4 of 6 runs. The budget has to sit
    // outside the countdown it is waiting for; 45 s is the countdown plus the
    // reply's own latency.
    session.wait_for("first reply", std::time::Duration::from_secs(45));
    session.send(&["/compact", "Enter"]);
    session.wait_for("compacted:", std::time::Duration::from_secs(30));

    let bodies = mock.bodies();
    let summarization = bodies
        .iter()
        .filter_map(|body| max_tokens(body))
        .next()
        .unwrap_or_else(|| panic!("the summarization request carried no max_tokens: {bodies:#?}"));
    assert_ne!(
        summarization.to_string(),
        STOPGAP,
        "the interactive path used the stopgap budget ({STOPGAP}) instead of deriving \
         one from the {MILLION}-token window (gh #594)"
    );
    assert_eq!(
        summarization.to_string(),
        DERIVED,
        "the budget is floor(0.8 × reserve) for a 1M window at the default threshold"
    );
}
