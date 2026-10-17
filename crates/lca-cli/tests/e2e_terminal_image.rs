//! End-to-end terminal row for image headers (gh #358): a truncated
//! WebP attachment must render the placeholder, not kill the render path.
//! Split from `e2e_terminal.rs` for the workspace file ceiling (gate 11).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #358 - a truncated WebP header renders the placeholder in a
// real pane instead of panicking the render path. `image_dimensions` runs
// on every transcript render and resize, and a panic there is fatal, so
// the receipt is the pane staying alive and showing the attached stub.
#[cfg(unix)]
#[test]
fn a_truncated_webp_attachment_renders_a_placeholder_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("seen"))]));
    let sandbox = sandbox("tui-webp-truncated");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // 20 bytes carrying the RIFF/WEBP magic and a VP8X fourcc: the header
    // stops 10 bytes short of the canvas size the arm reads. Before the
    // fix this indexed out of bounds during the first render.
    let webp = sandbox.project().join("truncated.webp");
    let mut bytes = vec![0u8; 20];
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[8..12].copy_from_slice(b"WEBP");
    bytes[12..16].copy_from_slice(b"VP8X");
    std::fs::write(&webp, &bytes).expect("write webp");

    let session = Tmux::new("webp-truncated");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    let attach_command = format!("/attach {}", webp.display());
    session.send(&[attach_command.as_str(), "Enter"]);
    session.wait_for("attached", std::time::Duration::from_secs(15));
    session.send(&["look at this", "Enter"]);
    session.wait_for("seen", std::time::Duration::from_secs(25));
    // The pane survived the render (a panic would have killed the session).
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));

    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let text = std::fs::read_to_string(log).expect("read log");
    assert!(
        text.contains("[image attachment"),
        "the truncated image still staged as an attachment stub: {text}"
    );
}
