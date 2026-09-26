//! Permission store tests: ADR-0006's split between project proposals and
//! the user grant store, keyed by the canonical project path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lca_permissions::{Action, Decision, GrantStore, PermissionPrompt, ProposalDiff, Proposals};

fn scratch(name: &str) -> PathBuf {
    lca_testkit::scratch_path(name)
}

fn store_path(root: &std::path::Path) -> PathBuf {
    root.join("grants.json")
}

struct ScriptedPrompt {
    answers: Vec<Decision>,
    asked: Vec<String>,
    reviews: Vec<ProposalDiff>,
    review_answer: bool,
}

impl ScriptedPrompt {
    fn new(answers: Vec<Decision>) -> Self {
        ScriptedPrompt {
            answers,
            asked: Vec::new(),
            reviews: Vec::new(),
            review_answer: true,
        }
    }
}

impl PermissionPrompt for ScriptedPrompt {
    fn ask(&mut self, action: &Action) -> Decision {
        self.asked.push(action.display());
        self.answers.pop().unwrap_or(Decision::Denied)
    }

    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool {
        self.reviews.push(diff.clone());
        self.review_answer
    }
}

fn proposals(entries: &[(&str, &str)]) -> Proposals {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<BTreeMap<_, _>>()
}

// Verifies: FR-PERM-8 (an always approval persists, keyed by the canonical
// path of the project)
#[test]
fn always_approval_persists_across_reloads() {
    let root = scratch("persist");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "cargo test".into(),
        cwd: project.clone(),
    };

    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Always]);
    let outcome = lca_permissions::authorize(&mut store, &project, &action, None, &mut prompt)
        .expect("authorize");
    assert!(outcome.allowed());
    assert_eq!(prompt.asked.len(), 1);

    let reloaded = GrantStore::open(&store_path(&root)).expect("reopen");
    assert!(
        reloaded.is_allowed(&project, &action),
        "always survives a restart"
    );
    assert!(
        lca_permissions::authorize(
            &mut GrantStore::open(&store_path(&root)).expect("open"),
            &project,
            &action,
            None,
            &mut ScriptedPrompt::new(vec![]),
        )
        .expect("authorize")
        .allowed(),
        "no second prompt"
    );
}

// Verifies: FR-PERM-8 (grants never land in a file inside the project)
#[test]
fn approvals_are_never_written_inside_the_project() {
    let root = scratch("no-project-write");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "ls".into(),
        cwd: project.clone(),
    };

    let before: Vec<_> = std::fs::read_dir(&project).expect("read").collect();
    let mut store = GrantStore::open(&root.join("grants.json")).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Always]);
    lca_permissions::authorize(&mut store, &project, &action, None, &mut prompt)
        .expect("authorize");
    let after: Vec<_> = std::fs::read_dir(&project).expect("read").collect();
    assert_eq!(
        before.len(),
        after.len(),
        "the project directory is untouched"
    );
    assert!(
        root.join("grants.json").is_file(),
        "the grant store holds it instead"
    );
}

// Verifies: FR-PERM-8 (keyed by canonical path: two spellings, one entry)
#[test]
fn keys_by_the_canonical_project_path() {
    let root = scratch("canonical");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "ls".into(),
        cwd: project.clone(),
    };

    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Always]);
    lca_permissions::authorize(&mut store, &project, &action, None, &mut prompt)
        .expect("authorize");

    // A different spelling of the same directory finds the same entry.
    let spelled_out = root.join(".").join("project");
    assert!(store.is_allowed(&spelled_out, &action));
}

#[test]
fn once_approvals_do_not_persist() {
    let root = scratch("once");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "ls".into(),
        cwd: project.clone(),
    };

    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Once]);
    assert!(
        lca_permissions::authorize(&mut store, &project, &action, None, &mut prompt)
            .expect("authorize")
            .allowed()
    );

    let mut reloaded = GrantStore::open(&store_path(&root)).expect("reopen");
    assert!(!reloaded.is_allowed(&project, &action), "once is once");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Denied]);
    let outcome = lca_permissions::authorize(&mut reloaded, &project, &action, None, &mut prompt)
        .expect("authorize");
    assert!(outcome.denied());
    assert_eq!(prompt.asked.len(), 1, "it asks again");
}

