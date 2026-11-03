//! Gh #381 gaps 1-2: a paste chip is **one unit** for every motion and delete path,
//! so the orphaned-marker corruption is unreachable.
//!
//! The predicate already existed - `engine::word::atomic_span_at`, added by gh #383
//! for word motion. What did not consult it: plain `Left`/`Right`, `Up`/`Down`, the
//! line wrapper, and `Backspace`. The consequences the issue records:
//!
//! - `Left` stepped **21 keystrokes** through `[paste #1 +123 lines]`;
//! - `Alt+Left/Right` were already fine (that is where the predicate came from);
//! - `Up`/`Down` could land **inside** a chip;
//! - a wrap split `paste`/`#1` across rows;
//! - one `Backspace` on `]` left `[paste #1 +123 lines`, which no longer matches the
//!   marker pattern - so `expand_paste_markers` passed it to the **model literally**.
//!
//! The behavioural rows live in `crates/lca-tui/src/widgets/editor_atomic_tests.rs`.
//! This file is the NFR-24 guard, and it pins the seam rather than the rows: **one
//! predicate, every motion path asks it**. A fix that repaired the orphan instead of
//! preventing it, or that grew a second notion of "unit", passes the behavioural rows
//! and fails here.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// Source with `//` comment lines removed, so a row about *code* is neither
/// satisfied nor defeated by prose.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("{signature} has a balanced body");
}

// Verifies: gh #381 gap 1 - every motion path consults the one predicate.
//
// The design rule from the brief: "one predicate, every motion path asks it". The
// predicate is `engine::word::atomic_span_at` (gh #383); the editor reaches it
// through exactly one helper, `chip_span`. So the guard is that `cursor_left`,
// `cursor_right`, `backspace`, `delete_forward` and `move_to_visual_row` all route
// through `chip_span`-derived helpers, and that `editor.rs` contains no second
// marker recogniser.
#[test]
fn every_motion_path_consults_the_one_atomic_predicate() {
    // The chip helpers were split into `editor_atomic.rs` at the 1,200-line ceiling;
    // the guard reads the whole editor surface rather than one file, so the split
    // does not weaken it.
    let source = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );

    // One recogniser, in `word.rs`. The editor must not re-implement it.
    assert!(
        !source.contains("[paste #\"]"),
        "the editor does not spell the marker shape itself"
    );
    let predicate_calls = source.matches("engine::word::atomic_span_at").count()
        + source
            .matches("engine::word::atomic_start_ending_at")
            .count();
    assert!(
        predicate_calls >= 2,
        "the editor reaches the shared predicate: {predicate_calls} call sites"
    );

    // Each motion/delete path routes through a chip-aware helper.
    for (signature, helper) in [
        ("pub fn cursor_left(", "atomic_boundary_left"),
        ("pub fn cursor_right(", "atomic_boundary_right"),
        ("pub fn backspace(", "chip_span"),
        ("pub fn delete_forward(", "chip_span"),
        ("fn move_to_visual_row(", "snap_out_of_chip"),
    ] {
        let body = function_body(&source, signature);
        assert!(
            body.contains(helper),
            "{signature} asks `{helper}`, which is the chip-aware path: {body}"
        );
    }

    // And each helper's chip branch must actually *do* something: a mutation that
    // keeps the call but makes the chip arm fall through to the cluster path leaves
    // the row above green (it did, before this assertion).
    //
    // The property, not the spelling (gh #381 gap 4's live finding): the chips arms
    // must be **guarded**, because an unguarded arm makes the movement a no-op at the
    // boundary it matches. An earlier version of this guard pinned
    // `Some((start, _)) => start,` literally - which is exactly the stuck-`Left` bug
    // it was meant to protect, so tightening it to the literal string locked the
    // defect in. The `if` is the thing that matters.
    let left = function_body(&source, "fn atomic_boundary_left(");
    assert!(
        left.contains("Some((start, _)) if start < col => start,"),
        "left moves off the chip's start (an unguarded arm is a no-op there): {left}"
    );
    let right = function_body(&source, "fn atomic_boundary_right(");
    assert!(
        right.contains("Some((start, end)) if col >= start && col < end => end,"),
        "right moves off the chip's end (the mirror case): {right}"
    );
    assert!(
        left.contains("Self::prev_cluster_boundary(line, col)")
            && right.contains("Self::next_cluster_boundary(line, col)"),
        "and the cluster snap is only the non-chip fallback: left={left} right={right}"
    );
}

