//! Gh #383 gaps 2-4: the kill ring's forward half (`Ctrl+K`/`Ctrl+U` at a line
//! boundary, `Alt+D`, `Alt+Y`), jump mode's cancel and search order, and the space's
//! undo unit.
//!
//! Split out of `editor_tests.rs` at gate 11's ceiling. The module is declared from
//! the file that owns the subject (`editor_tests.rs`), so the declared-from path never
//! moves and the rows report as `widgets::editor::tests::kill_tests::<row>` (gh #574).

use super::*;

// ---------------------------------------------------------------------------
// Gh #383 gaps 2, 3 and 4 — the kill ring's forward half, the jump mode, and
// the space's undo unit.
// ---------------------------------------------------------------------------

// Verifies: gh #383 gap 4 — **a space captures the state before itself and coalesces
// the word after it**, so `"hello world"` is two undo units rather than three.
//
// The pre-fix shape snapshotted before the space *and* again before `w`, which is
// why three undos walked `"hello world"` → `"hello "` → `"hello"` → `""`.
// `typing_coalesces_into_word_sized_undo_units` carries the sanctioned change; this
// row pins the other half — **each space is separately undoable**, which is the trap
// the issue names: coalescing after every space would merge `"a  b"` into one unit.
#[test]
fn every_space_is_a_separate_undo_unit() {
    let mut e = Editor::new();
    for c in "a  b".chars() {
        e.handle_key(&c.to_string());
    }
    assert_eq!(e.text(), "a  b");
    e.undo();
    assert_eq!(e.text(), "a ", "the second space carries the word after it");
    e.undo();
    assert_eq!(e.text(), "a", "the first space is its own unit");
    e.undo();
    assert_eq!(e.text(), "", "and the first word is the last unit");
}

// Verifies: gh #383 gap 4 - a **non-space** typed character still coalesces, so the
// fix to the space rule cannot regress pi's fish-style word runs.
#[test]
fn a_non_space_character_still_coalesces_with_the_word() {
    let mut e = Editor::new();
    for c in "hello".chars() {
        e.handle_key(&c.to_string());
    }
    assert_eq!(e.text(), "hello");
    e.undo();
    assert_eq!(e.text(), "", "the five letters are one undo unit");
}

// Verifies: gh #383 gap 2 - **`Ctrl+K` at end-of-line joins the next line down** and
// kills the newline, instead of doing nothing.
//
// Pre-fix `delete_to_line_end` killed `current()[byte..]`, which is the empty string
// at EOL: the caret was already at the end, so `Ctrl+K` was a no-op. pi treats the
// newline as the killed text (`deleteToEndOfLine`, `editor.ts:1690`).
#[test]
fn ctrl_k_at_end_of_line_joins_the_next_line_down() {
    let mut e = Editor::new();
    e.set_text("first\nsecond");
    // `set_text` leaves the caret at the end of the *last* line, so the fixture says
    // which line it means rather than relying on where the setter put it.
    e.cursor_line = 0;
    e.cursor_col = 5;
    assert_eq!(
        (e.cursor_line, e.cursor_col),
        (0, 5),
        "caret at the first EOL"
    );
    e.delete_to_line_end();
    assert_eq!(e.text(), "firstsecond", "the newline was killed");
    assert_eq!(e.kill_ring.last().map(String::as_str), Some("\n"));
}

// Verifies: gh #383 gap 2 - **`Ctrl+U` at column 0 joins the previous line up** and
// kills the newline. Same boundary on the other side: the killed slice was `""`.
#[test]
fn ctrl_u_at_column_zero_joins_the_previous_line_up() {
    let mut e = Editor::new();
    e.set_text("first\nsecond");
    e.cursor_line = 1;
    e.cursor_col = 0;
    assert_eq!(e.cursor_line, 1, "caret on the second line");
    e.delete_to_line_start();
    assert_eq!(e.text(), "firstsecond", "the newline was killed");
    assert_eq!(
        e.cursor_col, 5,
        "and the caret sits where the join happened"
    );
    assert_eq!(e.kill_ring.last().map(String::as_str), Some("\n"));
}

// Verifies: gh #383 gap 2 - **`Alt+D` now deletes forward**, which is the binding
// that was declared and matched by nothing. The span is exactly what `Alt+Right`
// crosses, so motion and deletion agree about the segment boundary.
#[test]
fn alt_d_deletes_the_word_forward_by_segment() {
    let mut e = Editor::new();
    e.insert_str("foo.bar(baz_qux)");
    e.cursor_line_start();
    let event = e.handle_key("\x1bd"); // Alt+D
    assert!(matches!(event, EditorEvent::Changed), "the key is consumed");
    assert_eq!(
        e.text(),
        ".bar(baz_qux)",
        "the contiguous word run is gone and punctuation stays"
    );
    assert_eq!(e.cursor_col, 0, "the caret does not move");
    assert_eq!(e.kill_ring.last().map(String::as_str), Some("foo"));
    // Twice more: the `.` is its own run, then `bar`.
    e.handle_key("\x1bd");
    assert_eq!(e.text(), "bar(baz_qux)");
    e.handle_key("\x1bd");
    assert_eq!(e.text(), "(baz_qux)");
}

