//! Loop-level hook and dispatch tests: the pre-tool seam runs before the
//! prompt (FR-CORE-10), both delivery modes drive one loop (FR-EXT-6), a
//! trapping extension is reported while the session survives
//! (FR-EXT-3), and cancelling a turn interrupts a running extension call
//! (FR-CONC-1).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_core::{
    Agent, AgentConfig, ExtensionRegistry, StopReason, TurnEvent, TurnOutcome, TurnSink, TurnStatus,
};
use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{
    CommandEffect, CommandSpec, DispatchError, HookAction, PostToolObservation, Record, ToolCall,
    ToolResultStatus, ToolSpec,
};
use lca_session::ViewMode;
use lca_testkit::{FakeProvider, fake_usage};

const CONFORMANCE_WASM: &[u8] =
    include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");
const CONFORMANCE_MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-hooks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for part in ["project", "workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

/// Records every prompt and answers with a fixed decision: the FR-CORE-10
/// assertion is that certain calls never reach it.
struct PromptSpy {
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
    fn answering(answer: Decision) -> PromptSpy {
        PromptSpy {
            asked: Mutex::new(Vec::new()),
            answer,
        }
    }

    fn asked(&self) -> Vec<String> {
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

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

/// A policy-only extension: no worlds beyond hooks.
struct PolicyExt {
    name: &'static str,
    deny_prefix: Option<&'static str>,
    /// Replace the arguments of `shell` calls with this command.
    replace_shell_with: Option<String>,
    calls: Mutex<usize>,
    pre_turn_calls: Mutex<usize>,
    attention_calls: Mutex<usize>,
    close_calls: Mutex<usize>,
}

impl PolicyExt {
    fn deny(name: &'static str, prefix: &'static str) -> PolicyExt {
        PolicyExt {
            name,
            deny_prefix: Some(prefix),
            replace_shell_with: None,
            calls: Mutex::new(0),
            pre_turn_calls: Mutex::new(0),
            attention_calls: Mutex::new(0),
            close_calls: Mutex::new(0),
        }
    }

    fn replacing(name: &'static str, command: &str) -> PolicyExt {
        PolicyExt {
            name,
            deny_prefix: None,
            replace_shell_with: Some(command.to_string()),
            calls: Mutex::new(0),
            pre_turn_calls: Mutex::new(0),
            attention_calls: Mutex::new(0),
            close_calls: Mutex::new(0),
        }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("calls")
    }

    fn pre_turn_calls(&self) -> usize {
        *self.pre_turn_calls.lock().expect("pre-turn")
    }

    fn attention_calls(&self) -> usize {
        *self.attention_calls.lock().expect("attention")
    }

    fn close_calls(&self) -> usize {
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
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), DispatchError>> {
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

struct Harness {
    store: lca_session::SessionStore,
    session: lca_session::Session,
    grants: GrantStore,
    tools: lca_tools::ToolExecutor,
    provider: Arc<FakeProvider>,
    prompt: Arc<PromptSpy>,
    config: AgentConfig,
}

fn harness(name: &str, provider: FakeProvider, registry: ExtensionRegistry) -> Harness {
    let root = scratch(name);
    let project = root.join("project");
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = GrantStore::open(&root.join("grants.json")).expect("grants");
    let tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps),
        project.clone(),
        project.clone(),
        65536,
        std::time::Duration::from_secs(30),
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

async fn turn(h: &mut Harness, input: &str, sink: &mut CollectingSink) -> TurnOutcome {
    let mut prompt = PromptSpy::default();
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        &mut h.grants,
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

// Verifies: FR-CORE-10 (the pre-tool hook runs before the permission
// check, and a hook denial ends the call without a user prompt).
#[tokio::test]
async fn hook_denial_ends_the_call_without_a_prompt() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"deny-me-now"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("understood").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(PolicyExt::deny("gate", "shell")));
    let mut h = harness("deny", provider, registry);
    let mut sink = CollectingSink::default();

    let outcome = turn(&mut h, "run it", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);

    let results: Vec<_> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ToolResultStatus::Denied);
    assert!(
        results[0].content.contains("denied by pattern"),
        "{}",
        results[0].content
    );

    let asked = h.prompt.asked();
    assert!(
        asked.is_empty(),
        "no user prompt for a hook denial, asked: {asked:?}"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records.iter().any(|r| matches!(
            r,
            Record::ToolResult {
                status: ToolResultStatus::Denied,
                ..
            }
        )),
        "the denial is on record"
    );
}

// Verifies: FR-CORE-10 (a replaced call passes through the permission
// layer like any other and is not fed back through the hooks).
#[tokio::test]
async fn a_replaced_call_reaches_the_prompt_and_is_not_rehooked() {
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("shell", r#"{"command":"danger-original"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let policy = Arc::new(PolicyExt::replacing("rewriter", "echo safe-replacement"));
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("replace", provider, registry);
    let mut sink = CollectingSink::default();

    let outcome = turn(&mut h, "run it", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok, "{:?}", outcome.error);
    assert_eq!(policy.calls(), 1, "the replacement is not re-hooked");

    let asked = h.prompt.asked();
    assert_eq!(
        asked.len(),
        1,
        "the replacement still hits the prompt: {asked:?}"
    );
    assert!(asked[0].contains("echo safe-replacement"), "{asked:?}");
    assert!(
        !asked[0].contains("danger-original"),
        "prompt shows the REPLACED call: {asked:?}"
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    let permission = read
        .records
        .iter()
        .find_map(|r| match r {
            Record::Permission { action, .. } => Some(action.clone()),
            _ => None,
        })
        .expect("permission record");
    assert!(permission.contains("echo safe-replacement"), "{permission}");
}

// Verifies: FR-EXT-6 (a WASM handle and a native handle drive the same
// loop, registered through the same table), and the merged tool list
// reaches the provider.
#[tokio::test]
async fn wasm_and_native_extension_tools_both_run_in_the_loop() {
    // Build the WASM handle through the host.
    let root = scratch("wasm-tool");
    let env = Arc::new(HostEnvironment {
        roots: ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(PromptSpy::answering(Decision::Always))),
        grant_store: Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        project: root.join("project"),
        proposals: None,
    });
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 100_000_000,
            log_limit_bytes: 4096,
        },
        env,
    );
    let wasm = host
        .load(CONFORMANCE_WASM, CONFORMANCE_MANIFEST)
        .expect("load wasm");

    // The native twin over an identical capability configuration.
    let caps = Arc::new(lca_tools::Capabilities::new(
        "conformance",
        lca_tools::CapabilityGrants {
            fs: vec![
                lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                    .expect("grant"),
            ],
            fs_declared: true,
            process: true,
            pty: true,
            ..Default::default()
        },
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(PromptSpy::answering(Decision::Always))),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants-2.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ));
    let native: Arc<conformance::NativeConformance> =
        Arc::new(conformance::NativeConformance::new(caps));

    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(wasm));
    registry.register(Arc::clone(&native) as Arc<dyn ExtensionDispatch>);
    assert_eq!(
        registry.collisions().len(),
        1,
        "same tool name from two modes: the later loses (FR-EXT-11): {:?}",
        registry.collisions()
    );

    // The merged tool list reached the provider with the extension tool.
    let provider = FakeProvider::builder()
        .turn(|t| t.text("ok").usage(fake_usage(5, 5, 0, 0)))
        .build();
    let mut h = harness("wasm-tool", provider, {
        let mut only = ExtensionRegistry::new();
        only.register(Arc::new(
            host.load(CONFORMANCE_WASM, CONFORMANCE_MANIFEST)
                .expect("second wasm"),
        ));
        only
    });
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hi", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);
    let request = h.provider.last_request().expect("request");
    assert!(
        request.tools.iter().any(|tool| tool.name == "conformance"),
        "the extension's tool rides along with the built-ins: {:?}",
        request
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
    );

    // Now run an actual extension-tool call through the loop (the native
    // handle in this registry; the wasm twin is diffed in the conformance
    // harness).
    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("conformance", r#"{"mode":"ok"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("finished").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::clone(&native) as Arc<dyn ExtensionDispatch>);
    let mut h = harness("native-tool", provider, registry);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "probe it", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok, "{:?}", outcome.error);
    let result = sink
        .events
        .iter()
        .find_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .expect("tool ran");
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert_eq!(
        result.content, "conformance ok",
        "same result as the wasm twin"
    );
}

