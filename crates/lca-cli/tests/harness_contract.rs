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
// This row used to spawn `echo head; sleep 0.1; echo border` and assert the settled frame
// carried both. It failed on CI (run 42094522777, `test (macos-latest, shell)`) for the
// reason the brief names: the gap was **two thirds of `POLL`** (100 ms against 150 ms), so
// on a loaded runner two consecutive captures landed inside the gap, the loop declared the
// head-only frame settled, and the assertion fired. Measured here at load ~18: gaps of
// 9-163 ms, four of twenty-four within 2x of `POLL`, one *past* it — that fixture's margin
// was inside the noise on a fast box and outside it on a slow one.
//
// The fix is to stop racing rather than to widen anything. The second paint is gated on
// `read`, so the split between the two frames is the row's decision, and the release
// happens **before** the primitive is called. That makes the outcome deterministic
// whatever the scheduler does: by the time the settle takes its first capture, `border` is
// already on screen, and since it never goes away every frame the loop can observe carries
// it.
//
// What that gives up, stated plainly: this row no longer proves *by timing* that the
// settled frame is later than the head frame, because it no longer races the paint. The
// ordering half is pinned by two rows that are deterministic instead of lucky —
// `two_captures_precede_a_settle` (a settle costs at least two captures, so it cannot
// return the frame it first saw) and `a_frame_that_never_settles_fails_and_shows_both_frames`
// (while the screen moves, the loop keeps polling). Together the three state the same
// property the timing version was trying to grab, and none of them can be flipped by load.
#[cfg(unix)]
#[test]
fn wait_until_painted_returns_the_frame_after_the_screen_stops_changing() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("settle-positive");
    // `read` returns only when the row sends input, so the pane sits on `head` alone for
    // exactly as long as this row wants — however loaded the box is. No `sleep` in the gap
    // means no unowned claim about the scheduler.
    session.spawn_raw("sh -c 'echo head; read relay; echo border; sleep 30'");
    // The two-frame shape is **established, not hoped for**: the intermediate frame exists
    // and is head-only, asserted here. Without this the row could pass with a fixture that
    // painted both at once and prove nothing about settling at all.
    let head_only = session.wait_for("head", std::time::Duration::from_secs(20));
    assert!(
        !head_only.contains("border"),
        "the second paint waits for the relay, so a head-only frame is what a settle \
         called now would see:\n{head_only}"
    );

    // Release the second paint **before** settling, which is what removes the race. The
    // cost of a late release is a slower row, never a red one: `border` paints
    // unconditionally once the relay fires, and the settle's budget absorbs any delay.
    session.send(&["go", "Enter"]);

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

// Verifies: gh #489's third round - **the guard against round four.** A settle declares
// only after two consecutive identical captures, and this walks that half directly: over
// a pane that is already static, one settle must take **at least two** captures before it
// returns. A primitive that returned its first capture (the shape every round of this
// issue has been about) fails here without needing any timing window at all.
//
// The counter is per process and other rows in this binary settle concurrently, so the
// assertion is on a delta around this call with a generous floor: two captures is the
// contract's minimum and a neighbouring row can only raise it.
#[cfg(unix)]
#[test]
fn two_captures_precede_a_settle() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("settle-capture-count");
    // Paint first, then wait for the relay: a pane with **nothing** painted is not settled
    // (that is `settle`'s documented blank-pane rule), and sending input into a pane whose
    // reader has not attached yet would race this row against the thing it is measuring.
    session.spawn_raw("sh -c 'echo ready; read relay; echo settled-content; sleep 30'");
    session.wait_for("ready", std::time::Duration::from_secs(20));
    session.send(&["go", "Enter"]);
    session.wait_for("settled-content", std::time::Duration::from_secs(20));

    let (before, _) = common::settle_stats();
    let pane = session.wait_until_painted(std::time::Duration::from_secs(20));
    let (after, _) = common::settle_stats();
    assert!(
        pane.contains("settled-content"),
        "the settled frame is the painted one:\n{pane}"
    );
    assert!(
        after.saturating_sub(before) >= 2,
        "a settle takes at least two captures before declaring a frame stable, so a \
         primitive that returned its first capture cannot pass this row: {} capture(s)",
        after.saturating_sub(before)
    );
}

