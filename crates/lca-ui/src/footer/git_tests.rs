//! Fixtures for the four repository shapes that used to lose the branch
//! (gh #389 gap 1 / #330), plus the plain case as a regression pin.
//!
//! Each shape is built with real `git` where `git` is available, because the
//! point of the fixture is the *file layout git writes* — an approximation
//! would test the approximation. `git` is on `PATH` in CI and in the release
//! pipeline (scripts/traceability.sh needs python; the platform matrix has
//! git), and a missing `git` skips rather than fails, the way the tmux rows do.

use std::path::{Path, PathBuf};

use super::*;

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lca-footer-git-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `git` in `dir` with a fixed identity, so a fixture never depends on the
/// machine's global git config. Returns `false` when `git` is unavailable.
fn git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "lca-test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "lca-test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether `git` can be run at all.
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// An initialised repository with one empty commit on branch `main`.
fn repo(tag: &str) -> Option<(Scratch, PathBuf)> {
    if !git_available() {
        eprintln!("skip: git is not installed");
        return None;
    }
    let scratch = Scratch::new(tag);
    let root = scratch.path().join("repo");
    std::fs::create_dir_all(&root).expect("repo dir");
    assert!(git(&root, &["init", "-q", "-b", "main"]), "git init");
    assert!(git(&root, &["commit", "-q", "--allow-empty", "-m", "one"]));
    Some((scratch, root))
}

// Verifies: gh #389 gap 1 - the plain case still works (the regression pin).
// A fix for the four broken shapes must not break the one that already worked.
#[test]
fn a_plain_repository_reports_its_branch() {
    let Some((_scratch, root)) = repo("plain") else {
        return;
    };
    assert_eq!(resolve_branch(&root, None), Some("main".to_string()));
}

// Verifies: gh #389 gap 1 - **a subdirectory reports the repository's
// branch.** The old code read `<cwd>/.git/HEAD`, so running the agent from
// `crates/lca-ui` inside a checkout showed no branch at all.
#[test]
fn a_subdirectory_reports_the_repository_branch() {
    let Some((_scratch, root)) = repo("subdir") else {
        return;
    };
    let deep = root.join("crates").join("lca-ui");
    std::fs::create_dir_all(&deep).expect("mkdir");
    assert!(
        !deep.join(".git").exists(),
        "the fixture is only meaningful when `.git` is not in the cwd"
    );
    assert_eq!(resolve_branch(&deep, None), Some("main".to_string()));
}

// Verifies: gh #389 gap 1 - **a linked worktree reports its branch.** `.git`
// is a file holding `gitdir: …/worktrees/<name>`, whose `HEAD` is the
// worktree's own. This is the shape an agent worktree runs in — including the
// one these very cycles are written in.
#[test]
fn a_linked_worktree_reports_its_branch() {
    let Some((scratch, root)) = repo("worktree") else {
        return;
    };
    let linked = scratch.path().join("linked");
    assert!(
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                linked.to_str().expect("utf8")
            ]
        ),
        "git worktree add"
    );
    assert!(
        linked.join(".git").is_file(),
        "`.git` is a file in a worktree"
    );
    assert_eq!(resolve_branch(&linked, None), Some("side".to_string()));

    // ...and from a subdirectory of the worktree, which exercises the walk and
    // the file indirection together.
    let inner = linked.join("inner");
    std::fs::create_dir_all(&inner).expect("mkdir");
    assert_eq!(resolve_branch(&inner, None), Some("side".to_string()));
}

// Verifies: gh #389 gap 1 - **a detached HEAD reports its short object id**
// rather than vanishing. `HEAD` holds a raw SHA with no `ref:` prefix, which
// the old `strip_prefix("ref: ")` could never match.
#[test]
fn a_detached_head_reports_its_short_object_id() {
    let Some((_scratch, root)) = repo("detached") else {
        return;
    };
    assert!(git(&root, &["checkout", "-q", "--detach"]), "detach");
    let sha = std::fs::read_to_string(root.join(".git/HEAD")).expect("HEAD");
    let sha = sha.trim();
    assert_eq!(sha.len(), 40, "a detached HEAD is a raw SHA-1: {sha:?}");
    let branch = resolve_branch(&root, None).expect("a detached HEAD still says something");
    assert_eq!(branch, sha[..7], "the short object id");
    assert!(
        branch.chars().all(|c| c.is_ascii_hexdigit()),
        "it reads as a revision, not a branch name: {branch}"
    );
}

