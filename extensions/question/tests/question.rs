//! Model-facing interactive questions (gh #221): the model asks
//! structured clarification questions mid-turn through the host's
//! dialogs, and resumes with the answers in context.
//!
//! Verifies: single and multiple picks, the custom-answer option,
//! free-text-only questions, declines degrade to marked answers
//! (headless-safe: no dialog installed means declined, never a
//! hang), and the answer text matches the documented shape.

use std::cell::RefCell;
use std::collections::VecDeque;

use question::{Dialogs, OptionItem, Question, ask_all};

/// A scripted dialog host: canned selects and inputs in order, plus the
/// tabbed questionnaire - which records that it was called, what it was asked,
/// and answers from its own script (gh #407).
#[derive(Default)]
struct Scripted {
    selects: RefCell<VecDeque<Option<String>>>,
    inputs: RefCell<VecDeque<Option<String>>>,
    /// One entry per questionnaire call: the title and the questions it got.
    tabbed: RefCell<Vec<(String, Vec<lca_permissions::Question>)>>,
    /// The answers the next questionnaire call returns; `None` dismisses.
    answers: RefCell<VecDeque<Option<Vec<lca_permissions::Answer>>>>,
}

impl Scripted {
    fn new(selects: Vec<Option<&str>>, inputs: Vec<Option<&str>>) -> Scripted {
        let owned = |items: Vec<Option<&str>>| {
            items
                .into_iter()
                .map(|item| item.map(str::to_string))
                .collect()
        };
        Scripted {
            selects: RefCell::new(owned(selects)),
            inputs: RefCell::new(owned(inputs)),
            ..Scripted::default()
        }
    }

    /// A host that answers one tabbed ask with `answers`, and nothing else.
    fn tabbed(answers: Option<Vec<(&str, &str, bool)>>) -> Scripted {
        let scripted = Scripted::default();
        scripted.answers.borrow_mut().push_back(answers.map(|rows| {
            rows.into_iter()
                .map(|(header, value, custom)| lca_permissions::Answer {
                    header: header.to_string(),
                    value: value.to_string(),
                    custom,
                })
                .collect()
        }));
        scripted
    }

    /// How many tabbed asks reached this host.
    fn tabbed_calls(&self) -> usize {
        self.tabbed.borrow().len()
    }
}

impl Dialogs for Scripted {
    fn select(&self, _title: &str, _options: &[String]) -> Option<String> {
        self.selects.borrow_mut().pop_front().flatten()
    }

    fn input(&self, _label: &str, _placeholder: Option<&str>) -> Option<String> {
        self.inputs.borrow_mut().pop_front().flatten()
    }

    fn questionnaire(
        &self,
        title: &str,
        questions: &[lca_permissions::Question],
    ) -> Option<Vec<lca_permissions::Answer>> {
        self.tabbed
            .borrow_mut()
            .push((title.to_string(), questions.to_vec()));
        self.answers.borrow_mut().pop_front().flatten()
    }
}

fn db_question() -> Question {
    Question {
        header: "Database Engine".to_string(),
        question: "Which database engine for local persistence?".to_string(),
        options: vec![
            OptionItem {
                label: "SQLite".to_string(),
                description: "Single-file, embedded, zero setup (Recommended)".to_string(),
            },
            OptionItem {
                label: "PostgreSQL".to_string(),
                description: "Client-server, production-grade".to_string(),
            },
        ],
        multiple: false,
    }
}

// Verifies: one pick from the list lands in the documented answer
// shape, and the turn can continue from the text alone.
#[test]
fn single_pick_returns_the_documented_shape() {
    let dialogs = Scripted::new(
        vec![Some(
            "SQLite — Single-file, embedded, zero setup (Recommended)",
        )],
        vec![],
    );
    let text = ask_all(&dialogs, &[db_question()]);
    assert_eq!(
        text,
        "User has answered your questions: \"Database Engine\" = \"SQLite\". You can now continue with the user's answers in mind."
    );
}

