//! Gh #407: `lca:host/ui-dialogs` gained `questionnaire`, so an extension can ask
//! N questions in **one** modal instead of N sequential host pickers.
//!
//! `wit/deps/lca-host/ui-dialogs.wit` now carries two records and one function:
//!
//! ```wit
//! record question { header: string, prompt: string, options: list<string>, multiple: bool }
//! record answer { header: string, value: string, custom: bool }
//! questionnaire: func(title: string, questions: list<question>) -> result<option<list<answer>>, string>;
//! ```
//!
//! ## What "additive for old artifacts" has to mean, again
//!
//! The interface is one every `tool` and `tool-catalog` guest already imports
//! (gh #172), so a *rebuilt* guest sees the new member - and a guest built
//! before this line imports the same interface and never calls it. The host
//! satisfies an import whether or not the guest calls it (the rule `tools`
//! established in gh #77 and `net` repeated in gh #302), so the old artifact
//! must still link **and** still run. That is the whole claim, so it is proven
//! here rather than asserted.
//!
//! ## What this guard pins
//!
//! 1. **Old-guest compatibility.** `extensions/tool-legacy`'s committed
//!    component predates the questionnaire; it loads, reports its tool, and
//!    runs a call.
//! 2. **The seam is nameable from outside.** The two records are
//!    `lca-permissions` types, so an asker can build them without depending on
//!    `lca-ext-host` (the brief's rule, and what `question`'s native and WASM
//!    halves both need).
//! 3. **The slot forwards, and answers dismissed when empty.** A dismissal is
//!    `None`, never a hang - the headless contract, one rung deeper than the
//!    `confirm`/`select`/`input` rows.
//!
//! Verifies: gh #407's ABI claim, ADR-0028 (in-place window), FR-EXT-1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_ext_abi::ExtensionDispatch as _;
use lca_permissions::{Answer, DialogPrompt, Question, SharedDialogs};
use std::sync::{Arc, Mutex};

/// The fixture built **before** the questionnaire existed: `tool-legacy` is the
/// single-tool guest kept precisely as a pre-import artifact (gh #77's
/// precedent, reused by gh #302's `net` row).
const LEGACY: &[u8] = include_bytes!("../../extensions/tool-legacy/fixtures/component.wasm");
const LEGACY_MANIFEST: &str = include_str!("../../extensions/tool-legacy/extension.toml");

struct Allow;
impl lca_permissions::PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