// Verifies: gh #383 gap 2 - **`Alt+Y` pops the kill ring** after a yank: the block the
// yank inserted is replaced by the older entry, which was the second dead binding.
#[test]
fn alt_y_after_a_yank_replaces_the_yanked_block() {
    let mut e = Editor::new();
    e.insert_str("one two");
    e.cursor_line_end();
    e.delete_word_backward(); // kills "two "
    e.delete_word_backward(); // prepends "one " → one entry "one two"
    // **Typing ends the kill run**, which is what makes the next kill a *new* ring
    // entry rather than an append - the accumulation rule the issue's gap 2 keeps.
    // Type through `handle_key` so the kill run **ends**: the accumulation rule
    // reads `kill_run`, which only `handle_key` re-arms, so a typed character is what
    // separates two ring entries (and it is what a user actually does).
    for c in "FIRST ".chars() {
        e.handle_key(&c.to_string());
    }
    e.handle_key("\x17"); // Ctrl+W kills "FIRST " as a new newest entry
    assert_eq!(
        e.kill_ring.len(),
        2,
        "two entries: {e_kill:?}",
        e_kill = e.kill_ring
    );
    e.yank(); // inserts "FIRST "
    assert_eq!(e.text(), "FIRST ");
    e.handle_key("\x1by"); // Alt+Y
    assert_eq!(
        e.text(),
        "one two",
        "the yanked block was taken back out and the older entry inserted"
    );
    assert_eq!(e.cursor_col, 7, "caret at the end of the replacement");
}

// Verifies: gh #383 gap 2 - **`Alt+Y` only pops immediately after a yank.** pi's
// guard is `lastAction !== "yank" || killRing.length <= 1`, so a pop that fired on
// any keystroke would eat text the user had just typed.
//
// The fixture is the interesting half: a yank **did** happen, and then a typed
// character intervened - which is the state that matters. With no yank at all the row
// would pass for the trivial reason (there is nothing to pop), so this row drives the
// real guard: a yank through the key path, then a letter, then Alt+Y.
#[test]
fn alt_y_outside_a_yank_is_inert() {
    let mut e = Editor::new();
    e.insert_str("one two");
    e.cursor_line_end();
    e.delete_word_backward(); // ring: ["two "]
    e.delete_word_backward(); // ring: ["one two"] (accumulated)
    e.handle_key("\x19"); // Ctrl+Y — a real yank through the key path
    assert_eq!(e.text(), "one two", "the yank inserted the block");
    e.handle_key("X"); // any other key ends the yank run
    let before = e.text();
    e.handle_key("\x1by"); // Alt+Y — no longer immediately after a yank
    assert_eq!(e.text(), before, "no pop once another key intervened");
}

// Verifies: gh #383 gap 3 - **re-pressing the jump key cancels the pending jump**,
// instead of leaving the editor armed for the next character.
//
// The mechanism is the **jump-mode arm**, not a second cancel branch: once
// `jump_pending` is set, the next key is consumed there, and a non-printable key (the
// hotkey itself, or any control key) leaves the mode abandoned with nothing inserted.
// That is why there is only one place the mode is cleared - a second "if already
// pending, cancel" arm in the arming block is unreachable, which a mutant confirmed
// (removing it left this row green). The row asserts the *observable*: after the
// second press the next letter is text.
//
// The pre-fix shape did **not** cancel because the arming arms ran only when the mode
// was not pending... they ran regardless, and re-assigned the direction, so the mode
// stayed armed and the letter vanished into a search - the behaviour this row pins.
#[test]
fn a_second_jump_press_cancels_the_pending_jump() {
    let mut e = Editor::new();
    e.insert_str("abcabc");
    e.cursor_col = 0;
    e.handle_key("\x1d"); // Ctrl+] — jump forward
    assert!(e.jump_pending.is_some(), "armed");
    e.handle_key("\x1d");
    assert!(e.jump_pending.is_none(), "the second press cancels");
    // And typing now inserts instead of jumping.
    e.handle_key("x");
    assert_eq!(e.text(), "xabcabc", "the letter is text, not a target");
}

// Verifies: gh #383 gap 3 - **the jump search answers from the caret's own line
// first**, and the whole-buffer `Vec` it used to build was the reason a large session
// allocated one per keystroke. Asserted as the *landing position*, which is the
// observable the ordering produces.
#[test]
fn a_jump_finds_the_next_target_on_the_same_line_first() {
    let mut e = Editor::new();
    e.set_text("axb\nxb");
    // `set_text` leaves the caret at the end of the last line; the fixture names the
    // line it means (`set_text`'s own docs say so).
    e.cursor_line = 0;
    e.cursor_col = 0;
    e.handle_key("\x1d"); // Ctrl+]
    e.handle_key("b");
    assert_eq!(
        (e.cursor_line, e.cursor_col),
        (0, 2),
        "the `b` on line 0, not the earlier one on line 1"
    );
    // Backward from line 1 finds its own line's `x` (column 0 of `xb`) before line
    // 0's, which is the ordering the current-line-first rule produces.
    e.cursor_line = 1;
    e.cursor_col = 2;
    // `Ctrl+Alt+]` is the **backward** jump (`tui.editor.jumpBackward`); plain
    // `Ctrl+]` is forward, which is why the first two presses above used `\x1d`.
    e.handle_key("\x1b\x1d");
    e.handle_key("x");
    assert_eq!(
        (e.cursor_line, e.cursor_col),
        (1, 0),
        "same line, backwards"
    );
}
