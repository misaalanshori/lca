//! The embedding API for host applications (SRDD's Embedding SDK: a
//! session handle, an event stream, and an input channel - a host
//! creates a session, subscribes to events, and sends input).
//!
//! The native half ships here. The same API's WASM build and the
//! JavaScript module story ride the web target this release deferred
//! (`scripts/deferred-requirements.txt`: NFR-11 with FR-WEB-1/2/3).
//!
//! Hosts hand in a provider directly - instantiate a provider
//! extension with `lca-ext-host` and wrap it in
//! `lca_core::ExtensionProvider`, or pass any `lca_provider::Provider`
//! implementation. The agent loop, the append-only session log, and
//! the built-in tools come from the core. An action that would need a
//! human's approval is denied (the headless rule, `docs/headless.md`);
//! standing grants already written to the data directory's grant store
//! still apply. Turns run one at a time per session: `send` waits for
//! its turn, and the next `send` queues on the same lock.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lca_core::{Agent, AgentConfig, TurnSink};
use lca_permissions::{Decision, GrantStore, PermissionPrompt};
use lca_session::SessionStore;
use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

pub use lca_core::{TurnEvent, TurnOutcome};

/// Events buffered per subscriber before late ones start dropping
/// (a host that stops reading must not stall the turn).
const EVENT_CAPACITY: usize = 256;
/// The built-in tools' result cap and timeout: the documented
/// defaults from `docs/configuration.md`.
const TOOL_RESULT_LIMIT: usize = 65_536;
const TOOL_TIMEOUT_SECS: u64 = 120;

/// The host's approval callback (gh #14): the action verbatim as the
/// TUI modal would show it in, the host's verdict out - once, always,
/// or deny, the same vocabulary the grant store persists.
pub type PermissionCallback = Arc<dyn Fn(String) -> Decision + Send + Sync>;

/// One embedding session: handle, stream, and input in one value.
///
/// Without [`Session::with_permission_prompt`], an action that would
/// need a human's approval is declined and recorded (deny-by-default,
/// NFR-13) - the headless rule, `docs/headless.md`.
pub struct Session {
    /// Where the append-only log and the grant store live.
    store: Arc<SessionStore>,
    /// The session handle (its id is the host's resume key).
    session: lca_session::Session,
    /// The provider this session talks through.
    provider: Arc<dyn lca_provider::Provider>,
    /// Standing grants, shared with every capability engine; the store is
    /// locked per authorize call, never for a whole turn.
    grants: Arc<std::sync::Mutex<GrantStore>>,
    /// The host's approval callback, when one is registered
    /// ([`Session::with_permission_prompt`]).
    approval: Option<PermissionCallback>,
    /// Serializes turns: one session, one conversation, in order.
    turn: tokio::sync::Mutex<()>,
    /// The tool executor, built once (gh #310): its `ReadTracker` and
    /// loaded-instruction dedup are per session, not per turn, so
    /// `edit_requires_read` and "inject a subfolder AGENTS.md once" mean what
    /// they say across a conversation. A per-turn executor wiped both.
    tools: tokio::sync::Mutex<ToolExecutor>,
    /// The agent configuration, owned by the session (gh #310). Its `Arc`
    /// fields - `sent_stable`, `instruction_baseline`, `skills_cache`,
    /// `steer`, `extensions` - are the cross-turn state FR-CACHE-6's
    /// divergence check depends on; rebuilding it per turn reset them all.
    config: std::sync::Mutex<AgentConfig>,
    /// The active turn's cancel flag (gh #309): re-armed at each `send`, so
    /// [`Session::cancel`] from another task cancels the turn that is
    /// actually running.
    cancel: std::sync::Mutex<CancelFlag>,
    /// The event stream's fan-out point.
    events: tokio::sync::broadcast::Sender<TurnEvent>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The provider, the tool executor, and the config are trait objects or
        // holders with no `Debug` of their own; identity and a live-turn flag
        // are what a log line needs.
        f.debug_struct("Session")
            .field("id", &self.session.id())
            .field("provider", &self.provider.name())
            .finish_non_exhaustive()
    }
}

/// Why a session could not start.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The session log or the grant store refused the operation.
    #[error("cannot start the session: {0}")]
    Start(String),
}

/// The prompt policy when no host callback is registered: deny what
/// would ask a human, exactly like headless mode (deny-by-default).
struct DenyAll;

/// The host's approval callback as a [`PermissionPrompt`] (gh #14):
/// the action's display text - verbatim what the TUI modal would show -
/// goes in, the host's verdict comes out, and the grant store and the
/// session log treat the answer exactly like a human's (an `always`
/// persists its pattern; a prompted answer writes its `permission`
/// record either way).
struct CallbackPrompt {
    callback: PermissionCallback,
}

impl PermissionPrompt for CallbackPrompt {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        (self.callback)(action.display())
    }

    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

