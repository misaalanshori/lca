//! The tabbed questionnaire (gh #407): N questions, one modal, one submit.
//!
//! An extension's `ui-dialogs.questionnaire` asks several questions at once and
//! gets them all back from **one** submit, instead of costing one host picker
//! per question. This module owns the chrome (painted on the host dialog
//! frame), the key contract, and the per-tab state the contract preserves.
//! `chat_host_dialog.rs` delegates the paint and `dialogs.rs` the keys, so both
//! that file and this one stay well inside gate 11.
//!
//! # The keyboard contract
//!
//! Written out because it is the product, and pinned row by row in
//! `chat_questionnaire_tests.rs`:
//!
//! **Axis separation, exactly.** `Tab`/`Shift+Tab` and `←`/`→` **always**
//! switch tabs - with one deliberate exception: while the inline editor holds
//! the field baton, `←`/`→` move its text cursor instead. The editor is a text
//! field at that moment, and a field whose cursor cannot move is broken; `Tab`
//! still switches even then, so no tab is ever unreachable. `↑`/`↓`/`PgUp`/
//! `PgDn`/`Enter`/`Space` belong to the active tab and never change it.
//!
//! **Two-stage Escape.** From the inline editor, `Esc` returns to that tab's
//! options *without closing the modal*. From the options list - or the review -
//! `Esc` dismisses the whole ask, which the asker reports as declined. (A
//! question with no options has no list to return to, so its first `Esc` is
//! already the dismissal.)
//!
//! **Picking.** A single-choice question takes `Enter` on the highlighted row.
//! A `multiple` question toggles a row with `Space` and finishes with `Enter`;
//! with nothing picked, `Enter` finishes nothing (the tab stays pending and the
//! hint row says why), because a checkbox question answered with an empty value
//! would be indistinguishable from one nobody answered. Picks join with `", "`
//! in toggle order, and a pick that came from the editor marks the answer
//! `custom`.
//!
//! **The submit end-cap** is the last tab. It paints as a review - every
//! question with its answer, custom ones marked `(wrote)` - and `Enter` there
//! submits only when nothing is unanswered; otherwise it jumps to the first
//! unanswered tab, so the refusal is a place to go rather than a dead key.
//!
//! **Per-tab state.** Every tab keeps its own highlight, scroll offset, draft,
//! cursor, editor baton and answer, and gets them back on return. That is the
//! whole point of tabs, so it has its own row.
//!
//! # Escapes
//!
//! The tree's per-channel rule applies to every paint here: the active tab's
//! background is opened with `48;N` and closed with `49`, the foreground with
//! `39`. No `\x1b[0m` is emitted in this module (the modal's own row would
//! catch it, and a full reset would drop the frame's colours for the rest of
//! the row).
//!
//! # Mouse
//!
//! Keyboard parity is the bar here: host dialogs swallow clicks wholesale
//! (`Chat::click_extension` returns `Swallowed` while one is open), so there is
//! no option hit-testing to extend. The wheel does reach the dialog and scrolls
//! the active tab, exactly as it scrolls a `select`.

use lca_protocol::{DialogAnswer, UiAnswer, UiDialog, UiQuestion};
use lca_tui::engine::text::visible_width;

use super::chat::Chat;
use crate::chat_commands::{paste_text, printable};
use crate::chat_focus::Field;
use crate::state::DialogModal;
use crate::theme::Role;

/// The inline editor's row, worded exactly as `extensions/question` words its
/// `CUSTOM_OPTION`: the tabbed and the sequential path are the same question
/// asked on different chrome, so they read alike. `tests/regressions/407-*`
/// pins the two literals to each other, because two copies that must stay
/// equal are a defect waiting for the next edit (gh #298's lesson).
pub(crate) const CUSTOM_ROW: &str = "Type your own answer...";

/// How many option rows a tab shows at once, and what `PgUp`/`PgDn` move by.
/// The paint clips to the modal when the terminal is shorter than a page, and
/// the frame's own `… n more` note then says so.
pub(crate) const PAGE: usize = 8;

