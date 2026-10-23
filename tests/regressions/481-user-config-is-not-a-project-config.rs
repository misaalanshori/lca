//! gh #481 - from `$HOME` the user config is not a project config.
//!
//! # The bug
//!
//! `#463` established that the project root and the data home can be one
//! directory, and fixed two paths that assumed otherwise. This is the third, and
//! the only one that is a hard error. The loader computed the project file as
//! `<cwd>/.lca/config.toml` and the user file as `<data>/config.toml`; when
//! `cwd == $HOME` those are **the same file**, so the user's own documented
//! setting
//!
//! ```toml
//! [trust]
//! default_project = "always"
//! ```
//!
//! was read as a *project* file and refused by FR-CFG-7:
//!
//! ```text
//! error: trust.default_project in project file is invalid: can only be set in the user file
//! ```
//!
//! The refusal is correct - a project file that granted its own trust would
//! defeat FR-PERM-9 - and the **classification** was wrong.
//!
//! # The rule
//!
//! There is no separate project layer when the project file *is* the user file.
//! The comparison is canonical, because a lexical one passes on Linux and fails
//! on a symlinked `$HOME`, macOS's `/var` -> `/private/var`, and Windows's `\\?\`
//! verbatim prefix - the fourth cycle in which that lesson has come up (#436,
//! #389, #463).
//!
//! # What must not change
//!
//! A **real** project file that sets `trust.default_project` is still refused,
//! with the same message. That is [`a_real_project_file_still_cannot_grant_trust`]
//! below, and it is the row that keeps the tempting fix (relaxing the refusal)
//! from reopening FR-PERM-9.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};

use lca_config::{Config, LoadInput, MergeSource};

/// A scratch directory that is definitely gone before it is filled in.
fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// A data home holding the documented user config, and the project file the CLI
/// would compute for `cwd = <that home>`.
fn data_home(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let home = scratch(tag);
    std::fs::create_dir_all(home.join(".lca")).expect("mkdir");
    let user = home.join(".lca").join("config.toml");
    write(&user, "[trust]\ndefault_project = \"always\"\n");
    (
        home.clone(),
        user.clone(),
        home.join(".lca").join("config.toml"),
    )
}

