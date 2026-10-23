//! gh #481 - from `$HOME` the user config is not a project config.
//!
//! The loader-level rows (the regression suite `tests/regressions/481-*.rs` covers
//! the same ground end to end, and `crates/lca-cli/tests/e2e_home_config.rs` drives
//! the real binary); these live beside `merge.rs`'s other merge rows because they
//! are merge rows - same helpers, same precedence ladder - and `merge.rs` was at
//! the 1,200-line ceiling.
//!
//! The refusal FR-CFG-7 makes is correct. The **classification** was not: from
//! `$HOME` the project file and the user file are one file, so the user's own
//! documented setting was read as a project override of itself and refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::Path;

use lca_config::Config;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

// ------------------------------------------------------------------
// gh #481: from `$HOME` the user config must not be read as a project config.
//
// #463 found the project root and the data home can be the same place, and fixed
// two paths that assumed otherwise. This is the third, and the only one that is a
// hard error: `<cwd>/.lca/config.toml` and `~/.lca/config.toml` are the same file
// when `cwd == $HOME`, so the user's own documented setting was classified as a
// project file and refused by FR-CFG-7.
//
// The refusal is right. The classification is not.
// ------------------------------------------------------------------

/// A pair of paths in one scratch directory: a data home and a project that *is*
/// that same directory, spelled the way the loader spells them.
fn coincident(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = scratch(tag);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".lca")).expect("mkdir");
    let user = home.join(".lca").join("config.toml");
    write(&user, "[trust]\ndefault_project = \"always\"\n");
    // The project file, computed the way the CLI computes it: `<cwd>/.lca/...`
    // with `cwd` being the data home itself.
    let project = home.join(".lca").join("config.toml");
    (user, project)
}

// Verifies: gh #481 - **the defect's row.** When the project file and the user
// file are the same file, the user's setting must be honoured, not refused as a
// project override.
#[test]
fn a_user_config_read_from_its_own_home_is_not_a_project_file() {
    let (user, project) = coincident("481-coincident");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(user.clone()),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect("the user's own file is not a project override");
    assert_eq!(
        config.trust_default_project(),
        "always",
        "the documented user setting takes effect"
    );
    assert_eq!(
        config.source_of("trust.default_project"),
        lca_config::MergeSource::UserFile,
        "and the layers report says it came from the user file, because there is no project layer"
    );
}

// Verifies: gh #481 / FR-CFG-7 - **the pin.** A *real* project file that sets
// `trust.default_project` is still refused, with the same message. The tempting
// fix here is relaxing the refusal, which would reopen FR-PERM-9.
#[test]
fn a_real_project_file_still_cannot_set_the_trust_default() {
    let dir = scratch("481-real-project");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project").join(".lca")).expect("mkdir");
    let user = dir.join("home").join(".lca").join("config.toml");
    let project = dir.join("project").join(".lca").join("config.toml");
    write(&user, "provider = \"openai-compatible\"\n");
    write(&project, "[trust]\ndefault_project = \"always\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect_err("a project file granting its own trust defeats FR-PERM-9");
    assert!(
        err.to_string().contains("user file"),
        "the message still points at the user file: {err}"
    );
}

// Verifies: gh #481 - **the canonical row.** The two paths coincide through a
// symlink, so a lexical comparison would call them different and reinstate the
// defect on that spelling alone. This is the fourth cycle in which a raw path
// comparison has passed on Linux and failed elsewhere (#436, #389, #463), so it
// is a row rather than an intention.
#[cfg(unix)]
#[test]
fn a_symlinked_home_is_still_the_same_file() {
    let root = scratch("481-symlink");
    let _ = std::fs::remove_dir_all(&root);
    let real = root.join("real");
    std::fs::create_dir_all(real.join(".lca")).expect("mkdir");
    write(
        &real.join(".lca").join("config.toml"),
        "[trust]\ndefault_project = \"always\"\n",
    );
    let link = root.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    // The user path goes through the link, the project path through the real
    // spelling - the same file, two spellings.
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(link.join(".lca").join("config.toml")),
        project_file: Some(real.join(".lca").join("config.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect("one file reached by two spellings is still one file");
    assert_eq!(config.trust_default_project(), "always");
}

// Verifies: gh #481 - **distinct files are still two layers.** The coincidence
// check must not collapse a genuine project override into the user file, which
// would silently stop every project's settings from applying.
#[test]
fn two_different_files_are_still_two_layers() {
    let dir = scratch("481-distinct");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home").join(".lca")).expect("mkdir");
    std::fs::create_dir_all(dir.join("proj").join(".lca")).expect("mkdir");
    let user = dir.join("home").join(".lca").join("config.toml");
    let project = dir.join("proj").join(".lca").join("config.toml");
    write(&user, "ui.color = \"auto\"\n");
    write(&project, "ui.color = \"never\"\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(user),
        project_file: Some(project),
        trusted: true,
        ..Default::default()
    })
    .expect("two files, two layers");
    assert_eq!(
        config.ui_color(),
        lca_config::ColorMode::Never,
        "the project layer still wins when it is a different file"
    );
    assert_eq!(
        config.source_of("ui.color"),
        lca_config::MergeSource::ProjectFile
    );
}
