//! GitHub #341: `{file:path}` substitution refused credential stores on
//! the LEXICAL path only, then `read_to_string` followed symlinks. A
//! workspace link `key-link -> ~/.lca/credentials/anthropic.json` has no
//! protected name component, so the secret was read into a config value —
//! the untrusted-project attacker reading plaintext API keys and refresh
//! tokens at config-load time.
//!
//! The refusal now re-runs on the canonical (resolved) path, and any
//! canonical path inside the agent state directory `~/.lca` is refused
//! wholesale.
//!
//! Verifies: end to end through the real `lca config` binary — a project
//! config referencing a symlink into the credential store is refused
//! loudly with a non-zero exit, and the fake secret appears nowhere in
//! stdout, stderr, or the session log.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The symlink escape is unix-only (Windows symlink creation needs
// Developer Mode or elevation). The happy-path control is gated with it
// so the file is coherent on Windows; the portable shape of the rule is
// pinned by `crates/lca-config`'s `a_plain_path_into_the_state_directory_is_refused`.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const FAKE_SECRET: &str = "FAKESECRET-341-NEVER-VISIBLE";

    /// The built `lca` binary, if this run also produced one. `CARGO_BIN_EXE_lca`
    /// is only set for the package that owns the bin target, so look in the
    /// shared target directory (the harness-absence rule, testing plan §14):
    /// CI's `shell` group builds `lca-cli` and `lca-regressions` together; a
    /// run of this package alone skips with a named reason.
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    /// Build the scratch layout: a home whose `.lca/` holds a fake
    /// credential, and a project whose `.lca/config.toml` references it
    /// through a symlink.
    fn layout() -> (PathBuf, PathBuf) {
        let root = lca_testkit::scratch_path("regression-341-subst-symlink");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(home.join(".lca/credentials")).expect("mkdir home");
        std::fs::create_dir_all(project.join(".lca")).expect("mkdir project");
        std::fs::write(
            home.join(".lca/credentials/anthropic.json"),
            format!("{{\"api_key\":\"{FAKE_SECRET}\"}}"),
        )
        .expect("write secret");
        // The link sits beside the config, so the config's own directory is
        // the resolution root (the realistic shape).
        std::os::unix::fs::symlink(
            home.join(".lca/credentials/anthropic.json"),
            project.join(".lca/key-link"),
        )
        .expect("symlink");
        std::fs::write(
            project.join(".lca/config.toml"),
            "model = \"{file:./key-link}\"\n",
        )
        .expect("write config");
        (home, project)
    }

    /// Run `lca -a config` with the scratch home, returning (stdout, stderr,
    /// exit).
    fn run_config(home: &Path, project: &Path) -> (String, String, i32) {
        let binary = binary().expect("the lca binary is built (harness-absence guard)");
        let out = Command::new(&binary)
            .args(["-a", "config"])
            .current_dir(project)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("LCA_UPDATE_CHECK", "false")
            .output()
            .expect("run lca config");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    // Verifies: gh #341 - a `{file:}` through a workspace symlink into
    // `~/.lca/credentials/` is refused loudly, non-zero, and the secret is
    // never emitted.
    #[test]
    fn a_config_symlink_into_the_credential_store_is_refused_and_leaks_nothing() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let (home, project) = layout();
        let (stdout, stderr, code) = run_config(&home, &project);
        assert_ne!(code, 0, "a refused substitution is a non-zero load");
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("refused"),
            "the refusal is named on the load:\n{combined}"
        );
        assert!(
            !combined.contains(FAKE_SECRET),
            "the credential value leaked into output:\n{combined}"
        );
        // The session log, if any was written, must not carry it either.
        let logs = home.join(".lca/sessions");
        if logs.is_dir() {
            let mut stack = vec![logs];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if let Ok(text) = std::fs::read_to_string(&path) {
                        assert!(
                            !text.contains(FAKE_SECRET),
                            "the credential value leaked into {}",
                            path.display()
                        );
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    }

    // Verifies: gh #341 - the resolved-location rule does not break the
    // feature: a plain in-project file still substitutes (and masks).
    #[test]
    fn a_legit_file_still_substitutes() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let (home, project) = layout();
        std::fs::write(project.join(".lca/plain.txt"), "supersecret-not-a-cred\n")
            .expect("write plain");
        std::fs::write(
            project.join(".lca/config.toml"),
            "model = \"{file:./plain.txt}\"\n",
        )
        .expect("write config");
        let (stdout, stderr, code) = run_config(&home, &project);
        assert_eq!(code, 0, "a plain file loads: {stderr}");
        assert!(
            stdout.contains("model = ********"),
            "the substituted value is masked in the dump:\n{stdout}"
        );
        assert!(
            !stdout.contains("supersecret-not-a-cred")
                && !stderr.contains("supersecret-not-a-cred"),
            "a substituted secret never prints in the clear:\n{stdout}{stderr}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    }
}
