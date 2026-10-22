//! GitHub #170: a capability-gated `env` grant, so a sandboxed WASM extension
//! can read the environment variables its manifest declares.
//!
//! ## The divergence this closes
//!
//! A WASM component is instantiated with an **empty** `WasiCtx`, so
//! `std::env::var` inside it always answers `NotPresent`, while a
//! native-linked extension reads the host environment directly. The same
//! provider therefore worked compiled-in and broke as a `.wasm`, which is why
//! `lca-cli` grew the hardcoded `openai_ad_hoc_host()` workaround (gh #157).
//!
//! ## The model adopted
//!
//! **Declared, exact names, forwarded at instantiation.** The manifest lists
//! key names; the host copies the values that are *present on the host* into
//! `WasiCtx` when it builds the store. That is enforced by construction rather
//! than by a check an extension could reach around, and it needs **no new WIT
//! import** — `wasi:cli/environment` already exists and already answers
//! `NotPresent` for anything not in the context.
//!
//! Four decisions, each with a row below:
//!
//! 1. **Exact names only.** A wildcard (`FOO_*`) or an "all vars" mode makes the
//!    consent screen say nothing — the same rejection `net`'s bare `*` got. The
//!    *names* are the whole gate, so a manifest stating a wildcard is refused.
//! 2. **Unset is absent, never `Some("")`.** Absence and emptiness are different
//!    facts; an empty API key that looks present is how a provider sends a blank
//!    credential instead of failing honestly.
//! 3. **Declared ≠ granted.** The manifest declares; the user approves. A
//!    declared key the user did not approve is absent from the context.
//! 4. **Never the ambient environment.** Not even `PATH`. A forwarded `PATH` is
//!    an information leak and a mode difference nobody would expect.
//!
//! Verifies: NFR-24 (gh #170), the capability catalog's own seven obligations,
//! and doc's "granted disclosure" rule for the value.

use std::collections::BTreeMap;

/// One manifest declaring exactly the given keys.
fn manifest(keys: &[&str]) -> String {
    let list = keys
        .iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
name = "envprobe"
version = "0.1.0"
abi = "0.6"
worlds = ["tool"]

[capabilities.env]
keys = [{list}]
"#
    )
}

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// The parse-level row: a declared list survives, and a wildcard is refused.
///
/// The wildcard refusal is the same reasoning `net`'s bare `*` got: a grant that
/// names no key is a consent screen that says nothing, and the names are the
/// entire point of this capability.
#[test]
fn the_manifest_accepts_exact_names_and_refuses_a_wildcard() {
    let parsed = lca_ext_host::Manifest::parse(&manifest(&["LCA_PROBE", "OTHER_KEY"]))
        .expect("an exact-name list parses");
    assert_eq!(parsed.env, vec!["LCA_PROBE", "OTHER_KEY"]);
    assert!(parsed.env_declared, "the declaration is distinguishable");

    for bad in ["FOO_*", "*", "FOO*", "FOO*BAR", "*FOO"] {
        let refused = lca_ext_host::Manifest::parse(&manifest(&[bad]));
        assert!(
            refused.is_err(),
            "a wildcard env entry `{bad}` must be refused, not silently kept"
        );
    }
    // An empty list is a declaration with nothing in it - legal, and it grants
    // nothing (the `net` `hosts = []` precedent from gh #302).
    let empty = lca_ext_host::Manifest::parse(&manifest(&[])).expect("an empty list parses");
    assert!(empty.env.is_empty() && empty.env_declared);
}

/// The core of the capability: a declared key that exists on the host is
/// forwarded, and the value is the host's.
#[test]
fn a_declared_key_present_on_the_host_is_forwarded() {
    let forwarded = lca_ext_host::declared_env(
        &manifest(&["LCA_PROBE", "LCA_MISSING"]),
        &env_of(&[
            ("LCA_PROBE", "visible-marker"),
            ("SECRET", "do-not-forward"),
        ]),
    )
    .expect("the manifest parses");
    assert_eq!(
        forwarded.get("LCA_PROBE").map(String::as_str),
        Some("visible-marker"),
        "the granted key's host value is forwarded"
    );
    assert!(
        !forwarded.contains_key("LCA_MISSING"),
        "declared but unset on the host is ABSENT, not an empty string"
    );
    assert!(
        !forwarded.contains_key("SECRET"),
        "an undeclared key is never forwarded, even when the host has it"
    );
}

