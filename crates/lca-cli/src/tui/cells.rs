//! The live cells, the model resolution helpers, and the agent config: everything
//! that turns a resolved provider/model into the wiring `Ui` holds.
//!
//! Split from `tui/mod.rs` at gate 11's 1,200-line ceiling (gh #543). The seam
//! is *what the cells are*: `Cells` and the four functions that build or update
//! one (`live_cells`, `adopt_login_identity`, `model_context_window`,
//! `apply_models_scope`), plus `agent_config_for`, which reads the same resolved
//! model. None of them needs a `Ui` - they take the pieces - so this file has
//! no `impl Ui` at all, and `Ui::new` is the single caller.
//!
//! Visibility is unchanged by the move: `pub(super)` inside `crate::tui` is
//! exactly what `pub(super)` was in `tui/mod.rs` (`crate`), so `hooks.rs` and
//! `attach.rs`'s tests reach the same names at the same path.

use std::path::Path;
use std::sync::{Arc, Mutex};

use lca_config::Config;
use lca_core::{AgentConfig, ExtensionRegistry};
use lca_provider::Provider;

use super::ModelChoice;

/// The live cells the footer and `/model`/`/thinking` share.
pub(super) struct Cells {
    // `pub(super)` here is `pub(in crate::tui)`, which is exactly the reach these
    // fields had as private fields of `tui/mod.rs`: readable from `tui` and its
    // descendants, writable nowhere else. The move did not widen them - and the
    // only reader either way is `Ui::new` in `tui`, which is `super`.
    pub(super) model: Arc<Mutex<ModelChoice>>,
    pub(super) label: Arc<Mutex<String>>,
    pub(super) context_window: Arc<Mutex<u64>>,
    pub(super) thinking: Arc<Mutex<Option<String>>>,
    pub(super) identity: Arc<Mutex<String>>,
}

/// Build the live cells from the resolved model (FR-UI-20, R1). `identity`
/// is the preset id when the login stored one, else the extension name (E5).
pub(super) fn live_cells(
    identity: &str,
    model_id: &str,
    context_window: u32,
    thinking: Option<String>,
) -> Cells {
    Cells {
        model: Arc::new(Mutex::new(ModelChoice {
            id: model_id.to_string(),
            window: context_window,
        })),
        label: Arc::new(Mutex::new(if model_id.is_empty() {
            String::new()
        } else {
            format!("{identity}/{model_id}")
        })),
        context_window: Arc::new(Mutex::new(u64::from(context_window))),
        thinking: Arc::new(Mutex::new(thinking)),
        identity: Arc::new(Mutex::new(identity.to_string())),
    }
}

/// Adopt a finished login's identity into the live cell (E5, the cycle-6
/// drive's regression). The login updated only the label, so a later
/// `/model` switch read the stale startup identity and the footer reverted
/// from the login preset to the extension name. Named so the
/// mid-session-login regression is testable without a live provider.
pub(super) fn adopt_login_identity(identity_cell: &Arc<Mutex<String>>, identity: &str) {
    *identity_cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = identity.to_string();
}

/// The active model's context window, or 0 when the provider does not
/// publish one (the threshold check then skips, FR-SESS-4).
pub(super) fn model_context_window(provider: &dyn Provider, model_id: &str) -> u32 {
    provider
        .list_models()
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.context_window)
        .unwrap_or(0)
}

/// Swap the live configuration scope (gh #204): the `/scoped-models`
/// checklist writes through this cell, so the model cycle reads the
/// live scope without a restart. `/reload` swaps the whole cell.
// `pub(crate)`, not `pub(super)`: in `tui/mod.rs` this function's `pub(super)`
// meant "visible in the crate root", i.e. `pub(crate)`. From `tui/cells.rs`,
// `pub(super)` would be `pub(in crate::tui)` - a *narrowing*, and a narrowing
// written by a refactor is a silent loss of a surface. Kept exactly as it was.
pub(crate) fn apply_models_scope(config: &Mutex<Config>, ids: Vec<String>) {
    crate::lock(config).set_models_enabled(ids);
}

/// The agent config for the resolved model and registry.
#[allow(clippy::too_many_arguments)]
pub(super) fn agent_config_for(
    config: &Config,
    provider_name: &str,
    model_id: &str,
    context_window: u32,
    image_policy: lca_tools::ImagePolicy,
    registry: &Arc<ExtensionRegistry>,
    completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    cwd: &Path,
    flags: &crate::CliFlags,
    trusted: bool,
) -> Result<AgentConfig, String> {
    // gh #271: advertise what the run offers, from the same resolution the
    // request's declaration uses (never a second filter).
    let effective = crate::prompt::effective_tools(registry);
    let names: Vec<&str> = effective.iter().map(String::as_str).collect();
    let system_prompt =
        crate::prompt::agent_system_prompt_for(cwd, model_id, flags, trusted, &names)?;
    Ok(AgentConfig {
        provider: provider_name.to_string(),
        model: model_id.to_string(),
        image_policy,
        retry_limit: config.provider_retry_limit() as u32,
        retry_base_delay: std::time::Duration::from_millis(config.provider_retry_base_delay_ms()),
        retry_max_delay: std::time::Duration::from_millis(config.provider_retry_max_delay_ms()),
        max_iterations: config.tool_max_iterations() as u32,
        extensions: registry.clone(),
        steering_mode: lca_protocol::QueueMode::parse(config.queue_steering_mode())
            .unwrap_or_default(),
        compaction_threshold: config.compaction_threshold(),
        compaction_enabled: config.compaction_enabled(),
        compaction_reserve_tokens: config.compaction_reserve_tokens(),
        compaction_keep_recent_tokens: config.compaction_keep_recent_tokens(),
        model_context_window: context_window,
        completion_backend,
        system_prompt,
        skills_roots: crate::skills_roots(cwd, flags),
        // Gh #217: the startup prompt sources, re-scanned every turn
        // for unified-diff updates (empty with `-nc`; the trust rule
        // matches the prompt build above).
        instruction_sources: crate::prompt::context_sources(
            &crate::data_dir(),
            cwd,
            trusted,
            flags.no_context_files,
        ),
        instruction_baseline: Default::default(),
        skills_inject_matched: config.skills_inject_matched(),
        edit_requires_read: config.tool_edit_requires_read(),
        auto_resize_images: config.images_auto_resize(),
        ..AgentConfig::default()
    })
}
