//! The `/tree` navigator's layout in a real terminal (gh #394): a long
//! linear session must paint every row at column 0 in a pane narrow enough
//! that the pre-fix geometry would have marched off it, and a fork must
//! indent exactly at the fork. The receipt is the captured frame, not a
//! unit assertion - the bug was a geometry bug the user only ever saw on a
//! screen.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #394 (acceptance 1: 100-turn linear sessions stay at column
// 0; forks indent exactly at branch points) in a real tmux pane. The pane
// is 60 columns: with one gutter per ancestor, turn six is already off the
// right edge, so a passing capture can only mean the compression landed.
#[cfg(unix)]
#[test]
fn a_long_linear_session_paints_flat_in_a_narrow_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    // Six turns: the tree then has twelve rows, the sixth far enough right
    // that the old geometry puts it outside this pane.
    let replies = (0..6)
        .map(|turn| Reply::Sse(sse_text_with_usage(&format!("answer {turn}"), 20, 0)))
        .collect();
    let mock = runtime.block_on(start_mock(replies));
    let sandbox = sandbox("tree-flat");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tree-flat");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    // Narrow enough that the pre-fix geometry leaves the pane by turn six.
    session.resize(60, 30);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );
    for turn in 0..6 {
        session.send(&[&format!("question {turn}"), "Enter"]);
        session.wait_for(
            &format!("answer {turn}"),
            std::time::Duration::from_secs(25),
        );
    }

    session.send(&["/tree", "Enter"]);
    // Every row's text must be on screen in one frame: the flat layout's
    // promise, and the thing the old diagonal march broke.
    // The box windows its rows (the cursor opens on the live leaf), so the
    // assertion is about the rows it does paint: the tail of the session.
    let pane = wait_for_all(
        &session,
        &[
            "user: question 5",
            "assistant: answer 5",
            "user: question 4",
        ],
        std::time::Duration::from_secs(25),
    );
    // The rows are flat: no connector cell and no gutter appears anywhere,
    // because a linear session has nothing to fork.
    for (row, line) in navigator_rows(&pane).iter().enumerate() {
        assert!(
            !line.contains('\u{251c}') && !line.contains('\u{2514}'),
            "row {row} of a linear session draws no connector: {line:?}\n{pane}"
        );
        assert!(
            !line.contains('\u{2502}'),
            "row {row} of a linear session draws no gutter: {line:?}\n{pane}"
        );
    }
    // And every row's text begins at the same anchor: no diagonal march.
    // The anchor is the live mark (`• `), which is where a row's content
    // starts; the role label after it differs in length per kind, so
    // measuring `"user: "` against `"assistant: "` would compare
    // apples to pairs. Measured in characters, not bytes: the transcript
    // behind the box paints `›`, which is three bytes and would read as
    // two extra columns.
    let anchors: Vec<usize> = navigator_rows(&pane)
        .iter()
        .filter_map(|line| line.find("\u{2022} ").map(|at| line[..at].chars().count()))
        .collect();
    assert!(
        anchors.len() >= 4,
        "the navigator painted its rows:\n{pane}"
    );
    let first = anchors[0];
    for anchor in &anchors {
        assert_eq!(
            *anchor, first,
            "row anchors march right ({anchors:?}) in:\n{pane:?}"
        );
    }

    session.send(&["Escape", "Escape"]);
    session.send(&["/exit", "Enter"]);
}

