//! Tool-surface journeys: deferred discovery (#55), annotated
//! prompts (#78), and nested usage (#49) against scripted providers.
//! Split from `loop.rs` for the workspace file ceiling (gate 11).
//! Behaviour unchanged (tests only).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use lca_core::{Agent, TurnEvent};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::Decision;
use lca_protocol::{DispatchError, Record};
use lca_testkit::{FakeProvider, fake_usage};
use std::sync::{Arc, Mutex};

// A suite double serving one deferred tool (gh #77's discovery shape).
struct DeferredSuite;

impl ExtensionDispatch for DeferredSuite {
    fn name(&self) -> &str {
        "deferred-suite"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::ToolCatalog]
    }
    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(vec![lca_protocol::ToolSpec {
            name: "lazy".to_string(),
            description: "a deferred tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Deferred,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }])
    }
    fn execute_tool<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::ToolResult::ok(
            "c", "lazy ran",
        ))))
    }
    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

// Verifies: #55 (PG-003) - the mid-turn journey: request one carries no
// deferred tool but offers `tool_search`; the search activates the hit;
// request two declares it; the log records the change with both lists.
#[tokio::test]
async fn tool_search_declares_deferred_hits_mid_turn() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("tool_search", r#"{"query":"lazy"}"#)
                .usage(fake_usage(10, 5, 0, 5))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("search-declares", provider, default_config());
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(DeferredSuite));
    h.config.extensions = Arc::new(registry);
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
    let outcome = turn(&mut h, "find the lazy tool", &mut sink, &mut prompt).await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2, "search round then answer round");
    let first: Vec<String> = requests[0]
        .tools
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(
        !first.contains(&"lazy".to_string()),
        "deferred stays out of request one: {first:?}"
    );
    assert!(
        first.contains(&"tool_search".to_string()),
        "discovery is offered while undisclosed tools exist: {first:?}"
    );
    let second: Vec<String> = requests[1]
        .tools
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert!(
        second.contains(&"lazy".to_string()),
        "request two declares the hit: {second:?}"
    );
    let read = h.store.read(&h.session).expect("read");
    let change = read.records.iter().find_map(|record| match record {
        Record::Custom {
            custom_type, data, ..
        } if custom_type == "tool-set-change" => Some(data),
        _ => None,
    });
    let data = change.expect("the activation is recorded");
    assert_eq!(
        data.get("discovered")
            .and_then(|value| value.as_array())
            .map(|list| list
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["lazy"]),
        "the record names the discovery: {data}"
    );
}

// A prompt behind the `SharedPrompt` slot: the engines hold the slot, so a
// turn-level row has to prove the hints survive that extra hop too.
struct SlotSpy {
    surfaces: Vec<String>,
}

impl lca_permissions::PermissionPrompt for SlotSpy {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.surfaces.push(format!("plain:{}", action.display()));
        Decision::Always
    }
    fn ask_annotated(
        &mut self,
        action: &lca_permissions::Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        self.surfaces.push(format!(
            "annotated:{}",
            action.display_annotated(annotations)
        ));
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

// A prompt that records the annotated surface it was shown (#78):
// scripted decision, observed surface.
struct HintSpy {
    surfaces: Vec<String>,
}

impl lca_permissions::PermissionPrompt for HintSpy {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.surfaces.push(action.display());
        Decision::Always
    }
    fn ask_annotated(
        &mut self,
        action: &lca_permissions::Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        self.surfaces.push(action.display_annotated(annotations));
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

// Verifies: #78 (PG-026) - the turn carries the called tool's hints to
// the prompt: a shell call's approval surface names its builtin hints,
// and the approved turn still runs.
#[tokio::test]
async fn shell_prompt_shows_builtin_hints() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"echo hi"}"#)
                .usage(fake_usage(10, 5, 0, 5))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("shell-hints", provider, default_config());
    let mut sink = CollectingSink::default();
    let mut prompt = HintSpy {
        surfaces: Vec::new(),
    };
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
        .run_turn("say hi loudly", &mut sink, &lca_tools::CancelFlag::new())
        .await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    assert!(
        prompt
            .surfaces
            .iter()
            .any(|surface| surface.contains("echo hi") && surface.contains("destructive")),
        "the approval surface names the builtin hints: {:?}",
        prompt.surfaces
    );
}

// Verifies: gh #418 Phase C#C5 (gh #303) - the turn-level path with the
// `SharedPrompt` slot in the middle, which is the object every capability
// engine actually holds. The slot's `ask_annotated` used to be the trait
// default (drop the hints, call `ask`), so hints reached the real prompt
// stripped. This row is the one the defect needed: authorize-level rows
// miss the slot entirely.
//
// An in-workspace `write` is silently allowed (gh #259's fast path), so the
// row drives a shell call, whose approval surface is where hints render.
#[tokio::test]
async fn the_shared_prompt_slot_carries_hints_to_the_turn_prompt() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"echo slot-hints"}"#)
                .usage(fake_usage(10, 5, 0, 5))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("slot-hints", provider, default_config());
    let mut sink = CollectingSink::default();
    let spy = Arc::new(Mutex::new(SlotSpy {
        surfaces: Vec::new(),
    }));
    let mut prompt = lca_permissions::SharedPrompt::default();
    prompt.set(spy.clone());
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
        .run_turn("say hi loudly", &mut sink, &lca_tools::CancelFlag::new())
        .await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let surfaces = spy.lock().expect("lock").surfaces.clone();
    assert!(
        surfaces
            .iter()
            .any(|surface| surface.starts_with("annotated:")
                && surface.contains("slot-hints")
                && surface.contains("destructive")),
        "hints survive the slot on the way to the turn prompt: {surfaces:?}"
    );
    assert!(
        !surfaces.iter().any(|surface| surface.starts_with("plain:")),
        "the slot must not collapse to the bare ask: {surfaces:?}"
    );
}

