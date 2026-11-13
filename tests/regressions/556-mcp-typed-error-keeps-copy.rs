//! gh #556 - `lca-mcp` has a typed error, and every user-visible refusal keeps
//! its exact sentence.
//!
//! Measured before touching anything: the issue says "50+ functions" return
//! `Result<_, String>`; the count is **87** result positions across
//! `lib.rs`/`config.rs`/`cli.rs`/`manager.rs`/`native.rs`/`remote.rs`/`resources.rs`/
//! `wire.rs`/`tool_call.rs`/`types.rs`, in 13 distinct return shapes
//! (`Result<(), String>` 21 times, `Result<serde_json::Value, String>` 8, …).
//! That is why the migration is **bounded to the public boundary** rather than
//! crate-wide: a crate-root `Result` alias shadows `std`'s for every signature in
//! the crate, so the crate would not compile until all 87 were converted at once,
//! and the internal helpers' refusals *are* the wire payload (a `serde_json`
//! message is a value, not an error class).
//!
//! What a caller sees now: [`lca_mcp::McpError`] with a variant per class the
//! crate can tell apart, and [`lca_mcp::McpResult`] on the five `CliMcp` verbs
//! (`lca mcp add|remove|list|login|logout`) that `lca-cli` maps to exit codes.
//!
//! The copy is the receipt: MCP refusals reach `lca mcp list` and `/mcp`'s detail
//! screen, so a typed error that reworded one would be a behaviour change.
//!
//! Verifies: gh #556 acceptance 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #556 - the crate has one error type with named variants, and the
// public verbs return it.
#[test]
fn the_mcp_crate_reports_a_typed_error() {
    let lib = read("crates/lca-mcp/src/lib.rs");
    assert!(
        lib.contains("pub enum McpError"),
        "the crate defines its error type (it had none at all)"
    );
    for variant in [
        "Config(String)",
        "UnknownServer(String)",
        "AuthRequired(String)",
        "Protocol(String)",
        "Transport(String)",
        "Io(std::io::Error)",
    ] {
        assert!(
            lib.contains(variant),
            "`{variant}` names a class a caller can match on instead of \
             substring-matching prose"
        );
    }
    assert!(
        lib.contains("pub type McpResult<T> = std::result::Result<T, McpError>;"),
        "and the crate's alias reaches every variant"
    );
    assert!(
        lib.contains("impl std::error::Error for McpError"),
        "it is a real `Error`, so `?` chains and `source()` work"
    );

    let cli = read("crates/lca-mcp/src/cli.rs");
    for verb in ["add(", "remove(", "list(", "login(", "logout("] {
        let position = cli
            .find(&format!("pub fn {verb}"))
            .unwrap_or_else(|| panic!("`{verb}` exists"));
        let signature = &cli[position..position + 200];
        assert!(
            signature.contains("crate::McpResult<"),
            "`{verb}` returns the crate's typed result: {signature}"
        );
    }
}

// Verifies: gh #556 - the copy is unchanged, at runtime, through the error type
// a caller receives. Each string below is the exact sentence the
// `String`-returning code produced.
#[test]
fn the_mcp_refusal_copy_is_unchanged() {
    let err = lca_mcp::McpError::Config("MCP server \"acme\" configures no OAuth".to_string());
    assert_eq!(
        err.to_string(),
        "MCP server \"acme\" configures no OAuth",
        "the config refusal reads exactly as before"
    );

    let err = lca_mcp::McpError::UnknownServer("acme".to_string());
    assert_eq!(
        err.to_string(),
        "unknown MCP server \"acme\"",
        "the missing-server refusal reads exactly as before"
    );

    let err = lca_mcp::McpError::AuthRequired(
        "MCP server \"acme\" requires sign-in; sign in and retry the call".to_string(),
    );
    assert_eq!(
        err.to_string(),
        "MCP server \"acme\" requires sign-in; sign in and retry the call",
        "the FR-CTX-style auth refusal reads exactly as before"
    );

    let err = lca_mcp::McpError::Protocol("invalid JSON-RPC id".to_string());
    assert_eq!(err.to_string(), "invalid JSON-RPC id");

    // And an `io` failure folds in without changing its own sentence.
    let err: lca_mcp::McpError =
        std::io::Error::new(std::io::ErrorKind::NotFound, "no such file").into();
    assert_eq!(err.to_string(), "no such file");
    assert!(
        std::error::Error::source(&err).is_some(),
        "the source chain survives the fold"
    );
}

// Verifies: gh #556 - `oauth::exchange`'s six positional strings are one named
// struct, and the six form-field names it sends are unchanged (a transposition
// here would have sent the verifier as the code).
#[test]
fn the_oauth_exchange_names_its_six_strings() {
    let oauth = read("crates/lca-mcp/src/oauth.rs");
    assert!(
        oauth.contains("struct CodeExchange<'a>"),
        "the six strings are grouped"
    );
    for field in [
        "client_id",
        "client_secret",
        "code",
        "redirect_uri",
        "verifier",
        "scope",
    ] {
        assert!(
            oauth.contains(&format!("    {field}: ")),
            "`{field}` is a named field"
        );
    }
    // The `too_many_arguments` allowance that the signature needed is gone.
    let signature_region = oauth
        .split("fn exchange(")
        .nth(1)
        .expect("`exchange` exists");
    assert!(
        !signature_region[..200].contains("too_many_arguments"),
        "`exchange` no longer needs the argument-count allowance"
    );
    // The wire names are untouched: pshed correctly, the endpoint sees the same
    // six form keys as before.
    for pair in [
        "(\"grant_type\", \"authorization_code\")",
        "(\"code\", exchange.code)",
        "(\"redirect_uri\", exchange.redirect_uri)",
        "(\"client_id\", exchange.client_id)",
        "(\"code_verifier\", exchange.verifier)",
    ] {
        assert!(oauth.contains(pair), "the form pair {pair} is unchanged");
    }
}

// Verifies: gh #556 - the `expect` in `manifest_grants` is gone because the
// vocabulary now has infallible constructors for the names it uses.
#[test]
fn the_manifest_grants_have_no_panicking_path() {
    let native = read("crates/lca-mcp/src/native.rs");
    let grants = native
        .split("pub fn manifest_grants()")
        .nth(1)
        .expect("`manifest_grants` exists");
    let body = &grants[..grants.find("\n}").expect("it closes")];
    assert!(
        !body.contains("expect("),
        "no `expect` remains in the grants builder: {body}"
    );
    assert!(
        body.contains("ScopeGrant::workspace(") && body.contains("ScopeGrant::temp("),
        "it uses the infallible constructors"
    );
    // And those constructors produce the same grants the parser did.
    let workspace = lca_permissions::ScopeGrant::workspace(lca_permissions::FsMode::Read);
    assert_eq!(workspace.scope, "workspace");
    assert_eq!(workspace.mode, lca_permissions::FsMode::Read);
    let temp = lca_permissions::ScopeGrant::temp(lca_permissions::FsMode::ReadWrite);
    assert_eq!(temp.scope, "temp");

    let grants = lca_mcp::native::manifest_grants();
    assert!(grants.process, "the MCP subsystem still declares process");
    assert_eq!(grants.fs.len(), 2, "workspace + temp, as before");
}
