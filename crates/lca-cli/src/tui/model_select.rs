//! Model selection: the `/model` picker command, provider switches,
//! the cycle, and the pick path's enumeration memo (gh #240). Split
//! from `commands.rs` under the workspace's 1,200-line file ceiling;
//! the switch honors the same cells, record, and metadata writes
//! whichever list resolved it.

use std::sync::Arc;

use lca_protocol::CommandEffect;
use lca_provider::Provider as _;

use super::Ui;
use super::display::model_effect_on;

/// The last model enumeration, keyed by registry identity (gh #240).
/// The pick path serves exact-id hits from here with zero provider
/// calls; the hit guard (same registry Arc, exact id, provider still
/// enabled) makes every staleness shape fall back to a fresh listing.
#[derive(Default)]
pub(crate) struct OfferedCache {
    registry: Option<std::sync::Weak<lca_core::ExtensionRegistry>>,
    models: Vec<lca_protocol::ModelInfo>,
}

impl OfferedCache {
    /// Remember an enumeration and the registry it came from.
    pub(crate) fn fill(
        &mut self,
        registry: &std::sync::Arc<lca_core::ExtensionRegistry>,
        models: Vec<lca_protocol::ModelInfo>,
    ) {
        self.registry = Some(std::sync::Arc::downgrade(registry));
        self.models = models;
    }

    /// Drop the memo (membership flips clear eagerly; a stale miss
    /// only costs one re-enumeration, never a wrong switch).
    pub(crate) fn clear(&mut self) {
        self.registry = None;
        self.models.clear();
    }

    /// The memoized entry for an exact model id, when the registry is
    /// unchanged since the enumeration and the owning provider is
    /// still enabled. Anything else is `None` (enumerate fresh).
    pub(crate) fn get(
        &self,
        registry: &std::sync::Arc<lca_core::ExtensionRegistry>,
        id: &str,
    ) -> Option<lca_protocol::ModelInfo> {
        let owned = self.registry.as_ref().and_then(std::sync::Weak::upgrade)?;
        if !std::sync::Arc::ptr_eq(&owned, registry) {
            return None;
        }
        let found = self.models.iter().find(|model| model.id == id)?.clone();
        let owner = found.extras.get("provider")?;
        registry.provider(owner)?;
        Some(found)
    }
}

/// Pick the model (gh #236): keep the
/// session's model when the new provider offers it, else that
/// provider's default, else its first listed model (a switch whose
/// current id exists nowhere still moves).
pub(super) fn pick_model<'a>(
    listed: &'a [lca_protocol::ModelInfo],
    current: &str,
    default_id: &str,
) -> Option<&'a lca_protocol::ModelInfo> {
    listed
        .iter()
        .find(|model| model.id == current)
        .or_else(|| listed.iter().find(|model| model.id == default_id))
        .or_else(|| listed.first())
}

impl Ui {
    /// `/model`: the picker, or a named switch that moves the live cell, the
    /// footer window, and the compaction backend together.
    pub(super) fn command_model(self: &Arc<Self>, argument: &str) -> CommandEffect {
        // EFG-041: `/model sonnet:high` names the level with the model.
        // The suffix splits before resolution (an id that matches as a
        // whole keeps its colons); what it - and a model switch - does to
        // the session's level is decided below, once the target model is
        // known, because the clamp is against *that* model's set.
        let (argument, suffix_level) = {
            let (base, level) = crate::models::split_thinking(argument.trim());
            (base.to_string(), level.map(str::to_string))
        };
        // Gh #240: the picker already displayed this exact id from the
        // memoized enumeration — swap with zero provider calls. The
        // memo hit carries the full entry, so the body below runs
        // unchanged over the singleton list.
        if !argument.trim().is_empty() {
            let registry = self.registry();
            let hit = self
                .offered_cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(&registry, argument.trim());
            if let Some(picked) = hit {
                return self.command_model_resolved(vec![picked], argument, suffix_level);
            }
        }
        let models = self.offered_models();
        self.command_model_resolved(models, argument, suffix_level)
    }

