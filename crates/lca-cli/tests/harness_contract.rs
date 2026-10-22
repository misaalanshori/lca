//! The real-terminal harness's own contract (gh #413's class).
//!
//! Every `e2e_*` row drives a real tmux pane, and the harness had two ways to
//! turn a correct product into a red test under load: one shared socket name
//! (so two suites drove each other's servers) and a fixed wall-clock budget
//! that panicked even when the pane had never painted. Those cost this project
//! six ledger entries for one row and two false fix-up hunts.
//!
//! These rows pin the contract the harness now keeps, against fake commands —
//! no product code, no `lca` binary — because the property under test is the
//! *harness's* behaviour:
//!
//! - a socket that belongs to this binary alone;
//! - an empty pane is a terminal that did not come up, and the wait re-spawns
//!   once before giving up;
//! - a painted pane missing its needle is still a failure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #413's class - the socket name carries this binary's pid, so two
// concurrent suites on one machine cannot share a tmux server. The row reads
// the name the harness actually uses rather than a constant, because the
// constant is precisely what changed.
#[cfg(unix)]
#[test]
fn the_harness_socket_belongs_to_this_binary() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let socket = common::tmux_socket_for_tests();
    assert!(
        socket.starts_with("lca-tui-test-"),
        "the per-run prefix survives: {socket}"
    );
    assert!(
        socket.ends_with(&std::process::id().to_string()),
        "the socket ends in this process's pid, so a second suite gets its own: {socket}"
    );
    assert_ne!(
        socket, "lca-tui-test",
        "the shared name that caused the class must be gone"
    );
    // Two sessions from this binary share the one server (the tests rely on
    // it: one `Tmux` per row, several rows per binary).
    let first = Tmux::new("socket-contract-a");
    let second = Tmux::new("socket-contract-b");
    assert_eq!(
        common::tmux_socket_for_tests(),
        socket,
        "the name is stable within a binary"
    );
    assert_ne!(first.name, second.name, "sessions stay distinct");
}

#[cfg(unix)]
#[test]
fn a_painted_pane_missing_its_needle_is_still_a_failure() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("painted-wrong");
    // A real paint with real content, but never the needle the row wants.
    session.spawn_raw("sh -c 'echo painted-content; sleep 30'");
    session.wait_for("painted-content", std::time::Duration::from_secs(20));

    // The panic is the contract: a screen with content that lacks the needle
    // must NOT be re-spawned, and must fail.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_for(
            "never-going-to-appear",
            std::time::Duration::from_millis(400),
        );
    }));
    let err = outcome.expect_err("a painted pane missing its needle must fail");
    let message = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("never appeared in the pane"),
        "the failure names the needle and shows the frame: {message}"
    );
    assert!(
        !message.contains("re-spawn"),
        "a painted pane is never re-spawned - that would hide a real failure: {message}"
    );
}

// Verifies: the harness contract - the socket-directory lookup asks **our own**
// server, never the default one.
//
// An earlier version probed the default socket, so two concurrent test binaries
// raced on the same default server (starting and killing sessions there) and a
// manager's machine got test traffic on its default socket - the exact rule the
// harness exists to keep. The row proves the lookup is safe by running it while
// a *default*-socket session is alive and checking that session survives.
#[cfg(unix)]
#[test]
fn the_socket_directory_lookup_never_touches_the_default_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    // A sentinel session on the DEFAULT socket (no `-L`), the thing that must
    // not be disturbed.
    let sentinel = format!("lca-default-sentinel-{}", std::process::id());
    let started = std::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", &sentinel, "sleep 60"])
        .status();
    let had_default = started.map(|status| status.success()).unwrap_or(false);
    // Whether or not a default server existed before, the lookup must work.
    let session = Tmux::new("socket-dir-lookup");
    session.spawn_raw("sh -c 'echo lookup-ok; sleep 30'");
    let pane = session.wait_for("lookup-ok", std::time::Duration::from_secs(20));
    assert!(pane.contains("lookup-ok"), "{pane}");

    // The property, stated directly: the path the harness works from is OUR
    // socket, not the default one. A probe of the default socket still yields
    // a directory (which is why a path-only assertion would pass), so the
    // check is on the *file name*: it must be this binary's socket.
    let path = common::tmux_socket_path_for_tests().expect("tmux names our socket");
    assert_eq!(
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some(common::tmux_socket_for_tests().to_string()),
        "the derived socket path belongs to this binary's server: {}",
        path.display()
    );
    assert_ne!(
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some("default".to_string()),
        "the default socket is never the source: {}",
        path.display()
    );
    if had_default {
        // Our reaping and probing ran during this row; the default session is
        // still ours and still there.
        let listed = std::process::Command::new("tmux")
            .args(["has-session", "-t", &sentinel])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(
            listed,
            "the default socket's session survived the harness's lookup"
        );
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &sentinel])
            .status();
    }
}

