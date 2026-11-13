//! The dispatch doubles the `hooks_*` rows drive: a hook-policy extension, a
//! prompt spy, the harness that wires them, and the cancellation breakdown the
//! quarantined row prints when it fires.
//!
//! Gh #548 phase 4 moved them here when `hooks.rs` split by subject: the two
//! sibling targets both need them, and a sibling target cannot import another
//! target's items, so they are shared rather than copied.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
#![allow(dead_code)] // a shared helper module: each test crate uses a subset.

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnOutcome};
use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{
    CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation, ToolCall, ToolSpec,
};
use lca_testkit::FakeProvider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use super::CollectingSink;

pub const CONFORMANCE_WASM: &[u8] =
    include_bytes!("../../../../extensions/conformance/fixtures/tool-world.wasm");
pub const CONFORMANCE_MANIFEST: &str =
    include_str!("../../../../extensions/conformance/extension.toml");

pub fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(name);
    for part in ["project", "workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

/// Records every prompt and answers with a fixed decision: the FR-CORE-10
/// assertion is that certain calls never reach it.
pub struct PromptSpy {
    asked: Mutex<Vec<String>>,
    answer: Decision,
}

impl Default for PromptSpy {
    fn default() -> PromptSpy {
        PromptSpy {
            asked: Mutex::new(Vec::new()),
            answer: Decision::Denied,
        }
    }
}

impl PromptSpy {
    pub fn answering(answer: Decision) -> PromptSpy {
        PromptSpy {
            asked: Mutex::new(Vec::new()),
            answer,
        }
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("spy").clone()
    }
}

impl PermissionPrompt for PromptSpy {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.asked.lock().expect("spy").push(action.display());
        self.answer
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// A policy-only extension: no worlds beyond hooks.
pub struct PolicyExt {
    name: &'static str,
    deny_prefix: Option<&'static str>,
    /// Replace the arguments of `shell` calls with this command.
    replace_shell_with: Option<String>,
    calls: Mutex<usize>,
    pre_turn_calls: Mutex<usize>,
    post_tool_calls: Mutex<usize>,
    post_turn_calls: Mutex<usize>,
    attention_calls: Mutex<usize>,
    close_calls: Mutex<usize>,
}

impl PolicyExt {
    pub fn deny(name: &'static str, prefix: &'static str) -> PolicyExt {
        PolicyExt {
            name,
            deny_prefix: Some(prefix),
            replace_shell_with: None,
            calls: Mutex::new(0),
            pre_turn_calls: Mutex::new(0),
            post_tool_calls: Mutex::new(0),
            post_turn_calls: Mutex::new(0),
            attention_calls: Mutex::new(0),
            close_calls: Mutex::new(0),
        }
    }

    pub fn replacing(name: &'static str, command: &str) -> PolicyExt {
        PolicyExt {
            name,
            deny_prefix: None,
            replace_shell_with: Some(command.to_string()),
            calls: Mutex::new(0),
            pre_turn_calls: Mutex::new(0),
            post_tool_calls: Mutex::new(0),
            post_turn_calls: Mutex::new(0),
            attention_calls: Mutex::new(0),
            close_calls: Mutex::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().expect("calls")
    }

    pub fn pre_turn_calls(&self) -> usize {
        *self.pre_turn_calls.lock().expect("pre-turn")
    }

    pub fn post_tool_calls(&self) -> usize {
        *self.post_tool_calls.lock().expect("post-tool")
    }

    pub fn post_turn_calls(&self) -> usize {
        *self.post_turn_calls.lock().expect("post-turn")
    }

    pub fn attention_calls(&self) -> usize {
        *self.attention_calls.lock().expect("attention")
    }

    pub fn close_calls(&self) -> usize {
        *self.close_calls.lock().expect("close")
    }
}

impl ExtensionDispatch for PolicyExt {
    fn name(&self) -> &str {
        self.name
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Hooks]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.to_string(),
            world: "tool",
        })
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
            extension: self.name.to_string(),
            world: "tool",
        })))
    }

    fn on_pre_turn(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        *self.pre_turn_calls.lock().expect("pre-turn") += 1;
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        *self.post_tool_calls.lock().expect("post-tool") += 1;
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        *self.post_turn_calls.lock().expect("post-turn") += 1;
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
        *self.attention_calls.lock().expect("attention") += 1;
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(&self) -> lca_ext_abi::DispatchFuture<'static, Result<(), DispatchError>> {
        *self.close_calls.lock().expect("close") += 1;
        Box::pin(std::future::ready(Ok(())))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.to_string(),
            world: "command",
        })
    }
    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Err(DispatchError::MissingWorld {
            extension: self.name.to_string(),
            world: "command",
        })
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<HookAction, DispatchError>> {
        *self.calls.lock().expect("calls") += 1;
        let action = if let Some(prefix) = self.deny_prefix {
            if call.name.starts_with(prefix) {
                HookAction::Deny(format!("{}: `{}` denied by pattern", self.name, call.name))
            } else {
                HookAction::Allow
            }
        } else if let Some(command) = &self.replace_shell_with
            && call.name == "shell"
        {
            let mut replacement = call.clone();
            replacement.arguments = serde_json::json!({ "command": command }).to_string();
            HookAction::Replace(replacement)
        } else {
            HookAction::Allow
        };
        Box::pin(std::future::ready(Ok(action)))
    }
}

