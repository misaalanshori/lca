//! Gh #549 - a turn ending is not a failure, so it does not travel as an `Err`.
//!
//! Nine helpers used to return `Result<T, TurnOutcome>` and spell an early exit
//! as `Err(outcome)`, which is why nine `result_large_err` suppressions existed:
//! `TurnOutcome` carries a whole `Usage`, so clippy saw a large error type
//! everywhere a *successful* early exit was written as one. `TurnStep<T>` says
//! exactly two things - `Continue(value)` or `End(outcome)` - and has no error
//! channel at all.
//!
//! These rows assert the observable contract, not the shape: an early-ended
//! turn still reports its own status/stop reason (so the end reached the caller
//! without being an error), and the step type cannot be confused with a failure.
//!
//! Verifies: gh #549 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use lca_core::TurnStatus;
use lca_protocol::StopReason;
use lca_testkit::{FakeProvider, fake_usage};

// Verifies: gh #549 - **a turn that ends early carries its outcome, and it is not
// an error.** The iteration-limit row already proves the *value*; this row proves
// the shape that carries it has no error channel: the same turn is driven twice,
// once to the limit and once naturally, and both arrive as an outcome (never an
// `is_err()`), with the status the turn itself decided.
#[tokio::test]
async fn an_early_ended_turn_carries_its_outcome_without_being_an_error() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("write", r#"{"path":"a.txt","content":"A"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| {
            t.tool_call("write", r#"{"path":"b.txt","content":"B"}"#)
                .usage(fake_usage(20, 10, 10, 10))
        })
        .build();
    let mut h = common::harness("step-limit", provider, {
        let mut config = common::default_config();
        config.max_iterations = 1;
        config
    });
    let mut sink = common::CollectingSink::default();
    let mut prompt = common::Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = common::turn(&mut h, "step", &mut sink, &mut prompt).await;
    // The end of the turn is an outcome, so this is not `is_err()` and the
    // status is the one the loop chose - "the turn is over" is data, not failure.
    assert_eq!(
        outcome.status,
        TurnStatus::Error,
        "limit ends with an error status"
    );
    assert_eq!(outcome.stop_reason, StopReason::IterationLimit);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|text| text.contains("iteration limit")),
        "and the outcome carries the message the loop built: {outcome:?}"
    );
    assert_eq!(
        h.provider.call_count(),
        2,
        "one round ran, the second never did"
    );
}

// Verifies: gh #549 - the step type exists, has exactly the two arms, and no
// error channel. It is `pub(super)`: internal to the turn module by design
// (nothing outside the loop should be able to spell "the turn ended"), so this
// row reads the source - the shape *is* the contract here, and the effect
// (outcomes arriving as data) is pinned by the row above.
#[test]
fn the_step_enum_has_two_arms_and_no_error_channel() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/turn/mod.rs"),
    )
    .expect("read turn/mod.rs");
    let declaration = source
        .split("pub(super) enum TurnStep<T> {")
        .nth(1)
        .expect("`TurnStep` is declared")
        .split('}')
        .next()
        .expect("the enum closes");
    assert!(
        declaration.contains("Continue(T)") && declaration.contains("End(TurnOutcome)"),
        "the two arms are the whole vocabulary: {declaration}"
    );
    assert!(
        !declaration.to_lowercase().contains("error"),
        "and neither arm is an error channel: {declaration}"
    );
    // Nine helpers used to suppress `result_large_err`; now zero do, and the
    // only mention left in the module is the doc comment explaining why.
    let allows = source.matches("#[allow(clippy::result_large_err)]").count();
    assert_eq!(
        allows, 0,
        "no `result_large_err` suppression remains in the turn module (found {allows})"
    );
    for file in ["execute", "persist", "request"] {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/turn/{file}.rs")),
        )
        .expect("read a turn sibling");
        assert_eq!(
            text.matches("#[allow(clippy::result_large_err)]").count(),
            0,
            "`{file}.rs` carries no suppression either"
        );
        assert!(
            text.contains("TurnStep<"),
            "`{file}.rs` returns steps, not `Result<_, TurnOutcome>`"
        );
    }
}

// Verifies: gh #549 - **the attachment API is typed and its copy is unchanged.**
// `lca-core` is a library, so a bare `String` error was the SRDD's rule broken;
// the one thing a conversion may not do is reword the refusal, because `/attach`
// prints it and the `read` tool's variant reaches a model.
#[test]
fn the_attachment_error_is_typed_and_keeps_its_copy() {
    let dir = lca_testkit::scratch_path("step-552-attach");
    std::fs::create_dir_all(&dir).expect("dir");
    let store = lca_session::SessionStore::new(dir.join("data"));
    let session = store.create_session(&dir, "attach").expect("create");

    let not_image = dir.join("notes.txt");
    std::fs::write(&not_image, b"just text").expect("write");
    let err: lca_core::AttachmentError =
        lca_core::stage_image(&session, &not_image).expect_err("not an image");
    // Exactly the sentence this API produced before it was typed.
    assert_eq!(
        err.to_string(),
        format!(
            "{} is not a recognized image (png, jpeg, gif, or webp)",
            not_image.display()
        ),
        "the refusal copy is unchanged"
    );
    assert_eq!(err.message(), err.to_string(), "and the accessor agrees");
    // It is a real `Error`, so a caller can box it or walk its source.
    let boxed: Box<dyn std::error::Error> = Box::new(err);
    assert!(boxed.to_string().contains("recognized image"));

    // A missing file keeps its own sentence, path first.
    let missing = dir.join("nope.png");
    let err = lca_core::stage_image(&session, &missing).expect_err("missing");
    assert!(
        err.to_string()
            .starts_with(&format!("cannot read {}:", missing.display())),
        "the read refusal names the path: {err}"
    );
}

// Verifies: gh #549 - the three output structs are closed to outside literals, so
// a field added later breaks no caller. Measured first: zero external literals for
// all three (`grep` over crates/, tests/, extensions/).
#[test]
fn the_output_structs_are_closed_to_outside_literals() {
    let assemble = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assemble/mod.rs"),
    )
    .expect("read assemble/mod.rs");
    assert!(
        assemble.contains("#[non_exhaustive]\npub struct Assembled"),
        "`Assembled` is `#[non_exhaustive]`: an outside literal is E0639"
    );
    let pricing = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pricing.rs"),
    )
    .expect("read pricing.rs");
    assert!(
        pricing.contains("#[non_exhaustive]\npub struct PricedUsage"),
        "`PricedUsage` is closed too"
    );
    let registry = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/registry/mod.rs"),
    )
    .expect("read registry/mod.rs");
    assert!(
        registry.contains("#[non_exhaustive]\npub struct NamespaceView"),
        "and `NamespaceView`"
    );
    // A `Attachment` stays exhaustive: `crates/lca-core/tests/attachments.rs`
    // builds it twice, and the row above this one says why the count matters.
}
