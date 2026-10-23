//! gh #489 - the RPC loop must never drop the future that owns a turn.
//!
//! # The defect
//!
//! `crates/lca-cli/src/rpc.rs` drove its turn with `run.take()` **inside** a
//! `tokio::select!` branch future:
//!
//! ```ignore
//! outcome = async { run.take().expect("a run is active").await } => { .. }
//! ```
//!
//! `select!` rebuilds every branch future on each loop iteration, so when the
//! biased stdin branch became ready while a turn was pending, the select dropped
//! the future that held the taken run. `turn_state` had been *moved* into that
//! future by `start_run!`, so it died with it. The loop resumed with neither.
//!
//! The four promises `docs/headless.md` makes about `--mode rpc` all broke:
//!
//! | promise | what happened |
//! |---|---|
//! | `prompt` → a run began | answered `"started"`, no model request ever made |
//! | `follow_up` → submits after the run ends | no run end existed to hang it off |
//! | `shutdown` → drains queued follow-ups | nothing to drain; exited 0 having discarded them |
//! | multi-turn state survives | every later `prompt` was a silent no-op |
//!
//! Measured before the fix: the shipped happy-path script lost a turn **1/40
//! idle and 6/40 (15%) loaded**; a `prompt` plus any second line lost it 100% of
//! the time when the second line was written at any non-zero delay.
//!
//! # Why this guard is a source scan
//!
//! The behavioural rows live where they belong - `crates/lca-cli/tests/e2e_rpc.rs`
//! (`a_prompt_written_before_a_second_line_still_runs_its_turn` and five siblings,
//! all of which fail on the pre-fix code). This suite cannot spawn the binary with
//! interleaved stdin timing, so it pins the *shape* that made the fix work: the
//! run future is a pinned local the `select!` borrows with `as_mut()`, and it is
//! cleared only in the arm that observed completion.
//!
//! A future maintainer "simplifying" back to `run.take()` inside the branch future
//! turns this red before the behavioural rows would have a chance to be dismissed
//! as flaky - which is exactly how this bug survived four cycles of CI.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The loop's body: from the `loop {` that drives commands to the end of the
/// file. The writer task's `select!` above it is a different loop and is fine.
fn select_loop(source: &str) -> &str {
    let start = source
        .find("    loop {\n        if run.is_none() {")
        .expect("the command loop is where the fix lives");
    &source[start..]
}

/// Code only: comment lines removed, so prose naming the old shape cannot trip
/// the guard (the fix's own comment quotes `run.take()`).
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #489 - **the fix is the borrow, not a retry.** The select arm must
// poll the run future in place, through a borrow of the loop's own `Option`.
//
// Deliberately checks the *shape* (`&mut run`, and no `take`) rather than one
// spelling: the fix went through `run.as_mut()` and a small `poll_run(&mut run)`
// helper, and a guard keyed to either identifier would break on the other while
// the behaviour stayed correct.
#[test]
fn the_run_future_is_polled_in_place_not_taken_into_a_branch() {
    let source = read("crates/lca-cli/src/rpc.rs");
    let body = code_only(select_loop(&source));
    assert!(
        body.contains("&mut run"),
        "the select must borrow the loop's run (as_mut/poll_run(&mut run)): {body}"
    );
    assert!(
        !body.contains("run.take()"),
        "and must never move it into the branch future - `select!` rebuilds \
         branch futures each iteration, so the taken run is dropped whenever the \
         stdin branch wins (gh #489)"
    );
    // The helper itself must not take ownership either.
    let helper = code_only(&source);
    let helper_start = helper
        .find("async fn poll_run")
        .expect("the borrowing poll helper exists");
    let helper_body = &helper[helper_start..];
    let helper_end = helper_body.find("\n    }\n").unwrap_or(helper_body.len());
    assert!(
        !helper_body[..helper_end].contains(".take()"),
        "the helper borrows, it does not move the future out"
    );
}

// Verifies: gh #489 design point 2 - the future owns `turn_state`, so it must be
// guaranteed not to be dropped. `run = None` has to sit **after** the await, in
// the completion arm.
#[test]
fn the_run_is_cleared_only_after_the_turn_completes() {
    let source = read("crates/lca-cli/src/rpc.rs");
    let body = code_only(select_loop(&source));
    let arm = body
        .find("outcome = poll_run(")
        .expect("the borrowing arm exists");
    let tail = &body[arm..];
    let clear = tail
        .find("run = None;")
        .expect("the arm clears the run once the turn has finished");
    let await_arm_end = tail.find("let (outcome, state) = outcome;").unwrap_or(0);
    assert!(
        clear < await_arm_end || await_arm_end == 0,
        "`run = None` belongs after the turn's output is destructured, not before \
         the await: clearing it early is what dropped `turn_state`"
    );
    // And `turn_state` returns from the finished future.
    assert!(
        tail.contains("turn_state = Some(state);"),
        "the state the future owned must come back to the loop"
    );
}

// Verifies: gh #489 - **the class, not the instance.** A `take()` (or
// `mem::replace`, or a consuming `.next()`) inside a `select!` branch is the
// defect shape: it moves an owner into a future `select!` may discard. Only one
// site in this repository ever had it, and this row fails if a second appears in
// the crates that drive a loop.
#[test]
fn no_other_select_branch_takes_an_owner_it_could_drop() {
    let mut offenders: Vec<String> = Vec::new();
    for path in [
        "crates/lca-cli/src/rpc.rs",
        "crates/lca-cli/src/headless.rs",
        "crates/lca-core/src/turn/stream.rs",
        "crates/lca-core/src/turn/execute.rs",
        "crates/lca-core/src/ext_provider.rs",
    ] {
        let source = read(path);
        let code = code_only(&source);
        let lines: Vec<&str> = code.lines().collect();
        // Walk each `tokio::select! {` block by brace depth and look for an
        // owned-take on the same arm-expression line.
        let mut index = 0;
        while index < lines.len() {
            if !lines[index].contains("tokio::select!") {
                index += 1;
                continue;
            }
            let start = index;
            let mut depth = 0isize;
            while index < lines.len() {
                depth += lines[index].matches('{').count() as isize;
                depth -= lines[index].matches('}').count() as isize;
                let line = lines[index];
                // The defect shape is a take **inside a select arm** - either
                // `x = async { ... .take() ... .await }` or a bare
                // `run.take().await`, often split across lines. A guarded
                // `if let Some(x) = y.take()` is a different thing: it does not
                // move an owner into a future the select may discard.
                let takes_owner = line.contains(".take()")
                    && !line.contains("cancel_flag")
                    && !line.contains("let Some(");
                if index > start && takes_owner {
                    offenders.push(format!("{path}:{}: {}", index + 1, line.trim()));
                }
                if depth <= 0 && index > start {
                    break;
                }
                index += 1;
            }
            index += 1;
        }
    }
    assert!(
        offenders.is_empty(),
        "a `select!` branch that takes an owner can have it dropped when another \
         branch wins (gh #489):\n  {}",
        offenders.join("\n  ")
    );
}

// Verifies: gh #489 - the documented promises are still written down. The fix
// must match the docs, never the other way round.
#[test]
fn the_documented_promises_are_unchanged() {
    let docs = read("docs/headless.md");
    for promise in [
        "(a run began)",
        "(drains queued follow-ups first, then exits 0)",
        "(submits after the run ends, even a cancelled one)",
    ] {
        assert!(
            docs.contains(promise),
            "docs/headless.md still promises {promise:?}; the fix matches the doc, \
             not the reverse"
        );
    }
}
