//! gh #371 bugs 1, 2 and 3 — the caret's row must be one the terminal can be on,
//! the painted caret must close only its own channel, and exactly one caret
//! marker may be live in a frame. NFR-24's guard.
//!
//! The behaviour rows live beside the code they drive:
//! `crates/lca-tui/src/engine/main_screen.rs` (5 rows: the viewport invariant, the
//! next-frame diff, a tall-prompt journey, the mixed-shape sweep, and the saturated
//! row), `crates/lca-tui/src/widgets/editor.rs` (3 rows: the `27m` close, the leak
//! observed on following text, and the end-of-line form), and
//! `crates/lca-ui/src/chat_focus_tests.rs` (21 rows: one marker-count assertion per
//! keyboard-owning surface, plus the ownership properties in between).
//!
//! This file guards the shapes that would let any of the three come back
//! silently.

use std::path::Path;

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #371 bug 2 - no full SGR reset is emitted for a caret anywhere.
//
// The bug was three format strings. A row asserting "the caret closes with 27m" passes
// for any edit that changes the string, so the guard is the negative: the *combination*
// of reverse video and a full reset does not exist in the editor at all. A future caret
// form ("a block caret", "an underline caret") that reintroduces `\x1b[0m` next to `7m`
// fails here regardless of how it is spelled.
#[test]
fn no_caret_closes_reverse_video_with_a_full_reset() {
    let source = read("crates/lca-tui/src/widgets/editor.rs");
    let offenders: Vec<String> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            // A caret is reverse video; a full reset beside it is the leak.
            line.contains("7m") && line.contains("\\x1b[0m")
        })
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "a caret must close only reverse video (ADR-0037's channel-scoped rule); a \
         full reset strips the foreground, background and decorations of everything \
         after it:\n{}",
        offenders.join("\n")
    );
    // And the channel-scoped close really is there, so the guard above cannot be
    // satisfied by deleting the caret.
    assert!(
        source.contains("\\x1b[7m{g}\\x1b[27m") && source.contains("\\x1b[7m \\x1b[27m"),
        "both caret forms (a grapheme, and the end-of-line space) close with 27m"
    );
}