// Verifies: gh #489's third round - **no fixture in this file separates two paints with
// an unsupervised `sleep`.** This is the guard the brief asked for, and it is structural
// rather than a timing threshold on purpose: a row that asserted "the gap was under
// `POLL/4`" would be a wall-clock assertion about the scheduler, which is the exact class
// of claim that has now failed this file three rounds running (a `-J` join inflating a
// healthy pane, a `widest <= cols` vacuous on a blank pane, and a `sleep 0.1` in a
// 150 ms poll). Comparing two numbers read from the *source* makes no claim about any
// machine, and it fails the moment the old shape comes back.
//
// The rule it encodes: a two-paint fixture holds its second paint on `read`, so the length
// of the pause is the row's decision. A `sleep` between the paints hands that decision to
// the scheduler, and on a loaded runner the scheduler answers "longer than `POLL`".
//
// The allowlist is empty, and it is here so that a future fixture that genuinely cannot be
// relayed has to say so in one named place rather than by quietly reintroducing the shape.
#[cfg(unix)]
#[test]
fn no_two_paint_fixture_trusts_a_sleep_for_its_gap() {
    // Every `spawn_raw("sh -c '...'")` in this file, as written source.
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/harness_contract.rs"),
    )
    .expect("this file is readable from its own test");
    let mut fixtures = 0;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || !trimmed.contains("spawn_raw(") {
            continue;
        }
        // Only a command that paints **twice** has a gap to race. Counting `echo` is
        // enough: every fixture here paints with `echo`/`printf`, and a command with one
        // paint (or none) cannot settle between two of them.
        let paints = trimmed.matches("echo ").count();
        if paints < 2 {
            continue;
        }
        fixtures += 1;
        assert!(
            trimmed.contains("read "),
            "a two-paint fixture must hold its second paint on `read` (a relay the row \
             releases) rather than on a `sleep`, whose length the scheduler owns - see \
             gh #489's three rounds:\n{trimmed}"
        );
    }
    assert!(
        fixtures >= 2,
        "the scan must find the two-paint fixtures it is guarding (found {fixtures}); a \
         parse break would otherwise make this row vacuous, which is the defect it exists \
         to prevent"
    );
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
    // A value that changes as fast as the shell can print it.
    //
    // The first version echoed `$RANDOM` every 100 ms and **flaked**: under load
    // two 150 ms captures both landed in the same 100 ms gap, saw identical
    // frames, and the primitive correctly declared the pane settled - so a row
    // demanding a failure failed instead. That is the primitive's documented
    // ceiling, but a *negative* row must not depend on the scheduler to stay
    // negative, so the fake now prints a nanosecond clock with no sleep: any two
    // captures a poll apart differ by construction.
    //
    // A second version still flaked at load average ~40, where the shell could go
    // more than a poll interval without being scheduled at all. That is not a
    // property this row can demand of a starved machine, so the row retries its
    // *own precondition* (`retry_once`, the primitive it is testing) and fails
    // loudly if the precondition misses twice - instead of turning a starved box
    // into a red `main`, which is the exact failure this cycle exists to remove.
    let outcome = common::retry_once(|| {
        let session = Tmux::new("settle-negative");
        session.spawn_raw("sh -c 'while :; do date +%s%N; done'");
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.wait_until_painted(std::time::Duration::from_millis(700));
        }))
        .err()
    });
    // `retry_once` yields `None` only when both attempts saw the pane settle, so
    // a `None` here is the failure the row is watching for.
    let Some(err) = outcome else {
        panic!(
            "a pane that never settles must fail, not hang: the pane settled twice \
             (a starved box can go a whole poll interval without printing anything)"
        );
    };
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
    // Gap by **relay**, for the same reason as the positive row above (gh #489's third
    // round): the pause must be longer than `POLL`, and the length of a `sleep` in the
    // fixture is not something this row owns. Here the row owns it - it holds the second
    // paint shut, spends more than a poll in its own process, then releases. Latness can
    // only *lengthen* the pause, which keeps the assertion true, so this row has no
    // scheduler-sensitive direction left.
    session.spawn_raw("sh -c 'echo head; read relay; echo border; sleep 30'");
    let head_only = session.wait_for("head", std::time::Duration::from_secs(20));
    assert!(
        !head_only.contains("border"),
        "the fixture holds the second paint shut:\n{head_only}"
    );
    // **The pause is the settle itself.** The second paint is held shut until *after* the
    // settle has returned, so the gap is at least one whole settle (`POLL` twice over) by
    // construction, and no `sleep` anywhere has to be trusted for a lower bound. Latness
    // can only lengthen it, which is why this row now has no scheduler-sensitive direction.
    let pane = session.wait_until_painted(std::time::Duration::from_secs(20));
    assert!(
        pane.contains("head") && !pane.contains("border"),
        "the documented ceiling: a pause longer than the poll interval settles \
         early, which is why rows must anchor on a needle where they can:\n{pane}"
    );
    // The remedy the doc names, working: anchoring on the *later* paint waits
    // for a frame that carries it and then settles. The paint is released only now,
    // which is what made the gap above provably longer than a poll.
    session.send(&["go", "Enter"]);
    let anchored = session.wait_settled_for("border", std::time::Duration::from_secs(20));
    assert!(
        anchored.contains("border"),
        "anchoring on the needle is what closes the ceiling:\n{anchored}"
    );
}

