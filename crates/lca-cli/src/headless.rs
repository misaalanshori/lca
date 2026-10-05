//! Headless mode: one turn, the `--json` envelope, and the exit-code map
//! (S3's ceiling split; the documented contract is `docs/headless.md`).

use super::*;

/// Maps a turn outcome onto the documented exit codes.
pub fn exit_code(outcome: &TurnOutcome, needed_approval: bool, error_class: Option<&str>) -> i32 {
    if needed_approval {
        return exit::PERMISSION;
    }
    match outcome.status {
        TurnStatus::Ok => exit::OK,
        TurnStatus::Error => match outcome.stop_reason {
            StopReason::IterationLimit | StopReason::Cancelled => exit::ABORTED,
            StopReason::Error => match error_class {
                Some("transport") | Some("auth") | Some("invalid") => exit::PROVIDER,
                Some("internal") => exit::INTERNAL,
                Some(_) => exit::ABORTED,
                None => exit::INTERNAL,
            },
            StopReason::Stop => exit::OK,
        },
    }
}

/// The sink headless mode renders through: `--json` envelopes or plain
/// final text (`docs/headless.md`).
pub struct HeadlessSink {
    /// Emit one JSON object per line.
    pub json: bool,
    /// The session loaded with a truncation warning.
    pub session_truncated: bool,
    pub(crate) plain: String,
    last_error_class: Option<String>,
    out: std::io::Stdout,
}

impl HeadlessSink {
    /// A sink writing to stdout.
    pub fn new(json: bool, session_truncated: bool) -> HeadlessSink {
        HeadlessSink {
            json,
            session_truncated,
            plain: String::new(),
            last_error_class: None,
            out: std::io::stdout(),
        }
    }

    /// The class of the last error event, for exit-code mapping.
    pub fn error_class(&self) -> Option<&str> {
        self.last_error_class.as_deref()
    }

    fn emit(&mut self, line: serde_json::Value) {
        let _ = writeln!(self.out, "{line}");
    }
}

impl TurnSink for HeadlessSink {
    fn on_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::AssistantText(text) => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "text", "content": text }));
                } else {
                    // A tool-using turn has several assistant messages (a
                    // preamble before the tool, then the answer). Gluing
                    // them ran the sentences together.
                    if !self.plain.is_empty() {
                        self.plain.push_str("\n\n");
                    }
                    self.plain.push_str(&text);
                }
            }
            TurnEvent::ToolStarted(call) if self.json => {
                self.emit(serde_json::json!({
                    "type": "tool-call",
                    "id": call.call_id,
                    "call_id": call.call_id,
                    "name": call.name,
                    "arguments": call.arguments,
                }));
            }
            TurnEvent::ToolFinished(result) if self.json => {
                self.emit(serde_json::json!({
                    "type": "tool-result",
                    "id": result.call_id,
                    "call_id": result.call_id,
                    "status": result.status,
                    "content": result.content,
                    "truncated": result.truncated,
                }));
            }
            TurnEvent::Usage(usage) if self.json => {
                self.emit(serde_json::json!({
                    "type": "usage",
                    "input": usage.input,
                    "output": usage.output,
                    "cache_read": usage.cache_read,
                    "cache_write": usage.cache_write,
                    "cache_write_1h": usage.cache_write_1h,
                    "cost": usage.cost,
                }));
            }
            TurnEvent::RetryScheduled {
                attempt,
                max,
                error,
                ..
            } if self.json => {
                self.emit(serde_json::json!({
                    "type": "error",
                    "message": format!("retry {attempt}/{max}: {error}"),
                    "class": "transport",
                    "retryable": true,
                }));
            }
            TurnEvent::Error {
                message,
                class,
                retryable,
            } => {
                self.last_error_class = Some(class.clone());
                if self.json {
                    self.emit(serde_json::json!({
                        "type": "error",
                        "message": message,
                        "class": class,
                        "retryable": retryable,
                    }));
                } else {
                    eprintln!("error: {message}");
                }
            }
            TurnEvent::TurnEnded {
                status,
                stop_reason,
            } => {
                if self.json {
                    self.emit(serde_json::json!({
                        "type": "turn-end",
                        "status": match status { TurnStatus::Ok => "ok", TurnStatus::Error => "error" },
                        "stop_reason": stop_reason_name(stop_reason),
                        "truncated_session": self.session_truncated,
                    }));
                } else if self.session_truncated {
                    eprintln!("warning: session loaded with a truncation warning");
                }
            }
            _ => {}
        }
        let _ = self.out.flush();
    }
}

fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Stop => "stop",
        StopReason::IterationLimit => "iteration-limit",
        StopReason::Cancelled => "cancelled",
        StopReason::Error => "error",
    }
}

/// The project's most recent session id, newest first from the store.
fn latest_session(store: &SessionStore, cwd: &Path) -> Option<String> {
    store
        .list_sessions(cwd)
        .ok()
        .and_then(|list| list.into_iter().next())
        .map(|summary| summary.id)
}

/// Run headless turns, in order, in one session (FR-CORE-3) and return
/// the exit code. The first failure stops the run; its outcome maps the
/// code, exactly as a single turn did before (#109 keeps one message the
/// common case and every old call site passes exactly one).
#[allow(clippy::too_many_arguments)]
pub async fn headless(
    messages: &[String],
    model_override: Option<&str>,
    session: &crate::SessionSelector,
    json: bool,
    cwd: &Path,
    attachments: &[std::path::PathBuf],
    yolo: bool,
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> i32 {
    let data = data_dir();
    let store = SessionStore::new(data.clone());
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => std::sync::Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config_flags(cwd, &lock(&grants), true, yolo, flags) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // Headless makes no request unless the option was switched on
    // (the config default is off); when it was, there is no status
    // line to report through, so stderr carries the notice.
    crate::update::spawn(config.update_check(true), None);
    // #96: installed here too, so headless restores the terminal even
    // when reached without `main`. Idempotent.
    lca_tui::install_panic_hook();
    // Print mode with nothing to run is a usage error, said out loud
    // (a bare `-p` with no message). Interactive mode would open the
    // TUI; headless has nothing to turn into.
    if messages.is_empty() {
        eprintln!(
            "error: no prompt given; pass `-p <text>`, `--prompt <text>`, or a positional message"
        );
        return exit::USAGE;
    }
    let provider_name = config.provider().to_string();
    // #111: the run appends to the selected session. A continued run
    // shares the session's `log.jsonl`; only a fresh run starts one.
    let title: String = messages[0].chars().take(60).collect();
    let title = if title.is_empty() { "headless" } else { &title };
    let session = match session {
        crate::SessionSelector::New => match store.create_session(cwd, title) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: cannot start a session: {err}");
                return exit::INTERNAL;
            }
        },
        crate::SessionSelector::Continue => match latest_session(&store, cwd) {
            Some(id) => match store.session(cwd, &id) {
                Ok(session) => session,
                Err(err) => {
                    eprintln!("error: cannot open the last session: {err}");
                    return exit::SESSION;
                }
            },
            None => {
                eprintln!("error: -c continues the last session, but this project has none yet");
                return exit::SESSION;
            }
        },
        crate::SessionSelector::Resume(id) => match store.session(cwd, id) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: cannot resume session `{id}`: {err}");
                return exit::SESSION;
            }
        },
    };
    let session_truncated = store
        .read(&session)
        .map(|read| read.truncated)
        .unwrap_or(false);
    let _temp_guard = crate::SessionTempGuard;
    crate::init_session_temp(session.id());

    // ADR-0042: the same mode application as the interface, surfaced on
    // stderr because headless has no transcript to put a banner in.
    if let Some(banner) = crate::apply_permission_mode(&config, &mut lock(&grants)) {
        eprintln!("{banner}");
    }
    // ADR-0041: same resolution as the interface, surfaced on stderr
    // because headless has no transcript to put a warning in.
    let ops = crate::native_ops(&config);
    if let Some(error) = ops.error() {
        eprintln!("warning: {error}");
    }
    let mut tools = ToolExecutor::new(
        std::sync::Arc::new(ops),
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        config.tool_result_limit_bytes() as usize,
        std::time::Duration::from_secs(config.tool_timeout_seconds()),
    );
    let mut prompt_impl = HeadlessPrompt::default();
    // Extension-originated commands route through the same denying prompt, so a
    // headless approval need still surfaces as exit code 4.
    let mut shared_prompt = lca_permissions::SharedPrompt::default();
    shared_prompt.set(std::sync::Arc::new(std::sync::Mutex::new(
        prompt_impl.clone(),
    )));
    // gh #29 (QA-004): `--allow-host` attaches its one-run grant first,
    // then the endpoint's host is consented to here instead of dead-ending
    // at request time. Headless has no modal, so denial exits 4 with the
    // host and the fix named.
    if let Err(err) =
        crate::net_consent::attach_allow_hosts(&grants, cwd, allow_host, &store, &session)
    {
        eprintln!("error: {err}");
        return exit::USAGE;
    }
    if let Some(host) = crate::net_consent::env_configured_host(&data)
        && crate::provider_ready(&provider_name, &data)
        && crate::net_consent::endpoint_consent(
            &host,
            &grants,
            cwd,
            &mut shared_prompt,
            &store,
            &session,
        ) == crate::net_consent::EndpointConsent::Denied
    {
        eprintln!("{}", crate::net_consent::denied_message(&host));
        return exit::PERMISSION;
    }
    let (agent_config, provider) = match wire(
        cwd,
        &config,
        model_override,
        flags.provider.as_deref(),
        &grants,
        &shared_prompt,
        &store,
        &session,
        &provider_name,
    ) {
        Ok(wired) => wired,
        Err(code) => return code,
    };
    let proposals = if lock(&grants).is_trusted(cwd) {
        Some(config.permissions_proposals().clone())
    } else {
        None
    };
    let mut sink = HeadlessSink::new(json, session_truncated);
    let close_registry = agent_config.extensions.clone();
    // #109: one turn per message, in order, in the same session (pi's
    // print loop). The first failure stops the run; `--attach` images
    // stage onto the first message only.
    let mut outcome = None;
    for (index, message) in messages.iter().enumerate() {
        // Stage `--attach` images before the turn: content-addressed,
        // owner-only, magic-byte sniffed (ADR-0029). The stub text rides
        // in the user message so a provider without vision still sees
        // that the image exists.
        let (turn_text, attach_hashes) = if index == 0 {
            match stage_attachments(&session, attachments, message) {
                Ok(staged) => staged,
                Err(code) => return code,
            }
        } else {
            (message.clone(), Vec::new())
        };
        let turn = {
            let mut agent = Agent::new(
                &store,
                &session,
                provider.as_ref(),
                &mut tools,
                grants.clone(),
                &mut prompt_impl,
                proposals.as_ref(),
                agent_config.clone(),
            );
            agent
                .run_turn_with_attachments(
                    &turn_text,
                    &attach_hashes,
                    &mut sink,
                    &CancelFlag::new(),
                )
                .await
        };
        let failed = turn.status != TurnStatus::Ok;
        outcome = Some(turn);
        if failed {
            break;
        }
    }
    let Some(outcome) = outcome else {
        eprintln!(
            "error: no prompt given; pass `-p <text>`, `--prompt <text>`, or a positional message"
        );
        return exit::USAGE;
    };
    // `session-close`: the session is about to end (SRDD hook points).
    lca_core::drive_blocking(async move {
        close_registry.on_session_close().await;
    });
    let _ = store.close(&session);
    let class = sink.error_class().map(str::to_string);
    if !json && !sink.plain.is_empty() {
        println!("{}", sink.plain);
    }
    exit_code(&outcome, prompt_impl.needed_approval(), class.as_deref())
}