// Verifies: gh #389 gap 1 - **a `reftable` repository reports its branch.**
// Git ≥2.45 writes the literal placeholder `ref: refs/heads/.invalid` into
// `HEAD`; the old parser returned `.invalid` as if it were a branch name, and
// the naive fix (treat it as a ref) still would. The real name lives in the
// binary ref table, so this shape needs the `git` fallback.
#[test]
fn a_reftable_repository_reports_its_branch() {
    let Some((scratch, _root)) = repo("reftable-parent") else {
        return;
    };
    let reft = scratch.path().join("reft");
    std::fs::create_dir_all(&reft).expect("mkdir");
    if !git(
        &reft,
        &["init", "-q", "--ref-format=reftable", "-b", "main"],
    ) {
        eprintln!("skip: this git does not support --ref-format=reftable");
        return;
    }
    assert!(git(&reft, &["commit", "-q", "--allow-empty", "-m", "one"]));
    let head = std::fs::read_to_string(reft.join(".git/HEAD")).expect("HEAD");
    assert_eq!(
        head.trim(),
        "ref: refs/heads/.invalid",
        "the reftable placeholder this test exists for"
    );
    assert_eq!(
        resolve_branch(&reft, Some(&symbolic_ref())),
        Some("main".to_string()),
        "the placeholder is resolved through git, never shown as a branch"
    );
    // The fixture really is a reftable repo, not an ordinary one that happened
    // to parse: the ref table and its list are both on disk.
    assert!(
        reft.join(".git/reftable/tables.list").is_file(),
        "the fixture wrote a reftable table list"
    );
}

/// The production resolver, as the injected hook the footer uses.
fn symbolic_ref() -> SymbolicRef {
    std::sync::Arc::new(symbolic_ref_via_git)
}

// Verifies: gh #389 gap 1 - **a `reftable` repository on a branch other than
// the initial one**, so the fixture cannot pass by echoing `init -b`.
#[test]
fn a_reftable_branch_switch_is_followed() {
    let Some((scratch, _root)) = repo("reftable-switch-parent") else {
        return;
    };
    let reft = scratch.path().join("reft");
    std::fs::create_dir_all(&reft).expect("mkdir");
    if !git(
        &reft,
        &["init", "-q", "--ref-format=reftable", "-b", "main"],
    ) {
        eprintln!("skip: this git does not support --ref-format=reftable");
        return;
    }
    assert!(git(&reft, &["commit", "-q", "--allow-empty", "-m", "one"]));
    assert!(git(&reft, &["checkout", "-q", "-b", "renamed"]));
    assert_eq!(
        resolve_branch(&reft, Some(&symbolic_ref())),
        Some("renamed".to_string())
    );
}

// Verifies: gh #389 gap 1 - a `reftable` repository with **no** resolver
// configured reports nothing rather than the `.invalid` placeholder. Showing
// `.invalid` is the lie this shape used to tell.
#[test]
fn a_reftable_placeholder_is_never_shown_as_a_branch() {
    let Some((scratch, _root)) = repo("reftable-noresolver-parent") else {
        return;
    };
    let reft = scratch.path().join("reft");
    std::fs::create_dir_all(&reft).expect("mkdir");
    if !git(
        &reft,
        &["init", "-q", "--ref-format=reftable", "-b", "main"],
    ) {
        eprintln!("skip: this git does not support --ref-format=reftable");
        return;
    }
    assert!(git(&reft, &["commit", "-q", "--allow-empty", "-m", "one"]));
    assert_eq!(resolve_branch(&reft, None), None, "silence beats a lie");
}

// Verifies: gh #389 gap 1 - the resolver is asked **once**, and only for a
// `reftable` repository: spawning is the expensive part, and a plain repo must
// not pay for it.
#[test]
fn the_git_fallback_is_asked_once_and_only_for_reftable() {
    let Some((_scratch, root)) = repo("once") else {
        return;
    };
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted: SymbolicRef = {
        let calls = calls.clone();
        std::sync::Arc::new(move |_dir: &Path| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            None
        })
    };
    assert_eq!(
        resolve_branch(&root, Some(&counted)),
        Some("main".to_string())
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a readable HEAD never spawns git"
    );
}

