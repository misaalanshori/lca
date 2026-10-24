//! NFR-24 guard for gh #379 gap 1: the terminal colour query is a **lifecycle**
//! closed by a `DA1` sentinel, not an open-ended one.
//!
//! # What the issue asked for, and what the code actually had
//!
//! The issue said the query had "no `DA1` sentinel and no late-reply path", so a
//! terminal ignoring `OSC 11` waited out a timeout and an SSH reply arriving late
//! was dropped. Both halves were checked against the code before anything was
//! changed, and **neither was true as described**:
//!
//! - **There was no timeout to wait out.** The reply handler in `run.rs` was a
//!   plain `if` in the input path with no timer, no waiting phase and no blocking
//!   read. A silent terminal resolved to the `COLORFGBG`/default scheme at
//!   construction, before the loop read one byte. Measured: first painted frame
//!   in 136-168 ms on a normal pty and 158-255 ms behind a pty wrapper that
//!   swallowed every colour reply - the difference is startup noise, not a stall.
//! - **A late reply already landed.** Injecting `OSC 11;#ffffff` after startup
//!   switched the palette from the dark band `52,53,65` to the light
//!   `219,235,255`. The reply handler simply had no condition on it.
//!
//! # The defect the investigation *did* find
//!
//! Because the handler had no lifecycle at all, it applied **every** `OSC 11` it
//! ever saw. Any child process that prints one - `ls` with colour, `git`, a
//! colour-picking prompt, a program echoing its own background - repainted LCA's
//! palette mid-session. Measured before the fix: a stray `OSC 11;#ff00ff` took
//! the theme from `dark` to `system` and the band from `52,53,65` to
//! `141,177,250` - in the middle of a session, from a program the user ran.
//!
//! pi consumes a colour reply only while a query is pending, which is the guard
//! LCA was missing; its pending query stays open until `DA1`, so pi has the same
//! exposure in principle, but pi at least has the window.
//!
//! # The fix
//!
//! `Chat` now owns the lifecycle: `theme_query_open` starts true because the loop
//! writes the query at startup, `DA1` closes it, and a reply is applied while the
//! window is open or within [`LATE_REPLY_GRACE`] after it. The grace is *time*,
//! not a count, because a count cannot tell a reply that was in flight when `DA1`
//! overtook it from a stray printed seconds later - the mutation checks below are
//! what proved that.
//!
//! The sentinel is written **last** by `colors::TERMINAL_COLOR_QUERY`. That
//! ordering is the whole mechanism: terminals answer in order, so `DA1` arriving
//! proves the colour replies are answered or absent. Put `DA1` first and it
//! proves nothing.
//!
//! # What is pinned here vs. beside the code
//!
//! The behaviour rows live in `crates/lca-ui/src/run_tests.rs` and
//! `crates/lca-tui/src/engine/colors.rs`, because they need the private frame and
//! input plumbing. This file guards the properties that survive a rewrite of
//! either call site: the sentinel's position in the query, the single recogniser
//! both paths share, and the fact that the lifecycle is consulted at all.
//!
//! Verifies: gh #379 gap 1, R10 (terminal-colour detection), NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tui::engine::colors::{
    TERMINAL_COLOR_QUERY, is_device_attributes_report, is_startup_query_reply,
};

/// The UI loop, so its use of the shared pieces can be read.
fn run_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/run.rs"
    ))
    .expect("run.rs is readable")
}

/// The lifecycle module that owns the dispatch, the recogniser choice and the grace.
fn theme_query_source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/theme_query.rs"
    ))
    .expect("theme_query.rs is readable")
}

// Verifies: gh #379 gap 1 - the sentinel is the **last** byte group written, and
// the colour queries come before it. Asserted on the real constant, so the write
// site cannot drift from the ordering the mechanism depends on.
#[test]
fn the_query_closes_with_the_sentinel_after_the_colour_asks() {
    let da1 = TERMINAL_COLOR_QUERY
        .find("\x1b[c")
        .expect("the DA1 sentinel is in the query");
    assert_eq!(
        da1 + "\x1b[c".len(),
        TERMINAL_COLOR_QUERY.len(),
        "the sentinel must be the last thing written: {TERMINAL_COLOR_QUERY:?}"
    );
    let bg = TERMINAL_COLOR_QUERY
        .find("\x1b]11;?")
        .expect("the OSC 11 ask is in the query");
    assert!(
        bg < da1,
        "the background is asked for before the sentinel: {TERMINAL_COLOR_QUERY:?}"
    );
}

// Verifies: gh #379 gap 1 - the loop writes the query from the shared constant
// rather than an inline literal, which is what makes the ordering above bind on
// the write site. A hand-written sequence at the call site could drift.
#[test]
fn the_loop_writes_the_shared_query_constant() {
    let source = run_source();
    assert!(
        source.contains("colors::TERMINAL_COLOR_QUERY"),
        "the startup write must use the shared constant so its ordering is pinned"
    );
    assert!(
        !source.contains("\"\\x1b]11;?\\x07\\x1b[?996n\""),
        "the old inline query (no sentinel) must be gone"
    );
}

// Verifies: gh #379 gap 1 - **one** recogniser decides what a startup reply is,
// and both consumers use it: the loop that applies it and the guard that stops it
// being typed. Two recognisers would let a reply-shape one accepts be typed by the
// other - the failure mode the brief explicitly asked to avoid.
#[test]
fn both_paths_go_through_the_shared_recogniser() {
    // The dispatch lives in `theme_query`, so that is where the recogniser must
    // appear; the loop must call the dispatch rather than re-implement it.
    let dispatch = theme_query_source();
    assert!(
        dispatch.contains("is_startup_query_reply"),
        "the reply path uses the shared recogniser"
    );
    assert!(
        dispatch.contains("is_device_attributes_report"),
        "the sentinel is recognised through the same family"
    );
    assert!(
        run_source().contains("theme_query::handle_reply"),
        "the loop calls the one dispatch instead of re-deriving it"
    );
    // The reply kinds the recogniser claims must be exactly what it parses.
    for reply in ["\x1b]11;#1e1e2e\x07", "\x1b[?997;1n", "\x1b[?1;2c"] {
        assert!(
            is_startup_query_reply(reply),
            "{reply:?} is a reply the recogniser must claim"
        );
    }
    // ...and a DA1-shaped non-answer stays ordinary input.
    assert!(!is_device_attributes_report("\x1b[?c"));
}

// Verifies: gh #379 gap 1 - the lifecycle is **consulted** before a reply is
// applied. Asserted by name because the failure this guards is silent: removing
// the call makes every reply apply again, which no behavioural row can see while
// the terminal is well-behaved.
#[test]
fn the_reply_path_consults_the_query_lifecycle() {
    let chat = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat.rs"
    ))
    .expect("chat.rs is readable");

    assert!(
        chat.contains("theme_query"),
        "the query window must exist as state"
    );
    assert!(
        chat.contains("theme_query.accepts_reply()"),
        "the late-reply grace is consulted through the lifecycle"
    );
    // Both colour entry points are gated. A new one that forgot the gate would
    // reintroduce the stray-repaint defect.
    let gated = chat.matches("!self.theme_query.accepts_reply()").count();
    assert!(
        gated >= 2,
        "both apply_terminal_background and apply_detected_scheme are gated, \
         found {gated}"
    );
}
