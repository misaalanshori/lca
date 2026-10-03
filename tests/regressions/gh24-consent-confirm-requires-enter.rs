//! GitHub issue #24 (released 0.5.3): `Allow these capabilities? [y/N]`
//! confirmed on the first `y` keystroke. The prompt went raw-mode and broke
//! `true` on a single key event, so a user who typed `y` and meant to keep
//! reading the capability list had already granted it - the reported
//! transcript shows `... [y/N] y` followed by `installed antigravity`
//! with no Enter in between. `ext update`'s capability-widening prompt
//! shares the same function, so both consent surfaces shipped the same way.
//!
//! Consent must be a **line**: the platform's own line discipline echoes the
//! keystrokes and requires Enter, and the answer is what arrives after that
//! newline. A line with no terminator means end of input, not agreement -
//! that is what lets a pipe confirm nothing by accident, and it is the row
//! that proves the fix.
//!
//! Answer semantics: trimmed, case-insensitive `y`/`yes` confirms; `n`/`no`
//! and the empty line decline; end of input declines (the pre-existing
//! "nothing on stdin declines" rule); anything else is asked again.
//!
//! A line is ended by `\n` or `\r`: a console a previous process left in
//! raw mode has no line discipline, and its Enter arrives as `\r` alone -
//! waiting for `\n` there would hang the one prompt that must not hang.
//!
//! Both callers (`ext install`'s grant consent and `ext update`'s
//! capability widening) pass through the same `confirm`, so one seam
//! covers them - asserted once, deliberately.
//!
//! Verifies: NFR-24 (a released defect's guard), GitHub issue #24.

use lca_cli::ext::confirm_with;

const PROMPT: &str = "Allow these capabilities? [y/N] ";

/// Feed `script` to the consent reader and report the answer plus exactly
/// what the prompt wrote, so a row can assert on re-prompting too.
fn ask(script: &str) -> (bool, String) {
    let mut input: &[u8] = script.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let answer = confirm_with(PROMPT, &mut input, &mut out);
    (answer, String::from_utf8_lossy(&out).into_owned())
}

// Verifies: gh #24's core row - a `y` at end of input is keystrokes the
// user never submitted, so it declines. Pre-fix this confirmed.
#[test]
fn a_bare_y_with_no_enter_declines() {
    let (answer, _) = ask("y");
    assert!(!answer, "a partial line must not grant capabilities");
}

// Verifies: gh #24 (unchanged row) - `y` followed by Enter confirms, the
// transcript the issue asks for.
#[test]
fn y_followed_by_enter_confirms() {
    let (answer, out) = ask("y\n");
    assert!(answer, "{out:?}");
}

// Verifies: gh #24 (unchanged row) - the answer is case-insensitive and
// accepts the word as well as the letter.
#[test]
fn capital_y_and_yes_confirm() {
    assert!(ask("Y\n").0, "capital Y");
    assert!(ask("yes\n").0, "yes");
}

// Verifies: gh #24 (unchanged rows) - `n`, the empty line and end of
// input all decline, so an unattended run never writes without consent.
#[test]
fn no_empty_and_end_of_input_all_decline() {
    assert!(!ask("n\n").0, "n");
    assert!(!ask("\n").0, "a bare Enter");
    assert!(!ask("").0, "end of input");
}

// Verifies: gh #24 (the re-prompt row) - an unrecognized answer is asked
// again rather than treated as agreement, and the prompt is what repeats.
#[test]
fn an_unrecognized_answer_is_asked_again_before_the_next_one_counts() {
    let (answer, out) = ask("maybe\ny\n");
    assert!(answer, "{out:?}");
    assert_eq!(
        out.matches(PROMPT).count(),
        2,
        "the prompt repeats once per line read: {out:?}"
    );
}

// Verifies: gh #24 fix-up - a console a crashed program left in raw mode
// has no line discipline, so Enter arrives as a bare `\r` and `read_line`
// would wait for a `\n` that never comes, with Ctrl+C delivered as a
// literal byte and no way out. `\r` ends a line too.
#[test]
fn a_raw_console_enter_confirms() {
    let (answer, out) = ask("y\r");
    assert!(answer, "{out:?}");
}

// Verifies: gh #24 fix-up - the answer is read without ever looking past
// the terminator that ended it. A peek after `\r` calls `fill_buf` on an
// empty buffer, which on a real console is a `read` blocking forever for
// a keystroke that never comes - the prompt hangs, and raw mode has
// already swallowed Ctrl+C, so there is no way out. Found by driving the
// prompt in a pane under `stty raw`; no slice-backed reader can show it.
#[test]
fn a_cr_terminated_answer_is_never_read_past() {
    let mut input = CrLine { data: b"y\r" };
    let mut out: Vec<u8> = Vec::new();
    assert!(
        confirm_with(PROMPT, &mut input, &mut out),
        "the raw console's Enter: {:?}",
        String::from_utf8_lossy(&out)
    );
}

/// A reader carrying one CR-terminated line that refuses to be probed past
/// its last byte: `fill_buf` on an exhausted buffer is a read-ahead, and a
/// slice-backed reader has no way to say so.
struct CrLine {
    data: &'static [u8],
}

impl std::io::Read for CrLine {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        // `fill_buf` below is the only read this seam should ever reach;
        // arriving here means the answer was not read line-wise.
        Ok(0)
    }
}

impl std::io::BufRead for CrLine {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        assert!(
            !self.data.is_empty(),
            "read past the line's terminator: on a terminal this blocks forever"
        );
        Ok(self.data)
    }

    fn consume(&mut self, amount: usize) {
        self.data = &self.data[amount.min(self.data.len())..];
    }
}

// Verifies: gh #24 fix-up - a Windows canonical console sends Enter as
// `\r\n`, which is ONE terminator: the pair answers once and the prompt
// is printed once.
#[test]
fn a_crlf_enter_is_one_terminator_and_answers_once() {
    let (answer, out) = ask("y\r\n");
    assert!(answer, "{out:?}");
    assert_eq!(out.matches(PROMPT).count(), 1, "{out:?}");
}

// Verifies: gh #24 fix-up - the same pairing behind a line that re-prompts:
// an unpaired `\n` would be read as a second, empty line and the exchange
// would end in a decline instead of the answer that follows it.
#[test]
fn a_crlf_pair_never_reads_as_a_second_empty_line() {
    let (answer, out) = ask("maybe\r\ny\n");
    assert!(answer, "{out:?}");
    assert_eq!(out.matches(PROMPT).count(), 2, "{out:?}");
}