impl PermissionPrompt for DenyAll {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// Every event the turn produces, copied to every subscriber.
struct Fanout(tokio::sync::broadcast::Sender<TurnEvent>);

impl TurnSink for Fanout {
    fn on_event(&mut self, event: TurnEvent) {
        let _ = self.0.send(event);
    }
}

impl Session {
    /// Create a session on disk under `cwd`, rooted at `data_dir`,
    /// talking through `provider` with `model` (empty selects the
    /// provider's first offer, falling back to the provider's name -
    /// the configured-model rule from `docs/configuration.md`).
    pub fn create(
        cwd: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        provider: Arc<dyn lca_provider::Provider>,
        model: impl Into<String>,
    ) -> Result<Session, Error> {
        let cwd = cwd.into();
        let data = data_dir.into();
        let store = Arc::new(SessionStore::new(data.clone()));
        let session = store
            .create_session(&cwd, lca_session::DEFAULT_TITLE)
            .map_err(|err| Error::Start(err.to_string()))?;
        let grants = GrantStore::open(&data.join("grants.json"))
            .map_err(|err| Error::Start(err.to_string()))?;
        let requested = model.into();
        let model = if requested.is_empty() {
            provider
                .list_models()
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| provider.name().to_string())
        } else {
            requested
        };
        Ok(Session::assemble(
            cwd,
            data,
            store,
            session,
            provider,
            model,
            Arc::new(std::sync::Mutex::new(grants)),
        ))
    }

    /// The shared tail of [`Session::create`] and [`Session::resume`]: build
    /// the session-scoped state that must outlive every turn (gh #309,
    /// #310).
    fn assemble(
        cwd: PathBuf,
        data_dir: PathBuf,
        store: Arc<SessionStore>,
        session: lca_session::Session,
        provider: Arc<dyn lca_provider::Provider>,
        model: String,
        grants: Arc<std::sync::Mutex<GrantStore>>,
    ) -> Session {
        let tools = ToolExecutor::new(
            Arc::new(NativeOps::default()),
            cwd.clone(),
            cwd.clone(),
            TOOL_RESULT_LIMIT,
            Some(Duration::from_secs(TOOL_TIMEOUT_SECS)),
        );
        let config = AgentConfig {
            provider: provider.name().to_string(),
            model: model.clone(),
            // Gh #309: the documented `LCA_*` row - a shell child reads the
            // agent's data home from here. `AgentConfig::default()` leaves it
            // empty, which is what every embedder's shell children saw.
            data_dir: data_dir.clone(),
            ..AgentConfig::default()
        };
        let (events, _first_subscriber) = tokio::sync::broadcast::channel(EVENT_CAPACITY);
        Session {
            store,
            session,
            provider,
            grants,
            approval: None,
            turn: tokio::sync::Mutex::new(()),
            tools: tokio::sync::Mutex::new(tools),
            config: std::sync::Mutex::new(config),
            cancel: std::sync::Mutex::new(CancelFlag::new()),
            events,
        }
    }

