//! The Copilot device login against the mock gateway (gh #184):
//! the code page opens with the fragment, the poll waits out one
//! pending, and the mint stores the token pair.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod common;

use common::{mock_server, sandbox};

fn stored(cap: &Arc<lca_tools::Capabilities>, key: &str) -> Option<String> {
    lca_protocol::ProviderCap::credentials_get(cap.as_ref(), key)
}

// Verifies: gh #184 - the device flow opens the verification page
// with the user code in the fragment (the host's display seam),
// polls past one pending, and stores the GitHub token beside the
// minted Copilot token.
#[test]
fn device_login_mints_and_stores_the_token_pair() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "device",
        &mock,
        github_copilot::manifest_grants(),
    );
    let for_thread = cap.clone();
    type Outcome = Result<Vec<(String, String)>, String>;
    let result: Arc<Mutex<Option<Outcome>>> = Arc::new(Mutex::new(None));
    let slot = result.clone();
    std::thread::spawn(move || {
        let answer = lca_protocol::LoginAnswer {
            choice: github_copilot::CHOICE_DEVICE.to_string(),
            values: Default::default(),
        };
        let outcome =
            github_copilot::login_submit(for_thread.as_ref(), for_thread.as_ref(), &answer);
        *slot.lock().expect("slot") = Some(outcome);
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let opened = loop {
        if let Some(url) = cap.oauth_opened().last().cloned() {
            break url;
        }
        if Instant::now() > deadline {
            panic!("the device login never published its page");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        opened, "https://github.com/login/device#code=ABCD-1234",
        "the page opens with the code fragment"
    );

    let deadline = Instant::now() + Duration::from_secs(90);
    let outcome = loop {
        if let Some(outcome) = result.lock().expect("slot").take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("the device login never finished polling");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    outcome.expect("device login runs");
    assert_eq!(
        stored(&cap, "github_token"),
        Some("gh-device-token".to_string())
    );
    assert!(
        stored(&cap, "access").is_some_and(|token| token.contains("proxy-ep=")),
        "the minted token lands"
    );
    assert!(stored(&cap, "expires").is_some(), "an expiry lands");
    assert!(cap.denials().is_empty(), "everything was granted");
}

// Verifies: gh #184 - the token's `proxy-ep` routes inference: a
// minted token names the individual API host.
#[test]
fn the_token_proxy_ep_routes_inference() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "proxyep",
        &mock,
        github_copilot::manifest_grants(),
    );
    lca_protocol::ProviderCap::credentials_set(
        &*cap,
        "access",
        "tid=x;proxy-ep=proxy.individual.githubcopilot.com;",
    )
    .expect("seed");
    lca_protocol::ProviderCap::credentials_set(&*cap, "expires", &u64::MAX.to_string())
        .expect("seed");
    // No `api_base` override here: the point is the token routing.
    lca_protocol::ProviderCap::credentials_delete(&*cap, "api_base").expect("unseed");
    let request = lca_protocol::CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: "gpt-4o".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    // Pure build, no network: the URL is decided before any call.
    let parts = github_copilot::build_request(&*cap, &request).expect("builds without network");
    assert!(
        parts
            .0
            .starts_with("https://api.individual.githubcopilot.com/"),
        "the proxy host routes: {}",
        parts.0
    );
}

// ---- gh #282: an enterprise domain's hosts, and the field that carries it ----

// Verifies: gh #282 - the enterprise hosts are derived from the submitted
// field in one place, so the grant offer and the request path cannot
// disagree. GitHub's own device endpoints all live on the domain; the token
// mint lives on `api.<domain>`; inference resolves through `copilot-api.
// <domain>` (pi's `getGitHubCopilotBaseUrl` fallback for an enterprise
// account). These are the three hosts a login and its first turn touch.
#[test]
fn the_enterprise_hosts_are_derived_in_one_place() {
    assert_eq!(
        github_copilot::enterprise_hosts("ghe.corp.com"),
        vec![
            "ghe.corp.com".to_string(),
            "api.ghe.corp.com".to_string(),
            "copilot-api.ghe.corp.com".to_string(),
        ],
        "the login host, the mint host, and the inference host"
    );
}

// Verifies: gh #282 - github.com needs no ad-hoc grant: every host the
// flow touches is already manifest-declared (acceptance 2). This is the
// row that keeps the enterprise work from widening the default path's
// consent surface.
#[test]
fn github_com_needs_no_ad_hoc_hosts() {
    assert!(
        github_copilot::enterprise_hosts("github.com").is_empty(),
        "the default domain is fully declared"
    );
    assert!(
        github_copilot::enterprise_hosts("").is_empty(),
        "no domain means the default"
    );
    assert!(
        github_copilot::enterprise_hosts("  ").is_empty(),
        "blank means the default"
    );
}

// Verifies: gh #282 - an already-declared host never appears in the
// derived list. The unfiltered derivation for `api.githubcopilot.com` is
// `[api.githubcopilot.com, api.api.githubcopilot.com,
// copilot-api.api.githubcopilot.com]`; only the second two are new, so the
// offer carries exactly those. Without the filter the user would be asked
// to grant a host the shipped manifest already covers.
#[test]
fn a_declared_host_is_not_offered_again() {
    let hosts = github_copilot::enterprise_hosts("api.githubcopilot.com");
    assert!(
        !hosts.iter().any(|host| host == "api.githubcopilot.com"),
        "the manifest already covers it: {hosts:?}"
    );
    assert_eq!(
        hosts,
        vec![
            "api.api.githubcopilot.com".to_string(),
            "copilot-api.api.githubcopilot.com".to_string(),
        ],
        "exactly the hosts the manifest does not cover"
    );
}

// Verifies: gh #282 - the field is a hostname and nothing else. Every
// shape the brief names is refused with the hostname message, before it
// could reach a grant offer or a URL: a scheme, a path, a port,
// whitespace, or a wildcard.
#[test]
fn the_enterprise_field_must_be_a_hostname() {
    let message = github_copilot::ENTERPRISE_HOSTNAME_HELP;
    for bad in [
        "https://ghe.corp.com",
        "http://ghe.corp.com",
        "ghe.corp.com/",
        "ghe.corp.com/path",
        "ghe.corp.com:8443",
        "ghe.corp.com ",
        " ghe.corp.com",
        "ghe corp.com",
        "*.ghe.corp.com",
        "ghe.corp.com*",
        "user@ghe.corp.com",
        "",
        "   ",
        "-leading.hyphen",
        "double..dot",
        "trailing.dot.",
    ] {
        let got = github_copilot::parse_enterprise_domain(bad).unwrap_err();
        assert!(
            got.contains(message),
            "`{bad}` must be refused with the hostname message, got: {got}"
        );
    }
}

// Verifies: gh #282 - the shapes that ARE a hostname parse to the
// lowercase form, so a user typing mixed case still matches the
// manifest and the derived hosts consistently.
#[test]
fn a_plain_hostname_parses_and_lowercases() {
    assert_eq!(
        github_copilot::parse_enterprise_domain("ghe.corp.com").unwrap(),
        "ghe.corp.com"
    );
    assert_eq!(
        github_copilot::parse_enterprise_domain("GHE.Corp.COM").unwrap(),
        "ghe.corp.com",
        "lowercased, so it matches the manifest and the derived hosts"
    );
    assert_eq!(
        github_copilot::parse_enterprise_domain("github.com").unwrap(),
        "github.com"
    );
}

// Verifies: gh #282 - the hosts the login offers are the hosts the flow
// requests, and the inference host is the one `build_request` builds. This
// is the drift guard: a second derivation would let a grant cover
// `copilot-api.<d>` while inference asked something else.
#[test]
fn the_offered_hosts_are_the_hosts_the_flow_uses() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "enterprise-hosts",
        &mock,
        github_copilot::manifest_grants(),
    );
    // An enterprise account: the domain drives every URL, so clear the
    // mock's endpoint overrides.
    for key in [
        "api_base",
        "device_code_endpoint",
        "device_token_endpoint",
        "copilot_token_endpoint",
    ] {
        lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), key).expect("unseed");
    }
    lca_protocol::ProviderCap::credentials_set(cap.as_ref(), "enterprise_domain", "ghe.corp.com")
        .expect("seed domain");
    // A token with no `proxy-ep`, so `api_base` falls through to the
    // enterprise shape under test rather than the token's routing.
    lca_protocol::ProviderCap::credentials_set(cap.as_ref(), "access", "tid=enterprise")
        .expect("seed token");
    lca_protocol::ProviderCap::credentials_set(cap.as_ref(), "expires", &u64::MAX.to_string())
        .expect("seed expiry");

    let offered = github_copilot::enterprise_hosts("ghe.corp.com");
    assert!(!offered.is_empty(), "an enterprise domain needs grants");

    // The login leg: `domain()` drives the device endpoints, and every
    // offered host that the login touches must be one of them.
    assert!(
        offered.iter().any(|host| host == "ghe.corp.com"),
        "the login host is offered: {offered:?}"
    );
    assert!(
        offered.iter().any(|host| host == "api.ghe.corp.com"),
        "the mint host is offered: {offered:?}"
    );

    // The inference leg: `build_request`'s URL host must be an offered host
    // (the token's proxy-ep is absent here, so the enterprise shape wins).
    //
    // A behavioural assertion alone is too weak here: writing
    // `format!("https://copilot-api.{domain}")` inline produces the same
    // string, so the row would pass while a second derivation was back.
    // The source check below is what actually pins "one derivation".
    let request = lca_protocol::CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: "gpt-4o".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let (url, _, _) = github_copilot::build_request(cap.as_ref(), &request).expect("builds");
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("a host");
    assert!(
        offered.iter().any(|offered| offered == host),
        "inference host `{host}` must be one the login granted: {offered:?}"
    );
}

