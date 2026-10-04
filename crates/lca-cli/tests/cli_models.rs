//! The model CLI parity rows (gh #8 / EFG-003, EFG-041): `--list-models`
//! as the scripting building block, and the flags' usage rules - pi's
//! `--provider`, `--thinking` vocabulary - decided at the command line
//! where a script can see the exit code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #8 acceptance (3) - `--list-models` prints three columns
// per model (`id`, the provider service, the context window) in a stable
// order and exits 0, so a script can read it.
#[cfg(unix)]
#[test]
fn list_models_prints_id_provider_context_and_exits_zero() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("list-models-shape");
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "models": "alpha-model,beta-model" }),
    );

    let out = sandbox.run_env(
        Some(&mock),
        &["--list-models"],
        &[("OPENAI_MODEL", "test-model")],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exit 0: {:?} {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 3, "one line per offered model:\n{stdout}");
    for line in &lines {
        let fields: Vec<&str> = line.split("  ").collect();
        assert_eq!(fields.len(), 3, "`id  provider  context`: {line}");
        assert_eq!(
            fields[1], "openai-compatible",
            "the provider column: {line}"
        );
        assert!(
            fields[2].parse::<u32>().is_ok(),
            "the context column is a number: {line}"
        );
    }
    assert_eq!(
        lines,
        vec![
            "alpha-model  openai-compatible  0",
            "beta-model  openai-compatible  0",
            "test-model  openai-compatible  0",
        ],
        "sorted by provider then id, so two listings diff cleanly:\n{stdout}"
    );
}

// Verifies: gh #8 acceptance (3) - the optional pattern filters the
// listing through the same matcher the scope uses, and a pattern that
// matches nothing says so (pi's line) while still exiting 0.
#[cfg(unix)]
#[test]
fn list_models_search_filters_the_listing() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("list-models-search");
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "models": "alpha-model,beta-model" }),
    );

    let out = sandbox.run_env(
        Some(&mock),
        &["--list-models", "beta"],
        &[("OPENAI_MODEL", "test-model")],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit 0 for a search");
    assert_eq!(
        stdout.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        vec!["beta-model  openai-compatible  0"],
        "only the matching model:\n{stdout}"
    );

    let out = sandbox.run_env(
        Some(&mock),
        &["--list-models", "nothing-matches"],
        &[("OPENAI_MODEL", "test-model")],
    );
    assert!(out.status.success(), "an empty search still exits 0");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("no models matching \"nothing-matches\""),
        "it says so: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// Verifies: gh #8 acceptance (3) - `--thinking` takes pi's vocabulary
// and nothing else: an unknown level is a usage error at parse time
// (exit 2), never a session that silently runs on something else.
#[cfg(unix)]
#[test]
fn thinking_refuses_a_level_outside_pis_vocabulary() {
    let sandbox = sandbox("thinking-vocabulary");
    let out = sandbox.run(None, &["--thinking", "hihg"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--thinking"), "names the flag: {stderr}");
    assert!(stderr.contains("hihg"), "echoes the bad value: {stderr}");
}

// Verifies: gh #8 (pi 1.0.0 parity) - `--provider` exists to scope the
// `--model` lookup, so it refuses to run without one: exit 2 (usage) and
// a message that names the rule, not a flag that quietly does nothing.
#[cfg(unix)]
#[test]
fn provider_without_a_model_is_a_usage_error_that_names_the_rule() {
    let sandbox = sandbox("provider-needs-model");
    let out = sandbox.run(None, &["--provider", "zen"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--provider"), "names the flag: {stderr}");
    assert!(stderr.contains("--model"), "names what it needs: {stderr}");
}
