//! Display formatting and the model-choice effects (S1): the model
//! picker's text and switch, the session age label, and the stats story.
//! Pure functions over records and store reads - no terminal, no loop.

use std::sync::{Arc, Mutex};

use lca_protocol::{CommandEffect, Record};
use lca_session::{Session, SessionStore, ViewMode};

use super::ModelChoice;

/// A short relative age (`now`, `5m`, `3h`, `2d`, `3w`, `2mo`, `1y`), pi's
/// session-row format (`selectors-large.md`).
pub(super) fn age_label(now_ms: u64, then_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(then_ms) / 1000;
    match seconds {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        604_800..=2_591_999 => format!("{}w", seconds / 604_800),
        2_592_000..=31_535_999 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// The model picker's text: every model the active provider offers,
/// the active one marked (FR-PROV-2 at the interface).
pub(super) fn model_picker_text(models: &[lca_protocol::ModelInfo], current: &str) -> String {
    if models.is_empty() {
        return "no models are offered by the active provider".to_string();
    }
    let active = if current.is_empty() { "none" } else { current };
    let mut lines = vec![format!("models offered (active: {active}):")];
    for model in models {
        let marker = if model.id == current { " (active)" } else { "" };
        // The bundled provider's name repeats its id; don't print it twice.
        let label = if model.name == model.id {
            String::new()
        } else {
            format!(" - {}", model.name)
        };
        lines.push(format!("  {}{marker}{label}", model.id));
    }
    lines.push("set one with /model <id>".to_string());
    lines.join("\n")
}

/// One `/model` invocation: no argument lists (the picker), a known
/// argument switches the session's model everywhere it is read, an
/// unknown one refuses with the real alternatives. The cells are
/// optional only so the listing and refusal paths stay testable
/// without a live session.
pub(super) fn model_effect_on(
    models: &[lca_protocol::ModelInfo],
    provider_name: &str,
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
        return CommandEffect::ShowWidget(model_picker_text(models, &current));
    }
    match models.iter().find(|model| model.id == argument) {
        Some(model) => {
            if let Some(cell) = model_cell {
                let mut current = cell.lock().unwrap_or_else(|err| err.into_inner());
                current.id = model.id.clone();
                current.window = model.context_window;
            }
            if let Some(label) = label_cell {
                *label.lock().unwrap_or_else(|err| err.into_inner()) =
                    format!("{provider_name}/{}", model.id);
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

/// The stats story (FR-UI-19): the same numbers the footer accumulates,
/// with per-model cost and cache waste.
pub(crate) fn session_stats(store: &SessionStore, session: &Session) -> String {
    let Ok(read) = store.read_with(session, ViewMode::Display) else {
        return "statistics unavailable".to_string();
    };
    let mut messages = 0usize;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    let mut cost = 0.0f64;
    // FR-UI-19: tokens per model, keyed by `provider/model`.
    let mut per_model: std::collections::BTreeMap<String, (u64, u64)> =
        std::collections::BTreeMap::new();
    for record in &read.records {
        match record {
            Record::User { .. } => messages += 1,
            Record::Assistant {
                model,
                provider,
                usage,
                ..
            } => {
                messages += 1;
                if let Some(usage) = usage {
                    input += usage.input;
                    output += usage.output;
                    cache_read += usage.cache_read;
                    cache_write += usage.cache_write;
                    cost += usage.cost;
                    let entry = per_model
                        .entry(model_label_for(provider.as_deref(), model.as_deref()))
                        .or_insert((0, 0));
                    // Cache reads/writes are billed as input on most
                    // endpoints, so they count toward the input side.
                    entry.0 += usage.input + usage.cache_read + usage.cache_write;
                    entry.1 += usage.output;
                }
            }
            _ => {}
        }
    }
    let waste = lca_session::compute_cache_waste(&read.records, 1024);
    // OpenAI-shaped endpoints report no pricing, so cost is 0; printing
    // `$0.0000` would read as "this was free" rather than "unknown".
    let cost_part = if cost > 0.0 {
        format!(", cost ${cost:.4}")
    } else {
        String::new()
    };
    let waste_cost = if waste.missed_cost > 0.0 {
        format!(" / ${:.4}", waste.missed_cost)
    } else {
        String::new()
    };
    // Only break down by model when more than one was used; a single-model
    // session would just repeat the total.
    let per_model_part = if per_model.len() > 1 {
        let parts: Vec<String> = per_model
            .iter()
            .map(|(model, (i, o))| format!("{model}: in {i}, out {o}"))
            .collect();
        format!("; by model: {}", parts.join("; "))
    } else {
        String::new()
    };
    format!(
        "{messages} messages, in {input} tokens (cache read {cache_read}, cache write {cache_write}), \
         out {output} tokens{cost_part}; cache waste {} tokens{waste_cost} across {} misses{per_model_part}",
        waste.missed_tokens, waste.miss_count
    )
}

/// `provider/model` when both are known, else whichever is, else `unknown`.
fn model_label_for(provider: Option<&str>, model: Option<&str>) -> String {
    match (provider, model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        (_, Some(model)) => model.to_string(),
        (Some(provider), None) => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::ModelInfo;

    fn models(ids: &[&str]) -> Vec<ModelInfo> {
        ids.iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                name: format!("Model {id}"),
                context_window: 100_000,
                max_tokens: 8_192,
            })
            .collect()
    }

    // Verifies: FR-PROV-2 (the model picker lists every model the
    // active provider offers - the /model built-in's listing half,
    // the interface where the provider world's listing reaches a
    // human).
    #[test]
    fn the_model_picker_lists_every_offered_model_and_marks_the_active_one() {
        let text = model_picker_text(&models(&["alpha", "beta"]), "beta");
        assert!(text.contains("alpha"), "first model listed:\n{text}");
        assert!(text.contains("beta"), "second model listed:\n{text}");
        assert!(
            text.contains("beta") && text.contains("(active)"),
            "the active model is marked:\n{text}"
        );
    }

    #[test]
    fn an_unknown_model_is_refused_with_the_real_alternatives() {
        let offered = models(&["alpha", "beta"]);
        let effect = model_effect_on(&offered, "openai-compatible", "gamma", None, None, None);
        let CommandEffect::ShowWidget(text) = effect else {
            panic!("an unknown model answers with text, not an action")
        };
        assert!(text.contains("gamma"), "names the mistake: {text}");
        assert!(
            text.contains("alpha") && text.contains("beta"),
            "offers the real list: {text}"
        );
    }

    // Verifies: FR-UI-19 (the stats story: tokens, cost, and cache waste
    // per model) and FR-UI-2 (the stats line does not print `$0.0000` when
    // the provider reports no pricing - that reads as "free", not
    // "unknown").
    #[test]
    fn stats_omit_the_cost_when_no_price_is_reported() {
        let root = lca_testkit::scratch_path("lca-stats");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let text = session_stats(&store, &session);
        assert!(!text.contains('$'), "no dead cost at zero:\n{text}");
        assert!(text.contains("0 messages"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ages_read_like_pis_session_rows() {
        assert_eq!(age_label(1_000_000, 1_000_000), "now");
        assert_eq!(age_label(1_000_000 + 5 * 60_000, 1_000_000), "5m");
        assert_eq!(age_label(1_000_000 + 3 * 3_600_000, 1_000_000), "3h");
        assert_eq!(age_label(1_000_000 + 2 * 86_400_000, 1_000_000), "2d");
    }
}