// Verifies: gh #381 gap 2 - the delete removes the **span**, not "a character that
// leaves invalidity".
//
// The brief's distinction is what makes the orphan unreachable rather than
// repaired: a repair would inspect the text after the fact and fix a bad shape, and
// would still ship if a future path forgot. The guard is that the removed range is
// the chip's own `(start, end)`.
#[test]
fn the_delete_removes_the_chip_span_rather_than_repairing_text() {
    let source = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let backspace = function_body(&source, "pub fn backspace(");
    assert!(
        backspace.contains("Some((start, end)) => (start, end)"),
        "backspace removes the chip's whole span: {backspace}"
    );
    assert!(
        !backspace.contains("if !self.text().contains(\"[paste")
            && !backspace.contains("trim_end_matches"),
        "it does not post-hoc repair the text: {backspace}"
    );
    let forward = function_body(&source, "pub fn delete_forward(");
    assert!(
        forward.contains("Some((start, end)) => (start, end)"),
        "delete_forward removes the whole span too: {forward}"
    );
}

// Verifies: gh #381 gap 2 - renumbering exists, and it moves labels rather than
// payloads.
//
// `insert_paste` derives the id from `pastes.len() + 1`, so after a delete the
// surviving chip's label and its payload index diverge - and the next paste takes a
// colliding id. `renumber_pastes` rebuilds `pastes` in document order, so each
// marker's payload is re-registered under its new position: a renumber can change a
// label but never what it expands to.
#[test]
fn the_renumber_rebinds_payloads_in_document_order() {
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let body = function_body(&atomic, "fn renumber_pastes(");
    assert!(
        body.contains("payloads.push(self.pastes[id - 1].clone())"),
        "each marker's payload is carried over as the label is rewritten: {body}"
    );
    assert!(
        body.contains("self.pastes = payloads"),
        "and `pastes` is replaced by that document-ordered list: {body}"
    );
    // Both delete paths call it.
    let source = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    for signature in ["pub fn backspace(", "pub fn delete_forward("] {
        let body = function_body(&source, signature);
        assert!(
            body.contains("renumber_pastes()"),
            "{signature} renumbers after removing a chip: {body}"
        );
    }
}

// Verifies: gh #381 gap 1 - the wrapper treats a chip as unbreakable.
//
// The same requirement as "do not split a wide glyph", and the reason `paste`/`#1`
// broke across rows. One mechanism only: two guards for one property means either
// can mask the other (my first attempt had two, and the mutation that removed one
// passed - a guard that cannot fail proves nothing).
#[test]
fn the_wrapper_treats_a_chip_as_unbreakable() {
    let source = code_only(&read("crates/lca-tui/src/widgets/editor_rows.rs"));
    let body = function_body(&source, "pub fn wrap_row(");
    assert!(
        body.contains("engine::word::atomic_span_at"),
        "the wrapper asks the same predicate the editor does: {body}"
    );
    assert!(
        body.contains("chip_end"),
        "and carries a chip's span so its inner graphemes cannot break: {body}"
    );
    // The chip must actually start a fresh row when it would overflow, or it is
    // simply allowed to split.
    assert!(
        body.contains(
            "if current_width > 0 && current_width + chip_width > width && chunk_start < *start {"
        ),
        "and moves to a new row when the chip would overflow: {body}"
    );
    // But a chip **wider than the pane** must fall through to the ordinary
    // force-break: a row wider than the terminal breaks the renderer's contract.
    // pi splits that case at grapheme granularity and keeps it atomic only for
    // cursor and editing ("the split is purely visual for word-wrap layout").
    assert!(
        body.contains("if chip_width > width {"),
        "a chip wider than the pane is split visually rather than overflowing: {body}"
    );
    // And the whole mechanism must be reachable: a guard that cannot fail proves
    // nothing, so a condition that has been neutered (`&& false` / `|| true`) is
    // rejected outright. This is what my first version of this row missed - it
    // asserted shapes that a `&& false` mutation left intact.
    for neutered in ["&& false", "|| true"] {
        assert!(
            !body.contains(neutered),
            "the unbreakable-chip path is not disabled ({neutered}): {body}"
        );
    }
}

