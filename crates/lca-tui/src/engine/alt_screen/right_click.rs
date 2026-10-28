//! The right-button paste gate (gh #374 gap 4).
//!
//! A separate module for the 1,200-line ceiling, and because the policy is the
//! one piece of this feature that is a *decision* rather than plumbing: the
//! renderer only records the intent, and this says when the intent is allowed.
//!
//! The two accessors live here rather than in `alt_screen.rs` so the intent and
//! the policy that permits it are one file.
//!
//! pi's gate, as a pure function of its two inputs (`tui-alt-screen.ts:1018`):
///
/// ```text
/// process.platform !== "win32" || TERM_PROGRAM?.toLowerCase() === "vscode"
/// ```
///
/// **Windows only**, because right-click-to-paste is a Windows terminal
/// convention rather than a general one - on macOS and Linux the button has other
/// meanings and a paste would be a surprise.
///
/// **Never under VS Code**, because its integrated terminal has its own
/// right-click behaviour; shadowing it would break the editor the terminal is
/// running inside. The comparison is case-insensitive, as pi's is, so the
/// environment's spelling cannot bypass the guard.
///
/// Taking `is_windows` as a parameter rather than reading `cfg!(windows)` is what
/// lets a row exercise every combination on any platform - the alternative is a
/// guard nothing can test until someone runs it on Windows.
pub fn right_click_paste_allowed(is_windows: bool, term_program: Option<&str>) -> bool {
    if !is_windows {
        return false;
    }
    match term_program {
        Some(program) => !program.eq_ignore_ascii_case("vscode"),
        None => true,
    }
}

use super::AltScreenRenderer;

impl AltScreenRenderer {
    /// Record a right-button press as a paste request (gh #374 gap 4).
    ///
    /// Returns `None` when this is not a right press, so `handle_mouse` can fall
    /// through to the selection arm; `Some(consumed)` when it is, where `consumed`
    /// is `false` for a right press the policy forbids (an embedder that handles
    /// right-click itself still sees the event).
    ///
    /// Not a paste when a selection drag is in flight: a gesture owns the pointer
    /// until it ends, which is the same rule the scrollbar drag follows.
    pub(super) fn right_click_press(&mut self, button: u16) -> Option<bool> {
        if button != 2 {
            return None;
        }
        if self.selection.press_active {
            return Some(false);
        }
        if !self.right_click.1 {
            return Some(false);
        }
        self.right_click.0 = true;
        Some(true)
    }

    /// Set whether a right-button press may paste (gh #374 gap 4).
    ///
    /// The host passes [`right_click_paste_allowed`] with this machine's answer;
    /// the renderer keeps no opinion of its own, so the platform policy has one
    /// home and can be driven both ways in a row.
    pub fn set_right_click_paste_allowed(&mut self, allowed: bool) {
        self.right_click.1 = allowed;
    }

    /// Take a right-button paste request (gh #374 gap 4).
    ///
    /// The host reads the clipboard through the gh #406 path and reports honestly
    /// when the read found nothing - the engine never owns a clipboard.
    pub fn take_right_click_paste(&mut self) -> bool {
        std::mem::take(&mut self.right_click.0)
    }
}
