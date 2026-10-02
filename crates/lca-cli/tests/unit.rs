//! Unit tests for the dispatch rules and exit-code mapping.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use clap::Parser;
use lca_cli::{Cli, Route, exit, exit_code, route};

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("lca").chain(args.iter().copied())).expect("parses")
}

// Verifies: FR-CORE-2 (no arguments opens the interactive interface in the
// current working directory)
#[test]
fn no_arguments_route_to_the_interactive_interface() {
    assert_eq!(
        route(&parse(&[])),
        Route::Interactive {
            resume: None,
            model: None,
        }
    );
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
            resume: Some("01ABC".to_string()),
            model: None,
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
    let config = lca_cli::load_config(&grants_dir, &grants, true, false).expect("config");
    assert!(!config.update_check(true), "headless default is off");
    assert!(config.update_check(false), "interactive default is on");
}

// The core default and the configuration default must agree: a consumer
// building `AgentConfig` directly (the embedding SDK path) gets the same
// `tool.max_iterations` cap the configuration doc and `lca-config`
// document. Drift here is invisible to every behavior test, so the two
// crates' defaults are compared directly.
#[test]
fn the_core_default_and_the_config_default_agree() {
    assert_eq!(
        u64::from(lca_core::AgentConfig::default().max_iterations),
        lca_config::DEFAULT_TOOL_MAX_ITERATIONS
    );
}

// Verifies: ADR-0043 / U1 - the product version is either the crate
// version (the stable line) or `X.Y.Z.b<sha7>` (what the unstable
// workflow bakes), and `--version`'s first line is the same string.
#[test]
fn the_product_version_is_stable_or_unstable_shape() {
    let product = env!("PRODUCT_VERSION");
    let (base, suffix) = lca_cli::split_product_version(product);
    let parts: Vec<&str> = base.split('.').collect();
    assert_eq!(parts.len(), 3, "X.Y.Z base: {product}");
    assert!(
        parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())),
        "every base part is digits: {product}"
    );
    match suffix {
        Some(sha) => {
            assert_eq!(sha.len(), 7, "the sha is seven hex digits: {product}");
            assert!(
                sha.chars().all(|c| c.is_ascii_hexdigit()),
                "hex only: {product}"
            );
        }
        None => assert_eq!(
            product,
            env!("CARGO_PKG_VERSION"),
            "no suffix: crate version"
        ),
    }
    // The build-time override is what the shape above reflects.
    match std::env::var("LCA_BUILD_VERSION") {
        Ok(baked) => assert_eq!(product, baked, "the override won at build time"),
        Err(_) => assert_eq!(product, env!("CARGO_PKG_VERSION")),
    }
    assert!(lca_cli::version_text().starts_with(product), "line one");
}

// Verifies: both forms of the scheme parse (the unstable workflow's
// composition and the stable default).
#[test]
fn both_product_version_forms_split() {
    let (base, sha) = lca_cli::split_product_version("0.5.2");
    assert_eq!((base, sha), ("0.5.2", None));
    let (base, sha) = lca_cli::split_product_version("0.5.2.b194950");
    assert_eq!((base, sha), ("0.5.2", Some("194950")));
}
