//! Git branch discovery for the footer (gh #389 gap 1, closing #330).
//!
//! The footer used to read `<cwd>/.git/HEAD` and nothing else, which is wrong in
//! four common shapes:
//!
//! | Shape | What `.git` is | What `HEAD` holds | Old result |
//! | --- | --- | --- | --- |
//! | Repository root | a directory | `ref: refs/heads/main` | correct |
//! | Subdirectory | absent (`../.git` has it) | — | **no branch** |
//! | Linked worktree / submodule | a *file* `gitdir: …` | in the gitdir | **no branch** |
//! | Detached HEAD | a directory | a raw 40-char SHA | **no branch** |
//! | `reftable` ref backend (Git ≥2.45) | a directory | `ref: refs/heads/.invalid` | **`.invalid`** |
//!
//! Each fails for a different reason, which is why each has its own fixture:
//! one fix would not cover them by accident.
//!
//! # Spawning is the expensive part, so it is almost never done
//!
//! A branch resolves from two small reads — `<gitdir>/HEAD` and, for a
//! worktree, `<commondir>/reftable/tables.list` — in every shape except
//! `reftable`, where Git stores the ref table in a binary format with no
//! ordering guarantee we may rely on (`tables.list` names the tables, but a
//! table's `#HEAD` section is not a pointer contract). So a `reftable`
//! repository falls back to `git --no-optional-locks symbolic-ref --short HEAD`.
//!
//! `--no-optional-locks` is deliberate and load-bearing (pi does the same):
//! plain `git symbolic-ref` may take `.git/index.lock`, and a background
//! invocation racing the user's own `git commit` fails it with
//! `fatal: Unable to create '.git/index.lock': File exists`.
//!
//! The spawn is **never** on the frame path: [`Footer`](super::Footer)'s cache
//! calls this once per cadence, on the tick, not per repaint. That is why the
//! cache exists (gh #389 gap 2) and why `symbolic_ref` is an injected hook:
//! the tests count it, and a frame loop must call it zero times.

use std::path::{Component, Path, PathBuf};

/// How far up the tree [`find_git_dir`] walks before giving up.
///
/// A repository nested deeper than this is not a shape anyone runs the agent
/// in, and the bound keeps a pathological tree (or a symlink loop, which
/// `Path::parent` can walk) from costing an unbounded number of `stat` calls
/// on the frame path's cache refresh.
pub const MAX_WALK_DEPTH: usize = 64;

/// Collapse `.` and `..` lexically, so a `gitdir: ../b` indirection or a
/// worktree's `commondir: ../..` becomes the path a human (and `git`, whose
/// `GIT_DIR` we pass) expects.
///
/// Lexical, not `canonicalize`: the git directory may be a symlink a user
/// relies on, `canonicalize` needs every component to exist, and following
/// links would defeat the "one indirection, never recursive" rule.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Keep a leading `..` on a relative path; nothing else here can
                // be popped, since these come from absolute or git-written
                // relative indirections.
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The two directories that make up a work tree's git state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// Where `HEAD` lives.
    pub git_dir: PathBuf,
    /// Where shared refs live. Equal to `git_dir`, except in a linked worktree
    /// or submodule, where it is the main repository's `.git`.
    pub common_dir: PathBuf,
}

/// Find the git directories for `start`, walking up to the repository root.
///
/// Handles both shapes of `.git`: a directory (an ordinary repository) and a
/// file holding `gitdir: <path>` (a linked worktree, or a submodule).
pub fn find_git_dir(start: &Path) -> Option<GitDirs> {
    for dir in start.ancestors().take(MAX_WALK_DEPTH) {
        // `None` covers both "no `.git` here" and "a `.git` that names
        // nothing usable": either way the repository may still be above us,
        // so the walk continues. Getting this wrong in the other direction
        // (stopping on a missing `.git`) breaks the subdirectory case, which
        // is the whole point of the walk.
        if let Some(git_dir) = resolve_dot_git(&dir.join(".git")) {
            return Some(with_common_dir(git_dir));
        }
    }
    None
}

/// The git directory a `.git` entry names, if it names one.
fn resolve_dot_git(dot_git: &Path) -> Option<PathBuf> {
    match std::fs::metadata(dot_git) {
        Ok(meta) if meta.is_dir() => Some(normalize(dot_git)),
        Ok(meta) if meta.is_file() => read_gitdir_file(dot_git),
        // A broken symlink, or a `.git` we may not stat.
        Ok(_) => None,
        Err(_) => None,
    }
}

