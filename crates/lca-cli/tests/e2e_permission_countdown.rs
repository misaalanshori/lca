//! Gh #592 - the permission modal's countdown is a **promise**, and a promise is
//! kept only if the effect lands without a keypress.
//!
//! The issue reported the modal still on screen at +28 s and +19 s. Classification
//! first (the numbers are in the cycle report): it is none of the three candidates.
//! The modal paints at +0.7 s showing `29s`, the painted second advances
//! 28, 26, 24 … 1, 0 on every capture, and it closes at **+30.4 to +30.7 s** with
//! the tool having run - so the paint keeps up and the decision fires on the
//! announced second. What the original observation saw was **two modals**: an
//! unconsented endpoint raises the net consent, whose own 30 s deadline expires
//! first, and the shell permission appears ~30 s *later* with its own 29 s
//! showing. Both are correct; the reader thought one modal had overrun.
//!
//! So the row asserts the effect, on a loaded box, with a turn in flight: nothing
//! is pressed, and within a budget outside the countdown the tool's own side effect
//! is on disk and the transcript shows it. A row that only asserted
//! `tick_permission` returned `true` would pass on a build where nothing ever
//! called it - which is exactly what the issue says it cannot prove.
//!
//! The countdown is **not** shortened and no budget is lengthened: the assertion
//! is that the effect lands inside `COUNTDOWN + slack`, where `COUNTDOWN` is the
//! product's own 30 s.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// The product's countdown (`run_turns.rs`: `now() + 30 s`, FR-UI-18). The row
/// reads it here rather than shortening it - a test that moves the deadline is a
/// test that stops testing the promise.
#[cfg(unix)]
const COUNTDOWN: Duration = Duration::from_secs(30);

