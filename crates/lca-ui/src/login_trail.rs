//! Rendering a multi-step login as a transcript (gh #398 gap 2).
//!
//! A login flow asks for a URL to visit, then a code to paste, then maybe a
//! workspace - and each step used to **clear** the one before it. By the time a
//! login failed, the user had lost the URL they were told to visit and the code
//! they were told to paste, which is precisely the information needed to work
//! out which step went wrong.
//!
//! pi keeps the trail: `showPrompt` **appends** to the dialog's content, and
//! `replaceInputWithSubmittedText` swaps the answered input for a static
//! `> value` line. This module is that shape for LCA's modal.
//!
//! Three rules, in the order they matter:
//!
//! 1. **A completed step becomes history, not a replacement** - so the modal
//!    reads like a transcript of the flow.
//! 2. **A submitted value is shown as input** (`> value`), so the user can tell
//!    what they typed from what the provider said.
//! 3. **The newest prompt stays visible.** Appending history that pushes the
//!    live prompt below the fold would trade a lost trail for a hidden input,
//!    so the trail is windowed from its *newest* end and the current step is
//!    never trimmed.
//!
//! The masking rule is **not** here: it lives in
//! [`crate::login_step::promote_pending_step`], which is the only place a step
//! becomes history. A secret is dropped there, so there is nothing for this
//! module to redact - and no future renderer change can leak a value that was
//! never stored.

use crate::state::LoginHistoryEntry;
use crate::theme::{Role, Theme};
use lca_tui::engine::core::{OverlayOptions, SizeValue, resolve_overlay_layout};
use lca_tui::engine::text::{linkify_urls, wrap_text_with_ansi};

/// The finished steps as modal rows, oldest first.
///
/// A [`LoginHistoryEntry::Message`] is provider prose and keeps the muted role it
/// had when it was live. A [`LoginHistoryEntry::Answer`] is the user's own input,
/// so it carries the `> ` marker (pi's `> ${value}`) and the text role - the two
/// are deliberately different so the flow reads as call and response rather
/// than as one wall of text.
pub fn trail_rows(trail: &[LoginHistoryEntry], theme: &Theme) -> Vec<String> {
    let marker = theme.role(Role::Accent);
    let value = theme.role(Role::Text);
    let prose = theme.role(Role::Muted);
    let mut rows = Vec::new();
    for entry in trail {
        match entry {
            LoginHistoryEntry::Message(text) => {
                rows.extend(text.lines().map(|line| prose(line).to_string()));
            }
            LoginHistoryEntry::Answer { value: typed, .. } => {
                // The marker is the whole point: a reader must be able to tell
                // what they typed from what the provider said.
                rows.push(format!("{}{}", marker("> "), value(typed)));
            }
        }
    }
    rows
}

/// The modal's body: the trail that fits above the current step, then the
/// current step itself.
///
/// The budget is the overlay's own - frame rows subtracted from the height the
/// layout will grant - so "does it fit" is answered by the same rule the painter
/// uses rather than a second estimate that could disagree with it. Rows are
/// measured **wrapped**, because a sign-in URL is three or four painted rows and
/// counting entries instead of lines is exactly how a trail would overflow the
/// frame while appearing to fit.
pub fn fit_body(trail: Vec<String>, current: Vec<String>, width: u16, height: u16) -> Vec<String> {
    let inner = inner_width(width, height);
    let budget = body_rows(height);
    let current_lines: usize = current.iter().map(|line| painted_lines(line, inner)).sum();
    // The current step is never trimmed: an input below the fold is a worse
    // failure than a shortened trail.
    let mut keep = trail.len();
    loop {
        // The elision note costs a row, so it is part of the budget it announces.
        let note = usize::from(keep < trail.len());
        let used: usize = current_lines
            + note
            + trail[trail.len() - keep..]
                .iter()
                .map(|line| painted_lines(line, inner))
                .sum::<usize>();
        if used <= budget || keep == 0 {
            break;
        }
        keep -= 1;
    }
    let mut body = Vec::new();
    if keep < trail.len() {
        let dropped = trail.len() - keep;
        body.push(format!(
            "… {dropped} earlier step{}",
            if dropped == 1 { "" } else { "s" }
        ));
    }
    body.extend(trail[trail.len() - keep..].iter().cloned());
    body.extend(current);
    body
}

/// Painted rows one body line takes, measured the way the painter wraps it.
fn painted_lines(line: &str, inner: usize) -> usize {
    wrap_text_with_ansi(&linkify_urls(line), inner).len().max(1)
}

