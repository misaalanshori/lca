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
    assert!(config.compaction_enabled(), "on by default");
    assert_eq!(
        config.compaction_reserve_tokens(),
        0,
        "0 = derive the reserve from the threshold fraction"
    );
    assert_eq!(config.compaction_keep_recent_tokens(), 20_000);
    assert_eq!(config.provider_retry_limit(), 3);
    assert_eq!(config.tool_timeout_seconds(), 120);
    assert_eq!(config.tool_result_limit_bytes(), 65536);
    assert_eq!(config.tool_max_iterations(), 0, "0 = unlimited (FR-CORE-9)");
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

// Verifies: gh #8 / EFG-003 (the enabled model set, pi's `enabledModels`)
// - `models.enabled` is an ordinary layered key: a TOML array in a file,
// a comma list through the environment and the flag layer, and an unset
// key means *everything the provider offers* (empty scope = all).
#[test]
fn the_enabled_model_scope_merges_from_every_layer() {
    let dir = scratch("models-enabled");
    write(
        &dir.join("user.toml"),
        "models.enabled = [\"zen/*\", \"*spark*\"]\n",
    );

    // 1. A file's array is the scope.
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["zen/*", "*spark*"]);

    // 2. Unset means no restriction: every offered model is in scope.
    let unset = Config::load(&lca_config::LoadInput::default()).expect("load");
    assert!(unset.models_enabled().is_empty(), "empty scope = all");

    // 3. The environment reads as a comma list and outranks the file.
    let env = || {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "LCA_MODELS_ENABLED".to_string(),
            "mimo-a, mimo-b".to_string(),
        );
        map
    };
    let config = Config::load(&lca_config::LoadInput {
        env: env(),
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["mimo-a", "mimo-b"]);

    // 4. `--models a,b` outranks the environment (FR-CFG-1's flag layer).
    let mut flags = std::collections::BTreeMap::new();
    flags.insert("models.enabled".to_string(), "only-this".to_string());
    let config = Config::load(&lca_config::LoadInput {
        flags,
        env: env(),
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.models_enabled(), vec!["only-this"]);
    assert!(
        config
            .resolved()
            .any(|(key, _, source)| key == "models.enabled" && source == MergeSource::Flag),
        "`/settings` names the flag as the winning source"
    );
}

// Verifies: gh #8 phase 4 (pi's `modelThinkingLevels`) - a per-model map
// of the levels that model accepts, in the file (it is a table, so it has
// no single-value environment form); a level outside the vocabulary or a
// value that is not a list of strings is refused at load.
#[test]
fn per_model_thinking_levels_parse_and_refuse_junk() {
    let dir = scratch("thinking-levels");
    write(
        &dir.join("user.toml"),
        // A model id carries dots, so its key is quoted - the same TOML
        // rule any dotted key follows (`tool.timeout_seconds`).
        "[models.thinking_levels]\n\"mimo-v2.6-flash-free\" = [\"low\", \"high\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(
        config.allowed_thinking_levels("mimo-v2.6-flash-free"),
        Some(&["low".to_string(), "high".to_string()][..]),
        "the map reads back as the model's allowed set"
    );
    assert_eq!(
        config.allowed_thinking_levels("something-else"),
        None,
        "a model the map does not name has no set"
    );
    assert_eq!(
        config.default_thinking_for("mimo-v2.6-flash-free"),
        Some("low"),
        "the first allowed level is the model's default"
    );

    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nbroken = [\"lightning\"]\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a level outside the vocabulary is refused");
    assert!(err.to_string().contains("lightning"), "{err}");

    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nbroken = \"high\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a single level is not a list");
    assert!(err.to_string().contains("models.thinking_levels"), "{err}");
}

// Verifies: gh #8 - the clamp: a level the model does not offer becomes
// the model's default (its first allowed level); a level inside the set
// passes through untouched; an unconfigured model keeps what it was
// asked for; and "unset" is not a level, so it stays unset.
#[test]
fn a_level_outside_the_models_set_clamps_to_its_default() {
    let dir = scratch("thinking-clamp");
    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\nstrict = [\"low\", \"medium\"]\nloose = [\"off\", \"high\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    assert_eq!(
        config.clamp_thinking(Some("high"), "strict"),
        Some("low".to_string()),
        "high is not offered here, so the model's default is"
    );
    assert_eq!(
        config.clamp_thinking(Some("medium"), "strict"),
        Some("medium".to_string()),
        "an offered level passes through"
    );
    assert_eq!(
        config.clamp_thinking(Some("xhigh"), "loose"),
        Some("off".to_string()),
        "the clamp lands on the model's own default, not the nearest level"
    );
    assert_eq!(
        config.clamp_thinking(Some("high"), "unconfigured"),
        Some("high".to_string()),
        "no map entry means no restriction"
    );
    assert_eq!(
        config.clamp_thinking(None, "strict"),
        None,
        "unset is the provider's choice, not a level to clamp"
    );
}

