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
    /// The project the session belongs to (the tools' workspace).
    cwd: PathBuf,
    /// Where the append-only log and the grant store live.
    store: Arc<SessionStore>,
    /// The session handle (its id is the host's resume key).
    session: lca_session::Session,
    /// The provider this session talks through.
    provider: Arc<dyn lca_provider::Provider>,
    /// The session's model (empty resolves to the provider's first).
    model: String,
    /// Standing grants, shared with every capability engine; the store is
    /// locked per authorize call, never for a whole turn.
    grants: Arc<std::sync::Mutex<GrantStore>>,
    /// The host's approval callback, when one is registered
    /// ([`Session::with_permission_prompt`]).
    approval: Option<PermissionCallback>,
    /// Serializes turns: one session, one conversation, in order.
    turn: tokio::sync::Mutex<()>,
    /// The event stream's fan-out point.
    events: tokio::sync::broadcast::Sender<TurnEvent>,
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
        let (events, _first_subscriber) = tokio::sync::broadcast::channel(EVENT_CAPACITY);
        Ok(Session {
            cwd,
            store,
            session,
            provider,
            model,
            grants: Arc::new(std::sync::Mutex::new(grants)),
            approval: None,
            turn: tokio::sync::Mutex::new(()),
            events,
        })
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

    /// Send one input through the agent loop: events reach every
    /// subscriber while the turn runs, records land in the log before
    /// this returns, and the outcome answers like headless mode's.
    pub async fn send(&self, input: &str) -> TurnOutcome {
        let _turn = self.turn.lock().await;
        let mut tools = ToolExecutor::new(
            Arc::new(NativeOps::default()),
            self.cwd.clone(),
            self.cwd.clone(),
            TOOL_RESULT_LIMIT,
            Duration::from_secs(TOOL_TIMEOUT_SECS),
        );
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
        let cancel = CancelFlag::new();
        let config = AgentConfig {
            provider: self.provider.name().to_string(),
            model: self.model.clone(),
            ..AgentConfig::default()
        };
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
