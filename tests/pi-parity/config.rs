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

// Verifies: pi:packages/coding-agent/docs/settings.md (`tuiMode`) - **a settled
// divergence, not a pin**: pi 1.0.0 defaults `tuiMode` to `"fullscreen"` and LCA's
// scrollback renderer is the default, with the alt screen an opt-in. The clause
// lives in `docs/pi-parity.md`'s *Settled divergences* and ADR-0037's 2026-10-02
// annotation settles LCA's side; this row is the guard that keeps the record true.
//
// **Why the assertion is split the way it is.** The default is not a constant in
// `UiState` - `UiState::new`'s test fixture says `fullscreen: false`, but the
// production answer is computed at one seam, `initial_screen_mode` in
// `crates/lca-cli/src/tui/reload.rs` (`tui` is not reachable from this crate unless
// that item is exported, and it is `pub(super)`), so:
//
// * **behavioural**, from config: the built-in default leaves `ui.fullscreen`
//   *unset* (`None`), which is what makes the resolution's fallback the decision
//   rather than a duplicated literal;
// * **behavioural**, from the key inventory: `ui.fullscreen` is a real setting, so
//   the opt-in exists as a documented surface;
// * **structural**, at the real resolution point: `initial_screen_mode`'s
//   *absent* branch answers `config_fullscreen.unwrap_or(false)` and the
//   *malformed* branch the same. A source pin here is the honest anchor rather
//   than a behavioural one against a proxy: the alternative would be asserting a
//   fixture that production does not use.
//
// **The mutation that proves it bites**: changing either `unwrap_or(false)` to
// `unwrap_or(true)` in this seam reddens the row (measured).
//
// **And the mutation the brief suggested does not, which is the finding worth
// recording**: `crates/lca-ui/src/ui_state.rs:192`'s `fullscreen: false` sits inside
// `#[cfg(test)] mod tests`'s `options()` fixture - flipping it changes no production
// behaviour and reddens nothing here (measured: the row still passed). That is
// precisely why the pin is at the resolution seam instead of at the literal the
// brief pointed at: a fixture cannot decide what a user gets.
#[test]
fn pi_parity_scrollback_renderer_is_the_default_and_fullscreen_is_the_opt_in() {
    // Pi's side, named for the reader: `tuiMode` defaults to "fullscreen"
    // (`packages/coding-agent/docs/settings.md`). LCA's side is below.
    let defaults = Config::defaults();
    assert_eq!(
        defaults.ui_fullscreen(),
        None,
        "the shipped default does not pin a mode; the resolution below decides"
    );

    // The opt-in is a real, documented surface (the documented spelling is
    // `ui.fullscreen`, and `/fullscreen` is the runtime toggle).
    assert!(
        lca_config::KNOWN_KEYS.contains(&"ui.fullscreen"),
        "`ui.fullscreen` is a setting this build knows: the opt-in exists"
    );

    // The resolution point, from the source: both non-persisted branches fall
    // back to `false` (scrollback). One literal, in one place.
    let reload = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-cli/src/tui/reload.rs"),
    )
    .expect("read reload.rs");
    let start = reload
        .find("pub(super) fn initial_screen_mode(")
        .expect("the resolution seam exists");
    let body = &reload[start..];
    let end = body.find("\n}\n").expect("its body ends");
    assert_eq!(
        body[..end].matches("unwrap_or(false)").count(),
        2,
        "both fallbacks (no ui.json, malformed ui.json) answer scrollback: {}",
        &body[..end]
    );
    assert!(
        !body[..end].contains("unwrap_or(true)"),
        "and neither answers fullscreen: the default is the scrollback renderer"
    );
}
