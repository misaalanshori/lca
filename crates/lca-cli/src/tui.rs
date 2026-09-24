//! Interactive mode: resolve sessions, provider, and tools, then run the
//! terminal interface against the real agent loop.

use std::path::Path;
use std::sync::{Arc, Mutex};

use lca_core::{Agent, AgentConfig};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, Proposals};
use lca_protocol::CommandEffect;
use lca_protocol::Record;
use lca_session::{Session, SessionStore, ViewMode};
use lca_tools::{NativeOps, ToolExecutor};
use lca_tui::{PromptRequest, TurnRunner, UiOptions};

/// The built-in slash slots the interface itself claims; the spec's
/// sixth built-in, `/stats`, arrives from the native hooks extension
/// that holds the stats source (ADR-0013).
const BUILTIN_SLOTS: [&str; 5] = ["login", "logout", "usage", "model", "compact"];

/// The session's live model: the runner reads it per turn, the
/// status-line label follows it, and the compaction backend is moved
/// with `set_model` - `/model` rewrites all three.
#[derive(Clone, Debug)]
struct ModelChoice {
    /// The model identifier.
    id: String,
    /// Its context window (the threshold math needs it).
    window: u32,
}

/// The model picker's text: every model the active provider offers,
/// the active one marked (FR-PROV-2 at the interface).
fn model_picker_text(models: &[lca_protocol::ModelInfo], current: &str) -> String {
    if models.is_empty() {
        return "no models are offered by the active provider".to_string();
    }
    let mut lines = vec![format!("model picker (active: {current}):")];
    for model in models {
        let marker = if model.id == current { " (active)" } else { "" };
        lines.push(format!("  {}{} - {}", model.id, marker, model.name));
    }
    lines.push("set one with /model <id>".to_string());
    lines.join("\n")
}

