//! Gh #407's questionnaire rows: the tab row, per-tab state, the axis rule,
//! the two-stage escape, the submit gate, and the escapes budget.
//!
//! Beside the widget (gh #572: terms the production seams; the state machine
//! and its rows are one subject), and on the fake terminal rather than a pane -
//! the e2e row in `crates/lca-cli/tests/e2e_questionnaire_407.rs` proves the
//! same contract through a real tmux pane.
//!
//! Every row asserts a *rendered* row or a delivered answer, never a field:
//! state that the user cannot see is not a feature, and an answer the asker
//! does not receive is not an answer.

use crate::chat::tests_support::chat;
use crate::state::{DialogExchange, DialogModal};
use lca_protocol::{DialogAnswer, UiDialog, UiQuestion};
use lca_tui::engine::text::strip_terminal_sequences;

/// Three questions with two options each - the shape the e2e row drives too.
fn three_questions() -> Vec<UiQuestion> {
    ["Scope", "Priority", "Retries"]
        .iter()
        .enumerate()
        .map(|(index, header)| UiQuestion {
            header: header.to_string(),
            prompt: format!("question {index}: which {header}?"),
            options: vec!["first".to_string(), "second".to_string()],
            multiple: false,
        })
        .collect()
}

/// One question, `count` options - for the width row and the multi-pick rows.
fn question(header: &str, count: usize, multiple: bool) -> UiQuestion {
    UiQuestion {
        header: header.to_string(),
        prompt: format!("which {header}?"),
        options: (1..=count).map(|n| format!("option {n}")).collect(),
        multiple,
    }
}