/// One tab's own state: everything leaving and returning must restore.
#[derive(Debug, Default)]
pub(crate) struct Tab {
    /// The highlighted row (an index into the question's rendered rows).
    pub(crate) selected: usize,
    /// The first visible row.
    pub(crate) scroll: usize,
    /// The inline editor's draft.
    pub(crate) draft: String,
    /// The caret's byte offset in `draft` (always on a char boundary).
    pub(crate) cursor: usize,
    /// Whether the inline editor holds the field baton.
    pub(crate) editing: bool,
    /// The picks of a `multiple` question, in toggle order.
    pub(crate) picks: Vec<String>,
    /// Whether any pick came from the editor.
    pub(crate) typed: bool,
    /// The answer once given.
    pub(crate) answer: Option<UiAnswer>,
}

/// The open questionnaire's state: one entry per question plus the active tab
/// (`questions.len()` is the submit end-cap).
///
/// Public because it is a field of the public [`DialogModal`]; the fields stay
/// crate-private, so this type is read by the interface's own chrome and by
/// its rows and built only by [`Questionnaire::new`].
#[derive(Debug)]
pub struct Questionnaire {
    /// Per question, in the asker's order.
    pub(crate) tabs: Vec<Tab>,
    /// The active tab, including the end-cap.
    pub(crate) active: usize,
}

impl Questionnaire {
    /// Open on the first question. A question with no options is free text
    /// only, so its editor holds the baton from the start (the WIT record's
    /// documented default).
    pub(crate) fn new(questions: &[UiQuestion]) -> Questionnaire {
        Questionnaire {
            tabs: questions
                .iter()
                .map(|question| Tab {
                    editing: question.options.is_empty(),
                    ..Tab::default()
                })
                .collect(),
            active: 0,
        }
    }

    /// Whether the active tab's inline editor holds the field baton.
    pub(crate) fn editing(&self) -> bool {
        self.tabs.get(self.active).is_some_and(|tab| tab.editing)
    }
}

/// The rows a tab renders: its options plus the inline editor's row. A
/// question with no options has no rows - it opens in the editor and returns
/// there until it is answered.
fn row_count(question: &UiQuestion) -> usize {
    if question.options.is_empty() {
        0
    } else {
        question.options.len() + 1
    }
}

/// The window a tab paints: `(start, end)` into its rows.
fn window(tab: &Tab, rows: usize) -> (usize, usize) {
    let start = tab.scroll.min(rows.saturating_sub(PAGE));
    (start, (start + PAGE).min(rows))
}

/// Move the highlight, keeping it inside the window (`↑`/`↓`).
fn move_highlight(tab: &mut Tab, rows: usize, to: usize) {
    if rows == 0 {
        return;
    }
    tab.selected = to.min(rows - 1);
    if tab.selected < tab.scroll {
        tab.scroll = tab.selected;
    } else if tab.selected >= tab.scroll + PAGE {
        tab.scroll = tab.selected + 1 - PAGE;
    }
}

/// Keep the highlight inside the painted window after a page step.
fn keep_in_window(tab: &mut Tab, rows: usize) {
    if rows == 0 {
        return;
    }
    let end = (tab.scroll + PAGE).min(rows);
    if tab.selected < tab.scroll {
        tab.selected = tab.scroll;
    } else if tab.selected >= end {
        tab.selected = end.saturating_sub(1);
    }
}

/// Advance after an answer, pi's `advanceAfterAnswer`: the next question, or
/// the submit end-cap after the last one.
fn advance(state: &mut Questionnaire, total: usize) {
    if state.active + 1 < total {
        state.active += 1;
    } else {
        state.active = total - 1;
    }
}

/// The answers in the asker's order, for the submit.
fn collected(state: &Questionnaire) -> Vec<UiAnswer> {
    state
        .tabs
        .iter()
        .filter_map(|tab| tab.answer.clone())
        .collect()
}

/// Which questions have no answer yet.
fn unanswered(questions: &[UiQuestion], state: &Questionnaire) -> Vec<usize> {
    (0..questions.len())
        .filter(|index| state.tabs[*index].answer.is_none())
        .collect()
}

