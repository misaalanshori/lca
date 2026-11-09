//! The **shipped** components' own data paths, not the conformance script's: a
//! per-model context window from the shared table (gh #34), the Antigravity
//! catalog at real size inside the production fuel budget (gh #236), and the
//! provider's environment reads reaching the WASM guest (gh #461).
//!
//! Gh #552 split `tests/provider.rs` at its own subjects; the `Fixture` is defined
//! once, in `support.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_ext_abi::{ExtensionDispatch, World};
use lca_ext_host::{DEFAULT_FUEL_PER_CALL, ExtHost, ExtensionLimits};
use lca_ext_native::NativeRegistry;

use super::support::Fixture;

/// A committed first-party component must export every world its manifest
/// declares: the manifest schema says the host verifies each one, and a
/// drift is exactly the defect that shipped an `antigravity` artifact the
/// host refused to link (`no exported instance named
/// lca:ext/command-spec@1.0.0`). The loader's own world-link step is the
/// assertion - a component missing a declared world fails to load.
#[test]
fn every_first_party_component_exports_the_worlds_its_manifest_declares() {
    let cases: [(&str, &[u8], &str); 2] = [
        (
            "openai-compatible",
            include_bytes!("../../../../extensions/openai-compatible/fixtures/component.wasm"),
            include_str!("../../../../extensions/openai-compatible/extension.toml"),
        ),
        (
            "antigravity",
            include_bytes!("../../../../extensions/antigravity/fixtures/component.wasm"),
            include_str!("../../../../extensions/antigravity/extension.toml"),
        ),
    ];
    for (name, bytes, manifest) in cases {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            Fixture::new(&format!("manifest-{name}")).env(),
        );
        let handle = host
            .load(bytes, manifest)
            .unwrap_or_else(|err| panic!("`{name}` must link against its manifest: {err}"));
        assert!(
            handle.worlds().contains(&World::Provider),
            "`{name}` exports the provider world"
        );
    }
}

// The shipped provider's committed component - the conformance script
// above proves the mapping, this proves the real product's own data path.
const OAC_FIXTURE: &[u8] =
    include_bytes!("../../../../extensions/openai-compatible/fixtures/component.wasm");
const OAC_MANIFEST: &str = include_str!("../../../../extensions/openai-compatible/extension.toml");

// Verifies: gh #34 (NFR-25's model-listing half) - a per-model context
// window reaches `ModelInfo` in BOTH delivery modes, from the same table.
// The native handle resolves it through the shared resolver over its
// embedded bag; the installed component reads those very bytes back
// through the host's resource seam (ADR-0030/0032). A window only one
// mode knew would show `ctx ?` on one delivery path and a number on the
// other - the same defect, twice.
#[test]
fn context_windows_are_identical_across_modes() {
    let fixture = Fixture::new("gh34-windows");
    // `ExtHost::load` serves a component's bag from the installed package
    // directory, so lay the bytes out where the host looks: the same
    // three files the native side embeds.
    let bag = fixture
        .root
        .join("data/extensions/openai-compatible/resources");
    std::fs::create_dir_all(&bag).expect("bag dir");
    for (name, text) in [
        (
            "provider-presets.toml",
            include_str!(
                "../../../../extensions/openai-compatible/resources/provider-presets.toml"
            ),
        ),
        (
            "context-windows.toml",
            include_str!("../../../../extensions/openai-compatible/resources/context-windows.toml"),
        ),
        (
            "image-limits.toml",
            include_str!("../../../../extensions/openai-compatible/resources/image-limits.toml"),
        ),
    ] {
        std::fs::write(bag.join(name), text).expect("write bag");
    }

    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 100_000_000,
            log_limit_bytes: 4096,
        },
        fixture.env(),
    );
    let wasm = host
        .load(OAC_FIXTURE, OAC_MANIFEST)
        .expect("the shipped component links");

    let mut native_cap = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        fixture.roots.clone(),
        fixture.prompt.clone(),
        fixture.store.clone(),
        fixture.root.join("project"),
        None,
    );
    native_cap.set_resources(openai_compatible::resources());
    let native = openai_compatible::OpenAiCompat::with_settings(
        Arc::new(native_cap),
        openai_compatible::Settings {
            model: String::new(),
            context_window: 0,
            ..Default::default()
        },
    );

    let passed = [(
        "models".to_string(),
        "mimo-v2.6-flash,no-such-model".to_string(),
    )];
    let native_models = native.provider_models(&passed).expect("native models");
    let wasm_models = wasm.provider_models(&passed).expect("wasm models");
    assert_eq!(wasm_models, native_models, "the two modes agree");
    let window = |models: &[lca_protocol::ModelInfo], id: &str| {
        models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("`{id}` in the list: {models:?}"))
            .context_window
    };
    assert_eq!(
        window(&native_models, "mimo-v2.6-flash"),
        1_048_576,
        "the curated window, both modes: {native_models:?}"
    );
    assert_eq!(
        window(&native_models, "no-such-model"),
        0,
        "an unknown id reports no window in either mode: {native_models:?}"
    );
}

