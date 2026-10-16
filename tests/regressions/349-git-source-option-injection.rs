//! GitHub #349: `git clone` / `git checkout` passed their url/rev to git
//! with no `--` end-of-options separator, and `parse_git_source` never
//! rejected a value starting with `-`. A source like
//! `git+--upload-pack=<cmd>` reaches git as an *option*, and
//! `--upload-pack` runs a COMMAND for local transports — arbitrary
//! execution.
//!
//! Verifies: end to end through the real `lca ext install` binary — an
//! option-injected git source is refused with a validation error (exit
//! non-zero), and a sentinel file the injected `--upload-pack` would have
//! written does not exist (the command never ran).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The CLI round trip is Unix-gated like its siblings in the e2e suite
// (the harness spawns a POSIX shell); the store-level rule is pinned
// portably by `crates/lca-registry`'s `parse_git_source_*` and
// `git_source_refuses_an_option_injected_*` rows.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The built `lca` binary, or `None` when this run did not build one
    /// (the harness-absence rule, testing plan §14).
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    fn run(home: &Path, project: &Path, args: &[&str]) -> (String, String, i32) {
        let binary = binary().expect("the lca binary is built (harness-absence guard)");
        let out = Command::new(&binary)
            .args(args)
            .current_dir(project)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("LCA_UPDATE_CHECK", "false")
            .output()
            .expect("run lca");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    fn layout(tag: &str) -> (PathBuf, PathBuf) {
        let root = lca_testkit::scratch_path(&format!("regression-349-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&project).expect("mkdir project");
        (home, project)
    }

    // Verifies: gh #349 AC1 - the `--upload-pack` url is refused by
    // validation, never executed: exit non-zero and the sentinel the
    // command would have created does not exist.
    #[test]
    fn an_option_injected_git_source_is_refused_and_never_executes() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let (home, project) = layout("inject");
        let sentinel = project.join("PWNED-349");
        let source = format!("git+--upload-pack=touch {}", sentinel.display());
        let (stdout, stderr, code) = run(&home, &project, &["ext", "install", &source]);
        assert_ne!(code, 0, "the injected source is refused: {stdout}{stderr}");
        assert!(
            stderr.contains("invalid artifact") || stderr.contains("not a git source"),
            "the refusal is a validation error, not a fetch failure: {stderr}"
        );
        assert!(!sentinel.exists(), "the injected command never ran");
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    }

    // Verifies: gh #349 - the `#--force`-shaped rev is refused the same
    // way (exit non-zero, validation error), before any clone.
    #[test]
    fn an_option_injected_rev_is_refused() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let (home, project) = layout("rev");
        let (stdout, stderr, code) = run(
            &home,
            &project,
            &[
                "ext",
                "install",
                "git+https://example.invalid/r.git#--force",
            ],
        );
        assert_ne!(code, 0, "the injected rev is refused: {stdout}{stderr}");
        assert!(
            stderr.contains("invalid artifact") || stderr.contains("not a git source"),
            "the refusal is a validation error: {stderr}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    }
}