// Verifies: gh #282 - an invalid field fails the login at the field, before
// anything is stored and before any request leaves. Nothing partial lands:
// a refused domain must not leave `enterprise_domain` behind, because the
// next login would silently use it.
#[test]
fn a_refused_domain_stores_nothing_and_requests_nothing() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "refused-domain",
        &mock,
        github_copilot::manifest_grants(),
    );
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "enterprise-domain".to_string(),
        "https://ghe.corp.com/".to_string(),
    );
    let answer = lca_protocol::LoginAnswer {
        choice: github_copilot::CHOICE_DEVICE.to_string(),
        values,
    };
    let err = github_copilot::login_submit(cap.as_ref(), cap.as_ref(), &answer)
        .expect_err("a URL is not a hostname");
    assert!(
        err.contains(github_copilot::ENTERPRISE_HOSTNAME_HELP),
        "the refusal tells the user what to type: {err}"
    );
    assert_eq!(
        stored(&cap, "enterprise_domain"),
        None,
        "nothing partial is stored"
    );
    assert!(
        mock.requests_of("/login/device/code").is_empty(),
        "no request leaves for a refused domain"
    );
    assert!(cap.denials().is_empty(), "no capability denial was needed");
}

// Verifies: gh #282 - a valid enterprise domain is stored lowercased, so a
// later login and the derived hosts agree on the same spelling.
#[test]
fn a_valid_domain_is_stored_canonically() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "canonical-domain",
        &mock,
        github_copilot::manifest_grants(),
    );
    let mut values = std::collections::BTreeMap::new();
    values.insert("enterprise-domain".to_string(), "GHE.Corp.COM".to_string());
    let answer = lca_protocol::LoginAnswer {
        choice: github_copilot::CHOICE_DEVICE.to_string(),
        values,
    };
    // The flow itself cannot reach ghe.corp.com here, but the domain is
    // stored before the request: that is what this row pins.
    let _ = github_copilot::login_submit(cap.as_ref(), cap.as_ref(), &answer);
    assert_eq!(
        stored(&cap, "enterprise_domain"),
        Some("ghe.corp.com".to_string())
    );
}

