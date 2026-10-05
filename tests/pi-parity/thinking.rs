//! Thinking-level parity: LCA speaks pi's reasoning vocabulary and refuses
//! anything outside it. The session default differs by design (unset means
//! the provider chooses; see `docs/pi-parity.md`).

use lca_config::{LoadInput, THINKING_LEVELS};

// Verifies: pi:packages/coding-agent/docs/settings.md (`defaultThinkingLevel`
// vocabulary) and pi:packages/coding-agent/docs/cli.md (`--thinking`).
#[test]
fn pi_parity_thinking_vocabulary_matches_pi() {
    assert_eq!(
        THINKING_LEVELS,
        &["off", "minimal", "low", "medium", "high", "xhigh", "max"],
        "pi's ThinkingLevel union, in full"
    );
}

// Verifies: pi:packages/coding-agent/docs/cli.md (`--thinking` sets one of
// the named levels; anything else is refused, never silently mapped).
#[test]
fn pi_parity_thinking_rejects_unknown_levels() {
    let mut flags = std::collections::BTreeMap::new();
    flags.insert("thinking".to_string(), "ultra".to_string());
    let err = lca_config::Config::load(&LoadInput {
        flags,
        ..Default::default()
    })
    .expect_err("an unknown level is refused at load");
    assert!(err.to_string().contains("ultra"), "{err}");

    let mut flags = std::collections::BTreeMap::new();
    flags.insert("thinking".to_string(), "high".to_string());
    let config = lca_config::Config::load(&LoadInput {
        flags,
        ..Default::default()
    })
    .expect("a named level loads");
    assert_eq!(config.thinking(), Some("high"));
}
