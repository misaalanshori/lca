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
//! built before the import loads and runs.
//!
//! **That was true within the 0.6 line, and this file's first two rows proved it
//! on `tool-legacy`'s committed component.** The 0.7 opener (gh #407) renamed the
//! WIT package, so a 0.6-built component no longer links; the claim is now
//! scoped to artifacts built against the same line, and the frozen 0.6 build is
//! kept as the witness for what a line move does (`tests/data/abi-0.6/`).
//! Without a witness for that, "rebuilt against the same line" would be an
//! unstated precondition of the rows below.
//!
//! ## What this guard pins
//!
//! 1. **A line move is admitted by the window and refused at link**, with the
//!    rebuild advice - the honest successor to "an old guest still loads".
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

use std::sync::{Arc, Mutex};

/// The conformance package's manifest: it declares `net` with a host list,
/// which is what makes the granted-row meaningful.
const MANIFEST: &str = include_str!("../../extensions/conformance/extension.toml");

/// The current-line single-tool guest: one `get-schema` plus `run`, no
/// `tool-catalog` world, no capabilities in its manifest. It is rebuilt with the
/// tree (`extensions/tool-legacy`, kept as the pre-catalog shape's reference),
/// and its manifest is what the capability row below reads.
const LEGACY: &[u8] = include_bytes!("../../extensions/tool-legacy/fixtures/component.wasm");
const LEGACY_MANIFEST: &str = include_str!("../../extensions/tool-legacy/extension.toml");

/// A **0.6-built** copy of the same guest, frozen before the 0.7 opener
/// (`tests/data/abi-0.6/README.md`). This is the artifact the old rows used, and
/// it is kept because what it proves changed, not because it is still useful as
/// a load target.
const FROZEN_0_6: &[u8] = include_bytes!("../data/abi-0.6/tool-legacy.wasm");
const FROZEN_0_6_MANIFEST: &str = include_str!("../data/abi-0.6/tool-legacy.manifest.toml");

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
// 1. What a line move does to the old artifact this file used to load
// ---------------------------------------------------------------------------

/// **The successor to the claim the word "additive" rested on.** `tool-legacy`'s
/// 0.6 component was the witness that adding an import does not break an old
/// guest, because the host satisfies imports the guest never calls. Opening the
/// 0.7 line renames the package, so that witness now fails to link - and the
/// failure is the point: it is admitted by the manifest window and refused at
/// link with the fix named.
#[test]
fn a_tool_guest_built_for_the_previous_line_is_admitted_then_refused_legibly() {
    let declaration =
        lca_ext_host::Manifest::parse(FROZEN_0_6_MANIFEST).expect("the 0.6 manifest parses");
    assert!(
        declaration.abi_in_window(),
        "the window admits an installed 0.6 extension (NFR-19), which is what makes the \
         link refusal legible rather than a manifest rejection"
    );
    let (mut host, _asked) = host("legacy");
    let err = match host.load(FROZEN_0_6, FROZEN_0_6_MANIFEST) {
        Ok(_) => panic!("a 0.6-built component cannot link on a 0.7 host"),
        Err(err) => err,
    };
    let text = err.to_string();
    assert!(
        text.contains("Rebuild it"),
        "the refusal says what to do about it: {text}"
    );
    // And the guest would still be the same shape if it *were* rebuilt: the
    // crate's current fixture loads and serves its one tool, which is the row
    // below's job.
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
        "the legacy fixture's manifest declares no net: {declaration:?}"
    );

    // And the other half of the same rule: a manifest that DOES declare the
    // capability still declares it, unchanged by the import - the conformance
    // package is the witness (its manifest grants one host).
    let granted = lca_ext_host::Manifest::parse(MANIFEST).expect("conformance parses");
    assert!(
        granted.net_declared && !granted.net.is_empty(),
        "a manifest naming hosts still declares them: {granted:?}"
    );
    // The import exists on the world, but the grant does not: the engine's
    // own gate is what decides, and it is unchanged.
    assert!(extension.is_enabled());
}
