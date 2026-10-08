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
/// final text (`docs/headless.md`). `--mode rpc` reuses the same sink
/// for its event stream and emits command responses beside it.
pub struct HeadlessSink {
    /// Emit one JSON object per line.
    pub json: bool,
    /// The session loaded with a truncation warning.
    pub session_truncated: bool,
    pub(crate) plain: String,
    last_error_class: Option<String>,
    out: Box<dyn std::io::Write + Send>,
}

impl HeadlessSink {
    /// A sink writing to stdout.
    pub fn new(json: bool, session_truncated: bool) -> HeadlessSink {
        HeadlessSink {
            json,
            session_truncated,
            plain: String::new(),
            last_error_class: None,
            out: Box::new(std::io::stdout()),
        }
    }

    /// A sink writing JSONL to `out` (tests pin the taxonomy here).
    pub fn with_writer(
        json: bool,
        session_truncated: bool,
        out: Box<dyn std::io::Write + Send>,
    ) -> HeadlessSink {
        HeadlessSink {
            json,
            session_truncated,
            plain: String::new(),
            last_error_class: None,
            out,
        }
    }

    /// The class of the last error event, for exit-code mapping.
    pub fn error_class(&self) -> Option<&str> {
        self.last_error_class.as_deref()
    }

    /// One JSONL record (an event or, in RPC mode, a command response).
    pub fn emit_json(&mut self, line: serde_json::Value) {
        let _ = writeln!(self.out, "{line}");
    }

    /// The session header pi's JSON mode opens with (our `session-start`
    /// record shape, not pi's file shape).
    pub fn emit_session_start(&mut self, id: &str) {
        if self.json {
            self.emit(serde_json::json!({ "type": "session-start", "id": id }));
        }
    }

    fn emit(&mut self, line: serde_json::Value) {
        self.emit_json(line);
    }
}

