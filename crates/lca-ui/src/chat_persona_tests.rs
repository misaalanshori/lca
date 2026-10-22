//! `/persona`: one command sets model, thinking, loadout, and instructions as a
//! bundle - or sets nothing at all (gh #412).
//!
//! The rows are ordered by what they protect. First the atomic guarantee (a
//! refusal leaves the pre-state untouched), because a half-applied persona is
//! worse than a refusal. Then the listing, the notice, and the names: **preset** is
//! reserved for provider endpoints (`docs/glossary.md`), so nothing here may use it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::state::PersonaBundle;

/// What a fake host recorded, so a row can assert the pre-state was untouched.
#[derive(Default)]
struct Recorder {
    checks: AtomicUsize,
    applies: AtomicUsize,
}

/// A chat wired with persona hooks that record and answer as told.
fn chat_with(
    personas: Vec<PersonaBundle>,
    check: impl Fn(&PersonaBundle) -> Result<(), String> + Send + Sync + 'static,
    apply: impl Fn(&PersonaBundle) -> String + Send + Sync + 'static,
) -> (crate::chat::Chat, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let mut chat = super::chat();
    let list = personas;
    chat.world.options.hooks.persona_list = Some(Arc::new(move || list.clone()));
    let seen = recorder.clone();
    chat.world.options.hooks.persona_check = Some(Arc::new(move |bundle| {
        seen.checks.fetch_add(1, Ordering::SeqCst);
        check(bundle)
    }));
    let seen = recorder.clone();
    chat.world.options.hooks.persona_apply = Some(Arc::new(move |bundle| {
        seen.applies.fetch_add(1, Ordering::SeqCst);
        apply(bundle)
    }));
    (chat, recorder)
}

/// A persona carrying all four fields.
fn plan() -> PersonaBundle {
    PersonaBundle {
        name: "plan".to_string(),
        model: "opus".to_string(),
        thinking: Some(Some("high".to_string())),
        tools: Some(vec!["read".to_string(), "grep".to_string()]),
        instructions: Some("You are planning; do not edit files.".to_string()),
    }
}

/// Run `/persona <name>` through the real input path.
fn run(chat: &mut crate::chat::Chat, line: &str) {
    for ch in line.chars() {
        chat.handle_key(&ch.to_string());
    }
    chat.handle_key("\r");
}

fn last_notice(chat: &crate::chat::Chat) -> String {
    chat.last_notice().unwrap_or_default()
}

// ------------------------------------------------------------ atomic apply

// Verifies: gh #412 - applying a valid persona validates the **whole** bundle once,
// applies once, and reports the model, the level and the loadout it set.
#[test]
fn applying_a_persona_sets_every_field_and_reports_them() {
    let (mut chat, recorder) = chat_with(
        vec![plan()],
        |_| Ok(()),
        |bundle| {
            assert_eq!(bundle.model, "opus");
            assert_eq!(bundle.thinking, Some(Some("high".to_string())));
            assert_eq!(
                bundle.tools.as_ref().map(|tools| tools.len()),
                Some(2),
                "the loadout reaches the apply seam intact"
            );
            format!("persona {}: applied", bundle.name)
        },
    );
    run(&mut chat, "/persona plan");
    assert_eq!(recorder.checks.load(Ordering::SeqCst), 1, "checked once");
    assert_eq!(recorder.applies.load(Ordering::SeqCst), 1, "applied once");
    let notice = last_notice(&chat);
    assert!(
        notice.contains("persona plan") && notice.contains("applied"),
        "the notice names the bundle: {notice}"
    );
    // The instructions ride along, attributed and conversational.
    let viewport = super::strip(&chat.viewport(120, 40, 0)).join("\n");
    assert!(
        viewport.contains("persona `plan`: You are planning"),
        "the instructions land with attribution:\n{viewport}"
    );
}

// Verifies: gh #412 design point 1 - **refusal is total.** A persona whose thinking
// level the model does not accept applies nothing: the apply seam is never called.
//
// The row asserts the seam was not reached, not merely that an error appeared -
// "showed an error" is compatible with a half-application, which is the whole
// failure mode this refuses.
#[test]
fn a_refused_persona_applies_nothing_at_all() {
    let (mut chat, recorder) = chat_with(
        vec![plan()],
        |_| Err("`opus` does not offer thinking level `high`".to_string()),
        |bundle| format!("persona {}: applied", bundle.name),
    );
    run(&mut chat, "/persona plan");
    assert_eq!(
        recorder.applies.load(Ordering::SeqCst),
        0,
        "the apply seam is never reached, so nothing was written"
    );
    assert_eq!(recorder.checks.load(Ordering::SeqCst), 1, "it was checked");
    let notice = last_notice(&chat);
    assert!(
        notice.contains("was not applied") && notice.contains("does not offer"),
        "the refusal says why: {notice}"
    );
    assert!(
        !notice.contains("persona plan: applied"),
        "and never claims success: {notice}"
    );
    // The instructions did not ride along either: nothing about a refused persona
    // may reach the transcript as if it applied.
    let viewport = super::strip(&chat.viewport(120, 40, 0)).join("\n");
    assert!(
        !viewport.contains("You are planning"),
        "a refused persona injects no instructions:\n{viewport}"
    );
}

