//! NFR-24 guard for gh #370 flaw 1: the interactive loop blocks on **every** source
//! it has, not stdin alone.
//!
//! # The flaw
//!
//! The loop's only blocking point was `input_rx.recv_timeout(50ms)` and only stdin
//! feeds that channel, so a `TurnEvent` (a streamed token), a shell chunk, or a
//! permission request could not wake it. The interface slept up to 50 ms before
//! noticing a delta: streaming capped at 20 FPS, and the *first* streamed token - the
//! one a reader is watching for - carried up to 50 ms of artificial latency.
//!
//! # The design, and why it cannot miss a wake
//!
//! Every producer signals a wake channel and the loop blocks on that instead. The two
//! failure directions are asymmetric and the asymmetry is the design:
//!
//! - **A dropped wake-up is a hang** - the delta waits for the next unrelated keypress.
//! - **A duplicate wake-up is a render** - `dirty` already coalesces.
//!
//! So the mechanism is built never to lose one, and the correctness argument is an
//! **ordering**: a producer enqueues its data first and signals second. The loop
//! cannot be between "drained the data" and "went to sleep" when the wake is sent,
//! because the data is already in the channel and the next drain sees it. Signal-then-
//! enqueue would open exactly that window.
//!
//! A shorter poll was deliberately rejected: lowering 50 ms would raise the frame cap
//! and still leave the first token waiting - the loop would remain blind to its other
//! sources, just less so.
//!
//! # What is pinned here vs. beside the code
//!
//! The behaviour rows live in `crates/lca-ui/src/run_wake.rs`, beside the mechanism:
//! three producers, input preemption, the 200 ms idle spin guard, the tick, and the
//! wake-during-render row. This file guards the **shape** of the loop, because the
//! regressions that would undo this fix are structural:
//!
//! 1. The loop must not go back to blocking on the input channel alone.
//! 2. `dirty` must remain the only render decision - no new paint path may appear.
//! 3. The tick must still exist, since it is the floor for sources with no producer.
//!
//! Verifies: gh #370 flaw 1, FR-UI-1's streaming contract, NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The loop, so its wait and its render decision can be read.
fn run_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/run.rs"
    ))
    .expect("run.rs is readable")
}

/// The wait mechanism itself.
fn wake_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/run_wake.rs"
    ))
    .expect("run_wake.rs is readable")
}

// Verifies: gh #370 flaw 1 - **the loop no longer blocks on the input channel alone**.
// This is the defect restated as a shape: `input_rx.recv_timeout(...)` as the loop's
// blocking point is exactly what made a streamed delta wait for a keystroke's poll
// period. Asserted on the source because the loop is not reachable from a unit test
// without a terminal, and the regression is someone "simplifying" back to it.
#[test]
fn the_loop_does_not_block_on_the_input_channel_alone() {
    let source = run_source();
    assert!(
        !source.contains("input_rx.recv_timeout"),
        "the loop must not block on stdin alone - that is the flaw (gh #370)"
    );
    assert!(
        !source.contains("early.recv_timeout(std::time::Duration::from_millis(50))"),
        "nor on the adopted early stream with the old fixed tick"
    );
    // ...and it does wait on the wake channel.
    assert!(
        source.contains("wakes.wait("),
        "the loop blocks on the wake channel"
    );
}

// Verifies: gh #370 flaw 1 - **every producer signals**. Three separate channels, so
// one being wired is not evidence about the others; this pins that all three forward
// through the wake (a `forwarded_channel` call per channel) rather than only the turn.
#[test]
fn every_producer_forwards_through_the_wake() {
    let source = run_source();
    let forwarded = source.matches("forwarded_channel").count();
    assert!(
        forwarded >= 3,
        "the turn, prompt and dialog channels each forward a wake; found {forwarded}"
    );
    // The shell path wires its waker through `UiState` instead, because a host
    // callback owns its producer end.
    assert!(
        source.contains("chat.world.waker = Some("),
        "the shell path is given a waker"
    );
}

// Verifies: gh #370 flaw 1 - **`dirty` is still the only render decision**. The fix
// changes when the loop wakes, not when it paints: a per-event paint or a second
// render path would undo the point of coalescing.
#[test]
fn dirty_stays_the_only_render_decision() {
    let source = run_source();
    // The loop's frame is the single `if dirty {` block.
    let renders = source.matches("screen.render(").count();
    assert_eq!(
        renders, 1,
        "the loop paints in exactly one place (found {renders})"
    );
    // ...and every wake arm feeds that decision rather than painting itself.
    let dirty_sets = source.matches("dirty = true;").count();
    assert!(
        dirty_sets >= 10,
        "the arms set the flag instead of painting (found {dirty_sets})"
    );
}

// Verifies: gh #370 flaw 1 - the tick exists and is finite, so sources with no
// producer to signal (the spinner, the OSC 9;4 keepalive, a hover's tooltip) still
// get their cadence. A wake-only loop would freeze its own animation.
#[test]
fn the_tick_remains_finite_and_is_used_as_the_wait_floor() {
    let wake = wake_source();
    assert!(
        wake.contains("pub const WAIT_TICK"),
        "the tick is a named constant, not a literal at the call site"
    );
    assert!(
        wake.contains("recv_timeout(WAIT_TICK)"),
        "and it is what the wait actually blocks on"
    );
    // Input must be consulted before the blocking wait, or a keystroke arriving
    // between the drain and the sleep would wait out the tick.
    let input_first = wake
        .find("if input_ready()")
        .expect("the wait consults input");
    let wait = wake
        .find("recv_timeout(WAIT_TICK)")
        .expect("the wait exists");
    assert!(
        input_first < wait,
        "input is tried before the blocking wait (preemption by ordering)"
    );
}

// Verifies: gh #370 flaw 1 - **the wake is sent after the enqueue**, which is the whole
// correctness argument for "cannot miss". A forwarder that signalled first would let
// the loop sleep between the wake and the data.
#[test]
fn each_forwarder_signals_after_enqueuing() {
    let wake = wake_source();
    let start = wake
        .find("pub fn forwarded_channel")
        .expect("the forwarder exists");
    let body = &wake[start..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    let send = body
        .find("loop_tx.send(")
        .expect("the forwarder enqueues into the loop's channel");
    let signal = body.find("waker.wake()").expect("and signals");
    assert!(
        send < signal,
        "enqueue first, then wake - the reverse order can lose a wake"
    );
}
