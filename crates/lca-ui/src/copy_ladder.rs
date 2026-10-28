//! The copy ladder (R6, gh #9, gh #406): a verified native write first, then
//! OSC 52, then an honest failure.
//!
//! Split from `run.rs` at gate 11's 1,200-line ceiling; behaviour unchanged. It
//! is its own module because the ladder is the one place that decides what the
//! user is told about a copy, and "did it work?" is a question worth reading in
//! isolation from the render loop around it.

/// Which copy step ran (R6, kept ours for gh #9): the host's verified
/// native clipboard first, then OSC 52 when this screen can send it.
/// [`CopyReport::notice`] says exactly which step ran, never claiming
/// a step that did not - except a caller that knows its target may
/// name the success itself, and only on [`CopyReport::Native`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CopyReport {
    /// A native clipboard tool confirmed the write.
    Native,
    /// OSC 52 went out unverified (testing plan §14).
    Osc52,
    /// No step could run.
    Failed,
}

impl CopyReport {
    /// Whether this report is a success, a caveat, or a failure, for the caller
    /// that has to pick a dwell time (gh #374 gap 4).
    ///
    /// Only [`CopyReport::Native`] is a success. [`CopyReport::Osc52`] is **not**:
    /// the write went out and the terminal may have ignored it, so treating it as
    /// a success is the exact lie this gap fixes.
    pub(super) fn is_error(&self) -> bool {
        !matches!(self, CopyReport::Native)
    }

    /// What the interface shows (gh #374 gap 4).
    ///
    /// Three distinguishable forms, and the wording is load-bearing:
    ///
    /// - [`CopyReport::Native`] - `✓ copied to clipboard`. A verified write, so
    ///   the tick is earned and the sentence is short.
    /// - [`CopyReport::Osc52`] - `? copy sent as OSC 52 - the terminal may have
    ///   ignored it`. **Not** a success claim: the tick is withheld and the
    ///   caveat is stated. This used to read "copied (if your terminal blocked the
    ///   clipboard, nothing was copied)", which opens with "copied" - a skimming
    ///   reader was told it worked when the terminal may have done nothing. pi's
    ///   bare-OSC-52 path flashes `Copied!` (`tui-alt-screen.ts:1484`), which is
    ///   that same lie; LCA does not copy it.
    /// - [`CopyReport::Failed`] - `✗ could not copy: no clipboard here (try
    ///   /fullscreen)`. Nothing happened, and the row names the action that can
    ///   fix it (OSC 52 needs the alt screen).
    ///
    /// Each form is distinct even with every non-ASCII symbol stripped, so a
    /// reader with no glyphs still gets three different sentences (NFR-28).
    pub(super) fn notice(&self) -> String {
        match self {
            CopyReport::Native => "\u{2713} copied to clipboard".to_string(),
            CopyReport::Osc52 => {
                "? copy sent as OSC 52 - the terminal may have ignored it".to_string()
            }
            CopyReport::Failed => {
                // Gh #406: the wording named a *missing tool*, which was the old
                // failure mode (none of `pbcopy`/`xclip`/... installed). The write
                // is in-process now, so failing means the platform would not take
                // it - no display (SSH, a container) or a stuck selection owner -
                // and the fix is still to use the alt screen, where OSC 52 can
                // reach the terminal.
                "\u{2717} could not copy: no clipboard here (try /fullscreen)".to_string()
            }
        }
    }
}

/// Run the copy ladder over `text`, returning which step ran; the
/// caller turns the report into the notice it shows.
pub(super) fn copy_through_ladder(
    text: &str,
    native: Option<&crate::state::ClipboardWriter>,
    osc52: Option<&mut dyn FnMut(&str)>,
) -> CopyReport {
    if native.is_some_and(|write| write(text)) {
        return CopyReport::Native;
    }
    match osc52 {
        Some(send) => {
            send(text);
            CopyReport::Osc52
        }
        None => CopyReport::Failed,
    }
}

/// Drive the copy ladder from outside this module, reporting the step as text
/// (`"native"`, `"osc52"`, `"failed"`).
///
/// The ladder itself is private because it takes a private `CopyReport`; this is
/// the seam a regression guard needs to prove the property gh #406 depends on -
/// that a **failed native write falls through to OSC 52** rather than stopping.
/// That property became load-bearing when the writer moved from a spawned helper
/// (where "failed" meant "no program found") to `arboard` (where it means "no
/// display"), and a guard cannot see the difference through the notice text.
pub fn route_a_copy(
    text: &str,
    native: Option<&crate::state::ClipboardWriter>,
    osc52: Option<&mut dyn FnMut(&str)>,
) -> &'static str {
    match copy_through_ladder(text, native, osc52) {
        CopyReport::Native => "native",
        CopyReport::Osc52 => "osc52",
        CopyReport::Failed => "failed",
    }
}
