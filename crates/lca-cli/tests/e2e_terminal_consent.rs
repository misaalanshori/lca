//! Real-terminal rows for the capability consent prompt (gh #24), in the
//! §14 style: a tmux pane, with `capture-pane` receipts either side of
//! Enter.
//!
//! The unit half of the fix is `confirm_with` beside
//! `crates/lca-cli/src/ext.rs`, pinned by
//! `tests/regressions/gh24-consent-confirm-requires-enter.rs`. What only a
//! real terminal can show is the line discipline itself: the keystroke
//! echoes, the prompt does not move, and the answer is handed over only
//! when its line ends.
//!
//! Split out of `e2e_terminal.rs` so both files stay under the
//! workspace-wide 1,200-line ceiling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so the glob import is too: an ungated
// `use common::*` is an unused import on Windows, which clippy denies.
#[cfg(unix)]
use common::*;

// Verifies: gh #24 in a real terminal - `Allow these capabilities? [y/N]`
// fronts a security decision, so a bare `y` keystroke leaves it
// unanswered (the prompt persists, nothing is written) and Enter is what
// commits the line. This is the reported transcript, driven properly.
#[cfg(unix)]
#[test]
fn gh24_typing_y_without_enter_leaves_the_capability_prompt_unanswered() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("gh24-consent");
    // A local component path installs with no registry and no network
    // (FR-DIST-5), which is all this row needs.
    let fixture = sandbox.project().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture dir");
    let component = fixture.join("component.wasm");
    std::fs::write(&component, OPENAI_COMPONENT).expect("component");
    std::fs::write(fixture.join("extension.toml"), OPENAI_MANIFEST).expect("manifest");

    let session = Tmux::new("gh24");
    session.spawn(
        &sandbox,
        None,
        false,
        &[],
        &[
            "ext",
            "install",
            component.to_str().expect("utf-8 path"),
            // Hold the pane open after the CLI returns, so both receipts
            // can be read back before the shell exits.
            ";",
            "sleep",
            "60",
        ],
    );

    const PROMPT: &str = "Allow these capabilities? [y/N]";
    session.wait_for(PROMPT, std::time::Duration::from_secs(25));

    // Receipt 1: `y` with no Enter. The keystroke echoes and is still
    // sitting in the line buffer - the prompt is unanswered, the install
    // has not run, and nothing has been written.
    session.send(&["y"]);
    std::thread::sleep(std::time::Duration::from_secs(1));
    let before_enter = session.capture();
    assert!(
        before_enter.contains(PROMPT),
        "the prompt is still up after a bare y:\n{before_enter}"
    );
    assert!(
        before_enter.contains("[y/N] y"),
        "the keystroke echoed, unanswered:\n{before_enter}"
    );
    assert!(
        !before_enter.contains("installed openai-compatible"),
        "a bare y installed the extension:\n{before_enter}"
    );
    let lock = sandbox.extensions_root().join("lockfile.json");
    let before_lock = std::fs::read_to_string(&lock).unwrap_or_default();
    assert!(
        !before_lock.contains("openai-compatible"),
        "nothing was written before Enter:\n{before_lock}"
    );

    // Receipt 2: Enter ends the line, the consent is granted, and the
    // install proceeds exactly as the issue's transcript shows it should.
    session.send(&["Enter"]);
    session.wait_for(
        "installed openai-compatible",
        std::time::Duration::from_secs(25),
    );
    let after_lock = std::fs::read_to_string(&lock).expect("lockfile after Enter");
    assert!(after_lock.contains("openai-compatible"), "{after_lock}");
}
