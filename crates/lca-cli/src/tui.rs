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

/// Enter the interactive interface for `cwd`, optionally resuming `resume`.
pub fn run(cwd: &Path, resume: Option<&str>) -> anyhow::Result<i32> {
    let data = crate::data_dir();
    let store = Arc::new(SessionStore::new(data.clone()));
    let grants = GrantStore::open(&data.join("grants.json"))
        .map_err(|err| anyhow::anyhow!("cannot open the grant store: {err}"))?;
    let trusted = grants.is_trusted(cwd);
    let config = crate::load_config(cwd, &grants, false)?;

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
    #[cfg(feature = "bundled-compaction-default")]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = {
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
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = None;
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

    let options = UiOptions {
        model_label: format!("{provider_name}/{model_id}"),
        initial_lines,
        plain: config.ui_color() == lca_config::ColorMode::Never,
        invoke_command: {
            let registry = registry.clone();
            let provider_name = provider_name.clone();
            Arc::new(move |name, argument| {
                // The generic identity commands dispatch across
                // installed providers first (FR-PROV-11); with zero
                // enabled providers FR-PROV-6's report shows instead.
                if matches!(name, "login" | "logout" | "usage") {
                    if let Some(effect) = registry.invoke_generic(name, argument, &provider_name) {
                        return effect;
                    }
                    return CommandEffect::ShowWidget(crate::no_model_message(&provider_name));
                }
                registry
                    .invoke_command(name, argument)
                    .unwrap_or(CommandEffect::None)
            })
        },
        slash_commands: {
            let mut names: Vec<String> = ["login", "logout", "usage"]
                .iter()
                .map(|name| format!("/{name}"))
                .collect();
            names.extend(
                registry
                    .command_names()
                    .into_iter()
                    .map(|name| format!("/{name}")),
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
                let mut agent = Agent::new(
                    &runner_store,
                    &runner_session,
                    provider.as_ref(),
                    &mut tools,
                    &mut grants,
                    &mut prompt,
                    proposals.as_ref(),
                    agent_config,
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
