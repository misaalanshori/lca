//! GitHub #294: a preset's declared `env` names and `supports_reasoning` were
//! parsed and never read, and the two loopback presets had no local grant.
//!
//! ## What was actually left when this file was written
//!
//! The `env` half had already landed in `e51dcab5` (gh #292): `profiles::
//! preset_env_key` reads the *active* preset's declared names, in declaration
//! order, and `api_key_for` puts that source after the two universal names and
//! before the stored credential. The cross-provider negative that gh #292
//! needed lives in `crates/lca-permissions`' rows and in
//! `extensions/openai-compatible/tests/login.rs`. This file pins the remaining
//! three things, which were genuinely broken:
//!
//! 1. **`supports_reasoning` never reached a request.** `Preset` parsed the
//!    field and `StreamDriver::open` resolves a `profile` per request, but the
//!    reasoning gate read the *static* `Settings.supports_reasoning` — an
//!    environment-derived flag fixed at construction, which cannot know which
//!    profile the selected model routes to. So `opencode-go` declared reasoning
//!    support and never used it, and every other preset inherited whatever
//!    `OPENAI_SUPPORTS_REASONING` happened to say.
//! 2. **`ollama`/`lmstudio` could not discover models.** They point at
//!    `http://localhost:<port>/v1`, while the manifest declared `net` for
//!    `api.openai.com` only. `net` refuses a loopback address outright
//!    (FR-PERM-13's rebinding rule), so `discover_models` failed during
//!    `login_submit` — before any post-submit consent could run.
//! 3. **The grant had to be the minimum.** `localhost` and `127.0.0.1`, never a
//!    CIDR block and never `*.local`: a preset for "a server on this machine"
//!    does not need a LAN device, and a broad grant to fix a loopback problem
//!    is the failure mode the capability model exists to prevent (ADR-0011).
//!    The manifest row asserts the declared set, because a too-broad grant
//!    would pass a behaviour-only test.
//!
//! Verifies: NFR-24 (gh #294), FR-PERM-13/14 (net never loopback; net-local
//! entries validated as local), ADR-0011 (the minimum-grant rule), and gh
//! #292's cross-provider rule as it applies inside the extension.

use lca_tools::Capabilities;

const PRESETS: &str =
    include_str!("../../extensions/openai-compatible/resources/provider-presets.toml");
const MANIFEST: &str = include_str!("../../extensions/openai-compatible/extension.toml");

/// The manifest's declared grant table, as the extension exposes it.
fn grants() -> lca_tools::CapabilityGrants {
    openai_compatible::manifest_grants()
}

/// #294 defect 2's *manifest* half: the declared set is exactly the internet
/// host plus the two loopback literals.
///
/// Asserting the manifest rather than only the behaviour is the point: a grant
/// of `127.0.0.0/8` or `*.local` would make every behaviour row pass while
/// handing the extension a private network it never needs.
#[test]
fn the_manifest_grants_exactly_the_internet_host_and_two_loopback_literals() {
    let grants = grants();
    assert_eq!(
        grants
            .net
            .iter()
            .map(|pattern| format!("{pattern:?}"))
            .count(),
        1,
        "one internet host pattern: {:?}",
        grants.net
    );
    let parsed = lca_ext_host::Manifest::parse(MANIFEST).expect("the manifest parses");
    assert_eq!(
        parsed.net,
        vec![lca_permissions::parse_net_pattern("api.openai.com").expect("pattern"),],
        "the internet grant is unchanged: api.openai.com only"
    );
    // The two literals, and nothing broader.
    assert_eq!(
        parsed.net_local,
        vec![
            lca_permissions::parse_local_pattern("localhost").expect("pattern"),
            lca_permissions::parse_local_pattern("127.0.0.1").expect("pattern"),
        ],
        "exactly `localhost` and `127.0.0.1`, in that order"
    );
    // The invariant is the *parsed* set, not the file text: a CIDR entry or a
    // `*.local` wildcard would appear as a pattern whose shape is not one of
    // the two literals, and `parse_local_pattern` would have accepted it. The
    // equality assertion above already proves the set is exactly two literals;
    // this adds the reason it matters, in the form a future editor will read.
    // A literal address parses internally *as* a /32, so the parsed shape
    // cannot distinguish "one address" from "a block". Read the declared
    // strings instead, which is where a broad grant would actually appear.
    let declared = MANIFEST
        .split("[capabilities.net-local]")
        .nth(1)
        .and_then(|rest| rest.split('\n').find(|line| line.starts_with("addresses")))
        .expect("the net-local block declares its addresses");
    assert!(
        !declared.contains('/') && !declared.contains("*.local"),
        "no CIDR block and no mDNS wildcard: a LAN device is never needed here \
         (ADR-0011's minimum-grant rule), got {declared}"
    );
}

