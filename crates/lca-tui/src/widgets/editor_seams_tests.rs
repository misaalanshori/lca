//! gh #381 gap 5: the three seams where a chip's text **leaves and re-enters** the
//! editor.
//!
//! Gaps 1-4 established the one property for motion, deletion, undo and paint: a
//! chip's text is a **label** and the registry is the **truth**, and every path either
//! preserves both or is explicit about which one it transforms. These are the three
//! places where text crosses the editor's boundary:
//!
//! 1. **a bracketed paste** coming in — a `\r` inside it is content, never a submit;
//! 2. **history recall** coming back in — the buffer must regain the chip form;
//! 3. **the `$EDITOR` handoff** going out and coming back.
//!
//! The asymmetry in (3) is the design and it is deliberate: an external editor cannot
//! know about our registry, so the handoff **expands** (that is the only honest thing
//! to show it), but the returned buffer **re-chips** through the same
//! `retain_live_payloads` rule gap 3 gave `set_text` — so a return that lost the
//! markers drops their payloads rather than shipping stale bytes.
//!
//! Split into its own module so the 1,200-line ceiling is not the reason a rule goes
//! untested.

use super::*;

/// A chip for `lines` lines of `PAYLOAD-nn`, inserted after `prefix`.
fn with_chip(prefix: &str, lines: usize) -> Editor {
    let mut e = Editor::new();
    e.insert_str(prefix);
    let big = (1..=lines)
        .map(|n| format!("PAYLOAD-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    e.insert_paste(&big);
    e
}

// ---------------------------------------------------------------------------
// seam 1: a bracketed paste is one unit, and a CR inside it is content
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 5 seam 1 - **a bracketed paste containing `\r\n` does not
// submit**, and its content arrives intact.
//
// This is the row the seam exists for. A `\r` is `Enter` in every terminal dialect, so
// a paste that is not treated as one unit submits at its first line break and the rest
// lands in a *new* prompt. The engine already hands the editor one
// `StdinEvent::Paste` (`engine/stdin_buffer.rs`), and the editor's first action is the
// paste branch - so the assertion is that no `Submitted` event is ever produced.
#[test]
fn a_bracketed_paste_with_crlf_never_submits() {
    let mut e = Editor::new();
    let event = e.handle_key("\x1b[200~a\r\nb\r\nc\x1b[201~");
    assert!(
        matches!(event, EditorEvent::Changed),
        "a paste is an edit, never a submit: {event:?}"
    );
    assert_eq!(
        e.text(),
        "a\nb\nc",
        "and the whole payload is present, with CRLF normalized to one line break"
    );
    // The buffer still holds the text - a submit would have cleared it.
    assert!(
        !e.text().is_empty(),
        "a submitted prompt would leave an empty buffer"
    );
}

// Verifies: gh #381 gap 5 seam 1 (control) - **a typed Enter still submits.**
//
// The other half: a fix that stopped a paste from submitting by weakening the submit
// path would pass the row above and break the product.
#[test]
fn a_typed_enter_still_submits() {
    let mut e = Editor::new();
    e.set_enter_inserts_newline(false);
    e.insert_str("hello");
    let event = e.handle_key("\r");
    assert!(
        matches!(event, EditorEvent::Submitted(_)),
        "a typed Enter submits: {event:?}"
    );
    assert_eq!(e.text(), "", "and the buffer is cleared");
}

// Verifies: gh #381 gap 5 seam 1 - **a CR-only paste is also content**, not a submit.
//
// ConPTY and some terminals send a bare `\r` as the line separator inside a paste, so
// the same guarantee has to hold for the CRLF-less form.
#[test]
fn a_paste_with_lone_carriage_returns_does_not_submit() {
    let mut e = Editor::new();
    let event = e.handle_key("\x1b[200~one\rtwo\rthree\x1b[201~");
    assert!(
        matches!(event, EditorEvent::Changed),
        "a CR-only paste is an edit too: {event:?}"
    );
    assert_eq!(e.text(), "one\ntwo\nthree", "every CR became one break");
}

// Verifies: gh #381 gap 5 seam 1 - **the paste branch runs before key handling**, so
// the paste cannot be reinterpreted as a sequence of keys.
//
// This pins the *mechanism*, which is what makes the guard above hold for any key the
// payload happens to contain: `handle_key` asks `bracketed_paste_content` first.
#[test]
fn the_paste_branch_precedes_key_dispatch() {
    let mut e = Editor::new();
    // `\r` alone submits, but inside a paste it must not.
    let pasted = e.handle_key("\x1b[200~\r\x1b[201~");
    assert!(
        matches!(pasted, EditorEvent::Changed),
        "a paste containing only CR is still a paste: {pasted:?}"
    );
    assert_eq!(e.text(), "\n", "the CR is content");
}

// ---------------------------------------------------------------------------
// seam 2: history recall keeps the chip form
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 5 seam 2 - **recalling a prompt that contained a chip keeps
// the chip**, instead of exploding the editor to the payload's full height.
//
// The defect, seen live: submit `look: [paste #1 +25 lines]`, press Up, and the
// composer fills with 25 lines of payload. A chip exists so a large paste stays
// readable; recall undoing that is the same harm the chip was introduced to prevent.
#[test]
fn recalling_a_prompt_keeps_its_chip() {
    let mut e = with_chip("look: ", 24);
    let chip_text = e.text();
    assert!(chip_text.contains("[paste #1 "), "a chip was formed");
    e.submit();
    assert_eq!(e.text(), "", "submitted");

    e.cursor_up();
    assert_eq!(
        e.text(),
        chip_text,
        "recall restores the chip form, not the expanded payload"
    );
    assert!(
        e.text().lines().count() == 1,
        "one line, as the chip was: {:?}",
        e.text()
    );
}

// Verifies: gh #381 gap 5 seam 2 - **a recalled prompt re-submits to the same bytes.**
//
// The same assertion gaps 3 and 4 used, one seam further out: "the marker came back"
// is not enough, because the marker is a label. The registry has to come back with it.
#[test]
fn a_recalled_prompt_expands_to_the_original_bytes() {
    let payload = (1..=24)
        .map(|n| format!("PAYLOAD-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut e = with_chip("look: ", 24);
    let first = e.submit();
    assert_eq!(
        first,
        format!("look: {payload}"),
        "the model receives the payload on the first submit"
    );

    e.cursor_up();
    // **The chip form must be back**, or the byte comparison below is vacuous: a
    // recalled *expanded* prompt resubmits identically too, and that is exactly the
    // broken behaviour. My first version of this row asserted only the bytes and
    // passed against the defect - the same "proof that proves nothing" the previous
    // cycle hit, so the shape is asserted first here.
    assert!(
        e.text().contains("[paste #1 "),
        "recall restored the chip form: {:?}",
        e.text()
    );
    assert!(
        !e.text().contains("PAYLOAD-24"),
        "and not the expanded payload: {:?}",
        &e.text()[..e.text().len().min(80)]
    );
    let second = e.submit();
    assert_eq!(
        second, first,
        "and a recalled prompt expands to the identical bytes - a stale or missing \
         registry would ship something else entirely"
    );
}

// Verifies: gh #381 gap 5 seam 2 - **a recalled chip is still a chip for editing.**
//
// Restoring the text without the registry would pass a "the marker is back" row and
// still be broken: one `Backspace` on it would orphan the marker into text that no
// longer matches and ships literally to the model (gap 2's harm).
#[test]
fn a_recalled_chip_is_still_one_unit_to_edit() {
    let mut e = with_chip("look: ", 24);
    e.submit();
    e.cursor_up();
    // The chip ends the buffer, so one Backspace from the end removes all of it.
    e.backspace();
    assert_eq!(
        e.text(),
        "look: ",
        "Backspace removed the whole chip, not one character of its marker"
    );
}

// Verifies: gh #381 gap 5 seam 2 - **plain history still works.**
//
// The control: a fix that stored every prompt as an opaque chip (or stopped pushing to
// history) would satisfy the rows above and break ordinary recall.
#[test]
fn plain_history_still_round_trips() {
    let mut e = Editor::new();
    e.insert_str("first");
    e.submit();
    e.insert_str("second");
    e.submit();
    e.cursor_up();
    assert_eq!(e.text(), "second");
    e.cursor_up();
    assert_eq!(e.text(), "first");
    e.cursor_down();
    assert_eq!(e.text(), "second");
    let again = e.submit();
    assert_eq!(
        again, "second",
        "a recalled plain prompt resubmits verbatim"
    );
}

// Verifies: gh #381 gap 5 seam 2 - **a recalled prompt that mixes text and a chip
// keeps both**, so the chip is restored in its own position.
#[test]
fn a_mixed_prompt_recalls_with_its_chip_in_place() {
    let mut e = with_chip("look: ", 24);
    e.insert_str(" and tail");
    let before = e.text();
    e.submit();
    e.cursor_up();
    assert_eq!(e.text(), before, "text on both sides of the chip survives");
    assert!(
        e.text().ends_with(" and tail"),
        "the suffix is after the chip, not folded into it: {:?}",
        e.text()
    );
}

// ---------------------------------------------------------------------------
// seam 3: the $EDITOR handoff expands out and re-chips back
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 5 seam 3 - **the handoff's outbound text is expanded.**
//
// An external editor cannot know about our registry, so the only honest thing to give
// it is the real content. `expanded_text` is the accessor the handoff uses; this pins
// that it never carries a marker.
#[test]
fn the_editor_handoff_text_carries_no_marker() {
    let e = with_chip("look: ", 24);
    let out = e.expanded_text();
    assert!(
        !out.contains("[paste #"),
        "the external editor sees content, never a label: {}",
        &out[..out.len().min(80)]
    );
    assert!(
        out.starts_with("look: PAYLOAD-01"),
        "and the content is the payload: {}",
        &out[..out.len().min(40)]
    );
}

// Verifies: gh #381 gap 5 seam 3 - **a returned buffer that kept its markers re-chips.**
//
// The return path is `set_text`, which gap 3 made re-derive the registry. A caller that
// round-trips the *marker* text therefore gets its chips back, with the payloads it
// already holds.
#[test]
fn a_returned_marker_buffer_rechips() {
    let mut e = with_chip("look: ", 24);
    let with_chip_form = e.text();
    // What an editor that left the marker text alone returns.
    e.set_text(&with_chip_form);
    assert_eq!(e.text(), with_chip_form, "the marker text is back");
    let expanded = e.submit();
    assert!(
        expanded.contains("PAYLOAD-24"),
        "and its payload came with it: {}",
        &expanded[..expanded.len().min(60)]
    );
}

// Verifies: gh #381 gap 5 seam 3 - **an edited payload is plain text, and its chip is
// gone - never a stale payload.**
//
// The hard case the brief asks to be decided rather than discovered. An external editor
// returns plain text, so once the marker is gone there is nothing to match it to. The
// decision: **the text wins and the chip is dropped**, because keeping a payload the
// user has edited past would ship bytes they never saw - this issue's original harm, one
// seam further out. `retain_live_payloads` is what enforces it, and the row proves the
// model cannot receive the old payload.
#[test]
fn an_edited_payload_drops_its_chip_rather_than_shipping_stale_bytes() {
    let mut e = with_chip("look: ", 24);
    // The editor expanded the chip, the user rewrote a line, the buffer came back with
    // no marker at all.
    let edited = "look: PAYLOAD-01\nPAYLOAD-02 EDITED BY HAND";
    e.set_text(edited);
    assert_eq!(e.text(), edited, "the edited text is what the buffer holds");
    assert!(
        e.pastes().is_empty(),
        "and no payload survives behind it: {:?}",
        e.pastes()
    );
    let submitted = e.submit();
    assert_eq!(
        submitted, edited,
        "the model receives exactly what the user left in the editor"
    );
    assert!(
        !submitted.contains("PAYLOAD-24"),
        "and never the payload the edit replaced"
    );
}

// Verifies: gh #381 gap 5 seam 3 - **a cleared editor clears the prompt**, and no
// payload can resurface afterwards.
#[test]
fn a_cleared_editor_leaves_no_payload_behind() {
    let mut e = with_chip("look: ", 24);
    e.set_text("");
    assert_eq!(e.text(), "");
    assert!(
        e.pastes().is_empty(),
        "the registry is re-derived from the empty text"
    );
    // And a later paste takes id 1 again - no dead entry to inherit.
    let mut e2 = with_chip("", 24);
    e2.set_text("");
    e2.insert_paste(
        &(1..=24)
            .map(|n| format!("X-{n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert!(
        e2.text().contains("[paste #1 "),
        "the next chip is numbered from a clean registry: {:?}",
        e2.text()
    );
}

// Verifies: gh #381 gap 5 seam 2 - **carrying the registry is not a memory
// regression.**
//
// A history entry now holds the marker text *and* the payloads, which looks like growth
// until you compare it with what it replaced: before this change the entry held the
// **expanded** text, i.e. the payload with the marker's place folded in. So the entry
// costs the payload either way. Measured rather than argued, because "we added a field"
// is exactly the kind of change that quietly doubles a buffer.
#[test]
fn carrying_the_registry_costs_no_more_than_the_expanded_text_did() {
    let mut e = Editor::new();
    let mut old_shape_bytes = 0usize;
    for round in 0..20 {
        let big = (0..=round)
            .map(|n| format!("line {n} {}", "x".repeat(500)))
            .collect::<Vec<_>>()
            .join("\n");
        e.insert_paste(&big);
        old_shape_bytes += e.submit().len();
    }
    let new_shape_bytes: usize = e
        .history
        .iter()
        .map(|h| h.text.len() + h.pastes.iter().map(String::len).sum::<usize>())
        .sum();
    // Not "smaller" - **within the same order**, which is the honest claim: the marker
    // text replaces the marker's own position inside the expanded text, so the two
    // differ by the marker's length and nothing else. Measured on 20 prompts of
    // growing size: 107104 vs 106715 bytes, a 0.4% difference. A doubling would be a
    // regression worth redesigning for; this is not.
    assert!(
        new_shape_bytes <= old_shape_bytes + old_shape_bytes / 100,
        "the marker+registry entry is the same size as the expanded text it replaced, \
         within one percent (new {new_shape_bytes} vs old {old_shape_bytes})"
    );
}