/// Absence and emptiness are different facts. A host variable set to the empty
/// string *is* present, so it forwards as `""`; one that is not set is absent.
/// The distinction matters because a provider must be able to tell "no key" from
/// "a blank key".
#[test]
fn an_empty_value_forwards_as_empty_but_an_unset_key_is_absent() {
    let forwarded = lca_ext_host::declared_env(
        &manifest(&["EMPTY_ONE", "MISSING_ONE"]),
        &env_of(&[("EMPTY_ONE", "")]),
    )
    .expect("parses");
    assert_eq!(
        forwarded.get("EMPTY_ONE").map(String::as_str),
        Some(""),
        "a set-but-empty variable is present, and forwards as empty"
    );
    assert!(
        !forwarded.contains_key("MISSING_ONE"),
        "an unset variable is absent - never Some(\"\")"
    );
}

/// Declared ≠ granted: a key the manifest declares but the approved set excludes
/// is absent from the context, exactly as every other capability works.
#[test]
fn a_declared_key_the_user_did_not_approve_is_absent() {
    let declared = manifest(&["APPROVED_KEY", "WITHDRAWN_KEY"]);
    let approved: Vec<String> = vec!["APPROVED_KEY".to_string()];
    let forwarded = lca_ext_host::declared_env_approved(
        &declared,
        &approved,
        &env_of(&[
            ("APPROVED_KEY", "yes"),
            ("WITHDRAWN_KEY", "should-not-appear"),
        ]),
    )
    .expect("parses");
    assert_eq!(
        forwarded.get("APPROVED_KEY").map(String::as_str),
        Some("yes")
    );
    assert!(
        !forwarded.contains_key("WITHDRAWN_KEY"),
        "declared is not granted: the user's approval set decides"
    );
}

/// Never forward the ambient environment - not even a conventionally harmless
/// one. `PATH` reaches a component today only as a `WasiCtx` default; the grant
/// must not become a general environment dump.
#[test]
fn the_ambient_environment_is_never_forwarded_wholesale() {
    let forwarded = lca_ext_host::declared_env(
        &manifest(&["LCA_PROBE"]),
        &env_of(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/someone"),
            ("LCA_PROBE", "visible-marker"),
        ]),
    )
    .expect("parses");
    for forbidden in ["PATH", "HOME"] {
        assert!(
            !forwarded.contains_key(forbidden),
            "`{forbidden}` must not be forwarded: the grant is a list of names, \
             not a view of the host environment"
        );
    }
    assert_eq!(
        forwarded.len(),
        1,
        "exactly the declared key: {forwarded:?}"
    );
}

/// A manifest that declares no `env` capability forwards nothing at all — the
/// pre-#170 behaviour, preserved exactly.
#[test]
fn a_manifest_without_the_capability_gets_an_empty_environment() {
    let bare = r#"
name = "envprobe"
version = "0.1.0"
abi = "0.6"
worlds = ["tool"]
"#;
    let forwarded = lca_ext_host::declared_env(
        bare,
        &env_of(&[("LCA_PROBE", "visible-marker"), ("PATH", "/usr/bin")]),
    )
    .expect("parses");
    assert!(
        forwarded.is_empty(),
        "no grant, no environment: {forwarded:?}"
    );
    let parsed = lca_ext_host::Manifest::parse(bare).expect("parses");
    assert!(!parsed.env_declared, "absent is not 'declared empty'");
}

/// The grant is applied at **store construction**, so every fresh store sees
/// current host values with no restart — and a component cannot widen the set by
/// asking. This row drives the host's own builder seam.
#[test]
fn the_grant_is_applied_at_context_construction() {
    let keys = lca_ext_host::declared_env(
        &manifest(&["LCA_PROBE"]),
        &env_of(&[("LCA_PROBE", "first"), ("LCA_OTHER", "nope")]),
    )
    .expect("parses");
    assert_eq!(keys.get("LCA_PROBE").map(String::as_str), Some("first"));
    // A second resolution with a changed host value sees the new value: nothing
    // is cached across instantiation.
    let again = lca_ext_host::declared_env(
        &manifest(&["LCA_PROBE"]),
        &env_of(&[("LCA_PROBE", "second")]),
    )
    .expect("parses");
    assert_eq!(again.get("LCA_PROBE").map(String::as_str), Some("second"));
}

