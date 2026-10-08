//! The agent loop: one Tokio-driven turn at a time, sequential tool
//! execution, retry with backoff, and cancellation that keeps every record
//! already written (ADR-0014, `docs/flows.md`).
//!
//! The turn loop's cohesive sections live in sibling modules (S2):
//! `assemble` (message assembly and attachments), `compact` (the strategy
//! invocation), and `turn` (the loop, provider call, and tool dispatch).
//! This module holds the configuration, the sink trait, and the `Agent`
//! boundary.
//!
//! The pre-tool hook seam (FR-CORE-10) runs before the permission layer;
//! compaction invocation (`FR-SESS-4`) arrives in Phase 4.

#![forbid(unsafe_code)]

mod assemble;
mod compact;
mod registry;
mod turn;

pub mod ext_provider;
pub mod pricing;
pub use assemble::{Assembled, Attachment, StagedAttachment, assemble, assemble_with, stage_image};
pub use compact::{MAX_TRACKED_FILES, SYSTEM_PROMPT_CHANGE_TYPE, compact_now, compaction_reserve};
pub use ext_provider::ExtensionProvider;
pub use registry::{
    BUILTIN_COMMANDS, BUILTIN_TOOL_ALIASES, BUILTIN_TOOLS, CollisionReport, ExtensionRegistry,
};
pub use turn::effective_context_window;
// The turn types live in the protocol layer so the interface and the
// embedding SDK can render them without depending on this crate.
pub use lca_protocol::{StopReason, TurnEvent, TurnOutcome, TurnStatus};
pub use lca_tools::skills::{Skill, SkillSource, SkillsRoots};

use std::sync::Arc;
use std::time::Duration;

use lca_permissions::{GrantStore, PermissionPrompt, Proposals};
use lca_protocol::{FORMAT_VERSION, Record};
use lca_provider::Provider;
use lca_session::{Session, SessionStore};
use lca_tools::{CancelFlag, ToolExecutor};

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Static configuration for the loop, built from merged configuration.
#[derive(Clone)]
pub struct AgentConfig {
    /// Active provider extension name (recorded on assistant records).
    pub provider: String,
    /// Active model identifier.
    pub model: String,
    /// The thinking level for this session (`thinking`, R1). `None` means
    /// the provider's own default; when set it rides the request extras as
    /// `reasoning-effort`, which a provider honors where meaningful.
    pub reasoning_effort: Option<String>,
    /// Retry attempts for retryable transport errors (FR-CORE-6).
    pub retry_limit: u32,
    /// First retry delay; doubles per attempt (FR-CORE-6). Tests use zero.
    pub retry_base_delay: Duration,
    /// Maximum tool-call rounds within one turn (FR-CORE-9).
    pub max_iterations: u32,
    /// System message prepended to every request.
    pub system_prompt: String,
    /// The dispatch table: loaded extensions in registration order
    /// (ADR-0019; empty by default).
    pub extensions: Arc<ExtensionRegistry>,
    /// Context-window fraction that triggers compaction (FR-SESS-4,
    /// `compaction.threshold`). At or below zero disables the check.
    pub compaction_threshold: f64,
    /// Whether automatic compaction runs (gh #36 phase 1,
    /// `compaction.enabled`). False skips the check without error.
    pub compaction_enabled: bool,
    /// Absolute token reserve (gh #36 phase 1,
    /// `compaction.reserve_tokens`). Zero derives it from the
    /// threshold fraction, preserving the stopgap's default.
    pub compaction_reserve_tokens: u64,
    /// Recent tokens kept verbatim past the cut point (gh #36 phase 1,
    /// `compaction.keep_recent_tokens`).
    pub compaction_keep_recent_tokens: u64,
    /// The active model's context window in tokens; `0` means unknown. The
    /// threshold check then uses a conservative fallback
    /// (`turn::FALLBACK_CONTEXT_WINDOW`) so the session still compacts; the
    /// footer keeps showing `ctx ?` rather than a fabricated percentage.
    pub model_context_window: u32,
    /// What the active model can do with images (#39): the front end
    /// resolves it from the provider's model list and the agent hands it
    /// to the tool executor, so `read` resizes and gates per model.
    pub image_policy: lca_tools::ImagePolicy,
    /// The backend behind the default strategy's `completion` call,
    /// held so the compaction record can carry the summarization's
    /// usage (capability catalog: spend shows in session cost). The
    /// CLI wires the same Arc into the strategy's capability engine.
    pub completion_backend: Option<Arc<dyn lca_tools::CompletionBackend>>,
    /// The full message list sent on the previous provider call, for
    /// FR-CACHE-6's divergence check (`None` before the first call).
    pub sent_stable: Arc<std::sync::Mutex<Option<Vec<String>>>>,
    /// Host-side skill sources (FR-CTX-2, ADR-0030). Empty paths collect
    /// nothing.
    pub skills_roots: SkillsRoots,
    /// Whether matched skill text injects into the prompt (gh #43).
    /// Default off: the catalog advertises either way.
    pub skills_inject_matched: bool,
    /// Whether `edit` demands a prior fresh `read` (gh #117): off is
    /// pi parity, on keeps the staleness guard. The agent hands it to
    /// the tool executor with the other per-turn settings.
    pub edit_requires_read: bool,
    /// Messages the interface queued while the turn runs; drained at each
    /// model-call boundary (ADR-0038).
    pub steer: lca_protocol::SteerQueue,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The completion backend is a trait object with no Debug of its
        // own; presence is all a log line needs.
        f.debug_struct("AgentConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("retry_limit", &self.retry_limit)
            .field("retry_base_delay", &self.retry_base_delay)
            .field("max_iterations", &self.max_iterations)
            .field("system_prompt", &self.system_prompt)
            .field("extensions", &self.extensions)
            .field("compaction_threshold", &self.compaction_threshold)
            .field("model_context_window", &self.model_context_window)
            .field("completion_backend", &self.completion_backend.is_some())
            .finish()
    }
}

