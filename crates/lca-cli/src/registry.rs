//! The extension registry every front end assembles (ADR-0013): installed
//! copies first (an installed copy shadows the bundled one of the same
//! name), the native first-party set, the bundled provider, and enablement
//! last (FR-DIST-8, FR-PROV-9).
//!
//! The interface and headless mode built this inline; `--list-models`
//! needs the very same registry with no session to hang a stats source on,
//! so the sequence lives here once (gh #8).

use std::path::Path;
use std::sync::{Arc, Mutex};

use lca_config::Config;
use lca_core::ExtensionRegistry;
use lca_ext_native::StatsSource;
use lca_permissions::{GrantStore, SharedPrompt};
use lca_provider::Provider;

/// Build the registry: installed extensions, the native first-party set,
/// the bundled provider, then this project's enablement rules.
#[allow(clippy::too_many_arguments)] // one more explicit than a regroup: every arg is used once, at one call site.
pub(crate) fn assemble(
    cwd: &Path,
    config: &Config,
    shared_prompt: SharedPrompt,
    shared_dialogs: lca_permissions::SharedDialogs,
    grants: &Arc<Mutex<GrantStore>>,
    stats: StatsSource,
    temp: &Path,
    flags: &crate::CliFlags,
) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    // gh #70: `--no-extensions` skips installed and built-in
    // extensions (providers still resolve, or the run cannot start).
    // Explicit `-e` paths load after everything, consent and all.
    if !flags.no_extensions {
        crate::ext::load_installed(
            &mut registry,
            cwd,
            config.extensions_log_limit_bytes() as usize,
            shared_prompt.clone(),
            shared_dialogs.clone(),
            grants,
        );
        for handle in lca_ext_native::default_native_extensions(stats) {
            registry.register(handle);
        }
    } else {
        let _ = stats;
    }
    crate::ext::load_extra(
        &mut registry,
        cwd,
        config.extensions_log_limit_bytes() as usize,
        shared_prompt.clone(),
        shared_dialogs,
        grants,
        &flags.extension,
    );
    // gh #64: the user's model-metadata overrides ride the provider's
    // settings into native mode (an absent file parses to nothing).
    // The WASM guest has no user-file channel, so it honors a
    // `models.toml` pair the host does not send yet (documented
    // divergence; the conformance suite guards identical answers).
    // gh #157: the bundled manifest is the grant source (parsed here,
    // read generically downstream). It is valid by construction; the
    // extension's own step-test keeps its grants in step with it.
    #[cfg(feature = "bundled-openai-compat")]
    #[allow(clippy::expect_used)]
    let manifest = lca_ext_host::Manifest::parse(openai_compatible::MANIFEST)
        .expect("the bundled openai-compatible manifest parses");
    #[cfg(feature = "bundled-openai-compat")]
    registry.register(Arc::new(openai_compatible::OpenAiCompat::with_settings(
        crate::provider_capabilities(
            cwd,
            "openai-compatible",
            &manifest,
            openai_compatible::resources(),
            shared_prompt,
            grants.clone(),
            temp,
        ),
        openai_compatible::Settings {
            model_overrides: std::fs::read_to_string(crate::data_dir().join("models.toml"))
                .unwrap_or_default(),
            ..Default::default()
        },
    )));
    #[cfg(not(feature = "bundled-openai-compat"))]
    let _ = shared_prompt;
    crate::apply_enablement(&mut registry, |name| {
        crate::lock(grants).extension_enabled(cwd, name) == Some(false)
    });
    registry
}

/// Register the bundled compaction strategy and return its backend.
#[cfg(feature = "bundled-compaction-default")]
#[allow(clippy::too_many_arguments)] // one more explicit than a regroup: every arg is used once, at one call site.
pub(crate) fn register_compaction(
    registry: &mut ExtensionRegistry,
    provider: &Arc<dyn Provider>,
    model_id: &str,
    session_id: &str,
    cwd: &Path,
    shared_prompt: SharedPrompt,
    grants: &Arc<Mutex<GrantStore>>,
    temp: &Path,
) -> Option<Arc<lca_core::ext_provider::ProviderBackend>> {
    let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
        provider.clone(),
        model_id.to_string(),
        session_id.to_string(),
    ));
    let cap = crate::extension_capabilities(
        cwd,
        "compaction-default",
        compaction_default::manifest_grants(),
        shared_prompt,
        grants.clone(),
        // No `resources/` bag: a compaction strategy carries code.
        lca_tools::ResourceSource::None,
        temp,
    );
    cap.set_completion(backend.clone());
    registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
    Some(backend)
}

/// No bundled compaction: no backend.
#[cfg(not(feature = "bundled-compaction-default"))]
pub(crate) fn register_compaction(
    _registry: &mut ExtensionRegistry,
    _provider: &Arc<dyn Provider>,
    _model_id: &str,
    _session_id: &str,
    _cwd: &Path,
    _shared_prompt: SharedPrompt,
    _grants: &Arc<Mutex<GrantStore>>,
    _temp: &Path,
) -> Option<Arc<lca_core::ext_provider::ProviderBackend>> {
    None
}

#[cfg(test)]
mod tests {
    // Verifies: FR-PROV-9 (#92, QA-017) — without the bundled provider
    // the registry assembles with no `openai-compatible` handle, so the
    // TUI takes the existing `NoProvider` path instead of refusing at
    // compile time. Runs only in the no-default-features build; the
    // default-features build is covered by the zero-provider regression.
    #[cfg(not(feature = "bundled-openai-compat"))]
    #[test]
    fn the_registry_assembles_without_the_bundled_provider() {
        let root = lca_testkit::scratch_path("lca-no-bundled-provider");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let config = lca_config::Config::defaults();
        let grants = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        let registry = super::assemble(
            &project,
            &config,
            lca_permissions::SharedPrompt::default(),
            lca_permissions::SharedDialogs::default(),
            &grants,
            std::sync::Arc::new(|| String::new()),
            &root.join("tmp"),
        );
        assert!(
            registry.provider("openai-compatible").is_none(),
            "no bundled provider, so the TUI opens in the zero-provider state"
        );
    }
}
