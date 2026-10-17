//! GitHub #287: two defects in `[limits]` parsing.
//!
//! 1. `memory_bytes` was assigned the raw `memory_mb` number, so a manifest
//!    asking for 64 MiB got a **64-byte** limiter. Wasmtime traps on the
//!    guest's first 64 KiB page, so the extension never runs at all:
//!    `Trap("memory minimum size of 17 pages exceeds memory limits")`.
//!    The clamp order was wrong too — `.min(MAX_MEMORY_BYTES)` ran on the
//!    un-multiplied value, so a 1000-MiB ask clamped to 1000 *bytes*.
//! 2. `manifest_limits` was called inside the `[capabilities]` branch, so a
//!    `[limits]`-declaring extension with no capabilities table silently ran
//!    on the host's defaults.
//!
//! Verifies: through the host's public load/execute API — both the applied
//! `ExtensionLimits` (the unit) and a real instantiation + call (the
//! behavior). Before the fix the load/execute row traps.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment, Manifest};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::{ToolCall, ToolResultStatus};

const FIXTURE: &[u8] = include_bytes!("../../extensions/conformance/fixtures/tool-world.wasm");

/// A host ceiling generous enough that any trap can only come from the
/// manifest's own parsed limits.
fn host_limits() -> ExtensionLimits {
    ExtensionLimits {
        memory_bytes: 256 * 1024 * 1024,
        fuel_per_call: 100_000_000,
        log_limit_bytes: 4096,
    }
}

struct DenyPrompt;
impl PermissionPrompt for DenyPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("gh287-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("workspace")).expect("mkdir");
        Sandbox { root }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        let roots = ScopeRoots {
            workspace: self.root.join("workspace"),
            private: self.root.join("private"),
            home_config: self.root.join("config"),
            temp: self.root.join("tmp"),
            state_dir: self.root.join("data"),
        };
        Arc::new(HostEnvironment {
            roots,
            prompt: Arc::new(Mutex::new(DenyPrompt)),
            grant_store: Arc::new(Mutex::new(
                GrantStore::open(&self.root.join("grants.json")).expect("grant store"),
            )),
            project: self.root.join("workspace"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        })
    }
}

/// A manifest declaring `[limits]` with the given `memory_mb`, plus a
/// capability table (so the pre-fix parse path actually ran) when asked.
fn manifest(memory_mb: u32, with_capabilities: bool) -> String {
    let caps = if with_capabilities {
        "\n[capabilities.completion]\nreason = \"Summarizes older parts of the conversation.\"\n"
    } else {
        ""
    };
    format!(
        "name = \"conformance\"\nversion = \"0.1.0\"\nabi = \"0.5\"\nworlds = [\"tool\"]\n{caps}\n[limits]\nmemory_mb = {memory_mb}\nfuel_per_call = 100000000\n"
    )
}

fn call() -> ToolCall {
    ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: r#"{"mode":"fs-read","scope":"workspace","path":"notes.txt"}"#.to_string(),
        parent_call_id: None,
    }
}

// Verifies: gh #287 defect 1 - the parsed unit is mebibytes, on the path
// that always parsed (capabilities present). Before: 64 bytes.
#[test]
fn memory_mb_parses_as_mebibytes() {
    let parsed = Manifest::parse(&manifest(64, true)).expect("parses");
    assert_eq!(
        parsed.limits.expect("limits").memory_bytes,
        64 * 1024 * 1024,
        "64 MiB is 67,108,864 bytes, not 64"
    );
}

// Verifies: gh #287 defect 2 - and it is applied even with no
// `[capabilities]` table. Before: `limits` was `None`, so the host's
// defaults silently won.
#[test]
fn limits_apply_without_capabilities() {
    let parsed = Manifest::parse(&manifest(128, false)).expect("parses");
    assert_eq!(
        parsed
            .limits
            .expect("limits parsed without capabilities")
            .memory_bytes,
        128 * 1024 * 1024
    );
}

// Verifies: gh #287 AC3 - the user-visible mechanism, end to end through
// the host: a component whose manifest asks for 64 MiB instantiates and
// answers a call. Before: `Trap("memory minimum size of 17 pages exceeds
// memory limits")` and the extension never runs.
#[test]
fn a_limits_manifest_instantiates_and_runs() {
    let sandbox = Sandbox::new("instantiate");
    let mut host = ExtHost::new(host_limits(), sandbox.env());
    let extension = host
        .load(FIXTURE, &manifest(64, true))
        .expect("a `[limits]` manifest loads");
    let result = extension
        .execute(&call())
        .expect("the guest runs inside its memory budget instead of trapping on page one");
    // Denied for lack of the capability, which still proves it executed: a
    // memory trap would have failed the load/execute, not returned a result.
    assert_eq!(result.status, ToolResultStatus::Error, "{}", result.content);
    assert!(
        result.content.contains("does not declare"),
        "the guest answered: {}",
        result.content
    );
}

// Verifies: gh #287 AC1 - the clamp order: multiply first, clamp after. A
// 1000-MiB ask must clamp to 512 MiB in BYTES; the old order clamped it to
// 1000 *bytes*.
#[test]
fn an_above_range_ask_clamps_to_the_host_maximum_in_bytes() {
    let parsed = Manifest::parse(&manifest(1000, true)).expect("parses");
    assert_eq!(
        parsed.limits.expect("limits").memory_bytes,
        512 * 1024 * 1024,
        "the clamp applies to the byte value, not the MiB number"
    );
}
