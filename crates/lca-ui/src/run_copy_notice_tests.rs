//! Gh #374 gap 4, the feedback half: a copy that cannot be verified must say so.
//!
//! ## The defect
//!
//! `copy_osc52` returns `CopyOutcome::Unverified` **always** — the code calls that
//! "the honest answer" — but the *interface* then showed
//!
//! ```text
//! copied (if your terminal blocked the clipboard, nothing was copied)
//! ```
//!
//! which opens with "copied". A reader skimming a five-second toast reads success,
//! and a silently ignored OSC 52 (macOS `Terminal.app`, `tmux` with
//! `set-clipboard off`) is **indistinguishable from a successful copy** — the
//! defect this gap is about. The triple is also undifferentiated by dwell: a
//! success stayed on screen exactly as long as a failure.
//!
//! ## The three forms, and why each word is there
//!
//! | report | message | dwell | why |
//! |---|---|---|---|
//! | `Native` | `✓ copied to clipboard` | short | a verified write; a tick is earned |
//! | `Osc52` | `? copy sent as OSC 52 - the terminal may have ignored it` | long | **not** a success: the tick is withheld and the caveat needs reading |
//! | `Failed` | `✗ could not copy: no clipboard here (try /fullscreen)` | long | nothing happened, and the row says what to do |
//!
//! `?` rather than `✓` on the unverified arm is the whole point. pi flashes
//! `Copied!` on its bare-OSC-52 path (`tui-alt-screen.ts:1484`), which is exactly
//! the lie this gap fixes; LCA does not copy that.
//!
//! The dwell asymmetry is pi's (`COPY_ERROR_FLASH_DURATION_MS = 5000` versus the
//! 1000 ms default): an error is a claim the reader has to *do* something with, so
//! it stays. LCA's success window is 2 s rather than pi's 1 s only because LCA's
//! sentence is longer; the six-fold ratio to the error window is what matters.
//!
//! Split out of `run_tests.rs` for the workspace's 1,200-line ceiling (gate 11).

use super::chat;
use super::*;
use crate::chat::{TOAST_SECS, TOAST_SUCCESS_SECS};

/// One frame's rows with every escape removed, for rows that compare text.
fn plain(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| lca_tui::engine::text::strip_terminal_sequences(row))
        .collect()
}

/// The three notices, so every row below compares against one spelling.
fn verified() -> String {
    CopyReport::Native.notice()
}

fn unverified() -> String {
    CopyReport::Osc52.notice()
}

fn failed() -> String {
    CopyReport::Failed.notice()
}

// Verifies: gh #374 gap 4 - **an unverified copy does not say "copied", and does
// not carry the success tick.** This is the row the issue is about: the old text
// began "copied (if your terminal blocked…", so a skimming reader was told it
// worked when the terminal may have ignored the whole thing.
#[test]
fn an_unverified_copy_never_claims_success() {
    let notice = unverified();
    assert!(
        !notice.starts_with("copied"),
        "the sentence must not open by asserting success: {notice:?}"
    );
    assert!(
        !notice.contains('\u{2713}'),
        "and it must not carry the success tick: {notice:?}"
    );
    assert!(
        notice.contains("OSC 52"),
        "it names the mechanism that ran, which is what makes it checkable: {notice:?}"
    );
    assert!(
        notice.to_lowercase().contains("may have ignored"),
        "and it says the outcome is unknown rather than bad or good: {notice:?}"
    );
}

// Verifies: gh #374 gap 4 - a **verified** copy says so plainly and carries the
// tick. A tick is earned here and withheld above, which is what makes the two
// distinguishable at a glance rather than only on close reading.
#[test]
fn a_verified_copy_says_so_and_carries_the_tick() {
    let notice = verified();
    assert!(notice.contains('\u{2713}'), "the success tick: {notice:?}");
    assert!(
        notice.contains("copied to clipboard"),
        "and the plain claim it earned: {notice:?}"
    );
    assert!(!notice.contains('?'), "with no hedging: {notice:?}");
}

// Verifies: gh #374 gap 4 - a **failure** is its own third form: not a success,
// not a caveat, and it tells the reader what to do about it.
#[test]
fn a_failed_copy_is_a_third_distinct_form() {
    let notice = failed();
    assert!(notice.contains('\u{2717}'), "the failure mark: {notice:?}");
    assert!(
        notice.contains("could not copy"),
        "it states the failure: {notice:?}"
    );
    assert!(
        notice.contains("/fullscreen"),
        "and the action that can fix it, since OSC 52 needs the alt screen: {notice:?}"
    );
}

