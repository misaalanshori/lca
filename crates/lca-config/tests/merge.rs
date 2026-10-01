//! Configuration merge tests.
//!
//! Precedence per FR-CFG-1: flags > environment > project file > user file >
//! built-ins. The project file only participates once the user trusts the
//! project (FR-PERM-9).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::Path;

use lca_config::{ColorMode, Config, MergeSource};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

// Verifies: FR-CFG-1 (built-in defaults are the lowest layer)
#[test]
fn defaults_match_the_documented_key_reference() {
    let config = Config::defaults();
    assert_eq!(config.provider(), "openai-compatible");
    assert!(
        config.model().is_none(),
        "model defaults to the provider's own default"
    );
    assert_eq!(config.compaction_threshold(), 0.8);
    assert_eq!(config.provider_retry_limit(), 3);
    assert_eq!(config.tool_timeout_seconds(), 120);
    assert_eq!(config.tool_result_limit_bytes(), 65536);
    assert_eq!(config.tool_max_iterations(), 100);
    assert_eq!(config.cache_noise_floor_tokens(), 1024);
    assert_eq!(config.extensions_log_limit_bytes(), 4096);
    assert_eq!(config.ui_color(), ColorMode::Auto);
    assert!(config.thinking().is_none(), "thinking defaults to unset");
    assert!(config.permissions_proposals().is_empty());
}

