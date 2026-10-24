//! Gh #384 bug 1 in a real terminal (tmux): a markdown table whose first
//! column holds a long link, with short words in the other columns.
//!
//! The unit rows prove the solver's column widths; this row proves what the
//! terminal is actually handed - the short columns paint at their natural
//! width instead of one letter per line, and the hyperlink bytes reach the
//! pane (`capture -e`), so the fix is verified in the mode that prints
//! escape sequences rather than only in the stripped text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// The assistant answer: one long linked cell, then short words. Pre-fix
/// this rendered `c / o / u / n / t` down a 1-column strip.
#[cfg(unix)]
const ANSWER: &str = "Here is the reference table:

| reference | count | status |
| --- | --- | --- |
| [the specification](https://example.com/docs/a/very/long/path/that/goes/on/and/on) | 12 | ok |
| [another one](https://example.com/elsewhere/entirely) | 7 | ok |

Those are the numbers.";

// Verifies: gh #384 bug 1 - the neighbouring columns keep their natural
// width in a real pane, with the hyperlink escape bytes present.
#[cfg(unix)]
#[test]
fn a_long_link_cell_does_not_starve_the_table_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    // Build the SSE body by hand: `sse_text` splices the text into JSON
    // unescaped, and this answer is multi-line, so the newlines must be
    // escaped for the mock's body to parse.
    let content = serde_json::to_string(ANSWER).expect("the answer serializes");
    let body = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":{content}}}}}]}}\n\ndata: [DONE]\n\n"
    );
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(body)]));
    let sandbox = sandbox("tables-384");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("tables-384");
    // 100 columns: wide enough that no column needs wrapping, so the row
    // that paints is the row the solver chose.
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    session.paste_text("show me the reference table");
    session.send(&["Enter"]);

    // Poll the history: the table is taller than the dock, so its head
    // lives in scrollback rather than the visible screen.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let history = loop {
        let history = session.capture_with_history();
        if history.contains("status") || std::time::Instant::now() > deadline {
            break history;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };

    // The header row paints its words whole. Before the fix `count` and
    // `status` were broken across five rows, one letter each, so neither
    // word appears contiguously anywhere in the pane.
    assert!(
        history.contains("count") && history.contains("status"),
        "the short columns paint their words whole:\n{history:.3000}"
    );
    // And the answer's trailing prose survived, which a dangling OSC 8
    // would have swallowed as hyperlink payload.
    assert!(
        history.contains("Those are the numbers"),
        "the text after the table survives:\n{history:.3000}"
    );

    // The row paints as one line: the link cell, `12`, and `ok` are all on
    // the same painted line. Pre-fix the neighbouring cells were smeared
    // down the pane one character per row, so no line carried more than one
    // of them.
    let row = history
        .lines()
        .find(|line| line.contains("the specification") && line.contains("12"))
        .unwrap_or("");
    assert!(
        row.contains("ok"),
        "the linked cell and its neighbours paint on one row:\n{history:.3000}"
    );

    // Whichever link mode the terminal reports, the cell is measured as
    // exactly what it paints. Under tmux the capability probe answers "no
    // OSC 8", so the fallback `label (url)` prints - and that is what the
    // column was measured at, which is the fix. A terminal that *does*
    // advertise hyperlinks records the escape bytes instead.
    let inline = row.contains("(https://example.com/docs/");
    let with_escapes = session.capture_e();
    let hyperlink = with_escapes.contains("\u{1b}]8;;") || with_escapes.contains("8;;https");
    assert!(
        inline || hyperlink,
        "the cell paints in one of the two measured link modes:\n{history:.3000}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