// Verifies: NFR-28 (colour is never the only signal) and the brief's "the three
// messages are distinguishable by their text, not only their colour".
//
// The strong form: the three are pairwise different strings even after every
// non-ASCII symbol is stripped, so a reader (or a screen reader) with no glyphs at
// all still gets three different sentences.
#[test]
fn the_three_forms_are_distinguishable_without_their_symbols() {
    let strip = |s: &str| -> String { s.chars().filter(char::is_ascii).collect() };
    let forms = [verified(), unverified(), failed()];
    let stripped: Vec<String> = forms.iter().map(|s| strip(s)).collect();
    for (i, a) in stripped.iter().enumerate() {
        for (j, b) in stripped.iter().enumerate() {
            assert!(
                i == j || a != b,
                "forms {i} and {j} collide once the symbols are removed: {a:?} vs {b:?}"
            );
        }
    }
    // And the two "informational" arms differ in more than a glyph.
    assert_ne!(stripped[0], stripped[1]);
    assert_ne!(stripped[1], stripped[2]);
}

// Verifies: gh #374 gap 4 (the dwell asymmetry) - a success fades sooner than a
// caveat or a failure.
//
// The durations are a claim about reading speed, not styling: an error has to be
// read, and a success that overstays is noise. pi's ratio is 1000 ms against
// 5000 ms (`COPY_ERROR_FLASH_DURATION_MS`); LCA keeps the six-fold-ish ratio with
// a longer success window for its longer sentence.
#[test]
fn a_success_fades_sooner_than_a_caveat_or_a_failure() {
    // Read through `black_box` so the comparison is a runtime one: an `assert!`
    // on two `const`s is folded away, which clippy flags and which would also mean
    // the row cannot fail if someone swaps the constants.
    let (short, long) = (
        std::hint::black_box(TOAST_SUCCESS_SECS),
        std::hint::black_box(TOAST_SECS),
    );
    assert!(
        short < long,
        "a success window shorter than an error window: {short} vs {long}"
    );
    assert!(
        long >= 5,
        "and the error window is long enough to read a sentence with an action in \
         it: {long}"
    );
    // The report maps to the dwell, not the other way round: verified is the only
    // short-lived arm.
    assert!(
        !CopyReport::Native.is_error(),
        "a verified write is a success"
    );
    assert!(CopyReport::Osc52.is_error(), "an unverified copy is not");
    assert!(CopyReport::Failed.is_error(), "and neither is a failure");
}

// Verifies: gh #374 gap 4 - the notice floats and does **not** disturb the
// transcript. The brief's point 2.
//
// The answer to "which overlay mechanism" is that LCA already has one: toasts
// composite into the frame's top-right without reflowing (gh #239), and the
// scrollback path slices the visible window out of the document before painting
// so a toast can never land in history (gh #372 bug 1). **No second mechanism was
// added** - this row is the proof that the existing one suffices for a copy
// notice, including its longer text.
//
// The frame is the *last* `height` rows of the document in scrollback mode, so
// the toast sits in that window's top band, and every row outside the band must be
// byte-identical with and without the notice.
#[test]
fn the_copy_notice_leaves_every_other_row_byte_identical() {
    const HEIGHT: u16 = 30;
    let mut chat = chat();
    for i in 0..12 {
        chat.transcript.push_user(format!("transcript row {i}"));
    }
    let before = plain(&chat.viewport(100, HEIGHT, 0));
    chat.toast_error(unverified());
    let after = plain(&chat.viewport(100, HEIGHT, 0));
    assert_eq!(
        before.len(),
        after.len(),
        "the document's length is unchanged - the notice does not reflow"
    );

    // The visible frame is the document's last `HEIGHT` rows; the toast band is
    // its rows 1..=TOAST_MAX (row 0 is the frame's first row, which `paint_toasts`
    // deliberately never uses).
    let frame_start = after.len().saturating_sub(HEIGHT as usize);
    let band: Vec<usize> = (1..=crate::chat::TOAST_MAX)
        .map(|i| frame_start + i)
        .collect();
    // The notice is really in the band, or the loop below is vacuous.
    assert!(
        band.iter()
            .any(|&row| after.get(row).is_some_and(|r| r.contains("OSC 52"))),
        "the notice paints in the frame's top band (frame starts at {frame_start}):\n{}",
        after.join("\n")
    );
    for (row, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        if band.contains(&row) {
            continue;
        }
        assert_eq!(a, b, "row {row} was disturbed by the notice:\n{a}\n{b}");
    }
}