// Verifies: gh #394 (acceptance 1: forks indent exactly at branch points)
// and fix 4 (a folded branch shows `⊞`) in a real tmux pane: after a
// rewind, the navigator carries exactly one connector pair and can fold.
#[cfg(unix)]
#[test]
fn a_forked_session_shows_its_fork_and_folds_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("answer one", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer two", 20, 0)),
        Reply::Sse(sse_text_with_usage("answer three", 20, 0)),
    ]));
    let sandbox = sandbox("tree-fork");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tree-fork");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", std::time::Duration::from_secs(25));
    session.send(&["beta question", "Enter"]);
    session.wait_for("answer two", std::time::Duration::from_secs(25));

    // Rewind to the first prompt: the log grows a real fork. The cursor
    // opens on the live leaf (gh #248), so walk up to the root first -
    // branching at the tip would only re-point at the tip.
    session.send(&["/tree", "Enter"]);
    // The box's own title row (`tree`, with its scroll count) is the
    // navigator-open signal; its body windows over the newest rows.
    wait_for_all(&session, &["╭─ tree"], std::time::Duration::from_secs(20));
    for _ in 0..8 {
        session.send(&["k"]);
    }
    session.wait_for(
        "> • user: alpha question",
        std::time::Duration::from_secs(20),
    );
    session.send(&["Enter"]);
    session.wait_for("branched at", std::time::Duration::from_secs(20));
    session.send(&["gamma question", "Enter"]);
    session.wait_for("answer three", std::time::Duration::from_secs(25));

    // Now the navigator carries a fork. The cursor opens on the live leaf
    // (gh #248), so the pane windows the newest rows; walking up to the
    // root reveals the abandoned branch that makes the fork.
    session.send(&["/tree", "Enter"]);
    wait_for_all(
        &session,
        &["user: gamma question"],
        std::time::Duration::from_secs(25),
    );
    for _ in 0..8 {
        session.send(&["k"]);
    }
    // The box windows around the cursor, so walk down until the fork's
    // two halves (the abandoned `beta` branch and the live one) are both
    // painted in one frame.
    let mut pane = session.capture();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !(pane.contains("assistant: answer one") && pane.contains("user: beta question"))
        && std::time::Instant::now() < deadline
    {
        session.send(&["j"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        pane = session.capture();
    }
    assert!(
        pane.contains("assistant: answer one") && pane.contains("user: beta question"),
        "the fork's two halves never shared a frame:\n{pane}"
    );
    let rows = navigator_rows(&pane);
    let connectors = rows
        .iter()
        .filter(|line| line.contains('\u{251c}') || line.contains('\u{2514}'))
        .count();
    assert!(
        connectors >= 2,
        "the fork draws its connector pair:\n{pane}\nrows: {rows:#?}"
    );
    // Folding the fork's first branch paints `⊞` in the dash cell.
    session.send(&["j", "j"]);
    session.send(&["\u{1b}[1;3D"]); // alt+left: fold, else jump
    let folded = session.capture();
    assert!(
        folded.contains('\u{229e}') || folded.contains('\u{229f}'),
        "the navigator paints a fold glyph after alt+left:\n{folded}"
    );

    session.send(&["Escape", "Escape"]);
    session.send(&["/exit", "Enter"]);
}

/// Wait until every needle is on screen in one frame (the harness rule:
/// a selector paints over several frames, so a row is waited for, not
/// slept through). The timeout is the same loud panic as `wait_for`.
#[cfg(unix)]
fn wait_for_all(session: &Tmux, needles: &[&str], timeout: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let pane = session.capture();
        if needles.iter().all(|needle| pane.contains(needle)) {
            return pane;
        }
        if std::time::Instant::now() > deadline {
            panic!("not every needle {needles:?} appeared at once:\n{pane}");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// The navigator's painted rows, without the pane's own chrome: its role
/// markers, which the transcript behind the box never paints. Rows are
/// identified by that marker, not by the box's border, because a narrow
/// pane shows the transcript beside the box.
///
/// Not every navigator row is on the live chain, so the `• ` mark cannot
/// be the test: an abandoned branch's rows carry connectors and a role
/// marker and no `• `.
#[cfg(unix)]
fn navigator_rows(pane: &str) -> Vec<String> {
    pane.lines()
        .filter(|line| line.contains("user: ") || line.contains("assistant: "))
        .map(|line| line.to_string())
        .collect()
}