/// Both halves of the open modal, disjointly borrowed: the questions the asker
/// sent, and the state this module owns.
fn split(modal: &mut DialogModal) -> Option<(&[UiQuestion], &mut Questionnaire)> {
    let UiDialog::Questionnaire { questions, .. } = &modal.exchange.dialog else {
        return None;
    };
    let state = modal.questionnaire.as_mut()?;
    Some((questions.as_slice(), state))
}

/// Handle one key while a questionnaire is open.
///
/// `None` means the modal stays open; `Some(answer)` closes it - the caller
/// sends the answer and drops the modal, exactly as the other dialogs do.
pub(crate) fn handle(
    modal: &mut DialogModal,
    data: &str,
    key: Option<&str>,
) -> Option<DialogAnswer> {
    let Some((questions, state)) = split(modal) else {
        // No questions or no state: nothing to ask, so dismiss rather than
        // leave a modal nobody can close.
        return Some(DialogAnswer::Questionnaire(None));
    };
    if questions.is_empty() || state.tabs.len() != questions.len() {
        return Some(DialogAnswer::Questionnaire(None));
    }
    let total = questions.len() + 1;
    let on_review = state.active == questions.len();

    // The axis rule, first: tab switching is unconditional except that the
    // editor's arrows move its cursor (they are the field's, then).
    let typing = !on_review && state.tabs[state.active].editing;
    match key {
        Some("tab") => {
            state.active = (state.active + 1) % total;
            return None;
        }
        Some("shift+tab") => {
            state.active = (state.active + total - 1) % total;
            return None;
        }
        Some("right") if !typing => {
            state.active = (state.active + 1) % total;
            return None;
        }
        Some("left") if !typing => {
            state.active = (state.active + total - 1) % total;
            return None;
        }
        _ => {}
    }

    // The submit end-cap: a review, and the one place that submits.
    if on_review {
        if key == Some("escape") {
            return Some(DialogAnswer::Questionnaire(None));
        }
        let missing = unanswered(questions, state);
        if key == Some("enter") {
            if missing.is_empty() {
                return Some(DialogAnswer::Questionnaire(Some(collected(state))));
            }
            // The refusal is navigable: go to the first question still open.
            state.active = missing[0];
        }
        return None;
    }

    let question = &questions[state.active];
    if state.tabs[state.active].editing {
        return edit_key(state, question, data, key);
    }
    pick_key(state, question, key)
}

/// The inline editor's keys (the field baton belongs to it).
fn edit_key(
    state: &mut Questionnaire,
    question: &UiQuestion,
    data: &str,
    key: Option<&str>,
) -> Option<DialogAnswer> {
    let tab = &mut state.tabs[state.active];
    if let Some(text) = paste_text(data) {
        tab.draft.insert_str(tab.cursor, &text);
        tab.cursor += text.len();
        return None;
    }
    match key {
        Some("escape") => {
            tab.editing = false;
            tab.draft.clear();
            tab.cursor = 0;
            // An optionless question has no list to return to, so its first
            // Esc is the dismissal (the two-stage escape has one stage).
            if question.options.is_empty() {
                return Some(DialogAnswer::Questionnaire(None));
            }
        }
        Some("enter") => {
            let value = tab.draft.trim().to_string();
            // An empty answer answers nothing: Enter with a blank field leaves
            // the tab as it was, rather than sending a value nobody typed.
            if value.is_empty() {
                return None;
            }
            if question.multiple {
                // A typed pick joins the checkbox list, and the list stays open
                // so Enter can finish - the sequential path's accumulate shape.
                tab.picks.push(value);
                tab.typed = true;
                tab.editing = false;
                tab.draft.clear();
                tab.cursor = 0;
            } else {
                tab.answer = Some(UiAnswer {
                    header: question.header.clone(),
                    value,
                    custom: true,
                });
                tab.editing = false;
                let total = state.tabs.len() + 1;
                advance(state, total);
            }
        }
        Some("left") => tab.cursor = step_back(&tab.draft, tab.cursor),
        Some("right") => tab.cursor = step_forward(&tab.draft, tab.cursor),
        Some("home") => tab.cursor = 0,
        Some("end") => tab.cursor = tab.draft.len(),
        Some("backspace") => {
            let from = step_back(&tab.draft, tab.cursor);
            if from < tab.cursor {
                tab.draft.replace_range(from..tab.cursor, "");
                tab.cursor = from;
            }
        }
        Some("delete") => {
            let to = step_forward(&tab.draft, tab.cursor);
            if to > tab.cursor {
                tab.draft.replace_range(tab.cursor..to, "");
            }
        }
        _ => {
            if let Some(text) = printable(data) {
                tab.draft.insert_str(tab.cursor, &text);
                tab.cursor += text.len();
            }
        }
    }
    None
}