// Verifies: gh #381 gap 2 - the model never receives marker text.
//
// The user-visible harm, and the reason this is the row that matters: a marker is a
// display stand-in, so any marker text reaching `expand_paste_markers` *unexpanded*
// means the model read a placeholder instead of the content. The behavioural rows
// drive real deletes; this guard pins the declaration they rely on.
#[test]
fn expansion_is_the_only_way_a_marker_leaves_the_editor() {
    // `expand_paste_markers` moved with the chip helpers; read both.
    let source = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );
    // `expand_paste_markers` is the only public exit that reads the buffer for a
    // submission, and it is called from the submit paths.
    assert!(
        source.contains("expand_paste_markers(&self.text(), &self.pastes)"),
        "the submission path expands markers: {source}"
    );
    // An unrecognised marker is left literal *by design* (a user may type the
    // shape); that is why the editor must never produce an unrecognised one.
    let expand = function_source(&source, "fn expand_paste_markers(");
    assert!(
        expand.contains("out.push('[')"),
        "a hand-typed marker staying literal is deliberate: {expand}"
    );
}

/// The whole text of a top-level function (from its signature to the line before the
/// next top-level item), for functions that are not methods.
fn function_source<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map(|at| at + 2).unwrap_or(rest.len());
    &rest[..end]
}

// ---------------------------------------------------------------------------
// gap 3: the registry is the truth, the marker text is its label
// ---------------------------------------------------------------------------

// Verifies: gh #381 gap 3 - the undo entry carries the registry.
//
// `undo.pop()` used to restore `(lines, cursor)` only, so undoing a paste-delete
// brought `[paste #1 …]` back with an **empty registry** and the marker expanded to
// nothing. The brief is explicit that the fix is the snapshot carrying what a restore
// needs - *not* `expand_paste_markers` learning to tolerate a missing entry, which
// turns corruption into silence. Both halves are pinned here.
#[test]
fn the_undo_entry_carries_the_registry_and_expansion_stays_strict() {
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let entry = function_body(&atomic, "pub(super) struct UndoEntry {");
    assert!(
        entry.contains("chips: Option<Vec<String>>"),
        "the undo entry carries the paste registry: {entry}"
    );

    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let undo = function_body(&editor, "pub fn undo(");
    assert!(
        undo.contains("self.pastes = pastes"),
        "and the restore puts it back: {undo}"
    );

    // The strictness half: an unknown id is still left literal, never silently
    // emptied. Tolerating absence would have "fixed" the symptom by hiding it.
    let expand = function_body(&atomic, "pub(super) fn expand_paste_markers(");
    assert!(
        expand.contains("out.push('[')"),
        "an unknown marker is still left as text, not expanded to nothing: {expand}"
    );
}

// Verifies: gh #381 gap 3 - `set_text` re-derives rather than clearing blindly.
//
// The brief's axis: history/draft restore needs chips preserved, a wholesale
// replacement legitimately drops them. One blanket behaviour is wrong either way, so
// the rule is that **a payload survives exactly when its marker does** - which gets
// both callers right without a second code path.
#[test]
fn set_text_re_derives_the_registry_from_the_text() {
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let set = function_body(&editor, "pub fn set_text(");
    assert!(
        set.contains("retain_live_payloads"),
        "set_text re-derives the registry from the new text: {set}"
    );
    assert!(
        !set.contains("self.pastes.clear()"),
        "and does not clear it blindly: {set}"
    );

    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let retain = function_body(&atomic, "pub(super) fn retain_live_payloads(");
    // Truncation, not compaction: ids are positional, so compacting would re-point
    // `#2` at `#1`'s content - the very corruption this gap is about, reintroduced in
    // the fix. (My first draft did exactly that and the row caught it.)
    assert!(
        retain.contains("pastes[..highest.min(pastes.len())].to_vec()"),
        "survivors keep their positions, so no id is re-pointed: {retain}"
    );
}

// Verifies: gh #381 gap 3 - a preserving caller can carry the registry.
//
// The text alone cannot reconstruct a payload (a marker is a label, not a
// container), so a tab switch restoring a saved draft must hand the payloads over.
// `set_text_with_pastes` is that pair; `set_text` alone remains the replacement path.
#[test]
fn a_preserving_caller_has_a_pair_to_carry_the_registry() {
    // The pair moved to `editor_atomic.rs` at the 1,200-line ceiling, so the guard
    // reads the whole editor surface rather than one file.
    let editor = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );
    assert!(
        editor.contains("pub fn set_text_with_pastes("),
        "the preserving entry point exists"
    );
    assert!(
        editor.contains("pub fn pastes(&self) -> &[String]"),
        "and the save side can read the registry to carry it"
    );
    // The preserving variant must still filter - carrying stale payloads would let a
    // later marker inherit content that is not its own.
    let with = function_body(&editor, "pub fn set_text_with_pastes(");
    assert!(
        with.contains("retain_live_payloads"),
        "and it keeps only what the restored text reaches: {with}"
    );
}

