//! GitHub #302 (the 0.6.x half): an extension's own `net` request had no
//! interactive consent path.
//!
//! `webfetch` ships `[capabilities.net] hosts = []` on purpose — every host
//! arrives by consent or an ad hoc grant — but the capability layer
//! required a *non-empty* host list to consider `net` declared at all, so
//! the first fetch died with `NotGranted("no network capability declared")`
//! on the *declaration* gate, never reaching the pattern check where a
//! prompt could have helped. Even past that gate the deny was terminal:
//! the prompt machinery (`Action::Net`, the ad hoc grant store, FR-PERM-18
//! mid-session honour) lived in the tool/permission layer and was never
//! consulted for an extension's host-mediated fetch. `--allow-host` worked
//! only because it plants a grant before the check.
//!
//! Two fixes, both at the shared point:
//!
//! 1. **Declaration is the gate** (`CapabilityGrants::net_declared`,
//!    mirrored from `Manifest::net_declared`): "declared with no hosts yet"
//!    stays distinct from "never declared". An extension that never
//!    declared `net` still gets the flat `NotGranted` refusal.
//! 2. **Consent where the deny happens**: an ungranted host asks through
//!    `lca_permissions::authorize` — the same modal, grant store, deny-rule
//!    precedence and yolo answer everything else uses — and proceeds on an
//!    approval. Headless (nothing installed in the shared prompt slot)
//!    answers denied, so the error copy is unchanged there.
//!
//! The net capability's deny stays the enforcement backstop: a grant-less
//! call never goes out just because no prompt was reachable.
//!
//! This file pins the **engine** half — the grants shape and the four
//! answers through the real permission seam. The rows that drive
//! `webfetch::fetch` itself live in `extensions/webfetch/tests/consent.rs`,
//! with that crate's own dependencies: `webfetch` pulls `html2md`, whose
//! GPL-3.0+ license is outside `deny.toml`'s allow list, so it must not
//! enter this crate's dependency graph.
//!
//! Verifies: FR-PERM-16/18/19 (ad hoc attach, mid-session honour,
//! per-project persistence), FR-PERM-8 (always writes to the user store),
//! FR-PERM-5 (every refusal recorded), ADR-0039 (deny rules win), ADR-0042
//! (yolo answers the same prompt), NFR-13 (undeclared stays refused),
//! gh #302 acceptance 1-4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots, SharedPrompt,
};
use lca_tools::{Capabilities, CapabilityGrants};

/// Records every ask and answers with a fixed decision.
struct Recording {
    answer: Decision,
    asked: Arc<Mutex<Vec<String>>>,
}

impl PermissionPrompt for Recording {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.asked.lock().expect("asked").push(action.display());
        self.answer
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

struct Sandbox {
    root: std::path::PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = lca_testkit::scratch_path(&format!("lca-302reg-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["workspace", "private", "config", "data", "tmp"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Sandbox { root }
    }

    fn project(&self) -> std::path::PathBuf {
        self.root.join("workspace")
    }

    fn roots(&self) -> ScopeRoots {
        ScopeRoots {
            workspace: self.project(),
            private: self.root.join("private"),
            home_config: self.root.join("config"),
            temp: self.root.join("tmp"),
            state_dir: self.root.join("data"),
        }
    }

    /// The engine `webfetch`'s manifest produces (`net` declared, no
    /// pre-granted hosts), with `prompt` installed the way the interface
    /// installs its modal.
    fn engine(&self, prompt: Option<Arc<Mutex<dyn PermissionPrompt>>>) -> Arc<Capabilities> {
        let shared = SharedPrompt::default();
        if let Some(prompt) = prompt {
            shared.set(prompt);
        }
        Arc::new(Capabilities::new(
            "webfetch",
            declaration_only_grants(),
            self.roots(),
            Arc::new(Mutex::new(shared)),
            Arc::new(Mutex::new(
                GrantStore::open(&self.root.join("grants.json")).expect("store"),
            )),
            self.project(),
            None,
        ))
    }
}

/// The grants shape `webfetch::manifest_grants()` produces: `net`
/// declared, nothing pre-granted, nothing else reachable.
fn declaration_only_grants() -> CapabilityGrants {
    CapabilityGrants {
        net_declared: true,
        ..CapabilityGrants::default()
    }
}

/// The prompt handle the engine takes, and the list of asks it recorded.
type PromptAndAsks = (Arc<Mutex<dyn PermissionPrompt>>, Arc<Mutex<Vec<String>>>);

fn recording(answer: Decision) -> PromptAndAsks {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let prompt: Arc<Mutex<dyn PermissionPrompt>> = Arc::new(Mutex::new(Recording {
        answer,
        asked: asked.clone(),
    }));
    (prompt, asked)
}

/// A loopback server answering one request with `body`.
fn server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}"), handle)
}

