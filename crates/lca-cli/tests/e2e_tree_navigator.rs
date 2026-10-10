//! DAG navigator journeys (gh #231, FR-UI-16): fork connectors,
//! fold/unfold, filter cycling, and in-place branching in a real
//! terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

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
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // Two turns, then a rewind plus a third turn: the first row forks.
    session.send(&["alpha question", "Enter"]);
    session.wait_for("answer one", std::time::Duration::from_secs(25));
    session.send(&["beta question", "Enter"]);
    session.wait_for("answer two", std::time::Duration::from_secs(25));
    session.send(&["/tree", "Enter"]);
    session.wait_for("Message branches", std::time::Duration::from_secs(15));
    session.send(&["Enter"]);
    session.wait_for("branched at", std::time::Duration::from_secs(20));
    session.send(&["gamma question", "Enter"]);
    session.wait_for("answer three", std::time::Duration::from_secs(25));

    // The fork paints connectors with role markers. The needle is
    // picker chrome: the questions echo in the transcript too.
    session.send(&["/tree", "Enter"]);
    let pane = session.wait_for("Message branches", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("├─") && pane.contains("└─"),
        "the fork paints branch connectors:\n{pane:.2000}"
    );
    assert!(
        pane.contains("user: alpha question"),
        "role markers name the rows:\n{pane:.2000}"
    );

    // Left folds the first row's subtree (connectors leave with it);
    // Right restores them. Connectors never appear in transcript
    // text, so their absence is the fold receipt.
    session.send(&["Left"]);
    // Content-driven: the fold lands when the connectors leave.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let pane = loop {
        let pane = session.capture();
        if (!pane.contains("├─") && !pane.contains("└─")) || std::time::Instant::now() > deadline
        {
            break pane;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    assert!(
        !pane.contains("├─") && !pane.contains("└─"),
        "the folded subtree hides:\n{pane:.2000}"
    );
    session.send(&["Right"]);
    let pane = session.wait_for("└─", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("├─"),
        "unfold restores the subtree:\n{pane:.2000}"
    );

    // `f` cycles default -> no-tools -> user-only -> labeled-only.
    session.send(&["f"]);
    session.wait_for("(no-tools)", std::time::Duration::from_secs(10));
    session.send(&["f"]);
    session.wait_for("(user-only)", std::time::Duration::from_secs(10));
    session.send(&["f"]);
    let pane = session.wait_for("(labeled-only)", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("(no matches)"),
        "nothing is bookmarked yet:\n{pane:.2000}"
    );

    // Back to default, down to a row, Enter branches there in place.
    // Escape until the header leaves (a keypress can go missing
    // under parallel load; the close itself is instant). Bounded:
    // three tries, then the run fails loudly below.
    for _ in 0..3 {
        session.send(&["Escape"]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
    session.wait_for("Message branches", std::time::Duration::from_secs(15));
    session.send(&["Down", "Enter"]);
    session.wait_for("branched at", std::time::Duration::from_secs(20));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