/// The cells a modal body row has to paint in, for a terminal `width`: the
/// layout [`crate::render::overlay_box`] resolves, minus the frame's border and
/// padding cells.
fn inner_width(width: u16, height: u16) -> usize {
    (overlay_rect(width, height).width as usize)
        .saturating_sub(4)
        .max(1)
}

/// The rows a modal's body may occupy: the tallest height the layout will grant
/// minus the two frame rows.
///
/// Derived from [`resolve_overlay_layout`] rather than from a repeated margin
/// formula, because a second estimate of the same quantity is a second answer -
/// and the one that disagrees with the painter is discovered as an overflow, not
/// as a failing test.
fn body_rows(height: u16) -> usize {
    (overlay_rect(u16::MAX, height).height as usize).saturating_sub(2)
}

/// The rectangle the login modal resolves to. Width does not depend on the
/// content height, so one call answers both questions above.
fn overlay_rect(width: u16, height: u16) -> lca_tui::engine::core::Rect {
    let options = OverlayOptions {
        width: Some(SizeValue::Percent(80)),
        min_width: Some(24),
        max_height: Some(SizeValue::Abs(height)),
        margin: 2,
        ..Default::default()
    };
    resolve_overlay_layout(&options, width, height, u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LoginHistoryEntry;

    fn answered(prompt: &str, value: &str) -> LoginHistoryEntry {
        LoginHistoryEntry::Answer {
            prompt: prompt.to_string(),
            value: value.to_string(),
            plain: Some(value.to_string()),
        }
    }

    /// **The marker is the contract**: a submitted value is rendered as input
    /// (`> value`), so a reader can tell what they typed from what the provider
    /// said. Asserting the marker rather than only the text is the difference
    /// between pinning the shape and pinning that a string is somewhere.
    #[test]
    fn a_submitted_value_is_painted_as_input_not_as_prose() {
        let theme = Theme::plain();
        let rows = trail_rows(
            &[answered(
                "Paste the callback URL:",
                "http://localhost/?code=1",
            )],
            &theme,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "> http://localhost/?code=1");
    }

    /// Provider prose keeps its own shape: it is not marked as the user's input.
    #[test]
    fn provider_prose_carries_no_input_marker() {
        let theme = Theme::plain();
        let rows = trail_rows(&[LoginHistoryEntry::Message("waiting…".into())], &theme);
        assert_eq!(rows, vec!["waiting…".to_string()]);
    }

    /// A multi-line step keeps every line, so a device code under a URL is not
    /// collapsed to just the URL.
    #[test]
    fn a_multi_line_step_keeps_every_line() {
        let theme = Theme::plain();
        let rows = trail_rows(
            &[LoginHistoryEntry::Message(
                "Enter this code:\n\nABCD-1234".into(),
            )],
            &theme,
        );
        assert!(rows.contains(&"ABCD-1234".to_string()), "{rows:?}");
        assert!(rows.contains(&"Enter this code:".to_string()), "{rows:?}");
    }

    /// The live step is never trimmed, however long the trail: an input pushed
    /// below the fold is worse than a shortened trail.
    #[test]
    fn the_live_step_is_never_trimmed() {
        let trail: Vec<String> = (0..40).map(|i| format!("step {i}")).collect();
        let current = vec![
            "the live prompt".to_string(),
            String::new(),
            "> typed".to_string(),
        ];
        let body = fit_body(trail, current.clone(), 80, 12);
        assert!(
            body.ends_with(&current),
            "the live step is the tail of the body: {body:?}"
        );
        assert!(
            body.iter().any(|line| line.contains("earlier step")),
            "the elided trail is announced: {body:?}"
        );
    }

    /// Everything fits: nothing is elided and no note is emitted, because a note
    /// on a complete trail is a false claim that something is missing.
    #[test]
    fn a_trail_that_fits_is_shown_whole_with_no_note() {
        let trail = vec!["one".to_string(), "two".to_string()];
        let current = vec!["live".to_string()];
        let body = fit_body(trail, current, 80, 24);
        assert_eq!(body, vec!["one", "two", "live"]);
    }

    /// A tall sign-in URL is measured **wrapped**, not as one entry. Counting
    /// entries instead of painted rows is exactly how a trail overflows a frame
    /// while appearing to fit.
    #[test]
    fn a_wrapping_url_is_measured_as_the_rows_it_paints() {
        let url = format!("https://example.test/auth?client_id={}", "7".repeat(300));
        let trail = vec![url];
        let current = vec!["live".to_string()];
        // A height whose body budget is smaller than the URL's wrapped height.
        let body = fit_body(trail, current, 80, 10);
        assert!(
            body.iter().any(|line| line.contains("earlier step")),
            "the one-entry trail was still elided, so it was measured wrapped: {body:?}"
        );
    }
}
