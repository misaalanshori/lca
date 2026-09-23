//! The Phase 2 exit-test centerpiece: the conformance extension run in
//! both delivery modes produces identical results (NFR-25, the dual-mode
//! interchangeability promise from ADR-0013 and the SRDD).
//!
//! Both sides share the same roots, prompt, and grant store; the WASM
//! side reaches them through host imports, the native side calls the
//! same engine directly. Any divergence is a defect regardless of which
//! side "wins" (docs/testing-plan.md section6).

use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_ext_native::NativeRegistry;
use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::{CommandEffect, HookAction, ToolCall};

const FIXTURE: &[u8] = include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");
const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Fixture {
    root: std::path::PathBuf,
    prompt: Arc<Mutex<Always>>,
    store: Arc<Mutex<GrantStore>>,
    roots: ScopeRoots,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("lca-diff-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        std::fs::write(root.join("project/notes.txt"), "diff content").expect("file");
        let roots = ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        };
        Fixture {
            store: Arc::new(Mutex::new(
                GrantStore::open(&root.join("grants.json")).expect("store"),
            )),
            prompt: Arc::new(Mutex::new(Always)),
            roots,
            root,
        }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        Arc::new(HostEnvironment {
            roots: self.roots.clone(),
            prompt: self.prompt.clone(),
            grant_store: self.store.clone(),
            project: self.root.join("project"),
            proposals: None,
        })
    }

    fn capabilities(&self) -> Arc<lca_tools::Capabilities> {
        Arc::new(lca_tools::Capabilities::new(
            "conformance",
            lca_tools::CapabilityGrants {
                fs: vec![
                    ScopeGrant::parse("workspace", lca_permissions::FsMode::ReadWrite)
                        .expect("grant"),
                ],
                fs_declared: true,
                process: true,
                pty: true,
                ..Default::default()
            },
            self.roots.clone(),
            self.prompt.clone(),
            self.store.clone(),
            self.root.join("project"),
            None,
        ))
    }

    fn both_modes(
        &self,
    ) -> (
        Arc<dyn ExtensionDispatch>,
        Arc<dyn ExtensionDispatch>,
        Arc<lca_tools::Capabilities>,
    ) {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        );
        let wasm = host.load(FIXTURE, MANIFEST).expect("load wasm mode");
        let engine = self.capabilities();
        let native = conformance::NativeConformance::new(engine.clone());
        // The same registry type holds both handle kinds (FR-EXT-6: one
        // handle type, no mode branching).
        let mut registry = NativeRegistry::new();
        registry.register(Arc::new(wasm));
        registry.register(Arc::new(native));
        let handles = registry.into_handles();
        // The SAME engine the native handle ran on, so its recorded
        // denials are visible to the test (FR-PERM-3).
        (handles[0].clone(), handles[1].clone(), engine)
    }
}

fn call(mode_args: &str) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: mode_args.to_string(),
    }
}

fn spawn_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(r#"{{"mode":"spawn","program":"{program}","args":["{marker}"],"cwd":"workspace"}}"#)
    } else {
        format!(
            r#"{{"mode":"spawn","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace"}}"#
        )
    }
}

fn pty_args(program: &str, marker: &str) -> String {
    if cfg!(unix) {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    } else {
        format!(
            r#"{{"mode":"pty","program":"{program}","args":["/C","echo","{marker}"],"cwd":"workspace","rows":24,"cols":80}}"#
        )
    }
}

