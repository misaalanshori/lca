//! gh #567 - `lib.rs` is split by OAuth concern, `DeviceAuthorization` is closed
//! to outside literals, and every public path resolves.
//!
//! Measured: the file was **897** lines (the issue said 817). It held six
//! concerns, and the split follows them: `spec` (the `OAuthSpec` table and the
//! credential-key readers), `crypto` (PKCE/base64url), `transport` (the bounded
//! POSTs and the clock), `token` (the credential lifecycle and account
//! extraction), `login` (the loopback flow), `device` (RFC 8628). `lib.rs` is the
//! barrel: module declarations plus the `pub use` list that keeps every caller's
//! path (`lca_subscription::run_login`, `pkce_pair`, `post_json`, ...) unchanged.
//!
//! Verifies: gh #567 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #567 - the split is the OAuth partition, each file owns one
// concern, and the barrel re-exports so no public path moved.
#[test]
fn the_subscription_kit_is_split_by_oauth_concern() {
    let module = crate::module_sources("crates/lca-subscription/src/lib.rs");
    let barrels: Vec<&str> = module
        .iter()
        .filter(|(path, _)| path.ends_with("src/lib.rs"))
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(barrels.len(), 1, "one crate root");

    // Each concern is its own file, and each one is under the ceiling with room.
    for (file, marker) in [
        ("spec.rs", "pub struct OAuthSpec"),
        ("crypto.rs", "pub fn pkce_pair"),
        ("transport.rs", "pub fn post_json"),
        ("token.rs", "pub fn access_token"),
        ("login.rs", "pub fn run_login"),
        ("device.rs", "pub struct DeviceAuthorization"),
    ] {
        let entry = module
            .iter()
            .find(|(path, _)| path.ends_with(file))
            .unwrap_or_else(|| panic!("`{file}` exists"));
        assert!(entry.1.contains(marker), "`{file}` owns {marker:?}");
        assert!(
            entry.1.lines().count() < 400,
            "`{file}` is {} lines, well clear of gate 11",
            entry.1.lines().count()
        );
    }

    // The barrel lists the six modules and re-exports their public surface.
    let root = barrels[0];
    for name in ["crypto", "device", "login", "spec", "token", "transport"] {
        assert!(
            root.contains(&format!("mod {name};")),
            "the barrel declares `{name}`"
        );
    }
    let reexports = root.split("pub use ").count().saturating_sub(1);
    assert!(
        reexports >= 6,
        "the barrel re-exports every public path it used to define ({reexports} `pub use`s)"
    );
    assert!(
        root.lines().count() < 120,
        "the crate root is a barrel now ({} lines)",
        root.lines().count()
    );
}

// Verifies: gh #567 - every public path the extensions and the tests name still
// resolves from the crate root. This is the "no public path renamed" receipt, and
// it is a compile-time check: the imports below would fail the suite otherwise.
#[test]
fn every_public_path_still_resolves_from_the_crate_root() {
    #[allow(unused_imports)]
    use lca_subscription::{
        AccountStrategy, DeviceAuthorization, DevicePollOutcome, IdentityFailure, IdentityOutcome,
        OAuthSpec, access_token, account_from_jwt, device_code_url, json_error_message, now_epoch,
        open_failure_event, percent_encode, pkce_pair, poll_device_code, poll_device_once,
        post_form_accept, post_json, post_json_with_headers, purge_tokens, request_device_code,
        run_login, run_logout,
    };

    // A smoke call through two of them, so this row cannot pass on imports alone.
    let (verifier, challenge) = pkce_pair().expect("PKCE pair");
    assert!(!verifier.is_empty() && !challenge.is_empty());
    assert_eq!(percent_encode("a b"), "a%20b");
    assert!(now_epoch() > 1_600_000_000);
}

// Verifies: gh #567 - `DeviceAuthorization` is closed and has the constructor,
// and the code it carries still round-trips through the URL builder.
#[test]
fn device_authorization_builds_through_its_constructor() {
    let device = read("crates/lca-subscription/src/device.rs");
    assert!(
        device.contains("#[non_exhaustive]\npub struct DeviceAuthorization"),
        "the struct is closed to outside literals (E0639), so RFC 8628's optional \
         `verification_uri_complete` can be added later"
    );
    assert!(
        device.contains("pub fn new("),
        "and the constructor replaces the literal for tests and fixtures"
    );
    assert!(
        device.contains("DeviceAuthorization::new("),
        "`request_device_code` builds through it, so there is one construction site"
    );

    let auth = lca_subscription::DeviceAuthorization::new(
        "d",
        "u",
        "https://example.invalid/device",
        5,
        30,
    );
    assert_eq!(auth.device_code, "d");
    assert_eq!(auth.interval_secs, 5);
    // The helper appends the `#code=` fragment (RFC 8628's pre-filled form), so
    // the assertion pins that the fields it reads are the ones just built.
    assert_eq!(
        lca_subscription::device_code_url(&auth),
        "https://example.invalid/device#code=u",
        "the URL helper reads the fields the constructor set"
    );
}