// A scripted completion backend (#49): every call accrues its usage
// into the slot and `take_usage` drains it - the accumulate-and-drain
// contract a turn relies on, without a provider behind it.
struct ScriptedBackend {
    per_call: lca_protocol::Usage,
    slot: Mutex<Option<lca_protocol::Usage>>,
}

impl lca_tools::CompletionBackend for ScriptedBackend {
    fn complete(
        &self,
        _messages: &[lca_protocol::ChatMessage],
    ) -> Result<(String, lca_protocol::Usage), lca_tools::CompletionError> {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let total = slot.get_or_insert_with(lca_protocol::Usage::default);
        let used = &self.per_call;
        total.input += used.input;
        total.output += used.output;
        total.cache_read += used.cache_read;
        total.cache_write += used.cache_write;
        total.cache_write_1h += used.cache_write_1h;
        total.cost += used.cost;
        Ok(("nested summary".to_string(), used.clone()))
    }
    fn take_usage(&self) -> Option<lca_protocol::Usage> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// An extension tool that spends through its capability engine (#49):
/// the turn installs the backend, the tool completes through it, and
/// the turn attributes the spend back to the call and the session.
struct CompletingTool {
    cap: std::sync::Arc<lca_tools::Capabilities>,
}

impl ExtensionDispatch for CompletingTool {
    fn name(&self) -> &str {
        "completing-tool"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Tool]
    }
    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
        Ok(vec![lca_protocol::ToolSpec {
            name: "summarize".to_string(),
            description: "summarize through a nested completion".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: Default::default(),
        }])
    }
    fn execute_tool<'a>(
        &'a self,
        call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
        let cap = self.cap.clone();
        let call_id = call.call_id.clone();
        Box::pin(async move {
            match cap.complete(Vec::new()) {
                Ok((text, _)) => Ok(lca_protocol::ToolResult::ok(call_id, text)),
                Err(err) => Ok(lca_protocol::ToolResult::error(call_id, err.to_string())),
            }
        })
    }
    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, DispatchError> {
        Ok(lca_protocol::CommandEffect::None)
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn set_completion_backend(&self, backend: &Arc<dyn std::any::Any + Send + Sync>) {
        if let Ok(shared) = backend.clone().downcast::<lca_tools::SharedBackend>() {
            self.cap.install_completion_backend(&shared);
        }
    }
}

// Verifies: #49 (EFG-017) - nested completion usage counts toward the
// session cost: the spend lands on the calling tool's result extras
// and joins the turn totals the session stats sum.
#[tokio::test]
async fn nested_usage_counts_toward_session_cost() {
    use lca_permissions::{Decision, PermissionPrompt, ProposalDiff};
    struct Allow;
    impl PermissionPrompt for Allow {
        fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("summarize", r#"{}"#)
                .usage(fake_usage(10, 5, 0, 0))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 0, 0)))
        .build();
    let mut h = harness("nested-usage", provider, default_config());
    let root = h.project.clone();
    let scratch = root.join("caps");
    for dir in ["workspace", "private", "config", "tmp", "data"] {
        std::fs::create_dir_all(scratch.join(dir)).expect("mkdir");
    }
    let cap = std::sync::Arc::new(lca_tools::Capabilities::new(
        "completing-tool",
        lca_tools::CapabilityGrants {
            completion: true,
            ..Default::default()
        },
        lca_permissions::ScopeRoots {
            workspace: scratch.join("workspace"),
            private: scratch.join("private"),
            home_config: scratch.join("config"),
            temp: scratch.join("tmp"),
            state_dir: scratch.join("data"),
        },
        std::sync::Arc::new(Mutex::new(Allow)),
        std::sync::Arc::new(Mutex::new(
            lca_permissions::GrantStore::open(&scratch.join("grants.json")).expect("grants"),
        )),
        scratch.join("workspace"),
        None,
    ));
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(CompletingTool { cap }));
    h.config.extensions = Arc::new(registry);
    let nested = lca_protocol::Usage {
        input: 7,
        output: 3,
        cost: 0.5,
        ..Default::default()
    };
    let backend: Arc<dyn lca_tools::CompletionBackend> = Arc::new(ScriptedBackend {
        per_call: nested.clone(),
        slot: Mutex::new(None),
    });
    let mut sink = CollectingSink::default();
    let mut prompt = Prompt {
        answers: vec![],
        asked: vec![],
    };
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
    agent.set_nested_backend(backend);
    let outcome = agent
        .run_turn("summarize this", &mut sink, &lca_tools::CancelFlag::new())
        .await;
    assert_eq!(outcome.status, lca_core::TurnStatus::Ok);
    let finished = sink.events.iter().find_map(|event| match event {
        TurnEvent::ToolFinished(result) => Some(result),
        _ => None,
    });
    let result = finished.expect("tool finished event");
    let attributed: lca_protocol::Usage = serde_json::from_str(
        result
            .extras
            .get("usage")
            .map(String::as_str)
            .unwrap_or("null"),
    )
    .expect("usage extras parse as Usage");
    assert_eq!(attributed.input, 7, "the call carries its nested spend");
    assert_eq!(
        outcome.usage.input, 37,
        "stream turns (10 + 20) plus nested (7): {:?}",
        outcome.usage
    );
    assert!(
        outcome.usage.cost >= 0.5,
        "nested cost joins the session cost: {:?}",
        outcome.usage
    );
}
