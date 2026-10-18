//! GitHub #265: `LoginFlow::pick` ignored its `provider` argument and
//! matched options by id alone across every provider, then overwrote the
//! requested provider with the matched row's owner.
//!
//! Several providers declare `id: "api-key"` and `id: "oauth"`, so the
//! search always landed on whichever provider came first in the list
//! (typically `anthropic`). Choosing "api-key" under OpenRouter therefore
//! stored the user's key in **Anthropic's credential namespace** — and
//! that provider then gets *sent* the key. A cross-vendor credential leak,
//! not merely a mislabelled row.
//!
//! The fix keeps the owner-overwrite (the matched option's owner IS the
//! credential namespace, which is correct once matching is scoped) but
//! matches inside the named provider first; the id-only search survives
//! only for a bare `/login` pick, where a row carries no provider of its
//! own and id-only is what the picker means.
//!
//! Verifies: the routing contract through the public `LoginFlow` — a
//! choice made under provider X submits to X, identical ids across
//! providers resolve to their own owners, a miss under a named provider is
//! a named refusal rather than a silent reroute, and the empty-provider
//! fallback still resolves by id alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_cli::login::{LoginFlow, Step};
use lca_protocol::LoginOption;
use lca_ui::LoginNext;

fn option(id: &str, fields: &[&str]) -> LoginOption {
    LoginOption {
        id: id.to_string(),
        name: id.to_string(),
        kind: "api-key".to_string(),
        host: "example.test".to_string(),
        fields: fields.iter().map(|f| (*f).to_string()).collect(),
        extras: BTreeMap::new(),
    }
}

/// Two providers that both declare `api-key` and `oauth` — the collision
/// the leak needs. `anthropic` is deliberately first, so an id-only search
/// always lands on it.
fn colliding() -> Vec<(String, LoginOption)> {
    vec![
        ("anthropic".into(), option("api-key", &["api-key"])),
        ("anthropic".into(), option("oauth", &[])),
        ("openrouter".into(), option("api-key", &["api-key"])),
        ("openrouter".into(), option("oauth", &[])),
    ]
}

/// The provider a completed flow submits to.
fn routed_provider(provider: &str, choice: &str, value: &str) -> String {
    let mut flow = LoginFlow::new();
    let _ = flow.offer(colliding());
    match flow.pick(provider, choice) {
        Step::Submit { provider, .. } => provider,
        Step::Next(_) => match flow.push(provider, value) {
            Step::Submit { provider, .. } => provider,
            Step::Next(next) => panic!("the flow did not complete: {next:?}"),
        },
    }
}

// Verifies: gh #265 acceptance 1 — a choice made under a provider routes
// to that provider. This is the leak: on the old code both of these
// returned "anthropic".
#[test]
fn a_colliding_option_id_routes_to_the_named_provider() {
    assert_eq!(routed_provider("openrouter", "oauth", ""), "openrouter");
    assert_eq!(
        routed_provider("openrouter", "api-key", "FAKESECRET-265"),
        "openrouter",
        "the key's namespace is the chosen provider"
    );
}

// Verifies: gh #265 acceptance 2 — the same id under different providers
// resolves to each one's own owner, in both directions.
#[test]
fn identical_ids_resolve_to_each_owner() {
    for chosen in ["anthropic", "openrouter"] {
        assert_eq!(
            routed_provider(chosen, "oauth", ""),
            chosen,
            "pick({chosen}, oauth)"
        );
        assert_eq!(
            routed_provider(chosen, "api-key", "FAKESECRET-265"),
            chosen,
            "pick({chosen}, api-key)"
        );
    }
}

// Verifies: gh #265 — a miss under a NAMED provider is refused by name,
// not silently rerouted to another provider's option with that id (a
// reroute would be the leak again, just harder to see).
#[test]
fn a_miss_under_a_named_provider_names_both_parties() {
    let mut flow = LoginFlow::new();
    let _ = flow.offer(colliding());
    let Step::Next(LoginNext::Message(text)) = flow.pick("grok", "api-key") else {
        panic!("grok declares no api-key");
    };
    assert!(text.contains("api-key"), "names the option: {text}");
    assert!(text.contains("grok"), "names the provider: {text}");
}

// Verifies: gh #265 — the bare `/login` pick keeps id-only resolution.
// That fallback is intended (the picker row carries no provider of its
// own there), so the fix must not remove it.
#[test]
fn an_empty_provider_still_resolves_by_id_alone() {
    assert_eq!(
        routed_provider("", "api-key", "FAKESECRET-265"),
        "anthropic",
        "first declarer: the documented fallback"
    );
}

// Verifies: gh #265 — the owner-overwrite survives: an option declared by
// provider X routes to X even when the caller named something else, which
// is what makes a preset's credential namespace its declaring extension.
#[test]
fn the_matched_owner_still_wins_over_the_request() {
    let mut flow = LoginFlow::new();
    let _ = flow.offer(vec![(
        "openai-compatible".into(),
        option("openrouter", &[]),
    )]);
    let Step::Submit { provider, .. } = flow.pick("openai-compatible", "openrouter") else {
        panic!("submits");
    };
    assert_eq!(provider, "openai-compatible");
}
