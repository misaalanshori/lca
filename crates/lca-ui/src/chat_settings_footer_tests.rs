//! The `/settings` footer (gh #386 gap 3): the highlighted row's description, whole.
//!
//! Gap 1 gave every settings row a description *column*, so the sentence on a row is
//! truncated to fit it. The footer is where the rest of it goes. Three properties
//! carry the requirement, and each has a row here:
//!
//! 1. it **shows the selected row's description** and follows the selection;
//! 2. it **wraps** rather than truncating (a footer that cut the sentence would just
//!    be the row's column with more air around it);
//! 3. it **does not push rows off the pane** - the visible *item* count is unchanged
//!    with the footer present. That is the constraint the brief is emphatic about,
//!    and it is a row-count assertion, not a pixel one.
//!
//! Plus gap 2's lesson applied to a new element: the footer is a **hover-free
//! surface**. A mouse move changes nothing, because a move no longer touches
//! `selected` and the footer is derived from it.

use super::*;
use super::{chat, strip};

fn row(section: &str, key: &str, label: &str, description: &str) -> crate::state::SettingRow {
    crate::state::SettingRow {
        section: section.to_string(),
        key: key.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        value: "v".to_string(),
        source: "default".to_string(),
        masked: false,
        values: Vec::new(),
        project: None,
    }
}

fn chat_with(rows: Vec<crate::state::SettingRow>) -> Chat {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
        rows,
        selected: 0,
        editing: None,
        query: String::new(),
        remembered: std::collections::BTreeMap::new(),
    });
    chat
}

fn paint(chat: &Chat, width: u16, height: u16) -> String {
    strip(&chat.viewport(width, height, 0)).join("\n")
}

// Verifies: gh #386 gap 3 - the footer shows the **selected** row's description. This
// is the piece of pi's `SettingsList` LCA lacked (`settings-list.ts::renderMainList`
// pushes the selected item's description below the list).
#[test]
fn the_footer_shows_the_selected_rows_description() {
    let chat = chat_with(vec![
        row("S", "a", "Aye", "The first row's whole sentence."),
        row("S", "b", "Bee", "The second row's whole sentence."),
    ]);
    let text = paint(&chat, 100, 30);
    assert!(
        text.contains("The first row's whole sentence."),
        "the highlighted row's description is in the footer:\n{text}"
    );
    assert!(
        !text.contains("The second row's whole sentence."),
        "and only the highlighted row's:\n{text}"
    );
}

// Verifies: gh #386 gap 3 - the footer **follows the selection**, because it is
// derived from it. A second piece of state could fall out of step; this row would
// catch that.
#[test]
fn the_footer_follows_the_selection() {
    let mut chat = chat_with(vec![
        row("S", "a", "Aye", "Alpha description."),
        row("S", "b", "Bee", "Beta description."),
    ]);
    let first = paint(&chat, 100, 30);
    assert!(first.contains("Alpha description."), "{first}");
    assert!(!first.contains("Beta description."), "{first}");

    let _ = chat.handle_picker_key("", Some("down"));

    let second = paint(&chat, 100, 30);
    assert!(
        second.contains("Beta description."),
        "the footer moved with the cursor:\n{second}"
    );
    assert!(
        !second.contains("Alpha description."),
        "and dropped the old row's:\n{second}"
    );
}

// Verifies: gh #386 gap 3 - the footer **wraps**. The description here is far wider
// than the box, so a truncating footer would lose its tail. Asserted on a phrase from
// the *end* of the sentence, which only a wrapping footer can reach.
#[test]
fn the_footer_wraps_instead_of_truncating() {
    let long = "Terminal scrollback or app-owned fullscreen, and here is a tail that \
                would be lost to any truncation at all because it is the last clause.";
    let chat = chat_with(vec![row("S", "a", "Aye", long)]);
    let text = paint(&chat, 80, 30);
    // The phrase below starts past 80 cells into the description, so it can only be
    // on screen if the footer wrapped onto a second row.
    assert!(
        text.contains("because it is the last clause."),
        "the sentence's tail is on screen, so it wrapped:\n{text}"
    );
}