#[test]
fn denials_store_nothing() {
    let root = scratch("deny");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "rm -rf /".into(),
        cwd: project.clone(),
    };
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Denied]);
    let outcome = lca_permissions::authorize(&mut store, &project, &action, None, &mut prompt)
        .expect("authorize");
    assert!(outcome.denied());
    assert!(!store.is_allowed(&project, &action));
    let saved = std::fs::read_to_string(store_path(&root)).unwrap_or_default();
    assert!(
        !saved.contains("rm -rf"),
        "denied commands are not persisted"
    );
}

// Verifies: NFR-13 (deny by default: an action with no explicit grant does
// not run), FR-PERM-11 (a grant that exists only in the project config
// never takes effect; it must be copied into the user store first)
#[test]
fn proposals_alone_never_authorize() {
    let root = scratch("proposal-no-force");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let action = Action::Shell {
        command: "git status".into(),
        cwd: project.clone(),
    };
    let props = proposals(&[("git status", "read-only git")]);

    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    // The review prompt shows what the project file would add; the user has
    // not approved this set, so nothing is copied into the user store.
    let mut prompt = ScriptedPrompt::new(vec![Decision::Denied]);
    prompt.review_answer = false;
    let outcome =
        lca_permissions::authorize(&mut store, &project, &action, Some(&props), &mut prompt)
            .expect("authorize");
    assert!(outcome.denied(), "a proposal has no force until approved");
    assert_eq!(prompt.asked.len(), 1);
}

// Verifies: FR-PERM-10 (after the user approved one proposal set, a changed
// set prompts with the difference before it applies)
#[test]
fn changed_proposals_prompt_with_the_difference() {
    let root = scratch("proposal-diff");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");

    let v1 = proposals(&[("git status", "read-only git"), ("cargo build", "build")]);
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Always]);
    let action = Action::Shell {
        command: "cargo build".into(),
        cwd: project.clone(),
    };
    lca_permissions::authorize(&mut store, &project, &action, Some(&v1), &mut prompt)
        .expect("first review");
    assert_eq!(prompt.reviews.len(), 1, "the first set is reviewed");
    assert!(prompt.reviews[0].added.len() == 2 && prompt.reviews[0].removed.is_empty());
    assert!(
        store.is_allowed(&project, &action),
        "approved proposals are copied into the user store"
    );

    let v2 = proposals(&[("cargo build", "build"), ("cargo clippy", "lints")]);
    let action2 = Action::Shell {
        command: "cargo clippy".into(),
        cwd: project.clone(),
    };
    let mut prompt2 = ScriptedPrompt::new(vec![Decision::Always]);
    lca_permissions::authorize(&mut store, &project, &action2, Some(&v2), &mut prompt2)
        .expect("second review");
    assert_eq!(prompt2.reviews.len(), 1, "a changed set re-prompts");
    let diff = &prompt2.reviews[0];
    assert_eq!(diff.added.keys().collect::<Vec<_>>(), vec!["cargo clippy"]);
    assert_eq!(diff.removed.keys().collect::<Vec<_>>(), vec!["git status"]);
    assert!(
        !diff.added.contains_key("cargo build"),
        "unchanged entries are not in the diff"
    );

    // The removed proposal loses its grant once the new set is applied.
    let old = Action::Shell {
        command: "git status".into(),
        cwd: project.clone(),
    };
    assert!(
        !store.is_allowed(&project, &old),
        "removed proposals stop applying"
    );
}

#[test]
fn rejecting_a_proposal_diff_keeps_the_old_set() {
    let root = scratch("proposal-reject");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let v1 = proposals(&[("git status", "read-only")]);
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let mut prompt = ScriptedPrompt::new(vec![Decision::Denied]);
    let action = Action::Shell {
        command: "git status".into(),
        cwd: project.clone(),
    };
    lca_permissions::authorize(&mut store, &project, &action, Some(&v1), &mut prompt)
        .expect("review");
    assert!(store.is_allowed(&project, &action));

    let v2 = proposals(&[("git push", "pushes")]);
    let mut prompt2 = ScriptedPrompt::new(vec![Decision::Denied]);
    prompt2.review_answer = false;
    let push = Action::Shell {
        command: "git push".into(),
        cwd: project.clone(),
    };
    let outcome = lca_permissions::authorize(&mut store, &project, &push, Some(&v2), &mut prompt2)
        .expect("authorize");
    assert!(outcome.denied());
    assert!(
        !store.is_allowed(&project, &push),
        "the rejected set never applies"
    );
    assert!(
        store.is_allowed(&project, &action),
        "the old approved set still holds"
    );
}

