//! GitHub #297: `/login` hid every subscription provider.
//!
//! Five first-party providers — `antigravity`, `codex`, `grok`, `kimi-coding`,
//! `meta` — sign in through the `provider-identity` `login` export (an OAuth
//! or device-code dance the provider runs itself) and declared **no**
//! `login-options`. The picker listed only providers whose options were
//! non-empty, so a user with a ChatGPT Plus, SuperGrok, Meta, Kimi or Google
//! subscription saw a picker that did not mention any of them, and had no
//! discoverable path unless they already knew the undocumented
//! `/login <name>` spelling. `openrouter` has the same shape and was hidden
//! the same way.
//!
//! The fix has two halves, and the second is what makes the class unable to
//! recur:
//!
//! 1. **A row-builder rule** (`gather_from`): every declared option becomes a
//!    row, and a provider that declared none but *can* sign in gets one
//!    synthesized row. A provider that cannot sign in gets nothing.
//! 2. **A dispatch rule** (`pick_route`): a synthesized row runs the
//!    provider's own `login`, not `login-submit`. This half is not cosmetic —
//!    none of the five implements `login-submit`, whose trait default answers
//!    `Ok(vec![])` having stored nothing, so routing a synthesized row down
//!    the submit path would have produced a sign-in that *reported success and
//!    did nothing at all*: strictly worse than an invisible row.
//!
//! This guard drives the real public seam, so it fails on the pre-fix tree
//! (no rows for the six) and on the tempting half-fix (rows present, dispatch
//! silent).
//!
//! Verifies: NFR-24 (a shipped defect's guard), gh #297 acceptance 1 and 2,
//! and ADR-0033's split between the preset protocol (`login-submit`) and a
//! provider's own flow (`provider-identity` `login`).

use lca_cli::tui::login::{IDENTITY_LOGIN_ID, PickRoute, ProviderFacts, gather_from, pick_route};

/// The subscription providers the issue names, plus `openrouter` — same
/// shape, same invisibility, found while fixing this.
const SUBSCRIPTION: &[&str] = &[
    "antigravity",
    "codex",
    "grok",
    "kimi-coding",
    "meta",
    "openrouter",
];

/// The facts `Ui::gather` collects for a provider that signs in through its
/// own flow and declares no picker presets.
fn identity_only(name: &str) -> ProviderFacts {
    ProviderFacts {
        name: name.to_string(),
        declared: Some(Vec::new()),
        can_sign_in: true,
    }
}

/// The facts for a provider with declared preset options.
fn declares(name: &str, ids: &[&str]) -> ProviderFacts {
    ProviderFacts {
        name: name.to_string(),
        declared: Some(
            ids.iter()
                .map(|id| lca_protocol::LoginOption {
                    id: (*id).to_string(),
                    name: format!("{name} {id}"),
                    kind: "api-key".to_string(),
                    host: format!("{name}.example.com"),
                    fields: vec!["api-key".to_string()],
                    extras: Default::default(),
                })
                .collect(),
        ),
        can_sign_in: false,
    }
}

// Verifies: gh #297 acceptance 1 - every subscription provider gets a row in
// the bare `/login` picker. This is the issue's headline.
#[test]
fn every_subscription_provider_is_visible_in_the_login_picker() {
    let facts: Vec<ProviderFacts> = SUBSCRIPTION
        .iter()
        .map(|name| identity_only(name))
        .collect();
    let rows = gather_from(&facts);
    let owners: Vec<&str> = rows.iter().map(|(owner, _)| owner.as_str()).collect();
    for provider in SUBSCRIPTION {
        assert!(
            owners.contains(provider),
            "`{provider}` must be offered by /login, got {owners:?}"
        );
    }
    assert_eq!(
        rows.len(),
        SUBSCRIPTION.len(),
        "one row each and nothing else: {rows:?}"
    );
}

// Verifies: gh #297 acceptance 2 - choosing a subscription row dispatches to
// that provider's own login flow, not to the preset submit protocol. Without
// this half the fix would be worse than the bug: a completed-looking sign-in
// that stored no credential.
#[test]
fn choosing_a_subscription_row_runs_that_providers_own_login() {
    for provider in SUBSCRIPTION {
        assert_eq!(
            pick_route(provider, IDENTITY_LOGIN_ID),
            PickRoute::IdentityLogin,
            "`{provider}` must run its identity login"
        );
    }
}

// Verifies: gh #297 - the negative half of the row rule: a provider that
// cannot sign in is not padded into the picker. `llama` is keyless until its
// preset is configured; a preset-only endpoint has no flow of its own.
#[test]
fn a_provider_that_cannot_sign_in_is_not_padded_in() {
    let facts = vec![
        ProviderFacts {
            name: "llama".to_string(),
            declared: Some(Vec::new()),
            can_sign_in: false,
        },
        identity_only("codex"),
    ];
    let rows = gather_from(&facts);
    assert_eq!(
        rows.len(),
        1,
        "only the provider that can sign in: {rows:?}"
    );
    assert_eq!(rows[0].0, "codex");
}

// Verifies: gh #297 - a provider with real presets is unchanged: no synthetic
// row on top of declared ones. This is the regression the "do both halves"
// recommendation could have introduced, and the reason the rule checks
// `declared` before synthesizing.
#[test]
fn a_provider_with_presets_keeps_exactly_its_declared_rows() {
    let facts = vec![
        declares("openai-compatible", &["openrouter", "custom"]),
        declares("anthropic", &["api-key", "subscription"]),
    ];
    let rows = gather_from(&facts);
    assert_eq!(rows.len(), 4, "the four declared rows, nothing added");
    assert!(
        rows.iter()
            .all(|(_, option)| option.id != IDENTITY_LOGIN_ID),
        "no synthesized row over declared options: {rows:?}"
    );
    // And a declared option still takes the submit path.
    assert_eq!(
        pick_route("anthropic", "api-key"),
        PickRoute::Submit,
        "the preset protocol is unchanged"
    );
}

// Verifies: gh #297 - the backstop in the shape it exists for: a provider
// that grows an identity login and forgets to declare an option is still
// discoverable. This is why the invisibility class cannot come back.
#[test]
fn a_new_provider_without_options_is_still_discoverable() {
    let rows = gather_from(&[identity_only("a-provider-that-does-not-exist-yet")]);
    assert_eq!(rows.len(), 1, "visible with no picker change needed");
    assert_eq!(rows[0].1.id, IDENTITY_LOGIN_ID);
}

// Verifies: gh #297 - a provider whose `login-options` call failed still
// appears. The failure is not an absence, and before the fix it vanished with
// the empty ones; losing a provider to a transient error would be the same
// bug wearing a different hat.
#[test]
fn a_provider_whose_options_call_failed_is_still_offered() {
    let facts = vec![ProviderFacts {
        name: "flaky".to_string(),
        declared: None,
        can_sign_in: true,
    }];
    let rows = gather_from(&facts);
    assert_eq!(
        rows.len(),
        1,
        "a failed options call is not a missing provider"
    );
    assert_eq!(rows[0].0, "flaky");
}