// Verifies: NFR-25's substance, FR-EXT-7 (the two delivery labels
// differ by design), and the SRDD Phase 2 exit test - the conformance
// extension passes in native mode and WASM mode with identical
// results, plus NFR-21's byte-stable fixture loading under the current
// runtime.
#[tokio::test]
async fn native_and_wasm_modes_produce_identical_results() {
    let fixture = Fixture::new("diff");
    let (wasm, native, _engine) = fixture.both_modes();

    // Identity and shape.
    assert_eq!(wasm.name(), native.name());
    assert_eq!(wasm.worlds(), native.worlds());
    assert_eq!(
        wasm.worlds().len(),
        7,
        "tool, command, hooks, provider, compaction, context-transform, ui"
    );
    assert_eq!(wasm.delivery(), DeliveryMode::Wasm);
    assert_eq!(native.delivery(), DeliveryMode::Native);
    // FR-EXT-7's labels differ BY DESIGN; everything else must not.
    assert_eq!(DeliveryMode::Wasm.label(), "sandboxed");
    assert_eq!(DeliveryMode::Native.label(), "in-process (unsandboxed)");

    // Tool schema.
    let wasm_schema = wasm.tool_specs().expect("wasm schema");
    let native_schema = native.tool_specs().expect("native schema");
    assert_eq!(wasm_schema, native_schema, "identical tool specs");

    // Tool execution: shared modes with exact-equality results.
    let scenarios: Vec<String> = vec![
        r#"{"mode":"ok"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"workspace","path":"notes.txt"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"workspace","path":"../../etc/passwd"}"#.to_string(),
        r#"{"mode":"fs-read","scope":"private","path":"x"}"#.to_string(),
        r#"{"mode":"fs-list","scope":"workspace","path":"."}"#.to_string(),
        spawn_args("echo", "diff-marker"),
        pty_args("echo", "pty-marker"),
    ];
    for scenario in &scenarios {
        let wasm_result = wasm.execute_tool(&call(scenario)).await.expect("wasm");
        let native_result = native.execute_tool(&call(scenario)).await.expect("native");
        assert_eq!(
            wasm_result, native_result,
            "divergence in scenario {scenario}"
        );
    }
    // Sanity: the scenarios actually exercised success and refusal paths.
    let ok = wasm
        .execute_tool(&call(r#"{"mode":"ok"}"#))
        .await
        .expect("ok");
    assert!(ok.content.contains("conformance ok"));
    let refused = wasm
        .execute_tool(&call(
            r#"{"mode":"fs-read","scope":"workspace","path":"../../etc/passwd"}"#,
        ))
        .await
        .expect("runs");
    assert_eq!(refused.status, lca_protocol::ToolResultStatus::Error);
    assert!(
        refused.content.contains("left the scope"),
        "{}",
        refused.content
    );
}

// Commands and hooks: same spec, same effects, same verdicts.
#[tokio::test]
async fn commands_and_hooks_agree_across_modes() {
    let fixture = Fixture::new("diff-hooks");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(
        wasm.command_specs().expect("wasm"),
        native.command_specs().expect("native")
    );
    for argument in ["submit", "insert:hello", "", "unknown"] {
        assert_eq!(
            wasm.invoke_command("probe", argument).expect("wasm"),
            native.invoke_command("probe", argument).expect("native"),
            "command effect divergence for {argument:?}"
        );
    }
    assert_eq!(
        wasm.invoke_command("probe", "submit").expect("effect"),
        CommandEffect::SubmitPrompt("conformance submitted".to_string())
    );

    for tool in ["read", "probe-deny-tool", "shell"] {
        let call = ToolCall {
            call_id: "c9".to_string(),
            name: tool.to_string(),
            arguments: "{}".to_string(),
        };
        let wasm_action = wasm.on_pre_tool_use(&call).await.expect("wasm");
        let native_action = native.on_pre_tool_use(&call).await.expect("native");
        assert_eq!(wasm_action, native_action, "hook divergence for {tool}");
    }
    let denied = wasm
        .on_pre_tool_use(&ToolCall {
            call_id: "c9".to_string(),
            name: "probe-deny-tool".to_string(),
            arguments: "{}".to_string(),
        })
        .await
        .expect("hook");
    assert_eq!(
        denied,
        HookAction::Deny("conformance policy denied this tool".to_string())
    );

    // Observing hooks return Ok on both sides.
    let observation = lca_protocol::PostToolObservation {
        call: ToolCall {
            call_id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
        },
        result: lca_protocol::ToolResult::ok("c1", "fine"),
    };
    wasm.on_post_tool_use(&observation).await.expect("wasm");
    native.on_post_tool_use(&observation).await.expect("native");
    wasm.on_pre_turn().await.expect("wasm");
    native.on_pre_turn().await.expect("native");
    wasm.on_post_turn_end("ok").await.expect("wasm");
    native.on_post_turn_end("ok").await.expect("native");
    wasm.on_session_close().await.expect("wasm");
    native.on_session_close().await.expect("native");

    assert!(wasm.worlds().contains(&World::Hooks));
}

// Verifies: the Phase 4 conformance cases (SRDD: the new worlds and
// the completion capability get conformance coverage alongside) -
// compact, transform, and rejection agree across delivery modes
// (NFR-25), and the undeclared `completion` capability refuses with a
// recorded denial on BOTH sides (FR-PERM-3).
#[tokio::test]
async fn compaction_and_transform_agree_across_modes_with_completion_denied() {
    let fixture = Fixture::new("diff-ctx");
    let (wasm, native, engine) = fixture.both_modes();

    // Mechanical compaction: identical summaries.
    let records: Vec<lca_protocol::Record> = {
        let ts = 1_700_000_000_000u64;
        vec![
            lca_protocol::Record::User {
                v: lca_protocol::FORMAT_VERSION,
                ts,
                id: "01".to_string(),
                content: "first request".to_string(),
                attachments: Vec::new(),
            },
            lca_protocol::Record::Assistant {
                v: lca_protocol::FORMAT_VERSION,
                ts,
                id: "02".to_string(),
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "reply".to_string(),
                }],
                reasoning: None,
                usage: None,
                provider: Some("fake".to_string()),
                model: Some("faux-1".to_string()),
            },
        ]
    };
    let wasm_summary = wasm.compact(&records).await.expect("wasm compact");
    let native_summary = native.compact(&records).await.expect("native compact");
    assert_eq!(wasm_summary, native_summary, "identical summaries");
    assert_eq!(wasm_summary, "conformance compacted 2 records");

    // The completion-carrying candidate: undeclared in this manifest,
    // so both modes surface the refusal (FR-PERM-3's completion case)
    // and the host records it.
    let mut completion_records = records.clone();
    completion_records.push(lca_protocol::Record::User {
        v: lca_protocol::FORMAT_VERSION,
        ts: 1_700_000_000_100u64,
        id: "03".to_string(),
        content: "call-completion".to_string(),
        attachments: Vec::new(),
    });
    let wasm_err = wasm
        .compact(&completion_records)
        .await
        .expect_err("completion is not declared");
    let native_err = native
        .compact(&completion_records)
        .await
        .expect_err("completion is not declared");
    assert!(
        wasm_err.to_string().contains("completion"),
        "wasm: {wasm_err}"
    );
    assert_eq!(format!("{wasm_err}"), format!("{native_err}"));
    assert!(
        engine
            .denials()
            .iter()
            .any(|denial| denial.capability == "completion"),
        "the denial was recorded (FR-PERM-3)"
    );

    // Transform: pass, inject, and reject - identical in both modes.
    let turns = vec![
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "plain turn",
        )],
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "please conformance-inject here",
        )],
        vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-reject this one",
        )],
    ];
    for messages in turns {
        let wasm_out = wasm
            .transform_messages(messages.clone())
            .await
            .expect("wasm transform host call");
        let native_out = native
            .transform_messages(messages.clone())
            .await
            .expect("native transform host call");
        assert_eq!(wasm_out, native_out, "transform divergence");
    }
    // The rejection carries its reason (FR-CTX-3's shape at the ABI).
    let rejected = wasm
        .transform_messages(vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-reject",
        )])
        .await
        .expect("host call");
    assert_eq!(rejected, Err("conformance transform rejection".to_string()));
    // The injection appended one message; nothing else changed.
    let injected = native
        .transform_messages(vec![lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::User,
            "conformance-inject",
        )])
        .await
        .expect("host call")
        .expect("not a rejection");
    assert_eq!(injected.len(), 2, "one appended message");
}