// Verifies: FR-PERM-19 (trust state lives in the user grant store, keyed by
// the canonical project path)
#[test]
fn trust_state_persists_per_canonical_path() {
    let root = scratch("trust");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    assert!(!store.is_trusted(&project), "distrust by default");
    store.set_trusted(&project, true).expect("save");
    let reloaded = GrantStore::open(&store_path(&root)).expect("reopen");
    assert!(reloaded.is_trusted(&project));
}

// Pattern approval breadth: the matcher must behave predictably, since a
// broad pattern is the model's weakest point (threat model).
#[test]
fn shell_patterns_match_predictably() {
    let root = scratch("patterns");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    store.approve_pattern(&project, "git status").expect("save");
    store
        .approve_pattern(&project, "cargo test*")
        .expect("save");

    let status = Action::Shell {
        command: "git status".into(),
        cwd: project.clone(),
    };
    let push = Action::Shell {
        command: "git push".into(),
        cwd: project.clone(),
    };
    let test_one = Action::Shell {
        command: "cargo test --all".into(),
        cwd: project.clone(),
    };
    let build = Action::Shell {
        command: "cargo build".into(),
        cwd: project.clone(),
    };
    assert!(store.is_allowed(&project, &status));
    assert!(
        !store.is_allowed(&project, &push),
        "exact patterns stay exact"
    );
    assert!(store.is_allowed(&project, &test_one));
    assert!(
        !store.is_allowed(&project, &build),
        "wildcards are not prefixes"
    );
}

// The action display is what the interface shows while waiting for approval
// (FR-UI-4): the exact command or path, nothing summarised.
#[test]
fn action_display_shows_the_exact_command_or_path() {
    let shell = Action::Shell {
        command: "ls -la /tmp".into(),
        cwd: PathBuf::from("/w"),
    };
    assert!(shell.display().contains("ls -la /tmp"));
    let write = Action::WritePath {
        path: PathBuf::from("/w/../etc/passwd"),
    };
    assert!(
        write.display().contains("/w/../etc/passwd"),
        "raw path, not canonicalised away"
    );
}

