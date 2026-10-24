//! The autocomplete chain's construction, split from `chat_commands.rs` (the
//! 1,200-line ceiling, gh #408).
//!
//! One place builds the provider the prompt editor uses: the built-in provider
//! (command names, their arguments, file paths) with the first-party trigger wrapped
//! over it. Keeping it here rather than beside the command language makes the
//! decorator's shape legible — the thing that owns `/` and `@` is constructed, then
//! a trigger is registered over it.

use std::sync::Arc;

use lca_tui::widgets::autocomplete::trigger::TriggerChain;
use lca_tui::widgets::autocomplete::{
    ArgumentCompletions, AutocompleteItem, CombinedAutocompleteProvider, SlashCommand,
};

use crate::state::UiOptions;

/// Build the autocomplete chain: the built-in provider (commands, arguments, paths)
/// with the first-party `#`-issue trigger wrapped over it (gh #408).
///
/// The trigger is registered *outside* the built-in provider, so the inner provider
/// keeps owning `/`, `@`, and paths by construction — a decorator cannot shadow a
/// token it never sees first.
pub(crate) fn provider_for(options: &UiOptions) -> TriggerChain {
    let inner = Arc::new(inner_provider_for(options));
    let mut chain = TriggerChain::new(inner, std::time::Duration::from_secs(3));
    // Gh #408: the failure to register would be a programming error, not a user
    // condition — `#` is a legal trigger by construction. Recording it as a notice
    // would be noise, so the `Result` is consumed and the chain stays valid either
    // way (an unregistered trigger simply delegates).
    let _ = chain.register(
        "#",
        "issues",
        Arc::new(crate::autocomplete_issues::IssueQuery::new(
            options.workspace.clone(),
        )),
    );
    chain
}

/// The built-in provider: command names, their arguments, and file paths.
fn inner_provider_for(options: &UiOptions) -> CombinedAutocompleteProvider {
    let models = options.models.clone();
    let providers: Vec<String> = options
        .slash_commands
        .iter()
        .filter_map(|c| c.strip_prefix('/'))
        .filter_map(|c| c.strip_suffix(".login"))
        .map(str::to_string)
        .collect();
    // A command is offered once (gh #408): the host's list legitimately repeats a
    // name - it inserts the interface commands explicitly and also derives them from
    // its dispatch table - and two identical popup rows is a defect a reader sees.
    // First occurrence wins, because the host's order is deliberate (interface
    // commands lead).
    let mut seen = std::collections::HashSet::new();
    let mut commands: Vec<SlashCommand> = options
        .slash_commands
        .iter()
        .filter(|command| {
            let name = command.strip_prefix('/').unwrap_or(command);
            seen.insert(name.to_string())
        })
        .map(|command| {
            let name = command.strip_prefix('/').unwrap_or(command).to_string();
            let argument_completions: Option<ArgumentCompletions> = match name.as_str() {
                "model" => {
                    let models = models.clone();
                    Some(Arc::new(move |prefix: &str| {
                        models
                            .iter()
                            .filter(|entry| entry.id.starts_with(prefix))
                            .map(|entry| AutocompleteItem {
                                // The insert is the raw id; only the menu
                                // row shows the label (G2).
                                value: entry.id.clone(),
                                label: entry.label.clone(),
                                description: None,
                                origin: None,
                                dim: false,
                            })
                            .collect()
                    }))
                }
                "login" => {
                    let providers = providers.clone();
                    Some(Arc::new(move |prefix: &str| {
                        providers
                            .iter()
                            .filter(|p| p.starts_with(prefix))
                            .map(|p| AutocompleteItem {
                                value: p.clone(),
                                label: p.clone(),
                                description: None,
                                origin: None,
                                dim: false,
                            })
                            .collect()
                    }))
                }
                _ => None,
            };
            SlashCommand {
                name,
                description: None,
                argument_hint: None,
                argument_completions,
            }
        })
        .collect();
    // Gh #58: templates complete with their description and hint.
    if let Some(templates) = options.hooks.prompt_templates.as_ref() {
        for template in templates() {
            if !commands.iter().any(|c| c.name == template.name) {
                commands.push(SlashCommand {
                    name: template.name,
                    description: (!template.description.is_empty()).then_some(template.description),
                    argument_hint: (!template.argument_hint.is_empty())
                        .then_some(template.argument_hint),
                    argument_completions: None,
                });
            }
        }
    }
    CombinedAutocompleteProvider::new(commands, options.workspace.clone())
}
