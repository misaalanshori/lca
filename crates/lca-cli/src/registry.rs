//! The extension registry every front end assembles (ADR-0013):
//! project-local copies first (a repo's own tool shadows a same-named
//! installed one, gh #138), then installed (an installed copy shadows
//! the bundled one of the same name), the native first-party set, the
//! bundled provider, and enablement last (FR-DIST-8, FR-PROV-9).
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
) -> (
    ExtensionRegistry,
    Arc<lca_mcp::manager::McpManager>,
    Vec<String>,
) {
    let mut registry = ExtensionRegistry::new();
    // gh #70: `--no-extensions` skips installed and built-in
    // extensions (providers still resolve, or the run cannot start).
    // Explicit `-e` paths load after everything, consent and all.
    if !flags.no_extensions {
        crate::project_ext::load_project_local(
            &mut registry,
            cwd,
            &crate::data_dir(),
            config.extensions_log_limit_bytes() as usize,
            shared_prompt.clone(),
            shared_dialogs.clone(),
            grants,
        );
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
    // gh #53: the MCP session manager (files, bridge, actions).
    // Empty entries connect nothing; the registry still owns the
    // handle so later turns see rebuilds with no re-registration.
    // Load warnings ride out beside the registry for the startup
    // notice (invalid entries never block the valid ones). Built
    // before the provider takes the prompt handle below.
    let data = crate::data_dir();
    let trusted = crate::lock(grants).is_trusted(cwd);
    // gh #418 Phase E (gh #266): enablement is a question asked **before**
    // anything connects, not after. Both answers are per-run facts: the
    // persisted record (`/mcp disable`, `lca ext disable mcp`) and the
    // `--no-mcp` flag, which is per-invocation and never written.
    //
    // The ordering bug was structural: `McpManager::load` spawned every
    // configured server *before* `apply_enablement` had marked the bridge
    // disabled, so a disabled MCP still started its children and connected to
    // its remotes. Asking here means a disabled MCP spawns nothing at all -
    // the guard the turn-level test asserts with a spawn marker.
    let mcp_enabled =
        !flags.no_mcp && crate::lock(grants).extension_enabled(cwd, "mcp") != Some(false);
    // gh #481: the fourth path with #463's root cause. `plan` reads
    // `<data>/mcp.json` (the user file) and, when trusted,
    // `<cwd>/.lca/mcp.json` (the project file). From `$HOME` those are the same
    // file, so the user's own servers were loaded twice: once as user entries
    // and again as project *overrides* of themselves. Visible in `lca mcp list`
    // - the source column said `project` for a server in `~/.lca/mcp.json` - and
    // not merely cosmetic: an override source routes later manager edits through
    // `write_full_entry` to the user's own file instead of patching it.
    //
    // There is no separate project layer when the two paths name the same file,
    // so `plan` is told there is no project (the user file is read either way).
    // Compared canonically for the same three reasons as the config loader: a
    // symlinked `$HOME`, macOS's `/var` -> `/private/var`, and Windows's `\\?\`
    // prefix all defeat a lexical comparison (gh #436, #389, #463).
    let project_mcp_is_the_user_file =
        crate::project_ext::project_file_is_the_user_file(cwd, &data, "mcp.json");
    let (mut entries, mgrants, warnings, user_path, project_path) =
        lca_mcp::manager::McpManager::plan(&data, cwd, trusted && !project_mcp_is_the_user_file);
    if !mcp_enabled {
        // Keep the entries so `/mcp` still lists the servers (as disabled),
        // but no worker starts for one.
        for entry in &mut entries {
            entry.enabled = false;
        }
    }
    let mcp_caps = crate::extension_capabilities(
        cwd,
        "mcp",
        mgrants,
        shared_prompt.clone(),
        grants.clone(),
        // No `resources/` bag: the bridge carries code, not data.
        lca_tools::ResourceSource::None,
        temp,
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
    {
        let cap = crate::provider_capabilities(
            cwd,
            "openai-compatible",
            &manifest,
            openai_compatible::resources(),
            shared_prompt,
            grants.clone(),
            temp,
        );
        // Gh #97: the host notes the engine it just built, so the
        // turn boundary resets it even though the handle's own
        // `turn_started` already does (belt and suspenders).
        registry.note_engine("openai-compatible", &cap);
        registry.register(Arc::new(openai_compatible::OpenAiCompat::with_settings(
            cap,
            openai_compatible::Settings {
                model_overrides: std::fs::read_to_string(crate::data_dir().join("models.toml"))
                    .unwrap_or_default(),
                ..Default::default()
            },
        )));
    }
    #[cfg(not(feature = "bundled-openai-compat"))]
    let _ = shared_prompt;
    // The bridge is registered **before** `apply_enablement` runs, so the
    // enablement sweep covers it like every other handle (gh #266's second
    // half: registering afterwards meant `mcp` was never consulted).
    let manager = Arc::new(lca_mcp::manager::McpManager::load(
        mcp_caps,
        user_path,
        project_path,
        entries,
    ));
    registry.register(manager.bridge());
    crate::apply_enablement(&mut registry, |name| {
        if name == "mcp" {
            // `--no-mcp` counts here too, so the flag reports the extension as
            // disabled to everything downstream (`/ext`, the crash file).
            return !mcp_enabled;
        }
        crate::lock(grants).extension_enabled(cwd, name) == Some(false)
    });
    // gh #81: the crash file names what actually loaded.
    lca_tui::set_crash_extensions(
        registry
            .handles()
            .iter()
            .map(|handle| handle.name().to_string())
            .collect(),
    );
    // gh #3: the preferred compaction strategy rides the registry
    // from configuration (the manager moves it live later).
    registry.set_preferred_compaction(config.compaction_strategy().map(str::to_string));
    (registry, manager, warnings)
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
    // Gh #97: note the engine for the host-side turn-boundary reset.
    registry.note_engine("compaction-default", &cap);
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
        let (registry, _mcp, _warnings) = super::assemble(
            &project,
            &config,
            lca_permissions::SharedPrompt::default(),
            lca_permissions::SharedDialogs::default(),
            &grants,
            std::sync::Arc::new(|| String::new()),
            &root.join("tmp"),
            &crate::CliFlags::default(),
        );
        assert!(
            registry.provider("openai-compatible").is_none(),
            "no bundled provider, so the TUI opens in the zero-provider state"
        );
    }
}
