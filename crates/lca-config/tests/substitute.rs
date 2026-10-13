//! Config variable substitution tests (gh #218).
//!
//! `{env:VAR}` and `{file:path}` in TOML string values resolve at load,
//! with loud failures and credential-path refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::Path;

use lca_config::{Config, LoadInput};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn load_user(dir: &Path, name: &str, text: &str) -> Result<Config, lca_config::ConfigError> {
    let path = dir.join(name);
    write(&path, text);
    Config::load(&LoadInput {
        user_file: Some(path),
        ..Default::default()
    })
}

// Verifies: gh #218 (`{env:VAR}` expands process environment variables).
#[test]
fn env_tokens_expand() {
    // SAFETY: unique var name, no other thread reads it.
    unsafe { std::env::set_var("LCA_TEST_SUBST_PROXY", "https://proxy.example:8080") };
    let dir = scratch("subst-env");
    let config = load_user(
        &dir,
        "user.toml",
        "shell.command_prefix = \"{env:LCA_TEST_SUBST_PROXY}\"\n",
    )
    .expect("load");
    assert_eq!(
        config.shell_command_prefix(),
        Some("https://proxy.example:8080")
    );
    assert!(config.substituted("shell.command_prefix"));
    // SAFETY: same unique var.
    unsafe { std::env::remove_var("LCA_TEST_SUBST_PROXY") };
}

// Verifies: gh #218 (an unset variable substitutes empty, OpenCode's rule).
#[test]
fn unset_env_substitutes_empty() {
    // SAFETY: unique var name, no other thread reads it.
    unsafe { std::env::remove_var("LCA_TEST_SUBST_DEFINITELY_UNSET") };
    let dir = scratch("subst-env-empty");
    let config = load_user(
        &dir,
        "user.toml",
        "shell.command_prefix = \"pre-{env:LCA_TEST_SUBST_DEFINITELY_UNSET}-post\"\n",
    )
    .expect("load");
    assert_eq!(config.shell_command_prefix(), Some("pre--post"));
}

// Verifies: gh #218 (`{file:path}` reads relative to the config file and
// trims trailing whitespace).
#[test]
fn file_tokens_read_relative_and_trim() {
    let dir = scratch("subst-file");
    write(&dir.join("token.txt"), "s3cr3t\n\n");
    let config = load_user(&dir, "user.toml", "model = \"{file:token.txt}\"\n").expect("load");
    assert_eq!(config.model(), Some("s3cr3t"));
    assert!(config.substituted("model"));
}

// Verifies: gh #218 (missing files fail loudly, naming the token).
#[test]
fn missing_files_fail_loudly() {
    let dir = scratch("subst-missing");
    let err = load_user(&dir, "user.toml", "model = \"{file:nope.txt}\"\n").expect_err("missing");
    let text = err.to_string();
    assert!(text.contains("{file:nope.txt}"), "{text}");
    assert!(text.contains("does not exist"), "{text}");
}

// Verifies: gh #218 (credential stores are never readable through
// substitution, however the path is spelled).
#[test]
fn credential_paths_are_refused() {
    let dir = scratch("subst-creds");
    for token in [
        "{file:credentials/api.json}",
        "{file:sub/../credentials/api.json}",
        "{file:sessions/abc.jsonl}",
        "{file:grants/x.json}",
    ] {
        let err = load_user(&dir, "user.toml", &format!("model = \"{token}\"\n"))
            .expect_err("refused {token}");
        assert!(err.to_string().contains("refused"), "{}: {err}", token);
    }
}

// Verifies: gh #218 (there is no execution hook: `!command` is inert text).
#[test]
fn bang_commands_are_inert_text() {
    let dir = scratch("subst-bang");
    let config = load_user(&dir, "user.toml", "model = \"{!echo pwned}\"\n").expect("load");
    assert_eq!(config.model(), Some("{!echo pwned}"));
    assert!(!config.substituted("model"));
}
