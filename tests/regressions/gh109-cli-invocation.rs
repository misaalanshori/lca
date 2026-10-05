//! GitHub #109: `-p` is a boolean print flag, messages are positional.
//!
//! Pi ground truth: `packages/coding-agent/docs/cli.md` (usage line,
//! `-p, --print`) and `src/cli/args.ts` (`print` boolean + `messages`
//! positional). `--prompt <text>` stays as the legacy spelling and
//! `@file` expansion is out of scope (it is #71's).

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
        } => (messages, model, session),
        other => panic!("headless, not {other:?}"),
    }
}

// Verifies: #109 (`lca -p hello` and `lca -p "hello"` each run one
// headless turn).
#[test]
fn gh109_print_flag_with_value_runs_one_headless_turn() {
    let (messages, _, _) = headless(&["-p", "hello"]);
    assert_eq!(messages, vec!["hello".to_string()]);
    let (messages, _, _) = headless(&["--print", "hello"]);
    assert_eq!(messages, vec!["hello".to_string()]);
}

// Verifies: #109 (`lca -p fix the bug`: several words, one turn).
#[test]
fn gh109_print_flag_collects_trailing_words_in_order() {
    let (messages, _, _) = headless(&["-p", "fix", "the", "bug"]);
    assert_eq!(messages, vec!["fix", "the", "bug"]);
}

// Verifies: #109 (a bare `-p` is print mode; with no message there is no
// turn to run).
#[test]
fn gh109_bare_print_flag_is_print_mode_with_no_message() {
    let (messages, _, _) = headless(&["-p"]);
    assert!(messages.is_empty(), "bare -p contributes no message");
}

// Verifies: #109 (the legacy `--prompt <text>` spelling keeps working).
#[test]
fn gh109_legacy_prompt_spelling_still_runs_headless() {
    let (messages, _, _) = headless(&["--prompt", "hello"]);
    assert_eq!(messages, vec!["hello".to_string()]);
}

// Verifies: #109 (`lca hello` opens the TUI and submits "hello" —
// asserted at the routing seam; the TUI preload is receipted live).
#[test]
fn gh109_positional_without_print_routes_interactive_with_initial() {
    match route(&parse(&["hello"])) {
        Route::Interactive {
            initial, resume, ..
        } => {
            assert_eq!(initial, vec!["hello".to_string()]);
            assert_eq!(resume, None);
        }
        other => panic!("interactive with initial, not {other:?}"),
    }
    match route(&parse(&["fix", "the", "bug"])) {
        Route::Interactive { initial, .. } => {
            assert_eq!(initial, vec!["fix", "the", "bug"]);
        }
        other => panic!("interactive with initials, not {other:?}"),
    }
}