/// One `/model` invocation: no argument lists (the picker), a known
/// argument switches the session's model everywhere it is read, an
/// unknown one refuses with the real alternatives. The cells are
/// optional only so the listing and refusal paths stay testable
/// without a live session.
fn model_effect_on(
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

/// Enter the interactive interface for `cwd`, optionally resuming `resume`.
pub fn run(cwd: &Path, resume: Option<&str>) -> anyhow::Result<i32> {
    let data = crate::data_dir();
    let store = Arc::new(SessionStore::new(data.clone()));
    let grants = GrantStore::open(&data.join("grants.json"))
        .map_err(|err| anyhow::anyhow!("cannot open the grant store: {err}"))?;
    let trusted = grants.is_trusted(cwd);
    let config = crate::load_config(cwd, &grants, false)?;
    // Today's update check, if enabled and due: stamped, then spawned
    // - the startup path never waits on it (FR-CFG-6), and the status
    // line picks the finding up from the shared cell once it lands.
    let update_notice = std::sync::Arc::new(std::sync::OnceLock::new());
    crate::update::spawn(config.update_check(false), Some(update_notice.clone()));

    let session = match resume {
        Some(id) => match store.session(cwd, id) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: {err}");
                return Ok(crate::exit::SESSION);
            }
        },
        None => store
            .create_session(cwd, "session")
            .map_err(|err| anyhow::anyhow!("cannot start a session: {err}"))?,
    };

    let provider_name = config.provider().to_string();

    let records = store
        .read_with(&session, ViewMode::Display)
        .map_err(|err| anyhow::anyhow!("cannot read the session: {err}"))?
        .records;
    let initial_lines: Vec<String> = records.iter().filter_map(display_line).collect();

    let tools = Arc::new(Mutex::new(ToolExecutor::new(
        Arc::new(NativeOps),
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        config.tool_result_limit_bytes() as usize,
        std::time::Duration::from_secs(config.tool_timeout_seconds()),
    )));
    let grants = Arc::new(Mutex::new(grants));
    let proposals: Option<Proposals> = trusted.then(|| config.permissions_proposals().clone());
    let stats_store = store.clone();
    let stats_session = session.clone();
    // First-party, native-linked extensions (ADR-0013): hooks-example
    // provides the reference policy and fills the /stats built-in slot,
    // which is the Phase 2 move of that behavior out of the TUI.
    let mut registry = lca_core::ExtensionRegistry::new();
    // Installed extensions first: an installed copy shadows the bundled
    // one of the same name (the duplicate rule disables the later
    // registration), and FR-DIST-8's load goes by the lockfile digest.
    crate::ext::load_installed(
        &mut registry,
        cwd,
        config.extensions_log_limit_bytes() as usize,
    );
    for handle in lca_ext_native::default_native_extensions(Arc::new(move || {
        session_stats(&stats_store, &stats_session)
    })) {
        registry.register(handle);
    }
    #[cfg(feature = "bundled-openai-compat")]
    registry.register(Arc::new(openai_compatible::OpenAiCompat::new(
        crate::openai_capabilities(cwd),
    )));
    crate::apply_enablement(&mut registry, |name| {
        grants.lock().expect("grants").extension_enabled(cwd, name) == Some(false)
    });
    // FR-PROV-6 before anything reaches the interface: the configured
    // provider must resolve to an enabled handle. (The registry stays
    // mutable until the two completion-dependent handles are in.)
    let provider: Arc<dyn lca_provider::Provider> = match registry.provider(&provider_name) {
        Some(handle) => Arc::new(lca_core::ExtensionProvider::new(handle.clone())),
        None => {
            eprintln!("{}", crate::no_model_message(&provider_name));
            return Ok(crate::exit::USAGE);
        }
    };
    let model_id = {
        let configured = config.model().unwrap_or_default();
        if configured.is_empty() {
            provider
                .list_models()
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| provider_name.clone())
        } else {
            configured.to_string()
        }
    };
    // The default compaction strategy asks THIS provider through the
    // `completion` capability; the same backend Arc drains its usage
    // onto the compaction record (capability catalog: spend shows in
    // session cost).
    // The default compaction strategy asks THIS provider through the
    // `completion` capability; the same backend Arc drains its usage
    // onto the compaction record (capability catalog: spend shows in
    // session cost). It stays concrete, not just a trait object, so
    // /model can move it with everything else the session reads.
    #[cfg(feature = "bundled-compaction-default")]
    let provider_backend: Option<Arc<lca_core::ext_provider::ProviderBackend>> = {
        let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
            provider.clone(),
            model_id.clone(),
            session.id().to_string(),
        ));
        let cap = crate::extension_capabilities(
            cwd,
            "compaction-default",
            compaction_default::manifest_grants(),
        );
        cap.set_completion(backend.clone());
        registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
        Some(backend)
    };
    #[cfg(not(feature = "bundled-compaction-default"))]
    let provider_backend: Option<Arc<lca_core::ext_provider::ProviderBackend>> = None;
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = provider_backend
        .clone()
        .map(|backend| backend as Arc<dyn lca_tools::CompletionBackend>);
    #[cfg(feature = "bundled-skills")]
    registry.register(Arc::new(skills::Skills::new(
        crate::extension_capabilities(cwd, "skills", skills::manifest_grants()),
    )));
    crate::apply_enablement(&mut registry, |name| {
        grants.lock().expect("grants").extension_enabled(cwd, name) == Some(false)
    });
    let registry = Arc::new(registry);

    let agent_config = AgentConfig {
        provider: provider_name.clone(),
        model: model_id.clone(),
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: registry.clone(),
        compaction_threshold: config.compaction_threshold(),
        model_context_window: provider
            .list_models()
            .iter()
            .find(|model| model.id == model_id)
            .map(|model| model.context_window)
            .unwrap_or(0),
        completion_backend,
        ..AgentConfig::default()
    };

    // The ui view over the registry: only handles that declared
    // regions register here (FR-UI-1's pull table; deny-by-default
    // comes from ui_regions returning empty).
    let render_regions = {
        let registry = registry.clone();
        Some(std::sync::Arc::new(
            move |region: &str| -> Vec<(String, lca_protocol::WidgetTree)> {
                registry
                    .enabled()
                    .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                    .filter_map(|handle| {
                        handle
                            .render(region)
                            .ok()
                            .flatten()
                            .map(|tree| (handle.name().to_string(), tree))
                    })
                    .collect()
            },
        ) as lca_tui::RegionRenderer)
    };
    let ui_events = {
        let registry = registry.clone();
        Some(std::sync::Arc::new(
            move |region: &str,
                  input: &lca_protocol::UiInput|
                  -> Option<(String, lca_protocol::UiEffect)> {
                registry
                    .enabled()
                    .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                    .find_map(|handle| {
                        handle
                            .on_ui_event(region, input)
                            .ok()
                            .map(|effect| (handle.name().to_string(), effect))
                    })
            },
        ) as lca_tui::RegionInteractor)
    };

    // The session's live model: /model rewrites the cell, the runner
    // reads it per turn, the status line reads the label every frame.
    let model_cell = Arc::new(Mutex::new(ModelChoice {
        id: model_id.clone(),
        window: agent_config.model_context_window,
    }));
    let label_cell = Arc::new(Mutex::new(format!("{provider_name}/{model_id}")));
    let options = UiOptions {
        model_label: label_cell.clone(),
        initial_lines,
        plain: config.ui_color() == lca_config::ColorMode::Never,
        invoke_command: {
            let registry = registry.clone();
            let provider_name = provider_name.clone();
            let provider = provider.clone();
            let model_cell = model_cell.clone();
            let label_cell = label_cell.clone();
            let provider_backend = provider_backend.clone();
            let store = store.clone();
            let session = session.clone();
            let extensions = agent_config.extensions.clone();
            let completion_backend = agent_config.completion_backend.clone();
            Arc::new(move |name, argument| match name {
                // The model picker and the manual compact: the two
                // spec-named slots the host itself fills, both routed
                // through surfaces that already exist (the provider
                // world's listing; the compaction world's strategy).
                "model" => {
                    let models = provider.list_models();
                    model_effect_on(
                        &models,
                        &provider_name,
                        argument,
                        Some(&model_cell),
                        Some(&label_cell),
                        provider_backend.as_ref(),
                    )
                }
                "compact" => match lca_core::compact_now(
                    store.clone(),
                    session.clone(),
                    extensions.clone(),
                    completion_backend.clone(),
                ) {
                    Ok(summary) => CommandEffect::ShowWidget(format!("compacted: {summary}")),
                    Err(detail) => {
                        CommandEffect::ShowWidget(format!("nothing was compacted: {detail}"))
                    }
                },
                // The generic identity commands dispatch across
                // installed providers first (FR-PROV-11); with zero
                // enabled providers FR-PROV-6's report shows instead.
                "login" | "logout" | "usage" => {
                    if let Some(effect) = registry.invoke_generic(name, argument, &provider_name) {
                        effect
                    } else {
                        CommandEffect::ShowWidget(crate::no_model_message(&provider_name))
                    }
                }
                _ => registry
                    .invoke_command(name, argument)
                    .unwrap_or(CommandEffect::None),
            })
        },
        render_regions,
        ui_events,
        update_notice: Some(update_notice),
        slash_commands: {
            let mut names: Vec<String> = BUILTIN_SLOTS
                .iter()
                .map(|name| format!("/{name}"))
                .collect();
            // Extension command names reach completion (and the screen);
            // sanitized because an extension chose these strings.
            names.extend(
                registry
                    .command_names()
                    .into_iter()
                    .map(|name| format!("/{}", lca_tui::sanitize_text(&name))),
            );
            names
        },
        workspace: cwd.to_path_buf(),
    };

    let runner_store = store.clone();
    let runner_session = session.clone();
    let runner: TurnRunner = Box::new(move |text, channels, cancel| {
        let tools = tools.clone();
        let grants = grants.clone();
        let runner_store = runner_store.clone();
        let runner_session = runner_session.clone();
        let provider = provider.clone();
        let agent_config = agent_config.clone();
        let proposals = proposals.clone();
        let model_cell = model_cell.clone();
        std::thread::spawn(move || {
            let mut sink = ChannelSink {
                tx: channels.events.clone(),
            };
            let mut prompt = UiPrompt {
                tx: channels.prompt,
            };
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    return lca_core::TurnOutcome {
                        status: lca_core::TurnStatus::Error,
                        stop_reason: lca_core::StopReason::Error,
                        usage: Default::default(),
                        error: Some(format!("cannot start the async runtime: {err}")),
                    };
                }
            };
            let mut tools = tools.lock().expect("tools lock");
            let mut grants = grants.lock().expect("grants lock");
            runtime.block_on(async {
                // The session's model is whatever /model last set:
                // the status line and the compaction backend follow
                // the same cell, so every consumer agrees per turn.
                let choice = model_cell
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let mut turn_config = agent_config;
                turn_config.model = choice.id;
                turn_config.model_context_window = choice.window;
                let mut agent = Agent::new(
                    &runner_store,
                    &runner_session,
                    provider.as_ref(),
                    &mut tools,
                    &mut grants,
                    &mut prompt,
                    proposals.as_ref(),
                    turn_config,
                );
                agent.run_turn(&text, &mut sink, &cancel).await
            })
        })
    });

    lca_tui::run(options, runner)
}