// Verifies: gh #282 - an enterprise account whose hosts are not granted
// fails with the fix, not a dead end. The refusal names every derived host
// and the command that grants them, because the guest has no way to offer
// consent from inside the sandbox (the host's `net` offer runs after a
// successful submit, and this submit's first request is what was denied).
#[test]
fn an_ungranted_enterprise_domain_names_the_fix() {
    let mock = mock_server();
    // The mock's endpoint overrides are cleared, so the enterprise domain
    // really drives the URL, and the loopback grant does not cover it.
    let cap = sandbox(
        "github-copilot",
        "ungranted-enterprise",
        &mock,
        github_copilot::manifest_grants(),
    );
    for key in [
        "api_base",
        "device_code_endpoint",
        "device_token_endpoint",
        "copilot_token_endpoint",
    ] {
        lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), key).expect("unseed");
    }
    let mut values = std::collections::BTreeMap::new();
    values.insert("enterprise-domain".to_string(), "ghe.corp.com".to_string());
    let answer = lca_protocol::LoginAnswer {
        choice: github_copilot::CHOICE_DEVICE.to_string(),
        values,
    };
    let err = github_copilot::login_submit(cap.as_ref(), cap.as_ref(), &answer)
        .expect_err("the hosts are not granted");
    for host in github_copilot::enterprise_hosts("ghe.corp.com") {
        assert!(
            err.contains(&host),
            "the refusal names `{host}` so the user can grant it: {err}"
        );
        assert!(
            err.contains(&format!("--allow-host {host}")),
            "the refusal spells the fix for `{host}`: {err}"
        );
    }
    assert!(
        err.contains("interactive"),
        "and points at the interactive path that offers consent: {err}"
    );
}

