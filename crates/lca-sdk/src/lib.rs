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

/// One embedding session: handle, stream, and input in one value.
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
    /// Standing grants; held for a whole turn, which also serializes
    /// turns - one session, one conversation, in order.
    grants: tokio::sync::Mutex<GrantStore>,
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

/// The prompt policy until a host-facing approval surface exists:
/// deny what would ask a human, exactly like headless mode.
struct DenyAll;

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
            .create_session(&cwd, "session")
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
            grants: tokio::sync::Mutex::new(grants),
            events,
        })
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
        let mut grants = self.grants.lock().await;
        let mut tools = ToolExecutor::new(
            Arc::new(NativeOps),
            self.cwd.clone(),
            self.cwd.clone(),
            TOOL_RESULT_LIMIT,
            Duration::from_secs(TOOL_TIMEOUT_SECS),
        );
        let mut prompt = DenyAll;
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
            &mut grants,
            &mut prompt,
            None,
            config,
        );
        agent.run_turn(input, &mut sink, &cancel).await
    }
}
