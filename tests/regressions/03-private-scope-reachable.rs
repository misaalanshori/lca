//! Finding 3 (high, `../lca-issues.md`): the production `private` root was
//! built under the state directory, which the scope resolver refused before
//! the scope check, so `private` was unreachable. The one sanctioned
//! exception must resolve, and the state directory stays refused through
//! every other scope.
//!
//! Verifies: FR-PERM-12, ADR-0005 (defect 3).

use lca_permissions::{FsMode, ScopeGrant, ScopeRoots, ScopeViolationKind};

#[test]
fn private_resolves_under_the_state_dir_and_home_config_still_cannot_reach_it() {
    let root = lca_testkit::scratch_path("regression-private");
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
}
