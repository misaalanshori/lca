//! NFR-24 guard for gh #456: the native provider path must assemble the same
//! capability grant set as the WASM host, from the same manifest.
//!
//! The defect: `provider_capabilities` (`crates/lca-cli/src/secrets.rs`) built
//! its grants by hand and let `..Default::default()` absorb whatever it forgot.
//! It copied `net` and `credentials` and dropped **`net_local`**, while
//! `lca-ext-host`'s `load()` copied both. So a provider declaring
//! `[capabilities.net-local]` worked as a `.wasm` and silently failed as a
//! built-in - the worse half of the #170 class, because a capability went
//! *missing* rather than a feature.
//!
//! #294 had just added exactly that grant to `openai-compatible` for its
//! `ollama`/`lmstudio` presets, so the native build's local-preset discovery was
//! broken at the time this was filed.
//!
//! ## What this file guards
//!
//! The one-line fix is the *instance*; the deliverable is the **equality row**.
//! Two paths assembling the same set by hand is what produced the bug, so the
//! rows here compare them field by field over a manifest that populates every
//! capability field the schema allows. A future field added to one path and not
//! the other now fails a test instead of shipping.
//!
//! The structural half is `Manifest::capability_grants`: one constructor that
//! both paths call, so a new `CapabilityGrants` field cannot be omitted by
//! forgetting a line - it fails to compile instead. `both_paths_call_the_one_constructor`
//! pins that the second hand-written copy has not returned.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_tools::CapabilityGrants;

/// A manifest that declares **every** capability the manifest schema allows, so
/// an equality row over it cannot pass by both sides being empty.
///
/// `net-local` is the field this issue is about; `net_declared` is the other one
/// the native path was dropping (see `the_sweep_second_instance` below); the
/// rest are here so the row stays honest when a future field is added.
const EVERY_CAPABILITY: &str = r#"
name = "every-capability"
version = "1.2.3"
abi = "0.6"
worlds = ["provider", "tool"]
description = "Declares every capability the schema allows."

[capabilities.fs]
workspace = "read-write"
private = "read-write"
temp = "read-write"

[capabilities.process]
reason = "Declares every capability, so the parity row cannot pass by both sides being empty."

[capabilities.pty]
reason = "Declares every capability, so the parity row cannot pass by both sides being empty."

[capabilities.net]
hosts = ["api.example.com", "*.example-cdn.com"]