fn drain(caps: &Capabilities, handle: u32) -> String {
    let mut out = Vec::new();
    while let Some(chunk) = caps.net_read_body(handle, 4096).expect("body chunk") {
        out.extend_from_slice(&chunk);
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Verifies: gh #302 acceptance 1 - the reported defect. Before the fix this
// failed at the declaration gate with `no network capability declared` and
// never asked anybody.
#[test]
fn an_ungranted_host_asks_then_fetches() {
    let sandbox = Sandbox::new("asks");
    let (url, srv) = server("consent-body");
    let (prompt, asked) = recording(Decision::Once);
    let caps = sandbox.engine(Some(prompt));

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("fetched after consent");
    assert_eq!(drain(&caps, handle), "consent-body");
    caps.net_close_response(handle).expect("close");

    let asked = asked.lock().expect("asked").clone();
    assert_eq!(asked.len(), 1, "asked exactly once: {asked:?}");
    assert!(
        asked[0].starts_with("connect to 127.0.0.1"),
        "the modal names the exact host (FR-PERM-16): {:?}",
        asked[0]
    );
    srv.join().expect("server");
}

// Verifies: gh #302 acceptance 1 - "always" persists the ad hoc grant and
// the next call is silent (FR-PERM-18 mid-session honour).
#[test]
fn allow_always_persists_and_silences_the_next_call() {
    let sandbox = Sandbox::new("always");
    let (url, srv) = server("always-body");
    let (prompt, asked) = recording(Decision::Always);
    let caps = sandbox.engine(Some(prompt));

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("allowed always");
    assert_eq!(drain(&caps, handle), "always-body");
    caps.net_close_response(handle).expect("close");
    assert_eq!(
        sandbox
            .engine(None)
            .grant_store()
            .lock()
            .expect("store")
            .net_patterns(&sandbox.project()),
        vec!["127.0.0.1".to_string()],
        "always writes the ad hoc net grant for this project"
    );

    let (url2, srv2) = server("second-body");
    let handle = caps
        .net_request_asking("GET", &url2, &[], None)
        .expect("silent the second time");
    assert_eq!(drain(&caps, handle), "second-body");
    caps.net_close_response(handle).expect("close");
    assert_eq!(
        asked.lock().expect("asked").len(),
        1,
        "the persisted grant silenced the second call"
    );
    srv.join().expect("server");
    srv2.join().expect("server");
}

// Verifies: gh #302 acceptance 2 - one-shot answers do not persist, so a
// fresh process asks again; this is "once", not "always".
#[test]
fn allow_once_persists_nothing() {
    let sandbox = Sandbox::new("once");
    let (url, srv) = server("once-body");
    let (prompt, _asked) = recording(Decision::Once);
    let caps = sandbox.engine(Some(prompt));

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("allowed once");
    assert_eq!(drain(&caps, handle), "once-body");
    caps.net_close_response(handle).expect("close");
    assert!(
        sandbox
            .engine(None)
            .grant_store()
            .lock()
            .expect("store")
            .net_patterns(&sandbox.project())
            .is_empty(),
        "once writes nothing to the user store"
    );
    srv.join().expect("server");
}

// Verifies: gh #302 acceptance 1 - deny keeps the honest error, names the
// host, sends nothing, and is recorded (FR-PERM-5).
#[test]
fn deny_keeps_the_honest_error_and_is_recorded() {
    let sandbox = Sandbox::new("deny");
    let (url, srv) = server("never");
    let (prompt, asked) = recording(Decision::Denied);
    let caps = sandbox.engine(Some(prompt));

    let err = caps
        .net_request_asking("GET", &url, &[], None)
        .expect_err("declined");
    assert_eq!(asked.lock().expect("asked").len(), 1, "the user was asked");
    let text = err.to_string();
    assert!(
        text.contains("matches no granted"),
        "the honest refusal survives: {text}"
    );
    assert!(text.contains("127.0.0.1"), "it names the host: {text}");
    assert!(
        caps.denials()
            .iter()
            .any(|denial| denial.parameter.contains("127.0.0.1")),
        "the refusal is recorded (FR-PERM-5)"
    );
    drop(srv);
}

// Verifies: gh #302 acceptance 2 - headless/unprompted is unchanged. The
// engine's slot carries a default `SharedPrompt`, whose empty slot answers
// denied exactly as headless installs it.
#[test]
fn headless_refuses_with_no_prompt_and_persists_nothing() {
    let sandbox = Sandbox::new("headless");
    let caps = sandbox.engine(None);

    let err = caps
        .net_request_asking("GET", "https://ungranted.example.com/x", &[], None)
        .expect_err("headless denies");
    assert!(
        err.to_string().contains("matches no granted"),
        "unchanged headless copy: {err}"
    );
    assert!(
        sandbox
            .engine(None)
            .grant_store()
            .lock()
            .expect("store")
            .net_patterns(&sandbox.project())
            .is_empty(),
        "a denial persists nothing"
    );
}

// Verifies: the seam split found by live testing. A **plain** `net_request`
// - what a provider's model discovery and login probes use - must never
// open a modal. Those are convenience calls whose callers already fall back
// on any error, several run on a background thread, and prompting there
// asked one question twice and left three `e2e_model_consent` rows hung
// behind a modal they never triggered.
#[test]
fn a_plain_net_request_never_opens_a_modal() {
    let sandbox = Sandbox::new("no-modal");
    let (prompt, asked) = recording(Decision::Always);
    let caps = sandbox.engine(Some(prompt));

    let err = caps
        .net_request("GET", "https://ungranted.example.com/x", &[], None)
        .expect_err("a plain request denies");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "a non-tool request must not prompt"
    );
    assert!(err.to_string().contains("matches no granted"), "{err}");
}

// Verifies: gh #302 acceptance 3 - a deny rule refuses without offering
// consent (ADR-0039), even with a permissive prompt installed.
#[test]
fn a_deny_rule_refuses_without_offering_consent() {
    let sandbox = Sandbox::new("denyrule");
    let (url, srv) = server("never");
    let (prompt, asked) = recording(Decision::Always);
    let shared = SharedPrompt::default();
    shared.set(prompt);
    let mut store = GrantStore::open(&sandbox.root.join("grants.json")).expect("store");
    store
        .add_rule(
            &sandbox.project(),
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Deny,
            "127.0.0.1",
        )
        .expect("rule");
    let caps = Capabilities::new(
        "webfetch",
        declaration_only_grants(),
        sandbox.roots(),
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(store)),
        sandbox.project(),
        None,
    );

    let err = caps
        .net_request_asking("GET", &url, &[], None)
        .expect_err("the rule refuses");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "a deny rule never reaches the prompt"
    );
    assert!(err.to_string().contains("matches no granted"), "{err}");
    drop(srv);
}

