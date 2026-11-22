//! DAG navigator journeys (gh #231, FR-UI-16): fork connectors,
//! fold/unfold, filter cycling, and in-place branching in a real
//! terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// This row's own pace, measured as it runs (gh #608).
///
/// The row used to hand every wait a fixed budget (15-25 s) and rely on the harness's
/// load factor (capped at 4x) for the rest. Measured on this box, an ignored `done`
/// needed **more than 100 s** - exactly `25 s x 4` - so the budget was the binding
/// constraint, not the box's signal. Two things changed, both from that measurement:
///
/// * the first turn is the row's **health probe**. It costs [`IDLE_TURN`](Pace::IDLE_TURN)
///   on an idle box and every later budget is multiplied by the ratio (bounded by
///   [`MAX_SCALE`](Pace::MAX_SCALE)), so a box that is uniformly slow from the start gets
///   proportionally more time without any constant being widened for everybody;
/// * each step is bounded by [`STEP_CEILING`](Pace::STEP_CEILING) and the whole row by
///   [`ROW_DEADLINE`](Pace::ROW_DEADLINE), so a genuinely hung app still fails in bounded
///   time, and nextest's per-test ceiling (the override in `.config/nextest.toml`, set
///   from the same measurement) sits just above the row's own deadline so the row's
///   message wins.
///
/// **What this cannot do, measured rather than assumed**: the one failure reproduced in
/// this cycle had every prior step at ~1 s and then a `done` that never painted inside
/// the harness's 100 s. No signal this row can measure predicted that stall - the model
/// banner answered in 276 ms in that very run - so the fix is a ceiling above the
/// observed stall plus a bound on the total, not a predictor. The alternative candidate
/// (drive the factor from the gh #589 overshoot probe on Linux too) was measured and
/// rejected: at loadavg 44 the probe's min-of-three 20 ms sleeps overran by 117-210 us
/// (factor **1x**) while this row's own cost rose 1.8x, so it is a worse predictor than
/// the load average the harness already uses.
#[cfg(unix)]
struct Pace {
    started: std::time::Instant,
    scale: u32,
}

#[cfg(unix)]
impl Pace {
    /// What the row's first turn (send -> `done`) costs on an idle box. Measured
    /// 2026-11-23: 520 ms idle; 0.65-1.6 s at loadavg 30-49 (1.3-3x).
    const IDLE_TURN: std::time::Duration = std::time::Duration::from_millis(520);
    /// The bound on the health-probe ratio. A box more than four times slower than this
    /// one on the first turn is beyond what a row should absorb silently.
    const MAX_SCALE: u32 = 4;
    /// One step may not exceed this, whatever the factor and the scale say.
    const STEP_CEILING: std::time::Duration = std::time::Duration::from_secs(150);
    /// The row's own wall-clock ceiling, so a hung app fails with this row's message
    /// ("the pane never showed X") rather than nextest's kill.
    const ROW_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            scale: 1,
        }
    }

    /// The budget for a step whose base is `secs` seconds on an idle box.
    fn budget(&self, secs: u64) -> std::time::Duration {
        (std::time::Duration::from_secs(secs) * self.scale)
            .min(Self::STEP_CEILING)
            .min(self.remaining())
    }

    /// What is left of the row's deadline, never zero: a zero budget would turn the
    /// deadline into a `wait_for` that cannot look at the pane at all.
    fn remaining(&self) -> std::time::Duration {
        Self::ROW_DEADLINE
            .saturating_sub(self.started.elapsed())
            .max(std::time::Duration::from_millis(500))
    }

    /// Learn from the row's first turn.
    fn observe_turn(&mut self, took: std::time::Duration) {
        let ratio = took.as_secs_f64() / Self::IDLE_TURN.as_secs_f64();
        self.scale = (ratio.ceil() as u32).clamp(1, Self::MAX_SCALE);
        if std::env::var_os("LCA_STEP_STATS").is_some() {
            eprintln!(
                "tree navigator pace: first turn {took:?} against {:?} idle -> scale {}x",
                Self::IDLE_TURN,
                self.scale
            );
        }
    }
}

