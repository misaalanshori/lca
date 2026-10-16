//! GitHub #293: `run_logout` deleted only the bare `api_key`/`base_url`,
//! while the login path also writes `profile.<id>.api_key`,
//! `profile.<id>.base_url`, and the cached `models`/`model`/`preset`
//! settings. A user who ran `/logout` believed their keys were gone while
//! every preset profile secret stayed on disk and `auth status` kept
//! reporting ready.
//!
//! Verifies: end to end through the real `lca auth logout` binary — after
//! logout the credential store holds none of the profile keys or cached
//! settings, the fake secret appears nowhere, and `auth check` reports
//! `not_ready`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The CLI round trip is Unix-gated like its siblings in the e2e suite
// (the harness spawns a POSIX shell); the store-level inverse is pinned
// portably by `extensions/openai-compatible/tests/login.rs`.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const BARE: &str = "FAKESECRET-293-BARE";
    const DEEPSEEK: &str = "FAKESECRET-293-DEEPSEEK";

    /// The built `lca` binary, or `None` when this run did not build one.
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    /// A scratch home with a credential namespace written the way a
    /// login leaves it: bare keys plus a preset profile's keys and the
    /// cached settings.
    fn layout() -> (PathBuf, PathBuf) {
        let root = lca_testkit::scratch_path("regression-293-logout");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        let creds = home.join(".lca/credentials");
        std::fs::create_dir_all(&creds).expect("mkdir creds");
        std::fs::create_dir_all(&project).expect("mkdir project");
        let store = serde_json::json!({
            "api_key": BARE,
            "base_url": "https://api.deepseek.com/v1",
            "profile.deepseek.api_key": DEEPSEEK,
            "profile.deepseek.base_url": "https://api.deepseek.com/v1",
            "models": "deepseek-chat@deepseek",
            "model": "deepseek-chat",
            "preset": "deepseek",
        });
        std::fs::write(
            creds.join("openai-compatible.json"),
            serde_json::to_string_pretty(&store).expect("serialize"),
        )
        .expect("write store");
        (home, project)
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

    // Verifies: gh #293 AC1 + AC2 - logout purges the profile keys and
    // cached settings, leaks no secret, and `auth check` goes not_ready.
    #[test]
    fn logout_purges_profile_secrets_and_clears_readiness() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let (home, project) = layout();
        let (_, stderr, code) = run(
            &home,
            &project,
            &["auth", "logout", "--provider", "openai-compatible"],
        );
        assert_eq!(code, 0, "logout succeeds: {stderr}");

        // The store file holds none of the keys login wrote; the fake
        // secrets appear nowhere in it.
        let store_path = home.join(".lca/credentials/openai-compatible.json");
        let text = std::fs::read_to_string(&store_path).unwrap_or_default();
        for key in [
            "api_key",
            "base_url",
            "profile.deepseek.api_key",
            "profile.deepseek.base_url",
            "models",
            "model",
            "preset",
        ] {
            assert!(!text.contains(key), "the key {key} survived logout: {text}");
        }
        for secret in [BARE, DEEPSEEK] {
            assert!(
                !text.contains(secret),
                "the secret {secret} survived: {text}"
            );
        }

        // Readiness: no environment key, nothing left stored -> not_ready.
        let (out, _, check_code) = run(
            &home,
            &project,
            &["auth", "check", "--provider", "openai-compatible"],
        );
        assert_eq!(
            (check_code, out.trim()),
            (1, "not_ready"),
            "after logout the provider is unconfigured"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap_or(&home));
    }
}
