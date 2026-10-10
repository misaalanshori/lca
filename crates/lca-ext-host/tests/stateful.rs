//! #103 (QA-017): stateful WASM guests through an epoch-safe instance
//! cache. Fresh-instances-per-call (the Phase 2 trap-isolation rule) made
//! every guest stateless by construction; the recorded upgrade path keeps
//! one cached instance per extension with its epoch deadline re-armed per
//! call, and evicts it on any trap, fuel exhaustion, or cancellation.
//!
//! ADR-0014's trap isolation must survive the change: per-guest epoch
//! accounting, poisoned-guest eviction, and cross-guest independence are
//! pinned here alongside the new statefulness itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch as _;
use lca_ext_host::{CallError, ExtHost, ExtensionLimits, HostEnvironment};
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
    let root = lca_testkit::scratch_path(&format!("lca-stateful-{tag}"));
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

fn call(mode: &str) -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: format!(r#"{{"mode":"{mode}"}}"#),
        parent_call_id: None,
    }
}

/// The guest's `call-count` answer reads `call N`; pull N out.
fn call_number(content: &str) -> u64 {
    content
        .strip_prefix("call ")
        .unwrap_or_else(|| panic!("not a call count: {content}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("not a call count: {content}"))
}

// Verifies: #103 (QA-017) - a WASM extension holds guest-side state
// across two calls: the second `call-count` answers exactly one more
// than the first, with no disk persistence in between.
#[test]
fn a_guest_holds_state_across_two_calls() {
    let mut host = ExtHost::new(limits(), env("holds"));
    let extension = host.load(fixture(), manifest()).expect("loads");
    let first = extension.execute(&call("call-count")).expect("first");
    assert_eq!(first.status, ToolResultStatus::Ok);
    let second = extension.execute(&call("call-count")).expect("second");
    assert_eq!(second.status, ToolResultStatus::Ok);
    assert_eq!(
        call_number(&second.content),
        call_number(&first.content) + 1,
        "the guest counted up across the two calls"
    );
}

// Verifies: #103 eviction with FR-EXT-4 - a fuel-exhausted guest is
// dropped, not resumed: the extension stays enabled, an ordinary call
// still works, and the counter restarts from 1 on the fresh instance
// rather than continuing mid-loop state.
#[test]
fn a_fuel_exhausted_guest_comes_back_fresh_and_stays_enabled() {
    let tiny = ExtensionLimits {
        fuel_per_call: 2_000_000,
        ..limits()
    };
    let mut host = ExtHost::new(tiny, env("evict"));
    let extension = host.load(fixture(), manifest()).expect("loads");
    let err = extension
        .execute(&call("loop"))
        .expect_err("runs out of fuel");
    assert!(
        matches!(err, CallError::FuelExhausted),
        "a spin exhausts fuel, got {err:?}"
    );
    assert!(extension.is_enabled(), "fuel exhaustion never disables");
    let ok = extension.execute(&call("ok")).expect("ordinary call works");
    assert_eq!(ok.status, ToolResultStatus::Ok);
    let counted = extension.execute(&call("call-count")).expect("counts");
    assert_eq!(
        call_number(&counted.content),
        1,
        "the exhausted instance was evicted, not resumed"
    );
}

// Verifies: #103 with FR-EXT-3 - one guest's trap never touches another:
// the trapping extension disables itself while its neighbor keeps
// serving, each counting only its own calls.
#[test]
fn one_guests_trap_never_touches_another_guest() {
    let mut host = ExtHost::new(limits(), env("isolate"));
    let doomed = host.load(fixture(), manifest()).expect("loads");
    let neighbor = host.load(fixture(), manifest()).expect("loads");
    let before = neighbor.execute(&call("call-count")).expect("counts");
    let err = doomed.execute(&call("trap")).expect_err("traps");
    assert!(matches!(err, CallError::Trap(_)), "got {err:?}");
    assert!(!doomed.is_enabled(), "the trap disables its own guest");
    assert!(neighbor.is_enabled(), "the neighbor stays enabled");
    let after = neighbor.execute(&call("call-count")).expect("counts");
    assert_eq!(
        call_number(&after.content),
        call_number(&before.content) + 1,
        "the neighbor's guest kept its own state"
    );
}

// Verifies: #103 per-guest epoch accounting (ADR-0014) - a global epoch
// bump aimed at one guest does not corrupt an idle guest's cached
// instance: the neighbor serves on, counting from where it left off.
#[test]
fn an_epoch_bump_for_one_guest_leaves_an_idle_guest_usable() {
    let mut host = ExtHost::new(limits(), env("epoch"));
    let first = host.load(fixture(), manifest()).expect("loads");
    let second = host.load(fixture(), manifest()).expect("loads");
    let before = second.execute(&call("call-count")).expect("counts");
    first.interrupt();
    let ok = second.execute(&call("ok")).expect("still serves");
    assert_eq!(ok.status, ToolResultStatus::Ok);
    let after = second.execute(&call("call-count")).expect("counts");
    assert_eq!(
        call_number(&after.content),
        call_number(&before.content) + 1,
        "the idle guest survived the other guest's epoch bump"
    );
}