// Verifies: gh #231 - the DAG navigator in a real terminal: a fork
// paints branch connectors, Left folds a subtree (Right restores),
// `f` cycles the filter with the header naming it, and Enter on a
// row branches there in place.
#[cfg(unix)]
#[test]
fn tree_navigator_folds_filters_and_selects_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("answer one", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer two", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer three", 20, 0)),
    ]));
    let sandbox = sandbox("tree-navigator");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tree-navigator");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    let mut pace = Pace::new();
    session.wait_for("openai-compatible/test-model", pace.budget(20));
    session.wait_for_input_ready(pace.budget(15));

    // Two turns, then a rewind plus a third turn: the first row forks.
    // Each turn is awaited to `done`, not just first-delta: the tree
    // reads a quiescent log, never a mid-stream one (gh #231).
    //
    // Gh #608: this first turn doubles as the row's health probe - what it costs
    // against `Pace::IDLE_TURN` decides the budget of everything after it.
    let probe = std::time::Instant::now();
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", pace.budget(25));
    session.wait_for("done", pace.budget(25));
    pace.observe_turn(probe.elapsed());
    session.send(&["beta question", "Enter"]);
    session.wait_for("answer two", pace.budget(25));
    session.wait_for("done", pace.budget(25));
    session.send(&["/tree", "Enter"]);
    session.wait_for("Message branches", pace.budget(15));
    // Gh #248 anchors the cursor on the live leaf: ride Home to
    // the first row, so Enter branches at the root like before.
    // (A missing Home fails loudly at the fork asserts below.)
    session.send(&["Home", "Enter"]);
    session.wait_for("branched at", pace.budget(20));
    session.send(&["gamma question", "Enter"]);
    session.wait_for("answer three", pace.budget(25));
    session.wait_for("done", pace.budget(25));

    // The fork paints connectors with role markers. The needle is
    // picker chrome: the questions echo in the transcript too.
    session.send(&["/tree", "Enter"]);
    // gh #452 (the #447 family): this row waited for ONE needle ("Message
    // branches", the picker header) and then asserted TWO connectors. The
    // header is painted before the tree body, so the frame the wait returned
    // could legitimately hold only one of them - which is exactly how this row
    // failed on macOS while passing locally. Settle on a frame, then assert:
    // the assertion is unchanged and a frame still missing a connector fails.
    let pane = session.wait_settled_for("Message branches", pace.budget(15));
    // SANCTIONED CHANGE (gh #394): fix 4 replaced the connector's dash
    // cell with the fold glyph, so the pair paints as `├⊟`/`└⊟` - one
    // column narrower, never `├─`/`└─`. The intent (both connector
    // ends visible) is unchanged.
    assert!(
        pane.contains("\u{251c}") && pane.contains("\u{2514}"),
        "the fork paints branch connectors:\n{pane}"
    );
    assert!(
        pane.contains("user: alpha question"),
        "role markers name the rows:\n{pane}"
    );

    // Left folds the first row's subtree (connectors leave with it);
    // Right restores them. Home first: the cursor opens on the live
    // leaf now (gh #248), and Left there would climb, not fold.
    // Connectors never appear in transcript text, so their absence
    // is the fold receipt.
    session.send(&["Home"]);
    session.send(&["Left"]);
    // Content-driven: the fold lands when the connectors leave.
    let deadline = std::time::Instant::now() + pace.budget(10);
    let pane = loop {
        let pane = session.capture();
        if (!pane.contains('\u{251c}') && !pane.contains('\u{2514}'))
            || std::time::Instant::now() > deadline
        {
            break pane;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    assert!(
        !pane.contains('\u{251c}') && !pane.contains('\u{2514}'),
        "the folded subtree hides:\n{pane}"
    );
    session.send(&["Right"]);
    // Same one-needle-two-glyphs shape as gh #452: wait for a settled frame
    // that has the bottom connector, then assert the top one too.
    let pane = session.wait_settled_for("\u{2514}", pace.budget(10));
    assert!(
        pane.contains('\u{251c}'),
        "unfold restores the subtree:\n{pane}"
    );

    // `f` cycles default -> no-tools -> user-only -> labeled-only.
    session.send(&["f"]);
    session.wait_for("(no-tools)", pace.budget(10));
    session.send(&["f"]);
    session.wait_for("(user-only)", pace.budget(10));
    session.send(&["f"]);
    let pane = session.wait_settled_for("(labeled-only)", pace.budget(10));
    assert!(
        pane.contains("(no matches)"),
        "nothing is bookmarked yet:\n{pane}"
    );

    // Back to default, down to a row, Enter branches there in place.
    // Escape until the header leaves (a keypress can go missing
    // under parallel load; the close itself is instant). Bounded:
    // three tries, then the run fails loudly below.
    for _ in 0..3 {
        session.send(&["Escape"]);
        let deadline = std::time::Instant::now() + pace.budget(5);
        loop {
            if !session.capture().contains("Message branches") {
                break;
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if !session.capture().contains("Message branches") {
            break;
        }
    }
    session.send(&["/tree", "Enter"]);
    session.wait_for("Message branches", pace.budget(15));
    // Home first, as above: Down from the live leaf would clamp.
    session.send(&["Home", "Down", "Enter"]);
    session.wait_for("branched at", pace.budget(20));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), pace.budget(15));
}

// Verifies: gh #608 - **the row's pace is its own measurement**, and it stays bounded:
// an idle first turn keeps 1x, a three-times-slower one gives 3x, anything beyond four
// times clamps, and no budget can exceed the ceiling or the row's deadline. Pure
// arithmetic, so it proves the rule without depending on how fast this box is.
#[cfg(unix)]
#[test]
fn the_rows_pace_scales_from_its_own_measurement_and_stays_bounded() {
    use std::time::Duration;
    let mut pace = Pace::new();
    assert_eq!(pace.scale, 1, "a row starts at the idle budget");
    pace.observe_turn(Pace::IDLE_TURN);
    assert_eq!(pace.scale, 1, "an idle first turn keeps the base budgets");
    pace.observe_turn(Pace::IDLE_TURN * 3);
    assert_eq!(
        pace.scale, 3,
        "a three-times-slower box gets three times the budget"
    );
    pace.observe_turn(Pace::IDLE_TURN * 40);
    assert_eq!(
        pace.scale,
        Pace::MAX_SCALE,
        "and the ratio is clamped, not unbounded"
    );
    assert_eq!(
        pace.budget(100),
        Pace::STEP_CEILING,
        "one step is capped at the ceiling however slow the box (100 s x 4 would be 400)"
    );
    assert_eq!(
        pace.budget(25),
        std::time::Duration::from_secs(100),
        "and an ordinary step is the base times the scale, not the ceiling"
    );
    assert!(
        pace.budget(1) <= Pace::ROW_DEADLINE,
        "and no budget can outlive the row's deadline"
    );
    assert!(
        pace.remaining() >= Duration::from_millis(500),
        "the remaining budget never reaches zero, so a wait can still look at the pane"
    );
}

// Verifies: gh #248 - pi-styled `/tree` in a real terminal: role
// colors reach the pane as SGR, the live chain carries `• `, and
// the cursor opens on the live leaf (the latest prompt), not row 0.
#[cfg(unix)]
#[test]
fn tree_pi_styling_reaches_the_pane_with_cursor_on_live() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("answer one", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer two", 20, 0)),
    ]));
    let sandbox = sandbox("tree-pi-style");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tree-pi-style");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // Two turns, each awaited to `done`: the tree reads a quiescent
    // log, never a mid-stream one (gh #231).
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", std::time::Duration::from_secs(25));
    session.wait_for("done", std::time::Duration::from_secs(25));
    session.send(&["beta question", "Enter"]);
    session.wait_for("answer two", std::time::Duration::from_secs(25));
    session.wait_for("done", std::time::Duration::from_secs(25));

    session.send(&["/tree", "Enter"]);
    let pane = session.wait_for("Message branches", std::time::Duration::from_secs(15));
    assert!(pane.contains("• "), "the live chain marks:\n{pane}");
    // The cursor rides the live leaf: exactly one tree row carries
    // `>`, and it names the session's latest entry (pi's `realLeafId`
    // anchoring) - the last assistant reply here, not row 0.
    let cursor_rows: Vec<&str> = pane
        .lines()
        .filter(|line| line.contains("user:") || line.contains("assistant:"))
        .filter(|line| line.contains('>'))
        .collect();
    assert_eq!(cursor_rows.len(), 1, "one cursor row in the tree:\n{pane}");
    assert!(
        cursor_rows[0].contains("answer two"),
        "the cursor opens on the live leaf, not row 0:\n{pane}"
    );
    // Raw SGR receipt: a color escape opens ahead of the role text.
    let sgr = session.capture_e();
    let at = sgr.find("user:").expect("a role marker in SGR:\n{sgr}");
    assert!(
        sgr[at.saturating_sub(30)..at].contains("\x1b["),
        "role colors reach the pane"
    );
    session.send(&["Escape"]);
    // The modal must be gone before `/exit` types: otherwise the
    // picker eats the command (Enter would branch in place).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if !session.capture().contains("Message branches") {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("the tree modal never closed");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
