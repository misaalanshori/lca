//! Unit tests for the dispatch rules and exit-code mapping.

use clap::Parser;
use lca_cli::{Cli, Route, exit, exit_code, route};

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("lca").chain(args.iter().copied())).expect("parses")
}

// Verifies: FR-CORE-2 (no arguments opens the interactive interface in the
// current working directory)
#[test]
fn no_arguments_route_to_the_interactive_interface() {
    assert_eq!(route(&parse(&[])), Route::Interactive { resume: None });
}

// Verifies: FR-CORE-3 (a prompt flag runs one turn headless)
#[test]
fn prompt_flag_routes_headless() {
    assert_eq!(
        route(&parse(&["-p", "hello", "--json"])),
        Route::Headless {
            prompt: "hello".to_string()
        }
    );
}

// Verifies: FR-SESS-2 (resume with no id lists sessions)
#[test]
fn resume_without_id_lists_sessions() {
    assert_eq!(route(&parse(&["resume"])), Route::ResumeList);
    assert_eq!(
        route(&parse(&["resume", "01ABC"])),
        Route::Interactive {
            resume: Some("01ABC".to_string())
        }
    );
}

// Verifies: FR-SESS-3 (fork names a session and a message)
#[test]
fn fork_routes_to_the_fork_command() {
    assert_eq!(
        route(&parse(&["fork", "s1", "r2"])),
        Route::Fork {
            session: "s1".into(),
            message: "r2".into()
        }
    );
}

// Verifies: D5 (the attachment GC is a host-side session subcommand)
#[test]
fn session_gc_routes_to_the_gc_command() {
    assert_eq!(
        route(&parse(&["session", "gc", "s1"])),
        Route::Gc {
            session: "s1".into()
        }
    );
}

// Exit-code mapping, docs/headless.md.
#[test]
fn exit_codes_follow_the_documented_table() {
    use lca_core::{StopReason, TurnOutcome, TurnStatus};
    use lca_protocol::Usage;

    let turn = |status, reason| TurnOutcome {
        status,
        stop_reason: reason,
        usage: Usage::default(),
        error: None,
    };
    assert_eq!(
        exit_code(&turn(TurnStatus::Ok, StopReason::Stop), false, None),
        exit::OK
    );
    assert_eq!(
        exit_code(&turn(TurnStatus::Ok, StopReason::Cancelled), false, None),
        exit::OK
    );
    assert_eq!(
        exit_code(
            &turn(TurnStatus::Error, StopReason::IterationLimit),
            false,
            None
        ),
        exit::ABORTED
    );
    assert_eq!(
        exit_code(
            &turn(TurnStatus::Error, StopReason::Error),
            false,
            Some("transport")
        ),
        exit::PROVIDER
    );
    assert_eq!(
        exit_code(
            &turn(TurnStatus::Error, StopReason::Error),
            false,
            Some("auth")
        ),
        exit::PROVIDER
    );
    assert_eq!(
        exit_code(
            &turn(TurnStatus::Error, StopReason::Error),
            false,
            Some("internal")
        ),
        exit::INTERNAL
    );
    // A needed approval wins over everything: headless cannot prompt.
    assert_eq!(
        exit_code(&turn(TurnStatus::Ok, StopReason::Stop), true, None),
        exit::PERMISSION
    );
}

// FR-CFG-6: headless mode resolves update.check to off by default.
#[test]
fn headless_defaults_the_update_check_off() {
    let grants_dir = lca_testkit::scratch_path("lca-cli-unit");
    let _ = std::fs::remove_dir_all(&grants_dir);
    std::fs::create_dir_all(&grants_dir).expect("mkdir");
    let grants = lca_permissions::GrantStore::open(&grants_dir.join("grants.json")).expect("open");
    let config = lca_cli::load_config(&grants_dir, &grants, true).expect("config");
    assert!(!config.update_check(true), "headless default is off");
    assert!(config.update_check(false), "interactive default is on");
}