// ---------------------------------------------------------------------------
// gh #236: the Antigravity catalog at real size (32 models, ~15 KiB,
// pi-antigravity's verified shape) through the shipped component.
// ---------------------------------------------------------------------------

/// A committed 32-model `fetchAvailableModels` catalog (~15 KiB) in
/// pi-antigravity's verified shape (`ModelInfoRaw`: displayName,
/// quotaInfo, capability flags). Written inline so the size is pinned
/// in review: a unit mock that shrinks this re-opens the fuel hole.
fn realistic_catalog() -> String {
    let families = [
        ("gemini-3.8-flash", "Gemini 3.8 Flash", true, true),
        ("gemini-3.7-flash", "Gemini 3.7 Flash", true, true),
        ("gemini-3.6-flash", "Gemini 3.6 Flash", true, true),
        ("gemini-3.5-flash", "Gemini 3.5 Flash", true, true),
        ("gemini-3.1-pro", "Gemini 3.1 Pro", true, true),
        ("gemini-2.5-pro", "Gemini 2.5 Pro", true, true),
        ("gemini-2.5-flash", "Gemini 2.5 Flash", true, true),
        ("gemini-2.0-flash", "Gemini 2.0 Flash", false, true),
        ("claude-opus-4-6", "Claude Opus 4.6", true, true),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6", true, true),
        ("claude-haiku-4-5", "Claude Haiku 4.5", true, false),
        ("gpt-oss-120b", "GPT OSS 120B", false, false),
    ];
    let mut models = serde_json::Map::new();
    for i in 0..32 {
        let (base, display, thinking, images) = families[i % families.len()];
        let id = if i < families.len() {
            base.to_string()
        } else {
            format!("{base}-exp-{i}")
        };
        models.insert(
            id.clone(),
            serde_json::json!({
                "isInternal": false,
                "displayName": format!("{display} (Antigravity)"),
                "label": display,
                "modelName": id,
                "modelProvider": if id.contains("gemini") { "google" } else { "anthropic" },
                "apiProvider": "antigravity",
                "supportsThinking": thinking,
                "supportsImages": images,
                "recommended": i < 3,
                "quotaInfo": {"remainingFraction": 0.87, "resetTime": "2026-10-17T00:00:00Z"},
                "experiments": [format!("exp-{i}-a"), format!("exp-{i}-b")],
            }),
        );
    }
    serde_json::json!({
        "models": models,
        "defaultAgentModelId": "gemini-3.8-flash",
    })
    .to_string()
}

/// One loopback Google: the catalog above for `fetchAvailableModels`,
/// `{}` elsewhere.
fn catalog_mock(catalog: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten().take(8) {
            let mut stream = stream;
            let mut buf = vec![0u8; 65536];
            let mut head = Vec::new();
            while let Ok(n) = std::io::Read::read(&mut stream, &mut buf) {
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&head).into_owned();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
            recorded.lock().expect("paths").push(path.clone());
            let payload = if path.contains("fetchAvailableModels") {
                catalog.clone()
            } else {
                "{}".to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
        }
    });
    (format!("http://{addr}"), seen)
}