// Verifies: gh #8 - a model switch applies the new model's configured
// default (pi's per-model default beating the global one); with no
// configured default the session's current level survives, clamped to
// what the new model accepts.
#[test]
fn switching_applies_the_models_configured_default() {
    let dir = scratch("thinking-switch");
    write(
        &dir.join("user.toml"),
        "[models.thinking_levels]\ndeep = [\"medium\", \"high\"]\nshallow = [\"off\"]\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");

    assert_eq!(
        config.switch_thinking(Some("off"), "deep"),
        Some("medium".to_string()),
        "the model's default wins over the session's level"
    );
    assert_eq!(
        config.switch_thinking(Some("high"), "shallow"),
        Some("off".to_string()),
        "and a level the new model refuses is clamped into its set"
    );
    assert_eq!(
        config.switch_thinking(Some("high"), "unconfigured"),
        Some("high".to_string()),
        "no default configured: the level keeps going"
    );
    assert_eq!(
        config.switch_thinking(None, "deep"),
        Some("medium".to_string()),
        "a model with a default still gets it, from unset too"
    );
    assert_eq!(config.switch_thinking(None, "unconfigured"), None);
}

// Verifies: gh #32 - `markdown.codeblock_border` takes the three shapes
// the issue defines (`full` is the default and the shipped look),
// through every layer, and refuses anything else at load.
#[test]
fn codeblock_border_takes_the_three_shapes_and_refuses_the_rest() {
    let config = Config::load(&lca_config::LoadInput::default()).expect("load");
    assert_eq!(
        config.markdown_codeblock_border(),
        "full",
        "today's framed look stays the default"
    );

    let dir = scratch("codeblock-border");
    write(
        &dir.join("user.toml"),
        "markdown.codeblock_border = \"horizontal\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.markdown_codeblock_border(), "horizontal");

    // A file value is validated at load, with the source named.
    write(
        &dir.join("user.toml"),
        "markdown.codeblock_border = \"boxed\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a shape outside the vocabulary is refused");
    assert!(err.to_string().contains("boxed"), "{err}");

    // The environment reads the same key (LCA_MARKDOWN_CODEBLOCK_BORDER).
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "LCA_MARKDOWN_CODEBLOCK_BORDER".to_string(),
        "none".to_string(),
    );
    let config = Config::load(&lca_config::LoadInput {
        env,
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.markdown_codeblock_border(), "none");
    assert!(
        config
            .resolved()
            .any(|(key, value, source)| key == "markdown.codeblock_border"
                && value == "none"
                && source == MergeSource::Env),
        "`lca config` names where it came from"
    );
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

// Verifies: gh #43 (`skills.inject_matched`): matched skill-text
// injection is opt-in, default off, settable from a file.
#[test]
fn skills_inject_matched_defaults_off_and_loads_from_file() {
    let plain = Config::defaults();
    assert!(!plain.skills_inject_matched(), "the default is the catalog");
    assert_eq!(
        plain.source_of("skills.inject_matched"),
        lca_config::MergeSource::Default
    );

    let dir = scratch("skills-inject");
    write(&dir.join("user.toml"), "[skills]\ninject_matched = true\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert!(config.skills_inject_matched());
    assert_eq!(
        config.source_of("skills.inject_matched"),
        lca_config::MergeSource::UserFile
    );
}

// Verifies: gh #36 phase 1 - the compaction budget keys load from the
// user file and report their source; a bad value names its key.
#[test]
fn compaction_budget_keys_load_and_a_bad_value_names_its_key() {
    let dir = scratch("compaction-budget");
    write(
        &dir.join("user.toml"),
        "[compaction]\nenabled = false\nreserve_tokens = 4096\nkeep_recent_tokens = 5000\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert!(!config.compaction_enabled());
    assert_eq!(config.compaction_reserve_tokens(), 4096);
    assert_eq!(config.compaction_keep_recent_tokens(), 5000);
    assert_eq!(
        config.source_of("compaction.reserve_tokens"),
        lca_config::MergeSource::UserFile
    );

    let bad = scratch("compaction-budget-bad");
    write(
        &bad.join("user.toml"),
        "[compaction]\nreserve_tokens = -5\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(bad.join("user.toml")),
        ..Default::default()
    })
    .expect_err("rejects");
    assert!(
        err.to_string().contains("compaction.reserve_tokens"),
        "{err}"
    );
}