// Verifies: FR-PERM-19 (per-project extension enablement in the grant
// store) with FR-PROV-9 riding on it: disable survives a save/reload,
// and a project that never decided stays at the default.
#[test]
fn extension_enablement_persists_per_project() {
    let dir = lca_testkit::scratch_path("lca-grants-en");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let project = dir.join("project");
    std::fs::create_dir_all(&project).expect("mkdir project");
    let path = dir.join("grants.json");

    let mut store = GrantStore::open(&path).expect("store");
    assert_eq!(store.extension_enabled(&project, "openai-compatible"), None);
    store
        .set_extension_enabled(&project, "openai-compatible", false)
        .expect("disable");
    drop(store);

    let reopened = GrantStore::open(&path).expect("reopen");
    assert_eq!(
        reopened.extension_enabled(&project, "openai-compatible"),
        Some(false),
        "the disable survives the round trip"
    );
    assert_eq!(
        reopened.extension_enabled(&project, "hooks-example"),
        None,
        "a provider nothing was said about keeps its default"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// Verifies: FR-PERM-16's persistence half (ADR-0022): an ad hoc net
// grant approved for one project is readable back for the capability
// engine, invalid vocabulary is refused at approval and skipped on
// read, and one project's grant never leaks to another.
#[test]
fn adhoc_net_grants_persist_per_project() {
    let dir = lca_testkit::scratch_path("lca-grants-net");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let project = dir.join("project");
    let other = dir.join("other");
    std::fs::create_dir_all(&project).expect("mkdir project");
    std::fs::create_dir_all(&other).expect("mkdir other");
    let path = dir.join("grants.json");

    let mut store = GrantStore::open(&path).expect("store");
    assert!(store.net_patterns(&project).is_empty());
    store
        .approve_net_pattern(&project, "llm.example.com:8443")
        .expect("approve");
    assert!(
        store.approve_net_pattern(&project, "workspace").is_err(),
        "fs vocabulary refused"
    );
    drop(store);

    let reopened = GrantStore::open(&path).expect("reopen");
    assert_eq!(
        reopened.net_patterns(&project),
        vec!["llm.example.com:8443".to_string()]
    );
    assert!(reopened.net_patterns(&other).is_empty(), "per project only");
    let _ = std::fs::remove_dir_all(&dir);
}

// Verifies: capability catalog `process`/`pty` - a capability engine's
// swappable prompt routes to whatever the interface installed (so an
// extension's command reaches the same modal the model's does), and denies
// when nothing is installed (headless, or between turns).
#[test]
fn shared_prompt_routes_to_the_installed_prompt_or_denies() {
    struct Allow;
    impl PermissionPrompt for Allow {
        fn ask(&mut self, _action: &Action) -> Decision {
            Decision::Once
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            true
        }
    }

    let mut shared = lca_permissions::SharedPrompt::default();
    let action = Action::Shell {
        command: "ls".to_string(),
        cwd: PathBuf::from("/"),
    };
    assert_eq!(
        shared.ask(&action),
        Decision::Denied,
        "nothing installed denies"
    );
    assert!(!shared.review_proposals(&ProposalDiff::default()));

    shared.set(std::sync::Arc::new(std::sync::Mutex::new(Allow)));
    assert_eq!(
        shared.ask(&action),
        Decision::Once,
        "routes to the installed prompt"
    );
    assert!(shared.review_proposals(&ProposalDiff::default()));
}

// Verifies: an empty store is fail-closed (NFR-13) - used when the store file
// cannot be read so a bad store does not panic the agent.
#[test]
fn an_empty_store_grants_nothing() {
    let store = GrantStore::empty();
    let action = Action::Shell {
        command: "ls".to_string(),
        cwd: PathBuf::from("/"),
    };
    assert!(!store.is_allowed(std::path::Path::new("/any"), &action));
    assert!(!store.is_trusted(std::path::Path::new("/any")));
}

// Verifies: a bad ad-hoc `net` pattern is a validation error, not a fake
// "corrupt store" error (the old type-abuse path through serde).
#[test]
fn approving_an_invalid_net_pattern_is_a_validation_error() {
    let root = scratch("bad-pattern");
    let mut store = GrantStore::open(&store_path(&root)).expect("open");
    let err = store
        .approve_net_pattern(&root, "*")
        .expect_err("bare wildcard refused");
    assert!(
        matches!(err, lca_permissions::Error::Pattern(_)),
        "got {err:?}"
    );
}

// FR-PERM-16 / ADR-0022: an "always" on a `Net` action lands in the net
// vocabulary (not the shell/path patterns set) and is allowed on the next
// call without another prompt.
#[test]
fn an_always_net_approval_persists_as_an_adhoc_grant() {
    struct Always;
    impl PermissionPrompt for Always {
        fn ask(&mut self, _action: &Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }
    let root = scratch("net-adhoc");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let mut store = GrantStore::open(&store_path(&root)).expect("store");
    let action = Action::Net {
        host: "llm.example.com".to_string(),
    };

    let outcome =
        lca_permissions::authorize(&mut store, &project, &action, None, &mut Always).expect("auth");
    assert!(outcome.allowed);
    assert_eq!(outcome.stored_pattern.as_deref(), Some("llm.example.com"));
    assert_eq!(
        store.net_patterns(&project),
        vec!["llm.example.com".to_string()],
        "stored in the net vocabulary"
    );

    // The next identical call is allowed without prompting.
    struct Panic;
    impl PermissionPrompt for Panic {
        fn ask(&mut self, _action: &Action) -> Decision {
            panic!("a granted host must not re-prompt")
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }
    let again = lca_permissions::authorize(&mut store, &project, &action, None, &mut Panic)
        .expect("authorize");
    assert!(again.allowed);
    assert!(!again.prompted, "the stored grant already covers it");
}