// Verifies: the custom-answer option routes to a free-text input
// whose text lands verbatim.
#[test]
fn custom_answer_routes_to_free_text() {
    let dialogs = Scripted::new(
        vec![Some(question::CUSTOM_OPTION)],
        vec![Some("Use Turso.")],
    );
    let text = ask_all(&dialogs, &[db_question()]);
    assert!(
        text.contains("\"Database Engine\" = \"Use Turso.\""),
        "{text}"
    );
}

// Verifies: multiple picks accumulate until Done, in order.
#[test]
fn multiple_picks_accumulate_until_done() {
    let question = Question {
        multiple: true,
        ..db_question()
    };
    let dialogs = Scripted::new(
        vec![
            Some("SQLite — Single-file, embedded, zero setup (Recommended)"),
            Some("PostgreSQL — Client-server, production-grade"),
            Some(question::DONE_OPTION),
        ],
        vec![],
    );
    let text = ask_all(&dialogs, &[question]);
    assert!(
        text.contains("\"Database Engine\" = \"SQLite, PostgreSQL\""),
        "{text}"
    );
}

// Verifies: a question with no options is pure free text.
#[test]
fn optionless_questions_are_free_text() {
    let question = Question {
        options: Vec::new(),
        ..db_question()
    };
    let dialogs = Scripted::new(vec![], vec![Some("Whatever you like.")]);
    let text = ask_all(&dialogs, &[question]);
    assert!(
        text.contains("\"Database Engine\" = \"Whatever you like.\""),
        "{text}"
    );
}

// Verifies: a dismissed dialog degrades to a marked decline — the
// headless shape, where no dialog is installed and every ask
// declines, never hangs.
#[test]
fn declines_degrade_to_marked_answers() {
    let dialogs = Scripted::new(vec![None], vec![]);
    let text = ask_all(&dialogs, &[db_question()]);
    assert!(
        text.contains("\"Database Engine\" = \"(declined)\""),
        "{text}"
    );
}

// Verifies: gh #407 - one question never grows tabs. The single-question shape
// is the sequential picker, which is what the model-facing tool promises for
// `1Q` and what every existing single-question ask costs; routing it through
// the questionnaire would add a review-and-submit step to a one-line answer.
#[test]
fn one_question_still_uses_the_sequential_path() {
    let dialogs = Scripted::new(
        vec![Some(
            "SQLite — Single-file, embedded, zero setup (Recommended)",
        )],
        vec![],
    );
    let text = ask_all(&dialogs, &[db_question()]);
    assert!(text.contains("\"Database Engine\" = \"SQLite\""), "{text}");
    assert_eq!(
        dialogs.tabbed_calls(),
        0,
        "a one-question ask must not open the tabbed questionnaire"
    );
}

// Verifies: gh #407 - three questions cost **one** questionnaire call, and the
// answers come back keyed by header in the asker's order. This is the whole
// point of the slice: N questions, one turn, one submit.
#[test]
fn three_questions_use_one_questionnaire_call() {
    let second = Question {
        header: "Priority".to_string(),
        question: "How soon?".to_string(),
        options: vec![
            OptionItem {
                label: "Now".to_string(),
                description: String::new(),
            },
            OptionItem {
                label: "Later".to_string(),
                description: String::new(),
            },
        ],
        multiple: false,
    };
    let dialogs = Scripted::tabbed(Some(vec![
        (
            "Database Engine",
            "SQLite — Single-file, embedded, zero setup (Recommended)",
            false,
        ),
        ("Priority", "Now", false),
    ]));
    let text = ask_all(&dialogs, &[db_question(), second]);
    assert_eq!(
        dialogs.tabbed_calls(),
        1,
        "three questions are one modal: {text}"
    );
    let asked = dialogs.tabbed.borrow();
    let (title, questions) = &asked[0];
    assert_eq!(title, question::QUESTIONNAIRE_TITLE);
    assert_eq!(questions.len(), 2);
    assert_eq!(
        questions[0].options[0], "SQLite — Single-file, embedded, zero setup (Recommended)",
        "the tab shows the same wording the sequential picker does"
    );
    assert_eq!(
        text,
        "User has answered your questions: \"Database Engine\" = \"SQLite\", \
         \"Priority\" = \"Now\". You can now continue with the user's answers in mind.",
        "the report is the sequential path's text, value for value"
    );
}

