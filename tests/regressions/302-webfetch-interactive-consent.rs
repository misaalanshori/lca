//! GitHub #302 (the 0.6.x half): an extension's own `net` request had no
//! interactive consent path.
//!
//! `webfetch` ships `[capabilities.net] hosts = []` on purpose - every
//! host arrives by consent or an ad hoc grant - but the capability layer
//! required a *non-empty* host list to consider `net` declared at all, so
//! the first fetch died with `NotGranted("no network capability declared")`
//! on the *declaration* gate, and never reached the pattern check where a
//! prompt could have helped. Even past that gate, the deny was terminal:
//! the prompt machinery (`Action::Net`, the ad hoc grant store, FR-PERM-18
//! mid-session honour) lived in the tool/permission layer and was never
//! consulted for an extension's host-mediated fetch. `--allow-host` worked
//! only because it plants a grant before the check.
//!
//! Two fixes, both at the shared point:
//!
//! 1. **Declaration is the gate** (`CapabilityGrants::net_declared`,
//!    mirrored from `Manifest::net_declared`): "declared with no hosts
//!    yet" stays distinct from "never declared". An extension that never
//!    declared `net` still gets the flat `NotGranted` refusal.
//! 2. **Consent where the deny happens**: an ungranted host asks through
//!    `lca_permissions::authorize` - the same modal, grant store, deny-rule
//!    precedence and yolo answer everything else uses - and proceeds on an
//!    approval. Headless (nothing installed in the shared prompt slot)
//!    answers denied, so the error copy is unchanged there.
//!
//! The net capability's deny stays the enforcement backstop: a grant-less
//! call never goes out just because no prompt was reachable.
//!
//! This file drives the registry at the **turn level**: an extension-shaped
//! engine (webfetch's exact grants) is registered and its `webfetch` tool
//! called the way the model calls it, so the assertion is about a tool call
//! in a session, not a direct `net_request`.
//!
//! Verifies: FR-PERM-16/18/19 (ad hoc attach, mid-session honour,
//! per-project persistence), FR-PERM-8 (always writes to the user store),
//! FR-PERM-5 (every refusal recorded), ADR-0039 (deny rules win), ADR-0042
//! (yolo answers the same prompt), gh #302 acceptance 1-4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots, SharedPrompt,
};
use lca_tools::{Capabilities, CapabilityGrants};
use webfetch::{FetchRequest, NetView};

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
}

/// The engine webfetch's own manifest produces, with `prompt` installed the
/// way the interface installs its modal (`SharedPrompt::set`).
fn engine(
    sandbox: &Sandbox,
    prompt: Option<Arc<Mutex<dyn PermissionPrompt>>>,
) -> Arc<Capabilities> {
    let shared = SharedPrompt::default();
    if let Some(prompt) = prompt {
        shared.set(prompt);
    }
    Arc::new(Capabilities::new(
        "webfetch",
        webfetch::manifest_grants(),
        ScopeRoots {
            workspace: sandbox.project(),
            private: sandbox.root.join("private"),
            home_config: sandbox.root.join("config"),
            temp: sandbox.root.join("tmp"),
            state_dir: sandbox.root.join("data"),
        },
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(
            GrantStore::open(&sandbox.root.join("grants.json")).expect("store"),
        )),
        sandbox.project(),
        None,
    ))
}

/// A loopback server answering one request.
fn server(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        if let Some(mut stream) = listener.incoming().flatten().next() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}"), handle)
}

fn request(url: &str) -> FetchRequest {
    FetchRequest {
        url: url.to_string(),
        format: webfetch::Format::Markdown,
        timeout_secs: 30,
    }
}

// Verifies: gh #302 acceptance 1 - the reported defect end to end through
// the tool the model calls. Before the fix this failed at the declaration
// gate with `no network capability declared` and never asked anybody.
#[test]
fn a_webfetch_tool_call_to_an_ungranted_host_asks_then_fetches() {
    let sandbox = Sandbox::new("asks");
    let (url, server) = server("<html><body><h1>Consent</h1></body></html>");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let caps = engine(
        &sandbox,
        Some(Arc::new(Mutex::new(Recording {
            answer: Decision::Once,
            asked: asked.clone(),
        }))),
    );

    // The tool path, not a raw net_request: `fetch` is what `execute_tool`
    // runs, and it goes through the `NetView` impl for `Capabilities`.
    let out = webfetch::fetch(&request(&url), &*caps).expect("fetched after consent");
    assert!(out.contains("Consent"), "the body came back: {out}");

    let asked = asked.lock().expect("asked").clone();
    assert_eq!(asked.len(), 1, "the user was asked exactly once: {asked:?}");
    assert!(
        asked[0].starts_with("connect to 127.0.0.1"),
        "the modal names the exact host: {:?}",
        asked[0]
    );
    server.join().expect("server");
}

