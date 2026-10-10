//! Unit tests for the dispatch rules and exit-code mapping.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use clap::Parser;
use lca_cli::{
    AuthCmd, Cli, OutputMode, Route, SessionSelector, check_flag_contradictions, exit, exit_code,
    output_mode, route,
};

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
            resume_picker: false,
            model: None,
            initial: Vec::new(),
            fork: None,
            session_id: None,
        }
    );
}

// Verifies: FR-CORE-3 (a prompt flag runs one turn headless; #109 keeps
// `--prompt` working and #111 threads the model and session selector)
#[test]
fn prompt_flag_routes_headless() {
    assert_eq!(
        route(&parse(&["-p", "hello", "--json"])),
        Route::Headless {
            messages: vec!["hello".to_string()],
            model: None,
            session: SessionSelector::New,
            mode: OutputMode::Json,
        }
    );
}

// Verifies: gh #56 (the `--mode` selector): text is the default,
// `--mode json` matches `--json`, and `--mode rpc` routes headless
// with no prompt of its own.
#[test]
fn mode_selector_routes_each_protocol() {
    assert_eq!(output_mode(&parse(&[])).expect("default"), OutputMode::Text);
    assert_eq!(
        output_mode(&parse(&["--mode", "json"])).expect("json"),
        OutputMode::Json
    );
    assert_eq!(
        output_mode(&parse(&["--json"])).expect("alias"),
        OutputMode::Json,
        "--json stays working as the deprecated alias"
    );
    assert_eq!(
        route(&parse(&["--mode", "rpc"])),
        Route::Headless {
            messages: Vec::new(),
            model: None,
            session: SessionSelector::New,
            mode: OutputMode::Rpc,
        }
    );
    assert_eq!(
        route(&parse(&["-p", "hi"])),
        Route::Headless {
            messages: vec!["hi".to_string()],
            model: None,
            session: SessionSelector::New,
            mode: OutputMode::Text,
        }
    );
}

// Verifies: gh #56 (contradictions are usage errors): `--json` against
// a non-json `--mode`, prompts with `--mode rpc`, and bogus modes.
#[test]
fn mode_contradictions_are_usage_errors() {
    assert!(
        check_flag_contradictions(&parse(&["--json", "--mode", "text"])).is_some(),
        "--json contradicts --mode text"
    );
    assert!(check_flag_contradictions(&parse(&["--json", "--mode", "rpc"])).is_some());
    assert!(
        check_flag_contradictions(&parse(&["--mode", "rpc", "-p", "hi"])).is_some(),
        "rpc takes no prompt arguments"
    );
    assert!(parse_try(&["--mode", "bogus"]).is_err(), "clap rejects it");
}

fn parse_try(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("lca").chain(args.iter().copied()))
}

