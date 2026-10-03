//! Characterization of `list-models`' **current** shape, written before
//! the D2 change so the change cannot move it silently.
//!
//! The asymmetry D2 names: `complete` receives the host-persisted settings
//! on every call (in its request `extras`), so the discovered model list -
//! which `login-submit` puts there - is visible to completion but not to
//! `list-models`, which reads only the environment. The fix makes the two
//! symmetric (ADR-0035). When it lands, this file changes with it, and the
//! two-mode parity tests take over.
//!
//! Verifies: ADR-0033 (the settings `login-submit` hands back), and pins
//! the shape ADR-0035 is about to change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn sandbox(name: &str) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("lca-models-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    let mut engine = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        roots,
        Arc::new(std::sync::Mutex::new(Always)),
        Arc::new(std::sync::Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("open"),
        )),
        root.join("project"),
        None,
    );
    engine.set_resources(openai_compatible::resources());
    Arc::new(engine)
}

// The shape ADR-0035 settled. `list-models` takes the same settings flow
// `complete` does, so the discovered model list - which `login-submit`
// hands the host to persist - is visible to both, from one source, with no
// second store. This is the characterization test from before the change,
// moved onto the new shape rather than deleted.
#[test]
fn list_models_reads_the_passed_settings_not_a_store_of_its_own() {
    let cap = sandbox("settings");
    cap.credentials_set("api_key", "sk-x").expect("key");
    // What `login-submit` handed the host to persist (ADR-0033).
    cap.credentials_set("models", "store-a,store-b")
        .expect("models");

    let handle = openai_compatible::OpenAiCompat::new(cap.clone());
    // With no settings passed, the extension cannot know better than the
    // fallback it can reach.
    let fallback: Vec<String> = handle
        .provider_models(&[])
        .expect("models")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert!(fallback.iter().any(|id| id == "store-a"), "{fallback:?}");

    // The passed settings are the source of truth and override anything the
    // extension might have stored: this is the symmetry with `complete`.
    let passed = [("models".to_string(), "passed-x,passed-y".to_string())];
    let offered: Vec<String> = handle
        .provider_models(&passed)
        .expect("models")
        .into_iter()
        .map(|model| model.id)
        .collect();
    assert!(
        offered.iter().any(|id| id == "passed-x") && offered.iter().any(|id| id == "passed-y"),
        "the passed list is what is offered: {offered:?}"
    );
    assert!(
        !offered.iter().any(|id| id == "store-a"),
        "the passed settings override the store rather than merging with it: {offered:?}"
    );
    let _ = std::fs::remove_dir_all(lca_testkit::scratch_path("lca-models-settings"));
}

/// Issue #3: shipped model data must be product data, never the development
/// harness's model whitelist. This is the snapshot that would have caught the
/// dev-ids leak into the `/model` picker.
#[test]
fn shipped_presets_contain_no_development_policy_ids() {
    let presets = include_str!("../resources/provider-presets.toml");
    for forbidden in [
        "deepseek-v4-flash",
        "deepseek-v4.1-flash",
        "mimo-v2.6-flash",
    ] {
        assert!(
            !presets.contains(forbidden),
            "development model id `{forbidden}` shipped in provider-presets.toml"
        );
    }
}

// ---------------------------------------------------------------------------
// gh #34: per-model context windows - the number the footer's `ctx` and
// the compaction threshold (FR-SESS-4) divide by. Every value below comes
// from the curated table or from the `models` setting; nothing is guessed,
// and an id neither source knows reports 0, which the footer renders
// `ctx ?`.
// ---------------------------------------------------------------------------

/// The list `provider_models` returns for these credentials, with the
/// window of every id the caller named resolved.
fn windows_for(
    cap: &Arc<lca_tools::Capabilities>,
    settings: openai_compatible::Settings,
) -> Vec<u32> {
    let handle = openai_compatible::OpenAiCompat::with_settings(cap.clone(), settings);
    let models = handle.provider_models(&[]).expect("models");
    ["mimo-v2.6-flash", "no-such-model"]
        .iter()
        .map(|id| {
            models
                .iter()
                .find(|model| model.id == *id)
                .unwrap_or_else(|| panic!("`{id}` in the list: {models:?}"))
                .context_window
        })
        .collect()
}