// Verifies: gh #381 gap 3 - labels are positions, so uniqueness needs no counter.
//
// A counter cannot be both unique across undo and contiguous after a delete:
// rewinding it collides with a restored chip, and not rewinding yields `#1,#3`. The
// resolution is that the label is the next free slot and `renumber_pastes` maps the
// live order onto `1..n` after every change, which makes a restored or new chip
// unique by construction.
#[test]
fn labels_are_positions_and_renumber_keeps_them_unique() {
    let editor = format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    );
    let paste = function_body(&editor, "fn insert_paste(");
    assert!(
        paste.contains("let id = self.pastes.len() + 1"),
        "the new chip takes the next free slot: {paste}"
    );
    assert!(
        paste.contains("self.renumber_pastes()"),
        "and the buffer is renumbered after the insert: {paste}"
    );
    assert!(
        !editor.contains("paste_counter"),
        "no counter is kept: it cannot be both unique across undo and contiguous"
    );
}

// ---------------------------------------------------------------------------
// gap 4: the caret on a chip paints the whole badge
// ---------------------------------------------------------------------------
//
// Gaps 1-3 made a chip one unit for motion, deletion and restore; on screen it still
// looked like any other column, so nothing told the user the thing under the caret was
// atomic. pi merges a marker's graphemes into one segment (`segmentWithMarkers`,
// editor.ts:44-60) and paints that segment reversed - so the badge, not one cell, is
// the caret.
//
// The behavioural rows live in `editor_atomic_tests.rs`. These pin the seam:

// Verifies: gh #381 gap 4 - **the render path asks `chip_span`, it does not grow a
// second notion of "where the caret is".**
//
// This is the guard the whole file exists for, one level down: the gap's own defect is
// a caret path that does not consult the predicate. A fix that computed the chip range
// from `atomic_span_at` directly, or re-implemented the bracket scan in `render`, would
// pass every behavioural row and fail here - and the next chip rule would then have two
// places to change, which is the bug class #381 is.
#[test]
fn the_render_path_asks_the_one_predicate() {
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let render = function_body(&editor, "pub fn render(&self");
    // The painting lives in `editor_atomic.rs` (split out at the ceiling), so the two
    // files are read together: the seam is "render delegates, the helper asks".
    let paint = function_body(&atomic, "pub(super) fn paint_caret_into_row(");
    assert!(
        render.contains("paint_caret_into_row("),
        "`render` delegates the caret to the one paint path: {render}"
    );
    assert!(
        paint.contains("Self::caret_chip_span("),
        "which asks `caret_chip_span`: {paint}"
    );
    // And that helper is the only place the chip geometry is derived.
    let span = function_body(&atomic, "pub(super) fn caret_chip_span(");
    assert!(
        span.contains("Self::chip_span("),
        "which asks `chip_span`, the predicate gaps 1-3 consult: {span}"
    );
    assert!(
        !span.contains("atomic_span_at"),
        "and does not re-derive it from the word predicate: {span}"
    );
    // Neither paint site may hold marker knowledge of its own.
    for (name, body) in [("render", render), ("paint_caret_into_row", paint)] {
        assert!(
            !body.contains("atomic_span_at") && !body.contains("\"[paste #\""),
            "`{name}` holds no marker knowledge of its own: {body}"
        );
    }
}

// The `27m`-not-`0m` property is guarded by #371's
// `no_caret_closes_reverse_video_with_a_full_reset`, whose scan this cycle widened to
// cover `editor_atomic.rs`. Two mechanisms for one property is how they drift, so this
// file does not add a second.