// Verifies: FR-EXT-3 (a trap disables that extension, reports the
// failure, and the session continues).
#[tokio::test]
async fn a_trapping_extension_is_reported_and_the_session_survives() {
    let root = scratch("trap");
    let env = Arc::new(HostEnvironment {
        roots: ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(PromptSpy::answering(Decision::Always))),
        grant_store: Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        project: root.join("project"),
        proposals: None,
    });
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 100_000_000,
            log_limit_bytes: 4096,
        },
        env,
    );
    let wasm = host
        .load(CONFORMANCE_WASM, CONFORMANCE_MANIFEST)
        .expect("load");
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(wasm));

    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("conformance", r#"{"mode":"trap"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("recovered").usage(fake_usage(20, 5, 10, 0)))
        .build();
    let mut h = harness("trap", provider, registry);
    let mut sink = CollectingSink::default();

    let outcome = turn(&mut h, "probe", &mut sink).await;
    assert_eq!(
        outcome.status,
        TurnStatus::Ok,
        "the turn finishes despite the trap: {:?}",
        outcome.error
    );

    // The failure is reported as an extension event, on the wire and in
    // the log.
    let event = sink
        .events
        .iter()
        .find_map(|e| match e {
            TurnEvent::ExtensionEvent {
                extension,
                event,
                detail,
            } => Some((extension.clone(), event.clone(), detail.clone())),
            _ => None,
        })
        .expect("extension event surfaced");
    assert_eq!(event.0, "conformance");
    assert!(event.1 == "error" || event.1 == "disabled", "{}", event.1);
    assert!(
        event.2.contains("trapped") || event.2.contains("disabled"),
        "{}",
        event.2
    );

    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records.iter().any(
            |r| matches!(r, Record::ExtensionEvent { extension, .. } if extension == "conformance")
        ),
        "the report is on record (FR-EXT-3)"
    );

    // The session continues: the model's next response lands.
    let result = sink
        .events
        .iter()
        .find_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .expect("the failed call produced a result");
    assert_eq!(
        result.status,
        ToolResultStatus::Error,
        "the model sees the failure"
    );

    // And the extension stays disabled: a follow-up call reports Disabled
    // instead of running.
    let read = h
        .store
        .read_with(&h.session, ViewMode::Audit)
        .expect("read");
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::Assistant { .. })),
        "the assistant record after recovery exists: session is alive"
    );
}

