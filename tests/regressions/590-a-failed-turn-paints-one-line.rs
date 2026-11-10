//! Gh #590: a failed turn paints **one** error line, and the count is what the
//! guards assert.
//!
//! The behavioural rows live beside the code they drive
//! (`crates/lca-ui/src/run_590_tests.rs`), because the duplicate appears only when
//! a turn's *event* and its *outcome* are applied to the same `Chat` through the
//! real `TurnState::reap` seam — a row that called `Chat::on_turn_event` alone
//! would see one line and pass on the broken tree.
//!
//! What this file pins is the part a later "simplification" would quietly undo:
//! the run loop's settle must paint `outcome.error` **only** when this turn's own
//! event did not already paint it, and the event must be what sets that fact. A
//! structural row is the right shape here because the failure mode is a *second*
//! call site appearing, which no single behavioural row can rule out: the next
//! one could live in a path this cycle's rows do not drive.
//!
//! The brief's own justification for the decision was wrong and is recorded here
//! so nobody re-derives it: there is **no persisted error record**. `Record` has
//! no `Error` variant and `replay_records` never pushes an `Entry::Error`, so both
//! paints are live-only - `Entry::Error` is history-shaped because the turn's own
//! event paints it, not because a resume restores it.
//!
//! Verifies: gh #590, FR-CORE-7 (the user sees the failure, once).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The run loop's settle region: from the identity of the owner to the usage fold.
fn reap_region() -> String {
    let source = crate::module_text("crates/lca-ui/src/run.rs");
    let start = source
        .find("fn reap(&mut self, chat: &mut Chat)")
        .expect("reap still exists under that name");
    let end = source[start..]
        .find("chat.with_tab_state(owner, |chat| chat.fold_turn_usage")
        .expect("the usage fold still ends the settle's paint region")
        + start;
    source[start..end].to_string()
}

// Verifies: gh #590 - **the outcome's notice is gated on this turn's paint flag.**
//
// The two failure shapes are named: reading the flag, and *not* calling
// `chat.say` when it is set. Mutation: deleting the `if !painted` guard (or
// looking the flag up anywhere but the owning tab) reddens this row.
#[test]
fn the_settle_paints_the_outcome_error_only_when_the_turn_painted_nothing() {
    let region = reap_region();
    assert!(
        region.contains("turn_error_painted"),
        "the settle reads the turn's own paint flag:\n{region}"
    );
    assert!(
        region.contains("if !painted"),
        "and it skips the notice when the event already painted one:\n{region}"
    );
    // One `say`, inside the guard: a second unconditional one is exactly the
    // duplicate this issue is about.
    assert_eq!(
        region.matches("chat.say(").count(),
        1,
        "exactly one notice call, the fallback:\n{region}"
    );
    // Gh #462: the flag is per turn, so it is read through the owning tab.
    assert!(
        region.contains("with_tab_state(owner"),
        "the flag is read on the tab that owns the turn, never the visible one:\n{region}"
    );
}

// Verifies: gh #590 - **the turn's own event is what sets the flag**, and the flag
// starts clear for a new turn.
//
// Without the setter the settle would always paint the fallback (the old
// behaviour); without the reset a second turn would inherit the first's answer.
#[test]
fn the_error_event_sets_the_flag_and_a_new_turn_clears_it() {
    let events = crate::module_text("crates/lca-ui/src/chat_turn_events.rs");
    let error_arm = events
        .find("TurnEvent::Error { message, .. } => {")
        .expect("the error arm still exists");
    let arm_end = error_arm
        + events[error_arm..]
            .find("TurnEvent::AssistantText(_) => {}")
            .expect("the arm still ends at the next variant");
    assert!(
        events[error_arm..arm_end].contains("self.turn_error_painted = true"),
        "the event that paints the line is the one that records it:\n{}",
        &events[error_arm..arm_end]
    );
    let lifecycle = crate::module_text("crates/lca-ui/src/chat/chat_turn_state.rs");
    let begin = lifecycle
        .find("pub fn begin_turn(")
        .expect("begin_turn still exists");
    let begin_end = begin
        + lifecycle[begin..]
            .find("pub fn take_submitted(")
            .expect("begin_turn still ends where the takers start");
    assert!(
        lifecycle[begin..begin_end].contains("self.turn_error_painted = false"),
        "each turn starts with a clean paint flag:\n{}",
        &lifecycle[begin..begin_end]
    );
}

// Verifies: gh #590 - **the flag travels with its tab** (gh #462's rule).
//
// The turn belongs to the tab that submitted it, so the flag must be in the
// swap set: a background turn's settle reads the flag beside the transcript that
// painted it. A flag left on `Chat` alone would be read against whichever tab
// happens to be visible when the worker finishes.
#[test]
fn the_paint_flag_is_per_tab_state() {
    let tabs = crate::module_text("crates/lca-ui/src/chat_tabs.rs");
    assert!(
        tabs.contains("pub turn_error_painted: bool"),
        "the flag is a field of the per-tab turn state"
    );
    let swaps = tabs.matches("stream.turn_error_painted").count();
    assert_eq!(
        swaps, 2,
        "it is swapped both ways (live_into and live_from), like the transcript it describes"
    );
}

// Verifies: gh #590 - **the decision's premise, from the code.** The brief
// justified its answer with "a persisted error record replays into `Entry::Error`".
// There is no such record: the decision still holds, but on the other ground (the
// event is the turn's own channel, and the one headless turns into JSON).
//
// This row exists so the next reader cannot re-introduce the false premise as a
// argument for deleting either paint.
#[test]
fn there_is_no_persisted_error_record_to_replay() {
    let records = crate::module_text("crates/lca-protocol/src/record.rs");
    assert!(
        !records.contains("    Error {"),
        "no `Record::Error` variant exists, so nothing replays an error as history"
    );
    let replay = crate::module_text("crates/lca-ui/src/transcript/replay_records.rs");
    assert!(
        !replay.contains("push_error"),
        "the replay path never pushes an error entry either: both paints are live-only"
    );
}
