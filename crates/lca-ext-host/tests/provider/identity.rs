//! ADR-0012's identity surface in both delivery modes: the exports themselves,
//! the loopback OAuth/credentials flows, their cancellation, and the declarations
//! a missing grant refuses through (ADR-0026, NFR-21, FR-PERM-6/7).
//!
//! Gh #552 split `tests/provider.rs` at its own subjects; the `Fixture` is defined
//! once, in `support.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits};
use lca_protocol::IdentityOutcome;

use super::support::{FIXTURE, Fixture, MANIFEST};

// Verifies: ADR-0012 — logout and usage export in both modes with
// identical outcomes, including the not-supported path. (`login` is
// exercised with callback injection in `oauth_and_credentials_match_across_modes`.)
#[tokio::test]
async fn identity_exports_are_identical_across_modes() {
    let fixture = Fixture::new("identity");
    let (wasm, native) = fixture.both_modes();

    let wasm_logout = wasm.identity_logout().await.expect("wasm logout");
    let native_logout = native.identity_logout().await.expect("native logout");
    assert_eq!(wasm_logout, native_logout);
    assert_eq!(
        wasm_logout,
        IdentityOutcome::NotSupported,
        "ADR-0012's optional-export path"
    );

    let wasm_usage = wasm.identity_usage().await.expect("wasm usage");
    let native_usage = native.identity_usage().await.expect("native usage");
    assert_eq!(wasm_usage, native_usage);
    let usage = wasm_usage.expect("conformance reports usage");
    let scripted = conformance::scripted_usage_report();
    assert_eq!(usage.input, scripted.input);
    assert_eq!(usage.cache_read, scripted.cache_read);
    assert_eq!(usage.cost, scripted.cost);
}

// Verifies: NFR-25, FR-PERM-6/FR-PERM-7 (credentials), FR-PROV-3/FR-PROV-4
// (the loopback oauth flow) at the WASM boundary: the conformance probe's
// `login` runs `credentials.set/get/delete` and
// `oauth.begin/open/await-callback/end-flow` through the host imports, and
// the native twin reaches the same outcome. The test injects the callback
// itself (the guest cannot reach the loopback port).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_and_credentials_match_across_modes() {
    let fixture = Fixture::new("identity-oauth");
    let wasm = fixture.wasm_mode();
    let cap = wasm.capabilities();
    // The browser launcher is a recorder, so the host opens nothing.
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = opened.clone();
    cap.set_browser_opener(Some(Arc::new(move |url| {
        recorder.lock().expect("opened").push(url.to_string());
        Ok(())
    })));
    let native = conformance::NativeConformance::new(cap.clone());

    let wasm_login = login_with_callback(&wasm, &cap).await;
    assert_eq!(wasm_login, IdentityOutcome::Ok, "wasm login");
    let native_login = login_with_callback(&native, &cap).await;
    assert_eq!(native_login, IdentityOutcome::Ok, "native login");
    assert_eq!(wasm_login, native_login);

    // The probe set then deleted its credential: the namespace file has no
    // trace of it (FR-PERM-6/FR-PERM-7).
    let creds = fixture.root.join("data/credentials/conformance.json");
    let text = std::fs::read_to_string(&creds).unwrap_or_default();
    assert!(
        !text.contains("probe"),
        "credential survived delete: {text}"
    );

    // `oauth.open` recorded the authorization URL (FR-PROV-3's browser half).
    let opened = opened.lock().expect("opened");
    assert_eq!(opened.len(), 2, "one open per login: {opened:?}");
    assert!(opened.iter().all(|url| url == conformance::AUTHORIZE_URL));
    assert_eq!(
        cap.oauth_opened(),
        vec![
            conformance::AUTHORIZE_URL.to_string(),
            conformance::AUTHORIZE_URL.to_string()
        ]
    );
}

// Verifies: NFR-21 (cancellation reaches a running extension call) at the
// blocking host boundary: an extension waiting in `oauth.await-callback`
// returns promptly when the host interrupts it, instead of sitting until the
// callback window elapses. An epoch bump alone cannot reach blocked host
// code, so this is the case that closes NFR-21 for host waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_a_blocked_oauth_wait_returns_promptly() {
    let fixture = Fixture::new("oauth-cancel");
    let wasm = fixture.wasm_mode();
    let cap = wasm.capabilities();

    let before = cap.oauth_begun().len();
    let task = tokio::spawn(wasm.identity_login());
    // Wait until the login is actually blocked in `oauth_await` (its flow has
    // bound the listener); interrupting earlier would trap at the call site
    // and prove nothing about the blocked wait.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while cap.oauth_begun().len() == before {
        assert!(
            std::time::Instant::now() < deadline,
            "the login never bound an oauth flow"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let start = std::time::Instant::now();
    wasm.interrupt();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the cancelled login returns in under 5s, not the 30s window")
        .expect("the login task joins");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "cancellation returns within the NFR-21 neighbourhood, took {:?}",
        start.elapsed()
    );
    // Either the guest handled the cancelled host call (`Failed`) or the
    // epoch trap surfaced as a dispatch error; both are prompt returns.
    assert!(
        matches!(joined, Err(_) | Ok(IdentityOutcome::Failed(_))),
        "the cancelled login did not report a cancellation: {joined:?}"
    );
}

