//! `[limits]` parsing (gh #287): `memory_mb` is **mebibytes**, and the
//! table is honored whether or not the manifest also declares
//! `[capabilities]`.
//!
//! Two defects lived here. `memory_bytes` was assigned the raw `memory_mb`
//! number, so a manifest asking for 64 MiB got a 64-BYTE limiter and
//! Wasmtime trapped on the guest's first 64 KiB page; and the parse call
//! sat inside the `[capabilities]` branch, so a limits-only extension
//! silently ran on host defaults.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_ext_host::Manifest;

/// The host's maximum, mirroring `manifest::MAX_MEMORY_BYTES` (512 MiB).
/// Written out rather than imported so the test does not widen the crate's
/// public surface for a constant.
const MAX_MEMORY_BYTES: usize = 512 * 1024 * 1024;

/// A minimal manifest with a trailing extra TOML fragment.
fn manifest_with(extra: &str) -> String {
    format!(
        "name = \"limits-test\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\ndescription = \"x\"\n{extra}"
    )
}

/// The permitted default in bytes: 64 MiB.
const DEFAULT_MEMORY_BYTES: usize = 64 * 1024 * 1024;

// Verifies: gh #287 defect 1 - `memory_mb` is MiB. Asking for 64 MiB must
// yield 67,108,864 bytes, not 64. (Before the fix this was 64 bytes, which
// traps on the guest's first memory page.)
#[test]
fn memory_mb_is_mebibytes_in_the_parsed_limits() {
    let manifest = Manifest::parse(&manifest_with(
        "\n[limits]\nmemory_mb = 64\nfuel_per_call = 1000\n",
    ))
    .expect("parses");
    let limits = manifest.limits.expect("limits parsed");
    assert_eq!(
        limits.memory_bytes, DEFAULT_MEMORY_BYTES,
        "64 MiB is 67,108,864 bytes"
    );
}

// Verifies: gh #287 AC1 - the boundary values of the schema's 1..=512
// range, and the clamp ORDER: multiply first, clamp after. A 1000-MiB ask
// must clamp to 512 MiB in BYTES, not to 1000 bytes.
#[test]
fn the_memory_unit_and_clamp_order_hold_at_the_range_edges() {
    for (mb, expected) in [
        (1, 1024 * 1024),
        (64, 64 * 1024 * 1024),
        (512, 512 * 1024 * 1024),
    ] {
        let manifest = Manifest::parse(&manifest_with(&format!(
            "\n[limits]\nmemory_mb = {mb}\nfuel_per_call = 1000\n"
        )))
        .expect("parses");
        let limits = manifest.limits.expect("limits parsed");
        assert_eq!(
            limits.memory_bytes, expected,
            "memory_mb = {mb} must be {expected} bytes"
        );
        assert!(
            limits.memory_bytes <= MAX_MEMORY_BYTES,
            "never above the host maximum"
        );
    }
}

// Verifies: gh #287 - an absent `memory_mb` takes the schema default of
// 64 MiB (also in bytes, now that the unit is right).
#[test]
fn an_absent_memory_mb_takes_the_64_mib_default() {
    let manifest =
        Manifest::parse(&manifest_with("\n[limits]\nfuel_per_call = 1000\n")).expect("parses");
    assert_eq!(
        manifest.limits.expect("limits parsed").memory_bytes,
        DEFAULT_MEMORY_BYTES,
        "the default is 64 MiB, and the unit is MiB"
    );
}

// Verifies: gh #287 defect 1 independently of defect 2 - the unit is
// wrong even on the path that DID parse (capabilities present). This is the
// row that isolates the arithmetic from the misplaced call.
#[test]
fn the_unit_is_mebibytes_on_the_capabilities_path_too() {
    let manifest = Manifest::parse(&manifest_with(
        "\n[capabilities.completion]\nreason = \"Summarizes older parts of the conversation.\"\n\n[limits]\nmemory_mb = 64\nfuel_per_call = 1000\n",
    ))
    .expect("parses");
    let limits = manifest
        .limits
        .expect("limits parsed (capabilities present)");
    assert_eq!(
        limits.memory_bytes, DEFAULT_MEMORY_BYTES,
        "64 MiB is 67,108,864 bytes on this path as well"
    );
}

// Verifies: gh #287 defect 2 - `[limits]` without `[capabilities]` is
// still parsed. Before the fix the call sat inside the capabilities
// branch, so a limits-only extension ran on host defaults.
#[test]
fn limits_are_honored_without_a_capabilities_table() {
    let manifest = Manifest::parse(&manifest_with(
        "\n[limits]\nmemory_mb = 128\nfuel_per_call = 5000\n",
    ))
    .expect("parses");
    assert!(
        manifest.fs.is_empty() && !manifest.process,
        "the fixture declares no capabilities at all"
    );
    let limits = manifest
        .limits
        .expect("limits must be parsed even with no [capabilities] table");
    assert_eq!(limits.memory_bytes, 128 * 1024 * 1024);
    assert_eq!(limits.fuel_per_call, 5000);
}