// Verifies: gh #302 acceptance 4 + NFR-13 - a manifest that never declared
// `net` is still refused at the boundary with no consent offered. The
// declaration gate must not become a way to ask for an undeclared
// capability.
#[test]
fn an_undeclared_manifest_never_offers_consent() {
    let sandbox = Sandbox::new("undeclared");
    let (prompt, asked) = recording(Decision::Always);
    let shared = SharedPrompt::default();
    shared.set(prompt);
    let caps = Capabilities::new(
        "webfetch",
        CapabilityGrants::default(),
        sandbox.roots(),
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(
            GrantStore::open(&sandbox.root.join("grants.json")).expect("store"),
        )),
        sandbox.project(),
        None,
    );

    let err = caps
        .net_request_asking("GET", "https://example.com/x", &[], None)
        .expect_err("undeclared refuses");
    assert!(
        matches!(err, lca_protocol::CapabilityError::NotGranted(_)),
        "the flat declaration refusal: {err}"
    );
    assert!(
        asked.lock().expect("asked").is_empty(),
        "never ask for an undeclared capability"
    );
}

// Verifies: ADR-0042 - yolo answers this prompt through the same path and
// persists like an always answer, so the store reads as if the user had
// approved.
#[test]
fn yolo_answers_and_persists_without_a_human() {
    let sandbox = Sandbox::new("yolo");
    let (url, srv) = server("yolo-body");
    let (prompt, asked) = recording(Decision::Denied);
    let shared = SharedPrompt::default();
    shared.set(prompt);
    let mut store = GrantStore::open(&sandbox.root.join("grants.json")).expect("store");
    store.set_permission_mode(lca_permissions::PermissionMode::Yolo);
    let caps = Capabilities::new(
        "webfetch",
        declaration_only_grants(),
        sandbox.roots(),
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(store)),
        sandbox.project(),
        None,
    );

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("yolo approves");
    assert_eq!(drain(&caps, handle), "yolo-body");
    caps.net_close_response(handle).expect("close");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "yolo answers without a human prompt"
    );
    assert_eq!(
        sandbox
            .engine(None)
            .grant_store()
            .lock()
            .expect("store")
            .net_patterns(&sandbox.project()),
        vec!["127.0.0.1".to_string()],
        "yolo's approval persists exactly like an always answer"
    );
    srv.join().expect("server");
}