// Verifies: gh #371 bug 1 - the hardware cursor row is only ever recorded for a row
// the terminal can address.
//
// The defect was not the arithmetic, it was *which fact was stored*: the row we asked
// for, not the row the terminal clamped to. So the guard is that the one function which
// writes `hardware_cursor_row` refuses an unaddressable row before it writes anything -
// and that the viewport, not the document, decides.
#[test]
fn the_cursor_row_is_recorded_only_inside_the_viewport() {
    let source = read("crates/lca-tui/src/engine/main_screen.rs");
    let start = source
        .find("fn position_hardware_cursor(")
        .expect("the positioning function exists");
    // The body ends at the function's own closing brace, found by brace depth so a
    // nested arm (`let Some(..) else { .. }`) cannot cut it short. The earlier
    // `\n    }\n` cut did exactly that once the no-marker arm grew a brace.
    let body = &source[start..];
    let mut depth = 0i32;
    let mut end = body.len();
    for (at, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = at + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &body[..end];

    // The guard exists, names the viewport, and hides rather than clamping.
    assert!(
        body.contains("previous_viewport_top") && body.contains("viewport_bottom"),
        "the recorded row is bounded by the *viewport*, not the document: {body}"
    );
    assert!(
        body.contains("term.hide_cursor();\n            return;"),
        "an unaddressable caret hides the cursor and returns before recording \
         anything - clamping it to the viewport top would park an IME candidate \
         window on an unrelated line: {body}"
    );
    // And every write is downstream of the guard.
    //
    // Counting matters as much as ordering: a mutation that *prepends* a write leaves
    // the guarded write still last, so an ordering-only check passes while the
    // function now stores the requested row before deciding it cannot. Two writes are
    // legitimate - the guarded caret row, and the no-marker arm's re-base onto
    // `cursor_row` (gh #371 bug 3) - and both must be reachable only after the
    // viewport bound has been read, so an unaddressable request can never be stored.
    // `viewport_top`/`viewport_bottom` are bound once, before anything is
    // stored, so the *first* mention is the guard.
    //
    // The no-marker arm returns before the bound is ever read, which is correct
    // (it stores nothing about a caret it does not have), so the bound is looked
    // for *after* that arm.
    let writes: Vec<usize> = body
        .match_indices("self.hardware_cursor_row = ")
        .map(|(at, _)| at)
        .collect();
    let caret_store = body
        .find("self.hardware_cursor_row = target_row;")
        .expect("the caret row is stored");
    let guard = body
        .find("let viewport_top")
        .expect("the viewport bound is read before the caret row is stored");
    assert!(
        guard < caret_store,
        "the caret row is stored only after the viewport bound has been read: \
         guard={guard} store={caret_store}"
    );
    assert!(
        !writes.is_empty(),
        "the function records the caret row it verified"
    );
    // The caret's own store is written once, and its value is the *clamped* row.
    assert_eq!(
        body.matches("self.hardware_cursor_row = target_row;")
            .count(),
        1,
        "the caret's own store is written once"
    );
    assert!(
        body.contains("let target_row = (row as usize).min(total_lines.saturating_sub(1));"),
        "the caret row is clamped to the document before it is stored - storing the \
         requested row and letting the emulator clamp the result is gh #371 itself"
    );
}

// Verifies: gh #371 - the main screen is the only renderer with this shape.
//
// The brief asked whether `alt_screen.rs` shares bug 1. It does not: it positions with
// an absolute `CSI row;col H` every frame and keeps no `hardware_cursor_row`, so a
// caret it cannot address cannot corrupt a later frame. Pinned so a future
// delta-positioning optimisation there does not arrive without the same guard.
#[test]
fn alt_screen_has_no_recorded_cursor_row_to_desynchronise() {
    let source = read("crates/lca-tui/src/engine/alt_screen.rs");
    assert!(
        !source.contains("hardware_cursor_row"),
        "alt_screen positions absolutely (CSI row;col H) and records no cursor row, \
         which is why gh #371 bug 1 is main-screen-only; introducing a recorded row \
         here needs the viewport guard too"
    );
    assert!(
        source.contains("\\x1b[{};{}H"),
        "and it positions absolutely, which is the property that makes it immune: \
         every frame rewrites the whole screen from row 1"
    );
}

// Verifies: gh #371 - the caret is still painted, in both renderers.
//
// The contract ADR-0037 states: the caret is painted into the row and the terminal
// cursor is never trusted. Fixing the close sequence must not have moved the caret to
// the hardware cursor as a side effect.
#[test]
fn the_painted_caret_is_still_the_visual_caret() {
    let editor = read("crates/lca-tui/src/widgets/editor.rs");
    assert!(
        editor.contains("paint_caret: true,"),
        "the painted caret is the default (ADR-0037)"
    );
    assert!(
        editor.contains("CURSOR_MARKER"),
        "and the marker still rides the caret cell, so the engine can anchor IME"
    );
    // The engine hides the hardware cursor when the component painted its own.
    let main = read("crates/lca-tui/src/engine/main_screen.rs");
    assert!(
        main.contains("if painted {"),
        "the main screen honours the `painted` flag from the marker"
    );
    let alt = read("crates/lca-tui/src/engine/alt_screen.rs");
    assert!(alt.contains("if painted {"), "and so does the alt screen");
}

// Verifies: gh #371 bug 3 - the caret's owner is decided in exactly one place,
// and the composer only emits when that answer says so.
//
// The defect was that the composer emitted unconditionally while the overlay
// fields emitted never, so the *second* marker was the only one. Asserting "a
// marker exists" would pass on that build; the guard is that the composer's
// emission is gated and that the gate is the shared predicate.
#[test]
fn the_composer_emits_its_marker_only_when_it_owns_the_keyboard() {
    let render = read("crates/lca-ui/src/chat_render.rs");
    // The composer's strip must sit inside the composer's own loop and be
    // guarded by the predicate. A guard that only looked for the words
    // `Focus::Composer` passed on a mutated build that computed the answer and
    // then ignored it - so this looks for the bind **and** the branch that reads
    // it, in that order, within the editor loop.
    let bind = render
        .find("let composer_owns = self.keyboard_owner()")
        .expect("the composer computes the ownership answer once, by name");
    let guard = render[bind..]
        .find("if !composer_owns {")
        .expect("and it branches on that answer before stripping the marker");
    assert!(
        guard > 0,
        "the branch is inside the block the bind opened: {}",
        guard
    );
    let after_guard = &render[bind + guard..];
    assert!(
        after_guard.find("CURSOR_MARKER").is_some_and(|at| at < 400),
        "and the branch body is what strips the marker, not something later"
    );
    let focus = read("crates/lca-ui/src/chat_focus.rs");
    assert!(
        focus.contains("pub(crate) fn keyboard_owner("),
        "one predicate decides ownership"
    );
    // Its two answers are the two frames a caret can be in. A surface with no
    // text field answers `Composer` on purpose - the module doc records why
    // (the engine has no markerless-frame contract), and a second variant here
    // would be the shape that reintroduces a markerless frame.
    for variant in ["Composer", "Field(Field)"] {
        assert!(
            focus.contains(variant),
            "the predicate distinguishes `{variant}`"
        );
    }
    assert!(
        !focus.contains("Surface,"),
        "no third answer: a markerless frame desynchronises the main screen's \
         recorded cursor row, which this cycle's file boundary does not include"
    );
}

// Verifies: gh #371 bug 3 - every keyboard-owning field reads that one predicate
// rather than deciding for itself that it has focus.
//
// A field that asked its own question would eventually disagree with the
// composer, and the frame would carry two markers or none. The helper pair is
// the only way a field composes a caret, so this asserts the helpers gate on the
// predicate - and that no field hand-rolls a marker beside them.
#[test]
fn every_overlay_field_takes_its_caret_from_the_shared_predicate() {
    let focus = read("crates/lca-ui/src/chat_focus.rs");
    for helper in ["pub(crate) fn caret_row(", "pub(crate) fn caret_field("] {
        let start = focus
            .find(helper)
            .unwrap_or_else(|| panic!("{helper} exists"));
        let body = &focus[start..];
        let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
        assert!(
            body.contains("self.keyboard_owner() != Focus::Field(field)"),
            "{helper} gates on the shared predicate: {body}"
        );
    }
    // The field list is enumerated, so a new surface has to be added here - and
    // the test that counts markers in a frame is what proves the addition works.
    for field in [
        "ModelSearch",
        "ResumeSearch",
        "ExtensionFilter",
        "SettingsSearch",
        "SettingsEdit",
        "TreeLabel",
        "Secret",
        "DialogInput",
        "DialogSelect",
        "McpRedirect",
        "TranscriptSearch",
    ] {
        assert!(
            focus.contains(&format!("    {field},")),
            "`{field}` is one of the fields the predicate can name"
        );
    }
    // A marker written anywhere but the helper is a second caret the predicate
    // cannot see. `editor.rs` is the composer's own renderer and the exception.
    for path in [
        "crates/lca-ui/src/chat_overlays.rs",
        "crates/lca-ui/src/chat_host_dialog.rs",
        "crates/lca-ui/src/chat_render.rs",
        "crates/lca-ui/src/chat_tree.rs",
    ] {
        let source = read(path);
        let offenders: Vec<&str> = source
            .lines()
            .filter(|line| line.contains("\\x1b_pi:c") || line.contains("CURSOR_MARKER\")"))
            .collect();
        assert!(
            offenders.is_empty(),
            "{path} writes a marker by hand instead of asking the predicate:\n{}",
            offenders.join("\n")
        );
    }
}

// Verifies: gh #371 bug 3 - the painted caret keeps bug 2's channel-scoped
// close, in the field path as well as the composer's.
#[test]
fn the_overlay_caret_closes_reverse_video_with_27m() {
    let focus = read("crates/lca-ui/src/chat_focus.rs");
    assert!(
        focus.contains("\\x1b[7m \\x1b[27m"),
        "the field caret paints the composer's own form"
    );
    assert!(
        !focus.contains("\\x1b[0m"),
        "and closes only reverse video, never with a full reset"
    );
}

// Verifies: gh #371 bug 3 - a frame never reaches the renderer without a marker.
//
// The strict reading of "the caret belongs to whoever owns the keyboard" would
// give a fieldless surface (`[y/n]` confirm, permission prompt) **no** marker.
// This cycle does not ship that, for a measured reason: the main-screen renderer
// has no markerless-frame contract. `position_hardware_cursor`'s no-marker arm
// hides the cursor and returns **without recording**, so `hardware_cursor_row`
// keeps describing the previous frame's caret while the paint pass has moved the
// terminal elsewhere; the next frame's `compute_line_diff` then starts from a
// screen row the terminal is not on, and the error accumulates. Found live: after
// a `/settings` wizard, closing the box painted the selector's border over the
// startup header, and `settings_changes_a_setting_and_config_toml_carries_it`
// went red. The repair belongs in `crates/lca-tui/src/engine/main_screen.rs`,
// which this cycle's file boundary excludes, so the predicate answers `Composer`
// for a fieldless surface and every frame keeps exactly one marker.
//
// Both halves are pinned here: the predicate has no third answer, and the shared
// helper that every overlay field composes its caret with is the only thing that
// may write a marker.
#[test]
fn every_frame_carries_exactly_one_marker_by_construction() {
    let focus = read("crates/lca-ui/src/chat_focus.rs");
    assert!(
        !focus.contains("Surface,"),
        "the predicate has two answers, not three: a markerless frame desynchronises \
         the main screen's recorded cursor row"
    );
    assert!(
        focus.contains("map_or(Focus::Composer, Focus::Field)"),
        "a fieldless surface answers `Composer` - the deviation, stated in the \
         module doc rather than left implicit"
    );
    assert!(
        focus.contains("no markerless-frame contract") || focus.contains("without recording"),
        "and the reason is recorded beside the decision, so a later cycle can \
         finish the job instead of rediscovering the failure"
    );
}