fn host(tag: &str) -> lca_ext_host::ExtHost {
    let root = lca_testkit::scratch_path(&format!("lca-407-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let prompt: Arc<Mutex<dyn lca_permissions::PermissionPrompt>> = Arc::new(Mutex::new(Allow));
    lca_ext_host::ExtHost::new(
        lca_ext_host::ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 20_000_000,
            log_limit_bytes: 4096,
        },
        Arc::new(lca_ext_host::HostEnvironment {
            roots: lca_permissions::ScopeRoots {
                workspace: root.join("workspace"),
                private: root.join("private"),
                home_config: root.join("config"),
                temp: root.join("tmp"),
                state_dir: root.join("data"),
            },
            prompt,
            grant_store: Arc::new(Mutex::new(
                lca_permissions::GrantStore::open(&root.join("grants.json")).expect("grants"),
            )),
            project: root.join("workspace"),
            proposals: None,
            dialogs: SharedDialogs::default(),
        }),
    )
}

// ---------------------------------------------------------------------------
// 1. Additivity: an old guest without the member still loads and runs
// ---------------------------------------------------------------------------

/// **The claim the word "additive" rests on.** `tool-legacy` was built against
/// a `tool` world whose `ui-dialogs` had no `questionnaire`. If adding the
/// member broke old artifacts, this would fail to link - and every installed
/// tool extension would break at once, which is the difference between an
/// additive window change and a label bump.
#[test]
fn a_tool_guest_built_before_the_questionnaire_still_loads_and_runs() {
    let mut host = host("legacy");
    let extension = host.load(LEGACY, LEGACY_MANIFEST).unwrap_or_else(|err| {
        panic!(
            "a tool-world guest built before the questionnaire must still load (gh #407's \
             additivity claim): {err}"
        )
    });
    assert_eq!(extension.name(), "tool-legacy");
    assert!(extension.is_enabled(), "and it comes up enabled");
    let specs = extension.tool_specs().expect("its schema still reads");
    assert!(
        !specs.is_empty(),
        "the old guest still registers its tool: {specs:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. The records are the asker's, not the host's
// ---------------------------------------------------------------------------

// Verifies: gh #407's seam rule - the two records live in `lca-permissions`, so
// a caller (an extension's native half, a test, a future host) builds and reads
// them without depending on `lca-ext-host`. This crate depends on both, which
// is exactly why it can prove the asker's shape is the host's shape: the same
// values go in and come out.
#[test]
fn the_question_and_answer_records_cross_the_slot_unchanged() {
    struct Scripted;
    impl DialogPrompt for Scripted {
        fn confirm(&mut self, _title: &str, _message: &str) -> bool {
            false
        }
        fn select(&mut self, _title: &str, _options: &[String]) -> Option<String> {
            None
        }
        fn input(&mut self, _label: &str, _placeholder: Option<&str>) -> Option<String> {
            None
        }
        fn notify(&mut self, _message: &str, _level: &str) {}
        fn questionnaire(&mut self, title: &str, questions: &[Question]) -> Option<Vec<Answer>> {
            assert_eq!(title, "Release check");
            assert_eq!(questions.len(), 2);
            assert!(questions[1].multiple, "the flag survives the crossing");
            assert_eq!(questions[0].options, vec!["sqlite", "postgres"]);
            Some(vec![
                Answer {
                    header: questions[0].header.clone(),
                    value: "sqlite".to_string(),
                    custom: false,
                },
                Answer {
                    header: questions[1].header.clone(),
                    value: "a, b".to_string(),
                    custom: true,
                },
            ])
        }
    }

    let mut slot = SharedDialogs::default();
    slot.set(Arc::new(Mutex::new(Scripted)));
    let asked = vec![
        Question {
            header: "Store".to_string(),
            prompt: "Which store?".to_string(),
            options: vec!["sqlite".to_string(), "postgres".to_string()],
            multiple: false,
        },
        Question {
            header: "Extras".to_string(),
            prompt: "Which extras?".to_string(),
            options: vec!["a".to_string(), "b".to_string()],
            multiple: true,
        },
    ];
    let answers = slot
        .questionnaire("Release check", &asked)
        .expect("answered");
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].value, "sqlite");
    assert!(answers[1].custom, "the custom flag comes back set");
}

// ---------------------------------------------------------------------------
// 3. The inline row reads the same in both shapes
// ---------------------------------------------------------------------------

// Verifies: gh #407 - the questionnaire's inline-editor row and the sequential
// path's custom option are **one wording**, not two copies that drift. The
// host's chrome cannot depend on the extension (the dependency points the other
// way), so the literal exists twice; this row is the pin. A user who sees
// "Type your own answer..." on a one-question ask must see it on a three-tab
// ask too, or the same feature looks like two.
#[test]
fn the_inline_row_and_the_custom_option_are_one_wording() {
    assert_eq!(
        lca_ui::chat_questionnaire::CUSTOM_ROW,
        question::CUSTOM_OPTION,
        "the tabbed inline row and the sequential custom option must read alike"
    );
}

// ---------------------------------------------------------------------------
// 4. Dismissal is an answer, not a hang
// ---------------------------------------------------------------------------

// Verifies: gh #407 - with nothing installed the questionnaire answers
// `None` (dismissed) instead of blocking. That is the headless contract every
// other dialog already keeps, and the one an extension's WASM half depends on:
// a headless run must reach the model with "declined" rather than wait forever
// for a modal nobody can open.
#[test]
fn an_empty_slot_answers_the_questionnaire_as_dismissed() {
    let mut slot = SharedDialogs::default();
    let asked = vec![Question {
        header: "Store".to_string(),
        prompt: "Which store?".to_string(),
        options: Vec::new(),
        multiple: false,
    }];
    assert_eq!(slot.questionnaire("Release check", &asked), None);
}
