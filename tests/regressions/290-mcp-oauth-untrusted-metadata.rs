//! Gh #290: MCP OAuth sign-in and the untrusted metadata document.
//!
//! Two defects named in the issue, and the parts of them that #418 Phase D's
//! first pass left open. The behavioural rows live beside the code:
//!
//! - the cross-host journey, the scheme refusal, and the flow-scope discipline in
//!   `crates/lca-mcp/tests/oauth_multihost.rs`;
//! - the challenge parser in `crates/lca-mcp/src/lib.rs`'s own tests;
//! - the flow host bound in `crates/lca-tools/src/capabilities/net.rs`.
//!
//! This file is the NFR-24 named guard. Its job is to pin the four decisions a
//! green suite could otherwise lose without any test going red:
//!
//! 1. a challenge member is matched **case-insensitively** (RFC 7235), which is
//!    what stops the RFC 9728 branch from being dead again for a server that
//!    spells `Resource_Metadata`;
//! 2. the **loopback rule is a comparison, not a ban** - a loopback endpoint is
//!    refused only when the MCP server itself is remote;
//! 3. a **wildcard** authorization server is refused by *validation*, not by
//!    happening to match no grant;
//! 4. one flow's host set is **bounded**, and the bound is one number in one
//!    place.

use std::path::PathBuf;

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    &source[start..]
}

/// A source file from the workspace root (`CARGO_MANIFEST_DIR` is the root here).
fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #290 - the challenge parser lowercases every member name.
//
// This is the guard for "the branch cannot silently become dead again". The
// behavioural row (`a_challenge_member_is_matched_regardless_of_case`) proves the
// canonical spelling arrives; this one proves *how*, so a later edit that reaches
// for an exact `get("resource_metadata")` and an ad-hoc case fold at one call site
// is caught rather than multiplying the decision.
#[test]
fn the_challenge_parser_normalises_member_case() {
    let text = source("crates/lca-mcp/src/lib.rs");
    let body = function_body(&text, "pub fn parse_www_authenticate(");
    assert!(
        body.contains("to_ascii_lowercase()"),
        "the parser lowercases the member name:\n{body}"
    );
    assert!(
        body.contains("params.insert(name.trim().to_ascii_lowercase()"),
        "and it does so at insertion, so every reader sees one spelling:\n{body}"
    );
}

// Verifies: gh #290 design point 6 - the loopback rule compares the endpoint
// against the **MCP server's own** locality.
//
// The distinction is the whole rule: a loopback IdP is legitimate from a local
// server (RFC 8252) and an attack from a remote one. A guard phrased "refuse
// loopback" would pass every unit test about refusals and break every local
// development flow, so both halves are asserted here and both have behavioural
// rows on either side.
#[test]
fn the_loopback_rule_compares_the_server_and_the_endpoint() {
    let text = source("crates/lca-mcp/src/oauth.rs");
    let body = function_body(&text, "fn refuse_remote_loopback(");
    assert!(
        body.contains("loopback_of(&server.url)"),
        "a local MCP server is exempt:\n{body}"
    );
    assert!(
        body.contains("!loopback_of(endpoint)"),
        "a non-loopback endpoint is exempt:\n{body}"
    );
    assert!(
        body.contains("a remote server may not direct this machine to itself"),
        "and the refusal says why rather than just no:\n{body}"
    );
}

// Verifies: gh #290 design point 5 - the wildcard refusal is **validation**.
//
// The behavioural row had to be tightened once already: with the guard removed
// the sign-in still failed, because `*.example.com` matches no granted pattern
// either, so an assertion that merely said "it failed and named the URL" passed
// on the permission denial the rule exists to prevent. This guard is the
// structural half: the check runs on the issuer string, before a grant is built.
#[test]
fn a_wildcard_issuer_is_refused_by_validation() {
    let text = source("crates/lca-mcp/src/oauth.rs");
    let body = function_body(&text, "fn discover(");
    let wildcard = body
        .find("issuer.contains('*')")
        .unwrap_or_else(|| panic!("the wildcard check exists in discover:\n{body}"));
    let validate = body
        .find("checked_endpoint(issuer")
        .unwrap_or_else(|| panic!("the issuer is validated in discover:\n{body}"));
    assert!(
        wildcard < validate,
        "the wildcard is refused before the URL is validated (and long before a grant):\n{body}"
    );
    assert!(
        body.contains("a sign-in may only be sent to a host the document states exactly"),
        "and the refusal states the rule:\n{body}"
    );
}

// Verifies: gh #290 design point 5 - the host bound is one number, defined where
// it is enforced and mirrored where it is documented, with a row pinning them
// equal.
//
// Two constants is a drift risk taken deliberately: `lca-tools` is the layer that
// enforces the cap and `lca-mcp` is the layer whose metadata is the input, and a
// caller reading the bound should not have to reach through the capability engine
// to learn it. The pin is what makes the pair safe.
#[test]
fn the_flow_host_bound_is_enforced_and_mirrored() {
    let net = source("crates/lca-tools/src/capabilities/net.rs");
    let body = function_body(&net, "pub fn oauth_flow_attach(");
    assert!(
        body.contains("merged.len() > MAX_OAUTH_FLOW_HOSTS"),
        "the enforcement compares the merged set against the cap:\n{body}"
    );
    assert!(
        body.contains("return Err("),
        "and exceeding it is a refusal, not a truncation:\n{body}"
    );
    assert!(
        !body.contains(".truncate("),
        "a silently shortened list would fail later at a request naming no reason:\n{body}"
    );

    let documented = source("crates/lca-mcp/src/oauth.rs");
    assert!(
        documented.contains("pub const MAX_FLOW_HOSTS: usize = 8;"),
        "the documented mirror states the number"
    );
    let enforcing = source("crates/lca-tools/src/capabilities/mod.rs");
    assert!(
        enforcing.contains("pub const MAX_OAUTH_FLOW_HOSTS: usize = 8;"),
        "and so does the enforcing one"
    );
}

// Verifies: gh #290's lifecycle - a sign-in consumes the persisted challenge on
// **every** path, not only the successful one.
//
// `exchange` cleared it after a successful token exchange, so the clean path was
// never the hole; a sign-in that failed before the exchange left the pointer
// behind. The row that matters is behavioural
// (`a_failed_sign_in_also_consumes_the_challenge`); this guard pins that the
// clearing is in `sign_in` after `sign_in_flow` returns, so it cannot be moved
// back inside the success path.
#[test]
fn a_sign_in_consumes_the_challenge_on_every_path() {
    let text = source("crates/lca-mcp/src/oauth.rs");
    let body = function_body(&text, "pub fn sign_in(");
    let flow = body
        .find("sign_in_flow(")
        .unwrap_or_else(|| panic!("sign_in runs the flow:\n{body}"));
    let clear = body
        .find("store.clear_pending()")
        .unwrap_or_else(|| panic!("sign_in consumes the challenge:\n{body}"));
    assert!(
        clear > flow,
        "the consume runs after the flow returns, so it covers failures too:\n{body}"
    );
    assert!(
        body.contains("outcome"),
        "and the flow's outcome is still what the caller receives:\n{body}"
    );
}
