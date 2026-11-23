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
//! ## What "additive for old artifacts" means now that 0.7 exists
//!
//! The interface is one every `tool` and `tool-catalog` guest already imports
//! (gh #172), so a *rebuilt* guest sees the new member - and a guest that never
//! calls it is unaffected, because the host satisfies an import whether or not
//! the guest calls it (the rule `tools` established in gh #77 and `net`
//! repeated in gh #302). That is a **within-line** property: it held while the
//! package stayed `lca:host@0.6.0`.
//!
//! Opening the 0.7 line changes that, and this file was rewritten to say so
//! rather than keep a proof that stopped being true. Renaming the package
//! (`lca:host@0.6.0` -> `@0.7.0`) renames every imported and exported instance,
//! so an artifact built for 0.6 no longer links: the window admits its
//! *manifest*, and the link refuses its *bytes* with advice to rebuild. That is
//! what a line move is, and it is why a released line is frozen.
//!
//! ## What this guard pins
//!
//! 1. **The line move is legible.** A 0.6-built component (the frozen artifact
//!    in `tests/data/abi-0.6/`) passes the manifest window and is then refused
//!    at link with a message naming the line and the fix.
//! 2. **The seam is nameable from outside.** The two records are
//!    `lca-permissions` types, so an asker can build them without depending on
//!    `lca-ext-host` (the brief's rule, and what `question`'s native and WASM
//!    halves both need).
//! 3. **The slot forwards, and answers dismissed when empty.** A dismissal is
//!    `None`, never a hang - the headless contract, one rung deeper than the
//!    `confirm`/`select`/`input` rows.
//!
//! Verifies: gh #407's ABI claim, ADR-0028 (the 0.7 line opens in place of a
//! frozen 0.6), FR-EXT-1, NFR-19 (the window), NFR-20 (a breaking change is a line).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_permissions::{Answer, DialogPrompt, Question, SharedDialogs};
use std::sync::{Arc, Mutex};

/// A component built for **0.6**, frozen before this cycle's line move and kept
/// outside `extensions/*/fixtures/` for that reason alone
/// (`tests/data/abi-0.6/README.md` says why it cannot be rebuilt).
const FROZEN_0_6: &[u8] = include_bytes!("../data/abi-0.6/tool-legacy.wasm");
/// Its manifest, which declares `abi = "0.6"` - inside the window, on purpose.
const FROZEN_0_6_MANIFEST: &str = include_str!("../data/abi-0.6/tool-legacy.manifest.toml");

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

/// **What a line move does to an installed extension**, proven on the artifact
/// that used to prove the opposite. `tests/data/abi-0.6/tool-legacy.wasm` was
/// the witness for "adding an import or a member does not break an old guest"
/// (gh #77, gh #302, and this file's first version). On a 0.7 host it must still
/// *pass the manifest window* - that is the promise NFR-19 makes - and must then
/// be refused at link, because the component's type names `lca:host@0.6.0` and
/// this host provides `@0.7.0`.
///
/// Both halves matter: a manifest-only refusal would be a different (and
/// weaker) rule than the one the window states, and a silent load would mean the
/// version in the package name is not load-bearing at all.
#[test]
fn a_component_built_for_the_previous_line_is_admitted_by_the_window_and_refused_at_link() {
    let declaration =
        lca_ext_host::Manifest::parse(FROZEN_0_6_MANIFEST).expect("the 0.6 manifest parses");
    assert_eq!(declaration.abi, "0.6");
    assert!(
        declaration.abi_in_window(),
        "the window admits the 0.6 line (NFR-19); that is what makes the link refusal \
         the interesting half"
    );

    let mut host = host("frozen-0-6");
    let err = match host.load(FROZEN_0_6, FROZEN_0_6_MANIFEST) {
        Ok(_) => panic!("a 0.6-built component cannot link on a 0.7 host"),
        Err(err) => err,
    };
    let text = err.to_string();
    assert!(
        text.contains("Rebuild it"),
        "the refusal tells the author what to do: {text}"
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
