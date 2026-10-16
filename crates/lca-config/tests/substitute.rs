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

// ---------------------------------------------------------------------------
// gh #341: the credential refusal ran on the lexical path only, then
// `read_to_string` followed symlinks, so a workspace link into
// `~/.lca/credentials/` read the secret. The refusal now re-runs on the
// canonical path, and any canonical path inside `~/.lca` is refused
// wholesale (the SRDD state-directory exclusion).
// ---------------------------------------------------------------------------

// Verifies: gh #341 - the refusal is by WHERE IT RESOLVES, not "is a
// symlink": a link into the credential store, the sessions dir, or the
// grants store is refused; a link to a plain scratch sibling still
// substitutes (that is a legitimate use of a link).
#[cfg(unix)]
#[test]
fn a_symlink_into_the_credential_store_is_refused_by_resolved_location() {
    let env = lca_testkit::TestEnv::new("subst-341-symlink");
    // A scratch state dir shaped like the production `~/.lca`.
    let state = env.home().join(".lca");
    write(&state.join("credentials/anthropic.json"), "FAKESECRET-AK-1");
    write(&state.join("sessions/s1/log.jsonl"), "FAKESECRET-SESS-1");
    write(&state.join("grants.json"), "FAKESECRET-GRANT-1");

    let dir = scratch("subst-341-project");
    std::fs::create_dir_all(&dir).expect("mkdir");

    for (link, target) in [
        ("cred_link", state.join("credentials/anthropic.json")),
        ("sess_link", state.join("sessions/s1/log.jsonl")),
        ("grant_link", state.join("grants.json")),
    ] {
        let link_path = dir.join(link);
        let _ = std::fs::remove_file(&link_path);
        std::os::unix::fs::symlink(&target, &link_path).expect("symlink");
        let err = load_user(&dir, "user.toml", &format!("model = \"{{file:{link}}}\"\n"))
            .expect_err("symlink into the state dir must refuse");
        let text = err.to_string();
        assert!(text.contains("refused"), "{link}: {text}");
        // The secret value must never appear in the error.
        assert!(!text.contains("FAKESECRET"), "{link} leaked: {text}");
    }

    // A link to a plain scratch file OUTSIDE the state dir still works.
    write(&dir.join("sibling.txt"), "plain-value");
    let legit_link = dir.join("legit_link");
    let _ = std::fs::remove_file(&legit_link);
    std::os::unix::fs::symlink(dir.join("sibling.txt"), &legit_link).expect("symlink");
    let config = load_user(&dir, "user.toml", "model = \"{file:legit_link}\"\n").expect("load");
    assert_eq!(config.model(), Some("plain-value"));
}

// Verifies: gh #341 - any canonical path inside `~/.lca` refuses, not just
// the named stores (state dir excluded wholesale).
#[cfg(unix)]
#[test]
fn a_symlink_into_any_part_of_the_state_directory_is_refused() {
    let env = lca_testkit::TestEnv::new("subst-341-state");
    let state = env.home().join(".lca");
    // A file with no protected name component, under the state dir.
    write(&state.join("themes/secret.txt"), "FAKESECRET-THEME-1");

    let dir = scratch("subst-341-state-project");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let link_path = dir.join("theme_link");
    std::os::unix::fs::symlink(state.join("themes/secret.txt"), &link_path).expect("symlink");
    let err = load_user(&dir, "user.toml", "model = \"{file:theme_link}\"\n")
        .expect_err("any state-dir path refuses");
    let text = err.to_string();
    assert!(text.contains("refused"), "{text}");
    assert!(!text.contains("FAKESECRET"), "leaked: {text}");
}

// Verifies: gh #341 - a dangling symlink refuses with the missing-file
// shape (a broken link is not a readable file).
#[cfg(unix)]
#[test]
fn a_dangling_symlink_refuses_like_a_missing_file() {
    let dir = scratch("subst-341-dangling");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let link_path = dir.join("dangling");
    std::os::unix::fs::symlink(dir.join("nowhere.txt"), &link_path).expect("symlink");
    let err = load_user(&dir, "user.toml", "model = \"{file:dangling}\"\n")
        .expect_err("dangling link refuses");
    let text = err.to_string();
    assert!(
        text.contains("does not exist") || text.contains("cannot read"),
        "{text}"
    );
}

// Verifies: gh #341 (Windows): symlink creation needs privilege, so the
// resolved-location rule is pinned portably through the `\\?\`-safe
// canonical comparison using real scratch directories (a plain file
// inside `~/.lca` referenced directly, not via a link).
#[test]
fn a_plain_path_into_the_state_directory_is_refused() {
    let env = lca_testkit::TestEnv::new("subst-341-plain");
    let state = env.home().join(".lca");
    write(&state.join("ui.json"), "FAKESECRET-UI-1");

    let dir = scratch("subst-341-plain-project");
    std::fs::create_dir_all(&dir).expect("mkdir");
    // `{file:<absolute path into ~/.lca>}` with no lexical protected name.
    let target = state.join("ui.json");
    let err = load_user(
        &dir,
        "user.toml",
        &format!("model = \"{{file:{}}}\"\n", target.display()),
    )
    .expect_err("a state-dir path refuses however it is spelled");
    let text = err.to_string();
    assert!(text.contains("refused"), "{text}");
    assert!(!text.contains("FAKESECRET"), "leaked: {text}");
}
