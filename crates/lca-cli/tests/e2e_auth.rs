//! `lca auth` end to end (gh #72, #38): the credential commands
//! against the bundled provider, which needs no network for any row
//! here — its identity probe answers locally.
//!
//! | command | state | exit | stdout |
//! |---|---|---|
//! | `check --provider nope` | unknown provider | 1 | `not_ready` |
//! | `check` (no selector) | usage error | 2 | stderr names the rule |
//! | `check --provider openai-compatible` | no key anywhere | 1 | `not_ready` |
//! | `check --provider openai-compatible` | `OPENAI_API_KEY` set | 0 | `ready` |
//! | `login/logout` | env key | 0, and the key persists for later checks |
//! | `print-api-key`, `print-bearer-token` | always | refused, secrets stay out of scrollback |
//!
//! The OAuth rows (paste-callback exchange, PKCE pin, expired-token
//! refresh) need an OAuth provider, which this binary does not bundle;
//! they are guarded by the classifier unit tests beside the
//! implementation and proven live by the mock-IdP receipt.

mod common;

#[cfg(unix)]
use common::*;

#[cfg(unix)]
fn check_out(output: &std::process::Output) -> (i32, String, String) {
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

// Verifies: gh #72 - an unknown provider is missing credentials, not a
// usage error: exit 1 with pi's `not_ready`, and `--json` carries pi's
// `provider_not_found` reason.
#[cfg(unix)]
#[test]
fn auth_check_reports_not_ready_for_an_unknown_provider() {
    let box_ = sandbox("auth-unknown-provider");
    let (code, stdout, _) = check_out(&box_.run(None, &["auth", "check", "--provider", "nope"]));
    assert_eq!(code, 1, "missing credentials exit 1");
    assert_eq!(stdout.trim(), "not_ready");

    let (code, stdout, _) =
        check_out(&box_.run(None, &["auth", "check", "--provider", "nope", "--json"]));
    assert_eq!(code, 1);
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON shape");
    assert_eq!(result["status"], "not_ready");
    assert_eq!(result["provider"], "nope");
    assert_eq!(result["reason"], "provider_not_found");
}

// Verifies: gh #72 - pi requires `--provider` or `--model` on every
// auth command; without either the check is a usage error (exit 2).
#[cfg(unix)]
#[test]
fn auth_check_without_a_selector_is_a_usage_error() {
    let box_ = sandbox("auth-no-selector");
    let (code, _, stderr) = check_out(&box_.run(None, &["auth", "check"]));
    assert_eq!(code, 2, "a missing selector exits 2");
    assert!(
        stderr.contains("--provider") && stderr.contains("--model"),
        "the rule is named: {stderr}"
    );
}

// Verifies: gh #72 - a model that resolves to nothing is pi's
// `invalid_state` (exit 2), the same bucket a throwing `checkAuth`
// lands in.
#[cfg(unix)]
#[test]
fn auth_check_with_an_unresolvable_model_is_invalid() {
    let box_ = sandbox("auth-bad-model");
    let (code, stdout, _) = check_out(&box_.run(
        None,
        &["auth", "check", "--model", "no-such-model-xyz", "--json"],
    ));
    assert_eq!(code, 2, "an unresolvable model exits 2");
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON shape");
    assert_eq!(result["status"], "invalid");
    assert_eq!(result["reason"], "invalid_state");
}

// Verifies: gh #72 - with no key in the environment and none stored,
// the bundled provider reports missing credentials (exit 1), and the
// JSON names `credentials_not_configured` without leaking anything.
#[cfg(unix)]
#[test]
fn auth_check_reports_not_ready_without_a_key() {
    let box_ = sandbox("auth-no-key");
    let (code, stdout, _) =
        check_out(&box_.run(None, &["auth", "check", "--provider", "openai-compatible"]));
    assert_eq!(code, 1);
    assert_eq!(stdout.trim(), "not_ready");

    let (code, stdout, _) = check_out(&box_.run(
        None,
        &["auth", "check", "--provider", "openai-compatible", "--json"],
    ));
    assert_eq!(code, 1);
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON shape");
    assert_eq!(result["status"], "not_ready");
    assert_eq!(result["provider"], "openai-compatible");
    assert_eq!(result["reason"], "credentials_not_configured");
    assert_eq!(result["authType"], "api_key");
    assert!(result.get("credentials").is_none(), "no secret rides along");
}

// Verifies: gh #72 - an environment key resolves without touching the
// network: exit 0 with `ready`, and the JSON carries the auth type.
#[cfg(unix)]
#[test]
fn auth_check_reports_ready_with_an_env_key() {
    let box_ = sandbox("auth-env-key");
    let (code, stdout, _) = check_out(&box_.run_env(
        None,
        &["auth", "check", "--provider", "openai-compatible", "--json"],
        &[("OPENAI_API_KEY", "sk-test")],
    ));
    assert_eq!(code, 0, "a configured key exits 0");
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON shape");
    assert_eq!(result["status"], "ready");
    assert_eq!(result["provider"], "openai-compatible");
    assert_eq!(result["authType"], "api_key");
    assert!(result.get("reason").is_none(), "ready carries no reason");
}

// Verifies: gh #72 - API-key login completes headlessly from the
// environment, persists the key (a later check with no environment
// still reads `ready`), and logout clears it back to `not_ready`.
#[cfg(unix)]
#[test]
fn auth_login_logout_round_trip_through_the_environment() {
    let box_ = sandbox("auth-round-trip");
    let (code, _, stderr) = check_out(&box_.run_env(
        None,
        &["auth", "login", "--provider", "openai-compatible"],
        &[("OPENAI_API_KEY", "sk-test")],
    ));
    assert_eq!(code, 0, "login from the environment works: {stderr}");

    // The key persisted: no environment, still ready.
    let (code, stdout, _) =
        check_out(&box_.run(None, &["auth", "check", "--provider", "openai-compatible"]));
    assert_eq!((code, stdout.trim()), (0, "ready"));

    let (code, _, stderr) =
        check_out(&box_.run(None, &["auth", "logout", "--provider", "openai-compatible"]));
    assert_eq!(code, 0, "logout works: {stderr}");

    let (code, stdout, _) =
        check_out(&box_.run(None, &["auth", "check", "--provider", "openai-compatible"]));
    assert_eq!((code, stdout.trim()), (1, "not_ready"));
}

// Verifies: gh #72 - the credential printers are refused, not merely
// missing: the secrets law outranks parity, so both names fail with the
// reason instead of a usage dump.
#[cfg(unix)]
#[test]
fn auth_credential_printers_are_refused() {
    let box_ = sandbox("auth-refused");
    for sub in ["print-api-key", "print-bearer-token"] {
        let (code, _, stderr) =
            check_out(&box_.run(None, &["auth", sub, "--provider", "openai-compatible"]));
        assert_ne!(code, 0, "{sub} never succeeds");
        assert!(
            stderr.contains("never prints credentials"),
            "{sub} names the refusal: {stderr}"
        );
    }
}
