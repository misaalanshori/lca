//! Typed-key rows: a key's closed vocabulary, its default, and the three ways
//! a bool spells itself.
//!
//! Split out of `tests/merge.rs` under gate 11's 1,200-line ceiling (gh #545).
//! Each row exercises one documented key end to end - TOML in, `Config` out -
//! so a new setting lands here beside the ones it resembles.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_config::{Config, MergeSource};

use super::support::{scratch, write};

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

// Verifies: gh #112 - `ui.fullscreen` is a typed config key: TOML
// true/false is honored, and a malformed value fails loud at load.
#[test]
fn ui_fullscreen_is_a_typed_bool_key() {
    let dir = scratch("fullscreen-key");
    write(&dir.join("user.toml"), "[ui]\nfullscreen = true\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.ui_fullscreen(), Some(true));

    write(&dir.join("user.toml"), "[ui]\nfullscreen = false\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.ui_fullscreen(), Some(false));

    write(&dir.join("user.toml"), "[ui]\nfullscreen = \"yes\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a non-bool fullscreen is refused at load");
    assert!(err.to_string().contains("ui.fullscreen"), "{err}");
}

// Verifies: gh #133 - `shell.command_prefix` is a typed config key,
// honored from TOML.
#[test]
fn shell_command_prefix_is_a_typed_key() {
    let dir = scratch("command-prefix-key");
    write(
        &dir.join("user.toml"),
        "[shell]\ncommand_prefix = \"export LCA_PROBE=1\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.shell_command_prefix(), Some("export LCA_PROBE=1"));
    assert_eq!(Config::defaults().shell_command_prefix(), None);
}

// Verifies: gh #117 - `tool.edit_requires_read` defaults off (pi
// parity), loads true from the file, and refuses non-booleans.
#[test]
fn edit_requires_read_defaults_off_and_loads() {
    assert!(
        !Config::defaults().tool_edit_requires_read(),
        "off by default: pi parity"
    );
    let dir = scratch("edit-requires-read");
    write(&dir.join("user.toml"), "tool.edit_requires_read = true\n");
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert!(config.tool_edit_requires_read(), "on when set");
    write(
        &dir.join("user.toml"),
        "tool.edit_requires_read = \"yes\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("a non-boolean is refused at load");
    assert!(err.to_string().contains("tool.edit_requires_read"), "{err}");
}

// Verifies: gh #131 - `ui.quiet_startup` defaults to showing the
// header, accepts true/false/"header", and refuses anything else.
#[test]
fn quiet_startup_defaults_to_show_and_parses_three_ways() {
    assert_eq!(Config::defaults().ui_quiet_startup(), "false");
    for (toml, want) in [
        ("ui.quiet_startup = true", "true"),
        ("ui.quiet_startup = false", "false"),
        ("ui.quiet_startup = \"header\"", "header"),
    ] {
        let dir = scratch("quiet-startup");
        write(&dir.join("user.toml"), &format!("{toml}\n"));
        let config = Config::load(&lca_config::LoadInput {
            user_file: Some(dir.join("user.toml")),
            ..Default::default()
        })
        .expect("load");
        assert_eq!(config.ui_quiet_startup(), want, "{toml}");
    }
    let dir = scratch("quiet-startup-bad");
    write(&dir.join("user.toml"), "ui.quiet_startup = \"sometimes\"\n");
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("anything else is refused at load");
    assert!(err.to_string().contains("ui.quiet_startup"), "{err}");
}

// Verifies: gh #132 - double-escape and tree-filter keys default to
// pi's values, parse their domains, and refuse anything else.
#[test]
fn double_escape_and_tree_filter_parse() {
    assert_eq!(Config::defaults().ui_double_escape_action(), "tree");
    assert_eq!(Config::defaults().ui_tree_filter_mode(), "default");
    let dir = scratch("double-escape");
    write(
        &dir.join("user.toml"),
        "ui.double_escape_action = \"fork\"\nui.tree_filter_mode = \"user-only\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.ui_double_escape_action(), "fork");
    assert_eq!(config.ui_tree_filter_mode(), "user-only");
    write(
        &dir.join("user.toml"),
        "ui.double_escape_action = \"sometimes\"\n",
    );
    let err = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("outside the domain is refused");
    assert!(err.to_string().contains("ui.double_escape_action"), "{err}");
}

// Verifies: gh #83 (the retry inventory) — `provider.retry_base_delay_ms`
// merges and defaults to the historical 250ms.
//
// SANCTIONED CHANGE (gh #273): this row used to end by writing
// `net.proxy_url = "http://proxy:8080"` and asserting the load
// *succeeded* with no source for the key - "an unknown key never fails
// the load". Gh #273 exists precisely to reverse that behavior: a
// phantom key now fails loud and names itself. The proxy decision the
// old tail was documenting (the proxy is environment-only) moved to
// `an_unknown_key_is_refused_rather_than_ignored` below, which keeps
// the same intent with the new contract.
#[test]
fn retry_base_delay_merges_and_defaults_to_the_historical_250ms() {
    let dir = scratch("retry-inventory");
    let config = Config::load(&lca_config::LoadInput {
        user_file: None,
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.provider_retry_base_delay_ms(), 250);

    write(
        &dir.join("user.toml"),
        "provider.retry_base_delay_ms = 100\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.provider_retry_base_delay_ms(), 100);
}

// Verifies: gh #52 - `provider.retry_max_delay_ms` caps every retry
// sleep (pi's `retry.maxAgentDelayMs`, 60s default); a configured
// value merges like the other retry keys.
#[test]
fn retry_max_delay_merges_and_defaults_to_sixty_seconds() {
    let dir = scratch("retry-max-delay");
    let config = Config::load(&lca_config::LoadInput {
        user_file: None,
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.provider_retry_max_delay_ms(), 60_000);

    write(
        &dir.join("user.toml"),
        "provider.retry_max_delay_ms = 5000\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.provider_retry_max_delay_ms(), 5_000);
}

// Verifies: gh #76 - `queue.steering_mode` defaults to `all`
// (today's flush) and `queue.follow_up_mode` to `one-at-a-time`
// (today's one-per-turn); both merge, anything else refuses.
#[test]
fn queue_modes_default_merge_and_refuse() {
    let dir = scratch("queue-modes");
    let config = Config::load(&lca_config::LoadInput {
        user_file: None,
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.queue_steering_mode(), "all");
    assert_eq!(config.queue_follow_up_mode(), "one-at-a-time");

    write(
        &dir.join("user.toml"),
        "queue.steering_mode = \"one-at-a-time\"\nqueue.follow_up_mode = \"all\"\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect("load");
    assert_eq!(config.queue_steering_mode(), "one-at-a-time");
    assert_eq!(config.queue_follow_up_mode(), "all");

    write(
        &dir.join("user.toml"),
        "queue.steering_mode = \"sometimes\"\n",
    );
    Config::load(&lca_config::LoadInput {
        user_file: Some(dir.join("user.toml")),
        ..Default::default()
    })
    .expect_err("unknown mode refuses");
}

// Verifies: gh #42 - cache warming is off unless opted in, and miss
// notices default off like pi; both ride a project file round-trip.
#[test]
fn cache_warming_keys_default_off_and_round_trip() {
    let defaults = Config::defaults();
    assert_eq!(defaults.cache_warming(), "off");
    assert!(!defaults.cache_miss_notices());
    let root = scratch("cache-warming-roundtrip");
    write(
        &root.join("lca.toml"),
        "cache.warming = \"idle\"\ncache.miss_notices = true\n",
    );
    let config = Config::load(&lca_config::LoadInput {
        project_file: Some(root.join("lca.toml")),
        trusted: true,
        ..Default::default()
    })
    .expect("loads");
    assert_eq!(config.cache_warming(), "idle");
    assert!(config.cache_miss_notices());
}