// Verifies: gh #386 gap 3 - **the footer does not push rows off the pane.** The
// number of visible list rows is the same with and without a description, which is a
// row-count assertion: the footer takes room from the box's spare height, and when
// there is none it must not take an item's.
//
// This is the constraint the brief calls out, and it is the reason `pinned_tail` is
// subtracted from the *item budget* rather than the item list being drawn first and
// painted into whatever is left.
#[test]
fn the_footer_does_not_push_list_rows_off_the_pane() {
    let without: Vec<crate::state::SettingRow> = (0..40)
        .map(|index| row("S", &format!("k{index:02}"), &format!("Key {index:02}"), ""))
        .collect();
    let with: Vec<crate::state::SettingRow> = (0..40)
        .map(|index| {
            row(
                "S",
                &format!("k{index:02}"),
                &format!("Key {index:02}"),
                "A description that is long enough to wrap onto a second footer row.",
            )
        })
        .collect();

    let count = |rows: Vec<crate::state::SettingRow>| -> usize {
        let chat = chat_with(rows);
        let text = paint(&chat, 100, 30);
        text.lines().filter(|line| line.contains("Key ")).count()
    };

    let bare = count(without);
    let footed = count(with);
    assert!(bare > 1, "the bare list shows several rows ({bare})");
    assert_eq!(
        bare, footed,
        "the footer took no item's room ({bare} rows with it, {footed} without)"
    );
}

// Verifies: gh #386 gap 3 and gap 2's lesson on a new element - the footer is a
// **hover-free surface**. A mouse move over a row changes nothing on screen,
// including the description below the list.
#[test]
fn a_mouse_move_does_not_change_the_footer() {
    use crate::chat_mouse::PickerHit;
    let mut chat = chat_with(vec![
        row("S", "a", "Aye", "Alpha description."),
        row("S", "b", "Bee", "Beta description."),
    ]);
    let (w, h) = (100u16, 30u16);
    let before = paint(&chat, w, h);
    let (rect, map) = chat.picker_layout(w, h).expect("the box maps");
    // Every item row, hovered in turn.
    for (offset, entry) in map.iter().enumerate() {
        if entry.is_none() {
            continue;
        }
        if let Some(PickerHit::Item(item)) =
            chat.picker_hit(rect.col + 5, rect.row + 1 + offset as u16, w, h)
        {
            chat.picker_hover(item);
        }
    }
    let after = paint(&chat, w, h);
    assert_eq!(before, after, "a hover changed the frame, footer included");
}

// Verifies: gh #386 gap 3 - a row with **no** description adds no footer rows. A blank
// separator with nothing after it would be two rows of air where the list could be.
#[test]
fn a_row_without_a_description_has_no_footer() {
    assert!(
        crate::chat_settings_footer::settings_footer(
            Some(&row("S", "a", "Aye", "")),
            crate::chat_settings_footer::settings_footer_width(80)
        )
        .is_empty(),
        "no description, no footer rows"
    );
    assert!(
        crate::chat_settings_footer::settings_footer(None, 80).is_empty(),
        "no row, no footer rows"
    );
}

// Verifies: gh #386 gap 3 - a description longer than the cap **says it was cut** with
// an ellipsis rather than ending mid-word. The cap exists so a footer cannot become
// the screen (twenty wrapped rows would leave one item visible, which is the
// push-the-list-off failure in its other form).
#[test]
fn an_over_long_footer_is_capped_and_says_so() {
    use crate::chat_settings_footer::{
        SETTINGS_FOOTER_MAX_ROWS, settings_footer, settings_footer_width,
    };
    let very_long = "word ".repeat(400);
    let width = settings_footer_width(60);
    let rows = settings_footer(Some(&row("S", "a", "Aye", &very_long)), width);
    // The separator plus the cap.
    assert_eq!(
        rows.len(),
        SETTINGS_FOOTER_MAX_ROWS + 1,
        "the footer is capped at {SETTINGS_FOOTER_MAX_ROWS} rows plus its separator"
    );
    assert!(
        rows.last()
            .is_some_and(|line| line.trim_end().ends_with('\u{2026}')),
        "the last row is elided, not cut mid-word: {:?}",
        rows.last()
    );
}

// Verifies: gh #386 gap 3 - a row being **edited** shows no footer: its buffer is on
// the row itself, and a description underneath would compete with the field the user
// is typing into.
#[test]
fn an_editing_row_has_no_footer() {
    let mut chat = chat_with(vec![row("S", "a", "Aye", "Alpha description.")]);
    let before = paint(&chat, 100, 30);
    assert!(before.contains("Alpha description."), "{before}");
    let _ = chat.handle_picker_key("", Some("enter"));
    assert!(
        chat.settings_picker
            .as_ref()
            .is_some_and(|picker| picker.editing.is_some()),
        "the row is editing"
    );
    let after = paint(&chat, 100, 30);
    assert!(
        !after.contains("Alpha description."),
        "the footer is gone while the buffer owns the row:\n{after}"
    );
}