/// The options list's keys.
fn pick_key(
    state: &mut Questionnaire,
    question: &UiQuestion,
    key: Option<&str>,
) -> Option<DialogAnswer> {
    let total = state.tabs.len() + 1;
    let rows = row_count(question);
    // The inline editor's row is the last one, and only exists beside a list.
    let custom_row = (!question.options.is_empty()).then_some(question.options.len());
    let tab = &mut state.tabs[state.active];
    match key {
        Some("escape") => return Some(DialogAnswer::Questionnaire(None)),
        Some("up") => move_highlight(tab, rows, tab.selected.saturating_sub(1)),
        Some("down") => move_highlight(tab, rows, tab.selected + 1),
        Some("pageup") => {
            tab.scroll = tab.scroll.saturating_sub(PAGE);
            tab.selected = tab.selected.saturating_sub(PAGE);
            keep_in_window(tab, rows);
        }
        Some("pagedown") => {
            tab.scroll = (tab.scroll + PAGE).min(rows.saturating_sub(PAGE.min(rows)));
            tab.selected = (tab.selected + PAGE).min(rows.saturating_sub(1));
            keep_in_window(tab, rows);
        }
        Some("space") if question.multiple => {
            let option = question.options.get(tab.selected)?;
            toggle(&mut tab.picks, option);
        }
        Some("enter") => {
            if custom_row == Some(tab.selected) {
                tab.editing = true;
                tab.draft.clear();
                tab.cursor = 0;
                return None;
            }
            if question.multiple {
                // Nothing picked, nothing finished: the hint row explains.
                if tab.picks.is_empty() {
                    return None;
                }
                tab.answer = Some(UiAnswer {
                    header: question.header.clone(),
                    value: tab.picks.join(", "),
                    custom: tab.typed,
                });
                advance(state, total);
                return None;
            }
            let option = question.options.get(tab.selected)?;
            tab.answer = Some(UiAnswer {
                header: question.header.clone(),
                value: option.clone(),
                custom: false,
            });
            advance(state, total);
        }
        _ => {}
    }
    None
}

/// Add or remove an option from a multi-pick list, keeping toggle order.
fn toggle(picks: &mut Vec<String>, option: &str) {
    match picks.iter().position(|pick| pick == option) {
        Some(at) => {
            picks.remove(at);
        }
        None => picks.push(option.to_string()),
    }
}

/// One grapheme-ish step left: the previous char boundary, so a multi-byte
/// character is never split (no panic, and the caret stays on a boundary).
fn step_back(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .chars()
        .next_back()
        .map_or(0, |ch| cursor - ch.len_utf8())
}

/// One step right.
fn step_forward(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |ch| cursor + ch.len_utf8())
}

