//! GitHub #96: the panic hook restores the terminal on every build
//! profile, including release `panic = "abort"` (where `Drop`-based
//! guards never run).
//!
//! The runtime calls the hook before unwinding or aborting, so the hook
//! — not the drop — is what makes a crash leave a usable terminal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Mutex, OnceLock};

fn recorder() -> &'static Mutex<Vec<u8>> {
    static RECORDER: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
    RECORDER.get_or_init(|| Mutex::new(Vec::new()))
}

fn record(bytes: &[u8]) {
    recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .extend_from_slice(bytes);
}

// Verifies: #96 (the hook emits the restore bytes for a synthetic panic:
// leave the alternate screen, cursor back on).
#[test]
fn gh96_panic_hook_emits_the_restore_bytes() {
    lca_tui::install_panic_hook_with(record);
    assert!(
        lca_tui::panic_hook_installed(),
        "installing records the installation"
    );
    let _ = std::panic::catch_unwind(|| panic!("gh96 synthetic crash"));
    let bytes = recorder()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("\x1b[?1049l"),
        "the alternate screen is left: {text:?}"
    );
    assert!(
        text.contains("\x1b[?25h"),
        "the cursor comes back on: {text:?}"
    );
    assert!(text.contains("\x1b[?7h"), "wrapping is restored: {text:?}");
}

// Verifies: #96 (the hook is installed on every entry path — the
// registration call exists in `main`, the TUI entry, and headless).
#[test]
fn gh96_hook_installation_covers_every_entry_path() {
    for (file, source) in [
        (
            "crates/lca-cli/src/main.rs",
            include_str!("../../crates/lca-cli/src/main.rs"),
        ),
        (
            "crates/lca-cli/src/tui/mod.rs",
            include_str!("../../crates/lca-cli/src/tui/mod.rs"),
        ),
        (
            "crates/lca-cli/src/headless.rs",
            include_str!("../../crates/lca-cli/src/headless.rs"),
        ),
    ] {
        assert!(
            source.contains("install_panic_hook"),
            "{file} installs the panic hook"
        );
    }
}