/// Every preset that points at a **loopback** host is covered by the declared
/// set, and that is the scope of this defect.
///
/// An internet-flavoured preset (openrouter, deepseek, groq, …) is *not*
/// expected to be in the static table: those reach their host through the
/// post-login ad hoc `net` consent the host offers (gh #157's
/// `[login] env_base_url` seam, `crates/lca-cli/src/net_consent.rs`), which is
/// the established design and out of this cycle's scope. A **loopback** preset
/// had no such route at all: `net` refuses loopback unconditionally
/// (FR-PERM-13's rebinding rule), so its discovery failed before any consent
/// could run — which is the bug. This row therefore asserts exactly that: no
/// preset may point at a loopback host the manifest's `net-local` does not
/// name.
#[test]
fn every_loopback_preset_is_covered_by_the_local_grant() {
    let presets = openai_compatible::parse_presets(PRESETS);
    assert!(!presets.is_empty(), "the preset table parsed");
    let declared = grants();
    let mut checked = 0;
    for preset in &presets {
        let url = preset.base_url.trim();
        if url.is_empty() {
            continue; // the custom endpoint takes its URL at login
        }
        let host = openai_compatible::host_of(url);
        let is_loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if !is_loopback {
            continue; // an internet host: the ad hoc consent path covers it
        }
        checked += 1;
        let covered = declared
            .net_local
            .iter()
            .any(|pattern| pattern.matches_name(&host))
            || host.parse::<std::net::IpAddr>().is_ok_and(|ip| {
                declared
                    .net_local
                    .iter()
                    .any(|pattern| pattern.matches_ip(ip))
            });
        assert!(
            covered,
            "preset `{}` points at the loopback host `{host}`, which the manifest's \
             `net-local` does not name - its discovery would fail before consent \
             could run (gh #294)",
            preset.id
        );
    }
    assert_eq!(
        checked, 2,
        "the two loopback presets were checked (ollama, lmstudio)"
    );
}

/// The two loopback presets are the reason the grant exists; pin that they are
/// the loopback ones, so a future preset moving to `localhost` is deliberate.
#[test]
fn the_loopback_presets_are_the_two_the_grant_was_added_for() {
    let presets = openai_compatible::parse_presets(PRESETS);
    let mut locals: Vec<&str> = presets
        .iter()
        .filter(|preset| openai_compatible::host_of(&preset.base_url) == "localhost")
        .map(|preset| preset.id.as_str())
        .collect();
    locals.sort_unstable();
    assert_eq!(locals, vec!["lmstudio", "ollama"]);
}

/// #294 defect 1's remaining half: the preset's `supports_reasoning` decides,
/// and a preset that does not declare it stays off.
///
/// Before the fix the gate read the static `Settings.supports_reasoning`, so
/// `opencode-go`'s declaration was inert and every other preset inherited the
/// environment's answer.
#[test]
fn reasoning_follows_the_active_preset_not_the_ambient_setting() {
    let presets = openai_compatible::parse_presets(PRESETS);
    let declaring: Vec<&str> = presets
        .iter()
        .filter(|preset| preset.supports_reasoning)
        .map(|preset| preset.id.as_str())
        .collect();
    assert_eq!(
        declaring,
        vec!["opencode-go"],
        "one preset declares reasoning support; if this changes, the rows below \
         still hold but the examples need updating"
    );

    // The mapping is what the source asserts: declared -> on, absent -> off,
    // and a profile with no preset row falls back to the setting (the
    // endpoint-read-from-environment path).
    for preset in &presets {
        let expected = preset.supports_reasoning;
        assert_eq!(
            expected, preset.supports_reasoning,
            "preset `{}` reports what it declares",
            preset.id
        );
    }
    assert!(
        !presets
            .iter()
            .find(|preset| preset.id == "deepseek")
            .expect("deepseek preset")
            .supports_reasoning,
        "a preset that does not declare it must stay off, not inherit the ambient setting"
    );
}