/// The first char boundary at or before `cursor` (a caret can only sit there).
fn boundary(text: &str, cursor: usize) -> usize {
    let mut at = cursor.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

impl Chat {
    /// Draw an open questionnaire (gh #407) on the host dialog frame.
    pub(super) fn compose_questionnaire(
        &self,
        viewport: &mut [String],
        width: u16,
        height: u16,
        modal: &DialogModal,
    ) {
        let UiDialog::Questionnaire { title, questions } = &modal.exchange.dialog else {
            return;
        };
        let Some(state) = modal.questionnaire.as_ref() else {
            return;
        };
        if questions.is_empty() || state.tabs.len() != questions.len() {
            return;
        }
        // The dialog box resolves the same layout as a picker's
        // (`Percent(80)`, `margin: 2`), so `picker_inner_width` is the one
        // arithmetic for the cells a row has to paint in.
        let inner = crate::render::picker_inner_width(width) as usize;
        let mut body = vec![self.tab_row(questions, state, inner)];
        body.push(String::new());
        let on_review = state.active == questions.len();
        if on_review {
            body.extend(self.review_rows(questions, state));
        } else {
            let question = &questions[state.active];
            let tab = &state.tabs[state.active];
            body.push(self.theme.role(Role::Text)(&question.prompt));
            body.push(String::new());
            if tab.editing {
                body.extend(self.editing_rows(question, state));
            } else {
                body.extend(self.option_rows(question, tab));
            }
        }
        body.push(String::new());
        body.push(self.theme.role(Role::Dim)(
            &self.questionnaire_hint(questions, state),
        ));
        super::render::overlay_box(viewport, width, height, title, &body, &self.theme);
    }

    /// The tab row: `← ■ answered □ pending ✓ Submit →`, the active tab on the
    /// selected background, clipped to the modal with an ellipsis when the
    /// headers do not fit (a twelve-question ask must not overflow).
    fn tab_row(&self, questions: &[UiQuestion], state: &Questionnaire, inner: usize) -> String {
        let accent = self.theme.role(Role::Accent);
        let text = self.theme.role(Role::Text);
        let success = self.theme.role(Role::Success);
        let muted = self.theme.role(Role::Muted);
        let dim = self.theme.role(Role::Dim);
        let selected_bg = self.theme.bg(Role::SelectedBg);
        let mut segments: Vec<(usize, String)> = vec![(2, accent("← "))];
        for (index, question) in questions.iter().enumerate() {
            let answered = state.tabs[index].answer.is_some();
            let marker = if answered { '■' } else { '□' };
            let label = format!(" {marker} {} ", header_label(question, index));
            let painted = if index == state.active {
                selected_bg(&text(&label))
            } else if answered {
                success(&label)
            } else {
                muted(&label)
            };
            segments.push((visible_width(&label), painted));
        }
        let can_submit = state.tabs.iter().all(|tab| tab.answer.is_some());
        let submit = " ✓ Submit ".to_string();
        let painted_submit = if state.active == questions.len() {
            selected_bg(&text(&submit))
        } else if can_submit {
            success(&submit)
        } else {
            dim(&submit)
        };
        segments.push((visible_width(&submit), painted_submit));
        segments.push((2, accent(" →")));
        clip_segments(&segments, inner)
    }

    /// The questions' option rows, windowed around the stored offset.
    fn option_rows(&self, question: &UiQuestion, tab: &Tab) -> Vec<String> {
        let accent = self.theme.role(Role::Accent);
        let text = self.theme.role(Role::Text);
        let muted = self.theme.role(Role::Muted);
        let rows = row_count(question);
        let (start, end) = window(tab, rows);
        let mut out = Vec::new();
        for index in start..end {
            let selected = index == tab.selected;
            let marker = if selected {
                accent("> ")
            } else {
                "  ".to_string()
            };
            let custom_row = question.options.len();
            if index == custom_row {
                let label = format!("{}. {CUSTOM_ROW}", index + 1);
                out.push(format!("{marker}{}", text(&label)));
                continue;
            }
            let Some(option) = question.options.get(index) else {
                continue;
            };
            let label = if question.multiple {
                let picked = tab.picks.iter().any(|pick| pick == option);
                format!(
                    "{}. [{}] {option}",
                    index + 1,
                    if picked { 'x' } else { ' ' }
                )
            } else {
                format!("{}. {option}", index + 1)
            };
            let painted = if selected {
                accent(&label)
            } else {
                text(&label)
            };
            out.push(format!("{marker}{painted}"));
        }
        if start > 0 || end < rows {
            out.push(muted(&format!("   … rows {}–{end} of {rows}", start + 1)));
        }
        out
    }

    /// The editor frame: the prompt for context, the options for reference,
    /// then the field with its caret and the `✎` marker (pi's shape).
    fn editing_rows(&self, question: &UiQuestion, state: &Questionnaire) -> Vec<String> {
        let accent = self.theme.role(Role::Accent);
        let muted = self.theme.role(Role::Muted);
        let dim = self.theme.role(Role::Dim);
        let tab = &state.tabs[state.active];
        let mut out = Vec::new();
        if !question.options.is_empty() {
            out.extend(self.option_rows(question, tab));
            out.push(String::new());
        }
        out.push(muted("Your answer:"));
        let row = self.caret_row_at(
            &accent("✎ "),
            &tab.draft,
            boundary(&tab.draft, tab.cursor),
            "type your answer",
            Field::DialogWrite,
        );
        out.push(row);
        out.push(dim("Enter submits; Esc goes back."));
        out
    }

    /// The submit end-cap's review: every answer, the custom ones marked
    /// `(wrote)`, and either the invitation or the `Unanswered:` line.
    fn review_rows(&self, questions: &[UiQuestion], state: &Questionnaire) -> Vec<String> {
        let accent = self.theme.role(Role::Accent);
        let muted = self.theme.role(Role::Muted);
        let text = self.theme.role(Role::Text);
        let success = self.theme.role(Role::Success);
        let warning = self.theme.role(Role::Warning);
        let mut out = vec![self.theme.role_bold(Role::Accent)("Ready to submit").to_string()];
        out.push(String::new());
        for (index, question) in questions.iter().enumerate() {
            let label = header_label(question, index);
            match &state.tabs[index].answer {
                Some(answer) => {
                    let wrote = if answer.custom { "(wrote) " } else { "" };
                    out.push(format!(
                        "{}{}",
                        muted(&format!("{label}: ")),
                        text(&format!("{wrote}{}", answer.value))
                    ));
                }
                None => out.push(muted(&format!("{label}: (unanswered)"))),
            }
        }
        out.push(String::new());
        let missing = unanswered(questions, state);
        if missing.is_empty() {
            out.push(success("Press Enter to submit."));
        } else {
            let names: Vec<String> = missing
                .iter()
                .map(|index| header_label(&questions[*index], *index))
                .collect();
            out.push(warning(&format!("Unanswered: {}", names.join(", "))));
            out.push(accent("Enter jumps to the first one."));
        }
        out
    }

    /// The one hint row under the active frame.
    fn questionnaire_hint(&self, questions: &[UiQuestion], state: &Questionnaire) -> String {
        if state.active == questions.len() {
            return "Esc dismisses · Tab/←→ moves between questions".to_string();
        }
        let question = &questions[state.active];
        let some_verb = if question.multiple {
            "Space picks · Enter done"
        } else {
            "↑↓ select · Enter confirms"
        };
        format!("{some_verb} · Tab/←→ questions · Esc cancels")
    }
}

/// A stable per-question label: the header, or `Q<n>` when the asker sent none
/// (pi defaults the same way). Tabs are labelled with the same string the
/// review uses, so a question is never named two things.
fn header_label(question: &UiQuestion, index: usize) -> String {
    let header = question.header.trim();
    if header.is_empty() {
        format!("Q{}", index + 1)
    } else {
        header.to_string()
    }
}

/// Clip styled segments to `inner` cells, ending in an ellipsis when they had
/// to be cut.
///
/// Deliberately not `truncate_to_width`: that helper finalises with `\x1b[0m`,
/// which clears every channel (the tab row's own background included). Every
/// segment here closes its own channels, so a plain cut plus `…` is complete.
fn clip_segments(segments: &[(usize, String)], inner: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for (width, painted) in segments {
        if used + *width > inner {
            // The ellipsis needs its own cell, so the last segment is dropped
            // rather than half-painted.
            if inner > 0 {
                out.push('…');
            }
            return out;
        }
        used += *width;
        out.push_str(painted);
    }
    out
}

#[cfg(test)]
#[path = "chat_questionnaire_tests.rs"]
mod tests;
