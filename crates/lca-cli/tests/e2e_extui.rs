//! Extension UI receipts on a real terminal (gh #172): styled bytes
//! reach the pane, and a synthesized button click names its widget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Install the conformance extension (its footer carries the 0.6
/// vocabulary page, its panel the clickable button).
#[cfg(unix)]
fn install_conformance(box_: &Sandbox) {
    box_.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );
}

// Verifies: gh #172 pillar 1 on a real pane - the footer StyledText
// reaches the terminal with its hex bytes on the frame.
#[cfg(unix)]
#[test]
fn styled_extension_bytes_reach_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("extui-styled");
    install_conformance(&sandbox);
    let session = Tmux::new("extui-styled");
    session.spawn(&sandbox, None, false, &[], &[]);
    // Wall-clock margin, not a performance claim: debug WASM compiles
    // stack up when heavy tmux tests run together on few cores.
    // No `[session in …]` header: fresh sessions stay pending (gh
    // #122) and replay no session-start until the first record.
    session.wait_for("no model", std::time::Duration::from_secs(30));

    // The footer vocabulary page renders; its SGR survives to glass.
    session.wait_for("NOT A PROMPT", std::time::Duration::from_secs(10));
    let framed = session.capture_e();
    assert!(
        framed.contains("38;2;80;250;123"),
        "the styled fg hex reaches the pane: {framed:?}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #172 pillar 4 on a real pane - SGR-1006 bytes at the
// panel button's cell arrive as `ClickWidget`, and the extension's
// notice proves it.
#[cfg(unix)]
#[test]
fn a_synthesized_click_names_the_panel_button() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("extui-click");
    install_conformance(&sandbox);
    let session = Tmux::new("extui-click");
    session.spawn(&sandbox, None, false, &[], &[]);
    // Wall-clock margin, not a performance claim: debug WASM compiles
    // stack up when heavy tmux tests run together on few cores.
    // No `[session in …]` header: fresh sessions stay pending (gh
    // #122) and replay no session-start until the first record.
    session.wait_for("no model", std::time::Duration::from_secs(30));

    // Fullscreen captures the mouse; the panel shows the button
    // (`alt+x` toggles it: `app.panel.toggle`).
    session.send(&["/fullscreen", "Enter"]);
    session.send(&["M-x"]);
    let pane = session.wait_for("OK", std::time::Duration::from_secs(10));
    // The button's cell, from the pane's own rows.
    let (col, row) = pane
        .lines()
        .enumerate()
        .find_map(|(row, line)| line.find("OK").map(|col| (col as u16, row as u16)))
        .expect("a visible button cell");
    // SGR-1006 press + release, 1-based cells.
    session.send_literal(&format!("\x1b[<0;{};{}M", col + 1, row + 1));
    session.send_literal(&format!("\x1b[<0;{};{}m", col + 1, row + 1));
    session.wait_for("clicked: ok", std::time::Duration::from_secs(10));

    // The open panel owns typed keys, so it closes first and `/exit`
    // reaches the editor.
    session.send(&["M-x"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
