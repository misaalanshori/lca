//! gh #467 - the origin fallback must be able to succeed, and its branch must be
//! covered.
//!
//! `origin_of` split a URL with `splitn(4, '/')` and rejected the **second**
//! piece. For `http://h:1/mcp` that piece is the empty string *between the two
//! slashes of `//`* - empty for every well-formed URL - so the guard fired on
//! exactly the inputs it was written for and the function could never succeed.
//!
//! It survived because it is the last resort in `discover`: the configured
//! metadata document first, the 401 challenge's RFC 9728 pointer second, then the
//! MCP server's own origin as its own issuer. Every existing row started at one of
//! the first two, so the third was never reached in a test. The platform trace made
//! that visible: the same `main` commit passed `test (macos-latest, shell)`'s
//! cross-host sign-in on Linux and failed it on macOS, because macOS actually
//! reached the fallback.
//!
//! Three things must stay true, and this file is the structural half:
//!
//! 1. **A well-formed URL has an origin.** The one row whose absence let this
//!    live. A guard that fires on valid input is not a guard.
//! 2. **The parse is a parse.** No `splitn` on `/` pretending to understand
//!    `://`; the authority ends at the path, query, or fragment, and userinfo
//!    never survives into a string that gets compared or written to a note.
//! 3. **The fallback branch is reachable from a row.** Not a unit assertion that
//!    `origin_of` works, but a `discover`-level row where no document and no
//!    pointer exist, so the fallback is the only path left.
//!
//! The behaviour rows live in `crates/lca-mcp/src/oauth.rs::origin_tests` (the
//! parse) and `crates/lca-mcp/tests/oauth.rs::discovery_falls_back_to_the_servers_own_origin`
//! (the branch). This file pins the shapes those rows cannot see from outside.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The body of `origin_of`, for text assertions.
fn origin_body() -> String {
    let source = read("crates/lca-mcp/src/oauth.rs");
    let start = source
        .find("fn origin_of(url: &str)")
        .expect("`origin_of` exists");
    let rest = &source[start..];
    // Up to the next top-level item: a `\n}\n` closing the function.
    let end = rest.find("\n}\n").expect("`origin_of` has a body");
    rest[..end].to_string()
}

// Verifies: gh #467 - the defect itself. The old body rejected the piece between
// the two slashes of `//`, which is empty for every valid URL, so this is the
// assertion that would have caught it before it shipped.
//
// Stated as a property of the *input shape* rather than of a call, because the
// failure mode was "the guard fires on valid input": a string with a scheme and
// an authority must not be refused for looking like a URL.
#[test]
fn the_old_splitn_guard_is_gone() {
    let body = origin_body();
    assert!(
        !body.contains("splitn(4"),
        "the `splitn(4, '/')` guard cannot succeed on any real URL and must not \\
         come back: {body}"
    );
    assert!(
        body.contains("split_once(\"://\")"),
        "the origin is found by splitting the scheme off, as `checked_endpoint` \\
         does: {body}"
    );
}

// Verifies: gh #467 design point 1 - the authority ends at the path, the query,
// or the fragment, whichever comes first.
//
// `split('/')` alone is the same class of mistake as `splitn(4, '/')`: it keeps
// `example.com?q=1` whole, so the origin would carry a query string and a grant
// comparison against `example.com` would miss.
#[test]
fn the_authority_is_cut_at_path_query_and_fragment() {
    let body = origin_body();
    assert!(
        body.contains("['/', '?', '#']"),
        "the authority ends at a path, query, or fragment: {body}"
    );
}

// Verifies: gh #467 design point 1 - userinfo is stripped. An origin that kept
// `user:pass@` would put a credential into a string that reaches a discovery note
// (and, in any future host check, a comparison).
#[test]
fn userinfo_is_stripped_from_the_origin() {
    let body = origin_body();
    assert!(
        body.contains("rsplit('@')"),
        "the authority keeps only what follows the last `@`: {body}"
    );
    assert!(
        !body.contains("format!(\"{scheme}://{authority}\")"),
        "the raw authority (userinfo and all) must not be the origin: {body}"
    );
}

// Verifies: gh #467 design point 1 - the shape validation is `checked_endpoint`'s,
// not a second opinion.
//
// An origin that disagreed with `checked_endpoint` about "which host is this"
// would be a quiet second answer to the same question: `checked_endpoint` already
// knows about userinfo, IPv6 brackets, and which schemes are allowed, and it is
// what `get_document` validates the metadata URL with.
#[test]
fn the_origin_agrees_with_the_endpoint_validator() {
    let body = origin_body();
    assert!(
        body.contains("checked_endpoint("),
        "the host (and whether the scheme is one we speak) comes from \\
         `checked_endpoint`: {body}"
    );
}

// Verifies: gh #467 design point 3 - the fallback branch is exercised by a row at
// the `discover` level, not only by a unit test of the helper.
//
// A unit test proves the helper works; it does not prove the branch that calls it
// is reachable, and the branch being unreachable *in tests* is precisely how this
// survived. The assertion is that such a row exists and that it configures the
// case that reaches the fallback: no metadata document, no pointer.
#[test]
fn a_row_reaches_the_fallback_branch() {
    let rows = read("crates/lca-mcp/tests/oauth.rs");
    let start = rows
        .find("fn discovery_falls_back_to_the_servers_own_origin()")
        .expect("the fallback row exists");
    let body = &rows[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    assert!(
        body.contains("unconfigured_server"),
        "the row builds a server with no configured document: {body}"
    );
    assert!(
        body.contains("Bearer\")") || body.contains("Some(\"Bearer\")"),
        "and a challenge with no `resource_metadata` pointer, so the fallback is \\
         the only path left: {body}"
    );

    // The fixture really is unconfigured - otherwise the row would exercise step
    // 1 and pass for the wrong reason, which is the bug being guarded against.
    let fixture_start = rows
        .find("fn unconfigured_server(")
        .expect("the unconfigured fixture exists");
    let fixture = &rows[fixture_start..];
    let fixture = &fixture[..fixture.find("\n}\n").unwrap_or(fixture.len())];
    assert!(
        fixture.contains("auth_server_metadata_url: None"),
        "no configured metadata document: {fixture}"
    );
}

// Verifies: gh #467 design point 4 - the `mcp_cli` sign-in row takes one path on
// every platform.
//
// It used to reach the configured/pointer steps on Linux and the fallback on
// macOS, which is what made a dead fallback invisible while CI was red on one OS
// and green on another. Naming the metadata document pins the documented first
// step; that the row no longer depends on platform-resolved hostnames is what
// keeps the two from meeting again.
#[test]
fn the_cli_sign_in_row_pins_its_discovery_step() {
    let common = read("crates/lca-cli/tests/common/mod.rs");
    let start = common
        .find("pub fn write_oauth_mcp_config(")
        .expect("the config writer exists");
    let body = &common[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    assert!(
        body.contains("authServerMetadataUrl"),
        "the row configures the documented metadata document, so discovery takes \\
         step 1 on every OS: {body}"
    );
    assert!(
        !body.contains("oauth-protected-resource"),
        "and does not rely on a challenge pointer to pick the path: {body}"
    );
}

// Verifies: gh #467 design point 6 and the dependency rule - the six-line parse
// needs no new crate.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("crates/lca-mcp/Cargo.toml");
    assert!(
        !manifest.contains("\nurl =") && !manifest.contains("\nurl="),
        "the origin parse is std-only; a URL crate is not pulled in for it: \\
         {manifest}"
    );
}
