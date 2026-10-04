//! gh #29 (QA-004): an env-configured endpoint host consents on the
//! request path instead of dead-ending.
//!
//! `OPENAI_BASE_URL` pointing at a host the manifest does not cover used to
//! be refused with `matches no granted pattern` until an interactive
//! `/login` happened to attach the ad hoc grant - a scripting dead-end. The
//! consent now fires where the tool commands' consent already fires:
//! [`lca_cli::net_consent::endpoint_consent`], through
//! `lca_permissions::authorize`, naming the exact host (FR-PERM-16),
//! persisting per project (FR-PERM-18/19), prompting once per host.
//!
//! Red first: with the consent still a dead-end (the pre-fix behavior) the
//! first row fails on `Denied` where `Allowed` is expected, with the
//! prompt never asked.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_cli::net_consent::{EndpointConsent, attach_allow_hosts, denied_message, endpoint_consent};
use lca_permissions::{
    Action, Decision, GrantStore, PermissionMode, PermissionPrompt, ProposalDiff, RuleDecision,
    RuleScope,
};
use lca_protocol::{PermissionDecision, Record};
use lca_session::SessionStore;

/// A prompt that answers from a script and records what it was asked, so a
/// row can assert both the answer and that the exact host was named.
struct ScriptedPrompt {
    answers: Vec<Decision>,
    asked: Vec<String>,
}

impl ScriptedPrompt {
    fn answering(answer: Decision) -> ScriptedPrompt {
        ScriptedPrompt {
            answers: vec![answer],
            asked: Vec::new(),
        }
    }

    fn silent() -> ScriptedPrompt {
        ScriptedPrompt {
            answers: Vec::new(),
            asked: Vec::new(),
        }
    }
}

