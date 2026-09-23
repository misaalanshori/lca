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

    fn both_modes(&self) -> (Arc<dyn ExtensionDispatch>, Arc<dyn ExtensionDispatch>) {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            self.env(),
        );
        let wasm = host.load(FIXTURE, MANIFEST).expect("load wasm mode");
        let native = conformance::NativeConformance::new(self.capabilities());
        // The same registry type holds both handle kinds (FR-EXT-6: one
        // handle type, no mode branching).
        let mut registry = NativeRegistry::new();
        registry.register(Arc::new(wasm));
        registry.register(Arc::new(native));
        let handles = registry.into_handles();
        (handles[0].clone(), handles[1].clone())
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

// Verifies: NFR-25's substance and the SRDD Phase 2 exit test — the
// conformance extension passes in native mode and WASM mode with
// identical results.
#[tokio::test]
async fn native_and_wasm_modes_produce_identical_results() {
    let fixture = Fixture::new("diff");
    let (wasm, native) = fixture.both_modes();

    // Identity and shape.
    assert_eq!(wasm.name(), native.name());
    assert_eq!(wasm.worlds(), native.worlds());
    assert_eq!(wasm.worlds().len(), 3, "tool, command, hooks");
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
    let (wasm, native) = fixture.both_modes();

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