    /// The `/model` body over a resolved list: fresh enumeration above,
    /// or the memoized singleton on the pick path.
    fn command_model_resolved(
        self: &Arc<Self>,
        models: Vec<lca_protocol::ModelInfo>,
        argument: String,
        suffix_level: Option<String>,
    ) -> CommandEffect {
        let argument = argument.as_str();
        let resolves = crate::models::resolve_pattern(argument, &models)
            .is_ok_and(|resolved| models.iter().any(|model| model.id == resolved.id));
        // An empty list with an env-configured endpoint means live
        // discovery is about to happen (gh #31 review): consent first,
        // off this thread so the modal can render. The list arrives
        // through `pending_models` once the answer lands, so the picker
        // opens then; until then this says what is happening instead of
        // the misleading "no models".
        if models.is_empty() && argument.trim().is_empty() && self.start_model_consent() {
            let registry = self.registry();
            let host = crate::net_consent::env_configured_host(
                &self.data,
                &self.live_name(),
                Some(registry.as_ref()),
            )
            .unwrap_or_default();
            return CommandEffect::ShowWidget(format!(
                "{host} is not granted yet - approve the prompt to list its models"
            ));
        }
        // Gh #177: a pick from another ready provider swaps the whole
        // generation (provider, model, backend, record), not just the
        // cell - the pick IS the confirmation, the way pi switches on
        // selection.
        if resolves
            && let Ok(resolved) = crate::models::resolve_pattern(argument, &models)
            && let Some(picked) = models.iter().find(|model| model.id == resolved.id)
            && picked
                .extras
                .get("provider")
                .is_some_and(|owner| *owner != self.live_name())
        {
            let target = picked.extras.get("provider").cloned().unwrap_or_default();
            return self.command_model_switch(
                &target,
                &picked.id,
                picked.context_window,
                picked.extras.get("profile").cloned(),
                suffix_level.as_deref(),
                Some(picked.clone()),
            );
        }
        // E5: the label keeps the preset identity across a model switch.
        let identity = self
            .identity_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        // The model the session ran on before this command: gh #8's
        // `model-change` record names it, and only a *change* appends one.
        let previous = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id
            .clone();
        let effect = model_effect_on(
            &models,
            &identity,
            &self.live_name(),
            argument,
            Some(&self.model_cell),
            Some(&self.label_cell),
            self.provider_backend.as_ref(),
        );
        // Keep the footer's window in step with the choice.
        let chosen = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        *self
            .context_window_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = chosen.window as u64;
        // gh #8 phase 4: the session's thinking follows the model it runs
        // on. An explicit `:level` is the user's choice, clamped into what
        // the model accepts; a model *switch* applies the new model's
        // configured default (pi's per-model precedence), else keeps the
        // session's level, clamped. A pattern that resolved to nothing
        // changes neither model nor level.
        if resolves {
            let current = self
                .thinking_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let effective = match suffix_level.as_deref() {
                Some(level) => crate::lock(&self.config).clamp_thinking(Some(level), &chosen.id),
                None if chosen.id != previous => {
                    crate::lock(&self.config).switch_thinking(current.as_deref(), &chosen.id)
                }
                None => current,
            };
            *self.thinking_cell.lock().unwrap_or_else(|p| p.into_inner()) = effective;
        }
        // ADR-0024: a switch moves the model everywhere it is read, and
        // `meta.json` is one of those readers' source (gh #20) - written
        // the moment the model is chosen, so a session that switches and
        // then closes says what it ran on. gh #8: the same moment appends
        // a `model-change` record, but only when the model actually
        // changed - reopening the picker or re-picking the active model is
        // no change.
        if !argument.trim().is_empty() && !chosen.id.is_empty() {
            let session = self.session();
            if chosen.id != previous {
                let profile = models
                    .iter()
                    .find(|model| model.id == chosen.id)
                    .and_then(|model| model.extras.get("profile").cloned());
                let record = crate::models::model_change_record(
                    Some(&previous),
                    &chosen.id,
                    &self.live_name(),
                    profile.as_deref(),
                );
                if let Err(err) = self.store.append(&session, record) {
                    return CommandEffect::ShowWidget(format!(
                        "cannot record the model change: {err}"
                    ));
                }
            }
            if let Err(err) = self
                .store
                .record_model_used(&session, &self.live_name(), &chosen.id)
            {
                return CommandEffect::ShowWidget(format!(
                    "cannot update the session metadata: {err}"
                ));
            }
        }
        effect
    }

