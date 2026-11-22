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

/// How often this row looks at the pane. It is also part of the close tolerance: the
/// row cannot observe a close sooner than its own next capture.
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(500);

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
    // Gh #608: one entry per *distinct* printed second, with the moment the pane
    // showed it. The timestamps are what let the clause below be a rate instead of a
    // span, so they are as load-bearing as the numbers.
    let mut paints: Vec<Paint> = Vec::new();
    // The harness's own resolution, measured rather than assumed: a capture that takes
    // three seconds cannot be evidence about a one-second promise, and the close
    // tolerance below is this number instead of a constant.
    let mut capture_costs: Vec<Duration> = Vec::new();
    while Instant::now() < deadline {
        let capture_started = Instant::now();
        let pane = session.capture();
        capture_costs.push(capture_started.elapsed());
        let elapsed = started.elapsed();
        if let Some(second) = countdown_second(&pane) {
            if first_paint.is_none() {
                first_paint = Some((elapsed, second));
            }
            if paints.last().map(|paint| paint.shown) != Some(second) {
                paints.push(Paint {
                    at: elapsed,
                    shown: second,
                });
            }
            last_shown = Some((elapsed, second));
            closed_at = None;
        } else if last_shown.is_some() && closed_at.is_none() {
            closed_at = Some(elapsed);
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let shown_series: Vec<u64> = paints.iter().map(|paint| paint.shown).collect();
    let capture_cost = median_capture(&mut capture_costs);

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
    // The paint keeps up - stated as a **rate**, not a span (gh #608).
    //
    // The clause this replaced asked the sampled series to span at least three printed
    // seconds, and it failed for a reason its own comment two lines above had already
    // named: that is a fact about the harness's capture cadence, not about the
    // countdown. Measured on a 4-core box shared with a build, the series was
    // `[29, 28, 27]` - three captures, two seconds of span - while the modal was
    // perfectly correct and closed on its announced second.
    //
    // What is true regardless of cadence, and what a broken countdown cannot fake:
    //   * it **advanced at least once** - a frozen clock shows one number forever;
    //   * every repaint shows a **strictly smaller** number than the last;
    //   * it never advanced **faster than real time** - between the first and last
    //     paint, the number may fall by no more than the wall time between them (+1
    //     for a 500 ms poll landing across a boundary). A clock that runs fast fails
    //     this; the old span clause could not see the difference.
    //
    // Both directions are exercised without a terminal in
    // `the_countdown_rate_gate_accepts_a_slow_capture_cadence` below, whose input is
    // the very series this row failed with.
    if let Some(violation) = countdown_rate_violation(&paints) {
        panic!(
            "{violation} - measured paints {paints:?}, capture cost median {capture_cost:?}, \
             elapsed {:?}",
            started.elapsed()
        );
    }
    // The effect. A promise kept means the modal CLOSED on its own and the tool
    // ran - no keypress was ever sent (this row sends only the initial prompt).
    let closed = closed_at.unwrap_or_else(|| {
        panic!(
            "the modal never closed: still on screen after {:?}, showing {:?}",
            deadline.saturating_duration_since(started),
            last_shown
        )
    });
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
             (modal closed at +{closed:?}, countdown series {shown_series:?}, capture cost {capture_cost:?})",
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
    // Gh #608: the close is stated against **this run's own facts**, not against an
    // absolute allowance. The product's promise is the marker's mtime (`ran_offset`,
    // asserted above and below); the modal being *gone* is the harness's stop condition,
    // and what it can honestly claim is: gone by the time the tool it released had run,
    // plus the row's own observation lag - one poll interval, the measured capture cost,
    // and one displayed second (the countdown's own granularity, since the frame that
    // clears the modal need not appear until the next printed second).
    //
    // Measured: idle the modal cleared at +30.3 s; at loadavg 42 it cleared at +31.72 s
    // while the series still ran 29..0 and the marker landed on the announced second -
    // a correct product whose close lag an absolute `countdown + 1.5 s` rejected.
    assert!(
        closed <= ran_offset + POLL_INTERVAL + capture_cost + Duration::from_secs(1),
        "the modal is gone by the time the tool it released has run: closed at \
         +{closed:?}, tool ran at +{ran_offset:?}, capture cost {capture_cost:?} \
         (series {shown_series:?}, elapsed {:?})",
        started.elapsed()
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

/// One distinct printed second, and when the pane showed it.
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct Paint {
    /// Since the prompt was sent.
    at: Duration,
    /// The `Ns` the modal printed.
    shown: u64,
}

/// The countdown's "the paint keeps up" rule as a **pure function of what the harness
/// saw** (gh #608).
///
/// `None` means the series is consistent with a correct countdown at *any* capture
/// cadence; `Some(reason)` names the first thing wrong with it. Being pure is the point:
/// the failing series from the issue (`[29, 28, 27]`, three captures over two seconds of
/// a one-second-per-second countdown) is red here under the old span rule and green under
/// this one, and that comparison is a unit row rather than a fight with a loaded box.
#[cfg(unix)]
fn countdown_rate_violation(paints: &[Paint]) -> Option<String> {
    if paints.len() < 2 {
        return Some(format!(
            "the countdown repainted at least once: only {} capture(s) saw a number: {paints:?}",
            paints.len()
        ));
    }
    for pair in paints.windows(2) {
        if pair[0].shown <= pair[1].shown {
            return Some(format!(
                "every repaint shows a smaller number than the last (a frozen clock \
                 repeats one): {paints:?}"
            ));
        }
    }
    // The rate: between the first and last paint the number may fall by no more than
    // the wall time between them, with one second of slack for a poll landing across a
    // boundary. This is what a fast clock fails and a slow box cannot.
    let first = paints[0];
    let last = paints[paints.len() - 1];
    let fell = first.shown.saturating_sub(last.shown) as f64;
    let elapsed = last.at.saturating_sub(first.at).as_secs_f64();
    if fell > elapsed + 1.0 {
        return Some(format!(
            "the printed second fell {fell} in {elapsed:.3}s of wall time, faster than \
             a clock can: {paints:?}"
        ));
    }
    None
}

/// The median capture cost of a run, or zero for an empty series.
#[cfg(unix)]
fn median_capture(costs: &mut [Duration]) -> Duration {
    if costs.is_empty() {
        return Duration::ZERO;
    }
    costs.sort_unstable();
    costs[costs.len() / 2]
}

// Verifies: gh #608 - **the failing series the issue measured passes the rate rule**,
// and it is the old span clause, not this one, that rejected it. The input is quoted
// from the issue verbatim: three captures, `[29, 28, 27]`, two seconds of span.
#[cfg(unix)]
#[test]
fn the_countdown_rate_gate_accepts_a_slow_capture_cadence() {
    let paints = [
        Paint {
            at: Duration::from_millis(700),
            shown: 29,
        },
        Paint {
            at: Duration::from_secs(10),
            shown: 28,
        },
        Paint {
            at: Duration::from_secs(20),
            shown: 27,
        },
    ];
    assert_eq!(
        countdown_rate_violation(&paints),
        None,
        "a slow cadence is not a defect"
    );
    // The clause this replaced, kept here as the negative control: a span bound of
    // three printed seconds rejects exactly the series above.
    let span = paints[0].shown - paints[paints.len() - 1].shown;
    assert!(
        span < 3,
        "the old span clause would have failed this run: span {span}"
    );
}

// Verifies: gh #608 - the rate rule fails the two directions it exists for: a **frozen**
// clock (one number forever) and a clock that **runs fast** (the number falls further
// than wall time allows).
#[cfg(unix)]
#[test]
fn the_countdown_rate_gate_rejects_a_frozen_or_fast_clock() {
    let frozen = [
        Paint {
            at: Duration::from_millis(700),
            shown: 29,
        },
        Paint {
            at: Duration::from_secs(10),
            shown: 29,
        },
        Paint {
            at: Duration::from_secs(20),
            shown: 29,
        },
    ];
    let reason = countdown_rate_violation(&frozen).expect("a frozen clock is caught");
    assert!(reason.contains("smaller number"), "{reason}");
    let single = [Paint {
        at: Duration::from_millis(700),
        shown: 29,
    }];
    assert!(
        countdown_rate_violation(&single).is_some(),
        "one capture proves no advance"
    );
    let fast = [
        Paint {
            at: Duration::from_millis(700),
            shown: 29,
        },
        Paint {
            at: Duration::from_secs(1),
            shown: 20,
        },
        Paint {
            at: Duration::from_secs(2),
            shown: 11,
        },
    ];
    let reason = countdown_rate_violation(&fast).expect("a fast clock is caught");
    assert!(reason.contains("faster than"), "{reason}");
    // And an *upward* jump, which monotonicity alone catches.
    let backwards = [
        Paint {
            at: Duration::from_millis(700),
            shown: 20,
        },
        Paint {
            at: Duration::from_secs(2),
            shown: 21,
        },
    ];
    assert!(
        countdown_rate_violation(&backwards).is_some(),
        "a rising number is caught"
    );
}
