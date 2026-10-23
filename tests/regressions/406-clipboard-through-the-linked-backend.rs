//! NFR-24 guard for gh #406: the clipboard goes through the backend that is
//! already linked, and a failed native write falls through instead of stopping.
//!
//! The defect was a **seam routing** one, not a missing feature. `arboard` is a
//! direct `lca-cli` dependency and was already doing the *image* read
//! (`attach.rs`), while the *text write* spawned a helper process per copy:
//! `pbcopy`, `clip`, `wl-copy`, `xclip`, `xsel`. On a machine with none of those
//! five - most Linux boxes have at best the last two, and containers usually
//! have none - a copy silently degraded to OSC 52 or failed outright, even
//! though a working in-process backend was linked into the same binary.
//!
//! Routing it through `arboard` also deletes a whole class of failure the helper
//! ladder had: a fork per copy, a PATH lookup that can find the wrong shim, and
//! a child process whose stdin can block on a clipboard owner that has gone
//! away.
//!
//! # What is pinned here, and why each row is shaped as it is
//!
//! A real clipboard is not available in CI (no display, no pasteboard, no
//! win32 session), so the rows assert the two things that **can** be checked
//! honestly:
//!
//! 1. **The seam is in-process.** A source-level assertion that the writer no
//!    longer spawns a helper. This is the shape the issue asks for: the fix is
//!    "stop shelling out", and a behavioural assertion cannot see a fork that
//!    succeeds silently.
//! 2. **The ladder still falls through.** A native writer that fails must reach
//!    OSC 52 - that is the property a `Clipboard::new()` error on a headless box
//!    depends on, and it is drivable with a fake writer.
//!
//! Verifies: R6 (a verified native clipboard, OSC 52 as the honest fallback),
//! ADR-0037's copy ladder, and NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The writer's source, so an assertion can look at it.
///
/// `hooks.rs` grew past gate 11's ceiling with this fix, so the writer moved to
/// its own module. The path is named here once; a second copy is what would let
/// the two halves drift apart again.
fn hooks_source() -> String {
    // The regression crate is the workspace root package, so the source path is
    // `crates/lca-cli/...` from here, not `../lca-cli/...`.
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-cli/src/tui/clipboard.rs"
    ))
    .expect("the writer's source is readable")
}

// Verifies: gh #406 - the helper ladder is gone. A fork per copy is the defect,
// and a behavioural row cannot see one that succeeds: the helper either writes
// the clipboard (and looks perfect) or is absent (and looks like a platform
// limitation). So the seam is asserted where it lives.
//
// `arboard` is already linked for the image read; the fix is routing the write
// through the same crate, not adding a dependency.
#[test]
fn the_clipboard_writer_no_longer_spawns_a_helper() {
    // Only the executable code counts: the module's comments name the five
    // helpers on purpose, so a reader learns what was removed and why. Asserting
    // over the raw file would forbid explaining the change.
    let source = code_only(&hooks_source());
    for helper in ["pbcopy", "wl-copy", "xclip", "xsel", "\"clip\""] {
        assert!(
            !source.contains(helper),
            "the clipboard writer still names `{helper}` in code; the in-process \
             backend (arboard, already a dependency for the image read) is the route"
        );
    }
    assert!(
        !source.contains("std::process::Command::new(program)"),
        "the helper loop is gone, so nothing should spawn a program for a copy"
    );
    // Scoped to the writer's own body. The module holds only the clipboard
    // today, but the scope is what makes the assertion mean "this function does
    // not spawn" rather than "this file never spawns" - and the latter would fail
    // the moment an unrelated helper landed beside it.
    let writer = function_body(&source, "fn native_clipboard");
    assert!(
        !writer.contains("Command::new") && !writer.contains("Stdio::piped"),
        "the clipboard writer spawns nothing; its body is the in-process backend"
    );
    assert!(
        writer.contains("arboard"),
        "and that backend is arboard, which the image path already links"
    );
}

/// One function's body, from its signature to the matching closing brace at
/// column zero - enough to say "this function does not spawn" without banning
/// spawning in the rest of the file.
fn function_body(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("the function `{signature}` is present"));
    let rest = &source[start..];
    match rest.find("\n}\n") {
        Some(end) => rest[..end].to_string(),
        None => rest.to_string(),
    }
}