// Verifies: gh #481 - **the defect's row.** Running from the data home loads, and
// the documented user setting takes effect. Before the fix this was the fatal
// error quoted in the module docs.
#[test]
fn a_user_config_in_its_own_home_loads_and_takes_effect() {
    let (home, user, project) = data_home("481-home");
    let config = Config::load(&LoadInput {
        user_file: Some(user.clone()),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect("the user's own file is not a project override");
    assert_eq!(
        config.trust_default_project(),
        "always",
        "the setting the user documented and wrote takes effect"
    );
    assert_eq!(
        config.source_of("trust.default_project"),
        MergeSource::UserFile,
        "and the layers report names the user file, because there is no project layer here"
    );
    // The same conclusion through the trust fallback the CLI uses at startup.
    assert_eq!(
        lca_config::Config::load(&LoadInput {
            user_file: Some(user),
            ..Default::default()
        })
        .expect("user-only peek")
        .trust_default_project(),
        "always"
    );
    let _ = std::fs::remove_dir_all(home);
}

// Verifies: gh #481 / FR-CFG-7 - **the pin.** A real project file that grants its
// own trust is still refused, with the message unchanged. The naive fix (relax the
// refusal) would reopen FR-PERM-9: a repository could make itself trusted by
// shipping a file.
#[test]
fn a_real_project_file_still_cannot_grant_trust() {
    let root = scratch("481-real");
    let home = root.join("home");
    let project_dir = root.join("project");
    let user = home.join(".lca").join("config.toml");
    let project = project_dir.join(".lca").join("config.toml");
    write(&user, "provider = \"openai-compatible\"\n");
    write(&project, "[trust]\ndefault_project = \"never\"\n");
    let err = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect_err("a project file granting its own trust defeats FR-PERM-9");
    let message = err.to_string();
    assert!(
        message.contains("trust.default_project") && message.contains("user file"),
        "the message is unchanged and names the user file: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #481 - **the canonical row.** The same file reached through two
// spellings is still one file, so a symlinked home does not reinstate the defect.
// This is the case a lexical comparison gets wrong, and the reason the helper
// canonicalizes.
#[cfg(unix)]
#[test]
fn a_symlinked_home_is_the_same_file_not_two() {
    let root = scratch("481-symlink");
    let real = root.join("real-home");
    std::fs::create_dir_all(real.join(".lca")).expect("mkdir");
    let user = real.join(".lca").join("config.toml");
    write(&user, "[trust]\ndefault_project = \"always\"\n");
    let link = root.join("linked-home");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    // `user` through the real spelling, `project` through the link: one file.
    let config = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: Some(link.join(".lca").join("config.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect("one file reached by two spellings is still one file");
    assert_eq!(config.trust_default_project(), "always");
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #481 - **a relocated data home is not a false collision.** When the
// home is somewhere else, `<cwd>/.lca/config.toml` and `<home>/.lca/config.toml`
// are genuinely two files, so the project layer must still participate. A fix that
// special-cased `$HOME` by string, or that compared only the last component, would
// quietly stop every project's settings from applying.
#[test]
fn a_relocated_home_keeps_the_project_layer_alive() {
    let root = scratch("481-relocated");
    let home = root.join("elsewhere");
    let project_dir = root.join("checkout");
    let user = home.join(".lca").join("config.toml");
    let project = project_dir.join(".lca").join("config.toml");
    write(&user, "ui.color = \"auto\"\n");
    write(&project, "ui.color = \"never\"\n");
    let config = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect("two distinct files, two layers");
    assert_eq!(
        config.ui_color(),
        lca_config::ColorMode::Never,
        "the project layer still wins when it really is a different file"
    );
    assert_eq!(config.source_of("ui.color"), MergeSource::ProjectFile);
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #481 - **the coincidence is about files, not about `$HOME`.** The
// fix must not be written as "is this path under $HOME": a project whose data home
// happens to sit inside it is two genuinely different files, and its project layer
// must still apply. Pinned because the tempting shortcut (`cwd == home`) passes the
// defect row and breaks this one.
#[test]
fn a_project_containing_the_data_home_still_has_two_layers() {
    let root = scratch("481-parent");
    // A checkout, with the agent's data home nested inside it - a real layout
    // (a container image, a CI workspace), not a contrived one.
    let project_dir = root.join("checkout");
    let home = project_dir.join("agent-home");
    let user = home.join(".lca").join("config.toml");
    let project = project_dir.join(".lca").join("config.toml");
    write(&user, "ui.color = \"auto\"\n");
    write(&project, "ui.color = \"never\"\n");
    let config = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect("two distinct files");
    assert_eq!(
        config.ui_color(),
        lca_config::ColorMode::Never,
        "the project layer applies: these are different files even though one contains the other"
    );
    assert_eq!(config.source_of("ui.color"), MergeSource::ProjectFile);
    let _ = std::fs::remove_dir_all(root);
}

// Verifies: gh #481 - **an absent project file is still absent.** The coincidence
// check must not turn "no project file" into an error or into a phantom layer.
#[test]
fn no_project_file_is_still_no_project_layer() {
    let (home, user, _project) = data_home("481-absent");
    let config = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: None,
        trusted: true,
        ..Default::default()
    })
    .expect("no project file, nothing to classify");
    assert_eq!(config.trust_default_project(), "always");
    assert_eq!(
        config.source_of("trust.default_project"),
        MergeSource::UserFile
    );
    let _ = std::fs::remove_dir_all(home);
}

// Verifies: gh #481 - **the untrusted case is unchanged.** FR-PERM-9 keeps an
// untrusted project file out of the merge whether or not it is also the user file;
// the coincidence check is about classification, not about trust.
#[test]
fn an_untrusted_project_file_stays_inert_even_when_it_is_the_user_file() {
    let (home, user, project) = data_home("481-untrusted");
    let config = Config::load(&LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: false,
        ..Default::default()
    })
    .expect("loads; the project layer is inert either way");
    assert_eq!(
        config.trust_default_project(),
        "always",
        "the user file is read on its own merits, trusted or not"
    );
    let _ = std::fs::remove_dir_all(home);
}

// ------------------------------------------------------------------
// gh #481, the sweep's **fourth** path: `mcp.json`.
//
// Not a config-loader bug, so not the brief's headline - but the same root cause,
// found by the sweep the brief asked for. `McpManager::plan` reads
// `<data>/mcp.json` and, when trusted, `<cwd>/.lca/mcp.json`; from `$HOME` those
// are one file, so the user's servers were loaded twice, the second time as
// project *overrides of themselves*. Two consequences, one cosmetic and one not:
// `lca mcp list` labelled them source `project`, and an override source routes a
// later manager edit through `write_full_entry` to the user's own file instead of
// patching it.
// ------------------------------------------------------------------

// Verifies: gh #481 - **the guard the CLI applies.** `plan` itself is
// policy-free: told `trusted`, it reads both paths, which is right for a caller
// with two files and wrong when they are one. The decision lives at the CLI seam
// (`mcp_cli::run` and `registry::assemble`), so this row pins the predicate both
// of them use rather than `plan`'s own behaviour.
#[test]
fn the_two_mcp_paths_are_recognised_as_one_file_from_home() {
    let home = scratch("481-mcp-one-file");
    std::fs::create_dir_all(home.join(".lca")).expect("mkdir");
    let data = home.join(".lca");
    write(
        &data.join("mcp.json"),
        r#"{"mcpServers": {"echo": {"command": "/bin/cat"}}}"#,
    );
    // The CLI's own comparison, against the CLI's own path construction.
    assert_eq!(
        lca_mcp::config::project_config_path(&home),
        lca_mcp::config::user_config_path(&data),
        "from `$HOME` the two MCP paths are literally the same file"
    );
    // Told there is no project layer (which is what the guard makes it pass),
    // the user's server keeps `User`, and nothing is skipped.
    let (entries, _grants, warnings, user_path, project_path) =
        lca_mcp::manager::McpManager::plan(&data, &home, false);
    let entry = entries
        .iter()
        .find(|entry| entry.name == "echo")
        .expect("the user's server loads");
    assert_eq!(
        entry.source,
        lca_mcp::config::ServerSource::User,
        "read once, as the user's own - not a second time as a project override of itself"
    );
    assert!(warnings.is_empty(), "nothing skipped: {warnings:?}");
    assert_eq!(
        project_path, None,
        "no separate project layer to persist into"
    );
    assert!(user_path.ends_with("mcp.json"));
    let _ = std::fs::remove_dir_all(home);
}

// Verifies: gh #481 - **a genuinely different project `mcp.json` still applies.**
// The guard must not disable project-local MCP configuration, which is the whole
// point of the trusted-project layer.
#[test]
fn a_real_project_mcp_file_still_applies() {
    let root = scratch("481-mcp-distinct");
    let data = root.join("data");
    let project = root.join("checkout");
    write(
        &data.join("mcp.json"),
        r#"{"mcpServers": {"echo": {"command": "/bin/cat"}}}"#,
    );
    write(
        &project.join(".lca").join("mcp.json"),
        r#"{"mcpServers": {"echo": {"enabled": false}}}"#,
    );
    let (entries, _grants, _warnings, _user_path, project_path) =
        lca_mcp::manager::McpManager::plan(&data, &project, true);
    let entry = entries
        .iter()
        .find(|e| e.name == "echo")
        .expect("one entry");
    assert_eq!(
        entry.source,
        lca_mcp::config::ServerSource::Project,
        "a different file is a real project override"
    );
    assert!(!entry.enabled, "and its `enabled` took effect");
    assert_eq!(project_path, Some(project.join(".lca").join("mcp.json")));
    let _ = std::fs::remove_dir_all(root);
}