const AG_FIXTURE: &[u8] =
    include_bytes!("../../../../extensions/antigravity/fixtures/component.wasm");
const AG_MANIFEST: &str = include_str!("../../../../extensions/antigravity/extension.toml");

/// Load the shipped Antigravity component at `fuel` with loopback
/// credentials seeded: a live access token, far-future expiry, and the
/// mock as its API base, plus a session 127.0.0.1 grant.
fn antigravity_guest(name: &str, fuel: u64, api_base: &str) -> Arc<dyn ExtensionDispatch> {
    let fixture = Fixture::new(name);
    {
        let mut store = fixture.store.lock().expect("store");
        store
            .attach_session_net_pattern(&fixture.roots.workspace, "127.0.0.1")
            .expect("loopback grant");
    }
    let creds = fixture.root.join("data/credentials/antigravity.json");
    std::fs::create_dir_all(creds.parent().expect("dir")).expect("mkdir");
    std::fs::write(
        &creds,
        serde_json::json!({
            "access": "mock-access",
            "expires": "9999999999",
            "project": "mock-project",
            "api_base": api_base,
        })
        .to_string(),
    )
    .expect("seed");
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: fuel,
            log_limit_bytes: 4096,
        },
        fixture.env(),
    );
    let handle = host.load(AG_FIXTURE, AG_MANIFEST).expect("load");
    let mut registry = NativeRegistry::new();
    registry.register(Arc::new(handle));
    let mut handles = registry.into_handles();
    handles.pop().expect("handle")
}

// Verifies: gh #236 - a real-size catalog lists through the shipped
// component within the production fuel budget (FR-EXT-4, FR-PROV-2).
#[tokio::test]
async fn antigravity_lists_a_realistic_catalog_within_production_fuel() {
    let catalog = realistic_catalog();
    assert!(
        catalog.len() >= 10_000,
        "the fixture must stay real-size: {} bytes",
        catalog.len()
    );
    let (base, seen) = catalog_mock(catalog);
    let guest = antigravity_guest("ag-catalog", DEFAULT_FUEL_PER_CALL, &base);
    let models = guest.provider_models(&[]).expect("listing succeeds");
    assert_eq!(models.len(), 32, "every catalog model lists");
    assert_eq!(models[0].id, "claude-haiku-4-5", "sorted by id: {models:?}");
    assert!(
        models.iter().any(|m| m.name.contains("Gemini 3.8 Flash")),
        "display names parse"
    );
    assert!(
        !seen.lock().expect("paths").is_empty(),
        "the catalog was fetched"
    );
}

// Verifies: gh #236 - the margin, not just the pass: a 4x catalog
// (128 models, ~49 KiB, measured floor ~6M fuel) lists within HALF
// the production budget, so catalog growth has somewhere to go
// before fuel matters.
#[tokio::test]
async fn antigravity_big_catalog_holds_half_budget_margin() {
    let catalog = realistic_catalog();
    let mut value: serde_json::Value = serde_json::from_str(&catalog).expect("json");
    let models = value
        .get("models")
        .expect("models")
        .as_object()
        .expect("obj")
        .clone();
    let mut merged = models.clone();
    for i in 0..96 {
        let (key, entry) = models.iter().nth(i % models.len()).expect("entry");
        merged.insert(format!("{key}-b{i}"), entry.clone());
    }
    value["models"] = serde_json::Value::Object(merged);
    let big = value.to_string();
    assert!(
        big.len() >= 45_000,
        "the margin probe must stay big: {}",
        big.len()
    );
    let (base, _) = catalog_mock(big);
    let guest = antigravity_guest("ag-margin", DEFAULT_FUEL_PER_CALL / 2, &base);
    let models = guest.provider_models(&[]).expect("margin holds");
    assert_eq!(models.len(), 128, "every model lists");
}

// ---------------------------------------------------------------------------
// gh #461: the provider's env reads must reach the WASM guest
// ---------------------------------------------------------------------------

