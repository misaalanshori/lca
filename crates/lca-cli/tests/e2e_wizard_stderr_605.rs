//! Gh #605 in a real pane: opening the thinking wizard writes **nothing** to stderr.
//!
//! The defect was found by hand, not by a test, and this is the row that closes that
//! gap. `crates/lca-ui/src/chat_commands.rs` shipped
//! `eprintln!("OPEN_THINKING_WIZARD called")` from gh #397's cycle until gh #605, so every
//! `/settings` → `effort` → Enter wrote a stray line to the process's stderr - i.e. into the
//! terminal the interface is drawing on, under a normal (un-redirected) stderr. No frame
//! assertion could see it: the painted rows are identical either way, which is why it
//! survived three weeks.
//!
//! This row is the receipt, mechanised: spawn with `2>` a file, drive the same keystrokes,
//! and require the file to be **empty** once the wizard is on screen. It also asserts the
//! wizard really opened (`Step 1/`), because an empty stderr from a wizard that never ran is
//! the vacuous version of this row.
//!
//! The structural half - no print macro anywhere in `lca-ui`'s production code - is
//! `tests/regressions/605-no-print-in-lca-ui.rs`. This row is the one that would have caught
//! the *original* defect even if the guard's scope had been wrong.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #605 - the shipped wizard path writes nothing to stderr.
#[cfg(unix)]
#[test]
fn a_wizard_open_writes_nothing_to_stderr() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("wizard-stderr-605");
    // The stderr file this row asserts on, inside the sandbox so it is torn down with it.
    let stderr_path = sandbox.root.join("stderr.log");
    let stderr_arg = stderr_path.display().to_string();

    // The harness's own `spawn` builds the tested binary's command line; the redirection is
    // this row's only addition, and `spawn_raw` is the seam for exactly that (`e2e_settings`
    // uses it for nothing, the contract rows for fake commands).
    let session = Tmux::new("wizard-stderr-605");
    let command = format!(
        "cd {project} && HOME={home} USERPROFILE={home} XDG_DATA_HOME={data} \
         APPDATA={data} LOCALAPPDATA={data} XDG_CONFIG_HOME={config} \
         LCA_UPDATE_CHECK=false {bin} 2>{stderr}",
        project = sandbox.project().display(),
        home = sandbox.home.display(),
        data = sandbox.data.display(),
        config = sandbox.home.join(".config").display(),
        bin = env!("CARGO_BIN_EXE_lca"),
        stderr = stderr_arg,
    );
    session.spawn_raw(&command);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // The same three keystrokes the issue used: open `/settings`, filter to the thinking
    // row, and open its wizard.
    session.send(&["/settings", "Enter"]);
    session.wait_for("key = value [source]", std::time::Duration::from_secs(15));
    session.send(&["effort"]);
    session.wait_settled_for("Thinking effort", std::time::Duration::from_secs(15));
    session.send(&["Enter"]);
    // The wizard's own step header is what proves the code path ran: `open_thinking_wizard`
    // is the function the print lived in.
    let pane = session.wait_for(" Step 1/2", std::time::Duration::from_secs(15));

    // **The assertion.** A stray diagnostic is on the line the interface would have drawn
    // over; a real failure names the file's contents rather than only its length.
    let stderr = std::fs::read_to_string(&stderr_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", stderr_path.display()));
    assert!(
        stderr.is_empty(),
        "opening the thinking wizard wrote to stderr, which under an un-redirected stderr \
         is the terminal the interface is drawing on (gh #605):\n{stderr}\n\
         the pane it opened: {pane}"
    );

    // Leave the way a user would, closing **whatever depth the wizard reached** - `esc`
    // cancels the wizard and closes the selector, and a queued `Enter` can advance the
    // wizard to step 2, which is exactly what this row hit while being written. The loop is
    // bounded and reads the pane, so it closes as many layers as are really there instead of
    // guessing a count. It is deliberately *not* an assertion on the screen: this renderer
    // appends to the real scrollback, so an overlay's rows survive in the capture after it
    // has closed (the trap `e2e_settings_selector` records - "waiting for `╰` never fires").
    // The receipt this row needs is the dead pane, which is what follows.
    for _ in 0..4 {
        let pane = session.capture();
        let open = pane.contains(" Step 1/2")
            || pane.contains(" Step 2/2")
            || pane.contains("key = value [source]");
        if !open {
            break;
        }
        session.send(&["Escape"]);
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if session.capture().trim().is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let pane = session.capture();
            panic!(
                "the interface never exited; the pane's tail shows:\n{}",
                pane.lines().rev().take(12).collect::<Vec<_>>().join("\n")
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}
