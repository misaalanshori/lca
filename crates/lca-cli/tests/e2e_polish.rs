//! UI polish receipts on a real terminal (gh #225, #230, #235,
//! #237): composer clearing, hover-without-scroll, friendly settings
//! labels, and the drawer tab plus panel tint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Install the conformance extension (its panel carries the
/// clickable button, gh #172).
#[cfg(unix)]
fn install_conformance(box_: &Sandbox) {
    box_.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );
}

// Verifies: gh #225 - the first C-c clears composer text (the hint's
// promise); empty-armed C-c exits on the second tap.
#[cfg(unix)]
#[test]
fn ctrl_c_clears_text_then_exits() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("polish-clear");
    let session = Tmux::new("polish-clear");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));

    session.paste_text("scratch-text");
    session.wait_for("scratch-text", std::time::Duration::from_secs(10));
    session.send(&["C-c"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(
        !pane.contains("scratch-text"),
        "the first tap clears:\n{pane}"
    );
    // Still alive: arming takes a second tap, exiting a third.
    session.send(&["C-c"]);
    std::thread::sleep(std::time::Duration::from_millis(400));
    session.send(&["C-c"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #230 - hovering settings rows highlights in place:
// the `>` cursor follows the pointer with no scroll indicators.
#[cfg(unix)]
#[test]
fn hover_highlights_settings_without_scrolling() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("polish-hover");
    let session = Tmux::new("polish-hover");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));

    // Fullscreen captures the mouse; the tall selector opens.
    session.send(&["/fullscreen", "Enter"]);
    session.send(&["/settings", "Enter"]);
    session.wait_for("Editor padding", std::time::Duration::from_secs(15));
    // A target row's cell, from the pane's own rows (1-based for SGR).
    let pane = session.capture();
    let (col, row) = pane
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("Display mode")
                .map(|col| (col as u16, row as u16))
        })
        .expect("a visible settings row");
    assert!(
        pane.lines()
            .find(|line| line.contains("Display mode"))
            .is_some_and(|line| !line.contains("> Display mode")),
        "the cursor starts elsewhere:\n{pane}"
    );
    // SGR-1006 hover (motion, no button). Content-driven: the
    // highlight lands when the cursor row moves, never on a sleep.
    session.send_literal(&format!("\x1b[<35;{};{}M", col + 1, row + 1));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let pane = loop {
        let pane = session.capture();
        let follows = pane
            .lines()
            .find(|line| line.contains("Display mode"))
            .is_some_and(|line| line.contains("> Display mode"));
        if follows || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    assert!(
        pane.lines()
            .find(|line| line.contains("Display mode"))
            .is_some_and(|line| { line.contains("> Display mode") }),
        "the highlight follows the pointer:\n{pane}"
    );
    assert!(!pane.contains('▲'), "nothing scrolled:\n{pane}");
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #235 - the selector paints friendly labels with the
// highlighted row's help, never raw dotted keys.
#[cfg(unix)]
#[test]
fn settings_selector_shows_labels_and_help() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("polish-labels");
    let session = Tmux::new("polish-labels");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));

    session.send(&["/settings", "Enter"]);
    session.wait_for("Editor padding", std::time::Duration::from_secs(15));
    // The pinned hint (with the description) sits at the box
    // bottom: End brings the tail and the last row's help on screen.
    session.send(&["End"]);
    let pane = session.wait_for("Update check", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("Daily background version check"),
        "the help shows:\n{pane}"
    );
    assert!(
        !pane.contains("ui.editor_padding_x"),
        "raw keys hide:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #237 - the drawer tab shows on the margin and the open
// panel draws its border on a SidePanel tint (either scheme).
#[cfg(unix)]
#[test]
fn drawer_tab_and_panel_tint_reach_the_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("polish-drawer");
    install_conformance(&sandbox);
    let session = Tmux::new("polish-drawer");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(30));

    session.send(&["/fullscreen", "Enter"]);
    session.send(&["M-x"]);
    let pane = session.wait_for("OK", std::time::Duration::from_secs(15));
    assert!(
        pane.contains('◀') || pane.contains('▶'),
        "the tab shows:\n{pane}"
    );
    let framed = session.capture_e();
    assert!(framed.contains('│'), "the panel border draws");
    assert!(
        framed.contains("48;2;51;51;63") || framed.contains("48;2;226;226;234"),
        "the SidePanel tint reaches the pane"
    );
    session.send(&["M-x"]);
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