    /// A `/model` pick from another ready provider (gh #177): swap the
    /// live generation - name, provider, agent config - then follow with
    /// the compaction backend and every cell, the thinking level, the
    /// `model-change` record carrying both sides, and `meta.json`. The
    /// pick is explicit, so no further confirmation asks.
    fn command_model_switch(
        self: &Arc<Self>,
        target: &str,
        model_id: &str,
        window: u32,
        profile: Option<String>,
        suffix_level: Option<&str>,
        cached: Option<lca_protocol::ModelInfo>,
    ) -> CommandEffect {
        match self.apply_provider_switch(target, model_id, window, profile, suffix_level, cached) {
            Ok(notice) => CommandEffect::ShowWidget(notice),
            Err(err) => CommandEffect::ShowWidget(err),
        }
    }

    /// Move this session to another provider (gh #177): the one routine
    /// both `/model` and the `/login` switch confirmation share. Returns
    /// the notice naming what now answers, or the refusal.
    pub(super) fn apply_provider_switch(
        &self,
        target: &str,
        model_id: &str,
        window: u32,
        profile: Option<String>,
        suffix_level: Option<&str>,
        cached: Option<lca_protocol::ModelInfo>,
    ) -> Result<String, String> {
        let Some(handle) = self.registry().provider(target).cloned() else {
            return Err(format!("no provider named `{target}`"));
        };
        let previous = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id
            .clone();
        let previous_provider = self.live_name();
        let provider = Arc::new(lca_core::ExtensionProvider::new_with_settings(
            handle,
            self.settings_cell.clone(),
        ));
        // Gh #240: the pick path already verified this id against the
        // memoized enumeration — re-listing here was the second WASM
        // call on every selection. Unverified callers (login switch)
        // still verify; the memo never reaches them.
        let image_policy = match cached {
            Some(ref entry) => {
                crate::models::image_policy_for(std::slice::from_ref(entry), model_id)
            }
            None => {
                let listed = provider.list_models();
                if !listed.iter().any(|model| model.id == model_id) {
                    return Err(format!("no model named `{model_id}` for {target}"));
                }
                crate::models::image_policy_for(&listed, model_id)
            }
        };
        // The new generation's identity reads like startup's (E5): the
        // stored login preset when one is known, else the extension.
        let identity =
            crate::stored_provider_preset(&self.data, target).unwrap_or_else(|| target.to_string());
        super::adopt_login_identity(&self.identity_cell, &identity);
        *self.model_cell.lock().unwrap_or_else(|p| p.into_inner()) = super::ModelChoice {
            id: model_id.to_string(),
            window,
        };
        *self
            .context_window_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = u64::from(window);
        *self.label_cell.lock().unwrap_or_else(|p| p.into_inner()) =
            format!("{identity}/{model_id}");
        if let Some(backend) = self.provider_backend.as_ref() {
            backend.set_provider(provider.clone());
            backend.set_model(model_id.to_string());
        }
        {
            let mut live = crate::lock(&self.live);
            live.name = target.to_string();
            live.provider = provider.clone();
            live.agent_config.provider = target.to_string();
            live.agent_config.model = model_id.to_string();
            live.agent_config.model_context_window = window;
            live.agent_config.image_policy = image_policy;
        }
        // The session's thinking follows the model it runs on (gh #8
        // phase 4, same rule as a same-provider switch).
        let current = self
            .thinking_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let effective = match suffix_level {
            Some(level) => crate::lock(&self.config).clamp_thinking(Some(level), model_id),
            None if model_id != previous => {
                crate::lock(&self.config).switch_thinking(current.as_deref(), model_id)
            }
            None => current,
        };
        *self.thinking_cell.lock().unwrap_or_else(|p| p.into_inner()) = effective;
        // The log's witness carries both sides (gh #8, gh #177); only an
        // actual change appends one. `meta.json` follows, the way a
        // same-provider switch writes it (ADR-0024).
        let session = self.session();
        if model_id != previous || target != previous_provider {
            let record = crate::models::model_change_record(
                Some(&previous),
                model_id,
                target,
                profile.as_deref(),
            );
            if let Err(err) = self.store.append(&session, record) {
                return Err(format!("cannot record the model change: {err}"));
            }
        }
        if let Err(err) = self.store.record_model_used(&session, target, model_id) {
            return Err(format!("cannot update the session metadata: {err}"));
        }
        Ok(format!("model for this session: {target}/{model_id}"))
    }
}