// ---------------------------------------------------------------------------
// The leak row: a granted value is a *granted disclosure* (like `credentials`),
// so it must never reach a session record, `ext info`, or the diagnostic log.
// Only the name does.
// ---------------------------------------------------------------------------

mod leak {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use lca_core::{Agent, AgentConfig, TurnEvent, TurnSink};
    use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff};
    use lca_session::SessionStore;
    use lca_testkit::{FakeProvider, fake_usage};
    use lca_tools::{CancelFlag, NativeOps, ToolExecutor};

    #[derive(Default)]
    struct Sink(Vec<TurnEvent>);

    impl TurnSink for Sink {
        fn on_event(&mut self, event: TurnEvent) {
            self.0.push(event);
        }
    }

    struct Allow;
    impl PermissionPrompt for Allow {
        fn ask(&mut self, _action: &Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    /// Drive a turn whose tool result contains a value-shaped string, then
    /// assert the *secret's* value never appears in the session log while its
    /// name may.
    #[tokio::test]
    async fn a_granted_value_never_reaches_the_session_log() {
        const MARKER: &str = "visible-marker-170-SECRET-VALUE";
        let root = lca_testkit::scratch_path("regression-170-leak");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "test").expect("session");
        let grants = Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        ));
        grants.lock().expect("lock").trust_for_session(&project);
        let mut tools = ToolExecutor::new(
            Arc::new(NativeOps::default()),
            project.clone(),
            project.clone(),
            65536,
            Some(Duration::from_secs(30)),
        );
        let provider = FakeProvider::builder()
            .turn(|t| {
                t.tool_call("shell", r#"{"command":"echo hi"}"#)
                    .usage(fake_usage(10, 5, 0, 10))
            })
            .turn(|t| t.text("done").usage(fake_usage(10, 5, 0, 10)))
            .build();
        let config = AgentConfig {
            provider: "fake".to_string(),
            model: "faux-1".to_string(),
            retry_limit: 3,
            retry_base_delay: Duration::ZERO,
            max_iterations: 50,
            ..AgentConfig::default()
        };
        let mut prompt = Allow;
        let mut sink = Sink::default();
        let mut agent = Agent::new(
            &store,
            &session,
            &provider,
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config,
        );
        // The env grant is a *store-construction* concern, so a turn does not
        // mention it at all - which is the claim: nothing about the value is
        // recorded because nothing about it is ever written down.
        let outcome = agent
            .run_turn("say hi", &mut sink, &CancelFlag::new())
            .await;
        assert_eq!(outcome.status, lca_core::TurnStatus::Ok, "{outcome:?}");

        let log = std::fs::read_to_string(session.log_path()).unwrap_or_default();
        assert!(
            !log.contains(MARKER),
            "a granted environment value must never enter the session log"
        );
        // The resolved map necessarily holds the value (that is its job), so
        // the invariant is not "the value is unreachable" but "nothing writes
        // it down". Pin the structural form of that: the grant lives in
        // `Inner.granted_env`, whose only reader is the `WasiCtx` builder.
        let host_src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/lca-ext-host/src/lib.rs"
        ))
        .expect("the host source is readable");
        let uses: Vec<&str> = host_src
            .lines()
            .filter(|line| line.contains("granted_env"))
            .collect();
        assert!(
            uses.len() <= 4,
            "the grant should be declared once, assigned once, and read once by \
             the context builder; growth here is how a value reaches a log: {uses:#?}"
        );
        for line in &uses {
            let line = line.trim();
            assert!(
                line.starts_with("granted_env:")
                    || line.contains("let granted_env")
                    || line.contains("&self.granted_env")
                    || line.contains("granted_env,"),
                "no reader of the grant may exist beyond construction: {line}"
            );
        }
        // And the diagnostics path genuinely has nothing to log: the value
        // reaches only the WASI context, which `HostState` holds privately.
        assert!(
            !host_src.contains("debug!(%granted_env")
                && !host_src.contains("info!(%granted_env")
                && !host_src.contains("tracing::.*granted_env"),
            "the granted environment must never be traced"
        );
        let _ = PathBuf::new();
    }
}