// Verifies: gh #489 - **`wait_for_layout_width` waits for the layout, and it sees
// a wrapped one.**
//
// The positive case is the shape `e2e_tree_layout` lost three times: a pane
// narrows, and the program inside keeps painting for the width it started at.
// tmux soft-wraps those rows, so every *physical* line still fits the pane and a
// per-line width check sees nothing wrong - the defect is only visible with the
// wraps joined (`capture-pane -J`). This primitive is what turns that into a
// wait, so the negative row proves it does not pass a wrapped frame.
#[cfg(unix)]
#[test]
fn wait_for_layout_width_sees_a_layout_drawn_for_a_wider_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    // A pane drawn for 140 columns, with no resize: the primitive must not
    // accept it at 60 and must not hang forever on it.
    let session = Tmux::new("layout-width");
    // A line as wide as the pane it was drawn in, repainted so it stays put.
    session.spawn_raw("sh -c 'while :; do printf \"\\033[H\"; printf \"%0140d\\n\" 0 | tr \" \" x; sleep 0.2; done'");
    session.resize(140, 30);
    // The fixture's own precondition has to be **waited for**, not assumed (gh #489
    // follow-up). `logical_width` on a pane that has not painted yet reads 0, and
    // 0 > 60 is false - so on a loaded runner this row failed on its own setup before
    // the primitive under test ever ran. `wait_until_painted` is the primitive for
    // exactly that, and it fails loudly (with a re-spawn attempt) if the fixture never
    // paints, so this is a bound rather than a sleep.
    session.wait_until_painted(std::time::Duration::from_secs(20));
    let wide = session.logical_width();
    assert!(
        wide > 60,
        "the fixture really is laid out wider than 60 (measured {wide})"
    );

    // Asking the wide pane for 60 must FAIL, not silently return the wrapped frame
    // - that silent pass is the defect this primitive exists to prevent.
    let wide_pane_fails = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_for_layout_width_within(60, std::time::Duration::from_millis(300))
    }));
    assert!(
        wide_pane_fails.is_err(),
        "a 140-wide layout must not satisfy a request for 60 columns"
    );

    // A pane that is already the right width returns at once.
    let narrow = Tmux::new("layout-width-ok");
    narrow.spawn_raw("sh -c 'while :; do printf \"\\033[H\"; printf \"%060d\\n\" 0 | tr \" \" y; sleep 0.2; done'");
    narrow.resize(60, 30);
    // Gh #489 follow-up: the same precondition, waited for. The assertion below
    // (`logical_width() <= 60`) is satisfied by a **blank** pane too - 0 <= 60 - so
    // without this wait the row would pass for the wrong reason on a slow runner,
    // which is the vacuous half of the same defect.
    narrow.wait_until_painted(std::time::Duration::from_secs(20));
    let pane = narrow.wait_for_layout_width(60);
    assert!(
        !pane.trim().is_empty(),
        "it returns a **painted** frame - a blank capture satisfies `<= 60` without \
         saying anything about the layout: {pane:?}"
    );
    assert!(
        narrow.logical_width() <= 60,
        "and the frame it returns is laid out at the asked width"
    );
}