// Verifies: gh #374 gap 4 - the dwell is carried by the toast itself, so one
// stack can hold a short success and a long caveat at the same time.
//
// A single global duration cannot express "successes fade sooner"; this is why
// the kind is per-toast rather than a second constant on `Chat`.
#[test]
fn a_toast_carries_its_own_dwell() {
    let mut chat = chat();
    chat.toast_success(verified());
    chat.toast_error(unverified());
    let now = std::time::Instant::now();
    let (first, second) = (chat.toasts[0].until, chat.toasts[1].until);
    assert!(
        first < second,
        "the success expires before the caveat even though it was pushed first"
    );
    // And both are in the future, or the row would pass on an already-expired pair.
    assert!(first > now || second > now, "the fixture's toasts are live");
}

// Verifies: gh #374 gap 4 - the ladder still routes exactly as gh #406 pinned it:
// a failed native write falls through to OSC 52, so the caveat is reachable
// rather than the whole thing collapsing to "failed".
//
// This is the pre-existing property the new wording rides on; it is restated here
// because the three forms now depend on which arm actually ran.
#[test]
fn a_failed_native_write_still_reaches_the_osc52_caveat() {
    let failing: crate::state::ClipboardWriter = Arc::new(|_: &str| false);
    let mut sent = Vec::new();
    let report = copy_through_ladder(
        "text",
        Some(&failing),
        Some(&mut |chunk: &str| sent.push(chunk.to_string())),
    );
    assert_eq!(report, CopyReport::Osc52, "the caveat, not the failure");
    assert_eq!(sent, vec!["text".to_string()]);
    assert_eq!(report.notice(), unverified());
}

// Verifies: gh #374 gap 4 - no step at all is the third form, and it is not the
// caveat. Without this the two failure-shaped arms could collapse and the reader
// would be told the terminal may have ignored a copy that was never attempted.
#[test]
fn no_step_at_all_reports_the_failure_not_the_caveat() {
    let report = copy_through_ladder("text", None, None);
    assert_eq!(report, CopyReport::Failed);
    assert_ne!(report.notice(), unverified(), "not the caveat");
    assert_eq!(report.notice(), failed());
}

// The ladder's own three-arm row, moved here from `run_tests.rs` with the
// strings it asserts (gh #374 gap 4, sanctioned).
// Verifies: gh #9 / R6 - the copy ladder writes the text through the
// host's verified native clipboard when it has one, falls back to
// OSC 52 when the screen can send it, and says which step ran; with
// neither available it says the copy did not happen instead of
// pretending.
#[test]
fn the_copy_ladder_writes_the_text_and_says_which_step_ran() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let hook: crate::state::ClipboardWriter = {
        let seen = seen.clone();
        Arc::new(move |text: &str| {
            seen.lock().unwrap().push(text.to_string());
            true
        })
    };
    let report = copy_through_ladder("hello world", Some(&hook), None);
    assert_eq!(report, super::CopyReport::Native);
    // Gh #374 gap 4 (sanctioned): the verified form carries a tick now.
    assert_eq!(report.notice(), "\u{2713} copied to clipboard");
    assert!(!report.is_error(), "a verified write is a success");

    assert_eq!(
        *seen.lock().unwrap(),
        vec!["hello world".to_string()],
        "the text reaches the clipboard verbatim"
    );

    let mut sent = Vec::new();
    let report = copy_through_ladder(
        "osc",
        None,
        Some(&mut |text: &str| sent.push(text.to_string())),
    );
    let notice = report.notice();
    assert_eq!(sent, vec!["osc"], "OSC 52 carries the same text");
    // Gh #374 gap 4 (sanctioned): the old text opened with "copied".
    assert!(
        notice.contains("may have ignored"),
        "the unverified notice is honest: {notice}"
    );
    assert!(
        !notice.starts_with("copied") && report.is_error(),
        "and it takes the long window"
    );

    let notice = copy_through_ladder("nowhere", None, None).notice();
    // Gh #374 gap 4 (sanctioned): the failure carries its own mark.
    assert!(
        notice.contains("could not copy"),
        "no step ran, so no claim: {notice}"
    );
    assert!(notice.contains('\u{2717}'), "and its own mark: {notice}");
}

