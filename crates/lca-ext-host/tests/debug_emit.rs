//! Sandbox emission (gh #258 §3E): guest exports and host imports
//! emit under the feature — names and timing, never arguments,
//! results, or secret values. Feature-gated.

#![cfg(feature = "debug-trace")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use std::sync::{Arc, Mutex};

use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::ToolCall;

fn fixture() -> &'static [u8] {
    include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm")
}

fn manifest() -> &'static str {
    include_str!("../../../extensions/conformance/extension.toml")
}

struct AllowPrompt;

impl PermissionPrompt for AllowPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn env(tag: &str) -> Arc<HostEnvironment> {
    let root = lca_testkit::scratch_path(&format!("lca-emit-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    Arc::new(HostEnvironment {
        roots: ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(AllowPrompt)),
        grant_store: Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grant store"),
        )),
        project: root.join("workspace"),
        proposals: None,
        dialogs: lca_permissions::SharedDialogs::default(),
    })
}

fn limits() -> ExtensionLimits {
    ExtensionLimits {
        memory_bytes: 64 * 1024 * 1024,
        fuel_per_call: 10_000_000,
        log_limit_bytes: 4096,
    }
}

// Verifies: gh #258 - a guest tool call emits its export crossing
// (name, timing, status) to the wire file, without arguments.
#[test]
fn guest_tool_calls_emit_their_crossing() {
    let root = lca_testkit::scratch_path("debug-wasm-emit");
    let paths = lca_debug::DebugPaths::in_dir(&root);
    let subscriber_config = lca_debug::DebugConfig {
        paths: paths.clone(),
        filter: Some("lca_wasm=trace".to_string()),
    };
    // Global (not scoped): the host runs guests on worker threads,
    // which a thread-local subscriber never sees. This target is its
    // own process, so the one-shot global init is safe.
    lca_debug::init(&subscriber_config).expect("init");
    {
        let mut host = ExtHost::new(limits(), env("x1"));
        let extension = host.load(fixture(), manifest()).expect("loads");
        extension
            .execute(&ToolCall {
                call_id: "c1".to_string(),
                name: "conformance".to_string(),
                arguments: r#"{"mode":"ok","secret":"hunter2"}"#.to_string(),
                parent_call_id: None,
            })
            .expect("execute");
    }
    let wire = std::fs::read_to_string(&paths.wire).unwrap_or_default();
    for event in ["export.tools.execute", "export.tools.done"] {
        assert!(wire.contains(event), "exports emit {event}:\n{wire}");
    }
    assert!(wire.contains("name=conformance"), "the name lands:\n{wire}");
    assert!(!wire.contains("hunter2"), "arguments never emit");
}