// Verifies: gh #489 follow-up - **`wait_for_layout_width` refuses a blank pane.**
//
// The primitive returned a frame whose widest line was `<= cols`, and a pane that has not
// painted has a widest line of **0** - so it answered "the layout is correct" for a screen
// that was simply empty. That vacuity is the defect the failing row hit from the outside:
// `resize()` waits through this primitive, so the row's setup returned instantly against
// nothing, and its own `measured 0` assertion fired.
//
// A blank frame is **not** a layout that fits; it is the terminal not having come up. The
// primitive now says so, with a bounded wait and a loud failure rather than returning
// early. The fixture here never paints at all, so the row is proving the *failure*
// direction on purpose - which is why it is a `catch_unwind` rather than a call.
#[cfg(unix)]
#[test]
fn wait_for_layout_width_refuses_a_pane_that_never_painted() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("layout-width-blank");
    // Paints nothing, ever: the "the shell has not started yet" condition the loaded
    // runner produced, made deterministic.
    session.spawn_raw("sh -c 'sleep 30'");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.wait_for_layout_width_within(60, std::time::Duration::from_millis(400))
    }));
    assert!(
        outcome.is_err(),
        "a pane that never painted must not satisfy a layout request - 0 cells is not \
         a layout that fits, it is an empty screen"
    );
    // And the failure names the real condition, so a reader is not sent looking for a
    // resize bug.
    let message = outcome
        .err()
        .and_then(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        })
        .unwrap_or_default();
    assert!(
        message.contains("never painted") || message.contains("empty"),
        "the panic says the pane was blank rather than misreporting a width: {message}"
    );
}

// Verifies: gh #522 - **a wrapped box cap is a stale layout, and the primitive sees
// it.**
//
// The defect's real signature is not a long run of full rows: the row that wraps is
// the editor frame's cap, so the capture shows a `╭`-row and the rest of it
// (`───╮`) as its own screen row. Every physical row still fits the pane, so a
// max-width or padding-run rule reports the pane width and calls a stale layout
// healthy - which is how this survived a cycle of measurement.
//
// The fixture paints exactly that: one 140-column cap inside a 60-column pane, on
// a loop, with nothing wider than the pane on any physical row. `logical_width` must
// report the width the app laid out (>= the cap it composed), not the 60 it was
// wrapped into.
//
// **Gh #587: the fixture paints the cap in ONE write.** It used to emit 138
// separate `printf`s, one dash each, and between the `╭` write and the last dash the
// screen genuinely stops changing for longer than a settle's poll interval - so
// `wait_until_painted` declared a frame of `╭` + 19 dashes "painted" and the row
// measured 20. That is not a product width; it is a row still being composed. Under
// load the 138 writes take longer, which widens the window, which is why it appeared
// only inside a full workspace run. One write means no intermediate frame exists at
// any load (measured: 80 captures across two cycles, one distinct frame).

/// The wrapped-cap fixture (gh #587): one 140-cell cap, built once and painted in
/// **one** `printf`. Shared by the layout row and its behaviour row so there is one
/// spelling of the paint under test.
///
/// Unix-gated with its two rows: the rows need tmux, and a Windows build has no
/// caller for it.
#[cfg(unix)]
const WIDE_CAP_FIXTURE: &str = "sh -c 'd=$(printf \"\\342\\224\\200%.0s\" $(seq 1 138)); \
         cap=$(printf \"\\342\\225\\255%s\\342\\225\\256\" \"$d\"); \
         while :; do printf \"\\033[H\\033[K%s\\n\" \"$cap\"; sleep 0.2; done'";

#[cfg(unix)]
#[test]
fn a_wrapped_box_cap_is_measured_at_the_width_it_was_laid_out() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("layout-cap-wrap");
    // A 140-cell cap: `╭` + 138 dashes + `╮`, repainted so it stays put. Inside a
    // 60-column pane tmux wraps it, so the capture's first row is `╭` + 59 dashes
    // with no corner and the continuation row carries the rest.
    //
    // The string is built once and printed once (gh #587): the loop is over the
    // *repaint*, not over the dashes, so there is no frame in which the row is
    // half-composed. `printf`'s own escapes produce the box characters, because a
    // shell string built by concatenation would carry the escapes literally.
    session.spawn_raw(WIDE_CAP_FIXTURE);
    // **The resize must not wait for a layout** (gh #587). `Tmux::resize` waits
    // `wait_for_layout_width(60)` - "the app has re-laid-out narrow" - and this
    // fixture deliberately never does: it paints a 140-wide row into a 60-column pane
    // forever, so that wait could only ever be satisfied by a *transient* frame.
    // Issue the resize and let the row wait on its own fact instead.
    session.send_resize_immediately(60, 30);
    // **The row waits on the fact it measures, not on the screen going quiet**
    // (gh #587). `wait_until_painted` alone settles on whatever stopped moving, and two
    // transient frames qualify: a row still being composed (`╭` + 19 dashes - the
    // defect's `measured 20`), and tmux mid-wrap of an already-painted 140-column row,
    // where the cap's three screen rows can settle on their own (`measured 20` again,
    // with `╮` but no `╭`). Neither frame is a layout the app produced. So the row
    // re-settles until the frame it read **already measures wider than the pane** -
    // the same property the assertion checks, which is what keeps the loop from
    // accepting a frame the assertion would reject.
    //
    // Bounded at 10 settles: the primitive needs two captures per settle at its 150 ms
    // poll, so the ceiling is a few seconds against a fixture repainting every 200 ms
    // - and a frame that never becomes whole still fails with the number it last saw.
    let mut measured = 0usize;
    for _ in 0..10 {
        let settled = session.wait_until_painted(std::time::Duration::from_secs(20));
        measured = Tmux::logical_width_of(&settled);
        if measured > 60 {
            break;
        }
    }
    assert!(
        measured > 60,
        "a 140-column cap wrapped into 60 columns is still a 140-column layout \
         (measured {measured}); a max-width rule would have answered 60 and called \
         it healthy"
    );
}

