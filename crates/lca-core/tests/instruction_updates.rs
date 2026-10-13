//! Mid-session instruction updates (gh #217, OpenCode's
//! `renderUpdate` shape): a rule-file change appends a unified diff at
//! the turn boundary; `messages[0]` stays byte-identical so the
//! provider KV cache survives.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_core::instructions::{InstructionBaseline, check_instructions};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

fn baseline() -> Arc<Mutex<InstructionBaseline>> {
    Arc::new(Mutex::new(InstructionBaseline::default()))
}

// Verifies: gh #217 (the first scan adopts silently; a change
// produces a unified diff naming the file).
#[tokio::test]
async fn changes_produce_unified_diffs() {
    let dir = scratch("instr-diff");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let rules = dir.join("AGENTS.md");
    // Long enough that the hunk beats the full text (OpenCode's
    // compactness rule picks the full text for tiny files).
    let body: String = (1..=40).map(|n| format!("rule line {n}\n")).collect();
    std::fs::write(&rules, &body).expect("write");
    let base = baseline();
    let adopted = check_instructions(std::slice::from_ref(&rules), &base).await;
    assert!(adopted.is_empty(), "startup adoption is silent");
    std::fs::write(&rules, format!("{body}Do not modify schema.rs directly.\n")).expect("edit");
    let deltas = check_instructions(std::slice::from_ref(&rules), &base).await;
    assert_eq!(deltas.len(), 1, "one changed file, one delta");
    let text = format!("{:?}", deltas[0]);
    assert!(text.contains("AGENTS.md"), "{text}");
    assert!(text.contains("@@"), "{text}");
    assert!(
        text.contains("+Do not modify schema.rs directly."),
        "{text}"
    );
    let quiet = check_instructions(std::slice::from_ref(&rules), &base).await;
    assert!(quiet.is_empty(), "the baseline moved; no repeat");
}

// Verifies: gh #217 (unchanged files inject nothing; a deleted file
// reports once that its instructions no longer apply).
#[tokio::test]
async fn unchanged_is_silent_and_removal_reports_once() {
    let dir = scratch("instr-remove");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let rules = dir.join("AGENTS.md");
    std::fs::write(&rules, "stay\n").expect("write");
    let base = baseline();
    assert!(
        check_instructions(std::slice::from_ref(&rules), &base)
            .await
            .is_empty()
    );
    assert!(
        check_instructions(std::slice::from_ref(&rules), &base)
            .await
            .is_empty()
    );
    std::fs::remove_file(&rules).expect("remove");
    let deltas = check_instructions(std::slice::from_ref(&rules), &base).await;
    assert_eq!(deltas.len(), 1);
    assert!(
        format!("{:?}", deltas[0]).contains("no longer apply"),
        "{:?}",
        deltas[0]
    );
    assert!(
        check_instructions(std::slice::from_ref(&rules), &base)
            .await
            .is_empty()
    );
    let _ = HashMap::<String, String>::new();
}

// Agent-level guards below: a recording provider captures each turn's
// request so the cache-honesty claim is asserted on real assemblies.

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnSink, TurnStatus};
use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff};
use lca_protocol::{
    CommandEffect, CommandSpec, CompletionRequest, DispatchError, EventSink, HookAction,
    PostToolObservation, StreamEvent, ToolCall, ToolResult, ToolSpec, Usage,
};