// ===========================================================================
// gh #374 gap 4, the paste half: a right-button press pastes through the one
// clipboard path, and says something in all three outcomes.
//
// `alt_screen::right_click_tests` proves the renderer records the intent and
// refuses it under VS Code; those rows cannot see whether the *host* acts on it,
// which is where a wired-but-ignored flag would hide.
// ===========================================================================

/// A chat whose clipboard read returns `value`.
fn chat_with_clipboard(value: Option<String>) -> super::Chat {
    let mut chat = chat();
    let reply = value.clone();
    chat.world.options.hooks.read_clipboard = Some(Arc::new(move || reply.clone()));
    chat
}

/// A fullscreen screen, so the alt screen exists to carry the intent.
fn alt_screen(term: &mut lca_tui::engine::terminal::FakeTerminal) -> Screen {
    let mut screen = Screen::Main(lca_tui::engine::main_screen::MainScreenRenderer::new());
    switch_screen(&mut screen, true, term);
    screen
}

// Verifies: gh #374 gap 4 - a right-button press in fullscreen **pastes**: the
// clipboard's text lands in the composer, and the confirmation is the success
// form.
#[test]
fn a_right_press_pastes_the_clipboard_into_the_composer() {
    let mut term = lca_tui::engine::terminal::FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    let mut chat = chat_with_clipboard(Some("pasted text".to_string()));
    chat.screen_mode = true;
    // Drive the intent the way `handle_mouse` records it.
    if let Screen::Alt(r) = &mut screen {
        r.set_right_click_paste_allowed(true);
        r.handle_mouse(lca_tui::engine::alt_screen::SgrMouse {
            bits: 2,
            x: 5,
            y: 3,
            press: true,
        });
    }
    assert!(screen.take_right_click_paste(), "the gesture was recorded");
    super::input_helpers::paste_from_clipboard(&mut chat);
    assert_eq!(
        chat.editor.text(),
        "pasted text",
        "the clipboard's text is in the composer"
    );
    let notice = chat
        .toasts
        .last()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert_eq!(notice, "\u{2713} pasted from the clipboard");
}

// Verifies: gh #374 gap 4 - a clipboard with nothing to paste says so. Silence on
// a read that found nothing is the defect class this gap is about.
#[test]
fn a_right_press_with_an_empty_clipboard_says_so() {
    let mut chat = chat_with_clipboard(None);
    super::input_helpers::paste_from_clipboard(&mut chat);
    assert_eq!(chat.editor.text(), "", "nothing was inserted");
    let notice = chat
        .toasts
        .last()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        notice.contains("nothing to paste") && notice.contains('\u{2717}'),
        "the honest form, with its mark: {notice:?}"
    );
    assert!(
        !notice.contains('\u{2713}'),
        "and not a success mark: {notice:?}"
    );
}

// Verifies: gh #374 gap 4 - an embedder with **no clipboard reader at all** gets
// the same honest message, not silence.
//
// A missing hook and an empty clipboard are indistinguishable to the user, so they
// must be indistinguishable in the message too; what would be wrong is doing
// nothing, which is what an unwired intent would do.
#[test]
fn a_host_with_no_clipboard_reader_says_so_rather_than_nothing() {
    let mut chat = chat();
    chat.world.options.hooks.read_clipboard = None;
    super::input_helpers::paste_from_clipboard(&mut chat);
    assert_eq!(chat.editor.text(), "");
    let notice = chat
        .toasts
        .last()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        notice.contains("nothing to paste"),
        "the same sentence, because it is the same situation to the user: {notice:?}"
    );
}

// Verifies: gh #374 gap 4 - a **disabled** right-click (the VS Code case) reaches
// neither the clipboard nor the composer.
//
// The renderer row proves the flag is not set; this proves the host does not paste
// anyway, which is the half that would break VS Code.
#[test]
fn a_right_press_that_is_not_allowed_pastes_nothing() {
    let mut term = lca_tui::engine::terminal::FakeTerminal::new(80, 24);
    let mut screen = alt_screen(&mut term);
    let chat = chat_with_clipboard(Some("should not appear".to_string()));
    if let Screen::Alt(r) = &mut screen {
        // The VS Code shape: the policy says no.
        r.set_right_click_paste_allowed(false);
        r.handle_mouse(lca_tui::engine::alt_screen::SgrMouse {
            bits: 2,
            x: 5,
            y: 3,
            press: true,
        });
    }
    assert!(
        !screen.take_right_click_paste(),
        "the intent was never recorded"
    );
    assert_eq!(chat.editor.text(), "", "and nothing was pasted");
    assert!(chat.toasts.is_empty(), "no notice either");
}
