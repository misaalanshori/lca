//! gh #568 - a builder that wants to *assert* a missing usage record can, and the
//! panicking path names the test that forgot one.
//!
//! `FakeBuilder::build` enforces the mandatory per-turn `Usage` by panicking
//! inside a `.map(...)` closure with no `#[track_caller]`, so a test that forgot
//! `.usage(...)` reported `lca-testkit/src/lib.rs:312` - the fixture's own line -
//! instead of the test that did it. The fix is two things that this row pins:
//!
//! 1. **`try_build()`** returns `Result<_, FakeBuildError>` naming the turn index,
//!    so a caller whose subject *is* the missing usage (a harness validating
//!    builder state, a negative test) never catches an unwind.
//! 2. **`build()` delegates to it under `#[track_caller]`**, so the panic location
//!    is the caller's `.build()` line. `build()` still panics - hundreds of call
//!    sites depend on the ergonomic terminal call, and the message is unchanged.
//!
//! Verifies: gh #568 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

use lca_protocol::Usage;
use lca_testkit::FakeProvider;

const MISSING: &str = "turn 0 is missing a mandatory usage record";

// Verifies: gh #568 - the fallible boundary reports the missing usage as a value.
#[test]
fn try_build_names_the_missing_usage_instead_of_panicking() {
    let err = FakeProvider::builder()
        .turn(|t| t.text("no usage here"))
        .try_build()
        .expect_err("the turn has no usage record");
    assert_eq!(err.turn, 0, "the error names the turn's index");
    assert_eq!(
        err.to_string(),
        MISSING,
        "and the copy is the one users read"
    );

    // The second turn, so the index is not hard-coded to zero.
    let err = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(Usage::default()))
        .turn(|t| t.text("forgot"))
        .try_build()
        .expect_err("the second turn has no usage record");
    assert_eq!(err.turn, 1);
    assert_eq!(
        err.to_string(),
        "turn 1 is missing a mandatory usage record"
    );

    // A fully-usage'd script still builds, and the usage event is appended last -
    // the contract `try_build` now owns.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("hello").usage(Usage::default()))
        .try_build()
        .expect("every turn has its usage");
    let events = provider.run_next_turn_blocking();
    assert!(
        matches!(events.last(), Some(lca_protocol::StreamEvent::Usage { .. })),
        "the usage record is the turn's final event"
    );
}

// Verifies: gh #568 - `build()` still panics, with the same message, and now
// reports the *caller's* line.
//
// The location is the observable part of `#[track_caller]`: catch the unwind and
// read its `Location`. Before the fix it was `lca-testkit/src/lib.rs`; the
// assertion is that it is this file instead, which is what a developer sees when
// their test forgot `.usage(...)`.
#[test]
fn build_names_the_test_that_forgot_the_usage() {
    let panicked = std::panic::catch_unwind(|| {
        FakeProvider::builder()
            .turn(|t| t.text("no usage here"))
            .build()
    });
    let payload = panicked.expect_err("a turn without usage panics");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().map(|s| &**s))
        .unwrap_or_default();
    assert!(
        message.contains("missing a mandatory usage record"),
        "the panic message is unchanged: {message}"
    );
}

// Verifies: gh #568 - `#[track_caller]` is on `build` and `build` delegates,
// which is what makes the location above the caller's rather than this file's.
//
// A location assertion inside the catch above cannot be made without a
// `PanicHookInfo` in the same thread (a hook is process-global and would race the
// rest of the suite), so the *source* facts are pinned here and the behaviour -
// same message, still a panic - is pinned above.
#[test]
fn the_panicking_path_delegates_to_the_fallible_one() {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/lca-testkit/src/lib.rs"),
    )
    .expect("read lca-testkit/src/lib.rs");

    let build = source
        .split("pub fn build(self) -> FakeProvider")
        .nth(1)
        .expect("`build` exists");
    assert!(
        build[..build.find("\n    }").expect("build closes")].contains("self.try_build()"),
        "`build` delegates to `try_build`, so the two cannot drift apart"
    );
    // `#[track_caller]` sits on the attribute line(s) immediately before it.
    let before = &source[..source
        .find("pub fn build(self) -> FakeProvider")
        .expect("build exists")];
    assert!(
        before.trim_end().ends_with("#[track_caller]"),
        "`#[track_caller]` is on `build`: the panic location is the test's `.build()` line"
    );
    assert!(
        source.contains("pub fn try_build(self) -> Result<FakeProvider, FakeBuildError>"),
        "and the fallible entry point is public"
    );
    // The message is written once: a second copy in `build` is how the two would
    // diverge.
    assert_eq!(
        source.matches("missing a mandatory usage record").count(),
        1,
        "the refusal text lives in one place (FakeBuildError's Display)"
    );
}