    /// Reopen an existing session by id (gh #309) and append to its log.
    ///
    /// `Session::create` always starts a new session; the embeddings that keep
    /// their own handle (`id()` is documented as "resume tools take it") had
    /// no way back in, even though `SessionStore::session` existed.
    pub fn resume(
        cwd: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        session_id: &str,
        provider: Arc<dyn lca_provider::Provider>,
        model: impl Into<String>,
    ) -> Result<Session, Error> {
        let cwd = cwd.into();
        let data = data_dir.into();
        let store = Arc::new(SessionStore::new(data.clone()));
        let session = store
            .session(&cwd, session_id)
            .map_err(|err| Error::Start(err.to_string()))?;
        let grants = GrantStore::open(&data.join("grants.json"))
            .map_err(|err| Error::Start(err.to_string()))?;
        let requested = model.into();
        let model = if requested.is_empty() {
            provider
                .list_models()
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| provider.name().to_string())
        } else {
            requested
        };
        Ok(Session::assemble(
            cwd,
            data,
            store,
            session,
            provider,
            model,
            Arc::new(std::sync::Mutex::new(grants)),
        ))
    }

    /// Register the host's approval callback (gh #14): wherever the
    /// interactive UI would open the permission modal, the host's
    /// `authorize` path calls this instead - the action verbatim as the
    /// modal would show it in, once/always/deny out, persisted and
    /// recorded exactly like a human's answer.
    ///
    /// # Default
    ///
    /// With no callback registered, an action that would need approval
    /// is declined and the denial is recorded (deny-by-default is
    /// load-bearing product law, NFR-13) - the headless rule. Registering
    /// a callback replaces the decline, never the record.
    ///
    /// Yolo-mode equivalence is out of scope: yolo is an agent-level
    /// mode, not an SDK switch.
    pub fn with_permission_prompt(
        mut self,
        callback: impl Fn(String) -> Decision + Send + Sync + 'static,
    ) -> Self {
        self.approval = Some(Arc::new(callback));
        self
    }

    /// The session identifier (its handle: resume tools take it).
    pub fn id(&self) -> &str {
        self.session.id()
    }

    /// Subscribe to this session's event stream: text as it arrives,
    /// tool lifecycle, usage, and the turn's end (FR-CORE-4's shape at
    /// the embedding surface). Subscribe before `send`.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TurnEvent> {
        self.events.subscribe()
    }

    /// Attach an extension registry (gh #309): the dispatch table the turn's
    /// tool calls, hooks, and context transforms run through. Registering
    /// here replaces whatever was set before, so it is meant for setup.
    pub fn with_extensions(self, extensions: Arc<lca_core::ExtensionRegistry>) -> Self {
        let mut config = self.config_guard();
        config.extensions = extensions;
        drop(config);
        self
    }

    /// Tune the agent configuration (gh #309): `edit_requires_read`,
    /// `system_prompt`, `max_iterations`, retry knobs, and the rest.
    ///
    /// The closure receives the live config, so `provider`, `model`, and
    /// `data_dir` stay coherent with the session unless the caller
    /// deliberately changes them.
    pub fn with_agent_config(self, f: impl FnOnce(&mut AgentConfig)) -> Self {
        let mut config = self.config_guard();
        f(&mut config);
        drop(config);
        self
    }

    /// The shared stable-prefix cell (gh #310): the observable seam for
    /// FR-CACHE-6, so an embedder can see what the last turn sent.
    pub fn sent_stable_handle(&self) -> Arc<std::sync::Mutex<Option<Vec<String>>>> {
        self.config_guard().sent_stable.clone()
    }

    /// Whether the session's config holds `registry` (gh #309): the
    /// observable seam for [`Session::with_extensions`], so an embedder can
    /// assert what it attached.
    pub fn shares_extensions(&self, registry: &Arc<lca_core::ExtensionRegistry>) -> bool {
        let config = self.config_guard();
        Arc::ptr_eq(&config.extensions, registry)
    }

    /// The lock over the session's config (poison-tolerant: one poisoned
    /// guard must not take the session down).
    fn config_guard(&self) -> std::sync::MutexGuard<'_, AgentConfig> {
        self.config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The lock over the active turn's cancel flag.
    fn cancel_guard(&self) -> std::sync::MutexGuard<'_, CancelFlag> {
        self.cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Cancel the turn that is running now (gh #309).
    ///
    /// Before this, the `CancelFlag` was a local of `send`: an embedder could
    /// see a turn start and had no way to stop it. The flag is re-armed at
    /// each `send`, so calling this with no turn in flight is a no-op on a
    /// fresh flag rather than a stale one.
    pub fn cancel(&self) {
        self.cancel_guard().cancel();
    }

    /// Send one input, letting the caller own the cancel flag (gh #309): the
    /// alternative to [`Session::cancel`] for hosts that would rather hold the
    /// handle themselves.
    pub async fn send_with_cancel(&self, input: &str, cancel: &CancelFlag) -> TurnOutcome {
        self.run(input, Some(cancel)).await
    }

    /// Send one input through the agent loop: events reach every
    /// subscriber while the turn runs, records land in the log before
    /// this returns, and the outcome answers like headless mode's.
    pub async fn send(&self, input: &str) -> TurnOutcome {
        // Re-arm the session's flag for this turn, so `cancel()` cancels the
        // turn that is actually running (gh #309).
        {
            let mut flag = self.cancel_guard();
            *flag = CancelFlag::new();
        }
        self.run(input, None).await
    }

    /// The shared body of `send` and `send_with_cancel`: `external` is the
    /// caller's flag when one was handed in, else the session's.
    async fn run(&self, input: &str, external: Option<&CancelFlag>) -> TurnOutcome {
        let _turn = self.turn.lock().await;
        // Clone the flag rather than borrow it across the await: `CancelFlag`
        // is an `Arc` pair inside, so the clone shares the cancel state.
        let session_flag = {
            let flag = self.cancel_guard();
            flag.clone()
        };
        let cancel: CancelFlag = external.cloned().unwrap_or(session_flag);
        let mut tools = self.tools.lock().await;
        // Gh #310: the config is cloned per turn, and its `Arc` fields are
        // shared - so `sent_stable`, `instruction_baseline`, `skills_cache`,
        // and `extensions` carry across turns instead of resetting.
        let config = self.config_guard().clone();
        let mut deny_all = DenyAll;
        let mut callback = self
            .approval
            .clone()
            .map(|callback| CallbackPrompt { callback });
        let prompt: &mut dyn PermissionPrompt = match callback.as_mut() {
            Some(prompt) => prompt,
            None => &mut deny_all,
        };
        let mut sink = Fanout(self.events.clone());
        let mut agent = Agent::new(
            &self.store,
            &self.session,
            self.provider.as_ref(),
            &mut tools,
            self.grants.clone(),
            prompt,
            None,
            config,
        );
        agent.run_turn(input, &mut sink, &cancel).await
    }
}