// Verifies: gh #407 - a dismissed questionnaire answers like a dismissed
// picker: every question reports `(declined)` and the turn continues, which is
// also the headless shape (an empty slot answers `None`).
#[test]
fn a_dismissed_questionnaire_answers_like_a_dismissed_picker() {
    let second = Question {
        header: "Priority".to_string(),
        ..db_question()
    };
    let dialogs = Scripted::tabbed(None);
    let text = ask_all(&dialogs, &[db_question(), second]);
    assert_eq!(
        text,
        "User has answered your questions: \"Database Engine\" = \"(declined)\", \
         \"Priority\" = \"(declined)\". You can now continue with the user's answers in mind.",
        "{text}"
    );
}

// Verifies: gh #407 - a multi-pick answer maps back to the labels the
// sequential path reports, and a label that itself contains `", "` survives the
// host's join instead of being split in the wrong place. The host joins the
// *display* strings this crate sent, so the walk parses a language it defined;
// a naive `split(", ")` would turn one pick into two answers.
#[test]
fn a_multi_pick_answers_in_labels_and_survives_a_comma_in_one() {
    let question_with_comma = Question {
        header: "Deploy".to_string(),
        question: "Where?".to_string(),
        options: vec![
            OptionItem {
                label: "eu-west, primary".to_string(),
                description: "the first region".to_string(),
            },
            OptionItem {
                label: "us-east".to_string(),
                description: String::new(),
            },
        ],
        multiple: true,
    };
    // The host joins the display strings it was given with ", ".
    let joined = "eu-west, primary — the first region, us-east";
    let dialogs = Scripted::tabbed(Some(vec![("Deploy", joined, false)]));
    let text = ask_all(&dialogs, &[db_question(), question_with_comma]);
    assert!(
        text.contains("\"Deploy\" = \"eu-west, primary, us-east\""),
        "both labels, joined in toggle order: {text}"
    );
}

// Verifies: gh #407 - a typed answer is reported verbatim (it was never an
// option, so there is no label to map it to), the same value the sequential
// custom arm produces.
#[test]
fn a_written_answer_is_reported_verbatim() {
    let dialogs = Scripted::tabbed(Some(vec![
        ("Database Engine", "Use Turso.", true),
        ("Priority", "Now", false),
    ]));
    let second = Question {
        header: "Priority".to_string(),
        options: vec![OptionItem {
            label: "Now".to_string(),
            description: String::new(),
        }],
        ..db_question()
    };
    let text = ask_all(&dialogs, &[db_question(), second]);
    assert!(
        text.contains("\"Database Engine\" = \"Use Turso.\""),
        "{text}"
    );
}

// Verifies: several questions join into one report (the sequential arm: one
// question at a time, which is still the path for a one-question ask).
#[test]
fn several_questions_join_into_one_report() {
    let second = Question {
        header: "Delete tests".to_string(),
        question: "Remove the deprecated tests?".to_string(),
        options: vec![OptionItem {
            label: "Yes".to_string(),
            description: String::new(),
        }],
        multiple: false,
    };
    let dialogs = Scripted::new(
        vec![
            Some("SQLite — Single-file, embedded, zero setup (Recommended)"),
            Some("Yes"),
        ],
        vec![],
    );
    // Two questions now take one questionnaire call (gh #407), so the
    // sequential join is exercised one question at a time - which is the only
    // way it is reached now, and still the shape a 1Q ask costs.
    let first = ask_all(&dialogs, &[db_question()]);
    let second_text = ask_all(&dialogs, &[second]);
    assert!(
        first.contains("\"Database Engine\" = \"SQLite\""),
        "{first}"
    );
    assert!(
        second_text.contains("\"Delete tests\" = \"Yes\""),
        "{second_text}"
    );
}