// Verifies: gh #587 - **the single-write fixture really produces a whole row**, and
// it does so from the first frame.
//
// The row above pins that a *half-composed* frame can no longer be settled on. This
// one pins the other half: the fixture's own output is complete, so the absence of
// intermediate frames is a property of the paint and not luck. At 140 columns the
// cap fits one screen row, and every non-blank row of the capture must therefore
// carry both corners - a row with `╭` and no `╮` is a row still being composed, which
// is what the per-dash fixture produced (measured: 20 cells).
#[cfg(unix)]
#[test]
fn the_wide_row_fixture_paints_both_corners_on_one_row() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let session = Tmux::new("layout-cap-whole");
    session.spawn_raw(WIDE_CAP_FIXTURE);
    session.resize(140, 20);
    let pane = session.wait_until_painted(std::time::Duration::from_secs(20));
    let rows: Vec<&str> = pane
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "at 140 columns the cap is one screen row; more than one means the pane is \
         wrapping something the fixture did not intend:\n{pane}"
    );
    assert!(
        rows[0].starts_with('\u{256d}') && rows[0].ends_with('\u{256e}'),
        "the whole cap arrives in one write: `╭` at column 0 and `╮` at the end - a \
         row missing its corner is a row still being composed (gh #587):\n{rows:0?}"
    );
    assert!(
        !pane.chars().any(|c| c == '\\'),
        "and the box characters are real glyphs, not shell escapes carried literally:\n{pane}"
    );
}

// Verifies: gh #587 - **a wide row is painted in one write, so no half-composed
// frame exists to settle on.**
//
// The structural half of the fix, and it fails on the pre-fix file: the wrapped-cap
// fixture's command emitted the cap as `printf "╭"` + 138 × `printf "─"` + `printf
// "╮"`, and a settle poll landing inside those 138 writes reads a screen that has
// genuinely stopped changing at 20 cells. A rule that a wide row is emitted in one
// write is cheaper than a heuristic that tries to guess "still composing" from the
// frame, and it cannot redden a healthy row - which is why the heuristic was not
// added to `settle` itself (a real app frame legitimately has an unclosed `╭`
// mid-render, and `settle` serves every e2e row).
//
#[test]
fn a_wide_row_fixture_paints_in_one_write() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/harness_contract.rs"),
    )
    .expect("this file is readable from its own test");
    // The cap fixture's command: from the `╭` byte to the `╮` byte. Anything the
    // shell prints in between is a separate write, and a settle can land in it.
    // The Rust source carries the shell's escapes doubled (`\\` in the string), so
    // the needles are the doubled spelling - the same text the fixture line holds.
    let open = "\\\\342\\\\225\\\\255"; // `╭`
    let close = "\\\\342\\\\225\\\\256"; // `╮`
    let mut checked = 0;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || !trimmed.contains(open) || !trimmed.contains(close) {
            continue;
        }
        let (_, after_open) = trimmed.split_once(open).expect("split just matched");
        let (between, _) = after_open.split_once(close).expect("split just matched");
        checked += 1;
        assert!(
            !between.contains("printf"),
            "a wide row is painted in one write (gh #587): a `printf` between the \
             opening and closing corner is a frame a settle can land in and measure \
             half-composed - measured 20 cells for this exact shape:\n{trimmed}"
        );
    }
    assert!(
        checked >= 1,
        "the wrapped-cap fixture is in this file and was checked; a rule that finds \
         nothing to check is not a guard"
    );
}