pub struct Harness {
    pub store: lca_session::SessionStore,
    pub session: lca_session::Session,
    pub grants: Arc<Mutex<GrantStore>>,
    pub tools: lca_tools::ToolExecutor,
    pub provider: Arc<FakeProvider>,
    pub prompt: Arc<PromptSpy>,
    pub config: AgentConfig,
}

pub fn harness(name: &str, provider: FakeProvider, registry: ExtensionRegistry) -> Harness {
    let root = scratch(name);
    let project = root.join("project");
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    let mut config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        ..AgentConfig::default()
    };
    config.extensions = Arc::new(registry);
    Harness {
        store,
        session,
        grants,
        tools,
        provider: Arc::new(provider),
        prompt: Arc::new(PromptSpy::default()),
        config,
    }
}

pub async fn turn(h: &mut Harness, input: &str, sink: &mut CollectingSink) -> TurnOutcome {
    let mut prompt = PromptSpy::default();
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        h.grants.clone(),
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = agent
        .run_turn(input, sink, &lca_tools::CancelFlag::new())
        .await;
    let answered = prompt.asked.into_inner().expect("unlocked");
    h.prompt.asked.lock().expect("spy").extend(answered);
    outcome
}

/// The elapsed breakdown the quarantine guard prints: event counts by
/// shape, provider calls served, and records durable. On a hang it names
/// the phase that never answered (provider silence, tool-call stall, or
/// record write); on green it is the one-line baseline the next red is
/// read against. Shapes and orderings only - no timing thresholds.
pub fn cancel_breakdown(
    h: &Harness,
    sink: &CollectingSink,
    elapsed: std::time::Duration,
) -> String {
    let (mut text, mut scheduled, mut started, mut finished, mut errors, mut other) =
        (0, 0, 0, 0, 0, 0);
    for event in &sink.events {
        match event {
            TurnEvent::TextDelta(_) => text += 1,
            TurnEvent::RetryScheduled { .. } => scheduled += 1,
            TurnEvent::ToolStarted(_) => started += 1,
            TurnEvent::ToolFinished(_) => finished += 1,
            TurnEvent::Error { .. } => errors += 1,
            _ => other += 1,
        }
    }
    let records = h
        .store
        .read(&h.session)
        .map(|read| read.records.len())
        .unwrap_or(0);
    format!(
        "turn2_elapsed={elapsed:?} provider_calls={} sink_text={text} sink_retries={scheduled} sink_tool_started={started} sink_tool_finished={finished} sink_errors={errors} sink_other={other} store_records={records}",
        h.provider.call_count(),
    )
}