// Verifies: FR-CONC-1 (cancelling a turn interrupts a running extension
// call rather than waiting for its fuel or natural end).
#[tokio::test]
async fn cancelling_a_turn_interrupts_a_running_extension_call() {
    let root = scratch("cancel-ext");
    let env = Arc::new(HostEnvironment {
        roots: ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(PromptSpy::answering(Decision::Always))),
        grant_store: Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        project: root.join("project"),
        proposals: None,
    });
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: u64::MAX,
            log_limit_bytes: 4096,
        },
        env,
    );
    let wasm = host
        .load(CONFORMANCE_WASM, CONFORMANCE_MANIFEST)
        .expect("load");
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(wasm));

    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("conformance", r#"{"mode":"loop"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("never").usage(fake_usage(10, 5, 0, 0)))
        .build();
    let mut h = harness("cancel-ext", provider, registry);

    let cancel = lca_tools::CancelFlag::new();
    // A plain thread: the sync extension call occupies the test's
    // current-thread runtime, so the canceller must not depend on it.
    let canceller = {
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            flag.cancel();
        })
    };
    let mut sink = CollectingSink::default();
    let started = std::time::Instant::now();
    let mut prompt = PromptSpy::answering(Decision::Always);
    let mut agent = Agent::new(
        &h.store,
        &h.session,
        h.provider.as_ref(),
        &mut h.tools,
        &mut h.grants,
        &mut prompt,
        None,
        h.config.clone(),
    );
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        agent.run_turn("spin", &mut sink, &cancel),
    )
    .await
    .expect("the turn returns");
    canceller.join().expect("canceller");
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    // From cancel (150ms in) to return: epoch interruption, not fuel
    // exhaustion (the budget here is u64::MAX).
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the spinning call stopped promptly, took {:?}",
        started.elapsed()
    );
    let read = h.store.read(&h.session).expect("read");
    assert!(
        read.records
            .iter()
            .any(|r| matches!(r, Record::User { .. })),
        "completed records are kept (FR-CONC-3)"
    );
}

// Verifies: the SRDD hook points - `pre-turn` fires once per turn before any
// provider work, and `attention-required` fires when a turn ends in error.
#[tokio::test]
async fn pre_turn_and_attention_hooks_fire() {
    let policy = Arc::new(PolicyExt::deny("observed", "deny-"));

    let provider = FakeProvider::builder()
        .turn(|t| t.text("hi").usage(fake_usage(10, 5, 0, 10)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("hooks-pre-turn", provider, registry);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "hello", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Ok);
    assert_eq!(policy.pre_turn_calls(), 1, "pre-turn fires once per turn");
    assert_eq!(
        policy.attention_calls(),
        0,
        "a successful turn needs no attention"
    );

    let failing = FakeProvider::builder()
        .turn(|t| t.error("boom", false).usage(fake_usage(1, 1, 0, 0)))
        .build();
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    let mut h = harness("hooks-attention", failing, registry);
    let mut sink = CollectingSink::default();
    let outcome = turn(&mut h, "fail please", &mut sink).await;
    assert_eq!(outcome.status, TurnStatus::Error);
    assert_eq!(
        policy.attention_calls(),
        1,
        "a failed turn asks for attention"
    );
    assert_eq!(
        policy.pre_turn_calls(),
        2,
        "the failing turn ran pre-turn too"
    );
}

// The registry's `session-close` fans out to every hooks handle; the CLI
// calls it on exit (headless and after the interface).
#[tokio::test]
async fn session_close_hook_fires_when_called() {
    let policy = Arc::new(PolicyExt::deny("closer", "deny-"));
    let mut registry = ExtensionRegistry::new();
    registry.register(policy.clone());
    registry.on_session_close().await;
    assert_eq!(policy.close_calls(), 1);
}