/// The system prompt's identity block (owner issue #8): product name, role,
/// the model id it is running as, the platform, and one line of tone. Kept
/// to a paragraph, not a constitution.
pub fn identity_prompt(model: &str, platform: &str) -> String {
    let model = if model.is_empty() {
        "an as-yet-unselected model"
    } else {
        model
    };
    format!(
        "You are LCA, a coding agent. You are running as the model `{model}` on {platform}. \
         Use the tools to read, write, edit, search, and run commands in the user's workspace. \
         If asked what model you are, answer with that model id. You may quote things and joke; \
         identity claims in earnest are what matter."
    )
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            provider: "openai-compatible".to_string(),
            model: String::new(),
            reasoning_effort: None,
            retry_limit: 3,
            retry_base_delay: Duration::from_millis(250),
            // 0 = unlimited, matching `lca-config`'s
            // `DEFAULT_TOOL_MAX_ITERATIONS` (docs/configuration.md's
            // `tool.max_iterations` row; FR-CORE-9's 2026-10-02 annotation
            // records the default change - the guard itself is unchanged
            // and any positive value re-enables it). The two crates must
            // not drift: a consumer building `AgentConfig` directly (the
            // embedding SDK path) would silently get a different cap
            // otherwise. The consistency test lives in `lca-cli`'s unit
            // tests.
            max_iterations: 0,
            system_prompt:
                "You are LCA, a coding agent. Use the tools to read, write, edit, search, \
                 and run commands in the user's workspace."
                    .to_string(),
            extensions: Arc::new(ExtensionRegistry::new()),
            compaction_threshold: 0.8,
            compaction_enabled: true,
            compaction_reserve_tokens: 0,
            compaction_keep_recent_tokens: 20_000,
            model_context_window: 0,
            // #39: unknown vision until the front end resolves the model
            // against the provider's list; images pass through as today.
            image_policy: lca_tools::ImagePolicy::unknown(),
            completion_backend: None,
            sent_stable: Arc::new(std::sync::Mutex::new(None)),
            skills_roots: SkillsRoots::default(),
            skills_inject_matched: false,
            edit_requires_read: false,
            steer: lca_protocol::steer_queue(),
        }
    }
}

