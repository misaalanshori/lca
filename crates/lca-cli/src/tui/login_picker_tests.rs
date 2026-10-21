//! Gh #297's picker-coverage rows, split from `login.rs` under the
//! workspace's 1,200-line ceiling (gate 11). Behaviour unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::{IDENTITY_LOGIN_ID, PickRoute, ProviderFacts, gather_from, pick_route};

// ---- gh #297: every enabled provider that can sign in must appear ----

/// The subscription providers this defect was reported for (the issue's
/// five), plus `openrouter`, which has the same shape and the same
/// invisibility. Each runs its own OAuth/device flow through the
/// `provider-identity` `login` export and declares no picker presets.
const IDENTITY_ONLY: &[&str] = &[
    "antigravity",
    "codex",
    "grok",
    "kimi-coding",
    "meta",
    "openrouter",
];

/// The facts the host collects for a provider that declares no presets
/// but can sign in - the shape `Ui::gather` builds for the six above.
fn identity_only(name: &str) -> ProviderFacts {
    ProviderFacts {
        name: name.to_string(),
        declared: Some(Vec::new()),
        can_sign_in: true,
    }
}

/// The facts for a provider with real preset options.
fn with_presets(name: &str, ids: &[&str]) -> ProviderFacts {
    ProviderFacts {
        name: name.to_string(),
        declared: Some(
            ids.iter()
                .map(|id| lca_protocol::LoginOption {
                    id: (*id).to_string(),
                    name: (*id).to_string(),
                    kind: "api-key".to_string(),
                    host: String::new(),
                    fields: vec!["api-key".to_string()],
                    extras: Default::default(),
                })
                .collect(),
        ),
        can_sign_in: true,
    }
}

// Verifies: gh #297 acceptance 1 - the picker's row builder returns a
// row for every enabled provider that can sign in, including each of
// the five subscription providers the issue names and `openrouter`.
// Before the fix these six contributed nothing and the picker hid them.
#[test]
fn every_enabled_subscription_provider_gets_a_row() {
    let facts: Vec<ProviderFacts> = IDENTITY_ONLY.iter().map(|n| identity_only(n)).collect();
    let gathered = gather_from(&facts);
    let owners: Vec<&str> = gathered.iter().map(|(owner, _)| owner.as_str()).collect();
    for provider in IDENTITY_ONLY {
        assert!(
            owners.contains(provider),
            "`{provider}` must appear in the /login picker, got {owners:?}"
        );
    }
    assert_eq!(
        gathered.len(),
        IDENTITY_ONLY.len(),
        "exactly one row each, no duplicates"
    );
}

// Verifies: gh #297 - the synthesized row's shape: a stable id the pick
// path can match, a human name, the `oauth` kind (these flows are OAuth
// or device-code, and `auth` reporting keys off the kind), and no
// fields so choosing it submits at once and starts the flow.
#[test]
fn the_subscription_row_is_oauth_shaped_and_fieldless() {
    let gathered = gather_from(&[identity_only("codex")]);
    let (owner, option) = gathered.first().expect("one row");
    assert_eq!(owner, "codex");
    assert_eq!(option.id, IDENTITY_LOGIN_ID);
    assert_eq!(option.kind, "oauth");
    assert!(
        option.fields.is_empty(),
        "no fields: choosing it starts the flow at once"
    );
    assert!(
        option.name.contains("codex"),
        "the label names the provider: {}",
        option.name
    );
    assert!(
        option.name != "codex",
        "the label is copy, not a bare id: {}",
        option.name
    );
}

// Verifies: gh #297 - the negative that keeps the picker honest: a
// provider that cannot sign in contributes no synthesized row, whether
// it declared nothing (`llama` before its preset) or declared options
// (`openai-compatible` with no OAuth of its own).
#[test]
fn a_provider_that_cannot_sign_in_gets_no_row() {
    let facts = vec![
        ProviderFacts {
            name: "llama".to_string(),
            declared: Some(Vec::new()),
            can_sign_in: false,
        },
        ProviderFacts {
            name: "preset-only".to_string(),
            declared: Some(vec![lca_protocol::LoginOption {
                id: "key".to_string(),
                name: "Key".to_string(),
                kind: "api-key".to_string(),
                host: String::new(),
                fields: vec!["api-key".to_string()],
                extras: Default::default(),
            }]),
            can_sign_in: false,
        },
    ];
    let gathered = gather_from(&facts);
    assert_eq!(
        gathered.len(),
        1,
        "only the declared row survives: {gathered:?}"
    );
    assert_eq!(gathered[0].0, "preset-only");
    assert_eq!(gathered[0].1.id, "key");
}