/// Build the two delivery modes of the **shipped** `openai-compatible`
/// provider, with `vars` supplied as the granted environment.
///
/// The WASM half gets `with_env_vars` so the result does not depend on the
/// test process's own environment, and the native half reads the real process
/// environment - so the caller sets the same variable both ways, and this
/// helper is what makes "same answer" mean something.
fn openai_compatible_both_modes(
    name: &str,
    vars: std::collections::BTreeMap<String, String>,
) -> (Arc<dyn ExtensionDispatch>, Arc<dyn ExtensionDispatch>) {
    let fixture = Fixture::new(name);
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 100_000_000,
            log_limit_bytes: 4096,
        },
        fixture.env(),
    )
    .with_env_vars(vars);
    let wasm = host
        .load(OAC_FIXTURE, OAC_MANIFEST)
        .expect("the shipped component links");

    let mut native_cap = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        fixture.roots.clone(),
        fixture.prompt.clone(),
        fixture.store.clone(),
        fixture.root.join("project"),
        None,
    );
    native_cap.set_resources(openai_compatible::resources());
    let native = openai_compatible::OpenAiCompat::with_settings(
        Arc::new(native_cap),
        openai_compatible::Settings {
            model: String::new(),
            context_window: 0,
            ..Default::default()
        },
    );
    (Arc::new(wasm), Arc::new(native))
}

// Verifies: gh #461 - **the row the capability has to satisfy.**
//
// `Settings::default()` reads six environment variables, and
// `profiles::base_url_for` reads `OPENAI_BASE_URL`. A WASM component is
// instantiated with an empty `WasiCtx`, so before this provider declared
// `[capabilities.env]` every one of those reads returned `NotPresent` and the
// guest behaved as if the user had configured nothing - which is why
// `lca-cli` once carried the hardcoded `openai_ad_hoc_host()` workaround
// (gh #157).
//
// The assertion is *parity*, not a hardcoded outcome: whatever the native twin
// decides, the sandboxed one must decide the same. With no key granted, both
// must refuse the login and name the variables in the same breath.
#[tokio::test]
async fn the_providers_env_reads_reach_the_guest() {
    let (wasm, native) =
        openai_compatible_both_modes("env-parity", std::collections::BTreeMap::new());
    // No key anywhere: the refusal is the observable, and both modes must
    // produce the *same* one. A guest that could not read the environment at
    // all produced this same message for the wrong reason, so the message
    // alone is not enough - the granted case below is what distinguishes them.
    let wasm_outcome = wasm.identity_login().await.expect("wasm login runs");
    let native_outcome = native.identity_login().await.expect("native login runs");
    assert_eq!(
        format!("{wasm_outcome:?}"),
        format!("{native_outcome:?}"),
        "both modes refuse identically with nothing configured"
    );
}

// Verifies: gh #461 - **the granted case**, which is the one the workaround
// existed for.
//
// With `OPENAI_API_KEY` granted, the guest must see it. Before the provider
// declared `[capabilities.env]`, the guest answered
// "no API key is configured. Set OPENAI_API_KEY ... " **even with the variable
// set and granted** - measured directly: `with_env_vars({OPENAI_API_KEY:
// "granted-key"})` still produced that refusal, because the manifest declared
// no names for the host to forward.
//
// The assertion is deliberately about the *store* the login performs, not the
// outcome variant: a keyless `auth = "none"` preset also returns `Ok`, so the
// variant alone would not prove the key was read.
#[tokio::test]
async fn a_granted_env_key_is_read_by_the_guest() {
    // SAFETY: this test owns the variable for its duration; the workspace has
    // no other reader of OPENAI_API_KEY in a unit-test context.
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("OPENAI_API_KEY".to_string(), "granted-env-key".to_string());
    let (wasm, native) = openai_compatible_both_modes("env-granted", vars.clone());

    let wasm_outcome = wasm.identity_login().await.expect("wasm login runs");
    let native_outcome = native.identity_login().await.expect("native login runs");
    assert_eq!(
        format!("{wasm_outcome:?}"),
        format!("{native_outcome:?}"),
        "the guest reaches the same identity decision as the in-process twin"
    );
    assert!(
        !format!("{wasm_outcome:?}").contains("no API key is configured"),
        "a granted key must not be reported missing: {wasm_outcome:?}"
    );
}