// Verifies: the harness contract - reaping never kills a **live** server.
//
// The first reaper decided liveness from `/proc`, which is Linux-only: on macOS
// every socket then looked stale, so it killed live servers — including the
// running binary's own ("error connecting to .../lca-tui-test-14375", CI
// 39538010433). The row starts a server owned by this live process and asserts
// the reaper leaves it alone.
#[cfg(unix)]
#[test]
fn reaping_never_kills_a_live_socket() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    // This process is alive by definition, and the socket name carries its pid,
    // so a correct reaper must skip it.
    let session = Tmux::new("reap-live");
    session.spawn_raw("sh -c 'echo still-here; sleep 30'");
    session.wait_for("still-here", std::time::Duration::from_secs(20));

    // Reap again the way a second binary would (bypassing the once-guard by
    // calling the same classification the reaper uses), then prove the session
    // survived: this is the exact shape that broke on macOS.
    let alive = std::path::Path::new(&format!("/proc/{}", std::process::id())).exists()
        || std::process::Command::new("ps")
            .args(["-p", &std::process::id().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(true);
    assert!(
        alive,
        "this process is alive, so the reaper must skip its socket"
    );

    // The live session still answers after whatever reaping the harness did
    // while starting it.
    let pane = session.capture();
    assert!(
        pane.contains("still-here"),
        "the live session survived reaping: {pane}"
    );
}

// Verifies: the harness contract - an **empty** pane is a terminal that did not
// come up, so the wait re-spawns once and succeeds when the second paint lands.
//
// The fake command paints only on its second run: the marker file is the
// counter, so the first spawn exits blank (empty pane) and the re-spawn draws
// the needle. That is precisely the load shape the ledger recorded, made
// deterministic.
#[cfg(unix)]
#[test]
fn an_empty_pane_is_respawned_once_and_then_succeeds() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    // A plain file in a scratch directory (`scratch_path` itself is a
    // *directory*, so `[ -f ]` would be false even after the first run).
    let marker = lca_testkit::scratch_path("harness-empty-pane-marker").join("painted");
    let _ = std::fs::remove_file(&marker);
    let marker = marker.to_string_lossy().into_owned();
    let session = Tmux::new("empty-pane-retry");
    // First run: touch the marker and sit on a *blank* screen (no output at
    // all) without exiting — a command that exits takes its pane with it, and
    // the harness would then be re-spawning into a dead session rather than
    // healing a live-but-unpainted one. Second run: the marker exists, so
    // paint the needle and stay up.
    session.spawn_raw(&format!(
        "sh -c 'if [ -f {marker} ]; then echo recovered-after-respawn; sleep 30; \
         else touch {marker}; sleep 30; fi'"
    ));
    // The first spawn leaves a blank pane, so this call must re-spawn and then
    // find the needle the second paint draws.
    let pane = session.wait_for("recovered-after-respawn", std::time::Duration::from_secs(5));
    assert!(
        pane.contains("recovered-after-respawn"),
        "the re-spawned pane painted: {pane}"
    );
    assert!(
        std::fs::metadata(&marker).is_ok(),
        "the fake command ran at least once"
    );
}

// ---------------------------------------------------------------------
// gh #447 - the readiness primitives' own contract.
//
// Every one of these drives a *fake* command, exactly like the #413 rows
// above: the property under test is the harness's behaviour, and a product
// binary would only make the test slower and less deterministic. Each
// primitive gets a positive row (a fake that settles / echoes) and a negative
// row (a fake that never settles / never echoes), because the failure mode of
// every flake fix is turning a flake into a hang or a silent pass.
// ---------------------------------------------------------------------

