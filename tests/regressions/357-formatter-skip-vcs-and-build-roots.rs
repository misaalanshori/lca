//! GitHub #357: `/formatter run` with no argument walked the whole workspace
//! with no exclusions, then handed every extension-matching path to
//! `rustfmt`/`prettier`/`black` in `--write` mode.
//!
//! Two harms, both shipped:
//!
//! 1. **Generated sources were rewritten.** `target/**/*.rs` — build-script
//!    output — matched `select_formatter`, so a bare run reformatted files the
//!    project regenerates. At best that is rebuild churn on every run; at
//!    worst it is divergent generated code, because the next build overwrites
//!    the reformat and the two disagree.
//! 2. **The walk paid for traversal it could never use.** `.git/objects` on a
//!    real repository is tens of thousands of files, each stat'ed
//!    individually through the `fs` capability boundary, and none of them can
//!    ever be a formatting target.
//!
//! The fix is one shared skip rule (`lca_tools::SKIP_DIRS`, the constant the
//! `glob`/`grep` walker already used) consulted at descent time, plus a
//! refusal for an explicitly named path under one of those roots.
//!
//! Verifies: gh #357 acceptance 1 and 3 — a bare walk over a fabricated tree
//! containing a VCS root, a build root, and a package root reaches none of
//! them, while a nested non-skip directory is still formatted.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::cell::RefCell;

use formatter::{FormatterState, FsView as _, HookOutcome, walk_workspace};

/// A fabricated workspace: a flat list of file paths, answering the three
/// calls the walker makes (`list`, `stat`, `exists`).
///
/// `stat_ed` records every path the walker stat'ed. Asserting that no path
/// under a skip root appears there is what pins the *traversal* half of the
/// defect — the half a post-filter would leave in place, since not descending
/// is the entire saving.
struct FakeTree {
    files: Vec<String>,
    stat_ed: RefCell<Vec<String>>,
}

impl FakeTree {
    fn new(files: &[&str]) -> FakeTree {
        FakeTree {
            files: files.iter().map(|file| file.to_string()).collect(),
            stat_ed: RefCell::new(Vec::new()),
        }
    }
}

impl formatter::FsView for FakeTree {
    fn exists(&self, path: &str) -> bool {
        self.files.iter().any(|file| file == path)
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut names: Vec<String> = Vec::new();
        for path in &self.files {
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            let Some(name) = rest.split('/').next() else {
                continue;
            };
            if name.is_empty() || names.iter().any(|seen| seen == name) {
                continue;
            }
            names.push(name.to_string());
        }
        names
    }

    fn stat(&self, path: &str) -> Option<(bool, u64)> {
        self.stat_ed.borrow_mut().push(path.to_string());
        if self.files.iter().any(|file| file == path) {
            return Some((false, 0));
        }
        let prefix = format!("{path}/");
        if self.files.iter().any(|file| file.starts_with(&prefix)) {
            return Some((true, 0));
        }
        None
    }
}

/// The tree the issue describes: a VCS store, a build-output tree, a package
/// directory, and one real source file.
fn shipped_defect_tree() -> FakeTree {
    FakeTree::new(&[
        ".git/objects/ab/cdef0123",
        ".git/objects/ef/456789",
        ".git/HEAD",
        ".git/config",
        "target/out/generated.rs",
        "target/debug/build/lca-core-1234/out/gen.rs",
        "target/release/deps/foo.rs",
        "node_modules/some-pkg/index.js",
        "node_modules/some-pkg/dist/bundle.js",
        "src/main.rs",
    ])
}

// Verifies: gh #357 acceptance 1 - the shipped defect. Before the fix this
// walk returned the four `target/**` and `node_modules/**` files alongside
// `src/main.rs`, and a bare `run` formatted them.
#[test]
fn a_bare_walk_returns_only_source_and_never_build_output() {
    let tree = shipped_defect_tree();
    let walked = walk_workspace(&tree, "");
    let formattable: Vec<String> = walked
        .iter()
        .filter(|path| formatter::select_formatter(path, |probe| tree.exists(probe)).is_some())
        .cloned()
        .collect();
    assert_eq!(
        formattable,
        vec!["src/main.rs".to_string()],
        "only source is formattable; the tree carries five generated files"
    );
}

// Verifies: gh #357 - the traversal half. `.git/objects` is never stat'ed, so
// the capability-boundary cost the issue names is gone rather than filtered
// away afterwards.
#[test]
fn the_walk_never_stats_inside_a_skip_root() {
    let tree = shipped_defect_tree();
    let _ = walk_workspace(&tree, "");
    let stat_ed = tree.stat_ed.borrow().clone();
    // One stat per skip root is unavoidable and cheap: the walker has to learn
    // that `.git` is a directory before it can decide not to enter. The
    // defect was stat'ing every entry *inside*, so that is what this pins.
    for path in &stat_ed {
        for root in lca_tools::SKIP_DIRS {
            assert!(
                !path.starts_with(&format!("{root}/")),
                "the walk stat'ed `{path}`, which is inside the skip root `{root}/`"
            );
        }
    }
    assert!(
        stat_ed.iter().any(|path| path == "src/main.rs"),
        "and it still reached the source file: {stat_ed:?}"
    );
}

