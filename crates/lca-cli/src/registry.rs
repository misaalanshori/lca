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

/// Build the registry: installed extensions, the native first-party set,
/// the bundled provider, then this project's enablement rules.
pub(crate) fn assemble(
    cwd: &Path,
    config: &Config,
    shared_prompt: SharedPrompt,
    grants: &Arc<Mutex<GrantStore>>,
    stats: StatsSource,
) -> ExtensionRegistry {
    let mut registry = ExtensionRegistry::new();
    crate::ext::load_installed(
        &mut registry,
        cwd,
        config.extensions_log_limit_bytes() as usize,
        shared_prompt.clone(),
        grants,
    );
    for handle in lca_ext_native::default_native_extensions(stats) {
        registry.register(handle);
    }
    #[cfg(feature = "bundled-openai-compat")]
    registry.register(Arc::new(openai_compatible::OpenAiCompat::new(
        crate::openai_capabilities(cwd, shared_prompt, grants.clone()),
    )));
    #[cfg(not(feature = "bundled-openai-compat"))]
    let _ = shared_prompt;
    crate::apply_enablement(&mut registry, |name| {
        crate::lock(grants).extension_enabled(cwd, name) == Some(false)
    });
    registry
}
