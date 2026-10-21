//! GitHub #427: a committed conformance fixture went stale against ABI 0.6.
//!
//! ## The mechanism, named
//!
//! The conformance package has always shipped its component under
//! `fixtures/` so tests run offline without a wasm toolchain
//! (`docs/phase-log.md`, Phase 2). It shipped **two** names for the same
//! artifact: `tool-world.wasm` (the test suite's, `include_bytes!`-ed by 18
//! test files) and `component.wasm`, added byte-identical in `df3663e`
//! (release 0.5.0) and never referenced by anything, before or since.
//!
//! From `d24192b` (2026-10-07) on, only `tool-world.wasm` got rebuilt when
//! the source moved. Then **`2ccf187` (2026-10-08) added the eight opt-in
//! hook worlds to `wit/` and the conformance source without rebuilding
//! either fixture** — `tool-world.wasm` was repaired later by hand
//! (`5c1c650`, 2026-10-20, "resource/header reads"), `component.wasm` never.
//! No automation ever covered conformance: `scripts/wasm-check.sh` builds
//! but explicitly refreshes nothing, and the publish workflow's "Refresh the
//! committed fixtures" step names five other extensions, not this one.
//!
//! The stale twin then sat where a user following the authoring guide would
//! reach for it, failing to link with:
//!
//! ```text
//! no exported instance named `lca:ext/hook-message-end@0.6.0`
//! ```
//!
//! ## What the guard pins
//!
//! Both halves of that mechanism, at the repo gate:
//!
//! 1. **No orphan twin.** Every committed fixture must have a test consumer
//!    naming it. A fixture nothing loads has no way to be kept current and
//!    no reason to exist — which is exactly how `component.wasm` rotted
//!    unnoticed for two weeks.
//! 2. **Every fixture links.** Each `fixtures/*.wasm` loads through
//!    `ExtHost::load`, which builds a typed pre for every world the manifest
//!    declares — the journey that failed in #427, asserted directly.
//!
//! The byte-level complement (a fresh build must equal the committed bytes)
//! lives in `scripts/wasm-check.sh`, where the component is already being
//! built; that catches source drift this row's link check cannot see when it
//! does not change exports.
//!
//! Verifies: NFR-24 (gh #427), gh #427 acceptance 1 and 2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};

/// Every committed conformance fixture, sorted for a stable walk.
fn fixtures() -> Vec<std::path::PathBuf> {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("extensions/conformance/fixtures");
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("the fixtures directory exists: {err}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    paths.sort();
    paths
}

/// Every `.rs` file under the source trees, excluding build output.
fn source_files() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for tree in ["crates", "extensions", "tests"] {
        let walk = match std::fs::read_dir(root.join(tree)) {
            Ok(walk) => walk,
            Err(_) => continue,
        };
        for entry in walk.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, &mut files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files
}

fn collect_rs(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(walk) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in walk.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            // Target output and vendored trees are not consumers.
            if name != "target" && name != "node_modules" {
                collect_rs(&path, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

struct Allow;
impl lca_permissions::PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

const MANIFEST: &str = include_str!("../../extensions/conformance/extension.toml");

fn host(tag: &str) -> ExtHost {
    let root = lca_testkit::scratch_path(&format!("lca-427-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 10_000_000,
            log_limit_bytes: 4096,
        },
        Arc::new(HostEnvironment {
            roots: lca_permissions::ScopeRoots {
                workspace: root.join("workspace"),
                private: root.join("private"),
                home_config: root.join("config"),
                temp: root.join("tmp"),
                state_dir: root.join("data"),
            },
            prompt: Arc::new(Mutex::new(Allow)),
            grant_store: Arc::new(Mutex::new(
                lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grants"),
            )),
            project: root.join("workspace"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        }),
    )
}

// Verifies: gh #427's mechanism - a fixture no test references is a fixture
// nothing keeps current. The pre-fix tree fails here naming `component.wasm`,
// the byte-identical twin added in `df3663e` and frozen since `d24192b`.
#[test]
fn no_committed_fixture_sits_unconsumed_in_the_repository() {
    let sources = source_files();
    assert!(
        sources.len() > 100,
        "the walk saw the repo: {}",
        sources.len()
    );
    let texts: Vec<String> = sources
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap_or_default())
        .collect();

    let mut orphans = Vec::new();
    for fixture in fixtures() {
        let name = fixture
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // The full, package-qualified path as a STRING LITERAL (the trailing
        // `"`): another extension's `fixtures/component.wasm` must not count
        // as a consumer of this package's file of the same name - that
        // shadowing is precisely how the twin survived a repo-wide search -
        // and this row's own backtick-quoted comments must not satisfy it,
        // which is the self-reference hole the first draft had. Residual: a
        // comment quoting the path in double quotes still counts; named here
        // rather than parser-whacked away.
        let needle = format!("conformance/fixtures/{name}\"");
        if !texts.iter().any(|text| text.contains(&needle)) {
            orphans.push(name);
        }
    }
    assert!(
        orphans.is_empty(),
        "these committed fixtures are referenced by no test file, so nothing \
         rebuilds them and nothing would notice if they went stale (gh #427): \
         {orphans:?} - delete them, or give them a consumer that names them"
    );
}

// Verifies: gh #427 acceptance 1 - every committed fixture links on this host
// and satisfies every world the manifest declares. Pre-fix this fails with the
// issue's exact error: `no exported instance named
// 'lca:ext/hook-message-end@0.6.0'`.
#[test]
fn every_committed_fixture_links_against_the_current_host() {
    let listed = fixtures();
    assert!(!listed.is_empty(), "the package still commits a fixture");
    for path in listed {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(&path).unwrap_or_else(|err| panic!("read {name}: {err}"));
        let extension = host(&name.replace('.', "-"))
            .load(&bytes, MANIFEST)
            .unwrap_or_else(|err| {
                panic!(
                    "fixture {name} must link against the current ABI 0.6 host before any \
                     test loads it (gh #427): {err}"
                )
            });
        assert_eq!(extension.name(), "conformance");
        assert!(extension.is_enabled());
    }
}