/// Stage `--attach` images onto the turn: the message text plus the
/// attachment hashes the user record carries (ADR-0029).
fn stage_attachments(
    session: &lca_session::Session,
    attachments: &[std::path::PathBuf],
    prompt: &str,
) -> Result<(String, Vec<String>), i32> {
    let mut turn_text = prompt.to_string();
    let mut attach_hashes = Vec::new();
    for path in attachments {
        match lca_core::stage_image(session, path) {
            Ok(staged) => {
                turn_text.push('\n');
                turn_text.push_str(&staged.stub);
                attach_hashes.push(staged.hash);
            }
            Err(err) => {
                eprintln!("error: {err}");
                return Err(exit::USAGE);
            }
        }
    }
    Ok((turn_text, attach_hashes))
}

/// The headless turn's wiring: the extension registry (installed, native,
/// the bundled provider, the compaction strategy), the resolved provider,
/// the model id, and the agent configuration built from them.
#[allow(clippy::too_many_arguments)]
fn wire(
    cwd: &Path,
    config: &Config,
    model_override: Option<&str>,
    provider_scope: Option<&str>,
    grants: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
    shared_prompt: &lca_permissions::SharedPrompt,
    store: &SessionStore,
    session: &lca_session::Session,
    provider_name: &str,
) -> Result<(AgentConfig, std::sync::Arc<dyn lca_provider::Provider>), i32> {
    // The one registry assembly, shared with the interface and with
    // `--list-models` (gh #8); the stats source is this session's (ADR-0013).
    let stats_store = store.clone();
    let stats_session = session.clone();
    let mut registry = crate::registry::assemble(
        cwd,
        config,
        shared_prompt.clone(),
        grants,
        Arc::new(move || crate::tui::session_stats(&stats_store, &stats_session)),
    );
    // The grant store's disable wins before the provider resolves
    // (FR-PROV-9/FR-PERM-19); applied again after the two
    // completion-dependent handles register below.
    let disabled = |name: &str| {
        grants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extension_enabled(cwd, name)
            == Some(false)
    };
    // FR-PROV-6: the configured provider must resolve to an enabled
    // handle; zero providers is an ordinary, reportable state. Resolved
    // before the completion-dependent handles register (they need it).
    let provider: Arc<dyn lca_provider::Provider> = match registry.provider(provider_name) {
        Some(handle) => Arc::new(lca_core::ExtensionProvider::new(handle.clone())),
        None => {
            eprintln!("{}", no_model_message(provider_name));
            return Err(exit::USAGE);
        }
    };
    // #111: `--model <pattern>[:thinking]` resolves against the
    // provider's list exactly the way the interface resolves it (a
    // pattern nothing matches is the id itself); otherwise the
    // configured model wins, else the first enabled one.
    let mut override_thinking: Option<String> = None;
    let model_id = match model_override {
        Some(pattern) => {
            let mut candidates = provider.list_models();
            if let Some(profile) = provider_scope {
                candidates
                    .retain(|model| crate::models::in_provider(model, profile, provider_name));
                if candidates.is_empty() {
                    eprintln!(
                        "error: unknown provider \"{profile}\". Use --list-models to see available models."
                    );
                    return Err(exit::USAGE);
                }
            }
            match crate::models::resolve_pattern(pattern, &candidates) {
                Ok(resolved) => {
                    override_thinking = resolved.thinking;
                    resolved.id
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    return Err(exit::USAGE);
                }
            }
        }
        None => {
            let configured = config.model().unwrap_or_default();
            if configured.is_empty() {
                // The enabled scope picks here too (gh #8): headless has no
                // cycle, but it agrees with the interface about which models
                // this configuration offers.
                crate::models::filter_enabled(provider.list_models(), config.models_enabled())
                    .into_iter()
                    .map(|model| model.id)
                    .find(|id| !id.is_empty())
                    .unwrap_or_else(|| provider_name.to_string())
            } else {
                configured.to_string()
            }
        }
    };
    #[cfg(feature = "bundled-compaction-default")]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = {
        let backend = Arc::new(lca_core::ext_provider::ProviderBackend::new(
            provider.clone(),
            model_id.clone(),
            session.id().to_string(),
        ));
        let cap = extension_capabilities(
            cwd,
            "compaction-default",
            compaction_default::manifest_grants(),
            shared_prompt.clone(),
            grants.clone(),
            // No `resources/` bag: a compaction strategy carries code.
            lca_tools::ResourceSource::None,
        );
        cap.set_completion(backend.clone());
        registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
        Some(backend)
    };
    #[cfg(not(feature = "bundled-compaction-default"))]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = None;
    apply_enablement(&mut registry, disabled);
    let model_context_window = provider
        .list_models()
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.context_window)
        .unwrap_or(0);
    let agent_config = AgentConfig {
        provider: provider_name.to_string(),
        model: model_id.clone(),
        // #39: the resolved model's image behavior reaches the tools.
        image_policy: crate::models::image_policy_for(&provider.list_models(), &model_id),
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: Arc::new(registry),
        compaction_threshold: config.compaction_threshold(),
        model_context_window,
        completion_backend,
        system_prompt: lca_core::identity_prompt(&model_id, std::env::consts::OS),
        skills_roots: skills_roots(cwd),
        // `--thinking` and the `thinking` key reach headless mode too: a
        // flag that works in one front end only is a flag that lies. A
        // `--model` suffix is explicit (gh #8 phase 4); otherwise the
        // model's configured default wins over the configured `thinking`,
        // clamped into what this model accepts.
        reasoning_effort: match override_thinking {
            Some(level) => config.clamp_thinking(Some(&level), &model_id),
            None => {
                config.switch_thinking(config.thinking().map(str::to_string).as_deref(), &model_id)
            }
        },
        ..AgentConfig::default()
    };
    Ok((agent_config, provider))
}