// Verifies: gh #381 gap 4 - **row clipping lives in the helper, so a split chip can
// only paint the cells drawn on its row.**
//
// Gap 1 established that a chip wider than the pane splits *visually* only
// (`editor.ts:174`). A reader that forgot to clip would paint a run longer than the
// row, which is a row wider than the pane - the renderer contract the gap-1 cycle
// already broke once.
#[test]
fn a_split_chip_can_only_paint_its_own_row() {
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let span = function_body(&atomic, "pub(super) fn caret_chip_span(");
    assert!(
        span.contains("start.max(row_start)") && span.contains("end.min(row_end)"),
        "the chip span is intersected with the row's own range: {span}"
    );
}

// Verifies: gh #381 gap 4 - **`paint_caret` gates *whether*, and the unpainted path
// still reports the caret's own column.**
//
// The brief's third design call. Gating only the *painting* is not enough: the chip
// arm also moves where the marker is emitted, so an ungated arm would put the IME
// candidate window at the chip's first cell.
#[test]
fn paint_caret_gates_the_chip_arm_itself() {
    let atomic = code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"));
    let body = function_body(&atomic, "pub(super) fn paint_caret_into_row(");
    // The arm as written: `if self.paint_caret { Self::caret_chip_span(...) } else {
    // None }`. Asserting the exact shape is right here - the property is that the
    // span is not computed at all when nothing is painted, because the arm also
    // decides where the marker lands.
    let at = body
        .find("let chip = if self.paint_caret {")
        .unwrap_or_else(|| panic!("the chip span is gated on `paint_caret`: {body}"));
    let arm = &body[at..];
    let else_at = arm
        .find("} else {")
        .unwrap_or_else(|| panic!("the gate has an else arm: {arm}"));
    assert!(
        arm[..else_at].contains("Self::caret_chip_span("),
        "the gated branch computes the chip span: {}",
        &arm[..else_at]
    );
    let tail = &arm[else_at..];
    let end = tail.find("};").expect("the arm ends");
    // Whitespace-insensitive: the property is the *value*, not the formatting.
    let tokens: Vec<&str> = tail[..end]
        .split_whitespace()
        .filter(|t| *t != "}")
        .collect();
    assert_eq!(
        tokens,
        vec!["else", "{", "None"],
        "the ungated branch is `None`, so the marker stays at the caret's own column \
         instead of jumping to the chip's first cell"
    );
}

// ---------------------------------------------------------------------------
// gap 5: the three seams where a chip's text leaves and re-enters the editor
// ---------------------------------------------------------------------------
//
// A chip's text is a **label** and the registry is the **truth**. Gaps 1-4 held that
// for motion, deletion, undo and paint; these three are the boundaries, and each either
// preserves both or is explicit about which one it transforms.
//
// The behavioural rows live in `editor_seams_tests.rs`. These pin the seams:

/// The editor's and the chip module's source, comment-free, joined - the paint and
/// registry code was split out of `editor.rs` at the ceiling, so a guard that read only
/// one file would silently stop covering half the seam.
fn editor_and_chip_sources() -> String {
    format!(
        "{}\n{}",
        code_only(&read("crates/lca-tui/src/widgets/editor.rs")),
        code_only(&read("crates/lca-tui/src/widgets/editor_atomic.rs"))
    )
}

// Verifies: gh #381 gap 5 seam 1 - **the paste branch runs before any key dispatch.**
//
// This is the property, not a spelling: `handle_key` asks `bracketed_paste_content`
// before it consults a single binding. A fix that filtered the `\r` inside a paste
// somewhere downstream would pass a behavioural row and leave the next control byte in
// a paste free to fire; ordering is what makes "a paste is a unit" true for *every*
// byte it carries.
#[test]
fn the_paste_branch_precedes_every_key_binding() {
    let source = editor_and_chip_sources();
    let body = function_body(&source, "pub fn handle_key(&mut self");
    let paste_at = body
        .find("bracketed_paste_content(")
        .unwrap_or_else(|| panic!("handle_key asks the paste primitive: {body}"));
    let first_binding = body
        .find("kb.matches(")
        .unwrap_or_else(|| panic!("handle_key consults a binding too: {body}"));
    assert!(
        paste_at < first_binding,
        "the paste branch is asked before any keybinding is matched, so no byte inside \
         a paste can reach a binding: paste at {paste_at}, first binding at \
         {first_binding}"
    );
    assert!(
        body[paste_at..].contains("insert_paste("),
        "and the paste is inserted as one unit rather than replayed as keys: {body}"
    );
}