// Verifies: gh #357 acceptance 1 - a bare `run` through the command surface
// spawns no formatter for generated content. This is the row that would have
// gone red on the shipped binary: it drives the same path `/formatter run`
// takes.
#[test]
fn a_bare_run_command_spawns_no_formatter_for_generated_content() {
    let tree = shipped_defect_tree();
    let state = FormatterState::new();

    // A process backend that records what it was asked to spawn.
    struct Recorder {
        spawned: RefCell<Vec<String>>,
    }
    impl formatter::ProcessView for Recorder {
        fn spawn(&self, program: &str, args: &[String]) -> Result<u32, String> {
            let last = args.last().cloned().unwrap_or_default();
            self.spawned.borrow_mut().push(format!("{program} {last}"));
            Ok(1)
        }
        fn wait(&self, _handle: u32) -> Result<i32, String> {
            Ok(0)
        }
        fn read_stderr(&self, _handle: u32) -> String {
            String::new()
        }
    }
    let process = Recorder {
        spawned: RefCell::new(Vec::new()),
    };

    let report = state.handle_command(&process, &tree, "run");
    let spawned = process.spawned.borrow().clone();
    assert_eq!(
        spawned,
        vec!["rustfmt src/main.rs".to_string()],
        "exactly one formatter run, over the one source file: {spawned:?}\nreport: {report}"
    );
    for path in &spawned {
        for root in lca_tools::SKIP_DIRS {
            assert!(
                !path.contains(&format!("{root}/")),
                "a formatter was spawned on `{path}` under `{root}/`"
            );
        }
    }
}

// Verifies: gh #357 acceptance 2 - naming a generated path explicitly is
// refused with a reason rather than formatted.
#[test]
fn an_explicitly_named_generated_path_is_refused_and_not_spawned() {
    let tree = shipped_defect_tree();
    let state = FormatterState::new();

    struct Never {
        spawned: RefCell<usize>,
    }
    impl formatter::ProcessView for Never {
        fn spawn(&self, _program: &str, _args: &[String]) -> Result<u32, String> {
            *self.spawned.borrow_mut() += 1;
            Ok(1)
        }
        fn wait(&self, _handle: u32) -> Result<i32, String> {
            Ok(0)
        }
        fn read_stderr(&self, _handle: u32) -> String {
            String::new()
        }
    }
    let process = Never {
        spawned: RefCell::new(0),
    };

    let report = state.handle_command(&process, &tree, "run target/out/generated.rs");
    assert!(
        report.contains("refused"),
        "the refusal is stated in the report: {report}"
    );
    assert!(report.contains("target"), "and names the root: {report}");
    assert_eq!(
        *process.spawned.borrow(),
        0,
        "no formatter ran on a build-root path: {report}"
    );

    // The same gate covers the hook path: a model that edits a generated
    // file must not trigger a reformat of build output.
    let outcome = state.handle_result(
        &process,
        &tree,
        "write",
        r#"{"path": "target/debug/build/lca-core-1234/out/gen.rs"}"#,
        true,
    );
    assert!(
        matches!(outcome, HookOutcome::Refused(_)),
        "the hook refuses build output: {outcome:?}"
    );
    assert_eq!(*process.spawned.borrow(), 0);
}

// Verifies: gh #357 - the positive control. A deep, non-skip nested directory
// still formats, so the fix cannot pass the rows above by formatting nothing.
#[test]
fn a_deep_non_skip_directory_still_formats() {
    let tree = FakeTree::new(&[
        ".git/HEAD",
        "target/out/gen.rs",
        "src/deep/nested/module/main.rs",
    ]);
    let walked = walk_workspace(&tree, "");
    assert_eq!(
        walked,
        vec!["src/deep/nested/module/main.rs".to_string()],
        "the nested source file is reached and the skip roots are not"
    );
}

// Verifies: gh #357 - one shared constant, not a second copy. The formatter
// imports `lca_tools::SKIP_DIRS`, the same list the `glob`/`grep` walker uses,
// so the two cannot drift.
#[test]
fn the_skip_rule_is_the_shared_one() {
    // The three roots the issue names, and the ones the walker already used.
    for required in [".git", "target", "node_modules"] {
        assert!(
            lca_tools::SKIP_DIRS.contains(&required),
            "`{required}` is in the shared skip list"
        );
        assert!(
            formatter::is_skipped_path(&format!("{required}/x.rs")),
            "the formatter honors `{required}`"
        );
    }
    // And the formatter's answer agrees with the constant for every entry,
    // which is what "one rule" means in practice.
    for root in lca_tools::SKIP_DIRS {
        assert_eq!(
            formatter::skip_root(&format!("a/b/{root}/c.rs")),
            Some(*root),
            "`{root}` is matched at depth by the shared list"
        );
    }
}