#[cfg(test)]
mod memo_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use lca_core::ExtensionRegistry;
    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::{CommandEffect, CommandSpec, DispatchError, ToolCall, ToolResult};

    use super::OfferedCache;

    /// A named provider handle with no behavior (the memo tests never
    /// call into it: registration and enablement are the registry's).
    struct StubProvider {
        label: &'static str,
    }

    impl ExtensionDispatch for StubProvider {
        fn name(&self) -> &str {
            self.label
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Provider]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: self.label.to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<CommandEffect, DispatchError> {
            Ok(CommandEffect::None)
        }

        fn provider_models(
            &self,
            _settings: &[(String, String)],
        ) -> Result<Vec<lca_protocol::ModelInfo>, DispatchError> {
            Ok(Vec::new())
        }

        fn stream_completion<'a>(
            &'a self,
            _request: lca_protocol::CompletionRequest,
            _sink: &'a dyn lca_protocol::EventSink,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn identity_login(
            &self,
        ) -> DispatchFuture<'static, Result<lca_protocol::IdentityOutcome, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::IdentityOutcome::Ok)))
        }

        fn identity_logout(
            &self,
        ) -> DispatchFuture<'static, Result<lca_protocol::IdentityOutcome, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::IdentityOutcome::Ok)))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn entry(id: &str, provider: &str, window: u32) -> lca_protocol::ModelInfo {
        lca_protocol::ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            context_window: window,
            max_tokens: 0,
            extras: BTreeMap::from([("provider".to_string(), provider.to_string())]),
        }
    }

    fn registry_with(provider: &'static str) -> Arc<ExtensionRegistry> {
        let mut registry = ExtensionRegistry::new();
        registry.register(Arc::new(StubProvider { label: provider }));
        Arc::new(registry)
    }

    // Verifies: gh #240 - an exact id hit returns the memoized entry
    // with its metadata intact (the pick path's zero-enumeration
    // swap reads window and extras from here, never the network).
    #[test]
    fn memo_serves_exact_hits_with_metadata() {
        let registry = registry_with("prov-a");
        let mut cache = OfferedCache::default();
        cache.fill(
            &registry,
            vec![entry("m1", "prov-a", 100), entry("m2", "prov-a", 200)],
        );
        let hit = cache.get(&registry, "m2").expect("exact hit");
        assert_eq!(hit.id, "m2");
        assert_eq!(hit.context_window, 200);
        assert_eq!(
            hit.extras.get("provider").map(String::as_str),
            Some("prov-a")
        );
    }

    // Verifies: gh #240 - unknown ids and empty memos miss (the slow
    // enumeration path handles them, unchanged).
    #[test]
    fn memo_misses_on_unknown_ids() {
        let registry = registry_with("prov-a");
        let mut cache = OfferedCache::default();
        assert!(cache.get(&registry, "m1").is_none(), "empty memo misses");
        cache.fill(&registry, vec![entry("m1", "prov-a", 100)]);
        assert!(cache.get(&registry, "nope").is_none());
        assert!(cache.get(&registry, "M1").is_none(), "ids match exactly");
    }

    // Verifies: gh #240 - a swapped registry (the `/reload` shape)
    // misses even for identical ids (staleness falls back, never swaps).
    #[test]
    fn memo_misses_on_registry_swap() {
        let first = registry_with("prov-a");
        let mut cache = OfferedCache::default();
        cache.fill(&first, vec![entry("m1", "prov-a", 100)]);
        let second = registry_with("prov-a");
        assert!(cache.get(&second, "m1").is_none(), "new Arc misses");
        assert!(cache.get(&first, "m1").is_some(), "old Arc still hits");
    }

    // Verifies: gh #240 - a disabled provider's models miss (the
    // `/ext` toggle path), so a pick re-enumerates instead of
    // swapping onto a dead provider.
    #[test]
    fn memo_misses_when_the_provider_is_disabled() {
        let mut inner = ExtensionRegistry::new();
        inner.register(Arc::new(StubProvider { label: "prov-a" }));
        let registry = Arc::new(inner);
        let mut cache = OfferedCache::default();
        cache.fill(&registry, vec![entry("m1", "prov-a", 100)]);
        assert!(cache.get(&registry, "m1").is_some());
        registry.set_enabled("prov-a", false);
        assert!(cache.get(&registry, "m1").is_none(), "disabled misses");
    }

    // Verifies: gh #240 - clearing drops everything (the login/logout
    // eager path).
    #[test]
    fn clearing_empties_the_memo() {
        let registry = registry_with("prov-a");
        let mut cache = OfferedCache::default();
        cache.fill(&registry, vec![entry("m1", "prov-a", 100)]);
        cache.clear();
        assert!(cache.get(&registry, "m1").is_none());
    }

    // Verifies: gh #240 - the pick path's singleton list switches
    // exactly like the full enumeration (same cells, same label,
    // same notice), so the fast path persists identically.
    #[test]
    fn singleton_list_switches_like_the_full_enumeration() {
        use super::super::ModelChoice;
        use super::super::display::model_effect_on;
        use std::sync::Mutex;

        let full = vec![entry("m1", "prov-a", 100), entry("m2", "prov-a", 200)];
        let run = |models: &[lca_protocol::ModelInfo]| {
            let model_cell = Arc::new(Mutex::new(ModelChoice {
                id: "m1".to_string(),
                window: 100,
            }));
            let label_cell = Arc::new(Mutex::new("prov-a/m1".to_string()));
            let effect = model_effect_on(
                models,
                "prov-a",
                "prov-a",
                "m2",
                Some(&model_cell),
                Some(&label_cell),
                None,
            );
            let text = match effect {
                lca_protocol::CommandEffect::ShowWidget(text) => text,
                other => panic!("a notice, got {other:?}"),
            };
            let model = model_cell
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            let label = label_cell
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            (model, label, text)
        };
        let (full_model, full_label, full_text) = run(&full);
        let (single_model, single_label, single_text) = run(&full[1..]);
        assert_eq!(single_model.id, full_model.id);
        assert_eq!(single_model.window, full_model.window);
        assert_eq!(single_label, full_label);
        assert_eq!(single_text, full_text);
    }
}

#[cfg(test)]
mod pick_tests {
    use super::pick_model;

    fn models() -> Vec<lca_protocol::ModelInfo> {
        ["b-one", "a-two"]
            .iter()
            .map(|id| lca_protocol::ModelInfo {
                id: id.to_string(),
                name: id.to_string(),
                context_window: 0,
                max_tokens: 0,
                extras: Default::default(),
            })
            .collect()
    }

    // Verifies: gh #236 - a switch keeps the current model when
    // offered, else the provider default, else the first listed model
    // (a current id that exists nowhere still moves).
    #[test]
    fn switch_pick_prefers_current_then_default_then_first() {
        let listed = models();
        assert_eq!(pick_model(&listed, "a-two", "b-one").unwrap().id, "a-two");
        assert_eq!(pick_model(&listed, "missing", "b-one").unwrap().id, "b-one");
        assert_eq!(
            pick_model(&listed, "missing", "also-missing").unwrap().id,
            "b-one",
            "first listed, not 'offers no models'"
        );
        assert!(pick_model(&[], "x", "y").is_none());
    }
}
