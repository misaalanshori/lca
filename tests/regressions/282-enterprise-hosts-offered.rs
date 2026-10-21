//! Gh #282 (NFR-24): an enterprise Copilot account's hosts are *field
//! data*, not manifest declarations.
//!
//! `login_options` asks for `enterprise-domain`, and the flow then calls
//! `https://<enterprise>/login/device/code`. The ad-hoc `net` offer only
//! knows the preset's declared host (`github.com`), so every non-github.com
//! account hit a grant denial with no way forward: the host's offer runs
//! *after* a successful `login-submit`, and the submit's own first request
//! is the one denied.
//!
//! These rows guard the extension's half, at the source level, because the
//! regression crate cannot depend on an extension crate (`Cargo.toml` is
//! outside this batch's fence) and the behaviour itself is pinned by the
//! extension's own `tests/login.rs`. What is worth pinning here is the
//! *shape* that keeps the two halves from drifting: one derivation for the
//! inference host, validation before the field becomes a URL or a grant
//! pattern, and a default domain that derives nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failing assertion.

fn source() -> &'static str {
    include_str!("../../extensions/github-copilot/src/lib.rs")
}

/// The body of one function, from its `fn` line to the next item boundary.
/// Line-independent: `cargo fmt` reflows these files, and a guard that
/// greps a reflowed layout fails for the wrong reason (it did, once).
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in the source"));
    let rest = &source[start..];
    // End at the next top-level `fn`, `pub fn`, or `const` line.
    let end = rest[1..]
        .find("\nfn ")
        .into_iter()
        .chain(rest[1..].find("\npub fn "))
        .chain(rest[1..].find("\npub const "))
        .chain(rest[1..].find("\nconst "))
        .min()
        .unwrap_or(rest.len() - 1);
    &rest[..end + 1]
}

/// One derivation for the inference host: `api_base` and the grant list
/// must not spell it separately. They agree today, which is exactly what
/// makes a duplicate dangerous - it drifts silently and takes the granted
/// host with it.
#[test]
fn the_enterprise_inference_host_has_one_derivation() {
    let source = source();
    assert_eq!(
        source.matches("fn enterprise_inference_url").count(),
        1,
        "one derivation for the inference URL"
    );
    assert!(
        source.contains("return enterprise_inference_url(&domain);"),
        "`api_base` uses the shared derivation rather than respelling the host"
    );
}

/// The field is validated before it can become a URL or a `net` pattern.
/// A scheme, path, port, wildcard, user info or whitespace is a mistake the
/// user can fix in place; letting it through turns it into a confusing
/// failure later (or, worse, consents to a host they did not mean).
#[test]
fn the_enterprise_field_is_validated_before_use() {
    let source = source();
    let validator = body_of(source, "pub fn parse_enterprise_domain");
    for rule in ["://", "/", "@", ":", "*"] {
        assert!(
            validator.contains(rule),
            "the validator refuses `{rule}` before the value becomes a URL/pattern"
        );
    }
    assert!(
        validator.contains("is_whitespace"),
        "whitespace is refused rather than silently trimmed"
    );
    // And the refusal text tells the user what to type.
    assert!(
        source.contains("ENTERPRISE_HOSTNAME_HELP"),
        "one hostname help string is shared by the refusal and the host"
    );
}

/// The default domain derives no ad-hoc hosts, so the github.com path keeps
/// exactly today's consent surface (acceptance 2). Read from the source's
/// own early return rather than a second copy of the rule.
#[test]
fn the_default_domain_derives_no_extra_hosts() {
    let source = source();
    let derivation = body_of(source, "pub fn enterprise_hosts");
    assert!(
        derivation.contains("domain == GITHUB_DOMAIN_DEFAULT"),
        "the default domain is special-cased to derive nothing"
    );
    assert!(
        derivation.contains("is_declared_host"),
        "already-declared hosts are filtered, so nobody is asked to grant what they have"
    );
}

/// The refusal for an ungranted enterprise account names the fix instead
/// of a dead end. A guest cannot offer consent from inside the sandbox, so
/// the copy must carry the hosts and the command.
#[test]
fn an_ungranted_enterprise_login_names_the_fix() {
    let source = source();
    let refusal = body_of(source, "fn enterprise_grant_failure");
    assert!(
        refusal.contains("--allow-host"),
        "the refusal spells the command that grants the hosts"
    );
    assert!(
        refusal.contains("enterprise_hosts"),
        "the refusal names the derived hosts rather than a generic denial"
    );
    assert!(
        source.contains("if domain == GITHUB_DOMAIN_DEFAULT"),
        "the default domain's failures are never rewritten as grant advice"
    );
}