// Verifies: gh #412 - a host that cannot validate refuses rather than applying
// blind. The atomic guarantee is worthless if it is skipped exactly when the
// fields cannot be checked.
#[test]
fn a_host_without_a_check_refuses_instead_of_applying_blind() {
    let mut chat = super::chat();
    let applied = Arc::new(AtomicUsize::new(0));
    chat.world.options.hooks.persona_list = Some(Arc::new(|| vec![plan()]));
    chat.world.options.hooks.persona_check = None;
    let counter = applied.clone();
    chat.world.options.hooks.persona_apply = Some(Arc::new(move |bundle| {
        counter.fetch_add(1, Ordering::SeqCst);
        format!("persona {}: applied", bundle.name)
    }));
    run(&mut chat, "/persona plan");
    assert_eq!(
        applied.load(Ordering::SeqCst),
        0,
        "no check means no apply: unverified fields are not applied"
    );
    assert!(
        last_notice(&chat).contains("cannot validate"),
        "and it says so: {}",
        last_notice(&chat)
    );
}

// -------------------------------------------------------------- the listing

// Verifies: gh #412 design point 4 - `/persona` with no argument lists every
// persona with what it carries. A bundle whose contents you cannot see is a bundle
// you cannot trust.
#[test]
fn the_listing_shows_what_each_persona_carries() {
    let thinking_only = PersonaBundle {
        name: "quiet".to_string(),
        thinking: Some(Some("off".to_string())),
        ..Default::default()
    };
    let no_tools = PersonaBundle {
        name: "chat".to_string(),
        tools: Some(Vec::new()),
        ..Default::default()
    };
    let (mut chat, _recorder) = chat_with(
        vec![plan(), thinking_only, no_tools],
        |_| Ok(()),
        |_| String::new(),
    );
    run(&mut chat, "/persona");
    let viewport = super::strip(&chat.viewport(120, 50, 0)).join("\n");
    for fact in [
        "personas:",
        "plan - opus + high + tools read+grep + instructions",
        "quiet - model unchanged + off + tools unchanged",
        "chat - model unchanged + thinking unchanged + no tools",
        "run /persona <name>",
    ] {
        assert!(
            viewport.contains(fact),
            "the listing shows {fact:?}:\n{viewport}"
        );
    }
}

// Verifies: gh #412 - a listing with nothing to show says how to define one, rather
// than printing an empty header.
#[test]
fn an_empty_listing_says_how_to_define_a_persona() {
    let (mut chat, _recorder) = chat_with(Vec::new(), |_| Ok(()), |_| String::new());
    run(&mut chat, "/persona");
    let notice = last_notice(&chat);
    assert!(
        notice.contains("no personas defined") && notice.contains("[personas."),
        "it points at the config key: {notice}"
    );
}

// Verifies: gh #412 - an unknown name is refused with the names that do exist, so a
// typo is not a silent no-op.
#[test]
fn an_unknown_persona_lists_the_ones_that_exist() {
    let (mut chat, recorder) = chat_with(vec![plan()], |_| Ok(()), |_| "applied".to_string());
    run(&mut chat, "/persona plna");
    let notice = last_notice(&chat);
    assert!(
        notice.contains("no persona named `plna`") && notice.contains("available: plan"),
        "the refusal names the alternatives: {notice}"
    );
    assert_eq!(
        recorder.checks.load(Ordering::SeqCst),
        0,
        "and nothing was even checked"
    );
}

// Verifies: gh #412 - the name match is case-insensitive, so `/persona Plan` is the
// same request as `/persona plan`.
#[test]
fn a_persona_name_matches_regardless_of_case() {
    let (mut chat, recorder) = chat_with(
        vec![plan()],
        |_| Ok(()),
        |bundle| format!("persona {}: applied", bundle.name),
    );
    run(&mut chat, "/persona PLAN");
    assert_eq!(recorder.applies.load(Ordering::SeqCst), 1);
}

// ------------------------------------------------------------- the wording

// Verifies: gh #412's binding terminology note - **preset** is reserved for provider
// endpoint entries (`docs/glossary.md`). The command must not offer it as a spelling,
// and neither must its helper text.
#[test]
fn the_reserved_word_is_not_a_spelling_of_this_command() {
    let (mut chat, _recorder) = chat_with(vec![plan()], |_| Ok(()), |_| String::new());
    run(&mut chat, "/preset plan");
    let viewport = super::strip(&chat.viewport(120, 40, 0)).join("\n");
    // `/preset` is not a persona command; whatever it does, it must not reach the
    // persona hooks, and the unknown-command line must not point at personas.
    assert!(
        !viewport.contains("persona plan"),
        "`/preset` must not route to personas:\n{viewport}"
    );
    // And the help text for `/persona` does not use the reserved word.
    let help = crate::chat_commands::command_help("/persona");
    assert!(!help.is_empty(), "/persona has a help line");
    assert!(
        !help.to_lowercase().contains("preset"),
        "the help line does not borrow the reserved word: {help}"
    );
}

// Verifies: gh #412 design point 3 - the applied notice is a **full statement**, not
// a diff. A persona that matches the current state still reports what it set, because
// a diff against an identical state reads as "nothing happened".
#[test]
fn the_applied_notice_states_the_bundle_not_a_diff() {
    let notice = lca_ui_summary(&plan());
    for fact in ["opus", "high", "tools read+grep", "instructions"] {
        assert!(
            notice.contains(fact),
            "the summary names {fact:?}: {notice}"
        );
    }
    assert!(
        !notice.contains("unchanged"),
        "and says nothing about leaving things alone when it sets all four: {notice}"
    );
}

fn lca_ui_summary(bundle: &PersonaBundle) -> String {
    crate::chat_persona::summary(bundle)
}