// Verifies: the Phase 6 conformance cases - both modes return the same
// tree for every region (NFR-25 over ADR-0003's arena), the hostile
// span crosses byte for byte so the host is the one that must defang
// it (FR-UI-2), and interaction effects agree (FR-UI-6's vocabulary).
#[tokio::test]
async fn ui_regions_and_effects_agree_across_modes() {
    let fixture = Fixture::new("diff-ui");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(wasm.ui_regions(), native.ui_regions());
    assert_eq!(wasm.ui_regions().len(), 4, "all four regions granted");

    for region in ["status-line", "footer", "panel", "modal"] {
        let wasm_tree = wasm.render(region).expect("wasm render");
        let native_tree = native.render(region).expect("native render");
        assert_eq!(wasm_tree, native_tree, "tree divergence in {region}");
        let tree = wasm_tree.expect("a scripted tree");
        assert!(!tree.is_empty(), "{region} has content");
        assert_eq!(tree.nodes.len(), 1, "the script's single root node");
    }

    // The hostile fixture crossed intact: the HOST must escape it
    // (lca-tui's sanitizer test is the other half of FR-UI-2).
    let footer = wasm.render("footer").expect("footer").expect("tree");
    match &footer.nodes[0] {
        lca_protocol::Widget::Text { content, .. } => {
            assert!(content.contains("\u{1b}[31m"), "{content}")
        }
        other => panic!("a text node, got {other:?}"),
    }

    // Events: the modal branch answers, other regions do nothing.
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        native
            .on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        lca_protocol::UiEffect::CloseModal
    );
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::Submit { text: "hi".into() }
        )
        .expect("event"),
        lca_protocol::UiEffect::ShowNotice("heard: hi".to_string())
    );
    assert_eq!(
        wasm.on_ui_event("panel", &lca_protocol::UiInput::Cancel)
            .expect("event"),
        lca_protocol::UiEffect::None,
        "only the modal region answers"
    );
}
