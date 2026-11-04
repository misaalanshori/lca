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

// ============================================================================
// gh #370 flaw 2: one iteration, many events.
// ============================================================================

/// The input-path helpers, where the drain lives.
fn helpers_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/run_input_helpers.rs"
    ))
    .expect("run_input_helpers.rs is readable")
}

// Verifies: gh #370 flaw 2 - **the loop drains before it paints**. The loop popped one
// event per iteration and then did a full `viewport()` + `render()` pass, so a drag's
// every cell cost a complete repaint. The regression this prevents is structural - a
// later "simplification" back to a single pop - so it is asserted on the shape: the
// loop's input step is a drain, and the pop it used to call is gone.
//
// Asserted on the source because the loop needs a terminal, a turn runner and a wake
// channel, none of which a unit row has. The *behaviour* is measured beside the code
// (`crates/lca-ui/src/run_batch_tests.rs`): twelve events cost one frame, and the
// composer holds all twelve characters.
#[test]
fn the_loop_drains_every_arrived_event_before_it_paints() {
    let source = run_source();
    assert!(
        source.contains("input_helpers::drain_input("),
        "the loop's input step drains the channel"
    );
    // The old shape must not survive anywhere in the loop: one `try_recv` per iteration
    // is the flaw, whichever channel it reads.
    assert!(
        !source.contains("let pending_input = match early_input.as_ref()"),
        "the single-pop shape is gone"
    );
    // ...and every drained event is handled, not just the last. The loop iterates the
    // drained vector rather than destructuring one event out of it.
    assert!(
        source.contains("for data in input.drain(..)"),
        "every drained event is handled in order"
    );
    // The paint still happens once per iteration, after the handling - the drain changes
    // how many events are consumed, not when the frame is painted.
    let drain_at = source
        .find("input_helpers::drain_input(")
        .expect("the drain exists");
    let render_at = source
        .find("screen.render(&mut terminal, viewport, document, width, height)")
        .expect("the paint exists");
    assert!(
        render_at < drain_at,
        "the frame is painted before the drain in the same iteration (the loop renders \
         at its top and polls at its bottom), so a batch costs one frame"
    );
}

// Verifies: gh #370 flaw 2 - **the drain is bounded**, and the bound is a named constant
// with its rationale attached. An unbounded `while let Ok(..) = try_recv()` would never
// leave the drain while a continuous producer fills it, so the loop would stop painting
// entirely - the trap the brief names. The bound row beside the code asserts the
// behaviour; this one asserts the shape, because a literal at the call site is how the
// rationale gets lost.
#[test]
fn the_drain_is_bounded_by_a_documented_constant() {
    let helpers = helpers_source();
    assert!(
        helpers.contains("pub const MAX_DRAINED"),
        "the bound is a named constant"
    );
    assert!(
        helpers.contains("while drained.len() < MAX_DRAINED"),
        "and the drain actually consults it"
    );
    // The doc comment must sit on the constant, not in a distant module header: the
    // number and the reason for the number travel together.
    let decl = helpers
        .find("pub const MAX_DRAINED")
        .expect("the constant exists");
    let context = &helpers[decl.saturating_sub(1200)..decl];
    assert!(
        context.contains("well above"),
        "the constant's doc explains why that number is safe: {context}"
    );
}

// Verifies: gh #370 flaw 2 - **the drain never sleeps or waits for more.** A deadline, a
// "settle window", or a `try_recv` retry loop would add latency to the first keystroke of
// a burst by construction - exactly the regression flaw 1's receipt (median echo 20.1 ->
// 20.3 ms) would show. The drain takes what is there and returns.
#[test]
fn the_drain_adds_no_wait_of_its_own() {
    let helpers = helpers_source();
    let start = helpers
        .find("pub fn drain_input")
        .expect("the drain exists");
    let body = &helpers[start..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    for forbidden in ["sleep", "recv_timeout", "Instant::now", "Duration"] {
        assert!(
            !body.contains(forbidden),
            "the drain must not wait for more events (`{forbidden}` found): a bounded \
             wait here is latency added to every burst's first keystroke"
        );
    }
}

// Verifies: gh #370 flaw 2 - **keystrokes and mouse motions are one contract.** The
// brief allowed collapsing a motion burst to its last position; LCA handles every event
// instead, because a per-kind rule ("mouse may collapse, keys may not") is a second
// contract for the next event kind to get wrong, and because a drag's intermediate
// positions are observable to the gesture that builds the selection. This pins that the
// drain has no kind test at all, so no future edit can introduce one silently.
#[test]
fn the_drain_does_not_distinguish_event_kinds() {
    let helpers = helpers_source();
    let start = helpers
        .find("pub fn drain_input")
        .expect("the drain exists");
    let body = &helpers[start..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    for forbidden in ["starts_with", "parse_sgr_mouse", "is_escape", "\"\\x1b\""] {
        assert!(
            !body.contains(forbidden),
            "the drain must not inspect the event kind (`{forbidden}` found): every \
             event is handled, which is the only rule that cannot eat characters"
        );
    }
}