/// Open a questionnaire the way the drain does, and hand back its answer
/// channel so a row can assert what the asker receives.
fn open(
    chat: &mut crate::chat::Chat,
    questions: &[UiQuestion],
) -> std::sync::mpsc::Receiver<DialogAnswer> {
    let (respond, response) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(DialogModal {
        exchange: DialogExchange {
            dialog: UiDialog::Questionnaire {
                title: "Setup".to_string(),
                questions: questions.to_vec(),
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
        questionnaire: Some(crate::chat_questionnaire::Questionnaire::new(questions)),
    });
    response
}

/// The painted frame, escapes stripped.
fn frame(chat: &mut crate::chat::Chat, width: u16, height: u16) -> Vec<String> {
    chat.viewport(width, height, 0)
        .iter()
        .map(|row| strip_terminal_sequences(row))
        .collect()
}

/// The painted frame as one string.
fn text(chat: &mut crate::chat::Chat, width: u16, height: u16) -> String {
    frame(chat, width, height).join("\n")
}

/// The body of the active question, from the choice list on: the rows the
/// keyboard contract is about, without the transcript around them.
fn visible(chat: &mut crate::chat::Chat) -> String {
    text(chat, 80, 24)
}

// ---------------------------------------------------------------------------
// 1. One modal, three tabs
// ---------------------------------------------------------------------------

// Verifies: gh #407 acceptance - a three-question ask costs **one** modal with
// three tabs, not three sequential pickers. Before this cycle the extension
// looped `select`/`input` per question, so the same ask opened three modals in
// a row and the second question only appeared once the first was answered.
#[test]
fn a_three_question_ask_opens_one_modal_with_three_tabs() {
    let mut chat = chat();
    let _answers = open(&mut chat, &three_questions());
    let painted = visible(&mut chat);
    assert_eq!(
        painted.matches("Setup").count(),
        1,
        "one modal, not one per question:\n{painted}"
    );
    for header in ["Scope", "Priority", "Retries"] {
        assert!(
            painted.contains(header),
            "the tab row names {header}:\n{painted}"
        );
    }
    assert!(
        painted.contains("question 0: which Scope?"),
        "the first question's prompt is the body:\n{painted}"
    );
    assert!(
        !painted.contains("question 1: which Priority?"),
        "the other questions are behind their tabs, not stacked:\n{painted}"
    );
}

// ---------------------------------------------------------------------------
// 2. The tab row's markers
// ---------------------------------------------------------------------------

// Verifies: gh #407 - the tab row carries the answered/pending state (`■`/`□`)
// and the submit end-cap, so N questions have a visible progress without a
// per-tab visit.
#[test]
fn the_tab_row_marks_answered_pending_and_submit() {
    let mut chat = chat();
    let _answers = open(&mut chat, &three_questions());
    let before = visible(&mut chat);
    assert_eq!(
        before.matches('■').count(),
        0,
        "nothing is answered yet:\n{before}"
    );
    assert_eq!(
        before.matches('□').count(),
        3,
        "three pending tabs:\n{before}"
    );
    assert!(before.contains("Submit"), "the end-cap exists:\n{before}");

    // Answer the first question with its first option.
    chat.handle_key("\r");
    let after = visible(&mut chat);
    assert_eq!(
        after.matches('■').count(),
        1,
        "the answered tab flips its marker:\n{after}"
    );
    assert_eq!(
        after.matches('□').count(),
        2,
        "the rest stay pending:\n{after}"
    );
    assert!(
        after.contains("■ Scope"),
        "the marker belongs to the answered tab:\n{after}"
    );
}

// ---------------------------------------------------------------------------
// 3. Per-tab state survives a round trip
// ---------------------------------------------------------------------------

// Verifies: gh #407 - each tab keeps its highlight, scroll offset, draft and
// answer, and restores them on return. This is what makes tabs worth having;
// without it a tab is a re-render of the same list.
#[test]
fn tab_and_arrows_switch_tabs_and_preserve_each_tabs_state() {
    let mut chat = chat();
    let _answers = open(&mut chat, &three_questions());

    // Question 1: drop the highlight to the second option, then walk away.
    chat.handle_key("\x1b[B");
    assert!(
        visible(&mut chat).contains("> 2. second"),
        "the highlight moved:\n{}",
        visible(&mut chat)
    );
    chat.handle_key("\t");
    assert!(
        !visible(&mut chat).contains("> 2. second") && visible(&mut chat).contains("> 1. first"),
        "a fresh tab starts at its own first row:\n{}",
        visible(&mut chat)
    );

    // Question 2: type into its editor and leave the draft there.
    chat.handle_key("\x1b[B");
    chat.handle_key("\x1b[B"); // third row: the inline editor
    assert!(
        visible(&mut chat).contains("Type your own answer..."),
        "the editor row is the last one:\n{}",
        visible(&mut chat)
    );
    chat.handle_key("\r");
    for character in "kept draft".chars() {
        chat.handle_key(&character.to_string());
    }
    assert!(visible(&mut chat).contains("kept draft"));

    // Back to question 1 with Shift+Tab, then forward again.
    chat.handle_key("\x1b[Z");
    assert!(
        visible(&mut chat).contains("> 2. second"),
        "the first tab restored its highlight:\n{}",
        visible(&mut chat)
    );
    chat.handle_key("\t");
    assert!(
        visible(&mut chat).contains("kept draft"),
        "the second tab restored its draft and its editor baton:\n{}",
        visible(&mut chat)
    );

    // And the restored highlight is the one Enter picks.
    chat.handle_key("\x1b");
    chat.handle_key("\x1b[Z");
    chat.handle_key("\r");
    let after = visible(&mut chat);
    assert!(
        after.contains("question 1: which Priority?"),
        "answering advances to the next question:\n{after}"
    );
}

// ---------------------------------------------------------------------------
// 4. The axis rule in the editor
// ---------------------------------------------------------------------------

// Verifies: gh #407's axis separation - while the inline editor holds the field
// baton `←`/`→` move the text cursor and `Tab` still switches tabs. Both halves
// because either alone is a different bug: arrows that switch tabs make the
// cursor uneditable, and a `Tab` that types makes the next tab unreachable.
#[test]
fn the_inline_editor_takes_the_arrows_and_esc_returns_to_the_options() {
    let mut chat = chat();
    let _answers = open(&mut chat, &three_questions());
    // Into the editor (third row of question 1), then type, then step left.
    chat.handle_key("\x1b[B");
    chat.handle_key("\x1b[B");
    chat.handle_key("\r");
    chat.handle_key("a");
    chat.handle_key("c");
    // `←` moves the cursor, not the tab: the next character lands before `c`.
    chat.handle_key("\x1b[D");
    assert!(
        visible(&mut chat).contains("question 0: which Scope?"),
        "the left arrow did not leave the question:\n{}",
        visible(&mut chat)
    );
    // The help line under a focused field is the *editor's*, not the options':
    // a second row saying `↑↓ select` under a field the arrows now move a
    // cursor in is a lie about the keys (pi omits it for the same reason).
    assert!(
        visible(&mut chat).contains("Enter submits; Esc goes back."),
        "the editor paints its own keys:\n{}",
        visible(&mut chat)
    );
    assert!(
        !visible(&mut chat).contains("↑↓ select"),
        "and the options' help is not repeated under it:\n{}",
        visible(&mut chat)
    );
    chat.handle_key("b");
    chat.handle_key("\r");
    // Answering from the editor is custom, and the value is what was typed.
    let after = visible(&mut chat);
    assert!(
        after.contains("question 1: which Priority?"),
        "the editor's Enter answers and advances:\n{after}"
    );
    chat.handle_key("\t"); // → question 3
    chat.handle_key("\t"); // → the review
    let review = visible(&mut chat);
    assert!(
        review.contains("Scope: (wrote) abc"),
        "the cursor moved inside the value, so `abc` came back:\n{review}"
    );

    // And `Tab` from inside the editor still switches, so no tab is stranded.
    let mut second = crate::chat::tests_support::chat();
    let _answers = open(&mut second, &three_questions());
    second.handle_key("\x1b[B");
    second.handle_key("\x1b[B");
    second.handle_key("\r");
    second.handle_key("\t");
    assert!(
        visible(&mut second).contains("question 1: which Priority?"),
        "Tab switches even while the editor holds the baton:\n{}",
        visible(&mut second)
    );
}

// ---------------------------------------------------------------------------
// 5. The submit gate
// ---------------------------------------------------------------------------

// Verifies: gh #407 - the submit end-cap refuses while a question is
// unanswered, and the refusal **jumps to the first unanswered tab** instead of
// doing nothing. A gate that only beeps leaves the user to find the gap.
#[test]
fn submit_refuses_while_a_question_is_unanswered_and_jumps_to_it() {
    let mut chat = chat();
    let answers = open(&mut chat, &three_questions());
    // Answer question 1 only (its Enter advances to Q2), then walk to the
    // review by hand: two more tabs, because the answer already moved one.
    chat.handle_key("\r");
    chat.handle_key("\t"); // Q3 (unanswered)
    chat.handle_key("\t"); // the end-cap
    assert!(
        visible(&mut chat).contains("Unanswered: Priority, Retries"),
        "the review names what is missing:\n{}",
        visible(&mut chat)
    );
    chat.handle_key("\r");
    assert_eq!(answers.try_recv().ok(), None, "the gate refused to submit");
    assert!(
        visible(&mut chat).contains("question 1: which Priority?"),
        "the refusal landed on the first unanswered question:\n{}",
        visible(&mut chat)
    );

    // Answer the remaining two, then submit.
    chat.handle_key("\r"); // answers Priority with `first`
    chat.handle_key("\r"); // answers Retries with `first`
    chat.handle_key("\r"); // the end-cap now submits
    let delivered = answers.try_recv().ok();
    let Some(DialogAnswer::Questionnaire(Some(delivered))) = delivered else {
        panic!("expected the answers, got {delivered:?}");
    };
    let values: Vec<(String, String)> = delivered
        .into_iter()
        .map(|answer| (answer.header, answer.value))
        .collect();
    assert_eq!(
        values,
        vec![
            ("Scope".to_string(), "first".to_string()),
            ("Priority".to_string(), "first".to_string()),
            ("Retries".to_string(), "first".to_string()),
        ],
        "every answer, in the asker's order"
    );
    assert!(
        chat.world.dialog.is_none(),
        "the submitted ask closes its modal"
    );
}

// ---------------------------------------------------------------------------
// 6. The review marks a written answer
// ---------------------------------------------------------------------------

// Verifies: gh #407 - a value that came from the inline editor is marked
// `(wrote)` in the review, pi's distinction: the asker (and the user) can tell
// a chosen option from one that was typed.
#[test]
fn the_review_marks_a_custom_answer_as_wrote() {
    let mut chat = chat();
    let answers = open(&mut chat, &three_questions());
    // Q1 through the editor.
    chat.handle_key("\x1b[B");
    chat.handle_key("\x1b[B");
    chat.handle_key("\r");
    for character in "Turso".chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
    // Q2 and Q3 from their lists.
    chat.handle_key("\r");
    chat.handle_key("\r");
    let review = visible(&mut chat);
    assert!(
        review.contains("Scope: (wrote) Turso"),
        "the review marks the written answer:\n{review}"
    );
    assert!(
        review.contains("Priority: first"),
        "and leaves a chosen one unmarked:\n{review}"
    );
    chat.handle_key("\r");
    let Some(DialogAnswer::Questionnaire(Some(delivered))) = answers.try_recv().ok() else {
        panic!("expected answers");
    };
    assert!(delivered[0].custom, "the written answer carries custom");
    assert!(!delivered[1].custom, "a chosen one does not");
    assert_eq!(delivered[0].value, "Turso");
}

// ---------------------------------------------------------------------------
// 7. Dismissal
// ---------------------------------------------------------------------------

// Verifies: gh #407's two-stage escape - the first `Esc` from the editor
// returns to the options and the modal stays open; the second dismisses the
// whole ask, which the asker reports as declined (never a hang).
#[test]
fn a_dismissal_answers_nothing_and_the_tool_reports_it() {
    let mut chat = chat();
    let answers = open(&mut chat, &three_questions());
    chat.handle_key("\x1b[B");
    chat.handle_key("\x1b[B");
    chat.handle_key("\r"); // into the editor
    assert!(chat.world.dialog.is_some());
    chat.handle_key("\x1b"); // stage one: back to the options
    assert!(
        chat.world.dialog.is_some(),
        "the first escape must not close the modal"
    );
    assert!(
        visible(&mut chat).contains("> 3. Type your own answer..."),
        "it put the highlight back on the editor row:\n{}",
        visible(&mut chat)
    );
    // From the editor again, the same key's second stage is the dismissal.
    chat.handle_key("\r");
    chat.handle_key("\x1b");
    chat.handle_key("\x1b");
    assert_eq!(
        answers.try_recv().ok(),
        Some(DialogAnswer::Questionnaire(None)),
        "the dismissal answers nothing"
    );
    assert!(chat.world.dialog.is_none(), "and closes the modal");
}

// ---------------------------------------------------------------------------
// 8. Twelve questions fit
// ---------------------------------------------------------------------------

// Verifies: gh #407 - a twelve-question ask must not overflow the modal. The
// tab row is clipped to the modal's inner width with an ellipsis, and the frame
// stays closed on both edges (a clipped row is a `…`, never a broken border).
#[test]
fn a_twelve_question_ask_never_exceeds_the_modal_width() {
    let many: Vec<UiQuestion> = (1..=12)
        .map(|n| question(&format!("Question about topic {n} in detail"), 2, false))
        .collect();
    let mut chat = chat();
    let _answers = open(&mut chat, &many);
    let (width, height) = (60u16, 24u16);
    let painted = frame(&mut chat, width, height);
    for row in &painted {
        assert!(
            lca_tui::engine::text::visible_width(row) <= width as usize,
            "no row is wider than the terminal: {row:?}\n{}",
            painted.join("\n")
        );
    }
    let tabs = painted
        .iter()
        .find(|row| row.contains('←'))
        .unwrap_or_else(|| panic!("the tab row is painted:\n{}", painted.join("\n")));
    assert!(
        tabs.contains('…'),
        "headers that do not fit are elided: {tabs:?}"
    );
    assert!(
        !tabs.contains("topic 12"),
        "the elision is what dropped the tail, not the width: {tabs:?}"
    );
    assert!(
        tabs.trim_end().ends_with('│'),
        "the modal's frame still closes: {tabs:?}"
    );
    // The elision must land *inside* the row: a tab row that exactly fills the
    // inner width and then grows an ellipsis is one cell too wide, and the
    // modal's painter wraps it - which is what a 70-column pane showed as a
    // lone `…` on the row below the tabs.
    for row in &painted {
        let body = row
            .trim()
            .trim_start_matches('│')
            .trim()
            .trim_end_matches('│')
            .trim();
        assert_ne!(
            body,
            "…",
            "the tab row was not wrapped:\n{}",
            painted.join("\n")
        );
    }

    // The boundary, swept rather than guessed: at *some* width the tab row's
    // tiles land exactly on the inner edge, and an ellipsis added on top of them
    // is one cell too wide - the modal's painter then wraps it, leaving a lone
    // `…` under the tabs. That is what a 70-column pane showed before the clip
    // reserved the ellipsis' own cell, and a single width would miss it the next
    // time a label or a theme changes.
    for width in (40u16..=120).step_by(2) {
        let mut narrow = crate::chat::tests_support::chat();
        let _answers = open(&mut narrow, &three_questions());
        let painted = frame(&mut narrow, width, 30);
        for row in &painted {
            assert!(
                lca_tui::engine::text::visible_width(row) <= width as usize,
                "no row is wider than the terminal at {width}: {row:?}"
            );
            let body = row
                .trim()
                .trim_start_matches('│')
                .trim()
                .trim_end_matches('│')
                .trim();
            assert_ne!(
                body,
                "…",
                "the tab row fits the modal at {width} instead of wrapping:\n{}",
                painted.join("\n")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 9. No all-channel reset
// ---------------------------------------------------------------------------

// Verifies: gh #407 - the questionnaire paints no `\x1b[0m`. This tree's
// per-channel rule: a full reset on a modal row clears the frame's own colours
// for whatever the diff renderer paints after it. The active tab opens a
// background and must close exactly that channel (`49`), and the foreground
// exactly its own (`39`).
#[test]
fn the_questionnaire_paints_no_all_channel_reset() {
    let mut options = crate::chat::tests_support::options();
    options.plain = false;
    let mut chat = crate::chat::Chat::new(
        options,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    let _answers = open(&mut chat, &three_questions());
    let rows = chat.viewport(80, 24, 0);
    // Only the modal's own rows: the transcript and dock behind it are other
    // paint paths, and this row is about what the questionnaire emits.
    let top = rows
        .iter()
        .position(|row| row.contains("Setup"))
        .unwrap_or_else(|| panic!("the modal is painted:\n{}", rows.join("\n")));
    let bottom = rows[top..]
        .iter()
        .position(|row| row.contains('╰'))
        .map(|offset| top + offset)
        .unwrap_or(rows.len() - 1);
    let modal = rows[top..=bottom].join("\n");
    // The shared painter frames every modal row with `SEGMENT_RESET` on both
    // sides (gh #373 gap 3's R4: the diff renderer writes only changed spans,
    // so an unpainted cell would inherit whatever SGR the terminal last saw).
    // That is two per row, and the claim here is that the questionnaire's own
    // rows add **none**: a body that reset mid-row would punch a hole in the
    // frame's colours for the rest of that row.
    assert_eq!(
        modal.matches("\u{1b}[0m").count(),
        2 * modal.lines().count(),
        "the body adds no reset of its own:\n{modal}"
    );
    // And every background the tab row opens is closed on its own channel.
    let opened = modal.matches("\u{1b}[48;").count();
    let closed = modal.matches("\u{1b}[49m").count();
    assert_eq!(
        opened, closed,
        "each background span closes the background channel, not all of them:\n{modal}"
    );
    assert!(opened >= 1, "the active tab is painted:\n{modal}");
}
