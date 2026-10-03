//! GitHub issue #34 (released 0.5.3): every preset model reported no
//! context window, so the footer showed `ctx ?` for `opencode-go/<model>`
//! and everything else, and the compaction threshold (FR-SESS-4) had no
//! real denominator to divide by.
//!
//! The cause was one setting: `list-models` filled `ModelInfo.context_window`
//! from `OPENAI_CONTEXT_WINDOW` alone, for every model in the list, and that
//! variable is unset by default. The windows were never missing from the
//! world - the provider's own catalog has them - they were just not read.
//!
//! gh #34 gives the extension a curated per-model table (every number
//! carries its source), lets an endpoint's own `context_length` ride the
//! `models` setting, and keeps `OPENAI_CONTEXT_WINDOW` on top of both. Two
//! rows hold it here, at the seam the footer reads:
//!
//! - a model with a curated window reports it, and the footer turns it into
//!   a percentage instead of `?`;
//! - an id no source confirms still reports 0, and the footer keeps `ctx ?`.
//!   That half must not move: a wrong window silently mis-triggers
//!   compaction, which is worse than admitting it is unknown.
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #34.

use std::sync::{Arc, Mutex};

/// The capability engine the CLI builds at startup, with ADR-0032's
/// compiled-in resource bag - the same shape `20` uses.
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

/// The registry the host resolves providers through, over a pinned
/// environment: no `OPENAI_CONTEXT_WINDOW`, no `OPENAI_MODEL`, so the row
/// can only be decided by the curated table.
fn registry() -> (lca_core::ExtensionRegistry, std::path::PathBuf) {
    let root = lca_testkit::scratch_path("regression-gh34-context-windows");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let handle = openai_compatible::OpenAiCompat::with_settings(
        caps(&root),
        openai_compatible::Settings {
            model: String::new(),
            context_window: 0,
            ..Default::default()
        },
    );
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(handle));
    (registry, root)
}

/// `list-models` as the host asks for it, with `models` from the setting
/// `login-submit` hands the host to persist (ADR-0033).
fn models(registry: &lca_core::ExtensionRegistry, list: &str) -> Vec<lca_protocol::ModelInfo> {
    let handle = registry
        .provider("openai-compatible")
        .expect("resolvable")
        .clone();
    handle
        .provider_models(&[("models".to_string(), list.to_string())])
        .expect("models")
}

fn window_of(models: &[lca_protocol::ModelInfo], id: &str) -> u64 {
    u64::from(
        models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("`{id}` in the list: {models:?}"))
            .context_window,
    )
}

/// The footer's stats line for this window and how much of it is in use.
fn footer_line(context_window: u64, context_used: u64) -> String {
    lca_ui::Footer {
        model: "opencode-go/mimo-v2.6-flash".into(),
        context_window,
        context_used,
        ..Default::default()
    }
    .render(200, &lca_ui::Theme::plain())
    .into_iter()
    .nth(1)
    .expect("the stats line")
}

// Verifies: gh #34 (the primary row) - a preset model with a curated
// window reports it through `list-models`, and the footer turns that
// number into a real share instead of `ctx ?`.
#[test]
fn a_preset_model_with_a_curated_window_reaches_the_footer() {
    let (registry, root) = registry();
    let models = models(&registry, "mimo-v2.6-flash");
    let window = window_of(&models, "mimo-v2.6-flash");
    assert_eq!(window, 1_048_576, "the curated 1.0M: {models:?}");

    // Half the window in use reads as half, which is the whole point of
    // the denominator being real.
    let line = footer_line(window, 524_288);
    assert!(line.contains("ctx 50%"), "{line}");
    assert!(!line.contains("ctx ?"), "{line}");
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #34's unchanged half - an id no source confirms reports no
// window, and the footer keeps `ctx ?` rather than fabricating a share
// (FR-UI-20's E4, FR-SESS-4's honest denominator).
#[test]
fn an_unknown_model_reports_no_window_and_the_footer_keeps_ctx_question_mark() {
    let (registry, root) = registry();
    let models = models(&registry, "a-model-nobody-confirms");
    let window = window_of(&models, "a-model-nobody-confirms");
    assert_eq!(window, 0, "no source, no number: {models:?}");

    let line = footer_line(window, 524_288);
    assert!(line.contains("ctx ?"), "{line}");
    assert!(!line.contains("ctx 50%"), "{line}");
    let _ = std::fs::remove_dir_all(root);
}
