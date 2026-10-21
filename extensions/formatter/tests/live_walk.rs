//! The live receipt for gh #357: the extension's own walk, over the real
//! `lca_tools::Capabilities` capability engine and a real scratch filesystem,
//! with a real process capability.
//!
//! The unit rows drive a fabricated tree, which is right for pinning the rule.
//! This one drives the engine a shipped binary uses, so it also covers the
//! path the fix has to survive: `fs_list`/`fs_stat` through the capability
//! boundary, and `process_spawn` for the formatter that does run.
//!
//! Verifies: gh #357 acceptance 1 — a bare `run` over a workspace containing
//! `.git/`, `target/`, and `src/` reformats `src/main.rs` while every file
//! under the skip roots stays byte-identical.
//!
//! `rustfmt` is a real dependency here; the row skips (rather than fails) when
//! it is absent, which is the same degradation the extension itself has.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use formatter::{FormatterState, HookOutcome};
use lca_permissions::{FsMode, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

/// Approve everything: this test is about the walk, not the prompt.
struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

/// A scratch workspace laid out exactly as the issue describes.
struct Workspace {
    root: std::path::PathBuf,
}

impl Workspace {
    fn new(name: &str) -> Workspace {
        // A unique scratch root without pulling `lca-testkit` into this
        // crate's dev-dependencies: pid + name is enough to keep parallel
        // test binaries apart.
        let root =
            std::env::temp_dir().join(format!("lca-formatter-live-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["src", "target/gen", ".git/objects/ab", "node_modules/p"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        // Deliberately misformatted, so "reformatted" is observable.
        std::fs::write(root.join("src/main.rs"), "fn main(){let  x=1;}\n").expect("write");
        std::fs::write(
            root.join("target/gen/out.rs"),
            "fn generated(){let  y=2;}\n",
        )
        .expect("write");
        std::fs::write(root.join("node_modules/p/index.js"), "var  z=3\n").expect("write");
        std::fs::write(root.join(".git/objects/ab/cdef"), "binary-ish\n").expect("write");
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write");
        Workspace { root }
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).expect("read")
    }

    fn capabilities(&self) -> Arc<Capabilities> {
        let grants = CapabilityGrants {
            fs: vec![ScopeGrant::parse("workspace", FsMode::ReadWrite).expect("scope")],
            fs_declared: true,
            process: true,
            ..Default::default()
        };
        Arc::new(Capabilities::new(
            "formatter",
            grants,
            ScopeRoots {
                workspace: self.root.clone(),
                private: self.root.join(".lca-private"),
                home_config: self.root.join(".lca-config"),
                temp: self.root.join(".lca-tmp"),
                state_dir: self.root.join(".lca-state"),
            },
            Arc::new(Mutex::new(Allow)),
            Arc::new(Mutex::new(
                GrantStore::open(&self.root.join(".lca-grants.json")).expect("grants"),
            )),
            self.root.clone(),
            None,
        ))
    }
}

fn rustfmt_present() -> bool {
    std::process::Command::new("rustfmt")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

// Verifies: gh #357 acceptance 1 — the real engine, the real filesystem.
#[test]
fn a_bare_run_over_the_real_engine_touches_only_source() {
    if !rustfmt_present() {
        eprintln!("skipping: rustfmt is not installed (the extension degrades the same way)");
        return;
    }
    let workspace = Workspace::new("bare-run");
    let cap = workspace.capabilities();

    let before_target = workspace.read("target/gen/out.rs");
    let before_node = workspace.read("node_modules/p/index.js");
    let before_git = workspace.read(".git/objects/ab/cdef");
    let before_src = workspace.read("src/main.rs");

    let state = FormatterState::new();
    let report = state.handle_command(cap.as_ref(), cap.as_ref(), "run");
    eprintln!("formatter run report: {report}");

    // The source file was reformatted.
    let after_src = workspace.read("src/main.rs");
    assert_ne!(
        after_src, before_src,
        "src/main.rs was reformatted (report: {report})"
    );
    assert!(report.contains("src/main.rs: rustfmt"), "{report}");

    // Every generated / VCS file is byte-identical.
    assert_eq!(
        workspace.read("target/gen/out.rs"),
        before_target,
        "target/gen/out.rs must be untouched"
    );
    assert_eq!(
        workspace.read("node_modules/p/index.js"),
        before_node,
        "node_modules content must be untouched"
    );
    assert_eq!(
        workspace.read(".git/objects/ab/cdef"),
        before_git,
        ".git content must be untouched"
    );

    // And the report never names a skip-root path as formatted.
    for line in report.lines() {
        for root in lca_tools::SKIP_DIRS {
            assert!(
                !(line.starts_with(root) || line.contains(&format!(" {root}/"))),
                "the report names a skip-root path `{line}`"
            );
        }
    }
}

// Verifies: gh #357 acceptance 2 — an explicit generated path is refused by
// the real engine, and the file is left alone.
#[test]
fn an_explicit_generated_path_is_refused_by_the_real_engine() {
    let workspace = Workspace::new("explicit-refusal");
    let cap = workspace.capabilities();
    let before = workspace.read("target/gen/out.rs");

    let state = FormatterState::new();
    let report = state.handle_command(cap.as_ref(), cap.as_ref(), "run target/gen/out.rs");
    assert!(report.contains("refused"), "{report}");
    assert!(report.contains("target"), "names the root: {report}");
    assert_eq!(
        workspace.read("target/gen/out.rs"),
        before,
        "the refused file is untouched"
    );

    // The same refusal through `format_file`, which is the hook's path.
    let outcome = state.format_file(cap.as_ref(), cap.as_ref(), "target/gen/out.rs");
    assert!(matches!(outcome, HookOutcome::Refused(_)), "{outcome:?}");
}
