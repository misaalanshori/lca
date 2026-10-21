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

// A card-body row in a real pane: the first whitespace-separated
// token is the bare output number (`seq` lines). `trim() == "40"`
// never holds on tmux: every row pads to the full width with the
// scrollbar glyph riding the end, so only the head is bare.
#[cfg(unix)]
fn body_number(line: &str) -> Option<u32> {
    if line.contains("extensions") {
        return None;
    }
    line.split_whitespace().next()?.parse::<u32>().ok()
}

// Verifies: gh #247 - a real terminal receipt: clicking a tool card's
// body collapses it, and the collapse keeps the card in view instead
// of throwing the viewport at the history's top.
#[cfg(unix)]
#[test]
fn tool_body_click_collapses_without_scroll_jump() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let filler: String = (1..=35)
        .map(|n| format!("filler-line-{n}"))
        .collect::<Vec<_>>()
        .join("\\n");
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{filler}\"}}}}]}}\n\ndata: [DONE]\n\n"
        )),
        Reply::Sse(sse_tool_call("shell", r#"{"command":"seq 1 40"}"#)),
        Reply::Sse(sse_text_with_usage("all done", 20, 0)),
    ]));
    let sandbox = sandbox("polish-toolclick");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let session = Tmux::new("polish-toolclick");
    session.spawn(&sandbox, Some(&mock), true, &[], &["--yolo"]);
    session.wait_for("YOLO MODE", std::time::Duration::from_secs(20));

    session.send(&["first filler turn", "Enter"]);
    session.wait_for("filler-line-35", std::time::Duration::from_secs(25));
    // True idle, never the `done`-in-`all done` substring: the status
    // line reads exactly `done` when the turn (including its tail
    // records) has settled, so later frames cannot shift rows under
    // the mouse.
    session.send(&["run the numbers", "Enter"]);
    session.wait_for("all done", std::time::Duration::from_secs(25));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let pane = session.capture();
        if pane.lines().any(|line| line.trim() == "done") || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        session.capture().lines().any(|line| line.trim() == "done"),
        "the turn settled before the mouse moves"
    );

    // Fullscreen captures the mouse; the collapsed card previews.
    // Gate on the fullscreen marker itself: the preview text also
    // renders in scrollback, where no mouse event can arrive.
    session.send(&["/fullscreen", "Enter"]);
    session.wait_for("app-owned screen", std::time::Duration::from_secs(15));
    session.wait_for("earlier lines", std::time::Duration::from_secs(15));
    // Expand through the header first. The 40-line body overflows the
    // pane, so the receipt is the full body reaching the scrollback
    // history - never a single viewport row, which the bottom-anchored
    // view keeps wherever the fold falls.
    // Converge to the expanded state from wherever the card is: a
    // late turn-finalization frame can flip the preview on its own,
    // so a missing header is recovered, never fatal. One click gets
    // a long poll window - re-clicking faster than the render lands
    // would collapse the card the first click just opened.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let expanded = loop {
        let pane = session.capture();
        let header = pane
            .lines()
            .enumerate()
            .find_map(|(row, line)| line.contains("seq 1 40").then_some(row));
        let marker = pane.contains("earlier lines");
        let body = pane.lines().any(|line| body_number(line).is_some());
        if !marker && body {
            break true;
        }
        if std::time::Instant::now() > deadline {
            break false;
        }
        match (header, marker) {
            // Collapsed and aimed: click once, then poll the tail
            // patiently before even thinking of another click.
            (Some(row), true) => {
                let line = pane.lines().nth(row).unwrap_or_default();
                let col = line.find("seq 1 40").unwrap_or(2) as u16 + 2;
                session.send_literal(&format!("\x1b[<0;{};{}M", col + 1, row as u16 + 1));
                session.send_literal(&format!("\x1b[<0;{};{}m", col + 1, row as u16 + 1));
                let inner = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let mut done = false;
                while std::time::Instant::now() < inner {
                    let pane = session.capture();
                    if !pane.contains("earlier lines")
                        && pane.lines().any(|line| body_number(line) == Some(40))
                    {
                        done = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                if done {
                    break true;
                }
            }
            // Header off-view: ease back toward the live bottom,
            // where the collapsed card previews.
            _ => {
                session.send_literal("\x1b[<65;70;20M");
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
    };
    let finale = session.capture();
    assert!(expanded, "the card expanded:\n{finale}");
    // Scroll up into the card: the collapse defect only bites away
    // from the live bottom (at scroll 0 old and new agree). Alt+wheel
    // climbs fast; the loop stops when the header shows above the
    // fold with body rows under it.
    let wheel = |bits: u16| {
        session.send_literal(&format!("\x1b[<{bits};70;20M"));
        std::thread::sleep(std::time::Duration::from_millis(120));
    };
    for _ in 0..4 {
        wheel(72);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let pane = loop {
        let pane = session.capture();
        let header = pane
            .lines()
            .enumerate()
            .find_map(|(row, line)| line.contains("seq 1 40").then_some(row));
        let body_below = header.is_some_and(|at| {
            pane.lines()
                .skip(at + 1)
                .any(|line| body_number(line).is_some())
        });
        if body_below || std::time::Instant::now() > deadline {
            break pane;
        }
        // Too far up: the header left through the top. Ease back.
        wheel(65);
        std::thread::sleep(std::time::Duration::from_millis(120));
    };
    let header_row = pane
        .lines()
        .enumerate()
        .find_map(|(row, line)| line.contains("seq 1 40").then_some(row))
        .expect("the header stays in view");
    // A body click collapses: the preview marker returns and the
    // clicked tail line leaves. Click the deepest visible body
    // line: the collapsed preview keeps lines 1-5, so only a tail
    // number proves the collapse.
    // Collapse through the body: re-aim fresh each round, since a
    // frame can land between the read and the click. One click gets
    // a patient poll: re-clicking faster than the render lands would
    // re-expand the card just collapsed. The preview marker's return
    // is the receipt.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut clicked = 0u32;
    // The collapse keeps the card in view: the pre-fix clamp threw
    // the viewport at the log's top, leaving the header below the
    // fold (the `expect` below is the no-jump receipt). Exact-row
    // stationarity rides the unit-pinned `anchor_scroll`, not tmux
    // timing.
    let pane = loop {
        let pane = session.capture();
        if pane.contains("earlier lines") || std::time::Instant::now() > deadline {
            break pane;
        }
        let live_header = pane
            .lines()
            .enumerate()
            .find_map(|(row, line)| line.contains("seq 1 40").then_some(row))
            .unwrap_or(header_row);
        if let Some((n, col, row)) = pane
            .lines()
            .enumerate()
            .skip(live_header + 1)
            .filter_map(|(row, line)| body_number(line).map(|n| (n, 6u16, row as u16)))
            .max_by_key(|(n, _, _)| *n)
        {
            clicked = n;
            session.send_literal(&format!("\x1b[<0;{};{}M", col + 1, row + 1));
            session.send_literal(&format!("\x1b[<0;{};{}m", col + 1, row + 1));
            let inner = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut done = false;
            while std::time::Instant::now() < inner {
                if session.capture().contains("earlier lines") {
                    done = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if done {
                break session.capture();
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    };
    assert!(
        pane.contains("earlier lines"),
        "the body click collapsed the card:\n{pane}"
    );
    // Gh #391 gap 1: the collapsed shell card keeps its **tail**, so a click
    // on a high body line can leave it visible. What proves the collapse is
    // that the *head* is folded away again (the preview shows 36..40, not
    // 1..5), plus the marker above it.
    assert!(
        !pane.lines().any(|line| body_number(line) == Some(1)),
        "the head is folded away by the collapse:\n{pane}"
    );
    let _ = clicked;
    pane.lines()
        .find(|line| line.contains("seq 1 40"))
        .expect("the header stays in view");
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