// Verifies: argument parsing — questions required, options optional,
// `multiple` defaulting to false.
#[test]
fn arguments_parse_with_sane_defaults() {
    let questions = question::parse_question_args(
        r#"{"questions": [{"header": "H", "question": "Q?", "options": [{"label": "A"}]}]}"#,
    )
    .expect("parse");
    assert_eq!(questions.len(), 1);
    assert!(!questions[0].multiple);
    assert_eq!(questions[0].options[0].description, "");
    assert!(question::parse_question_args(r#"{"questions": []}"#).is_err());
    assert!(question::parse_question_args(r#"{}"#).is_err());
    assert!(question::parse_question_args(r#"{"questions": [{"header": "H"}]}"#).is_err());
}

// Verifies: the manifest test contract — the tool world, no
// capabilities (dialogs ride the world's import, not the manifest).
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = question::MANIFEST;
    assert!(manifest.contains("worlds = [\"tool\"]"), "{manifest}");
    assert!(!manifest.contains("[capabilities."), "{manifest}");
}

// ---------------------------------------------------------------------------
// gh #298: one schema, two delivery modes
// ---------------------------------------------------------------------------

// Verifies: gh #298 acceptance 3 - the native spec and the WASM
// `get_schema` declare the *same* parameters, compared for exact
// equality so the row cannot rot the way "contains `items`" would.
//
// Before the fix this failed: the WASM side carried a hand-written
// string with no `items` at all, so a WASM-mode model was told the
// array was untyped.
#[test]
fn the_wasm_schema_is_exactly_the_native_schema() {
    let native = question::question_tool_spec();
    assert_eq!(
        question::question_schema_parameters(),
        native.parameters.to_string(),
        "the two delivery modes must declare one schema"
    );
}

// Verifies: gh #298 acceptance 1 - the schema a WASM-mode model receives
// names the structure `parse_question_args` actually requires.
#[test]
fn the_wasm_schema_declares_the_items_shape_the_parser_requires() {
    let parameters: serde_json::Value =
        serde_json::from_str(&question::question_schema_parameters()).expect("valid JSON");
    let items = &parameters["properties"]["questions"]["items"];
    assert_eq!(items["type"], "object", "{parameters}");
    assert_eq!(items["required"], serde_json::json!(["header", "question"]));
    assert_eq!(items["properties"]["header"]["type"], "string");
    assert_eq!(items["properties"]["question"]["type"], "string");
    assert_eq!(items["properties"]["options"]["type"], "array");
    assert_eq!(
        items["properties"]["options"]["items"]["required"],
        serde_json::json!(["label"])
    );
    assert_eq!(items["properties"]["multiple"]["type"], "boolean");
}

// Verifies: gh #298 - the plain-string array the truncated WASM schema
// invited is rejected by the guest's own parser, the user-visible half of
// the failure. The host-schema half (gh #306's `items` enforcement
// catching that same answer) lives beside the other validator rows in
// `tests/regressions/298-*`, where `lca-provider` is already a test
// dependency - this crate stays dependency-free.
#[test]
fn a_plain_string_array_is_rejected_by_the_parser() {
    assert!(
        question::parse_question_args(r#"{"questions":["Which port?"]}"#).is_err(),
        "the shape the truncated schema invited never parses"
    );
}

// Verifies: gh #298 - a structured answer from the *shared* schema
// round-trips through the guest's parser unchanged. This is the path
// acceptance 2 promises: what the model is told to send is what the
// tool accepts.
#[test]
fn a_structured_answer_from_the_shared_schema_parses() {
    let parsed = question::parse_question_args(
        r#"{"questions":[{"header":"Database Engine","question":"Which?","options":[{"label":"SQLite","description":"embedded"}],"multiple":false}]}"#,
    )
    .expect("the schema's own shape parses");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].header, "Database Engine");
    assert_eq!(parsed[0].options[0].label, "SQLite");
    assert_eq!(parsed[0].options[0].description, "embedded");
    assert!(!parsed[0].multiple);
}