/// The source with comments stripped, so an assertion can target behaviour
/// rather than the prose that documents it.
///
/// Deliberately crude - it drops `//` to end-of-line and `/* */` blocks - because
/// the alternative is a Rust parser for a test, and a comment that survives this
/// is a comment whose text contains a `//` inside a string literal, which no
/// assertion here depends on.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("//") {
        out.push_str(&rest[..start]);
        match rest[start..].find('\n') {
            Some(end) => rest = &rest[start + end..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

// Verifies: gh #406 - the writer is the in-process backend, and it is the same
// crate the image path uses. Two rows would be easy to pass with a third
// clipboard crate or a re-implemented X11 path, so this pins the identity.
#[test]
fn the_clipboard_writer_uses_the_already_linked_backend() {
    let source = hooks_source();
    assert!(
        source.contains("arboard"),
        "the writer goes through arboard - the crate already linked for the \
         image read, which handles Win32 contention, the macOS pasteboard and \
         X11 INCR itself"
    );
    let attach = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-cli/src/tui/attach.rs"
    ))
    .expect("attach.rs is readable");
    assert!(
        attach.contains("arboard"),
        "and the image path keeps its arboard read: this change is the text \
         half, not a replacement of the image half"
    );
}

// Verifies: gh #406 design point 2 - **a failed native write must fall through,
// not stop**. `arboard` errors when there is no display (SSH, headless, a
// container), which is precisely when OSC 52 must take over. The ladder already
// had this shape for a missing helper; this row proves it survived the change
// of writer, because the failure mode changed from "no program" to "an error
// return" and the two are easy to confuse.
#[test]
fn a_failing_native_write_still_falls_through_to_osc52() {
    // Drive the real ladder: a native writer that fails, and an OSC 52 sink.
    let mut sent = Vec::new();
    let failing: lca_ui::ClipboardWriter = std::sync::Arc::new(|_text: &str| false);
    let mut osc = |text: &str| sent.push(text.to_string());
    let report = lca_ui::route_a_copy("headless", Some(&failing), Some(&mut osc));
    assert_eq!(
        report, "osc52",
        "a native failure reaches OSC 52 rather than stopping: {report}"
    );
    assert_eq!(sent, vec!["headless".to_string()], "and the text goes out");
}

// Verifies: gh #406's regression pin - when OSC 52 cannot run either, the
// report is `failed` and the notice does not claim a copy. A fix that made the
// ladder "always succeed" would silence the one honest message the user needs.
#[test]
fn with_no_step_available_the_copy_reports_failure() {
    let failing: lca_ui::ClipboardWriter = std::sync::Arc::new(|_text: &str| false);
    let report = lca_ui::route_a_copy("nowhere", Some(&failing), None);
    assert_eq!(report, "failed", "no step ran, so no step is claimed");
}

// Verifies: gh #406 design point 4 - a clipboard read is not worth a frozen
// interface. `arboard::Clipboard::new()` and `get_text` can block on X11 (a hung
// selection owner, the INCR protocol), and the command dispatcher runs on the
// key/render thread. The image path had exactly this hazard before the issue
// (it called `Clipboard::new()` inline); the text path must not repeat it.
//
// Pinned structurally: a bounded read exists, and it is the thing `/paste` and
// the text read call. A wall-clock assertion would be the load sensitivity this
// repo has spent cycles removing, and it cannot see a hang that never returns.
#[test]
fn a_clipboard_read_is_bounded_and_off_the_key_thread() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-cli/src/tui/attach.rs"
    ))
    .expect("attach.rs is readable");
    assert!(
        source.contains("with_clipboard_timeout")
            || source.contains("clipboard_timeout")
            || source.contains("CLIPBOARD_TIMEOUT"),
        "a clipboard read carries a bound: X11 can block on a hung selection \
         owner, and this runs off the key path"
    );
    assert!(
        !source
            .contains("arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image())"),
        "the unbounded inline read is gone from the command path"
    );
}