impl TurnSink for HeadlessSink {
    fn on_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "turn-start" }));
                }
            }
            TurnEvent::MessageStarted { role } => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "message-start", "role": role }));
                }
            }
            TurnEvent::MessageEnded { role } => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "message-end", "role": role }));
                }
            }
            TurnEvent::TextDelta(delta) => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "text-delta", "delta": delta }));
                }
            }
            TurnEvent::ReasoningDelta(delta) => {
                if self.json {
                    self.emit(serde_json::json!({ "type": "thinking-delta", "delta": delta }));
                }
            }
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
                    // Gh #77's nesting: the calling tool's id, null
                    // for model-issued calls.
                    "parent_call_id": call.parent_call_id,
                }));
            }
            TurnEvent::ToolOutputChunk { call_id, chunk } if self.json => {
                self.emit(serde_json::json!({
                    "type": "tool-update",
                    "call_id": call_id,
                    "chunk": chunk,
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
                    "exit_code": result.exit_code,
                    "full_output_path": result.full_output_path,
                    // Gh #77's bounded record rides the envelope the
                    // session log keeps (empty for calls that nested
                    // nothing).
                    "nested": result.nested.iter().map(|entry| serde_json::json!({
                        "name": entry.name,
                        "status": entry.status,
                        "content_head": entry.content_head,
                    })).collect::<Vec<_>>(),
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
                delay_ms,
                error,
            } if self.json => {
                // The `error` row stays: scripts may match on it, and the
                // headless contract removes nothing.
                self.emit(serde_json::json!({
                    "type": "error",
                    "message": format!("retry {attempt}/{max}: {error}"),
                    "class": "transport",
                    "retryable": true,
                }));
                self.emit(serde_json::json!({
                    "type": "retry-scheduled",
                    "attempt": attempt,
                    "max_attempts": max,
                    "delay_ms": delay_ms,
                    "error": error,
                }));
            }
            TurnEvent::RetryFinished { success } if self.json => {
                self.emit(serde_json::json!({
                    "type": "retry-end",
                    "success": success,
                }));
            }
            TurnEvent::CompactionStarted { reason } if self.json => {
                self.emit(serde_json::json!({
                    "type": "compaction-start",
                    "reason": reason,
                }));
            }
            TurnEvent::CompactionEnded { reason, success } if self.json => {
                self.emit(serde_json::json!({
                    "type": "compaction-end",
                    "reason": reason,
                    "success": success,
                }));
            }
            TurnEvent::UserInjected { text, mode } if self.json => {
                self.emit(serde_json::json!({
                    "type": "queue-flushed",
                    "mode": mode,
                    "text": text,
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

/// Everything a headless-family run needs after flag parsing: the store
/// and session, grants, config, provider, tools, and agent config (gh
/// #56: `-p` turns and the RPC loop share it, so the two cannot drift).
pub(crate) struct Setup {
    pub(crate) store: SessionStore,
    pub(crate) session: lca_session::Session,
    pub(crate) grants: std::sync::Arc<std::sync::Mutex<GrantStore>>,
    pub(crate) provider: std::sync::Arc<dyn lca_provider::Provider>,
    pub(crate) tools: ToolExecutor,
    pub(crate) agent_config: AgentConfig,
    pub(crate) proposals: Option<lca_permissions::Proposals>,
    pub(crate) prompt_impl: HeadlessPrompt,
    // Held for the run: dropping it removes the session temp dir.
    pub(crate) _temp_guard: crate::SessionTempGuard,
}

/// Grants, config, provider, tools, and agent config for one headless
/// session: every early exit maps the headless codes, exactly as the
/// one-shot path did before the RPC loop shared it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn setup(
    cwd: &Path,
    model_override: Option<&str>,
    session: &crate::SessionSelector,
    yolo: bool,
    allow_host: &[String],
    flags: &crate::CliFlags,
    title: &str,
) -> Result<Setup, i32> {
    let data = data_dir();
    let store = SessionStore::new(data.clone());
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => std::sync::Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return Err(exit::INTERNAL);
        }
    };
    let config = match load_config_flags(cwd, &lock(&grants), true, yolo, flags) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return Err(exit::USAGE);
        }
    };
    // Headless makes no request unless the option was switched on
    // (the config default is off); when it was, there is no status
    // line to report through, so stderr carries the notice.
    crate::update::spawn(config.update_check(true), None);
    // #96: installed here too, so headless restores the terminal even
    // when reached without `main`. Idempotent.
    lca_tui::install_panic_hook();
    let provider_name = config.provider().to_string();
    // #111: the run appends to the selected session. A continued run
    // shares the session's `log.jsonl`; only a fresh run starts one.
    let session = match session {
        crate::SessionSelector::New => match store.create_session(cwd, title) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: cannot start a session: {err}");
                return Err(exit::INTERNAL);
            }
        },
        crate::SessionSelector::Continue => match latest_session(&store, cwd) {
            Some(id) => match store.session(cwd, &id) {
                Ok(session) => session,
                Err(err) => {
                    eprintln!("error: cannot open the last session: {err}");
                    return Err(exit::SESSION);
                }
            },
            None => {
                eprintln!("error: -c continues the last session, but this project has none yet");
                return Err(exit::SESSION);
            }
        },
        crate::SessionSelector::Resume(id) => match store.session(cwd, id) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("error: cannot resume session `{id}`: {err}");
                return Err(exit::SESSION);
            }
        },
    };
    // Gh #160: the run's temp dir resolves here, from this session,
    // with creation failures fatal (never the old silent `let _`).
    let temp_dir = match crate::ensure_session_temp(&data, session.id()) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("error: cannot create the session temp dir: {err}");
            return Err(exit::INTERNAL);
        }
    };
    let temp_guard = crate::SessionTempGuard(temp_dir.clone());

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
    // Headless keeps a backstop where the interactive default is none:
    // nobody can cancel a headless run, and an unbounded hang in CI is
    // a worse failure than a timeout error (gh #40).
    let default_timeout =
        crate::configured_tool_timeout(&config).or(Some(std::time::Duration::from_secs(120)));
    let tools = ToolExecutor::new(
        std::sync::Arc::new(ops),
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        config.tool_result_limit_bytes() as usize,
        default_timeout,
    );
    let prompt_impl = HeadlessPrompt::default();
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
        return Err(exit::USAGE);
    }
    // The one registry assembly, shared with the interface and with
    // `--list-models` (gh #8); the stats source is this session's
    // (ADR-0013). Assembled before the consent check (gh #157) so the
    // provider's manifest - not a host literal - drives the env-var
    // lookup.
    let stats_store = store.clone();
    let stats_session = session.clone();
    let registry = crate::registry::assemble(
        cwd,
        &config,
        shared_prompt.clone(),
        lca_permissions::SharedDialogs::default(),
        &grants,
        Arc::new(move || crate::tui::session_stats(&stats_store, &stats_session)),
        &temp_dir,
    );
    if let Some(host) =
        crate::net_consent::env_configured_host(&data, &provider_name, Some(&registry))
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
        return Err(exit::PERMISSION);
    }
    let (agent_config, provider) = match wire(
        cwd,
        &config,
        model_override,
        flags.provider.as_deref(),
        &grants,
        &shared_prompt,
        &session,
        &provider_name,
        flags,
        registry,
        &temp_dir,
    ) {
        Ok(wired) => wired,
        Err(code) => return Err(code),
    };
    let proposals = if lock(&grants).is_trusted(cwd) {
        Some(config.permissions_proposals().clone())
    } else {
        None
    };
    Ok(Setup {
        store,
        session,
        grants,
        provider,
        tools,
        agent_config,
        proposals,
        prompt_impl,
        _temp_guard: temp_guard,
    })
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
    // Print mode with nothing to run is a usage error, said out loud
    // (a bare `-p` with no message). Interactive mode would open the
    // TUI; headless has nothing to turn into.
    if messages.is_empty() {
        eprintln!(
            "error: no prompt given; pass `-p <text>`, `--prompt <text>`, or a positional message"
        );
        return exit::USAGE;
    }
    let title: String = messages[0].chars().take(60).collect();
    let title = if title.is_empty() { "headless" } else { &title };
    let setup = match setup(cwd, model_override, session, yolo, allow_host, flags, title).await {
        Ok(setup) => setup,
        Err(code) => return code,
    };
    let Setup {
        store,
        session,
        grants,
        provider,
        mut tools,
        agent_config,
        proposals,
        mut prompt_impl,
        _temp_guard: _,
    } = setup;
    let session_truncated = store
        .read(&session)
        .map(|read| read.truncated)
        .unwrap_or(false);

    let mut sink = HeadlessSink::new(json, session_truncated);
    sink.emit_session_start(session.id());
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
    session: &lca_session::Session,
    provider_name: &str,
    flags: &crate::CliFlags,
    mut registry: lca_core::ExtensionRegistry,
    temp: &Path,
) -> Result<(AgentConfig, std::sync::Arc<dyn lca_provider::Provider>), i32> {
    // Without the compaction strategy neither the prompt nor the session
    // feeds anything downstream; the bindings stay for the default build.
    #[cfg(not(feature = "bundled-compaction-default"))]
    let _ = (&shared_prompt, &session);
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
    let summarization_backend = {
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
            temp,
        );
        cap.set_completion(backend.clone());
        registry.register(Arc::new(compaction_default::CompactionDefault::new(cap)));
        Some(backend)
    };
    #[cfg(not(feature = "bundled-compaction-default"))]
    let summarization_backend: Option<Arc<lca_core::ext_provider::ProviderBackend>> = None;
    apply_enablement(&mut registry, disabled);
    let model_context_window = provider
        .list_models()
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.context_window)
        .unwrap_or(0);
    // gh #36 phase 1: the summarization budget derives from the same
    // reserve the trigger uses, once the model window is known.
    if let Some(backend) = &summarization_backend {
        backend.set_summarization_budget(lca_core::ext_provider::summarization_max_tokens(
            lca_core::compaction_reserve(
                config.compaction_threshold(),
                config.compaction_reserve_tokens(),
                lca_core::effective_context_window(model_context_window),
            ),
        ));
    }
    #[cfg(feature = "bundled-compaction-default")]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> =
        summarization_backend.map(|backend| backend as Arc<dyn lca_tools::CompletionBackend>);
    #[cfg(not(feature = "bundled-compaction-default"))]
    let completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>> = None;
    let agent_config = AgentConfig {
        provider: provider_name.to_string(),
        model: model_id.clone(),
        // #39: the resolved model's image behavior reaches the tools.
        image_policy: crate::models::image_policy_for(&provider.list_models(), &model_id),
        retry_limit: config.provider_retry_limit() as u32,
        max_iterations: config.tool_max_iterations() as u32,
        extensions: Arc::new(registry),
        compaction_threshold: config.compaction_threshold(),
        compaction_enabled: config.compaction_enabled(),
        compaction_reserve_tokens: config.compaction_reserve_tokens(),
        compaction_keep_recent_tokens: config.compaction_keep_recent_tokens(),
        model_context_window,
        completion_backend,
        system_prompt: match crate::prompt::agent_system_prompt(
            cwd,
            &model_id,
            flags,
            lock(grants).is_trusted(cwd),
        ) {
            Ok(prompt) => prompt,
            Err(err) => {
                eprintln!("error: {err}");
                return Err(exit::USAGE);
            }
        },
        skills_roots: skills_roots(cwd),
        skills_inject_matched: config.skills_inject_matched(),
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

#[cfg(test)]
mod sink_tests {
    use super::*;
    use lca_core::TurnSink;

    struct VecWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for VecWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn sink() -> (HeadlessSink, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = HeadlessSink::with_writer(true, false, Box::new(VecWriter(out.clone())));
        (sink, out)
    }

    fn lines(out: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> Vec<serde_json::Value> {
        let bytes = out
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        String::from_utf8(bytes)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json line"))
            .collect()
    }

    fn tool_call() -> lca_protocol::ToolCall {
        lca_protocol::ToolCall {
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        }
    }

    // Verifies: gh #44 (the golden turn): a scripted turn emits the
    // taxonomy in order - turn-start, message-start, text deltas,
    // tool-call, tool-update, tool-result, usage, turn-end - so a
    // transcript rebuilds offline.
    #[test]
    fn the_json_stream_reconstructs_the_transcript() {
        let (mut sink, out) = sink();
        sink.emit_session_start("session-1");
        sink.on_event(TurnEvent::TurnStarted);
        sink.on_event(TurnEvent::MessageStarted { role: "user" });
        sink.on_event(TurnEvent::MessageEnded { role: "user" });
        sink.on_event(TurnEvent::MessageStarted { role: "assistant" });
        sink.on_event(TurnEvent::TextDelta("Hello ".into()));
        sink.on_event(TurnEvent::TextDelta("world".into()));
        sink.on_event(TurnEvent::ToolStarted(tool_call()));
        sink.on_event(TurnEvent::ToolOutputChunk {
            call_id: "call-1".into(),
            chunk: "partial".into(),
        });
        sink.on_event(TurnEvent::ToolFinished(lca_protocol::ToolResult::ok(
            "call-1", "done",
        )));
        sink.on_event(TurnEvent::Usage(lca_protocol::Usage::default()));
        sink.on_event(TurnEvent::AssistantText("Hello world".into()));
        sink.on_event(TurnEvent::MessageEnded { role: "assistant" });
        sink.on_event(TurnEvent::TurnEnded {
            status: TurnStatus::Ok,
            stop_reason: StopReason::Stop,
        });

        let types: Vec<String> = lines(&out)
            .iter()
            .map(|line| line["type"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            types,
            vec![
                "session-start",
                "turn-start",
                "message-start",
                "message-end",
                "message-start",
                "text-delta",
                "text-delta",
                "tool-call",
                "tool-update",
                "tool-result",
                "usage",
                "text",
                "message-end",
                "turn-end",
            ]
        );
        let records = lines(&out);
        assert_eq!(records[0]["id"], "session-1");
        assert_eq!(records[2]["role"], "user");
        assert_eq!(records[5]["delta"], "Hello ");
        assert_eq!(records[7]["name"], "shell");
        assert_eq!(records[8]["chunk"], "partial");
    }

    // Verifies: gh #44 (queue and compaction surface): a flushed steer
    // and a compaction round-trip name their modes and reasons.
    #[test]
    fn queue_and_compaction_events_name_their_modes() {
        let (mut sink, out) = sink();
        sink.on_event(TurnEvent::UserInjected {
            text: "faster".into(),
            mode: "steer".into(),
        });
        sink.on_event(TurnEvent::CompactionStarted {
            reason: "threshold".into(),
        });
        sink.on_event(TurnEvent::CompactionEnded {
            reason: "threshold".into(),
            success: true,
        });
        sink.on_event(TurnEvent::RetryScheduled {
            attempt: 1,
            max: 3,
            delay_ms: 2000,
            error: "overloaded".into(),
        });
        sink.on_event(TurnEvent::RetryFinished { success: true });
        let records = lines(&out);
        let types: Vec<&str> = records
            .iter()
            .map(|line| line["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            types,
            vec![
                "queue-flushed",
                "compaction-start",
                "compaction-end",
                "error",
                "retry-scheduled",
                "retry-end",
            ]
        );
        assert_eq!(records[0]["mode"], "steer");
        assert_eq!(records[2]["success"], true);
        assert_eq!(records[4]["max_attempts"], 3);
    }
}
