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
    // Both roots: gh #575 moved `TurnState` (and its channels) into
    // `run_turns.rs`, which `run.rs` declares with `pub(crate) use` (a re-export,
    // not a `mod`), so the module walk cannot reach it from `run.rs` alone.
    crate::text_of(&["crates/lca-ui/src/run.rs", "crates/lca-ui/src/run_turns.rs"])
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
    // ...and it does wait on the wake channel. Flaw 3 made the timeout the scheduler's
    // remaining budget (capped at the tick), so the call is `wait_for`; the property this
    // guard exists for - the loop blocks on the wake channel, not on stdin - is unchanged.
    assert!(
        source.contains("wakes.wait_for("),
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

// Verifies: gh #370 flaw 1, carried through flaw 3 - **one render decision**. The fix
// changes when the loop wakes, not when it paints: a per-event paint or a second render
// path would undo the point of coalescing.
//
// Flaw 3 replaced the bare `dirty` bool with `FrameClock`, which is the same decision plus
// *when* it is allowed - so this guard now asserts the shipped shape rather than the old
// flag's spelling: still exactly one paint site, and every wake arm still **requests** a
// frame (through one of the clock's two tiers) instead of painting itself.
#[test]
fn there_is_still_exactly_one_render_decision() {
    let source = run_source();
    // The loop's frame is the single `screen.render(` call.
    //
    // Gh #551 fixed `module_sources` to follow `pub(crate) mod` declarations, so
    // the walk now reaches `run_input_helpers.rs`, whose prose *mentions*
    // `screen.render()` in a comment. Counting code, not comments, is what this
    // row always meant (the same reason `402` uses `code_only`).
    let code = crate::code_lines(&source);
    let renders = code.matches("screen.render(").count();
    assert_eq!(
        renders, 1,
        "the loop paints in exactly one place (found {renders})"
    );
    // ...and the gate is the clock's, consulted in that one place.
    assert_eq!(
        source.matches("frames.render(now)").count(),
        1,
        "the one paint site asks the clock"
    );
    // Every background arm requests tier 1...
    let stream_requests = source.matches("frames.request_stream()").count();
    assert!(
        stream_requests >= 10,
        "the background arms set the paced flag instead of painting (found {stream_requests})"
    );
    // ...and exactly one site - the input drain - requests tier 2.
    let immediate_requests = source.matches("frames.request_immediate()").count();
    assert_eq!(
        immediate_requests, 2,
        "input and resize are the only unpaced requests (found {immediate_requests})"
    );
    // No arm may paint directly: nothing outside the gate calls the renderer.
    assert!(
        !source.contains("dirty = true;"),
        "the bare flag is gone; a reintroduced second decision would show up here"
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
    // The wait blocks on a timeout the caller passes, and flaw 3's scheduler passes the
    // clock's remaining budget. The property this guard exists for is unchanged - the wait
    // is bounded by a *named* value derived from the tick, never an unbounded `recv()`:
    // `wait_for` is documented to shorten the wait but never lengthen it, and
    // `FrameClock::next_wait` returns `WAIT_TICK` whenever nothing is pending.
    assert!(
        wake.contains("recv_timeout(timeout)"),
        "and it is what the wait actually blocks on"
    );
    assert!(
        wake.contains(".min(WAIT_TICK)"),
        "the scheduler's wait is capped by the tick, so nothing waits longer than before"
    );
    // Input must be consulted before the blocking wait, or a keystroke arriving
    // between the drain and the sleep would wait out the tick.
    let input_first = wake
        .find("if input_ready()")
        .expect("the wait consults input");
    let wait = wake.find("recv_timeout(timeout)").expect("the wait exists");
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

// ============================================================================
// gh #370 flaw 3: the two-tier render scheduler.
// ============================================================================

// Verifies: gh #370 flaw 3 - **the two tiers are wired to the two sources**. Tier 1 (paced)
// is what background activity requests; tier 2 (immediate) is what input and resize
// request. The pairing is the whole design: sharing one flag would make the answer depend
// on arrival order, and a stream delta that landed after a keystroke would be
// indistinguishable from one that landed before.
//
// The behaviour rows live beside the clock (`run_wake::frame_clock_tests`); this asserts
// the wiring, because "the clock exists but the loop uses one flag for both" is exactly the
// half-done shape that would pass every unit row.
#[test]
fn the_two_tiers_are_wired_to_their_two_sources() {
    let source = run_source();
    let stream = source.matches("frames.request_stream()").count();
    let immediate = source.matches("frames.request_immediate()").count();
    assert!(
        stream >= 10,
        "background arms request the paced tier (found {stream})"
    );
    assert_eq!(
        immediate, 2,
        "only input and resize request the unpaced tier (found {immediate})"
    );
    // The input drain must be one of them, or tier 2 has no producer: the request has to
    // sit in the same arm that handles the burst, between the drain and the arm's close.
    let drain = source
        .find("for data in input.drain(..)")
        .expect("the input drain exists");
    let after_drain = &source[drain..];
    let request = after_drain
        .find("frames.request_immediate()")
        .expect("the input drain requests an immediate frame");
    let arm_end = after_drain
        .find("\n        }\n")
        .unwrap_or(after_drain.len());
    assert!(
        request < arm_end,
        "the tier-2 request belongs to the burst-handling arm, so no path can handle input \
         without requesting a frame"
    );
}

// Verifies: gh #370 flaw 3 - **the loop asks the clock before it paints, and the clock's
// answer is what gates the paint.** A loop that kept painting on `dirty` and merely *also*
// updated a clock would pass the unit rows and change nothing.
#[test]
fn the_paint_gate_consults_the_clock() {
    let source = run_source();
    let gate = source
        .find("frames.render(now)")
        .expect("the loop asks the clock");
    let paint = source
        .find("screen.render(&mut terminal, viewport, document, width, height)")
        .expect("the paint exists");
    assert!(
        gate < paint,
        "the clock is consulted before the paint, in the same pass"
    );
    // ...and the frame it releases is recorded, or every stream frame would think no
    // interval had elapsed and the pace would never apply.
    assert!(
        source.contains("frames.mark_rendered(now)"),
        "a painted frame is recorded on the clock"
    );
    // The wait uses the clock's remaining budget, so a queued tier-1 frame lands on its
    // boundary instead of the tick.
    assert!(
        source.contains("frames.next_wait("),
        "the wait is the clock's remaining budget"
    );
}

// Verifies: gh #370 flaw 3 (**the docs claim**) - `docs/flows.md` promises that "the
// renderer coalesces on a frame interval, so a fast stream does not cost one redraw per
// token". The brief's rule is that this cycle either honours that sentence or amends it
// with a reason; this guard asserts the sentence and the code say the same thing, in both
// directions, so the promise can never quietly become false again.
#[test]
fn the_documented_coalescing_claim_is_true_of_the_code() {
    let flows = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/flows.md"))
        .expect("docs/flows.md is readable");
    assert!(
        flows.contains("The renderer coalesces on a frame interval, so a fast stream does"),
        "the documented promise is still stated"
    );
    // The promise needs a real interval in the code, which is the clock's constant.
    let wake = wake_source();
    assert!(
        wake.contains("pub const MIN_RENDER_INTERVAL"),
        "the interval the sentence promises exists"
    );
    // ...and a real coalescing decision, which is the pace.
    assert!(
        wake.contains("saturating_duration_since(last) >= MIN_RENDER_INTERVAL"),
        "and the pace is applied relative to the last frame, as the sentence describes"
    );
    // The doc must not still claim the *old* single-tier shape: the sentence now has to
    // mention the input tier, because throttling everything is the mushy-keystroke bug.
    assert!(
        flows.contains("preempt") || flows.contains("keystroke"),
        "the paragraph states the input-preemption half too, since throttling everything \
         is the failure the two tiers exist to avoid"
    );
}
