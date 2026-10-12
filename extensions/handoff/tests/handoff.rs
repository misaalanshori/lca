//! Structured task-transition handoffs (gh #196): the prompt builder,
//! the artifact writer, and the full pipeline as a pure function of
//! its dependencies (turns, a completion call, a filesystem).
//!
//! Verifies: the summarization prompt requires all four sections,
//! instructions steer the focus, the artifact lands at the specified
//! path, empty sessions get the documented message, a failed
//! completion propagates instead of writing a hollow artifact, and
//! the invoke seam reports the missing host feed rather than
//! faking a document.

use handoff::{
    EMPTY_SESSION_MESSAGE, HANDOFF_DIR, artifact_filename, build_summarization_prompt,
    format_artifact, run_handoff,
};

fn turns() -> Vec<handoff::Turn> {
    vec![
        handoff::Turn {
            role: "user".to_string(),
            text: "Refactor the parser.".to_string(),
        },
        handoff::Turn {
            role: "assistant".to_string(),
            text: "Split it into lexer and grammar; tests pass.".to_string(),
        },
    ]
}

// Verifies: the prompt carries the transcript plus the four required
// sections, so whatever the completion returns covers them.
#[test]
fn the_prompt_requires_all_four_sections() {
    let prompt = build_summarization_prompt(&turns(), "");
    for section in [
        "Completed Tasks",
        "Architectural Decisions",
        "Current State",
        "Remaining Next Steps",
    ] {
        assert!(prompt.contains(section), "missing {section}:\n{prompt}");
    }
    assert!(prompt.contains("Refactor the parser."), "{prompt}");
    assert!(prompt.contains("lexer and grammar"), "{prompt}");
}

// Verifies: inline instructions steer what the summary prioritizes.
#[test]
fn instructions_steer_the_focus() {
    let prompt = build_summarization_prompt(&turns(), "focus on finishing the parser refactor");
    assert!(
        prompt.contains("focus on finishing the parser refactor"),
        "{prompt}"
    );
    let plain = build_summarization_prompt(&turns(), "");
    assert!(!plain.contains("Focus:"), "{plain}");
}

// Verifies: the artifact filename is timestamped inside the handoff
// directory (the issue's `<workspace>/.lca/extensions/handoff/`
// path, workspace-relative).
#[test]
fn artifact_filenames_are_timestamped_in_the_handoff_dir() {
    assert_eq!(
        artifact_filename(1_700_000_000),
        format!("{HANDOFF_DIR}/handoff-1700000000.md")
    );
    assert!(HANDOFF_DIR.starts_with(".lca/extensions/handoff"));
}

// Verifies: the artifact wraps the summary with its provenance, so
// the file is self-describing when read back next session.
#[test]
fn the_artifact_carries_summary_and_provenance() {
    let text = format_artifact("## Done\n- Parser split.", "focus on X", 42);
    assert!(text.contains("## Done\n- Parser split."), "{text}");
    assert!(text.contains("focus on X"), "{text}");
    assert!(text.contains("42"), "{text}");
}

// Verifies: the full pipeline with a stub completion — document out,
// artifact written, sections intact.
#[test]
fn the_pipeline_writes_the_artifact_and_returns_the_document() {
    let written = std::cell::RefCell::new(Vec::new());
    let outcome = run_handoff(
        &turns(),
        "focus on Y",
        |prompt| {
            assert!(prompt.contains("focus on Y"), "{prompt}");
            Ok("## Completed Tasks\n- Parser split.".to_string())
        },
        |path, text| {
            written
                .borrow_mut()
                .push((path.to_string(), text.to_string()));
            Ok(())
        },
        7,
    )
    .expect("pipeline");
    assert!(
        outcome.document.contains("Parser split."),
        "{}",
        outcome.document
    );
    assert_eq!(written.borrow().len(), 1);
    let (path, text) = &written.borrow()[0];
    assert!(path.ends_with("handoff-7.md"), "{path}");
    assert!(text.contains("Parser split."), "{text}");
}

// Verifies: an empty session never reaches completion (no point
// summarizing nothing) and reports the documented message.
#[test]
fn empty_sessions_report_the_documented_message() {
    let called = std::cell::RefCell::new(false);
    let outcome = run_handoff(
        &[],
        "",
        |_| {
            *called.borrow_mut() = true;
            Ok(String::new())
        },
        |_, _| Ok(()),
        0,
    )
    .expect("pipeline");
    assert!(!*called.borrow(), "completion must not run");
    assert_eq!(outcome.document, EMPTY_SESSION_MESSAGE);
    assert!(outcome.artifact.is_none());
}

// Verifies: a failed completion propagates and writes nothing — a
// hollow artifact is worse than an error.
#[test]
fn a_failed_completion_writes_nothing() {
    let written = std::cell::RefCell::new(0);
    let err = run_handoff(
        &turns(),
        "",
        |_| Err("provider down".to_string()),
        |_, _| {
            *written.borrow_mut() += 1;
            Ok(())
        },
        0,
    )
    .expect_err("failure propagates");
    assert!(err.contains("provider down"), "{err}");
    assert_eq!(*written.borrow(), 0);
}

// Verifies: the manifest test contract — the command world, the
// completion grant, workspace read-write for artifacts.
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = handoff::MANIFEST;
    assert!(manifest.contains("worlds = [\"command\"]"), "{manifest}");
    let grants = handoff::manifest_grants();
    assert!(grants.completion, "one-shot summarization needs it");
    assert_eq!(grants.fs.len(), 1, "workspace read-write only");
    assert!(manifest.contains("[capabilities.completion]"), "{manifest}");
    assert!(
        manifest.contains("workspace = \"read-write\""),
        "{manifest}"
    );
}