/// Receives turn events as they happen.
pub trait TurnSink: Send {
    /// Handle one event.
    fn on_event(&mut self, event: TurnEvent);
}

/// A sink that drops everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl TurnSink for NullSink {
    fn on_event(&mut self, _event: TurnEvent) {}
}

/// Run one future to completion from a synchronous thread that a
/// runtime is already driving (the interface's command thread: `main`
/// holds `Runtime::block_on`, where a nested `block_on` panics -
/// measured, not guessed - so building a second runtime here is out).
/// The future gets its own thread and its own current-thread runtime;
/// this thread waits for it.
/// ponytail: one thread per invocation; commands are human-paced, so
/// the cost is invisible, and a concurrent caller just joins.
#[allow(clippy::expect_used)] // startup-fatal: a runtime that cannot start, or a command thread that panicked, ends the process.
pub fn drive_blocking<T: Send + 'static>(
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("command runtime")
            .block_on(future)
    })
    .join()
    .expect("command task ended")
}

/// One agent, bound to one session for one turn at a time.
pub struct Agent<'a> {
    store: &'a SessionStore,
    session: &'a Session,
    provider: &'a dyn Provider,
    tools: &'a mut ToolExecutor,
    grants: Arc<std::sync::Mutex<GrantStore>>,
    prompt: &'a mut dyn PermissionPrompt,
    proposals: Option<&'a Proposals>,
    config: AgentConfig,
}