/// One conversation line for the scrollback.
fn display_line(record: &Record) -> Option<String> {
    Some(match record {
        Record::User { content, .. } => format!("user: {content}"),
        Record::Assistant { content, .. } => {
            let text: String = content
                .iter()
                .filter_map(|block| match block {
                    lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
                    lca_protocol::ContentBlock::ToolCall { name, .. } => {
                        Some(format!("[{name} requested]"))
                    }
                    lca_protocol::ContentBlock::Reasoning { .. } => None,
                })
                .collect::<Vec<String>>()
                .join(" ");
            if text.is_empty() {
                return None;
            }
            format!("assistant: {text}")
        }
        Record::ToolCall {
            name, arguments, ..
        } => format!("> {name}({arguments})"),
        Record::ToolResult {
            status, content, ..
        } => format!(
            "result: {}: {}",
            match status {
                lca_protocol::ToolResultStatus::Ok => "ok",
                lca_protocol::ToolResultStatus::Error => "error",
                lca_protocol::ToolResultStatus::Denied => "denied",
                lca_protocol::ToolResultStatus::Timeout => "timeout",
            },
            content.as_deref().unwrap_or("")
        ),
        Record::Compaction { summary, .. } => format!("[compacted: {summary}]"),
        Record::SessionStart { working_dir, .. } => format!("[session in {working_dir}]"),
        _ => return None,
    })
}

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
    for record in &read.records {
        match record {
            Record::User { .. } => messages += 1,
            Record::Assistant { usage, .. } => {
                messages += 1;
                if let Some(usage) = usage {
                    input += usage.input;
                    output += usage.output;
                    cache_read += usage.cache_read;
                    cache_write += usage.cache_write;
                    cost += usage.cost;
                }
            }
            _ => {}
        }
    }
    let waste = lca_session::compute_cache_waste(&read.records, 1024);
    format!(
        "{messages} messages, in {input} tokens (cache read {cache_read}, cache write {cache_write}), \
         out {output} tokens, cost ${cost:.4}; cache waste {} tokens / ${1:.4} across {2} misses",
        waste.missed_tokens, waste.missed_cost, waste.miss_count
    )
}

