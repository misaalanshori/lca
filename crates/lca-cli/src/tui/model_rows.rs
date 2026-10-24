//! The model rows and labels: what `/model` offers and how each row reads
//! (FR-PROV-2, gh #3, gh #396).
//!
//! Split from `display.rs` for the 1,200-line ceiling (gate 11). The rules here are
//! their own thing: which models appear, what a row's label says, and what
//! resolving an argument does. `display.rs` keeps the session and settings surfaces.

use super::ModelChoice;
use lca_protocol::CommandEffect;
use std::sync::{Arc, Mutex};

/// The model picker's text: every model the active provider offers,
/// the active one marked (FR-PROV-2 at the interface).
pub(super) fn model_picker_text(
    models: &[lca_protocol::ModelInfo],
    current: &str,
    provider: &str,
) -> String {
    if models.is_empty() {
        return "no models are offered - no provider is ready; run /login".to_string();
    }
    let active = if current.is_empty() { "none" } else { current };
    let mut lines = vec![format!("models offered (active: {active}):")];
    for model in models {
        let marker = if model.id == current { " (active)" } else { "" };
        // A model from another provider names it (gh #177); the active
        // provider's own rows read as they always did.
        let origin = match model.extras.get("provider") {
            Some(origin) if origin != provider => format!(" ({origin})"),
            _ => String::new(),
        };
        // The bundled provider's name repeats its id; don't print it twice.
        let label = if model.name == model.id {
            String::new()
        } else {
            format!(" - {}", model.name)
        };
        lines.push(format!("  {}{marker}{origin}{label}", model.id));
    }
    lines.push("set one with /model <id>".to_string());
    lines.join("\n")
}

/// One `/model` row for a provider's list: the raw id, and pi's
/// `model (provider)` label for it (issue #3). Both hosts that build a
/// model list - the live `hooks.models` hook and the startup
/// `UiOptions.models` snapshot - go through here, so the decoration rule
/// exists once and only the label half ever carries it (G2: labels are
/// display-only; selection, completion inserts, resolution, session
/// metadata, and provider calls keep the raw id).
pub(super) fn model_rows(
    models: &[lca_protocol::ModelInfo],
    provider: &str,
) -> Vec<lca_ui::ModelRow> {
    models
        .iter()
        .filter_map(|model| model_label(model, provider))
        .collect()
}

/// The `(id, label)` halves of [`model_rows`] for one model (gh #204):
/// the checklist reuses the label rule without the list shape. `None`
/// for a blank id.
pub(super) fn model_label(
    model: &lca_protocol::ModelInfo,
    provider: &str,
) -> Option<lca_ui::ModelRow> {
    let id = model.id.trim();
    if id.is_empty() {
        return None;
    }
    // Gh #396: the picker's search needs the **provider** as its own field, not
    // just as label decoration - that is what lets `openai` rank the direct entry
    // above a proxy whose label merely contains it.
    //
    // The provider is the model's own `extras["provider"]` when it names one (gh
    // #177: a model from another provider), else the caller's. Deliberately *not*
    // `extras["label"]`: that is a display name (`opencode-go` for a model served
    // by `openai-compatible`), and searching a brand name as if it were the
    // provider is how the wrong row wins.
    let billing = model
        .extras
        .get("provider")
        .map(String::as_str)
        .unwrap_or(provider)
        .to_string();
    if id.contains('/') {
        // Already provider-qualified: printing it twice helps nobody.
        return Some(lca_ui::ModelRow {
            id: id.to_string(),
            label: id.to_string(),
            provider: billing,
        });
    }
    // gh #177: a model from another provider names that
    // provider. gh #31: the row names the service that will
    // bill the call. The provider says so per model - in
    // `extras`, the carrier the WIT record already had - and
    // the caller's provider name is only the fallback for a
    // provider that carries no per-model identity (a bundled
    // or fake one).
    let foreign = model
        .extras
        .get("provider")
        .filter(|origin| *origin != provider);
    let label = foreign
        .map(String::as_str)
        .or_else(|| model.extras.get("label").map(String::as_str));
    let label = match label {
        Some(label) => format!("{id} ({label})"),
        None => format!("{id} ({provider})"),
    };
    Some(lca_ui::ModelRow {
        id: id.to_string(),
        label,
        provider: billing,
    })
}

/// One `/model` invocation: no argument lists (the picker), a known
/// argument switches the session's model everywhere it is read, an
/// unknown one refuses with the real alternatives. The cells are
/// optional only so the listing and refusal paths stay testable
/// without a live session.
pub(super) fn model_effect_on(
    models: &[lca_protocol::ModelInfo],
    provider_name: &str,
    active_provider: &str,
    argument: &str,
    model_cell: Option<&Arc<Mutex<ModelChoice>>>,
    label_cell: Option<&Arc<Mutex<String>>>,
    backend: Option<&Arc<lca_core::ext_provider::ProviderBackend>>,
) -> CommandEffect {
    let argument = argument.trim();
    if argument.is_empty() {
        let current = model_cell
            .map(|cell| {
                cell.lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .id
                    .clone()
            })
            .unwrap_or_default();
        return CommandEffect::ShowWidget(model_picker_text(models, &current, active_provider));
    }
    // EFG-041: `/model <arg>` resolves like `--model` does - exact id,
    // `profile/id`, then a fuzzy substring (gh #8's `resolve_pattern`,
    // one resolver for both surfaces). The id that lands in the cell is
    // the resolved one; a pattern nothing matches stays the id the user
    // typed, and the refusal below is what says it is not offered here.
    let target = crate::models::resolve_pattern(argument, models)
        .map(|resolved| resolved.id)
        .unwrap_or_else(|_| argument.to_string());
    match models.iter().find(|model| model.id == target) {
        Some(model) => {
            if let Some(cell) = model_cell {
                let mut current = cell.lock().unwrap_or_else(|err| err.into_inner());
                current.id = model.id.clone();
                current.window = model.context_window;
            }
            if let Some(label) = label_cell {
                // gh #31: after a switch the footer names the service
                // that will answer for *this* model - its own label out
                // of `extras`, with the caller's provider name only for
                // a model that carries no per-model identity.
                let service = model
                    .extras
                    .get("label")
                    .map(String::as_str)
                    .unwrap_or(provider_name);
                *label.lock().unwrap_or_else(|err| err.into_inner()) =
                    format!("{service}/{}", model.id);
            }
            if let Some(backend) = backend {
                backend.set_model(model.id.clone());
            }
            CommandEffect::ShowWidget(format!(
                "model for this session: {provider_name}/{}",
                model.id
            ))
        }
        None => {
            let offered: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
            CommandEffect::ShowWidget(format!(
                "no model named `{argument}` for {provider_name}; offered: {}",
                offered.join(", ")
            ))
        }
    }
}