/// Read a `.git` **file**: `gitdir: <path>`, relative to the file's own
/// directory when not absolute (which is how `git worktree add` writes it).
fn read_gitdir_file(dot_git: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(dot_git).ok()?;
    let target = content.lines().next()?.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let path = Path::new(target);
    Some(normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        dot_git.parent()?.join(path)
    }))
}

/// A linked worktree and a submodule keep their own `HEAD` but share refs with
/// the main repository, which is where `commondir` points. `.git` is then a
/// small directory holding `HEAD`, `commondir`, and an `index` — the refs
/// themselves live in the common dir.
fn with_common_dir(git_dir: PathBuf) -> GitDirs {
    let commondir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .map(|text| {
            let path = Path::new(&text);
            normalize(&if path.is_absolute() {
                path.to_path_buf()
            } else {
                // `commondir` is relative to the gitdir that holds it, and
                // `git worktree add` writes `../..`.
                git_dir.join(path)
            })
        });
    GitDirs {
        git_dir: git_dir.clone(),
        common_dir: commondir.unwrap_or(git_dir),
    }
}

/// What `HEAD` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadRef {
    /// `ref: refs/heads/<name>` — on a branch.
    Branch(String),
    /// `ref: <other>` — `refs/tags/v1`, `refs/remotes/origin/x`. Reported as
    /// the short spelling with a marker, so it never reads as a branch name.
    OtherRef(String),
    /// A raw object id: a detached HEAD. Carries the short form.
    Detached(String),
    /// `ref: refs/heads/.invalid`, which is what `reftable` repositories write.
    Reftable,
}

/// Parse the contents of a `HEAD` file.
///
/// Pure, so every shape is a table row rather than a filesystem fixture.
pub fn parse_head(head: &str) -> Option<HeadRef> {
    let head = head.trim();
    if head.is_empty() {
        return None;
    }
    if let Some(rest) = head.strip_prefix("ref:") {
        let target = rest.trim();
        if target.is_empty() {
            return None;
        }
        if target == "refs/heads/.invalid" {
            return Some(HeadRef::Reftable);
        }
        if let Some(branch) = target.strip_prefix("refs/heads/") {
            return Some(HeadRef::Branch(branch.to_string()));
        }
        return Some(HeadRef::OtherRef(
            target
                .strip_prefix("refs/tags/")
                .map(|tag| format!("tag:{tag}"))
                .unwrap_or_else(|| target.to_string()),
        ));
    }
    // A detached HEAD is a bare object id: 40 hex (SHA-1) or 64 (SHA-256).
    if head.chars().all(|c| c.is_ascii_hexdigit()) && head.len() >= 7 {
        return Some(HeadRef::Detached(head[..7].to_string()));
    }
    None
}

/// How a `reftable` repository's branch is resolved, since its `HEAD` file
/// carries no name. Injected so tests can count calls and so a build without a
/// `git` on `PATH` degrades instead of failing.
pub type SymbolicRef = std::sync::Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// Spawn `git --no-optional-locks symbolic-ref --short HEAD`.
///
/// Returns `None` on a detached HEAD (git exits 1) and on any spawn failure,
/// which is why a `reftable` repository in a detached state reports nothing
/// rather than guessing.
pub fn symbolic_ref_via_git(git_dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("symbolic-ref")
        .arg("--quiet")
        .arg("--short")
        .arg("HEAD")
        // `git` finds the repository from here, so the `-C` form would be
        // equivalent; passing the gitdir directly keeps the worktree case
        // (whose gitdir is not the work tree) honest.
        .env("GIT_DIR", git_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Resolve the branch label to display, or `None` when `start` is not in a
/// work tree.
///
/// The label is the branch name, `tag:<name>` / a short ref for a non-branch
/// ref, or the 7-character short object id on a detached HEAD.
pub fn resolve_branch(start: &Path, symbolic_ref: Option<&SymbolicRef>) -> Option<String> {
    let dirs = find_git_dir(start)?;
    let head = std::fs::read_to_string(dirs.git_dir.join("HEAD")).ok()?;
    match parse_head(&head)? {
        HeadRef::Branch(name) => Some(name),
        HeadRef::OtherRef(name) => Some(name),
        HeadRef::Detached(short) => Some(short),
        HeadRef::Reftable => symbolic_ref.and_then(|resolve| resolve(&dirs.git_dir)),
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
