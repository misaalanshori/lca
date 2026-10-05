//! Configuration parity: layered precedence and explicit zero-prompt mode.
//! Pi reads settings with environment overrides; LCA keeps prompts by
//! default and reserves `--yolo` for pi's unprompted flow (a settled
//! divergence, documented in `docs/pi-parity.md`).

use std::collections::BTreeMap;

use lca_config::{Config, LoadInput};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(&format!("pi-parity-config-{name}"))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn input() -> LoadInput {
    LoadInput {
        flags: BTreeMap::new(),
        env: BTreeMap::new(),
        project_file: None,
        trusted: true,
        user_file: None,
        headless: true,
    }
}

// Verifies: pi:packages/coding-agent/docs/environment-variables.md
// (environment overrides settings) and pi:packages/coding-agent/docs/cli.md
// (flags override everything) — LCA's FR-CFG-1 precedence in full.
#[test]
fn pi_parity_config_precedence_flag_beats_env_beats_file() {
    let dir = scratch("precedence");
    write(&dir.join("user.toml"), "tool.timeout_seconds = 111\n");
    let mut base = input();
    base.user_file = Some(dir.join("user.toml"));

    let file_only = Config::load(&base).expect("load");
    assert_eq!(file_only.tool_timeout_seconds(), 111);

    let mut with_env = base.clone();
    with_env
        .env
        .insert("LCA_TOOL_TIMEOUT_SECONDS".to_string(), "222".to_string());
    let env_wins = Config::load(&with_env).expect("load");
    assert_eq!(
        env_wins.tool_timeout_seconds(),
        222,
        "environment beats the file"
    );

    let mut with_flag = with_env.clone();
    with_flag
        .flags
        .insert("tool.timeout_seconds".to_string(), "333".to_string());
    let flag_wins = Config::load(&with_flag).expect("load");
    assert_eq!(
        flag_wins.tool_timeout_seconds(),
        333,
        "flags beat everything"
    );
}

// Verifies: pi:packages/coding-agent/docs/security.md (pi runs unprompted;
// LCA asks unless yolo is explicitly chosen — ADR-0042).
#[test]
fn pi_parity_zero_prompt_mode_is_explicit_never_default() {
    let unset = Config::load(&input()).expect("load");
    assert!(
        unset.permissions_mode().is_none(),
        "unset means ask, per the key reference"
    );
    assert_eq!(lca_config::PERMISSION_MODES, &["ask", "yolo"]);

    let mut yolo = input();
    yolo.flags
        .insert("permissions.mode".to_string(), "yolo".to_string());
    let explicit = Config::load(&yolo).expect("load");
    assert_eq!(
        explicit.permissions_mode(),
        Some("yolo"),
        "yolo is always a deliberate choice"
    );
}