// Verifies: FR-SESS-2 (resume with no id lists sessions)
#[test]
fn resume_without_id_lists_sessions() {
    assert_eq!(route(&parse(&["resume"])), Route::ResumeList);
    assert_eq!(
        route(&parse(&["resume", "01ABC"])),
        Route::Interactive {
            resume: Some("01ABC".to_string()),
            resume_picker: false,
            model: None,
            initial: Vec::new(),
            fork: None,
            session_id: None,
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

// Verifies: gh #72 (the credential subcommands route to the auth seam
// with their selectors intact)
#[test]
fn auth_routes_to_the_auth_command() {
    assert_eq!(
        route(&parse(&[
            "auth",
            "check",
            "--provider",
            "openai-compatible"
        ])),
        Route::Auth(AuthCmd::Check {
            provider: Some("openai-compatible".to_string()),
            model: None,
            json: false,
        })
    );
    assert_eq!(
        route(&parse(&["auth", "login", "--provider", "antigravity"])),
        Route::Auth(AuthCmd::Login {
            provider: "antigravity".to_string(),
        })
    );
    assert_eq!(
        route(&parse(&[
            "auth",
            "logout",
            "--provider",
            "openai-compatible"
        ])),
        Route::Auth(AuthCmd::Logout {
            provider: "openai-compatible".to_string(),
        })
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
    let (base, sha) = lca_cli::split_product_version("0.5.3");
    assert_eq!((base, sha), ("0.5.3", None));
    let (base, sha) = lca_cli::split_product_version("0.5.3.b194950");
    assert_eq!((base, sha), ("0.5.3", Some("194950")));
}

// Verifies: gh #205 (`lca clone` names a session and an optional title)
#[test]
fn clone_routes_to_the_clone_command() {
    assert_eq!(
        route(&parse(&["clone", "s1"])),
        Route::Clone {
            session: "s1".into(),
            title: None,
        }
    );
    assert_eq!(
        route(&parse(&["clone", "s1", "experimental"])),
        Route::Clone {
            session: "s1".into(),
            title: Some("experimental".into()),
        }
    );
}

// Verifies: gh #160 (session temp without the global) - distinct,
// validated per-session dirs; creation failures error.
// Verifies: gh #160 - two sessions resolve distinct, existing dirs
// under the data dir (no process-global involved).
#[test]
fn two_sessions_resolve_distinct_existing_dirs() {
    let base = std::env::temp_dir().join(format!("lca-temp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let first = lca_cli::ensure_session_temp(&base, "session-a").expect("create a");
    let second = lca_cli::ensure_session_temp(&base, "session-b").expect("create b");
    assert_ne!(first, second);
    assert!(first.is_dir() && second.is_dir());
    assert!(first.starts_with(base.join("tmp")));
    let _ = std::fs::remove_dir_all(&base);
}

// Verifies: gh #160 - a creation failure errors instead of the old
// silent `let _`.
#[test]
fn a_creation_failure_errors() {
    let base = std::env::temp_dir().join(format!("lca-temp-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::write(&base, "not a directory").expect("write");
    lca_cli::ensure_session_temp(&base, "session-a").expect_err("must fail");
    let _ = std::fs::remove_file(&base);
}

// Verifies: gh #133 receipt - the configured prefix rides the resolved
// shell the backend runs, so every command carries it.
#[test]
fn the_configured_prefix_rides_the_resolved_shell() {
    use std::collections::BTreeMap;
    let mut flags = BTreeMap::new();
    flags.insert(
        "shell.command_prefix".to_string(),
        "export LCA_PROBE=1".to_string(),
    );
    let config = lca_config::Config::load(&lca_config::LoadInput {
        flags,
        ..Default::default()
    })
    .expect("load");
    let ops = lca_cli::native_ops(&config);
    assert_eq!(
        ops.shell().command_prefix.as_deref(),
        Some("export LCA_PROBE=1")
    );
    assert_eq!(
        lca_cli::native_ops(&lca_config::Config::defaults())
            .shell()
            .command_prefix,
        None
    );
}

// Verifies: gh #110 - bare `-r` opens the session picker (pi parity),
// `--session` resumes direct, `-r <id>` keeps working.
#[test]
fn bare_r_opens_the_picker_and_session_resumes_direct() {
    assert!(matches!(
        route(&parse(&["-r"])),
        Route::Interactive {
            resume: None,
            resume_picker: true,
            ..
        }
    ));
    assert_eq!(
        route(&parse(&["-r", "abc"])),
        Route::Interactive {
            resume: Some("abc".to_string()),
            resume_picker: false,
            model: None,
            initial: Vec::new(),
            fork: None,
            session_id: None,
        }
    );
    assert_eq!(
        route(&parse(&["--session", "abc"])),
        Route::Interactive {
            resume: Some("abc".to_string()),
            resume_picker: false,
            model: None,
            initial: Vec::new(),
            fork: None,
            session_id: None,
        }
    );
}

// Verifies: gh #110 - a picker cannot render headless, so bare `-r`
// with a prompt is a usage error, not a silent new session.
#[test]
fn bare_r_with_a_prompt_is_a_contradiction() {
    assert!(
        check_flag_contradictions(&parse(&["-r", "-p", "hi"])).is_some(),
        "picker + headless prompt contradict"
    );
    assert!(
        check_flag_contradictions(&parse(&["--session", "abc", "-c"])).is_some(),
        "--session + -c contradict like -r + -c"
    );
}

// Verifies: FR-PERM-28 (`-a` and `-na` contradict; each parses alone).
#[test]
fn trust_flag_contradictions_are_usage_errors() {
    assert!(
        check_flag_contradictions(&parse(&["-a", "--no-approve"])).is_some(),
        "-a contradicts --no-approve"
    );
    assert!(
        check_flag_contradictions(&parse(&["--approve", "--na"])).is_some(),
        "--na is --no-approve"
    );
    assert!(check_flag_contradictions(&parse(&["-a"])).is_none());
    assert!(check_flag_contradictions(&parse(&["--no-approve"])).is_none());
}

// Verifies: FR-PERM-28 (startup trust precedence: flags beat stored
// trust beats the configured fallback).
#[test]
fn startup_trust_precedence_flags_stored_default() {
    use lca_cli::{CliFlags, apply_startup_trust};
    use lca_permissions::GrantStore;
    let root = std::env::temp_dir().join(format!("lca-trust-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let plain = root.join("plain");
    let stored = root.join("stored");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::create_dir_all(&stored).unwrap();
    let mut grants = GrantStore::open(&root.join("grants.json")).unwrap();
    grants.set_trusted(&stored, true).unwrap();

    // No flags: stored trust stands, the rest fall to the default.
    apply_startup_trust(&mut grants, &stored, &CliFlags::default(), "ask");
    assert!(grants.is_trusted_here(&stored));
    apply_startup_trust(&mut grants, &plain, &CliFlags::default(), "always");
    assert!(
        grants.is_trusted_here(&plain),
        "always trusts for the session"
    );
    assert!(!grants.is_trusted(&plain), "session-scoped, never stored");
    let refused = root.join("refused");
    std::fs::create_dir_all(&refused).unwrap();
    apply_startup_trust(&mut grants, &refused, &CliFlags::default(), "never");
    assert!(
        grants.is_refused_for_session(&refused),
        "never refuses the ask"
    );

    // `-a` trusts an unstored project for the session only.
    let fresh = root.join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    let flags = CliFlags {
        approve: true,
        ..Default::default()
    };
    apply_startup_trust(&mut grants, &fresh, &flags, "ask");
    assert!(grants.is_trusted_here(&fresh));
    assert!(!grants.is_trusted(&fresh), "never persisted");

    // `-na` flattens stored trust for the process.
    let flags = CliFlags {
        no_approve: true,
        ..Default::default()
    };
    apply_startup_trust(&mut grants, &stored, &flags, "ask");
    assert!(!grants.is_trusted_here(&stored), "forced distrust wins");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #98 (migration is a host-side session subcommand)
#[test]
fn session_migrate_routes_to_the_migrate_command() {
    assert_eq!(
        route(&parse(&["session", "migrate", "s1"])),
        Route::Migrate {
            session: "s1".into()
        }
    );
}