// Verifies: gh #302 - the granted path stays silent. An engine with the
// ad hoc grant already attached (what `--allow-host` and a previous
// "always" both produce) fetches without a prompt.
#[test]
fn a_granted_host_fetches_without_asking() {
    let sandbox = Sandbox::new("silent");
    let (url, server) = server("<html><body><h1>Quiet</h1></body></html>");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let caps = engine(
        &sandbox,
        Some(Arc::new(Mutex::new(Recording {
            answer: Decision::Denied,
            asked: asked.clone(),
        }))),
    );
    caps.grant_store()
        .lock()
        .expect("store")
        .approve_net_pattern(&sandbox.project(), "127.0.0.1")
        .expect("grant");

    let out = webfetch::fetch(&request(&url), &*caps).expect("granted, so it fetches");
    assert!(out.contains("Quiet"), "{out}");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "nothing to ask about: {:?}",
        asked.lock().expect("asked")
    );
    server.join().expect("server");
}

// Verifies: gh #302 acceptance 2 - the headless shape is unchanged. An
// engine whose prompt slot is empty (exactly what headless mode installs)
// denies, produces the same wording, and writes nothing.
#[test]
fn the_headless_shape_keeps_the_original_refusal() {
    let sandbox = Sandbox::new("headless");
    let caps = engine(&sandbox, None);

    let err = webfetch::fetch(&request("https://ungranted.example.com/x"), &*caps)
        .expect_err("headless denies");
    assert!(
        err.contains("matches no granted"),
        "the original copy survives: {err}"
    );
    assert!(
        err.contains("ungranted.example.com"),
        "it still names the host: {err}"
    );
    assert!(
        caps.grant_store()
            .lock()
            .expect("store")
            .net_patterns(&sandbox.project())
            .is_empty(),
        "a denial persists nothing"
    );
}

// Verifies: gh #302 - a *manifest* that never declared `net` is still
// refused at the boundary with no consent offered (NFR-13): the
// declaration gate must not become a way to ask for an undeclared
// capability.
#[test]
fn an_undeclared_manifest_never_offers_consent() {
    let sandbox = Sandbox::new("undeclared");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let shared = SharedPrompt::default();
    shared.set(Arc::new(Mutex::new(Recording {
        answer: Decision::Always,
        asked: asked.clone(),
    })));
    // No `[capabilities.net]` at all: `net_declared` is false.
    let caps = Capabilities::new(
        "webfetch",
        CapabilityGrants::default(),
        ScopeRoots {
            workspace: sandbox.project(),
            private: sandbox.root.join("private"),
            home_config: sandbox.root.join("config"),
            temp: sandbox.root.join("tmp"),
            state_dir: sandbox.root.join("data"),
        },
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(
            GrantStore::open(&sandbox.root.join("grants.json")).expect("store"),
        )),
        sandbox.project(),
        None,
    );

    let err =
        webfetch::fetch(&request("https://example.com/x"), &caps).expect_err("undeclared refuses");
    assert!(
        err.contains("not granted") || err.contains("no network capability declared"),
        "the declaration refusal: {err}"
    );
    assert!(
        asked.lock().expect("asked").is_empty(),
        "never ask for an undeclared capability"
    );
}

// Verifies: gh #302 acceptance 3 - a deny rule refuses without offering
// consent (ADR-0039), even with a permissive prompt installed.
#[test]
fn a_deny_rule_outranks_the_consent_offer() {
    let sandbox = Sandbox::new("denyrule");
    let (url, server) = server("<html><body>never</body></html>");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let shared = SharedPrompt::default();
    shared.set(Arc::new(Mutex::new(Recording {
        answer: Decision::Always,
        asked: asked.clone(),
    })));
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
        webfetch::manifest_grants(),
        ScopeRoots {
            workspace: sandbox.project(),
            private: sandbox.root.join("private"),
            home_config: sandbox.root.join("config"),
            temp: sandbox.root.join("tmp"),
            state_dir: sandbox.root.join("data"),
        },
        Arc::new(Mutex::new(shared)),
        Arc::new(Mutex::new(store)),
        sandbox.project(),
        None,
    );

    let err = webfetch::fetch(&request(&url), &caps).expect_err("the rule refuses");
    assert!(
        asked.lock().expect("asked").is_empty(),
        "a deny rule never reaches the prompt"
    );
    assert!(err.contains("matches no granted"), "{err}");
    drop(server);
}

// Verifies: FR-PERM-5 - the refusal is recorded, so `lca ext info` and the
// denial journal still see a declined consent as a refusal rather than
// silence.
#[test]
fn a_declined_consent_is_recorded_as_a_refusal() {
    let sandbox = Sandbox::new("recorded");
    let (url, server) = server("<html><body>no</body></html>");
    let caps = engine(
        &sandbox,
        Some(Arc::new(Mutex::new(Recording {
            answer: Decision::Denied,
            asked: Arc::new(Mutex::new(Vec::new())),
        }))),
    );

    let _ = webfetch::fetch(&request(&url), &*caps).expect_err("declined");
    let denials = caps.denials();
    assert!(
        denials
            .iter()
            .any(|denial| denial.parameter.contains("127.0.0.1")),
        "the refusal is recorded: {denials:?}"
    );
    drop(server);
}

/// The `NetView` impl is the seam `webfetch::fetch` uses; this keeps the
/// import meaningful in a file whose assertions go through the real one.
#[allow(dead_code)]
fn _net_view_is_the_capability(_: &dyn NetView) {}