impl PermissionPrompt for ScriptedPrompt {
    fn ask(&mut self, action: &Action) -> Decision {
        self.asked.push(action.display());
        if self.answers.is_empty() {
            Decision::Denied
        } else {
            self.answers.remove(0)
        }
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// One project, its grant store (the session's single handle), and its
/// session - everything the consent call takes.
struct World {
    root: PathBuf,
    project: PathBuf,
    grants: Arc<Mutex<GrantStore>>,
    store: SessionStore,
    session: lca_session::Session,
}

impl World {
    fn new(name: &str) -> World {
        let root = lca_testkit::scratch_path(name);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let grants = Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        let store = SessionStore::new(root.join("data"));
        let session = store.create_session(&project, "gh29").expect("session");
        World {
            root,
            project,
            grants,
            store,
            session,
        }
    }

    fn consent(&self, host: &str, prompt: &mut ScriptedPrompt) -> EndpointConsent {
        endpoint_consent(
            host,
            &self.grants,
            &self.project,
            prompt,
            &self.store,
            &self.session,
        )
    }

    /// The persisted ad hoc grants, read through a *fresh* handle: what a
    /// later process would see.
    fn persisted(&self) -> Vec<String> {
        GrantStore::open(&self.root.join("grants.json"))
            .expect("reopen")
            .net_patterns(&self.project)
    }

    /// Every `permission` record the session carries.
    fn permissions(&self) -> Vec<(String, PermissionDecision, Option<String>)> {
        self.store
            .read(&self.session)
            .expect("read")
            .records
            .into_iter()
            .filter_map(|record| match record {
                Record::Permission {
                    action,
                    decision,
                    pattern,
                    ..
                } => Some((action, decision, pattern)),
                _ => None,
            })
            .collect()
    }
}

// Verifies: gh #29 acceptance row `a_new_host_prompts_once_and_persists`
// (FR-PERM-16's exact-host consent, FR-PERM-18/19's per-project
// persistence). The prompt names the host, `allow` persists it, and the
// second request - through a fresh handle over the same file - goes
// through with no prompt.
#[test]
fn a_new_host_prompts_once_and_persists() {
    let world = World::new("gh29-prompt-once");
    let mut prompt = ScriptedPrompt::answering(Decision::Always);

    let first = world.consent("env-only.example", &mut prompt);
    assert_eq!(
        first,
        EndpointConsent::Allowed,
        "the first request to an ungranted host asks instead of refusing"
    );
    assert_eq!(
        prompt.asked,
        vec!["connect to env-only.example".to_string()],
        "the consent names the exact host, never a pattern (FR-PERM-16)"
    );

    assert!(
        world.persisted().iter().any(|p| p == "env-only.example"),
        "allow persists the grant per project (FR-PERM-18/19)"
    );
    let second = world.consent("env-only.example", &mut prompt);
    assert_eq!(
        second,
        EndpointConsent::Granted,
        "after persistence the same host needs no consent"
    );
    assert_eq!(
        prompt.asked.len(),
        1,
        "prompt once per host: the second request asked nothing"
    );
    assert!(
        world
            .permissions()
            .iter()
            .any(
                |(action, decision, pattern)| action == "connect to env-only.example"
                    && matches!(decision, PermissionDecision::Always)
                    && pattern.as_deref() == Some("env-only.example")
            ),
        "a human answer writes a permission record: {:?}",
        world.permissions()
    );
}

// Verifies: gh #29's deny row - refusing the host refuses the request,
// records the answer, and leaves the grant unattached (so the next turn
// asks again).
#[test]
fn refusing_the_host_refuses_the_request_and_records_the_answer() {
    let world = World::new("gh29-deny");
    let mut prompt = ScriptedPrompt::answering(Decision::Denied);

    let consent = world.consent("refuse.example", &mut prompt);
    assert_eq!(consent, EndpointConsent::Denied, "deny refuses the request");
    assert_eq!(prompt.asked.len(), 1, "the prompt was asked once");
    assert!(
        !world.persisted().iter().any(|p| p == "refuse.example"),
        "a refusal attaches nothing"
    );
    assert!(
        world
            .permissions()
            .iter()
            .any(|(_, decision, _)| matches!(decision, PermissionDecision::Denied)),
        "the refusal is recorded: {:?}",
        world.permissions()
    );
}

// Verifies: gh #29's yolo row (ADR-0042) - yolo answers this prompt like
// every other, persisting the pattern the way a human "always" does, and
// writing the same permission record.
#[test]
fn yolo_answers_the_host_consent_and_records_it_like_a_human() {
    let world = World::new("gh29-yolo");
    world
        .grants
        .lock()
        .unwrap()
        .set_permission_mode(PermissionMode::Yolo);
    let mut prompt = ScriptedPrompt::silent();

    let consent = world.consent("yolo.example", &mut prompt);
    assert_eq!(consent, EndpointConsent::Allowed, "yolo allows");
    assert!(
        prompt.asked.is_empty(),
        "yolo never shows the prompt: {:?}",
        prompt.asked
    );
    assert!(
        world.persisted().iter().any(|p| p == "yolo.example"),
        "the pattern is persisted exactly as a human answer persists it"
    );
    assert!(
        world
            .permissions()
            .iter()
            .any(
                |(action, decision, pattern)| action == "connect to yolo.example"
                    && matches!(decision, PermissionDecision::Always)
                    && pattern.as_deref() == Some("yolo.example")
            ),
        "the yolo answer writes the same permission record: {:?}",
        world.permissions()
    );
}

// Verifies: gh #29's rules-first guard (ADR-0039) - a `deny` rule refuses
// the host without prompting, and outranks a grant the store already
// holds, in yolo mode too.
#[test]
fn a_deny_rule_refuses_the_host_without_prompting() {
    let world = World::new("gh29-deny-rule");
    world
        .grants
        .lock()
        .unwrap()
        .add_rule(
            &world.project,
            RuleScope::Project,
            RuleDecision::Deny,
            "blocked.example",
        )
        .expect("rule");
    // A grant the store already holds, for the same host: the rule wins.
    world
        .grants
        .lock()
        .unwrap()
        .approve_net_pattern(&world.project, "blocked.example")
        .expect("grant");
    world
        .grants
        .lock()
        .unwrap()
        .set_permission_mode(PermissionMode::Yolo);
    let mut prompt = ScriptedPrompt::answering(Decision::Always);

    let consent = world.consent("blocked.example", &mut prompt);
    assert_eq!(consent, EndpointConsent::Denied, "the deny rule wins");
    assert!(
        prompt.asked.is_empty(),
        "a deny rule never reaches the prompt: {:?}",
        prompt.asked
    );
    assert!(
        world
            .permissions()
            .iter()
            .any(|(_, decision, _)| matches!(decision, PermissionDecision::Denied)),
        "the rule denial is recorded: {:?}",
        world.permissions()
    );
}

// Verifies: gh #29's `--allow-host` row - a process-scoped grant that
// passes the consent without a prompt, records a `once` answer, and never
// reaches `grants.json`.
#[test]
fn allow_host_grants_this_run_only_and_records_a_once_answer() {
    let world = World::new("gh29-allow-host");
    attach_allow_hosts(
        &world.grants,
        &world.project,
        &["flagged.example".to_string()],
        &world.store,
        &world.session,
    )
    .expect("attach");
    let mut prompt = ScriptedPrompt::answering(Decision::Always);

    let consent = world.consent("flagged.example", &mut prompt);
    assert_eq!(
        consent,
        EndpointConsent::Granted,
        "the flag's grant covers the host for this run"
    );
    assert!(
        prompt.asked.is_empty(),
        "nothing to ask: {:?}",
        prompt.asked
    );
    assert!(
        world
            .permissions()
            .iter()
            .any(
                |(action, decision, pattern)| action == "connect to flagged.example"
                    && matches!(decision, PermissionDecision::Once)
                    && pattern.is_none()
            ),
        "the flag writes a once-shaped permission record: {:?}",
        world.permissions()
    );
    assert!(
        !world.persisted().iter().any(|p| p == "flagged.example"),
        "--allow-host never writes the grant store"
    );

    // A fresh handle over the same file - the next run - grants nothing,
    // so that run prompts (interactively) or exits 4 (headless) again.
    let reopened = GrantStore::open(&world.root.join("grants.json")).expect("reopen");
    assert!(
        !reopened
            .net_patterns(&world.project)
            .iter()
            .any(|p| p == "flagged.example"),
        "the process-scoped grant died with the run"
    );
}

// Verifies: gh #31 review (one answer mapping for every host consent) -
// `once` on a host grant is a *session* allowance: it covers the work
// that asked and the rest of this run, it joins the same session set
// `--allow-host` uses, and it is never persisted - a fresh process asks
// again. That is what separates it from `always`; the pre-mapping
// behavior allowed the asking call and granted nothing, so the request
// that followed was denied (the picker's silent stall).
#[test]
fn once_allows_the_host_for_this_session_and_is_not_persisted() {
    let world = World::new("gh29-once");
    let mut prompt = ScriptedPrompt::answering(Decision::Once);

    let consent = world.consent("once.example", &mut prompt);
    assert_eq!(
        consent,
        EndpointConsent::Allowed,
        "`once` still allows the work it was given"
    );
    assert_eq!(prompt.asked.len(), 1, "asked exactly once");
    assert!(
        world
            .grants
            .lock()
            .unwrap()
            .session_net_pattern(&world.project, "once.example"),
        "`once` attached the session-scoped grant - the set `--allow-host` joins"
    );

    // The distinguishing row: nothing reached the file, so a fresh
    // process over the same store asks again.
    let reopened = GrantStore::open(&world.root.join("grants.json")).expect("reopen");
    assert!(
        !reopened
            .net_patterns(&world.project)
            .iter()
            .any(|pattern| pattern == "once.example"),
        "a fresh process must ask again: `once` is never persisted"
    );

    // And inside this run it is covered: no second prompt.
    let again = world.consent("once.example", &mut prompt);
    assert_eq!(
        again,
        EndpointConsent::Granted,
        "covered for the rest of this run"
    );
    assert_eq!(prompt.asked.len(), 1, "no re-ask inside the session");
    assert!(
        world
            .permissions()
            .iter()
            .any(|(action, decision, pattern)| {
                action == "connect to once.example"
                    && matches!(decision, PermissionDecision::Once)
                    && pattern.is_none()
            }),
        "the answer is recorded in the `once` shape: {:?}",
        world.permissions()
    );
}

// Verifies: gh #29's headless row - with no modal to show, the denial
// names the host and the fix rather than the subsystem that refused.
#[test]
fn the_headless_denial_names_the_host_and_the_fix() {
    let message = denied_message("ungranted.example");
    assert!(
        message.contains("ungranted.example"),
        "the message names the host: {message}"
    );
    assert!(
        message.contains("--allow-host ungranted.example"),
        "the message names the flag: {message}"
    );
    assert!(
        message.contains("interactively"),
        "the message names the interactive fix: {message}"
    );
}