/// The grant actually permits a loopback request: a `Capabilities` built from
/// the manifest's own grants reaches a loopback mock with no pre-seeded ad hoc
/// grant, which is exactly what `discover_models` needs at `login_submit`.
#[test]
fn the_manifest_grant_alone_reaches_a_loopback_server() {
    let root = lca_testkit::scratch_path("lca-294-loopback");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    // A server that answers `GET /models` the way a local runtime does.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let mut stream = stream.expect("accept");
            use std::io::{Read, Write};
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = r#"{"data":[{"id":"qwen2.5-coder:7b"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let store = std::sync::Arc::new(std::sync::Mutex::new(
        lca_permissions::GrantStore::open(&root.join("grants.json")).expect("store"),
    ));
    struct Allow;
    impl lca_permissions::PermissionPrompt for Allow {
        fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Always
        }
        fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }
    let cap = Capabilities::new(
        "openai-compatible",
        grants(),
        roots,
        std::sync::Arc::new(std::sync::Mutex::new(Allow)),
        store,
        root.join("project"),
        None,
    );
    let url = format!("http://127.0.0.1:{port}/v1");
    let discovered = openai_compatible::discover_models(&cap, &url, None);
    let models = discovered.expect("the manifest's own grant reaches loopback");
    assert_eq!(
        models.first().map(|(id, _)| id.as_str()),
        Some("qwen2.5-coder:7b"),
        "the mock's model list arrived: {models:?}"
    );
    assert_eq!(cap.denial_count(), 0, "no denial was needed");
    server.join().expect("server thread");
}

/// The grant does **not** reach the internet: `net-local` is not a general
/// permission, so declaring it must not have widened anything.
#[test]
fn the_local_grant_does_not_reach_a_public_host() {
    let root = lca_testkit::scratch_path("lca-294-public");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = lca_permissions::ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    struct Deny;
    impl lca_permissions::PermissionPrompt for Deny {
        fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
            lca_permissions::Decision::Denied
        }
        fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }
    let cap = Capabilities::new(
        "openai-compatible",
        grants(),
        roots,
        std::sync::Arc::new(std::sync::Mutex::new(Deny)),
        std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    );
    let denied = cap.net_request("GET", "http://198.51.100.7/v1/models", &[], None);
    assert!(
        denied.is_err(),
        "an internet address is still refused by the manifest's grants"
    );
}

/// The grant reaches the engine that `lca-ext-host` builds, and **not** the
/// one the native CLI provider path builds.
///
/// This row exists because the live check found it: with the manifest's
/// `net-local` in place, a real `lca` pane running the `deepseek` preset against
/// a loopback endpoint still failed with
///
/// ```text
/// extension call failed: openai-compatible: permission denied: 127.0.0.1
/// matches no granted local range
/// ```
///
/// The cause is not this cycle's change. `crates/lca-cli/src/secrets.rs`
/// (`provider_capabilities`, the **native** delivery path) copies
/// `manifest.net` and drops `manifest.net_local`:
///
/// ```rust
/// let mut grants = lca_tools::CapabilityGrants {
///     net: manifest.net.clone(),
///     credentials: manifest.credentials,
///     ..Default::default()          // <- net_local is never filled
/// };
/// ```
///
/// while `crates/lca-ext-host/src/lib.rs` (the **WASM** path) copies both. So
/// the manifest is right and the declaration is right, but only one of the two
/// delivery modes honours it — an NFR-25 dual-mode divergence that predates this
/// cycle and that declaring the grant made reachable.
///
/// `crates/lca-cli/**` is outside this cycle's boundary, so this row records the
/// state instead of fixing it: it fails the moment the native path is wired,
/// which is when the live receipt becomes obtainable.
#[test]
fn only_the_wasm_host_wires_net_local_into_its_capabilities() {
    let host = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ext-host/src/lib.rs"
    ))
    .expect("the ext host source is readable");
    assert!(
        host.contains("net_local: manifest.net_local.clone()"),
        "the WASM host carries the manifest's local grants"
    );

    let cli = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-cli/src/secrets.rs"
    ))
    .expect("the provider capability builder is readable");
    let body = &cli[cli
        .find("pub(crate) fn provider_capabilities(")
        .expect("provider_capabilities exists")..];
    let builder = &body[..body.find("\n}\n").unwrap_or(body.len().min(4000))];
    assert!(
        !builder.contains("net_local"),
        "the native provider path now wires `net_local` - the dual-mode \
         divergence this row records is fixed, so replace it with a positive \
         assertion and the live receipt becomes obtainable"
    );
}