struct Allow;
impl PermissionPrompt for Allow {
    fn ask(&mut self, _: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

#[derive(Default)]
struct Sink;
impl TurnSink for Sink {
    fn on_event(&mut self, _: lca_core::TurnEvent) {}
}

/// One recorded request: every message's debug text, plus the stable prefix.
/// A provider double that records each request's messages and answers
/// with one text line (no tools, so turns end after one round).
type SeenRequest = (Vec<String>, usize);
struct Recorder {
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl ExtensionDispatch for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }
    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }
    fn worlds(&self) -> Vec<World> {
        vec![World::Provider]
    }
    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Err(DispatchError::Failed(format!(
            "no tools: {}",
            call.name
        )))))
    }
    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }
    fn invoke_command(&self, name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Err(DispatchError::Failed(format!("no commands: {name}")))
    }
    fn interrupt(&self) {}
    fn stream_completion<'a>(
        &'a self,
        request: CompletionRequest,
        sink: &'a dyn EventSink,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        self.seen.lock().unwrap().push((
            request
                .messages
                .iter()
                .map(|message| format!("{message:?}"))
                .collect(),
            request.stable_prefix,
        ));
        sink.push(StreamEvent::TextDelta {
            delta: "done".to_string(),
        });
        sink.push(StreamEvent::Usage {
            usage: Usage::default(),
        });
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }
    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> DispatchFuture<'a, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

fn harness(name: &str) -> (PathBuf, PathBuf, Arc<Recorder>, Arc<ExtensionRegistry>) {
    let root = scratch(name);
    let project = root.join("project");
    for part in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(part)).expect("mkdir");
    }
    let double: Arc<Recorder> = Arc::new(Recorder {
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::clone(&double) as Arc<dyn ExtensionDispatch>);
    (root, project, double, Arc::new(registry))
}

// Verifies: gh #217 end to end - a mid-session rule edit appends a
// diff to the next turn while messages[0] stays byte-identical and
// the stable prefix never collapses.
#[tokio::test]
async fn rule_edits_append_diffs_without_touching_the_prefix() {
    let (root, project, double, registry) = harness("instr-e2e");
    let rules = project.join("AGENTS.md");
    let body: String = (1..=40).map(|n| format!("rule line {n}\n")).collect();
    std::fs::write(&rules, &body).expect("write");
    let baseline = Arc::new(Mutex::new(InstructionBaseline::default()));
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let one_config = || AgentConfig {
        provider: "recorder".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        extensions: registry.clone(),
        system_prompt: "You are LCA, a coding agent.".to_string(),
        instruction_sources: vec![rules.clone()],
        instruction_baseline: baseline.clone(),
        ..AgentConfig::default()
    };
    let mut tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    let mut prompt = Allow;
    let backend = lca_core::ExtensionProvider::new(double.clone() as Arc<dyn ExtensionDispatch>);
    let mut agent = Agent::new(
        &store,
        &session,
        &backend,
        &mut tools,
        grants.clone(),
        &mut prompt,
        None,
        one_config(),
    );
    let cancel = lca_tools::CancelFlag::new();
    let mut sink = Sink;
    let first = agent.run_turn("first", &mut sink, &cancel).await;
    assert_eq!(first.status, TurnStatus::Ok, "{first:?}");
    std::fs::write(
        &rules,
        format!(
            "{body}Do not modify schema.rs directly.
"
        ),
    )
    .expect("edit");
    let mut tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    let mut prompt = Allow;
    let backend = lca_core::ExtensionProvider::new(double.clone() as Arc<dyn ExtensionDispatch>);
    let mut agent = Agent::new(
        &store,
        &session,
        &backend,
        &mut tools,
        grants.clone(),
        &mut prompt,
        None,
        one_config(),
    );
    let cancel = lca_tools::CancelFlag::new();
    let mut sink = Sink;
    let second = agent.run_turn("second", &mut sink, &cancel).await;
    assert_eq!(second.status, TurnStatus::Ok, "{second:?}");
    let seen = double.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one request per turn");
    assert_eq!(seen[0].0[0], seen[1].0[0], "messages[0] is byte-identical");
    assert!(
        !seen[0]
            .0
            .iter()
            .any(|message| message.contains("AGENTS.md")),
        "turn one is clean"
    );
    let last = seen[1].0.last().expect("a delta");
    assert!(last.contains("AGENTS.md"), "{last}");
    assert!(last.contains("@@"), "{last}");
    assert!(
        last.contains("+Do not modify schema.rs directly."),
        "{last}"
    );
    assert!(
        seen[1].1 >= seen[0].1,
        "the boundary advances, never collapses"
    );
    let _ = backend;
}