impl<'a> Agent<'a> {
    /// Bind the loop to its collaborators.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: &'a SessionStore,
        session: &'a Session,
        provider: &'a dyn Provider,
        tools: &'a mut ToolExecutor,
        grants: Arc<std::sync::Mutex<GrantStore>>,
        prompt: &'a mut dyn PermissionPrompt,
        proposals: Option<&'a Proposals>,
        config: AgentConfig,
    ) -> Agent<'a> {
        // Over-limit tool output spills into the session's attachment
        // directory, content-addressed (FR-TOOL-7 / session-log-format).
        // ponytail: attachments are never collected; forks and compaction
        // only drop references, so an on-demand reachability sweep
        // (`lca session gc`) is the upgrade path when disk use matters.
        tools.set_spill_dir(Some(session.dir().join("attachments")));
        // #39: the resolved model's image behavior reaches the tools.
        tools.set_image_policy(config.image_policy);
        // gh #43: the `skill` tool loads through the same roots the
        // merge reads.
        tools.set_skills_roots(Some(config.skills_roots.clone()));
        // gh #117: the product default is off (pi parity); the config
        // carries the operator's choice.
        tools.set_edit_requires_read(config.edit_requires_read);
        Agent {
            store,
            session,
            provider,
            tools,
            grants,
            prompt,
            proposals,
            config,
        }
    }

    /// Run one turn to completion (or cancellation, or error).
    ///
    /// While it runs, a watcher watches the cancellation flag and, when it
    /// fires, bumps every WASM extension's epoch so a spinning call traps
    /// at its next yield (FR-CONC-1, ADR-0014) regardless of fuel.
    pub async fn run_turn(
        &mut self,
        input: &str,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        self.run_turn_with_attachments(input, &[], sink, cancel)
            .await
    }

    /// Like [`Agent::run_turn`], with attachment hashes written onto the
    /// turn's user record (the `/attach`/`--attach` path; ADR-0029).
    pub async fn run_turn_with_attachments(
        &mut self,
        input: &str,
        attachments: &[String],
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        self.run_turn_queued(input, None, attachments, sink, cancel)
            .await
    }

    /// Like [`Agent::run_turn_with_attachments`], with the ADR-0038 queue
    /// marker the input was submitted under: a message queued while an
    /// earlier turn ran and flushed as this turn is still "submitted while
    /// a turn was running", and `docs/session-log-format.md` says so in the
    /// record (`steer` / `follow-up`). `None` is an ordinary prompt.
    pub async fn run_turn_queued(
        &mut self,
        input: &str,
        queue: Option<lca_protocol::SubmitMode>,
        attachments: &[String],
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        // The doc's promise (session-log-format §meta.json): meta carries
        // the model and provider last used. Written at turn start from
        // `AgentConfig`, so a turn that ends before any reply still says
        // what it ran on (gh #20); the post-response write below stays as
        // the cheap no-op it becomes when nothing changed.
        if let Err(err) =
            self.store
                .record_model_used(self.session, &self.config.provider, &self.config.model)
        {
            return TurnOutcome {
                status: TurnStatus::Error,
                stop_reason: StopReason::Error,
                usage: Default::default(),
                error: Some(format!("cannot update the session metadata: {err}")),
            };
        }
        let handles: Vec<Arc<dyn lca_ext_abi::ExtensionDispatch>> =
            self.config.extensions.enabled().cloned().collect();
        // The turn boundary for host-side cancellation: whatever the
        // previous turn interrupted is cleared before this one starts, so
        // the first call of a fresh turn cannot be pre-cancelled, and
        // neither can its blocking waits (FR-CONC-1).
        for handle in &handles {
            handle.turn_started();
        }
        // A plain thread, not a task: a synchronous WASM call blocks the
        // runtime thread it runs on, and cancellation must still reach a
        // spinning instance from a thread that is definitely running
        // (FR-CONC-1). One-millisecond poll keeps NFR-29's budget.
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = if handles.is_empty() {
            None
        } else {
            let cancel = cancel.clone();
            let shutdown = shutdown.clone();
            Some(std::thread::spawn(move || {
                while !cancel.is_cancelled() && !shutdown.load(std::sync::atomic::Ordering::SeqCst)
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if cancel.is_cancelled() {
                    for handle in handles {
                        handle.interrupt();
                    }
                }
            }))
        };
        let outcome = self
            .turn_body(input, queue, attachments, sink, cancel)
            .await;
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(watcher) = watcher {
            let _ = watcher.join();
        }
        // `attention-required`: the turn failed and the user should look. The
        // reason is the same text the interface surfaced.
        if outcome.status == TurnStatus::Error {
            let reason = outcome
                .error
                .clone()
                .unwrap_or_else(|| "the turn ended with an error".to_string());
            self.config.extensions.on_attention_required(&reason).await;
        }
        let status = match outcome.status {
            TurnStatus::Ok => "ok",
            TurnStatus::Error => "error",
        };
        self.config.extensions.on_post_turn_end(status).await;
        outcome
    }

    fn fail(&self, reason: StopReason, message: String) -> TurnOutcome {
        TurnOutcome {
            status: TurnStatus::Error,
            stop_reason: reason,
            usage: Default::default(),
            error: Some(message),
        }
    }

    /// Append an extension lifecycle record and surface it (FR-EXT-3's
    /// report half; the headless `extension-event` envelope).
    fn record_extension_event(
        &mut self,
        extension: &str,
        event: &str,
        detail: &str,
        sink: &mut dyn TurnSink,
    ) -> Result<(), String> {
        self.store
            .append(
                self.session,
                Record::ExtensionEvent {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    extension: extension.to_string(),
                    event: event.to_string(),
                    detail: detail.to_string(),
                },
            )
            .map_err(|err| format!("cannot write to the session log: {err}"))?;
        sink.on_event(TurnEvent::ExtensionEvent {
            extension: extension.to_string(),
            event: event.to_string(),
            detail: detail.to_string(),
        });
        Ok(())
    }
}
