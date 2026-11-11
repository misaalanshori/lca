//! `/reload`: re-running discovery without restarting, and the two pieces of
//! persisted interface state it re-reads.
//!
//! Split out of `tui/hooks.rs` at gate 11's 1,200-line ceiling (gh #543). The
//! seam is **what a restart would refresh**: `/reload` (gh #130) rebuilds the
//! registry, the prompt-bearing agent config, the keybindings and the theme
//! list; `persist_ui_setting` is the write half of the `/theme`//`/thinking`
//! picks it re-applies; `persist_screen_mode`/`read_ui_json`/
//! `initial_screen_mode` are the `ui.json` fullscreen flag (FR-UI-21) a fresh
//! start and a reload both read.
//!
//! What deliberately does *not* live here: `hooks()` (the `UiHooks` literal that
//! wires every seam), `grants`, `revoke_grant` and `shell_runner`, which stay in
//! `hooks.rs`. A `Ui` method may live in any module of the crate, so `impl Ui` is
//! partitioned - same `impl`, two files, no widening.

use super::Ui;

impl Ui {
    pub(super) fn reload_hook(self: &std::sync::Arc<Self>) -> lca_ui::state::Reload {
        let ui = self.clone();
        std::sync::Arc::new(move || ui.reload())
    }

    fn reload(&self) -> lca_ui::state::ReloadReport {
        use lca_ui::state::ReloadReport;
        let fail = |reason: String| ReloadReport {
            notice: format!("reload failed: {reason}"),
            themes: Vec::new(),
            key_bindings: Default::default(),
            keybinding_error: None,
        };
        let config = match crate::load_config_flags(
            &self.cwd,
            &crate::lock(&self.grants),
            false,
            self.yolo,
            &self.flags,
        ) {
            Ok(config) => config,
            Err(err) => return fail(err.to_string()),
        };
        let trusted = crate::lock(&self.grants).is_trusted(&self.cwd);
        let (mut fresh, mcp_manager, mcp_warnings) = super::startup::load_registry(
            &self.cwd,
            &config,
            self.shared_prompt.clone(),
            self.shared_dialogs.clone(),
            &self.grants,
            &self.store,
            &self.current_session,
            &self.temp_dir,
            &self.flags,
        );
        // Gh #53: `/reload` re-reads `mcp.json` too (files stay
        // hand-editable); its warnings join the report.
        *crate::lock(&self.mcp) = mcp_manager;
        let live = crate::lock(&self.live);
        let model_id = crate::lock(&self.model_cell).id.clone();
        let window = super::model_context_window(live.provider.as_ref(), &model_id);
        let image_policy = crate::models::image_policy_for(&live.provider.list_models(), &model_id);
        let session_id = crate::lock(&self.current_session).id().to_string();
        let backend = crate::registry::register_compaction(
            &mut fresh,
            &live.provider,
            &model_id,
            &session_id,
            &self.cwd,
            self.shared_prompt.clone(),
            &self.grants,
            &self.temp_dir,
        );
        let fresh = std::sync::Arc::new(fresh);
        // Gh #594: `/reload` rebuilds the backend, so it re-derives the budget
        // for the model that is live now (the config it re-read may have moved
        // the threshold or the reserve too).
        if let Some(backend) = &backend {
            let max_output = super::model_max_output(live.provider.as_ref(), &model_id);
            super::apply_summarization_budget(backend, &config, window, max_output);
        }
        let agent_config = match super::agent_config_for(
            &config,
            &live.name,
            &model_id,
            window,
            image_policy,
            &fresh,
            backend.map(|backend| backend as std::sync::Arc<dyn lca_tools::CompletionBackend>),
            &self.cwd,
            &self.flags,
            trusted,
        ) {
            Ok(agent_config) => agent_config,
            Err(err) => return fail(err),
        };
        let extensions = fresh.registered_names().len();
        drop(live);
        let mut live = crate::lock(&self.live);
        live.agent_config = agent_config;
        *crate::lock(&self.registry) = fresh;
        // Gh #204: the re-read settings take effect live too, so a
        // hand-edited `models.enabled` applies without a restart.
        *crate::lock(&self.config) = config;
        let (bindings, bindings_error) = crate::load_user_keybindings(&self.data);
        let mut notice =
            format!("reloaded: {extensions} extensions, settings, prompts, themes, keybindings");
        for warning in mcp_warnings {
            notice.push_str("\nwarning: ");
            notice.push_str(&warning);
        }
        ReloadReport {
            notice,
            themes: lca_ui::theme::theme_names_all(&crate::invoke::theme_extra_dirs(
                &self.flags,
                &self.cwd,
                &self.data,
            )),
            key_bindings: bindings,
            keybinding_error: bindings_error,
        }
    }
}

/// Persist a `/theme`/`/thinking` pick to the config file (E2); a write
/// failure is a warning the user can still act on, never a crash.
pub(super) fn persist_ui_setting(key: &str, value: Option<String>) {
    if let Err(err) = crate::persist_setting(key, value.as_deref()) {
        tracing::warn!("could not persist {key}: {err}");
    }
}

/// Persist the screen mode across runs (FR-UI-21).
pub(super) fn persist_screen_mode(fullscreen: bool) {
    let path = crate::data_dir().join("ui.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{{\"fullscreen\":{fullscreen}}}"));
}

/// The screen mode a fresh session starts in (FR-UI-21).
///
/// **The default is the main-screen (terminal scrollback) renderer (S1).**
/// S1 ported pi's bottom-anchored incremental append contract so that real
/// scrollback history survives in the terminal and the terminal's native
/// selection and scrollbar just work. The alt screen is the app-owned-selection
/// opt-in (`ui.fullscreen = true` or `/fullscreen`).
///
/// Precedence (gh #112): a persisted `ui.json` wins when present, so an
/// explicit `/fullscreen` choice survives either way; else the config
/// file's `ui.fullscreen`; else scrollback. ADR-0037 carries the dated
/// annotations.
///
/// What `ui.json` says, if anything: typed, so `{"fullscreen": false,
/// "other": true}` stays scrollback instead of matching a substring.
pub(super) enum UiJson {
    /// No file: nothing persisted.
    Absent,
    /// The persisted choice.
    Fullscreen(bool),
    /// Unparseable or non-boolean: the message names the problem.
    Malformed(String),
}

pub(super) fn read_ui_json(config_dir: &std::path::Path) -> UiJson {
    let text = match std::fs::read_to_string(config_dir.join("ui.json")) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return UiJson::Absent,
        Err(err) => return UiJson::Malformed(format!("cannot read ui.json: {err}")),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => return UiJson::Malformed(format!("ui.json is not JSON: {err}")),
    };
    match value.get("fullscreen").and_then(|flag| flag.as_bool()) {
        Some(fullscreen) => UiJson::Fullscreen(fullscreen),
        None => UiJson::Malformed("ui.json has no boolean \"fullscreen\"".to_string()),
    }
}

/// The startup renderer plus an optional warning for the transcript
/// head: `(fullscreen, warning)`. Malformed persistence falls back to
/// the config file (then scrollback) and says so loudly.
pub(super) fn initial_screen_mode(
    config_dir: &std::path::Path,
    config_fullscreen: Option<bool>,
) -> (bool, Option<String>) {
    match read_ui_json(config_dir) {
        UiJson::Absent => (config_fullscreen.unwrap_or(false), None),
        UiJson::Fullscreen(fullscreen) => (fullscreen, None),
        UiJson::Malformed(problem) => (
            config_fullscreen.unwrap_or(false),
            Some(format!("warning: {problem}; using the config default")),
        ),
    }
}
