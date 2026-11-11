//! Gh #575: `run` is infallible, and the binary's seam is where fallibility lives.
//!
//! The row that matters is a **compile-time** fact the issue got wrong: it asked
//! for `Result<(), UiError>` on a function whose body cannot fail (zero `?`, zero
//! `Err(`, every exit an `Ok`). A typed error there would have been three
//! variants nobody constructs - so the type on this boundary is `i32`, and this
//! file pins the two halves that a reader would otherwise have to trust:
//!
//! 1. `run` returns the exit code directly (no `Result` to unwrap), and
//! 2. the one real failure a terminal can report - a refused raw mode - is a
//!    **notice**, not an error, because the interface keeps working.
//!
//! The signature half is asserted by the call below: it would not compile against
//! a `Result`. That is the point - a row that grepped the signature could pass on
//! a tree with a dead `Err` arm, which is exactly what shipped.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

// Verifies: gh #575 - **the signature is the assertion.** `let code: i32 =
// lca_ui::run(...)` cannot compile if the function returns a `Result`, so this
// checks the type at build time rather than by reading a string.
#[test]
fn the_run_entry_returns_an_exit_code_not_a_result() {
    // Named, not called: a function item coerces to `fn(UiOptions, TurnRunner)
    // -> i32` only when that is its type.
    let entry: fn(crate::state::UiOptions, crate::state::TurnRunner) -> i32 = super::run;
    assert_eq!(
        std::mem::size_of_val(&entry),
        std::mem::size_of::<fn(crate::state::UiOptions, crate::state::TurnRunner) -> i32>(),
        "the entry is an ordinary `fn` returning the code"
    );
    // And the srdd rule the change restores: a library carries no `anyhow`.
    // Code only: the doc block above *names* the type it replaced, and a row
    // that banned the string would fail on its own explanation.
    let source = {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
        let lib = std::fs::read_to_string(&path).expect("lib.rs is readable");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut code = lib;
        let run = std::fs::read_to_string(path.join("run.rs")).expect("run.rs");
        code.push_str(
            &run.lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        code
    };
    assert!(
        !source.contains("anyhow::Result"),
        "no opaque error in a library crate (docs/lca-srdd.md, coding standards)"
    );
}

// Verifies: gh #575 - the CLI's own seam still reports *its* failures, and the
// interface's exit code passes through untouched.
//
// `tui::run` returns `Result` for the setup before the loop (a session, a temp
// dir). Both halves are in its shape: `Ok(code)` from the interface, and an error
// that only the pre-loop code can produce.
#[test]
fn the_binary_seam_still_carries_its_own_failures() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../lca-cli/src/tui/mod.rs");
    let source = std::fs::read_to_string(&path).expect("tui/mod.rs is readable");
    let seam = source
        .find("let code = lca_ui::run(options, runner);")
        .expect("the interface's result is taken as a code, not matched");
    let tail = &source[seam..];
    let end = tail.find("\n}\n").unwrap_or(tail.len());
    let body = &tail[..end];
    assert!(
        body.contains("Ok(code)"),
        "the code passes through unchanged: {body}"
    );
    assert!(
        !body.contains("Err("),
        "and there is no error arm for a call that cannot fail: {body}"
    );
}