// Verifies: gh #386 gap 3 - on a pane too short for both, the footer **yields**, and
// the list keeps its rows. This is the case the tall-pane row cannot see: there the
// box had spare height, so the footer cost nothing either way. Here every row of the
// box is already spoken for, so a footer that appeared anyway would come out of the
// items - the "pushes the list off the pane" failure in the form it actually takes.
#[test]
fn a_short_pane_gives_up_the_footer_not_a_list_row() {
    let rows: Vec<crate::state::SettingRow> = (0..40)
        .map(|index| {
            row(
                "S",
                &format!("k{index:02}"),
                &format!("Key {index:02}"),
                "A description long enough to want two footer rows of its own.",
            )
        })
        .collect();
    let chat = chat_with(rows);
    // 14 rows tall: the granted box is at most 10, so the list fills it and there is
    // nothing spare.
    let text = paint(&chat, 100, 14);
    let items = text.lines().filter(|line| line.contains("Key ")).count();
    assert!(
        items > 1,
        "the list still paints its rows ({items}):\n{text}"
    );
    assert!(
        !text.contains("A description long enough"),
        "the footer yielded to the list:\n{text}"
    );
}

// Verifies: gh #386 gap 3 - the footer sits **above** the pinned hint and paints
// exactly **once**. Both halves are one fact: the hint is the box's last line (pi's
// `addHintLine` runs after the description in `renderMainList`), and the footer is
// inserted in one place. A body that carried a copy *and* a painter that inserted one
// painted the description twice and pushed the hint off the box - which is how this
// row's first version found the double insert.
#[test]
fn the_footer_sits_above_the_pinned_hint() {
    let chat = chat_with(vec![row("S", "a", "Aye", "Alpha description here.")]);
    let text = paint(&chat, 100, 30);
    let lines: Vec<&str> = text.lines().collect();
    let footers: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == "Alpha description here.")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        footers.len(),
        1,
        "the footer paints exactly once (at {footers:?}):\n{text}"
    );
    let hint = lines
        .iter()
        .position(|line| line.contains("type to search"))
        .unwrap_or_else(|| panic!("the hint paints:\n{text}"));
    assert!(
        footers[0] < hint,
        "the footer is above the hint (footer {}, hint {hint}):\n{text}",
        footers[0]
    );
}

// Verifies: gh #386 gap 3 with gh #235 - on a pane too short for a footer, the help
// does not **vanish**: it goes back onto the hint line in its old truncated form.
//
// This is the row the first version of the footer was missing, and a live row
// (`e2e_polish::settings_selector_shows_labels_and_help`) found it. "The footer yields
// to the list" and "the highlighted row's help is on screen" are both requirements, so
// yielding has to mean *demoting*, not dropping.
#[test]
fn a_short_pane_demotes_the_description_to_the_hint_line() {
    let rows: Vec<crate::state::SettingRow> = (0..40)
        .map(|index| {
            row(
                "S",
                &format!("k{index:02}"),
                &format!("Key {index:02}"),
                "A description long enough to want two footer rows of its own.",
            )
        })
        .collect();
    let chat = chat_with(rows);
    let text = paint(&chat, 100, 14);
    assert!(text.contains("Key 00"), "the list paints its rows:\n{text}");
    assert!(
        text.contains("A description long"),
        "the help is still on screen, demoted to the hint line (truncated there):\n{text}"
    );
}

// Verifies: gh #386 gap 3 - the footer and the hint are **one decision**, so the
// description can never appear in both places at once. A double appearance is not
// hypothetical: it is what the first version of the splice produced.
#[test]
fn the_description_appears_once_whichever_form_it_takes() {
    for (width, height) in [(100u16, 30u16), (100, 14), (60, 24), (200, 50)] {
        let rows: Vec<crate::state::SettingRow> = (0..40)
            .map(|index| {
                row(
                    "S",
                    &format!("k{index:02}"),
                    &format!("Key {index:02}"),
                    "Distinctive marker sentence.",
                )
            })
            .collect();
        let chat = chat_with(rows);
        let text = paint(&chat, width, height);
        let seen = text.matches("Distinctive marker").count();
        assert_eq!(
            seen, 1,
            "the description appears exactly once at {width}x{height} (saw {seen}):\n{text}"
        );
    }
}