// Verifies: gh #592 - **the effect**, not the paint: with no keypress, the
// permission modal's announced deadline is honoured and the tool actually runs.
//
// The observation method is the pane's: capture every ~0.5 s and record when the
// modal first paints, what second it shows, when it closes, and when the tool's
// side effect appears. That is what distinguishes "the promise was kept" from
// "the modal was still up" - the second reading is the one the issue got and it
// cannot tell the two apart without the effect.
#[cfg(unix)]
#[test]
fn an_untouched_permission_modal_runs_the_tool_at_its_announced_second() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    // The tool's command writes a file named after the moment it ran, so the
    // effect is observable off-screen: the row does not have to trust the
    // transcript's rendering to know the command executed.
    let sandbox = sandbox("perm-countdown-592");
    // An **absolute** marker path: a shell command's cwd is the session's
    // workspace, not necessarily the directory the row calls the project, so the
    // effect is made observable by naming the path the row itself will read.
    let marker = sandbox.project().join("countdown-marker");
    // The command must **not** be workspace-scoped: this project is trusted, and
    // ADR-0039 lets trust auto-approve a command the analyzer can prove stays
    // inside the workspace (`is_allowed` -> `trust.allow`), which would raise no
    // modal at all. A `$( )` substitution is one of the forms the analyzer
    // deliberately refuses to reason about (`shell::workspace_scoped`), so it is
    // the shape that reaches the prompt - and it still writes a marker whose
    // mtime says when it ran.
    let args = serde_json::json!({
        "command": format!("touch {} && echo $(date +%s)", marker.display())
    })
    .to_string();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", &args)),
        Reply::Sse(sse_text_with_usage("ran after the countdown", 20, 0)),
    ]));
    // Loopback is *covered* by the extension manifest (#294's `net-local`), so
    // this row raises exactly one modal: the shell permission. Two modals would
    // put a net consent in front of it and make the timing ambiguous - which is
    // the classification the issue needed and this row removes.
    sandbox.approve_loopback_net(serde_json::json!({}));
    // The modal only exists in `ask` mode: with no setting the shipped default
    // applies, and this row must not depend on what that default happens to be
    // (that is FR-PERM-4's knob, not this promise's). Written the way a user
    // would, into the user config the binary reads.
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "[permissions]\nmode = \"ask\"\n",
    )
    .expect("write the user config that opens the modal");
    let session = Tmux::new("perm-countdown");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("openai-compatible/test-model", Duration::from_secs(45));
    session.wait_for_input_ready(Duration::from_secs(20));

    let started = Instant::now();
    let started_wall = std::time::SystemTime::now();
    session.send(&["run the tool", "Enter"]);

    // Poll the pane the way the original observation did, recording the modal's
    // own printed second each time, so a frozen countdown is visible in the
    // failure message rather than inferred from a timeout.
    let mut first_paint: Option<(Duration, u64)> = None;
    let mut last_shown: Option<(Duration, u64)> = None;
    let mut closed_at: Option<Duration> = None;
    let deadline = started + COUNTDOWN + Duration::from_secs(45);
    let mut shown_series: Vec<u64> = Vec::new();
    while Instant::now() < deadline {
        let pane = session.capture();
        let elapsed = started.elapsed();
        if let Some(second) = countdown_second(&pane) {
            if first_paint.is_none() {
                first_paint = Some((elapsed, second));
            }
            if shown_series.last() != Some(&second) {
                shown_series.push(second);
            }
            last_shown = Some((elapsed, second));
            closed_at = None;
        } else if last_shown.is_some() && closed_at.is_none() {
            closed_at = Some(elapsed);
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let (painted_at, painted_second) = first_paint.unwrap_or_else(|| {
        panic!(
            "the permission modal never painted; pane was:\n{}",
            session.capture()
        )
    });
    assert!(
        painted_at < COUNTDOWN,
        "the modal paints before its deadline, showing {painted_second}s at +{painted_at:?}"
    );
    // The paint keeps up: the printed second must reach 0 before the modal leaves.
    assert!(
        shown_series.last() == Some(&0),
        "the countdown advances to `0s` rather than freezing: saw {shown_series:?}"
    );
    // The effect. A promise kept means the modal CLOSED on its own and the tool
    // ran - no keypress was ever sent (this row sends only the initial prompt).
    let closed = closed_at.unwrap_or_else(|| {
        panic!(
            "the modal never closed: still on screen after {:?}, showing {:?}",
            deadline.saturating_duration_since(started),
            last_shown
        )
    });
    assert!(
        closed <= COUNTDOWN + Duration::from_secs(2),
        "the modal closes at +{closed:?}, inside the announced {COUNTDOWN:?} \
         (measured paints: first {painted_at:?} at {painted_second}s, series {shown_series:?})"
    );

    // ...and the tool's own side effect is on disk, which is the fact the pane's
    // text can only reflect. Its **mtime** is when the command ran, so the row
    // measures the moment directly instead of trusting a rendered timestamp.
    //
    // **Wait for the marker; do not read it the instant the modal vanishes.** The
    // first version did the latter and CI's macOS leg caught it: the modal closed
    // at +30.33 s with the countdown series [29, …, 0] (the promise kept, exactly
    // as this row exists to prove), and the marker was not there yet - the
    // decision has to travel to the waiting worker and the tool then runs, which
    // is a second fact after the deadline rather than part of it. On Linux the
    // exec landed inside the last 500 ms poll window, so the immediate read
    // passed by luck. A bounded wait reads the *effect* rather than the race.
    let mut meta = None;
    while Instant::now() < deadline {
        match std::fs::metadata(&marker) {
            Ok(metadata) => {
                meta = Some(metadata);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let meta = meta.unwrap_or_else(|| {
        panic!(
            "the tool ran without a keypress; its marker is missing at {} \
             (modal closed at +{closed:?}, countdown series {shown_series:?})",
            marker.display()
        )
    });
    let ran_offset = meta
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(started_wall).ok())
        .unwrap_or(Duration::ZERO);
    assert!(
        ran_offset >= COUNTDOWN.saturating_sub(Duration::from_secs(1)),
        "the tool ran at +{ran_offset:?}: not before the announced second, which would \
         be a modal that does not actually wait"
    );
    assert!(
        ran_offset <= COUNTDOWN + Duration::from_secs(10),
        "and it ran at +{ran_offset:?}: the deadline is the promise, and the decision \
         then has to reach the waiting worker - but ten seconds of exec latency after \
         the modal closed would be the defect this row is about, not scheduler noise"
    );

    // The transcript shows it, so the turn was not merely unblocked but observed.
    let pane = session.wait_for("ran after the countdown", Duration::from_secs(45));
    assert!(
        pane.contains("countdown-marker") || pane.contains("shell touch"),
        "the tool card is painted:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(20));
}

/// The `Ns` the modal's auto-approve line currently shows, when one is up.
#[cfg(unix)]
fn countdown_second(pane: &str) -> Option<u64> {
    let start = pane.find("auto-approves in ")? + "auto-approves in ".len();
    let tail = &pane[start..];
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