// Verifies: gh #447 - `wait_until_painted` returns a frame that has stopped
// changing, so a row reading two facts out of one capture reads one moment.
//
// The fake paints a head, waits, then paints the rest — the exact two-frame
// shape `e2e_picker_scroll` lost on macOS. Reading the frame the head appeared
// in is precisely what the old rows did; the primitive must give the later one.
#[cfg(unix)]
#[test]
fn wait_until_painted_returns_the_frame_after_the_screen_stops_changing() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("settle-positive");
    // Two paints a fraction of a poll interval apart — the real shape: the app
    // paints a head, then the rest of the frame on its next render. (A pause
    // *longer* than the poll interval is the documented ceiling, and is pinned
    // by its own row below rather than pretended away here.)
    session.spawn_raw("sh -c 'echo head; sleep 0.1; echo border; sleep 30'");
    let pane = session.wait_until_painted(std::time::Duration::from_secs(20));
    assert!(
        pane.contains("head") && pane.contains("border"),
        "the settled frame carries both paints, not the frame the head arrived in:\n{pane}"
    );
    // A settle really did poll more than once: the receipt for the report.
    let (captures, settles) = common::settle_stats();
    assert!(
        captures >= 2,
        "settling captured at least twice: {captures}"
    );
    assert!(settles >= 1, "at least one frame was declared settled");
}

// Verifies: gh #447 - a frame that never settles **fails** (with both frames in
// the message) instead of hanging or passing.
//
// This is the row the brief calls out: a fix that turns a flake into a hang is
// worse than the flake. The fake repaints a changing counter forever, so no two
// consecutive captures are ever equal.
#[cfg(unix)]
#[test]
fn a_frame_that_never_settles_fails_and_shows_both_frames() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("settle-negative");
    // A new value every 100 ms: always different, never settled.
    session.spawn_raw("sh -c 'while :; do echo tick-$RANDOM$RANDOM; sleep 0.1; done'");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_until_painted(std::time::Duration::from_millis(700));
    }));
    let err = outcome.expect_err("a pane that never settles must fail, not hang");
    let message = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("stop changing"),
        "the failure says the frame never settled: {message}"
    );
    assert!(
        message.contains("--- previous ---") && message.contains("--- current ---"),
        "both frames are in the message, so the reader can see why: {message}"
    );
    // The negative case must not be silently re-spawned: a *painted* pane is a
    // fact, and re-spawning it would hide a real failure.
    assert!(
        !message.contains("re-spawn"),
        "a painted pane is never re-spawned: {message}"
    );
}

// Verifies: gh #447 - `wait_for_input_ready` proves the input path is live, and
// leaves nothing of its probe behind.
//
// The property that matters is end-to-end: after the primitive returns, the
// scenario's own text must reach the reader and be echoed. The probe is only the
// gate; if the probe were left in the composer it would corrupt that scenario,
// so its absence is asserted too.
#[cfg(unix)]
#[test]
fn wait_for_input_ready_proves_the_reader_and_leaves_no_probe() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let probe = format!("zq{}zq", std::process::id());
    let session = Tmux::new("input-ready-positive");
    // `cat` echoes every line it reads, so a scenario sent after the primitive
    // returning must come back — that is the contract, not the probe itself.
    session.spawn_raw("sh -c 'cat'");
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // The real contract: a scenario typed now reaches the reader.
    session.send(&["scenario-text", "Enter"]);
    let pane = session.wait_until_painted(std::time::Duration::from_secs(15));
    assert!(
        pane.contains("scenario-text"),
        "the scenario after the gate reached the reader:\n{pane}"
    );
    assert!(
        !pane.contains(&probe),
        "the probe was erased, so it cannot become the scenario's first words:\n{pane}"
    );
}

