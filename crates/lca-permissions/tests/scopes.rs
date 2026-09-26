//! Filesystem scope resolution (ADR-0005, `docs/capabilities.md` `fs`):
//! named vocabulary, mode enforcement, traversal and symlink refusal, and
//! the state-directory exclusion that keeps credential isolation true.

use lca_permissions::{FsMode, ScopeGrant, ScopeRoots, ScopeViolationKind};

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("lca-scope-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["workspace", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Sandbox { root }
    }

    fn roots(&self) -> ScopeRoots {
        ScopeRoots {
            workspace: self.root.join("workspace"),
            private: self.root.join("private"),
            home_config: self.root.join("config"),
            temp: self.root.join("tmp"),
            state_dir: self.root.join("data"),
        }
    }

    fn granted(&self, entries: &[(&str, FsMode)]) -> Vec<ScopeGrant> {
        entries
            .iter()
            .map(|(name, mode)| ScopeGrant::parse(name, *mode).expect("known scope"))
            .collect()
    }
}

// The fixed vocabulary: a manifest names a scope, never a path
// (ADR-0005), and an unknown name is refused (deny by default, NFR-13).
#[test]
fn vocabulary_is_fixed_and_unknown_scopes_are_refused() {
    let sandbox = Sandbox::new("vocab");
    let roots = sandbox.roots();
    for name in ["workspace", "private", "home-config", "temp"] {
        assert!(
            ScopeGrant::parse(name, FsMode::Read).is_ok(),
            "{name} is in the vocabulary"
        );
    }
    let err = ScopeGrant::parse("home", FsMode::Read).expect_err("not a scope name");
    assert!(
        matches!(err, ScopeViolationKind::UnknownScope),
        "got {err:?}"
    );
    let _ = roots;
}

// Verifies: FR-PERM-12 (a guest path resolution that leaves its granted
// scope is refused).
#[test]
fn parent_traversal_out_of_a_scope_is_refused() {
    let sandbox = Sandbox::new("traversal");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::Read)]);

    let ok = roots.resolve(&grants, "workspace", "src/main.rs", false);
    assert!(ok.is_ok(), "inside the scope resolves: {ok:?}");

    let err = roots
        .resolve(&grants, "workspace", "../../etc/passwd", false)
        .expect_err("escapes");
    assert!(
        matches!(err.kind, ScopeViolationKind::Escape),
        "got {err:?}"
    );
}

// The boundary class this check exists for: a symlink created AFTER the
// grant, pointing outside the scope (capability catalog, threat model).
// Creating symlinks on Windows needs Developer Mode or elevation; the
// requirement stays covered on unix, noted in the phase log.
#[cfg(unix)]
#[test]
fn a_symlink_created_after_the_grant_is_refused() {
    let sandbox = Sandbox::new("symlink");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::ReadWrite)]);

    // Grant resolves normally first.
    assert!(roots.resolve(&grants, "workspace", "ok.txt", true).is_ok());

    // Now plant a link out of the workspace.
    std::os::unix::fs::symlink("/etc/hostname", sandbox.root.join("workspace/escape"))
        .expect("symlink");
    let err = roots
        .resolve(&grants, "workspace", "escape", false)
        .expect_err("link leaves the scope");
    assert!(
        matches!(err.kind, ScopeViolationKind::Escape),
        "got {err:?}"
    );

    // A dangling link to outside is refused too (deepest-existing-prefix
    // canonicalization still lands outside).
    std::os::unix::fs::symlink("/nowhere/outside", sandbox.root.join("workspace/dangling"))
        .expect("symlink");
    let err = roots
        .resolve(&grants, "workspace", "dangling", false)
        .expect_err("link target is outside");
    assert!(
        matches!(err.kind, ScopeViolationKind::Escape),
        "got {err:?}"
    );
}

// The state directory (sessions, extension tree, credential store) is
// refused under EVERY scope, including home-config, which is what keeps
// credential isolation true where the config directory also holds
// application data (platform notes, macOS).
#[test]
fn the_state_directory_is_refused_under_every_scope() {
    let sandbox = Sandbox::new("state");
    let roots = sandbox.roots();
    // Simulate the macOS overlap: a scope root that contains the state dir.
    let grants = sandbox.granted(&[("home-config", FsMode::Read)]);
    std::fs::create_dir_all(sandbox.root.join("config/lca-sessions")).expect("mkdir");

    // A path inside home-config but INTO the state dir: pretend state lives
    // under config for this case by pointing roots' state_dir there.
    let roots = ScopeRoots {
        state_dir: sandbox.root.join("config/lca-sessions"),
        ..roots
    };

    let err = roots
        .resolve(&grants, "home-config", "lca-sessions/log.jsonl", false)
        .expect_err("state dir refused");
    assert!(
        matches!(err.kind, ScopeViolationKind::StateDirectory),
        "got {err:?}"
    );

    // Ordinary config files still resolve.
    std::fs::write(sandbox.root.join("config/tool.toml"), "x").expect("write");
    assert!(
        roots
            .resolve(&grants, "home-config", "tool.toml", false)
            .is_ok()
    );
}