fn pinned(context_window: u32) -> openai_compatible::Settings {
    openai_compatible::Settings {
        // Env-derived and irrelevant here; pinned so no developer
        // environment can move the row.
        model: String::new(),
        context_window,
        ..Default::default()
    }
}

// Verifies: gh #34 (the primary row) - a preset model with a curated
// window reports it through `ModelInfo`, so the footer can show a real
// percentage instead of `ctx ?`.
#[test]
fn a_model_with_a_curated_window_reports_it() {
    let cap = sandbox("gh34-curated");
    cap.credentials_set("api_key", "sk-x").expect("key");
    cap.credentials_set("models", "mimo-v2.6-flash,no-such-model")
        .expect("models");
    let windows = windows_for(&cap, pinned(0));
    assert_eq!(windows[0], 1_048_576, "the curated 1.0M: {windows:?}");
    // And the unchanged row: an id no source confirms reports no window.
    assert_eq!(windows[1], 0, "an unknown model keeps `ctx ?`: {windows:?}");
}

// Verifies: gh #34's precedence - a limit the endpoint itself reported
// rides the `models` setting (`id=window`) out of `list-models` and wins
// over the curated value for the same id.
#[test]
fn a_limit_the_endpoint_reported_wins_over_the_curated_value() {
    let cap = sandbox("gh34-reported");
    cap.credentials_set("api_key", "sk-x").expect("key");
    // What `login_submit` writes when `GET /models` answers with
    // `context_length` (verified against OpenRouter's live endpoint).
    cap.credentials_set("models", "mimo-v2.6-flash=262144,no-such-model")
        .expect("models");
    let windows = windows_for(&cap, pinned(0));
    assert_eq!(
        windows[0], 262_144,
        "the endpoint's own answer: {windows:?}"
    );
    assert_eq!(windows[1], 0, "a bare id carries no window: {windows:?}");
}

// Verifies: gh #34's precedence - `OPENAI_CONTEXT_WINDOW` still overrides
// every per-model value, curated or reported (ADR-0035's chain: the
// environment wins). `Settings::default()` is where that variable lands,
// so the row goes through it rather than through a pinned struct.
#[test]
fn the_context_window_override_wins_over_every_per_model_value() {
    let cap = sandbox("gh34-override");
    cap.credentials_set("api_key", "sk-x").expect("key");
    cap.credentials_set("models", "mimo-v2.6-flash=262144")
        .expect("models");
    let lock = lca_testkit::fixture::env_lock();
    // SAFETY: the environment lock is held for the rest of the test, and
    // cargo-nextest runs each test in its own process - the discipline
    // `crates/lca-testkit/src/fixture.rs` documents.
    unsafe { std::env::set_var("OPENAI_CONTEXT_WINDOW", "4096") };
    let handle = openai_compatible::OpenAiCompat::new(cap.clone());
    let models = handle.provider_models(&[]).expect("models");
    unsafe { std::env::remove_var("OPENAI_CONTEXT_WINDOW") };
    drop(lock);
    let window = |id: &str| {
        models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("`{id}` in the list: {models:?}"))
            .context_window
    };
    assert_eq!(
        window("mimo-v2.6-flash"),
        4_096,
        "the override wins: {models:?}"
    );
}

// Verifies: gh #34 - the curated table itself is product data with a
// source per number, and it parses: a model the table names has a window,
// and the ids issue #34 actually reported are in it.
#[test]
fn the_curated_window_table_parses_and_covers_the_reported_models() {
    let windows =
        openai_compatible::parse_context_windows(include_str!("../resources/context-windows.toml"));
    assert!(
        !windows.is_empty(),
        "the table shipped with entries: {windows:?}"
    );
    for reported in ["mimo-v2.6-flash", "deepseek-flash"] {
        assert!(
            windows.get(reported).is_some_and(|tokens| *tokens > 0),
            "`{reported}` is one of issue #34's models and has a window: {windows:?}"
        );
    }
}