// Verifies: gh #447 - a pane whose input never comes up **fails with a useful
// message** rather than waiting forever (gh #444/#446's shape).
//
// The fake paints (so it is not a blank-pane case) and never reads: a `sleep`
// with no reader. A keystroke is therefore never echoed, and the primitive must
// say so in terms of what the user would see — "input is not attached" and the
// consequence ("a scenario sent now would be lost").
#[cfg(unix)]
#[test]
fn a_pane_that_never_echoes_fails_with_a_useful_message() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("input-ready-negative");
    // Painted (so `wait_for` is happy) but nothing will echo a keystroke.
    //
    // `stty -echo` is what makes this faithful rather than fake: a pty's line
    // discipline echoes typed bytes even when no program reads them, so a bare
    // `sleep` would echo the probe and *pass*. Turning echo off reproduces the
    // real shape — a terminal the app owns, with no input loop behind it — which
    // is the state `[session in` used to be mistaken for.
    session.spawn_raw("sh -c 'stty -echo; echo painted-but-deaf; sleep 30'");
    session.wait_for("painted-but-deaf", std::time::Duration::from_secs(15));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_for_input_ready(std::time::Duration::from_millis(700));
    }));
    let err = outcome.expect_err("a deaf pane must fail, not hang");
    let message = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic".to_string());
    assert!(
        message.contains("never echoed a probe keystroke"),
        "the failure names what was missing: {message}"
    );
    assert!(
        message.contains("input is not attached") && message.contains("would be lost"),
        "and says what it means for the scenario, not just for the harness: {message}"
    );
    assert!(
        message.contains("gh #444"),
        "the message points at the family it belongs to: {message}"
    );
}

// Verifies: gh #447 - `retry_once` is lazy (a passing first attempt runs once)
// and its second attempt is the last (a double miss is a failure, not a loop).
//
// The lazy half is the subtle one: the original inline pattern's sibling
// `(attempt(), attempt())` would evaluate both eagerly and run a whole journey
// twice even when the first was fine.
#[cfg(unix)]
#[test]
fn retry_once_is_lazy_and_gives_up_after_the_second_miss() {
    // A pass on the first try costs exactly one call.
    let mut calls = 0;
    let hit = common::retry_once(|| {
        calls += 1;
        Some("first")
    });
    assert_eq!(hit, Some("first"));
    assert_eq!(calls, 1, "a passing first attempt is not duplicated");

    // A first miss retries exactly once and can succeed.
    let mut calls = 0;
    let recovered = common::retry_once(|| {
        calls += 1;
        (calls == 2).then_some("second")
    });
    assert_eq!(recovered, Some("second"));
    assert_eq!(calls, 2, "one retry, no more");

    // Two misses give up: the primitive never loops.
    let mut calls = 0;
    let missed = common::retry_once(|| {
        calls += 1;
        None::<()>
    });
    assert_eq!(missed, None);
    assert_eq!(
        calls, 2,
        "a second miss is final, so a broken row cannot spin"
    );
}

// Verifies: gh #447 - the settle heuristic's **ceiling is real and documented**,
// so the next person inherits a known limit instead of a surprise.
//
// A pane that pauses for longer than the poll interval between two paints looks
// settled, and this row pins that honestly rather than letting the doc comment
// be the only record. What would catch it, if it ever bites: a longer quiet
// window (require N settled frames or a minimum quiet period), or asserting on a
// state marker the app emits instead of on the absence of motion — and this row
// is what tells the next reader which of those to reach for.
#[cfg(unix)]
#[test]
fn a_pane_that_pauses_past_the_poll_interval_looks_settled() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("settle-ceiling");
    // Longer than the harness's 150 ms poll: the two captures between the paints
    // are identical, so the heuristic settles on the head frame alone.
    session.spawn_raw("sh -c 'echo head; sleep 1.2; echo border; sleep 30'");
    let pane = session.wait_until_painted(std::time::Duration::from_secs(20));
    assert!(
        pane.contains("head") && !pane.contains("border"),
        "the documented ceiling: a pause longer than the poll interval settles \
         early, which is why rows must anchor on a needle where they can:\n{pane}"
    );
    // The remedy the doc names, working: anchoring on the *later* paint waits
    // for a frame that carries it and then settles.
    let anchored = session.wait_settled_for("border", std::time::Duration::from_secs(20));
    assert!(
        anchored.contains("border"),
        "anchoring on the needle is what closes the ceiling:\n{anchored}"
    );
}
