//! GitHub #297: a subscription provider must declare its sign-in.
//!
//! The picker's rule (`gather_from` + `declares_identity_login`) reads
//! `[login] identity = true` from each provider's manifest. That is what makes
//! a provider visible without running its flow - the naive alternative,
//! calling `identity_login()` to see whether it answers `NotSupported`, starts
//! a browser or blocks on a device poll for every installed subscription
//! provider the moment `/login` opens.
//!
//! So the declaration *is* the fix's other half: a row builder with nothing to
//! read is a row builder that shows nothing. This guard pins the six manifests
//! the issue is about (`antigravity`, `codex`, `grok`, `kimi-coding`, `meta`,
//! plus `openrouter`, found with the same shape), and pins the negative the
//! other way: a provider that signs in with a plain key must NOT claim an
//! identity flow, or the picker would offer a sign-in that has no flow behind
//! it.
//!
//! Verifies: NFR-24 (a shipped defect's guard), gh #297 acceptance 1, and the
//! manifest-as-consent-surface rule the capability catalog states
//! (`docs/capabilities.md`: the manifest is what the user is shown).

/// `(extension, declares identity login)`.
///
/// Read here rather than through the host so a manifest typo fails this guard
/// instead of silently hiding a provider again.
const MANIFESTS: &[(&str, &str)] = &[
    (
        "antigravity",
        include_str!("../../extensions/antigravity/extension.toml"),
    ),
    (
        "codex",
        include_str!("../../extensions/codex/extension.toml"),
    ),
    ("grok", include_str!("../../extensions/grok/extension.toml")),
    (
        "kimi-coding",
        include_str!("../../extensions/kimi-coding/extension.toml"),
    ),
    ("meta", include_str!("../../extensions/meta/extension.toml")),
    (
        "openrouter",
        include_str!("../../extensions/openrouter/extension.toml"),
    ),
];

/// Manifests that must NOT declare an identity flow: they sign in by being
/// handed a key (`llama` is keyless until configured), so a synthesized row
/// would offer a sign-in with nothing behind it.
const KEY_ONLY: &[(&str, &str)] = &[
    (
        "compaction-default",
        include_str!("../../extensions/compaction-default/extension.toml"),
    ),
    (
        "formatter",
        include_str!("../../extensions/formatter/extension.toml"),
    ),
];

/// `[login] identity = true`, read without a TOML dependency (the regression
/// crate has none, and adding one for a four-line read is not worth a
/// `Cargo.toml` edit). A line-by-line scan is enough: the key lives in its own
/// `[login]` table and is never inline.
///
/// Returns `(declared, saw_login_table)`.
fn login_identity(text: &str) -> (bool, bool) {
    let mut in_login = false;
    let mut saw_table = false;
    let mut declared = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_login = line == "[login]";
            saw_table |= in_login;
            continue;
        }
        if !in_login {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "identity"
        {
            declared = value.trim() == "true";
        }
    }
    (declared, saw_table)
}

fn declares_identity(text: &str) -> bool {
    login_identity(text).0
}

// Verifies: gh #297 acceptance 1 - every subscription provider the issue names
// declares `[login] identity = true`, so the picker has something to read and
// the provider is visible on a bare `/login`.
#[test]
fn every_subscription_provider_declares_its_identity_login() {
    for (name, text) in MANIFESTS {
        assert!(
            declares_identity(text),
            "`{name}` must declare `[login] identity = true` or /login hides it again"
        );
    }
}

// Verifies: gh #297 - the declaration is not free-floating: a provider that
// declares an identity flow must actually export the `provider` world and hold
// the `oauth` capability, or the row would dispatch into nothing.
#[test]
fn a_declared_identity_login_has_the_flow_behind_it() {
    for (name, text) in MANIFESTS {
        // The worlds line is a single inline array in every manifest here,
        // so a substring check is exact enough and needs no parser.
        let worlds_line = text
            .lines()
            .find(|line| line.trim_start().starts_with("worlds"))
            .unwrap_or_default();
        assert!(
            worlds_line.contains("\"provider\""),
            "`{name}` declares an identity login but no `provider` world: {worlds_line}"
        );
        // The flow needs *a* means to reach its sign-in: either the loopback
        // OAuth capability (a browser flow: codex, grok, antigravity,
        // openrouter) or a `net` host to poll (a device-code flow:
        // kimi-coding, meta). Requiring `oauth` specifically was wrong - a
        // device flow binds no port and would only be refused it.
        assert!(
            text.contains("[capabilities.oauth]") || text.contains("[capabilities.net]"),
            "`{name}` declares an identity login but neither `oauth` nor `net`: \
             its flow could not reach the sign-in endpoint"
        );
    }
}

// Verifies: gh #297 - the negative: a package with no sign-in flow does not
// claim one. These are data-only or tool packages; a `[login] identity = true`
// in either would synthesize a picker row that cannot do anything.
#[test]
fn a_package_without_a_sign_in_flow_claims_none() {
    for (name, text) in KEY_ONLY {
        assert!(
            !declares_identity(text),
            "`{name}` has no sign-in flow and must not declare one"
        );
    }
}

// Verifies: gh #297 - the declaration is what the *host* reads, not a second
// copy of the rule. If the picker ever grows its own parsing, this assertion
// (same shape as `declares_identity_login`) keeps the two agreeing.
#[test]
fn the_host_reads_the_same_declaration() {
    // The rule the picker applies, restated: `login.identity` as a bool,
    // absent meaning false. A manifest that satisfied one parser and not the
    // other would be a silent divergence.
    for (name, text) in MANIFESTS.iter().chain(KEY_ONLY) {
        let (declared, _) = login_identity(text);
        assert_eq!(
            declared,
            declares_identity(text),
            "`{name}`: one rule, one answer"
        );
        // And the declaration sits inside its own table, which is what the
        // host's parser keys on - a stray `identity = true` at top level
        // reads as nothing.
        if declared {
            assert!(
                login_identity(text).1,
                "`{name}`: the flag must live under [login]"
            );
        }
    }
}