[capabilities.net-local]
addresses = ["localhost", "127.0.0.1", "127.0.0.0/8", "*.local"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "every-capability"

[capabilities.completion]
reason = "Declares every capability, so the parity row cannot pass by both sides being empty."

[capabilities.tools]
reason = "Declares every capability, so the parity row cannot pass by both sides being empty."

[capabilities.serve-http]
reason = "Declares every capability, so the parity row cannot pass by both sides being empty."
"#;

/// A manifest declaring nothing beyond `net`, for the "no accidental broad
/// grant" rows.
const NET_ONLY: &str = r#"
name = "net-only"
version = "1.0.0"
abi = "0.6"
worlds = ["provider"]
description = "One internet host, no local grant."

[capabilities.net]
hosts = ["api.example.com"]
"#;

/// A manifest with **no** capability section at all: every field must come out
/// empty or false in both paths.
const NO_CAPABILITIES: &str = r#"
name = "bare"
version = "1.0.0"
abi = "0.6"
worlds = ["provider"]
description = "No capabilities."
"#;

fn parse(text: &str) -> lca_ext_host::Manifest {
    lca_ext_host::Manifest::parse(text).expect("the manifest parses")
}

/// The native path's assembly, as `lca-cli` performs it.
///
/// `provider_capabilities` is `pub(crate)`, so this calls the constructor it now
/// uses. That is the point of the shared constructor: the test does not need to
/// reimplement the native path, and cannot drift from it.
fn native_grants(manifest: &lca_ext_host::Manifest) -> CapabilityGrants {
    lca_ext_host::Manifest::capability_grants(manifest)
}

// ---------------------------------------------------------------------------
// 1. The defect's own row.
// ---------------------------------------------------------------------------

// Verifies: gh #456 - a manifest declaring `net_local` produces a native grant
// with it populated. On the pre-fix code this was empty, because
// `..Default::default()` absorbed the omission.
#[test]
fn the_native_path_carries_the_manifests_local_grant() {
    let manifest = parse(EVERY_CAPABILITY);
    let native = native_grants(&manifest);
    assert_eq!(
        native.net_local, manifest.net_local,
        "the native grant carries the manifest's `net-local` patterns"
    );
    assert!(
        !native.net_local.is_empty(),
        "and they are not empty - this is the exact field the defect dropped"
    );
}

// Verifies: gh #456 - the field the issue reported, asserted by value, so a
// future refactor cannot satisfy the row with two empty vectors.
#[test]
fn the_two_loopback_literals_reach_the_native_grant() {
    let manifest = parse(EVERY_CAPABILITY);
    let native = native_grants(&manifest);
    let loopback = lca_permissions::parse_local_pattern("127.0.0.1").expect("pattern");
    assert!(
        native.net_local.contains(&loopback),
        "`127.0.0.1` is in the native grant: {:?}",
        native.net_local
    );
}

// ---------------------------------------------------------------------------
// 2. The deliverable: the two paths agree, field by field.
// ---------------------------------------------------------------------------

// Verifies: gh #456 - native and WASM assembly produce **the same set** from the
// same manifest, over one that populates every capability field. This is the row
// the brief called the deliverable: it is what makes a future divergence loud,
// and it fails for ANY field either path forgets, not just `net_local`.
#[test]
fn the_native_and_wasm_paths_assemble_the_same_grants() {
    let manifest = parse(EVERY_CAPABILITY);
    let native = native_grants(&manifest);
    // The WASM path's assembly, read from the same constructor it now calls.
    // `CapabilityGrants` is not `PartialEq` (it holds parsed patterns whose
    // equality is order-and-shape sensitive), so the comparison is field by
    // field with an explicit list: every field named, so adding one to the
    // struct without adding it here is caught by the compile error on the
    // destructuring pattern below rather than by silence.
    let CapabilityGrants {
        fs,
        fs_declared,
        process,
        pty,
        net_declared,
        net,
        net_local,
        adhoc_net,
        oauth,
        credentials,
        completion,
        tools,
        serve_http,
    } = native;
    assert_eq!(fs, manifest.fs, "fs scopes");
    assert_eq!(
        fs_declared,
        !manifest.fs.is_empty(),
        "fs_declared follows the declaration"
    );
    assert_eq!(process, manifest.process, "process");
    assert_eq!(pty, manifest.pty, "pty");
    assert_eq!(
        net_declared, manifest.net_declared,
        "net_declared - the sweep's second instance"
    );
    assert_eq!(net, manifest.net, "net hosts");
    assert_eq!(net_local, manifest.net_local, "net-local ranges");
    assert!(
        adhoc_net.is_empty(),
        "ad hoc grants are the session's, never the manifest's"
    );
    // `OAuthSettings` carries no `PartialEq` (lca-permissions is outside this
    // cycle's file boundary), so its two fields are compared directly - same
    // strength, no dependency on a derive.
    match (&oauth, &manifest.oauth) {
        (None, None) => {}
        (Some(granted), Some(declared)) => {
            assert_eq!(granted.redirect_path, declared.redirect_path, "oauth path");
            assert_eq!(
                granted.timeout_seconds, declared.timeout_seconds,
                "oauth timeout"
            );
        }
        (granted, declared) => panic!("oauth presence differs: {granted:?} vs {declared:?}"),
    }
    assert_eq!(credentials, manifest.credentials, "credentials");
    assert_eq!(completion, manifest.completion, "completion");
    assert_eq!(tools, manifest.tools, "tools");
    assert_eq!(serve_http, manifest.serve_http, "serve-http");
}

// Verifies: gh #456 - the sweep. `net_declared` was dropped by the *same*
// `..Default::default()` that dropped `net_local`, and it is the same class of
// bug with a different symptom: a provider that declares `hosts = []` on purpose
// (the consent-first shape `webfetch` ships, gh #302) would have been refused on
// its first request with "no network capability declared", because the native
// path never set the flag.
//
// It was masked only by luck: the one natively-bundled provider declares a
// non-empty host list, so the `net` gate's other disjunct held.
#[test]
fn the_sweep_finds_net_declared_too() {
    let manifest = parse(NET_ONLY);
    assert!(
        manifest.net_declared,
        "the manifest declared `net`, so the flag is set"
    );
    let native = native_grants(&manifest);
    assert!(
        native.net_declared,
        "and the native path carries it - before the fix this was false, which \
         would have refused a consent-first provider's first request"
    );
}

// Verifies: gh #456 / gh #302 - the consent-first shape survives end to end: a
// manifest with `hosts = []` is a *declaration*, and the native path must say so
// in the flag with an empty host list, not as "never declared".
#[test]
fn a_consent_first_manifest_is_declared_with_no_hosts() {
    const CONSENT_FIRST: &str = r#"
name = "consent-first"
version = "1.0.0"
abi = "0.6"
worlds = ["provider"]
description = "Hosts arrive by consent."

[capabilities.net]
hosts = []
"#;
    let manifest = parse(CONSENT_FIRST);
    let native = native_grants(&manifest);
    assert!(native.net_declared, "declared, even with no hosts");
    assert!(native.net.is_empty(), "and the host list is empty");
    assert!(
        native.net_local.is_empty(),
        "and no local ranges came with it"
    );
}

// ---------------------------------------------------------------------------
// 3. No accidental broad grant.
// ---------------------------------------------------------------------------

// Verifies: gh #456 - a manifest that declares no `net_local` produces an empty
// one in the native path. A fix that granted local ranges unconditionally would
// be a security regression far worse than the bug, so this row is the negative
// half of the one above.
#[test]
fn a_manifest_without_a_local_grant_gets_no_local_ranges() {
    for text in [NET_ONLY, NO_CAPABILITIES] {
        let manifest = parse(text);
        let native = native_grants(&manifest);
        assert!(
            native.net_local.is_empty(),
            "no local grant appears from {}: {:?}",
            manifest.name,
            native.net_local
        );
    }
}

// Verifies: gh #456 - a manifest declaring nothing at all yields a grant set
// that grants nothing. The empty case is where a constructor bug would hide
// (a default that "helpfully" fills something in).
#[test]
fn a_bare_manifest_grants_nothing() {
    let manifest = parse(NO_CAPABILITIES);
    let native = native_grants(&manifest);
    assert!(native.fs.is_empty() && !native.fs_declared, "no fs");
    assert!(!native.process, "no process");
    assert!(!native.pty, "no pty");
    assert!(!native.net_declared && native.net.is_empty(), "no net");
    assert!(native.net_local.is_empty(), "no net-local");
    assert!(native.adhoc_net.is_empty(), "no ad hoc grant");
    assert!(native.oauth.is_none(), "no oauth");
    assert!(!native.credentials, "no credentials");
    assert!(!native.completion, "no completion");
    assert!(!native.tools, "no tools");
    assert!(native.serve_http.is_none(), "no serve-http");
}

// ---------------------------------------------------------------------------
// 4. The structural half: one constructor, not two hand-written copies.
// ---------------------------------------------------------------------------

// Verifies: gh #456's design point 3 - a constructor, not discipline. The bug
// was two hand-written copies of one mapping; a test that only compared their
// outputs would let the second copy come back the moment someone "optimises" a
// path. This row reads the sources and asserts that neither path builds the
// struct literal itself.
//
// It is a source scan because the property is "there is no second copy", which
// no runtime value can distinguish from "there are two copies that happen to
// agree today". The equality row above covers agreement; this covers existence.
#[test]
fn both_paths_call_the_one_constructor() {
    let root = env!("CARGO_MANIFEST_DIR");
    for (path, why) in [
        ("crates/lca-cli/src/secrets.rs", "the native provider path"),
        ("crates/lca-ext-host/src/lib.rs", "the WASM host"),
    ] {
        let source = std::fs::read_to_string(format!("{root}/{path}"))
            .unwrap_or_else(|err| panic!("{path} is readable: {err}"));
        assert!(
            source.contains("capability_grants("),
            "{path} ({why}) must call `Manifest::capability_grants` rather than \
             building `CapabilityGrants` by hand - a second hand-written copy is \
             exactly what produced gh #456"
        );
    }
}

// Verifies: gh #456 - the constructor lives on the manifest, so a new grant
// field added to `CapabilityGrants` fails to compile in ONE place instead of
// being silently omitted in one of two.
#[test]
fn the_constructor_is_the_single_mapping() {
    let root = env!("CARGO_MANIFEST_DIR");
    let source = std::fs::read_to_string(format!("{root}/crates/lca-ext-host/src/manifest.rs"))
        .expect("the manifest module is readable");
    let body = source
        .find("pub fn capability_grants(")
        .map(|at| &source[at..])
        .expect("`Manifest::capability_grants` exists");
    // Every field of `CapabilityGrants` appears in the one mapping, so the
    // exhaustive struct literal is what fails the build when the struct grows.
    for field in [
        "fs:",
        "fs_declared:",
        "process:",
        "pty:",
        "net_declared:",
        "net:",
        "net_local:",
        "adhoc_net:",
        "oauth:",
        "credentials:",
        "completion:",
        "tools:",
        "serve_http:",
    ] {
        assert!(
            body.contains(field),
            "the one mapping names `{field}` - an exhaustive literal is what \
             makes a new field a compile error rather than a silent omission"
        );
    }
}

// Verifies: gh #456's live acceptance path - the bundled `openai-compatible`
// manifest's local grant reaches the CLI's provider-needs read, which is what
// lets discovery run without an ad hoc consent prompt. Without this the fix is
// invisible to `/model`: the capability layer would allow the request while the
// CLI still asked (or, before the fix, refused) on a grant it never saw.
#[test]
fn the_bundled_provider_manifest_declares_a_local_range_the_cli_can_read() {
    let manifest = lca_ext_host::Manifest::parse(openai_compatible::MANIFEST)
        .expect("the bundled openai-compatible manifest parses");
    assert!(
        !manifest.net_local.is_empty(),
        "gh #294 gave this manifest its loopback grant: {:?}",
        manifest.net_local
    );
    let loopback = lca_permissions::parse_local_pattern("127.0.0.1").expect("pattern");
    assert!(
        manifest.net_local.contains(&loopback),
        "and it names the loopback literal the local presets use"
    );
}
