//! Extension host tests: the FR-EXT behaviors the host itself owns, run
//! against the committed conformance fixture (`tool` world slice).

use std::sync::{Arc, Mutex};

use lca_ext_host::{CallError, ExtHost, ExtensionLimits, HostEnvironment, LoadError, Manifest};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{ToolCall, ToolResultStatus};

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

/// A fresh environment per test: isolated scope roots and grant store.
fn env(tag: &str) -> Arc<HostEnvironment> {
    let root = std::env::temp_dir().join(format!("lca-host-{tag}-{}", std::process::id()));
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
    })
}

fn limits() -> ExtensionLimits {
    ExtensionLimits {
        memory_bytes: 64 * 1024 * 1024,
        fuel_per_call: 10_000_000,
        log_limit_bytes: 4096,
    }
}

fn call(mode: &str) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: format!(r#"{{"mode":"{mode}"}}"#),
    }
}

// Verifies: FR-EXT-1 (an extension implementing a published world loads)
// and FR-EXT-2 (instantiation happens during load, before any input is
// accepted: nothing can call the component until `load` has linked and
// instantiated it).
#[test]
fn loads_a_tool_world_component_and_calls_it() {
    let mut host = ExtHost::new(limits(), env("h1"));
    let extension = host.load(fixture(), manifest()).expect("loads");
    let schema = extension.schema().expect("schema");
    assert_eq!(schema.name, "conformance");
    assert!(
        matches!(schema.parameters, serde_json::Value::Object(_)),
        "schema carries a JSON object"
    );

    let result = extension.execute(&call("ok")).expect("execute");
    assert_eq!(result.status, ToolResultStatus::Ok);
    assert!(result.content.contains("conformance ok"));
    assert!(extension.is_enabled());
}

// Verifies: FR-EXT-3 (a trap disables that extension for the session,
// reports the failure, and the host continues).
#[test]
fn a_trapping_call_disables_the_extension_and_the_host_survives() {
    let mut host = ExtHost::new(limits(), env("h2"));
    let extension = host.load(fixture(), manifest()).expect("loads");

    let err = extension.execute(&call("trap")).expect_err("traps");
    assert!(matches!(err, CallError::Trap(_)), "got {err:?}");
    assert!(
        !extension.is_enabled(),
        "disabled for the session (FR-EXT-3)"
    );

    // Reporting and continuing: a later call reports Disabled, not a crash,
    // and the host can still load other extensions.
    let err = extension.execute(&call("ok")).expect_err("stays disabled");
    assert!(matches!(err, CallError::Disabled), "got {err:?}");
    let second = host
        .load(fixture(), manifest())
        .expect("host still loads extensions");
    assert!(second.execute(&call("ok")).is_ok());
}

// Verifies: FR-EXT-4 (exceeding the fuel budget cancels the call and
// returns an error to the caller).
#[test]
fn fuel_budget_cancels_a_spinning_call() {
    let mut host = ExtHost::new(
        ExtensionLimits {
            fuel_per_call: 2_000_000,
            ..limits()
        },
        env("h3"),
    );
    let extension = host.load(fixture(), manifest()).expect("loads");
    let started = std::time::Instant::now();
    let err = extension
        .execute(&call("loop"))
        .expect_err("runs out of fuel");
    assert!(matches!(err, CallError::FuelExhausted), "got {err:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "bounded work"
    );
}

// Verifies: FR-CONC-1 (epoch interruption cancels a running WASM call,
// independent of its fuel budget).
#[test]
fn epoch_interrupt_cancels_a_running_call() {
    let mut host = ExtHost::new(
        ExtensionLimits {
            fuel_per_call: u64::MAX,
            ..limits()
        },
        env("h4"),
    );
    let extension = host.load(fixture(), manifest()).expect("loads");

    let engine = host.engine().clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        engine.increment_epoch();
    });
    let started = std::time::Instant::now();
    let err = extension.execute(&call("loop")).expect_err("interrupted");
    stopper.join().expect("stopper");
    assert!(matches!(err, CallError::Cancelled), "got {err:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "epoch interruption is prompt (NFR-29's shape)"
    );
}

// Verifies: FR-EXT-5 (exceeding the memory limit stops the instance and
// disables the extension).
#[test]
fn memory_limit_stops_a_hungry_instance() {
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 8 * 1024 * 1024,
            ..limits()
        },
        env("h5"),
    );
    let extension = host.load(fixture(), manifest()).expect("loads");
    let err = extension
        .execute(&call("alloc"))
        .expect_err("hits the ceiling");
    assert!(
        matches!(err, CallError::MemoryLimit | CallError::Trap(_)),
        "got {err:?}"
    );
    assert!(
        !extension.is_enabled(),
        "disabled for the session (FR-EXT-5)"
    );
}

// Verifies: FR-EXT-10 (log messages above the configured limit truncate
// before reaching diagnostic output).
#[test]
fn long_log_messages_truncate_at_the_configured_limit() {
    let mut host = ExtHost::new(
        ExtensionLimits {
            log_limit_bytes: 1000,
            ..limits()
        },
        env("h6"),
    );
    let extension = host.load(fixture(), manifest()).expect("loads");
    extension.execute(&call("log")).expect("logs and succeeds");
    let logs = extension.captured_logs();
    assert!(!logs.is_empty(), "the log import recorded something");
    for message in &logs {
        assert!(
            message.len() <= 1000,
            "truncated to the limit, got {}",
            message.len()
        );
    }
    assert!(
        logs[0].contains("truncated"),
        "the truncation is visible: {:?}",
        &logs[0][logs[0].len().saturating_sub(40)..]
    );
}

// Verifies: FR-EXT-8 (an ABI version outside the supported window fails
// that load with a typed error while the host keeps loading everything
// else, so the session continues).
#[test]
fn abi_outside_the_window_fails_that_load_only() {
    let mut host = ExtHost::new(limits(), env("h7"));
    let unsupported = r#"
name = "too-new"
version = "1.0.0"
abi = "9.9"
worlds = ["tool"]
"#;
    let err = match host.load(fixture(), unsupported) {
        Err(err) => err,
        Ok(_) => panic!("expected the load to fail"),
    };
    match &err {
        LoadError::AbiUnsupported { declared, .. } => assert_eq!(declared, "9.9"),
        other => panic!("expected AbiUnsupported, got {other:?}"),
    }
    // The session continues: the next load still works.
    assert!(host.load(fixture(), manifest()).is_ok());
}

// The manifest is the consent surface; identity fields must parse and the
// name must match the schema's identifier rules (FR-PERM-1's shape check;
// the full schema validation lands with distribution in Phase 5).
#[test]
fn manifest_parses_identity_fields() {
    let manifest: Manifest = Manifest::parse(manifest()).expect("parses");
    assert_eq!(manifest.name, "conformance");
    assert_eq!(manifest.abi, "0.1");
    assert_eq!(
        manifest.worlds,
        vec![
            "tool".to_string(),
            "command".to_string(),
            "hooks".to_string(),
            "provider".to_string(),
            "compaction".to_string(),
            "context-transform".to_string(),
            "ui".to_string()
        ]
    );
    assert!(
        Manifest::parse(
            "name = \"Bad Name\"\nversion = \"1.0.0\"\nabi = \"0.1\"\nworlds = [\"tool\"]"
        )
        .is_err(),
        "identifier rules enforced"
    );
}
