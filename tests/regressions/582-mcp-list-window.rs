//! NFR-24 guard for gh #582: `lca mcp list`'s window is the handshake's, not a turn's.
//!
//! ## The defect
//!
//! `main` was red on `windows-latest`: `list_reports_a_connected_server_and_exits_zero`
//! failed with `Some(1)` and an **empty stderr**, three times across two attempts. That
//! signature is ambiguous by construction — it is what a child that never started looks
//! like *and* what a deadline expiring looks like — so the issue's first ask was
//! diagnosability and the second was a root cause.
//!
//! The cause was a borrowed constant. `LIST_WAIT_SECONDS` was `10`, copied from
//! `DEFAULT_STARTUP_WAIT_MS` — how long a **turn** waits before it answers the user. A
//! handshake is a different question, and its own default is 60 s
//! (`HttpServerConfig::timeout`, pi's `DEFAULT_TIMEOUT_SECONDS`). Measured, a child that
//! answers at 11 s was reported `connecting`, exited 1, and **never connected on any
//! later run** — because the budget expired before the handshake it was waiting for
//! could finish. On the failing job the same fixture took 7.5 s inside a single turn, so
//! this was not an exotic runner.
//!
//! ## What this pins
//!
//! Both halves of the fix, because either alone would let it come back:
//!
//! 1. **The number** — `list`'s window is at least the handshake default, and the module
//!    says why so a future reader cannot re-derive "10 looks about right".
//! 2. **The message** — a `connecting` row past the window carries the window that
//!    passed. That is what makes the *next* failure attributable; the behavioural half of
//!    the same ask is `a_failing_list_names_the_cause_not_just_the_exit_code` and
//!    `a_slow_but_healthy_server_is_not_called_dead` in `crates/lca-cli/tests/mcp_cli.rs`,
//!    which cannot live here (they drive the real binary).
//!
//! Deliberately not pinned: the exact value `60`. A larger window is a legitimate future
//! change and this guard should not veto it; what it forbids is going back *below* the
//! handshake default that made a healthy server look dead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

/// The window `lca mcp list` waits, as written in the source.
fn list_wait_seconds() -> u64 {
    let source = read("crates/lca-cli/src/mcp_cli.rs");
    let line = source
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("const LIST_WAIT_SECONDS: u64 =")
        })
        .unwrap_or_else(|| panic!("`LIST_WAIT_SECONDS` exists in the CLI"));
    line.split('=')
        .next_back()
        .and_then(|rest| rest.trim().trim_end_matches(';').parse::<u64>().ok())
        .unwrap_or_else(|| panic!("`LIST_WAIT_SECONDS` is a literal number: {line}"))
}

// Verifies: gh #582 - the list window is not the first-turn startup window.
//
// `DEFAULT_STARTUP_WAIT_MS` is a *turn's* budget: how long the interface waits before it
// answers the user, where answering with a partial tool list is the correct trade. A
// `list` is a question with no user waiting on a deadline, and its answer for a healthy
// but slow server is "connected in 11 s", not "dead".
#[test]
fn the_list_window_is_not_the_first_turn_startup_window() {
    let wait = list_wait_seconds();
    assert!(
        wait >= 60,
        "`lca mcp list` must not give a handshake less than the 60 s default it documents \
         (and that the HTTP transport uses); got {wait} s, which is the first-turn window \
         that made a healthy server look dead (gh #582)"
    );
    let source = read("crates/lca-cli/src/mcp_cli.rs");
    assert!(
        source.contains("DEFAULT_STARTUP_WAIT_MS"),
        "the module records where the old number came from, so it is not re-derived: a \
         constant with no explanation is how 10 came back"
    );
    assert!(
        source.contains("gh #582"),
        "and names the issue that measured it"
    );
}

// Verifies: gh #582 - the number agrees with the transport it is waiting on.
//
// A stdio handshake and an HTTP request are the same kind of wait, so two numbers there
// would be two answers to one question — which is exactly the shape of the bug (one
// number answering a different question).
#[test]
fn the_list_window_agrees_with_the_http_handshake_default() {
    let lib = read("crates/lca-mcp/src/lib.rs");
    let default = lib
        .split("pub fn timeout(&self)")
        .nth(1)
        .expect("`HttpServerConfig::timeout` exists")
        .lines()
        .find_map(|line| line.trim().strip_prefix("60"))
        .map(|_| 60u64)
        .unwrap_or_else(|| panic!("the HTTP default is still 60 s"));
    assert_eq!(
        list_wait_seconds(),
        default,
        "the stdio list window and the HTTP per-request default must be the same number, \
         because they bound the same kind of wait"
    );
}

// Verifies: gh #582 - a `connecting` row past the window says which window.
//
// This is the diagnosability half in the product: without it the report is a bare label
// that means both "nothing happened yet" and "this is never going to happen" (gh #459's
// own lesson, which this issue rediscovered from the outside).
#[test]
fn a_connecting_row_past_the_window_names_the_window() {
    let cli = read("crates/lca-mcp/src/cli.rs");
    let arm = cli
        .split("crate::ServerStateKind::Connecting =>")
        .nth(1)
        .expect("the connecting arm");
    let arm = arm
        .split("crate::ServerStateKind::NeedsSignIn")
        .next()
        .unwrap_or(arm);
    assert!(
        arm.contains("did not answer within {}s"),
        "the deadline that passed is in the message: {arm}"
    );
    // gh #582's cosmetic companion: the sentence used to carry a 34-space run from a
    // broken line continuation, so a reader saw the gap before the explanation.
    assert!(
        !arm.contains("still be                                  starting"),
        "no line-continuation artefact in the sentence a user reads: {arm}"
    );
}
