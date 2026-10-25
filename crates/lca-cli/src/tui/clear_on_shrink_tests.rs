//! Gh #538: the configured value reaches the renderer.
//!
//! Split from `hooks.rs`'s own test module for the workspace's 1,200-line
//! ceiling (gate 11), which is the same reason the workspace splits anything.
//!
//! **Why these rows exist at all.** Gh #372 bug 4 implemented pi's
//! `terminal.clearOnShrink` in the renderer and wrote rows for it - rows that set
//! the flag by hand. That proves the renderer and nothing about the path to it,
//! which is exactly how the config key stayed inert for a whole cycle. So these
//! rows start at a *loaded config* and end at what the renderer painted.

use crate::tui::hooks::display_tuning;

/// A loaded config with `terminal.clear_on_shrink` set through one layer.
fn config_with_clear_on_shrink(value: &str) -> lca_config::Config {
    let dir = lca_testkit::scratch_path("lca-538-clear-on-shrink");
    let file = dir.join("user.toml");
    std::fs::write(
        file.as_path(),
        format!("terminal.clear_on_shrink = {value}\n"),
    )
    .expect("write");
    lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(file),
        ..Default::default()
    })
    .expect("load")
}

/// What the renderer actually does with these tunables: paint three rows,
/// then repaint one, and report whether the emptied rows were cleared.
///
/// The renderer is the one production builds - `MainScreenRenderer::new()`,
/// then `apply_screen_tuning`, exactly as `lca_ui::run` does it - so this row
/// fails if the seam that carries the value changes, not only if the value does.
fn renderer_clears_emptied_rows(tuning: &lca_ui::state::DisplayTuning) -> bool {
    let mut renderer = lca_tui::engine::main_screen::MainScreenRenderer::new();
    renderer.set_clear_on_shrink(tuning.clear_on_shrink);
    let mut term = lca_tui::engine::terminal::FakeTerminal::new(20, 20);
    let lines = |text: &[&str]| text.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    renderer.render(&mut term, lines(&["a", "b", "c"]), 20, 20);
    let _ = term.take_output();
    renderer.render(&mut term, lines(&["a"]), 20, 20);
    let out = term.take_output();
    // `\x1b[3J` is the scrollback-clearing arm of `full_render(true, ..)`:
    // the only path a shrink takes when the setting is on.
    out.contains("\x1b[3J")
}

// Verifies: gh #538 (the row the issue is about) - a config *file* value of
// `true` travels: loader -> `display_tuning` -> the renderer, which clears
// the rows the shrink emptied.
#[test]
fn a_configured_clear_on_shrink_reaches_the_renderer() {
    let on = display_tuning(&config_with_clear_on_shrink("true"));
    assert!(on.clear_on_shrink, "the loader's value reaches the tuning");
    assert!(
        renderer_clears_emptied_rows(&on),
        "and the renderer clears the emptied rows"
    );
}

// Verifies: gh #538 - the inverse direction, so the row above cannot pass on
// a renderer that always clears.
#[test]
fn an_explicitly_disabled_clear_on_shrink_does_not_clear() {
    let off = display_tuning(&config_with_clear_on_shrink("false"));
    assert!(!off.clear_on_shrink);
    assert!(
        !renderer_clears_emptied_rows(&off),
        "off means the shrink leaves the rows alone"
    );
}

// Verifies: gh #538 - the default is off end to end: an empty load produces a
// tuning that builds a renderer which does not clear. This is pi's default and
// the deliberate choice of the cycle that implemented the renderer half.
#[test]
fn an_unset_clear_on_shrink_builds_a_renderer_that_does_not_clear() {
    let default = display_tuning(&lca_config::Config::defaults());
    assert!(
        !default.clear_on_shrink,
        "unset means off, exactly as pi ships it"
    );
    assert!(!renderer_clears_emptied_rows(&default));
}

// Verifies: gh #538 - the environment layer reaches the renderer too, in both
// directions: `terminal.clear_on_shrink` is in `KNOWN_KEYS`, the list the
// environment loop iterates (the gh #272 drift class).
#[test]
fn an_environment_clear_on_shrink_reaches_the_renderer() {
    for (raw, expected) in [("true", true), ("false", false)] {
        let config = lca_config::Config::load(&lca_config::LoadInput {
            env: std::collections::BTreeMap::from([(
                "LCA_TERMINAL_CLEAR_ON_SHRINK".to_string(),
                raw.to_string(),
            )]),
            ..Default::default()
        })
        .expect("load");
        let tuning = display_tuning(&config);
        assert_eq!(tuning.clear_on_shrink, expected, "env said {raw}");
        assert_eq!(
            renderer_clears_emptied_rows(&tuning),
            expected,
            "and the renderer agrees (env said {raw})"
        );
    }
}

// Verifies: gh #538 - the environment outranks the user file *all the way
// into the renderer*: the precedence ladder is the thing being exercised, not
// just the loader's own merge.
#[test]
fn the_precedence_ladder_holds_at_the_renderer() {
    let dir = lca_testkit::scratch_path("lca-538-ladder");
    let file = dir.join("user.toml");
    std::fs::write(file.as_path(), "terminal.clear_on_shrink = true\n").expect("write");
    let config = lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(file),
        env: std::collections::BTreeMap::from([(
            "LCA_TERMINAL_CLEAR_ON_SHRINK".to_string(),
            "false".to_string(),
        )]),
        ..Default::default()
    })
    .expect("load");
    assert!(
        !renderer_clears_emptied_rows(&display_tuning(&config)),
        "the environment's `false` wins over the file's `true` (FR-CFG-1)"
    );
}
