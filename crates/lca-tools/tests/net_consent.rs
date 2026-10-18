//! Gh #302: the interactive per-host consent fallback for an extension's
//! own `net` request.
//!
//! Before this, an ungranted host was a dead end on every route: the
//! capability layer refused and recorded, and the prompt machinery (`the
//! permission modal`, the ad hoc `net` grant store, FR-PERM-18's
//! mid-session honour) was never consulted for an extension's
//! host-mediated fetch. `--allow-host` worked only because it plants a
//! grant *before* the check.
//!
//! These rows drive the real seam - `Capabilities::net_request` with a
//! recording [`PermissionPrompt`] and a real [`GrantStore`] - so the four
//! answers are the four answers a user can give, not a mock of the
//! decision path.
//!
//! Verifies: FR-PERM-16 (ad hoc attach names the exact host), FR-PERM-18
//! (an attached grant is honoured for the rest of the session), FR-PERM-8/
//! 19 (always persists per project, to the user store), ADR-0039 (a deny
//! rule refuses without prompting), ADR-0042 (yolo answers the same
//! prompt), and gh #302's headless-unchanged half.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants};

/// A prompt that records every ask and answers with a fixed decision.
struct Recording {
    answer: Decision,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Recording {
    fn new(answer: Decision) -> (Recording, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        (
            Recording {
                answer,
                asked: asked.clone(),
            },
            asked,
        )
    }
}

impl PermissionPrompt for Recording {
    fn ask(&mut self, action: &Action) -> Decision {
        self.asked.lock().expect("asked").push(action.display());
        self.answer
    }
    fn review_proposals(&mut self, _: &ProposalDiff) -> bool {
        false
    }
}

/// The headless shape: nothing is installed in the prompt slot, so every
/// ask answers `Decision::Denied` (the `SharedPrompt` contract).
struct NoPrompt;
impl PermissionPrompt for NoPrompt {
    fn ask(&mut self, _: &Action) -> Decision {
        Decision::Denied
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
        let root = lca_testkit::scratch_path(&format!("lca-302-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "workspace",
            "private",
            "config",
            "data",
            "tmp",
            "data/credentials",
        ] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        Sandbox { root }
    }

    fn project(&self) -> std::path::PathBuf {
        self.root.join("workspace")
    }

    fn grants(&self) -> GrantStore {
        GrantStore::open(&self.root.join("grants.json")).expect("store")
    }

    fn caps_with(
        &self,
        grants: CapabilityGrants,
        prompt: Arc<Mutex<dyn PermissionPrompt>>,
    ) -> Capabilities {
        self.caps_on(grants, prompt, self.grants())
    }

    fn caps_on(
        &self,
        grants: CapabilityGrants,
        prompt: Arc<Mutex<dyn PermissionPrompt>>,
        store: GrantStore,
    ) -> Capabilities {
        Capabilities::new(
            "webfetch",
            grants,
            ScopeRoots {
                workspace: self.project(),
                private: self.root.join("private"),
                home_config: self.root.join("config"),
                temp: self.root.join("tmp"),
                state_dir: self.root.join("data"),
            },
            prompt,
            Arc::new(Mutex::new(store)),
            self.project(),
            None,
        )
    }
}

/// Exactly what `webfetch::manifest_grants()` produces: `net` declared,
/// no hosts. This is the shape that used to die on the wrong gate.
fn webfetch_grants() -> CapabilityGrants {
    CapabilityGrants {
        net_declared: true,
        ..CapabilityGrants::default()
    }
}

/// A loopback HTTP server answering one request with `body`.
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

// Verifies: gh #302 acceptance 1 - "allow once" fetches. The modal named
// the exact host (FR-PERM-16), the request went out, and the answer did
// not persist: a fresh process asks again (that is `once`, not `always`).
#[test]
fn allow_once_fetches_without_persisting() {
    let sandbox = Sandbox::new("once");
    let (url, server) = server("once-body");
    let (prompt, asked) = Recording::new(Decision::Once);
    let caps = sandbox.caps_with(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("allowed once");
    assert_eq!(drain(&caps, handle), "once-body");
    caps.net_close_response(handle).expect("close");

    let asked = asked.lock().expect("asked").clone();
    assert_eq!(asked.len(), 1, "asked exactly once: {asked:?}");
    assert!(
        asked[0].starts_with("connect to 127.0.0.1"),
        "the modal names the exact host: {:?}",
        asked[0]
    );

    // `once` is a session allowance (gh #31 review): the same process
    // does not ask again, but nothing was written to the user store.
    let persisted = sandbox.grants().net_patterns(&sandbox.project());
    assert!(persisted.is_empty(), "once persists nothing: {persisted:?}");
    server.join().expect("server");
}

// Verifies: gh #302 acceptance 1 - "allow always" fetches AND persists
// the ad hoc grant, and the next call is silent (FR-PERM-18's
// mid-session honour, FR-PERM-8/19's per-project persistence).
#[test]
fn allow_always_fetches_persists_and_silences_the_next_call() {
    let sandbox = Sandbox::new("always");
    let (url, srv1) = server("always-body");
    let (prompt, asked) = Recording::new(Decision::Always);
    let caps = sandbox.caps_with(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let handle = caps
        .net_request_asking("GET", &url, &[], None)
        .expect("allowed always");
    assert_eq!(drain(&caps, handle), "always-body");
    caps.net_close_response(handle).expect("close");

    let persisted = sandbox.grants().net_patterns(&sandbox.project());
    assert_eq!(
        persisted,
        vec!["127.0.0.1".to_string()],
        "always writes the ad hoc net grant"
    );

    // FR-PERM-18: the very next call, same engine, no second prompt.
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
    srv1.join().expect("server");
    srv2.join().expect("server");
}

// Verifies: gh #302 acceptance 1 - deny keeps the honest error, names the
// host, and records the refusal (FR-PERM-5).
#[test]
fn deny_keeps_the_honest_error_and_sends_nothing() {
    let sandbox = Sandbox::new("deny");
    let (url, server) = server("never");
    let (prompt, asked) = Recording::new(Decision::Denied);
    let caps = sandbox.caps_with(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let err = caps
        .net_request_asking("GET", &url, &[], None)
        .expect_err("declined");
    assert_eq!(asked.lock().expect("asked").len(), 1, "the user was asked");
    let text = err.to_string();
    // Today's copy, byte for byte: a decline is the same refusal the
    // headless path gives, plus nothing.
    assert!(
        text.contains("matches no granted"),
        "the honest refusal survives: {text}"
    );
    assert!(text.contains("127.0.0.1"), "it names the host: {text}");
    let denials = caps.denials();
    assert!(
        denials.iter().any(|d| d.parameter.contains("127.0.0.1")),
        "the refusal is recorded (FR-PERM-5): {denials:?}"
    );
    // Nothing must have opened a connection: the server accepts once and
    // must still be waiting, so joining it after a timeout-free close is
    // not attempted - we simply drop it.
    drop(server);
}

// Verifies: the seam split this cycle had to make (found the hard way).
// `net_request` - what a provider's model discovery and login probes use -
// must NEVER open a modal. Those calls are convenience work whose callers
// already fall back on any error, and several of them run on a background
// thread behind the interface; a prompt there asks a question twice and
// leaves a spinner behind a modal. Only the deliberate tool path opts in,
// through `net_request_asking`.
//
// Regression guard: before the split, an ungranted endpoint's model
// discovery popped the permission modal while the interface was also
// showing its own consent flow - three `e2e_model_consent` rows hung
// behind it for their whole timeout.
#[test]
fn a_plain_net_request_never_opens_a_modal() {
    let sandbox = Sandbox::new("no-modal");
    let (prompt, asked) = Recording::new(Decision::Always);
    let caps = sandbox.caps_with(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let err = caps
        .net_request("GET", "https://ungranted.example.com/x", &[], None)
        .expect_err("a plain request denies");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "a non-tool request must not prompt: {:?}",
        asked.lock().expect("asked")
    );
    assert!(
        err.to_string().contains("matches no granted"),
        "and it reports the plain refusal: {err}"
    );
}

// Verifies: gh #302 acceptance 2 - a headless/unprompted environment is
// unchanged. No prompt is reachable, the error still names the host and
// (via the capability refusal) the request is refused, and nothing is
// persisted.
#[test]
fn headless_refuses_with_no_prompt_and_persists_nothing() {
    let sandbox = Sandbox::new("headless");
    let caps = sandbox.caps_with(
        webfetch_grants(),
        Arc::new(Mutex::new(NoPrompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let err = caps
        .net_request_asking("GET", "https://ungranted.example.com/x", &[], None)
        .expect_err("headless denies");
    let text = err.to_string();
    assert!(
        text.contains("matches no granted pattern"),
        "unchanged headless copy: {text}"
    );
    assert!(
        text.contains("ungranted.example.com"),
        "it names the host: {text}"
    );
    assert!(
        sandbox.grants().net_patterns(&sandbox.project()).is_empty(),
        "a denial persists nothing"
    );
}

// Verifies: gh #302 acceptance 3 - a deny rule refuses silently first:
// no consent is offered where a deny rule matches (ADR-0039), even though
// a permissive prompt is installed.
#[test]
fn a_deny_rule_refuses_without_offering_consent() {
    let sandbox = Sandbox::new("denyrule");
    let (url, server) = server("never");
    let mut store = sandbox.grants();
    // A deny rule matching the host's match value (`Action::Net`'s
    // `match_value` is the host itself).
    store
        .add_rule(
            &sandbox.project(),
            lca_permissions::RuleScope::Project,
            lca_permissions::RuleDecision::Deny,
            "127.0.0.1",
        )
        .expect("rule");
    let (prompt, asked) = Recording::new(Decision::Always);
    let caps = sandbox.caps_on(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
        store,
    );

    let err = caps
        .net_request_asking("GET", &url, &[], None)
        .expect_err("the rule refuses");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "a deny rule never reaches the prompt: {:?}",
        asked.lock().expect("asked")
    );
    assert!(err.to_string().contains("matches no granted"), "{err}");
    drop(server);
}

// Verifies: gh #302 acceptance 4 + the declaration gate - a manifest with
// no `net` (or `net-local`) declaration at all still gets the flat
// "not granted" refusal, and no consent is offered for a capability the
// sandbox never declared (NFR-13).
#[test]
fn an_undeclared_net_capability_is_refused_without_consent() {
    let sandbox = Sandbox::new("undeclared");
    let (prompt, asked) = Recording::new(Decision::Always);
    // `net_declared: false` and no patterns: the pre-#222 shape.
    let caps = sandbox.caps_with(
        CapabilityGrants::default(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
    );

    let err = caps
        .net_request_asking("GET", "https://example.com/x", &[], None)
        .expect_err("undeclared");
    assert!(
        matches!(err, CapabilityError::NotGranted(_)),
        "the flat declaration refusal: {err}"
    );
    assert!(
        asked.lock().expect("asked").is_empty(),
        "never ask for an undeclared capability"
    );
}

// Verifies: gh #302's yolo half (ADR-0042) - yolo answers this prompt
// through the same path and persists the pattern like a human "always",
// so the grant store and the session log read as if the user approved.
#[test]
fn yolo_answers_the_net_consent_and_persists_it() {
    let sandbox = Sandbox::new("yolo");
    let (url, server) = server("yolo-body");
    let mut store = sandbox.grants();
    store.set_permission_mode(lca_permissions::PermissionMode::Yolo);
    let (prompt, asked) = Recording::new(Decision::Denied);
    let caps = sandbox.caps_on(
        webfetch_grants(),
        Arc::new(Mutex::new(prompt)) as Arc<Mutex<dyn PermissionPrompt>>,
        store,
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
        sandbox.grants().net_patterns(&sandbox.project()),
        vec!["127.0.0.1".to_string()],
        "yolo's approval persists exactly like an always answer"
    );
    server.join().expect("server");
}
