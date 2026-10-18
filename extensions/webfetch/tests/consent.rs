//! Gh #302: the interactive per-host consent fallback, driven through the
//! tool the model actually calls.
//!
//! These rows live in `webfetch`'s own suite rather than the workspace
//! regression crate for a licensing reason, not a structural one: the
//! regression crate is a dependency of the shipped binary's graph check, and
//! `webfetch` pulls `html2md`, whose license is GPL-3.0+ — outside
//! `deny.toml`'s allow list. Keeping the dependency inside `webfetch`'s
//! dev-dependencies means the deny gate sees it only where it already was.
//! The engine-level rows (which need no `webfetch` types) stay in
//! `tests/regressions/302-webfetch-interactive-consent.rs`.
//!
//! Before this, an ungranted host was a dead end: `webfetch` ships
//! `[capabilities.net] hosts = []` on purpose, and the capability layer
//! required a *non-empty* host list to consider `net` declared, so the
//! first fetch died with `no network capability declared` on the
//! declaration gate; even past that, the deny was terminal because the ad
//! hoc-grant modal lived in the tool/permission layer and was never
//! consulted for an extension's own fetch.
//!
//! Verifies: gh #302 acceptance 1-4 through `webfetch::fetch` and its
//! `NetView` impl for `lca_tools::Capabilities`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};

use lca_permissions::{
    Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots, SharedPrompt,
};
use lca_tools::{Capabilities, CapabilityGrants};
use webfetch::FetchRequest;

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
        let root = lca_testkit::scratch_path(&format!("lca-302wf-{name}"));
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

// Verifies: the registration half, found by live testing rather than by a
// test. The consent fallback above is worthless if nothing calls it: the
// only `webfetch` the binary shipped was the WASM half, which cannot fetch
// on 0.6.x (`the tool world has no net import`, gh #222/#302), so a
// model-driven webfetch reported that deferral and the host-side fetch -
// consent and all - was never reached. The native half is now registered
// with the other bundled extensions, and this row pins that the grants it
// carries are the declaration-only shape the consent path needs.
#[test]
fn the_bundled_fetch_tool_ships_with_declaration_only_grants() {
    let grants = webfetch::manifest_grants();
    assert!(
        grants.net_declared,
        "the bundled tool declares net so consent can be offered"
    );
    assert!(
        grants.net.is_empty(),
        "and pre-grants no host: every host arrives by consent"
    );
    // Nothing else is reachable through this tool, which is what makes the
    // ad hoc grant the only way its hosts arrive.
    assert!(grants.net_local.is_empty(), "no local range is pre-granted");
    assert!(!grants.fs_declared, "the fetch tool reads no files");
    assert!(!grants.process, "the fetch tool spawns nothing");
    assert!(!grants.pty, "the fetch tool opens no terminal");
}
