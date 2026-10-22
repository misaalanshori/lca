//! `/persona`'s host half (gh #412): validate a bundle against this session, then
//! apply it through the seams `/model`, `/thinking`, and the tool gate already use.
//!
//! The split between [`Ui::persona_check`] and [`Ui::persona_apply`] is the atomic
//! guarantee, not a style choice: the check runs first and writes **nothing**, so a
//! bundle with one unreachable field leaves the session exactly as it was. A
//! half-applied persona is worse than a refusal, because the user believes they are
//! in a persona they are not in and nothing on screen says otherwise.
//!
//! Applying writes through the existing paths on purpose. A "current persona" cell
//! the footer could read instead would be a second owner for the same facts, and the
//! first thing it would do is disagree with them (the gh #369 lesson).

use std::sync::Arc;

use lca_ui::state::PersonaBundle;

impl super::Ui {
    /// Whether this project is trusted, for the persona listing.
    pub(super) fn is_trusted(&self) -> bool {
        crate::lock(&self.grants).is_trusted_here(&self.cwd)
    }

    /// Can this bundle be applied here?
    ///
    /// `Ok(())` only when every field it names is reachable in **this session**: the
    /// model is offered, *that* model accepts the thinking level, and every tool in
    /// the loadout is registered. Writes nothing.
    pub(super) fn persona_check(&self, bundle: &PersonaBundle) -> Result<(), String> {
        let target_model = if bundle.model.trim().is_empty() {
            None
        } else {
            let offered = self.offered_models();
            let resolved = crate::models::resolve_pattern(&bundle.model, &offered)
                .map_err(|err| format!("model `{}`: {err}", bundle.model))?;
            Some(resolved.id)
        };
        if let Some(Some(level)) = &bundle.thinking {
            // The allowed set is per model, so the level is checked against the
            // model the **persona** names, not the one running now. Switching first
            // and then discovering the level does not fit is exactly the
            // half-application this refuses.
            let model = match &target_model {
                Some(id) => id.clone(),
                None => self
                    .model_cell
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .id
                    .clone(),
            };
            if let Some(allowed) = crate::lock(&self.config).allowed_thinking_levels(&model)
                && !allowed.is_empty()
                && !allowed.iter().any(|candidate| candidate == level)
            {
                return Err(format!(
                    "`{model}` does not offer thinking level `{level}` (it offers {})",
                    allowed.join(", ")
                ));
            }
        }
        if let Some(tools) = &bundle.tools {
            let registry = self.registry();
            let unknown: Vec<&str> = tools
                .iter()
                .map(String::as_str)
                .filter(|tool| {
                    !lca_core::BUILTIN_TOOLS.contains(tool)
                        && !lca_core::BUILTIN_TOOL_ALIASES.contains(tool)
                        && registry.tool_owner(tool).is_none()
                })
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "unknown tool{} {} (registered: {})",
                    if unknown.len() == 1 { "" } else { "s" },
                    unknown.join(", "),
                    registered_tools(&registry).join(", ")
                ));
            }
        }
        Ok(())
    }

    /// Apply an already-validated bundle, returning the notice.
    ///
    /// Model first, then the level: the level is resolved against the model that is
    /// now live, so a persona naming both lands on the pair it asked for rather than
    /// on what the *previous* model allowed.
    pub(super) fn persona_apply(self: &Arc<Self>, bundle: &PersonaBundle) -> String {
        if !bundle.model.trim().is_empty() {
            let effect = self.command_model(&bundle.model);
            if let lca_protocol::CommandEffect::ShowWidget(text) = effect {
                // A model switch that reports something is one that did not simply
                // happen - a consent prompt, a refusal. The check already passed, so
                // this is the host saying more; pass it through rather than claiming
                // a success that did not occur.
                let text = text.trim().to_string();
                if !text.is_empty() {
                    return format!("persona `{}` stopped at the model: {text}", bundle.name);
                }
            }
        }
        if let Some(level) = &bundle.thinking {
            let set = self.hooks().set_thinking;
            if let Some(set) = set {
                // The host clamps and records; its own notice is not repeated here
                // because the persona's summary is the line the user asked for.
                let _ = set(level.as_deref());
            }
        }
        if let Some(tools) = &bundle.tools {
            self.apply_tool_loadout(tools);
        }
        super::display::persona_notice(bundle)
    }

    /// Enable the extensions that serve `tools`, and disable the ones that serve
    /// nothing the loadout keeps.
    ///
    /// A loadout names **tools**; enablement is per **extension**. Built-in tools
    /// (`read`, `edit`, `shell`, ...) are always available and toggle nothing, so a
    /// loadout of built-ins alone leaves the registry alone. An extension keeps its
    /// place when at least one of its tools survives.
    fn apply_tool_loadout(self: &Arc<Self>, tools: &[String]) {
        let registry = self.registry();
        let wanted: std::collections::BTreeSet<&str> = tools.iter().map(String::as_str).collect();
        let mut per_extension: std::collections::BTreeMap<String, bool> =
            std::collections::BTreeMap::new();
        for (tool, owner) in registry.callable_names() {
            let keeps = wanted.contains(tool.as_str());
            per_extension
                .entry(owner)
                .and_modify(|current| *current |= keeps)
                .or_insert(keeps);
        }
        for (name, enabled) in per_extension {
            registry.set_enabled(&name, enabled);
        }
    }
}

/// Every tool name this session could call, for a refusal that tells the user what
/// they *could* have written.
fn registered_tools(registry: &lca_core::ExtensionRegistry) -> Vec<String> {
    let mut names: Vec<String> = lca_core::BUILTIN_TOOLS
        .iter()
        .map(|tool| (*tool).to_string())
        .collect();
    names.extend(
        registry
            .callable_names()
            .into_iter()
            .map(|(tool, _owner)| tool),
    );
    names.sort();
    names.dedup();
    names
}