// Verifies: gh #282 - the default domain's refusals are untouched. A
// github.com failure must read as itself (a live server's own answer, a DNS
// error), never as enterprise grant advice: the default flow has nothing to
// grant and acceptance 2 requires it to behave exactly as today.
#[test]
fn a_default_domain_failure_is_not_rewritten() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "default-domain",
        &mock,
        github_copilot::manifest_grants(),
    );
    // Force a refusal that is NOT a grant denial: the endpoint answers 404.
    mock.fail_once("/login/device/code", 404, Some("{\"error\":\"not_found\"}"));
    let answer = lca_protocol::LoginAnswer {
        choice: github_copilot::CHOICE_DEVICE.to_string(),
        values: Default::default(),
    };
    let err = github_copilot::login_submit(cap.as_ref(), cap.as_ref(), &answer)
        .expect_err("a 404 is a failure");
    assert!(
        !err.contains("--allow-host"),
        "no grant advice for the default domain: {err}"
    );
}

// Verifies: gh #282 - there is exactly one enterprise inference derivation.
// `api_base` must call the shared helper, not spell the URL out again: the
// two spellings agree today, which is precisely what makes a duplicated one
// dangerous - it can drift silently and take the granted host with it.
#[test]
fn the_inference_url_has_one_derivation() {
    let source = include_str!("../src/lib.rs");
    assert_eq!(
        source.matches("fn enterprise_inference_url").count(),
        1,
        "one derivation"
    );
    // Code occurrences only: the doc comments legitimately name the host.
    let in_code = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| line.contains("copilot-api."))
        .count();
    assert_eq!(
        in_code, 2,
        "the `copilot-api.` shape appears in code exactly twice - the helper's \
         format string and `enterprise_hosts`' candidate list, which builds the \
         same host from the same shape"
    );
    assert!(
        source.contains("return enterprise_inference_url(&domain);"),
        "`api_base` calls the shared helper"
    );
}

// Verifies: gh #282 - the guest-readable declared-host list matches the
// manifest it mirrors, in both directions. The list exists because the
// enterprise derivation runs inside the component, where neither
// `manifest_grants` (native-only) nor `lca_ext_host` is available; a
// second copy is only safe while a test fails the moment they disagree.
#[test]
fn the_declared_hosts_match_the_manifest() {
    let manifest: toml::Value = github_copilot::MANIFEST.parse().expect("MANIFEST parses");
    let from_file: Vec<String> = manifest["capabilities"]["net"]["hosts"]
        .as_array()
        .expect("hosts list")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    for host in &from_file {
        assert!(
            github_copilot::is_declared_host(host),
            "the manifest declares `{host}` but the guest-readable list does not"
        );
    }
    // No extra entries: a host in the list that the manifest does not
    // carry would silently suppress a grant offer.
    for host in github_copilot::DECLARED_HOSTS {
        assert!(
            from_file.iter().any(|declared| declared == host),
            "the list carries `{host}` which the manifest does not declare"
        );
    }
    assert_eq!(
        from_file.len(),
        github_copilot::DECLARED_HOSTS.len(),
        "same set, same size: {from_file:?}"
    );
}

// Verifies: gh #282 - a domain stored by an older build (which accepted a
// pasted URL) still drives the flow. The validator governs new writes; a
// credential written before it must keep working, or an upgrade would
// strand every existing enterprise account.
#[test]
fn a_legacy_stored_domain_still_routes() {
    let mock = mock_server();
    let cap = sandbox(
        "github-copilot",
        "legacy-domain",
        &mock,
        github_copilot::manifest_grants(),
    );
    for key in [
        "api_base",
        "device_code_endpoint",
        "device_token_endpoint",
        "copilot_token_endpoint",
    ] {
        lca_protocol::ProviderCap::credentials_delete(cap.as_ref(), key).expect("unseed");
    }
    // The shape the old code stored verbatim.
    lca_protocol::ProviderCap::credentials_set(
        cap.as_ref(),
        "enterprise_domain",
        "https://GHE.Corp.COM/",
    )
    .expect("seed legacy");
    lca_protocol::ProviderCap::credentials_set(cap.as_ref(), "access", "tid=enterprise")
        .expect("seed token");
    lca_protocol::ProviderCap::credentials_set(cap.as_ref(), "expires", &u64::MAX.to_string())
        .expect("seed expiry");
    let request = lca_protocol::CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: "gpt-4o".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let (url, _, _) = github_copilot::build_request(cap.as_ref(), &request).expect("builds");
    assert!(
        url.starts_with("https://copilot-api.ghe.corp.com/"),
        "the legacy value still resolves to the enterprise inference host: {url}"
    );
}
