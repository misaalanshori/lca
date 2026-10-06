//! GitHub #111: headless honors `--model`, `-c`, `-r`.
//!
//! Routing rows live here; the live runs (append to `log.jsonl`, model in
//! the records) are `crates/lca-cli/tests/e2e_headless_session.rs`
//! against the loopback mock.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use clap::Parser;
use lca_cli::{Cli, Route, SessionSelector, route};

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("lca").chain(args.iter().copied())).expect("parses")
}

fn headless(args: &[&str]) -> (Vec<String>, Option<String>, SessionSelector) {
    match route(&parse(args)) {
        Route::Headless {
            messages,
            model,
            session,
            ..
        } => (messages, model, session),
        other => panic!("headless, not {other:?}"),
    }
}

// Verifies: #111 (`lca -c -p x` continues the last session headlessly).
#[test]
fn gh111_continue_selects_the_latest_session() {
    let (messages, model, session) = headless(&["-c", "-p", "x"]);
    assert_eq!(messages, vec!["x".to_string()]);
    assert_eq!(model, None);
    assert_eq!(session, SessionSelector::Continue);
}

// Verifies: #111 (`lca -r <id> -p x` resumes that session headlessly).
#[test]
fn gh111_resume_selects_the_named_session() {
    let (_, _, session) = headless(&["-r", "abc123", "-p", "x"]);
    assert_eq!(session, SessionSelector::Resume("abc123".to_string()));
}

// Verifies: #111 (`lca --model m -p x` carries the override).
#[test]
fn gh111_model_override_rides_the_headless_route() {
    let (messages, model, session) = headless(&["--model", "zen-free", "-p", "x"]);
    assert_eq!(messages, vec!["x".to_string()]);
    assert_eq!(model, Some("zen-free".to_string()));
    assert_eq!(session, SessionSelector::New);
}

// Verifies: #111 (contradictions are rejected with clear errors —
// exercised through `run`, which owns the exit code).
#[test]
fn gh111_continue_and_resume_together_are_rejected() {
    let cli = parse(&["-c", "-r", "abc", "-p", "x"]);
    let code = lca_cli::check_flag_contradictions(&cli);
    assert!(code.is_some(), "-c with -r is a usage error");
}