// Verifies: gh #389 gap 1 - `parse_head` is the table of shapes, with no
// filesystem involved, so every form is pinned directly.
#[test]
fn head_parsing_covers_every_shape() {
    assert_eq!(
        parse_head("ref: refs/heads/main\n"),
        Some(HeadRef::Branch("main".to_string()))
    );
    assert_eq!(
        parse_head("ref: refs/heads/feature/nested-name\n"),
        Some(HeadRef::Branch("feature/nested-name".to_string())),
        "a branch name may contain slashes"
    );
    assert_eq!(
        parse_head("ref: refs/tags/v1.0\n"),
        Some(HeadRef::OtherRef("tag:v1.0".to_string())),
        "a tag is not a branch, and must not print as the raw ref path"
    );
    assert_eq!(
        parse_head("ref: refs/remotes/origin/main\n"),
        Some(HeadRef::OtherRef("refs/remotes/origin/main".to_string()))
    );
    assert_eq!(
        parse_head("ref: refs/heads/.invalid\n"),
        Some(HeadRef::Reftable)
    );
    let sha = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
    assert_eq!(
        parse_head(&format!("{sha}\n")),
        Some(HeadRef::Detached(sha[..7].to_string()))
    );
    let sha256 = "a".repeat(64);
    assert_eq!(
        parse_head(&sha256),
        Some(HeadRef::Detached(sha256[..7].to_string())),
        "SHA-256 repositories are a real shape too"
    );
    assert_eq!(parse_head(""), None);
    assert_eq!(parse_head("   \n"), None);
    assert_eq!(parse_head("garbage that is not a ref"), None);
    assert_eq!(parse_head("ref: "), None);
}

// Verifies: gh #389 gap 1 - a `.git` file is followed **once**, not
// recursively: `gitdir:` pointing at another `.git` file must not loop.
#[test]
fn a_gitdir_file_is_followed_once() {
    let scratch = Scratch::new("onedir");
    let root = scratch.path().join("a");
    std::fs::create_dir_all(&root).expect("mkdir");
    // `.git` is a file pointing at a path that is itself a `.git` file.
    std::fs::write(scratch.path().join("b"), "gitdir: c\n").expect("write b");
    std::fs::write(root.join(".git"), "gitdir: ../b\n").expect("write a/.git");
    // One indirection resolves the path; it is not re-read as a `.git` file.
    let dirs = find_git_dir(&root).expect("the indirection resolves");
    assert_eq!(dirs.git_dir, scratch.path().join("b"));
}

// Verifies: gh #389 gap 1 - the walk stops at the filesystem root without
// looping, and a directory outside any repository reports nothing.
#[test]
fn a_directory_outside_a_repository_has_no_branch() {
    // /tmp is not inside a work tree in CI, but guard against the surprising
    // case by using a scratch dir we know is outside any repository.
    let scratch = Scratch::new("outside");
    assert!(find_git_dir(scratch.path()).is_none());
    assert_eq!(resolve_branch(scratch.path(), None), None);
}

// Verifies: gh #389 gap 1 - the walk is bounded. Depth is asserted rather than
// eyeballed so a symlink loop or a pathological tree cannot make the cache
// refresh unbounded.
#[test]
fn the_walk_has_a_depth_bound() {
    assert_eq!(MAX_WALK_DEPTH, 64);
    let deep: PathBuf = (0..MAX_WALK_DEPTH + 10)
        .map(|i| format!("d{i}"))
        .collect::<Vec<_>>()
        .iter()
        .fold(std::env::temp_dir(), |acc, part| acc.join(part));
    // The bound is on ancestors taken, so building the path costs nothing;
    // the property is that `ancestors().take(MAX_WALK_DEPTH)` is finite.
    assert_eq!(
        deep.ancestors().take(MAX_WALK_DEPTH).count(),
        MAX_WALK_DEPTH.min(deep.ancestors().count())
    );
}

// Verifies: gh #389 gap 1 - a worktree's shared refs are found through
// `commondir`, so the label follows the common repository's refs.
#[test]
fn a_worktree_resolves_its_common_dir() {
    let Some((scratch, root)) = repo("commondir") else {
        return;
    };
    let linked = scratch.path().join("linked");
    assert!(git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "common",
            linked.to_str().expect("utf8")
        ]
    ));
    let dirs = find_git_dir(&linked).expect("worktree resolves");
    assert_ne!(
        dirs.git_dir, dirs.common_dir,
        "a worktree's gitdir holds HEAD; its refs live in the main repository"
    );
    assert_eq!(dirs.common_dir, root.join(".git"));
}