// Verifies: R1 (`thinking` is a documented key with pi's level vocabulary)
#[test]
fn thinking_merges_and_refuses_unknown_levels() {
    let dir = scratch("thinking");
    write(&dir.join("user.toml"), "thinking = \"high\"\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.thinking(), Some("high"));

    write(&dir.join("user.toml"), "thinking = \"hihg\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("an unknown level is refused at load");
    assert!(err.to_string().contains("hihg"), "{err}");
}

// Verifies: ADR-0041 - `shell.tool` and `shell.path` are documented keys;
// the tool vocabulary is closed, and a bad value is refused at load rather
// than silently ignored.
#[test]
fn shell_selection_merges_and_refuses_unknown_tools() {
    let dir = scratch("shell");
    write(
        &dir.join("user.toml"),
        "shell.tool = \"pwsh\"\nshell.path = \"C:\\\\tools\\\\pwsh.exe\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.shell_tool(), Some("pwsh"));
    assert_eq!(config.shell_path(), Some("C:\\tools\\pwsh.exe"));

    write(&dir.join("user.toml"), "shell.tool = \"csh\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("an unknown shell.tool is refused at load");
    assert!(err.to_string().contains("csh"), "{err}");

    // Defaults: auto, and no explicit path.
    let config = Config::defaults();
    assert_eq!(config.shell_tool(), None, "`auto` is the unset default");
    assert_eq!(config.shell_path(), None);
}

// Verifies: ADR-0042 - `permissions.mode` is a documented key with a
// closed vocabulary; a bad value is refused at load, and the default is ask.
#[test]
fn permissions_mode_merges_and_refuses_unknown_values() {
    let dir = scratch("perm-mode");
    write(&dir.join("user.toml"), "permissions.mode = \"yolo\"\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.permissions_mode(), Some("yolo"));

    write(&dir.join("user.toml"), "permissions.mode = \"reckless\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("an unknown mode is refused at load");
    assert!(err.to_string().contains("reckless"), "{err}");

    assert_eq!(
        Config::defaults().permissions_mode(),
        None,
        "ask is the default"
    );
}

// Verifies: R6 - `ui.thinking` is a separate key from `thinking`'s
// effort level, with a closed vocabulary and a snippet default.
#[test]
fn thinking_visibility_merges_beside_the_effort_level() {
    let dir = scratch("thinking-vis");
    write(
        &dir.join("user.toml"),
        "thinking = \"high\"\nui.thinking = \"full\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.thinking(), Some("high"), "the effort level");
    assert_eq!(config.thinking_visibility(), Some("full"), "the visibility");

    write(&dir.join("user.toml"), "ui.thinking = \"verbose\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("an unknown visibility is refused");
    assert!(err.to_string().contains("verbose"), "{err}");
    assert_eq!(Config::defaults().thinking_visibility(), None, "snippet");
}

// Verifies: FR-CFG-2 (every resolved value names the source that set it)
#[test]
fn every_resolved_value_carries_its_source() {
    let dir = scratch("sources");
    write(&dir.join("user.toml"), "provider = \"acme\"\n");
    let config = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: None,
        trusted: true,
        user_file: Some(dir.join("user.toml")),
        headless: false,
    })
    .expect("load");
    let resolved: Vec<_> = config.resolved().collect();
    assert!(
        resolved
            .iter()
            .any(|(k, _, s)| *k == "provider" && *s == MergeSource::UserFile)
    );
    assert!(
        resolved
            .iter()
            .any(|(k, _, s)| *k == "model" && *s == MergeSource::Default)
    );
    assert!(
        resolved.len() >= 12,
        "all documented keys reported: {}",
        resolved.len()
    );
}

// Verifies: FR-CFG-1 (project file outranks the user file)
#[test]
fn project_file_outranks_user_file() {
    let dir = scratch("project-wins");
    write(&dir.join("user.toml"), "provider = \"from-user\"\n");
    write(&dir.join("project.toml"), "provider = \"from-project\"\n");
    let config = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: Some(dir.join("project.toml")),
        trusted: true,
        user_file: Some(dir.join("user.toml")),
        headless: false,
    })
    .expect("load");
    assert_eq!(config.provider(), "from-project");
}

// Verifies: FR-PERM-9 (an untrusted project file changes nothing)
#[test]
fn untrusted_project_file_is_ignored() {
    let dir = scratch("untrusted");
    write(
        &dir.join("project.toml"),
        "provider = \"from-project\"\npermissions.proposals.git = [\"git status\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: Some(dir.join("project.toml")),
        trusted: false,
        user_file: None,
        headless: false,
    })
    .expect("load");
    assert_eq!(config.provider(), "openai-compatible");
    assert!(
        config.permissions_proposals().is_empty(),
        "an untrusted project proposes nothing (FR-PERM-9)"
    );
}

// Verifies: FR-CFG-1 (environment outranks the project file)
#[test]
fn environment_outranks_project_file() {
    let dir = scratch("env");
    write(&dir.join("project.toml"), "provider = \"from-project\"\n");
    let mut env = std::collections::BTreeMap::new();
    env.insert("LCA_PROVIDER".to_string(), "from-env".to_string());
    let config = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env,
        project_file: Some(dir.join("project.toml")),
        trusted: true,
        user_file: None,
        headless: false,
    })
    .expect("load");
    assert_eq!(config.provider(), "from-env");
}

// Verifies: FR-CFG-1 (flags outrank everything)
#[test]
fn flags_outrank_environment() {
    let mut env = std::collections::BTreeMap::new();
    env.insert("LCA_MODEL".to_string(), "from-env".to_string());
    let mut flags = std::collections::BTreeMap::new();
    flags.insert("model".to_string(), "from-flag".to_string());
    let config = Config::load(&lca_config::LoadInput {
        flags,
        env,
        project_file: None,
        trusted: true,
        user_file: None,
        headless: false,
    })
    .expect("load");
    assert_eq!(config.model(), Some("from-flag"));
}

// The documented environment naming: LCA_ prefix, key uppercased, dots
// replaced by underscores (docs/configuration.md).
#[test]
fn environment_names_follow_the_documented_mapping() {
    let mut env = std::collections::BTreeMap::new();
    env.insert("LCA_TOOL_TIMEOUT_SECONDS".to_string(), "7".to_string());
    env.insert(
        "LCA_CACHE_NOISE_FLOOR_TOKENS".to_string(),
        "512".to_string(),
    );
    env.insert("LCA_UI_COLOR".to_string(), "never".to_string());
    let config = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env,
        project_file: None,
        trusted: true,
        user_file: None,
        headless: false,
    })
    .expect("load");
    assert_eq!(config.tool_timeout_seconds(), 7);
    assert_eq!(config.cache_noise_floor_tokens(), 512);
    assert_eq!(config.ui_color(), ColorMode::Never);
}

// Verifies: FR-CFG-6 (update check defaults to on interactively, off headless)
#[test]
fn update_check_default_depends_on_mode() {
    let interactive = Config::defaults();
    assert!(interactive.update_check(false), "interactive default is on");
    let headless = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: None,
        trusted: true,
        user_file: None,
        headless: true,
    })
    .expect("load");
    assert!(!headless.update_check(true), "headless default is off");
}

// Verifies: FR-CFG-1 (typed keys reject bad values at load, with the source named)
#[test]
fn bad_values_report_the_source_that_set_them() {
    let dir = scratch("bad");
    write(&dir.join("user.toml"), "tool.timeout_seconds = \"soon\"\n");
    let err = Config::load(&lca_config::LoadInput {
        flags: Default::default(),
        env: Default::default(),
        project_file: None,
        trusted: true,
        user_file: Some(dir.join("user.toml")),
        headless: false,
    })
    .expect_err("must fail");
    let message = err.to_string();
    assert!(message.contains("tool.timeout_seconds"), "{message}");
    assert!(message.contains("user file"), "{message}");
}
