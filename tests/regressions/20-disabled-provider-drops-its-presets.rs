//! Cycle 5's driving found two things about a disabled provider.
//!
//! 1. `ext disable` must actually drop the extension's presets from the
//!    `/login` picker. The picker sources its options from
//!    `ExtensionRegistry::provider_names()` and `provider()`, so a disabled
//!    entry has to fall out of both - otherwise `/login` keeps advertising
//!    presets whose consumer cannot answer.
//! 2. Disabling the *only* provider leaves `lca` unable to start at all
//!    (FR-PROV-6's report exits with USAGE). "Install a provider" is then
//!    the wrong advice: the usual fix is `lca ext enable <name>`. The
//!    message has to name that escape.
//!
//! The report-wording half lives in `crates/lca-cli`'s own tests, where
//! `no_model_message` is visible.
//!
//! Verifies: FR-PROV-9, ADR-0031 (presets are extension data, so a disabled
//! extension takes them away).

use std::sync::{Arc, Mutex};

/// A capability engine for the bundled provider, the same shape the CLI
/// builds at startup (including ADR-0032's compiled-in resource bag).
fn caps(project: &std::path::Path) -> Arc<lca_tools::Capabilities> {
    let data = project.join("data");
    let roots = lca_permissions::ScopeRoots {
        workspace: project.to_path_buf(),
        private: data.join("private"),
        home_config: data.join("config"),
        temp: data.join("temp"),
        state_dir: data.clone(),
    };
    let store = Arc::new(Mutex::new(
        lca_permissions::GrantStore::open(&project.join("grants.json")).expect("open"),
    ));
    let mut engine = lca_tools::Capabilities::new(
        "openai-compatible",
        openai_compatible::manifest_grants(),
        roots,
        Arc::new(Mutex::new(lca_permissions::SharedPrompt::default())),
        store,
        project.to_path_buf(),
        None,
    );
    engine.set_resources(openai_compatible::resources());
    Arc::new(engine)
}

#[test]
fn a_disabled_provider_stops_contributing_picker_options() {
    let root = lca_testkit::scratch_path("regression-disable-presets");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");

    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(openai_compatible::OpenAiCompat::new(caps(&root))));
    assert_eq!(
        registry.provider_names(),
        vec!["openai-compatible".to_string()],
        "enabled and registered as a provider"
    );

    // The picker's source: the extension's own presets, from its own bag.
    let handle = registry
        .provider("openai-compatible")
        .expect("resolvable")
        .clone();
    let options =
        lca_core::drive_blocking(async move { handle.login_options().await }).expect("options");
    assert!(
        options.iter().any(|option| option.id == "openrouter"),
        "the presets are offered while it is enabled"
    );

    // Disabling drops it from both lookup paths the picker uses.
    registry.set_enabled("openai-compatible", false);
    assert!(
        registry.provider_names().is_empty(),
        "a disabled provider is no longer offered"
    );
    assert!(
        registry.provider("openai-compatible").is_none(),
        "and no longer resolves"
    );
    let _ = std::fs::remove_dir_all(&root);
}