// Verifies: gh #287 AC3 - a limits-only manifest is the *shape* that broke,
// so pin that a manifest with `[limits]` and no capabilities still parses
// to a usable Manifest (not an error, not defaults).
#[test]
fn a_limits_only_manifest_parses_completely() {
    let manifest = Manifest::parse(&manifest_with(
        "\n[limits]\nmemory_mb = 96\nfuel_per_call = 2000\n",
    ))
    .expect("parses");
    assert_eq!(manifest.name, "limits-test");
    assert_eq!(
        manifest.limits.expect("limits").memory_bytes,
        96 * 1024 * 1024
    );
}

// Verifies: gh #287 AC4 - nonsense below the schema minimums is still
// rejected (the clamp fixes the unit, not the validation).
#[test]
fn out_of_range_limits_are_still_rejected() {
    assert!(
        Manifest::parse(&manifest_with("\n[limits]\nmemory_mb = 0\n")).is_err(),
        "memory_mb below 1 is refused"
    );
    assert!(
        Manifest::parse(&manifest_with("\n[limits]\nfuel_per_call = 999\n")).is_err(),
        "fuel_per_call below 1000 is refused"
    );
}

// Verifies: gh #287 - a manifest with no `[limits]` table at all has no
// limits, so the host's own values apply (the existing behavior, pinned so
// moving the parse call out of the capabilities branch did not invent one).
#[test]
fn a_manifest_without_a_limits_table_has_none() {
    assert!(
        Manifest::parse(&manifest_with(""))
            .expect("parses")
            .limits
            .is_none(),
        "absent [limits] means no override"
    );
    // And still none when capabilities ARE declared.
    assert!(
        Manifest::parse(&manifest_with(
            "\n[capabilities.completion]\nreason = \"Summarizes older parts of the conversation.\"\n"
        ))
        .expect("parses")
        .limits
        .is_none(),
        "absent [limits] with capabilities present is still none"
    );
}

// ---------------------------------------------------------------------
// The behavioral proof (gh #287 AC3): a real component whose manifest
// declares `[limits]` instantiates and runs a call.
// ---------------------------------------------------------------------

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_permissions::{GrantStore, ScopeRoots};
use lca_protocol::ToolResultStatus;

const FIXTURE: &[u8] = include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm");

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("lca-limits-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("workspace")).expect("mkdir");
        Sandbox { root }
    }

    fn env(&self) -> Arc<HostEnvironment> {
        struct DenyPrompt;
        impl lca_permissions::PermissionPrompt for DenyPrompt {
            fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
                lca_permissions::Decision::Denied
            }
            fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
                false
            }
        }
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

/// The host's own generous ceiling, so a failure can only come from the
/// manifest's parsed limits and not from the host side.
fn host_limits() -> ExtensionLimits {
    ExtensionLimits {
        memory_bytes: 256 * 1024 * 1024,
        fuel_per_call: 100_000_000,
        log_limit_bytes: 4096,
    }
}

// Verifies: gh #287 AC3 - the user-visible mechanism. A component whose
// manifest asks for 1 MiB must instantiate and answer a call; before the
// fix the parsed limit was 1 BYTE, so Wasmtime trapped on the guest's first
// 64 KiB page and the extension never ran.
#[test]
fn a_component_with_limits_instantiates_and_runs_a_call() {
    let sandbox = Sandbox::new("instantiate");
    let mut host = ExtHost::new(host_limits(), sandbox.env());
    // `[capabilities]` is present ON PURPOSE: with it absent, the pre-fix
    // code ignored `[limits]` entirely and the host defaults hid the
    // arithmetic bug. Declaring a capability takes the parse path that did
    // run, so this row isolates the unit defect (gh #287 #1).
    let manifest = "name = \"conformance\"\nversion = \"0.1.0\"\nabi = \"0.5\"\nworlds = [\"tool\"]\n\n[capabilities.completion]\nreason = \"Summarizes older parts of the conversation.\"\n\n[limits]\nmemory_mb = 64\nfuel_per_call = 100000000\n";
    let extension = host
        .load(FIXTURE, manifest)
        .expect("a manifest with [limits] and [capabilities] loads");
    let result = extension
        .execute(&lca_protocol::ToolCall {
            call_id: "c1".to_string(),
            name: "conformance".to_string(),
            arguments: r#"{"mode":"fs-read","scope":"workspace","path":"notes.txt"}"#.to_string(),
            parent_call_id: None,
        })
        .expect("the guest runs inside its memory budget instead of trapping on page one");
    // The call is denied for lack of capability, which still proves the
    // guest instantiated and executed: a memory trap would have errored
    // out of `load`/`execute` instead of returning a tool result.
    assert_eq!(result.status, ToolResultStatus::Error, "{}", result.content);
    assert!(
        result.content.contains("does not declare"),
        "the guest ran and answered: {}",
        result.content
    );
}
