//! The git source rows (gh #59): a pinned commit resolves, an explicit rev pins,
//! and a hostile url/rev is refused before git is spawned.
//!
//! Split out of `tests/registry.rs` beside the `git` module; the repo fixtures are
//! in `support.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::support::{git_repo, head_sha};

// Verifies: gh #59 - `git+<path>` resolves the checkout like any
// other source: digest-verified bytes, consent-ready manifest, and
// the pinned commit recorded.
#[tokio::test]
async fn git_source_resolves_a_pinned_commit() {
    let Some(repo) = git_repo("git-pin") else {
        eprintln!("skip: git is not installed");
        return;
    };
    let sha = head_sha(&repo);
    let source = format!("git+{}", repo.display());
    let resolved = lca_registry::resolve(&source, None)
        .await
        .expect("git resolves");
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(b"v2-bytes-longer")
    );
    assert_eq!(resolved.commit, sha, "the HEAD commit pins the install");
    assert!(resolved.source.starts_with("git+"), "{}", resolved.source);
    assert!(resolved.manifest.contains("word-count"));
}

// Verifies: gh #59 - an explicit `#rev` pins that commit, not the
// floating HEAD: the older digest comes back.
#[tokio::test]
async fn git_source_pins_an_explicit_rev() {
    let Some(repo) = git_repo("git-rev") else {
        eprintln!("skip: git is not installed");
        return;
    };
    let out = std::process::Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .current_dir(&repo)
        .output()
        .expect("git runs");
    let first = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let resolved = lca_registry::resolve(&format!("git+{}#{first}", repo.display()), None)
        .await
        .expect("resolves");
    assert_eq!(
        resolved.digest,
        lca_registry::Resolved::digest_of(b"v1-bytes")
    );
    assert_eq!(resolved.commit, first);
}

// Verifies: gh #349 - an option-injected git url is refused by
// validation before any git process starts. The value reaches
// `parse_git_source`, sits in front of every git invocation, so the
// reject is `Error::Invalid`, never a `git clone` that merely fails.
#[test]
fn git_source_refuses_an_option_injected_url_before_spawning_git() {
    let root = lca_testkit::scratch_path("lca-registry-git-inject");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let sentinel = root.join("spawned");
    // The exact AC1 payload: `--upload-pack` runs a command for local
    // transports. The `#rev`-free form puts it in the url position.
    let source = format!("git+--upload-pack=touch {}", sentinel.display());
    let err = lca_registry::resolve_git(&source).expect_err("the injected url is refused");
    assert!(
        matches!(err, lca_registry::Error::Invalid(_)),
        "a validation error, not a fetch error: {err}"
    );
    assert!(!sentinel.exists(), "the injected command never ran");
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #349 - the same for a `#--force`-shaped rev: refused by
// validation, without spawning git (today it starts a clone that only
// fails on the transport).
#[test]
fn git_source_refuses_an_option_injected_rev_before_spawning_git() {
    let root = lca_testkit::scratch_path("lca-registry-git-inject-rev");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let source = format!("git+{}#--force", root.join("repo").display());
    let err = lca_registry::resolve_git(&source).expect_err("the injected rev is refused");
    assert!(
        matches!(err, lca_registry::Error::Invalid(_)),
        "a validation error, not a fetch error: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