// The production layout: `private` lives under the state directory and must
// resolve (the one sanctioned exception), while the macOS/Windows overlap
// where home-config contains the state directory still refuses the agent's
// own subtrees. This is exactly the fixture drift the audit found: the tests
// used separate private/state roots and never saw `private` be unusable.
#[test]
fn private_resolves_under_the_state_dir_and_home_config_still_cannot_reach_it() {
    let root = lca_testkit::scratch_path("lca-scope-prod");
    let _ = std::fs::remove_dir_all(&root);
    let home_config = root.join("app-support");
    let data = home_config.join("lca");
    let private = data.join("private").join("ext");
    let workspace = root.join("ws");
    for dir in [
        &private,
        &workspace,
        &data.join("sessions"),
        &data.join("credentials"),
    ] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace,
        private: private.clone(),
        home_config: home_config.clone(),
        temp: root.join("tmp"),
        state_dir: data.clone(),
    };

    let private_grant = vec![ScopeGrant::parse("private", FsMode::ReadWrite).expect("grant")];
    let resolved = roots
        .resolve(&private_grant, "private", "cache.bin", true)
        .expect("private must resolve on the production layout");
    // Compare canonically: temp_dir is a symlink on macOS, and the resolved
    // path is canonical.
    let canonical_private = std::fs::canonicalize(&private).expect("canonical private");
    assert!(resolved.starts_with(&canonical_private), "{resolved:?}");

    let home_grant = vec![ScopeGrant::parse("home-config", FsMode::Read).expect("grant")];
    let refused = roots
        .resolve(
            &home_grant,
            "home-config",
            "lca/credentials/ext.json",
            false,
        )
        .expect_err("the agent's own state is refused through home-config");
    assert!(
        matches!(refused.kind, ScopeViolationKind::StateDirectory),
        "{refused:?}"
    );

    std::fs::write(home_config.join("other-tool.toml"), "x").expect("write");
    assert!(
        roots
            .resolve(&home_grant, "home-config", "other-tool.toml", false)
            .is_ok(),
        "another tool's config still resolves"
    );
}

// Deny by default: a scope absent from the granted set cannot be reached,
// whatever the manifest (or component) says (NFR-13, capability catalog).
#[test]
fn ungranted_scopes_are_refused_at_resolution_time() {
    let sandbox = Sandbox::new("ungranted");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::Read)]);
    let err = roots
        .resolve(&grants, "private", "cache.bin", false)
        .expect_err("not granted");
    assert!(
        matches!(err.kind, ScopeViolationKind::NotGranted),
        "got {err:?}"
    );
}

// Modes: a read grant refuses writes (capability catalog `fs` modes).
#[test]
fn read_grants_refuse_writes() {
    let sandbox = Sandbox::new("modes");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::Read)]);
    std::fs::write(sandbox.root.join("workspace/a.txt"), "x").expect("write");
    let err = roots
        .resolve(&grants, "workspace", "a.txt", true)
        .expect_err("write under a read grant");
    assert!(
        matches!(err.kind, ScopeViolationKind::ModeRefused),
        "got {err:?}"
    );
    assert!(roots.resolve(&grants, "workspace", "a.txt", false).is_ok());
}

// Absolute paths and NUL bytes are refused outright: the guest only ever
// names paths relative to a handle (ADR-0005).
#[test]
fn absolute_paths_are_refused() {
    let sandbox = Sandbox::new("absolute");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::ReadWrite)]);
    let err = roots
        .resolve(&grants, "workspace", "/etc/passwd", false)
        .expect_err("absolute");
    assert!(
        matches!(err.kind, ScopeViolationKind::Escape),
        "got {err:?}"
    );
}

// The refusal carries enough detail for the record the catalog demands
// (extension identity is added by the caller).
#[test]
fn violations_describe_themselves_for_the_record() {
    let sandbox = Sandbox::new("detail");
    let roots = sandbox.roots();
    let grants = sandbox.granted(&[("workspace", FsMode::Read)]);
    let err = roots
        .resolve(&grants, "workspace", "../outside", false)
        .expect_err("escape");
    let text = err.to_string();
    assert!(text.contains("workspace"), "{text}");
    assert!(text.contains("../outside"), "{text}");
}