// Verifies: NFR-21 in the native delivery mode (the same rule as the WASM
// case, exercised through the native handle's `interrupt`): a native call
// blocked in `oauth.await-callback` returns promptly when the host
// interrupts it, because `Capabilities::cancel` is what the wait polls and
// `NativeConformance::interrupt` sets it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupting_a_blocked_native_oauth_wait_returns_promptly() {
    let fixture = Fixture::new("oauth-cancel-native");
    let cap = fixture.capabilities();
    let native: Arc<dyn ExtensionDispatch> =
        Arc::new(conformance::NativeConformance::new(cap.clone()));

    let before = cap.oauth_begun().len();
    let login_handle = native.clone();
    let task = tokio::spawn(async move { login_handle.identity_login().await });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while cap.oauth_begun().len() == before {
        assert!(
            std::time::Instant::now() < deadline,
            "the native login never bound an oauth flow"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let start = std::time::Instant::now();
    native.interrupt();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the cancelled native login returns in under 5s, not the 30s window")
        .expect("the login task joins");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "native cancellation returns within the NFR-21 neighbourhood, took {:?}",
        start.elapsed()
    );
    assert!(
        matches!(joined, Ok(IdentityOutcome::Failed(_))),
        "the cancelled native login did not report a cancellation: {joined:?}"
    );
}

/// Drive one identity-login call to completion: spawn it (the call blocks in
/// `oauth.await-callback`), wait for the flow to bind, inject the loopback
/// callback, and return the outcome.
async fn login_with_callback(
    handle: &dyn ExtensionDispatch,
    cap: &lca_tools::Capabilities,
) -> IdentityOutcome {
    let before = cap.oauth_begun().len();
    let login = handle.identity_login();
    let task = tokio::spawn(login);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = loop {
        if let Some(url) = cap.oauth_begun().get(before) {
            break url.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the oauth flow never bound a callback URL"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    inject_callback(&url, conformance::FIXTURE_CODE, "fixture-state");
    task.await.expect("login task").expect("login dispatch")
}

/// One loopback GET with no dependency: the host's listener only reads the
/// request line, so no response read is needed.
fn inject_callback(url: &str, code: &str, state: &str) {
    use std::io::Write;
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .expect("the flow binds a loopback URL");
    let (port, path) = rest
        .split_once('/')
        .map(|(port, tail)| (port, format!("/{tail}")))
        .expect("a redirect path");
    let port = port.parse::<u16>().expect("a bound port");
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect the callback listener");
    let request = format!(
        "GET {path}?code={code}&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the callback request");
    let _ = stream.flush();
}

/// A native capability engine with exactly the identity grants asked for.
fn capabilities_with(
    fixture: &Fixture,
    credentials: bool,
    oauth: bool,
) -> Arc<lca_tools::Capabilities> {
    Arc::new(lca_tools::Capabilities::new(
        "conformance",
        lca_tools::CapabilityGrants {
            credentials,
            oauth: oauth.then(|| lca_permissions::OAuthSettings {
                redirect_path: "/callback".to_string(),
                timeout_seconds: 30,
            }),
            ..Default::default()
        },
        fixture.roots.clone(),
        fixture.prompt.clone(),
        fixture.store.clone(),
        fixture.root.join("project"),
        None,
    ))
}

/// The conformance manifest with the identity grants stripped: the denied
/// state ADR-0026 links in.
fn manifest_without_identity_grants() -> String {
    let mut text = MANIFEST.to_string();
    for block in [
        "[capabilities.net]\nhosts = [\"conformance.example.com\"]\n\n",
        "[capabilities.oauth]\nredirect_path = \"/callback\"\ntimeout_seconds = 30\n\n",
        "[capabilities.credentials]\nnamespace = \"conformance\"\n\n",
    ] {
        text = text.replace(block, "");
    }
    text
}

// Verifies: ADR-0026 + NFR-25 — the denied state is a refusal, not a crash,
// at both boundaries, and each refusal is recorded as a denial. Closes the
// last NFR-25 surface (the credentials and oauth host imports).
#[tokio::test]
async fn denied_identity_capabilities_refuse_at_both_boundaries() {
    let fixture = Fixture::new("identity-denied");

    // Native: credentials undeclared refuses at the first call.
    let no_creds = conformance::NativeConformance::new(capabilities_with(&fixture, false, true));
    assert!(
        matches!(
            no_creds.identity_login().await.expect("dispatch"),
            IdentityOutcome::Failed(_)
        ),
        "an undeclared credentials capability refuses"
    );

    // Native: credentials declared but oauth undeclared refuses at the flow.
    let no_oauth = conformance::NativeConformance::new(capabilities_with(&fixture, true, false));
    assert!(
        matches!(
            no_oauth.identity_login().await.expect("dispatch"),
            IdentityOutcome::Failed(_)
        ),
        "an undeclared oauth capability refuses"
    );

    // WASM: the same manifest without the grants refuses through the imports
    // and records a denial.
    let wasm = {
        let mut host = ExtHost::new(
            ExtensionLimits {
                memory_bytes: 64 * 1024 * 1024,
                fuel_per_call: 100_000_000,
                log_limit_bytes: 4096,
            },
            fixture.env(),
        );
        host.load(FIXTURE, &manifest_without_identity_grants())
            .expect("load denied mode")
    };
    let outcome = wasm.identity_login().await.expect("wasm dispatch");
    assert!(
        matches!(outcome, IdentityOutcome::Failed(_)),
        "the denied WASM call refuses: {outcome:?}"
    );
    assert!(
        wasm.denial_count() > 0,
        "the refusal is recorded as a denial (FR-EXT-9)"
    );
}
