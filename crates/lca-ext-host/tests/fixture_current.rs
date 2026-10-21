//! gh #427: every committed fixture under `extensions/conformance/fixtures/`
//! must link against the current ABI 0.6 host and the manifest that names it.
//!
//! The committed fixture exists so tests run offline without a wasm toolchain
//! (docs/phase-log.md, Phase 2) — but a committed binary has no way to notice
//! that the source moved on. On 2026-10-08 commit `2ccf187` added the eight
//! opt-in hook worlds to `wit/` and the conformance source, without rebuilding
//! the fixtures; the commit before it (`d24192b`, 2026-10-07) was the last to
//! touch `fixtures/component.wasm`. The result: installing that stale file
//! failed with
//!
//! ```text
//! skipping `conformance`: cannot link extension: no exported instance named `lca:ext/hook-message-end@0.6.0`
//! ```
//!
//! — a link error that looks like a host bug to anyone following the
//! authoring guide.
//!
//! This row is the guard against the same drift reappearing. It walks the
//! fixtures directory at **run time** (not `include_bytes!`, so a newly added
//! stale file cannot hide) and loads each one through `ExtHost::load`. That is
//! precisely the journey that failed in #427, because `load` builds a typed
//! pre for **every world the manifest declares** (`hooks_pre` in
//! `lib.rs`): a fixture missing any declared world's exports fails here with
//! `LoadError::Link`, before any call. A fixture that loads therefore
//! satisfies every declared world — the acceptance condition, by construction.
//!
//! Verifies: NFR-24 (gh #427), gh #427 acceptance 1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};

const MANIFEST: &str = include_str!("../../../extensions/conformance/extension.toml");

struct AllowPrompt;
impl lca_permissions::PermissionPrompt for AllowPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

fn env(tag: &str) -> Arc<HostEnvironment> {
    let root = lca_testkit::scratch_path(&format!("lca-fixture-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    Arc::new(HostEnvironment {
        roots: lca_permissions::ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        prompt: Arc::new(Mutex::new(AllowPrompt)),
        grant_store: Arc::new(Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grant store"),
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

/// The committed fixture directory (runtime, so the listing tracks the tree).
fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/conformance/fixtures")
}

// Verifies: gh #427 - every committed conformance fixture links on the current
// host and satisfies every world the manifest declares. The pre-fix state
// fails here with the issue's exact error (`no exported instance named
// 'lca:ext/hook-message-end@0.6.0'`).
#[test]
fn every_committed_conformance_fixture_links_against_the_declared_worlds() {
    let dir = fixtures_dir();
    let mut fixtures: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("the fixtures directory must exist ({dir:?}): {err}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    fixtures.sort();
    assert!(
        !fixtures.is_empty(),
        "no fixtures left in {dir:?}: if the offline-fixture value is being retired on purpose, this row and the phase-log note go with it"
    );

    for path in fixtures {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes =
            std::fs::read(&path).unwrap_or_else(|err| panic!("read the fixture {name}: {err}"));
        let mut host = ExtHost::new(limits(), env(&name.replace('.', "-")));
        let loaded = host.load(&bytes, MANIFEST).unwrap_or_else(|err| {
            panic!(
                "committed fixture {name} no longer links against the current ABI 0.6 host \
                 and the manifest that declares its worlds (gh #427): {err}"
            )
        });
        assert_eq!(
            loaded.name(),
            "conformance",
            "the fixture links as the manifest names it: {name}"
        );
        assert!(loaded.is_enabled(), "and it comes up enabled: {name}");
    }
}
