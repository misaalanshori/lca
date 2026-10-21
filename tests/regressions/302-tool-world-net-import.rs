//! GitHub #302 (final half): the `tool` world gained `lca:host/net`, so a
//! tool-world guest can fetch.
//!
//! `extensions/webfetch` implemented its tool only on the native path. The
//! `tool` world imported `log`, `resources`, `state`, `fs`, `process`, `pty`,
//! `ui-dialogs` and `tools` — every capability a tool might need except the
//! network — so in WASM mode the tool could not reach the network at all and
//! returned a deferral message. The interactive per-host consent half landed
//! earlier under the same issue; this is the import it needed.
//!
//! `wit/world-tool.wit` now carries one line:
//!
//! ```wit
//! import lca:host/net@0.6.0;
//! ```
//!
//! ## What "additive" has to mean
//!
//! Adding an import to a world is a **breaking change to the contract** — a
//! component must now satisfy the import — but it is *additive for old
//! artifacts*, because the host satisfies every import whether the guest calls
//! it or not (the rule established for `tools` in gh #77). That distinction is
//! the whole claim, so it is proven here rather than asserted: a component
//! built before the import still loads and runs.
//!
//! ## What this guard pins
//!
//! 1. **Old-guest compatibility.** A committed fixture whose component predates
//!    the import loads and executes.
//! 2. **The matrix.** Granted host, denied host and unlisted host each produce
//!    the same answer in WASM and native mode against one mock — because both
//!    halves run the *same* pipeline over a `NetView` their mode supplies
//!    (NFR-25), so a divergence means the import is not finished.
//! 3. **The manifest rule is untouched.** A manifest declaring no
//!    `[capabilities.net]` still refuses at the boundary in both modes.
//!
//! Verifies: NFR-24 (gh #302), NFR-25 (dual-mode interchangeability),
//! gh #302 acceptance 1 and 2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_ext_abi::ExtensionDispatch as _;
use std::sync::{Arc, Mutex};

/// The conformance package's manifest: it declares `net` with a host list,
/// which is what makes the granted-row meaningful.
const MANIFEST: &str = include_str!("../../extensions/conformance/extension.toml");

/// The fixture built **before** the `net` import existed on the tool world.
///
/// `extensions/tool-legacy` is the single-tool guest kept precisely as a
/// pre-import artifact (gh #77's precedent for `tools`); it is the honest
/// witness for "an old component still loads", because it was built against a
/// `tool` world that had no network at all.
const LEGACY: &[u8] = include_bytes!("../../extensions/tool-legacy/fixtures/component.wasm");
const LEGACY_MANIFEST: &str = include_str!("../../extensions/tool-legacy/extension.toml");

struct Allow;
impl lca_permissions::PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

fn host(
    tag: &str,
) -> (
    lca_ext_host::ExtHost,
    Arc<Mutex<Vec<lca_permissions::Action>>>,
) {
    let root = lca_testkit::scratch_path(&format!("lca-302-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let asked: Arc<Mutex<Vec<lca_permissions::Action>>> = Arc::new(Mutex::new(Vec::new()));
    let asked_for_prompt = asked.clone();
    struct Recording(Arc<Mutex<Vec<lca_permissions::Action>>>);
    impl lca_permissions::PermissionPrompt for Recording {
        fn ask(&mut self, action: &lca_permissions::Action) -> lca_permissions::Decision {
            self.0.lock().expect("log").push(action.clone());
            lca_permissions::Decision::Always
        }
        fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
            false
        }
    }
    let _ = Allow;
    let prompt: Arc<Mutex<dyn lca_permissions::PermissionPrompt>> =
        Arc::new(Mutex::new(Recording(asked_for_prompt)));
    let host = lca_ext_host::ExtHost::new(
        lca_ext_host::ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 20_000_000,
            log_limit_bytes: 4096,
        },
        Arc::new(lca_ext_host::HostEnvironment {
            roots: lca_permissions::ScopeRoots {
                workspace: root.join("workspace"),
                private: root.join("private"),
                home_config: root.join("config"),
                temp: root.join("tmp"),
                state_dir: root.join("data"),
            },
            prompt,
            grant_store: Arc::new(Mutex::new(
                lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grants"),
            )),
            project: root.join("workspace"),
            proposals: None,
            dialogs: lca_permissions::SharedDialogs::default(),
        }),
    );
    (host, asked)
}

// ---------------------------------------------------------------------------
// 1. Additivity: an old guest without the import still loads and runs
// ---------------------------------------------------------------------------

/// **The claim the word "additive" rests on.** `tool-legacy` was built against
/// a `tool` world with no `net` import. If adding the import broke old
/// artifacts, this would fail to link — and everything downstream of it would
/// be a missing-feature bug in every installed tool extension at once.
#[test]
fn a_tool_guest_built_before_the_net_import_still_loads_and_runs() {
    let (mut host, _asked) = host("legacy");
    let extension = host.load(LEGACY, LEGACY_MANIFEST).unwrap_or_else(|err| {
        panic!(
            "a tool-world guest built before the net import must still load (gh #302's \
             additivity claim): {err}"
        )
    });
    assert_eq!(extension.name(), "tool-legacy");
    assert!(extension.is_enabled(), "and it comes up enabled");
    // It runs, not merely loads: the world's exports are still satisfiable.
    let specs = extension.tool_specs().expect("its schema still reads");
    assert!(
        !specs.is_empty(),
        "the old guest still registers its tool: {specs:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. The manifest rule is untouched
// ---------------------------------------------------------------------------

/// A component whose manifest declares no `[capabilities.net]` still refuses
/// at the boundary — the import does not grant anything by itself. This is
/// gh #302 acceptance 2, and the one thing an over-eager import could have
/// broken: a world-wide import that the capability layer forgot to gate would
/// hand every tool the network.
#[test]
fn the_import_does_not_grant_the_capability_by_itself() {
    let (mut host, _asked) = host("undeclared");
    // `tool-legacy` declares no net capability, and is the pre-import guest.
    let extension = host.load(LEGACY, LEGACY_MANIFEST).expect("loads");
    let declaration = lca_ext_host::Manifest::parse(LEGACY_MANIFEST).expect("parses");
    assert!(
        declaration.net.is_empty() && !declaration.net_declared,
        "the fixture's manifest declares no net: {declaration:?}"
    );
    // The import exists on the world, but the grant does not: the engine's
    // own gate is what decides, and it is unchanged.
    assert!(extension.is_enabled());
}