/// Forwards turn events to the interface.
struct ChannelSink {
    tx: std::sync::mpsc::SyncSender<lca_core::TurnEvent>,
}

impl lca_core::TurnSink for ChannelSink {
    fn on_event(&mut self, event: lca_core::TurnEvent) {
        let _ = self.tx.send(event);
    }
}

/// The interactive permission prompt: shows the exact action (FR-UI-4) and
/// blocks the worker until the user answers (FR-UI-6's modal rules are
/// enforced by the interface, which only opens it outside a running turn's
/// input path).
struct UiPrompt {
    tx: std::sync::mpsc::SyncSender<PromptRequest>,
}

impl PermissionPrompt for UiPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if self
            .tx
            .send(PromptRequest {
                action: action.display(),
                respond,
            })
            .is_err()
        {
            return Decision::Denied;
        }
        response.recv().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool {
        let mut display = String::from("Project proposals changed:\n");
        for (pattern, note) in &diff.added {
            display.push_str(&format!("  + {pattern} ({note})\n"));
        }
        for (pattern, note) in &diff.removed {
            display.push_str(&format!("  - {pattern} ({note})\n"));
        }
        display.push_str("Apply the new set?");
        let (respond, response) = std::sync::mpsc::sync_channel(1);
        if self
            .tx
            .send(PromptRequest {
                action: display,
                respond,
            })
            .is_err()
        {
            return false;
        }
        !matches!(
            response.recv().unwrap_or(Decision::Denied),
            Decision::Denied
        )
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

    // SRDD's interface section: the six built-in slots, of which five
    // live in this list and /stats arrives from the native hooks
    // extension that holds the stats source.
    #[test]
    fn the_spec_named_builtins_are_claimed() {
        for slot in ["login", "logout", "usage", "model", "compact"] {
            assert!(
                BUILTIN_SLOTS.contains(&slot),
                "/{slot} is a built-in the interface claims"
            );
        }
    }
}