// Verifies: gh #381 gap 5 seam 2 - **history stores the marker form and its registry,
// never the expanded text.**
//
// The defect was one line: `history.push(expanded)`. Pushing the expansion makes recall
// rebuild the editor at payload height, which is the harm the chip exists to prevent. A
// guard on the *entry type* is what stops that line coming back, because the two forms
// are distinguishable there and nowhere else.
#[test]
fn history_entries_carry_the_marker_form_and_its_registry() {
    let source = editor_and_chip_sources();
    let submit = function_body(&source, "pub fn submit(&mut self");
    assert!(
        submit.contains("HistoryEntry {"),
        "submit pushes a structured entry, not a bare String: {submit}"
    );
    assert!(
        submit.contains("text: self.text()"),
        "and the entry's text is the **buffer** (markers intact), not the expansion \
         held in the local `text`: {submit}"
    );
    assert!(
        submit.contains("pastes: self.pastes.clone()"),
        "with the registry beside it, because the text alone cannot reconstruct the \
         payloads: {submit}"
    );
    // The entry stores exactly two fields and no expansion: the expanded form is
    // derived by whoever needs it, so a recall cannot accidentally be given a second
    // copy that drifts from the first.
    let decl = source
        .find("pub(super) struct HistoryEntry {")
        .unwrap_or_else(|| panic!("the entry type is declared: {source}"));
    let body_end = source[decl..]
        .find('}')
        .unwrap_or_else(|| panic!("the entry type is closed"));
    let fields = &source[decl..decl + body_end];
    assert!(
        fields.contains("text: String") && fields.contains("pastes: Vec<String>"),
        "the entry holds the marker text and its registry: {fields}"
    );
    assert!(
        !fields.contains("expanded"),
        "and no third, derivable field: {fields}"
    );
}

// Verifies: gh #381 gap 5 seam 2 - **recall restores both halves through the pair gap 3
// built**, rather than a `set_text` that would leave dead markers.
//
// `set_text_with_pastes` exists precisely for a caller that holds a saved (text,
// registry) pair. A recall that used plain `set_text` would restore the marker and lose
// the payload - the marker would expand to nothing, which is gap 3's harm at a new seam.
#[test]
fn recall_restores_the_registry_with_the_text() {
    let source = editor_and_chip_sources();
    // Both directions: browsing back and browsing forward.
    let occurrences = source.matches("set_text_with_pastes(").count();
    assert!(
        occurrences >= 3,
        "the pair is used by recall in both directions (and defined once), so a \
         recalled chip keeps its payload: {occurrences} use(s)"
    );
    // And the history recall arms are the ones that use it.
    for signature in ["pub fn cursor_up(&mut self", "pub fn cursor_down(&mut self"] {
        let body = function_body(&source, signature);
        assert!(
            body.contains("set_text_with_pastes("),
            "{signature} restores the registry with the text: {body}"
        );
        assert!(
            !body.contains("set_text(&self.history"),
            "{signature} does not fall back to the registry-less restore: {body}"
        );
    }
}

// Verifies: gh #381 gap 5 seam 3 - **the `$EDITOR` handoff expands on the way out and
// lets `set_text` re-derive on the way back.**
//
// The asymmetry is the design: an external editor cannot know our registry, so it gets
// content; the buffer gets its chips back only if the returned text still carries the
// markers, which `set_text`'s `retain_live_payloads` decides. **A returned buffer
// without markers therefore drops the payloads** rather than shipping bytes the user
// edited away - this issue's original harm, one seam out.
#[test]
fn the_editor_handoff_expands_out_and_rederives_back() {
    let run = code_only(&read("crates/lca-ui/src/run.rs"));
    let arm = run
        .find("Action::ExternalEditor =>")
        .unwrap_or_else(|| panic!("the handoff arm exists: {run}"));
    let body = &run[arm..];
    let end = body.find("Action::InteractiveShell").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("expanded_text()"),
        "the editor is handed the expanded text, never a marker: {body}"
    );
    assert!(
        body.contains("set_text(&edited)"),
        "and the return goes through `set_text`, whose re-derivation is what drops a \
         payload whose marker the edit removed: {body}"
    );
    // The re-derivation really is on that path (gap 3 put it there).
    let editor = code_only(&read("crates/lca-tui/src/widgets/editor.rs"));
    let set_text = function_body(&editor, "pub fn set_text(&mut self");
    assert!(
        set_text.contains("retain_live_payloads("),
        "`set_text` re-derives the registry from the new text: {set_text}"
    );
}