// Verifies: gh #297 - a provider whose options were already gathered
// gains nothing: no synthetic row on top of declared ones. That is the
// regression doing both halves would otherwise introduce.
#[test]
fn a_provider_with_real_options_gains_no_synthetic_row() {
    let facts = vec![
        with_presets("openai-compatible", &["openrouter", "custom"]),
        with_presets("anthropic", &["api-key", "subscription"]),
    ];
    let gathered = gather_from(&facts);
    assert_eq!(gathered.len(), 4, "exactly the four declared rows");
    assert!(
        gathered
            .iter()
            .all(|(_, option)| option.id != IDENTITY_LOGIN_ID),
        "no synthetic row over declared options: {gathered:?}"
    );
}

// Verifies: gh #297 - the backstop in the shape it exists for: a
// provider that grows an identity login but forgets its option is
// still visible. This is why the class cannot recur.
#[test]
fn a_future_provider_that_forgets_its_option_is_still_listed() {
    let gathered = gather_from(&[identity_only("brand-new-provider")]);
    assert_eq!(
        gathered.len(),
        1,
        "a new provider is visible without a change in the picker"
    );
    assert_eq!(gathered[0].0, "brand-new-provider");
    assert_eq!(gathered[0].1.id, IDENTITY_LOGIN_ID);
}

// Verifies: gh #297 - a provider whose `login-options` call FAILED is
// still offered, because the failure is not an absence. Before this it
// silently vanished along with the empty ones.
#[test]
fn a_provider_whose_options_failed_still_gets_its_row() {
    let facts = vec![ProviderFacts {
        name: "flaky".to_string(),
        declared: None,
        can_sign_in: true,
    }];
    let gathered = gather_from(&facts);
    assert_eq!(gathered.len(), 1);
    assert_eq!(gathered[0].0, "flaky");
}

// Verifies: gh #297 acceptance 2 - choosing a synthesized row routes to
// that provider's `provider-identity` login, NOT to `login-submit`. The
// five declare no `login-submit` (the trait default returns an empty
// setting list), so a pick that went down the submit path would succeed
// having done nothing - a silent no-op, worse than the invisible row.
#[test]
fn picking_a_synthesized_row_runs_the_identity_login() {
    for provider in IDENTITY_ONLY {
        assert_eq!(
            pick_route(provider, IDENTITY_LOGIN_ID),
            PickRoute::IdentityLogin,
            "`{provider}` must run its own login flow"
        );
    }
}

// Verifies: gh #297 - a real preset option still goes down the submit
// path, unchanged: the fix adds a branch, it does not reroute every
// pick.
#[test]
fn picking_a_declared_option_still_submits() {
    for (provider, id) in [
        ("anthropic", "api-key"),
        ("anthropic", "subscription"),
        ("openai-compatible", "openrouter"),
        ("github-copilot", "device"),
        ("", ""),
    ] {
        assert_eq!(
            pick_route(provider, id),
            PickRoute::Submit,
            "`{provider}/{id}` is not a synthesized identity row"
        );
    }
}

// Verifies: gh #297 - `Ui::gather` really calls `gather_from`. A pure
// row-builder is only worth its guard if the live path uses it; this
// asserts the source-level wiring, which is the regression a call-site
// deletion would otherwise slip past every row above.
#[test]
fn the_picker_path_uses_the_row_builder() {
    let source = include_str!("login.rs");
    // The call site, spelled so this test's own text cannot satisfy it.
    let call = ["let mut out", " = gather_from(", "&facts);"].concat();
    assert!(
        source.contains(&call),
        "Ui::gather must build its rows through gather_from"
    );
    // And the pre-fix short-circuit is gone: no early `out` return that
    // skips the builder.
    let legacy = ["with_identity", "_rows"].concat();
    assert!(
        !source.contains(&legacy),
        "one row-builder only; the earlier helper is gone"
    );
}
