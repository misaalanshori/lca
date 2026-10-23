//! Applying the CLI's login steps and the label cleanup that goes with them
//! (ceiling split from `state.rs`, gate 11). Behaviour unchanged.
//!
//! `apply_login_next` is the one place a login step becomes interface state, so
//! it is also the one place that captures the step's sign-in URL for the copy
//! path (gh #398 gap 1). Keeping it beside `state.rs` inside one module keeps the
//! "one step owns the screen" rule readable as a unit.

use super::state::{
    GrantPrompt, LoginNext, PickerPrompt, SecretPrompt, SwitchPrompt, UiState, sanitize_block,
};

/// Apply the CLI's next login step: one modal at a time, and a
/// [`LoginNext::Secret`] mid-flow is the next field of a multi-field
/// login, not a refusal. Returns a message for history when the step
/// says something (gh #239: the dock catch-all is gone, so login
/// progress lands in the transcript).
pub fn apply_login_next(state: &mut UiState, next: LoginNext) -> Option<String> {
    // One step owns the screen at a time: entering one clears the others.
    // `Waiting` is the exception that must not clear a manual-callback
    // secret field, so it only sets the waiting label.
    match next {
        LoginNext::Message(text) => {
            state.login_waiting = None;
            Some(sanitize_block(&text))
        }
        LoginNext::Secret {
            provider,
            label,
            masked,
        } => {
            state.login_waiting = None;
            state.secret = Some(SecretPrompt {
                provider,
                label,
                input: String::new(),
                masked,
            });
            None
        }
        LoginNext::Picker { options } => {
            state.login_waiting = None;
            if options.is_empty() {
                Some("nothing to sign in to".to_string())
            } else {
                state.picker = Some(PickerPrompt {
                    options,
                    selected: 0,
                });
                None
            }
        }
        LoginNext::ConfirmSwitch { provider, prompt } => {
            state.login_waiting = None;
            state.switch_confirm = Some(SwitchPrompt { provider, prompt });
            None
        }
        LoginNext::Grant {
            provider,
            host,
            prompt,
        } => {
            state.login_waiting = None;
            state.grant = Some(GrantPrompt {
                provider,
                host,
                prompt,
            });
            None
        }
        LoginNext::Waiting { label } => {
            // Gh #398 gap 1: the step's URL is the copy target, so it is
            // captured once here rather than re-derived from the rendered
            // rows (which are wrapped, padded, and bordered). A step with no
            // URL clears it, so the previous step's link cannot linger as a
            // copy target the user is no longer looking at.
            let mut label = sanitize_block(&label);
            state.auth_url = crate::chat::Chat::http_url_of(&label);
            // The CLI sends the URL pre-wrapped in `OSC 8` (gh #178, so a
            // wrapped row clicks whole). `sanitize_block` escapes those bytes,
            // so what reached the modal was the *literal text* `\x1b]8;;` around
            // the URL - visible garbage, and worse, a non-whitespace character
            // before `http` that stops `linkify_urls` from linking it at all.
            // The renderer draws the link itself from the bare URL (and gets the
            // re-open-on-every-wrapped-row behaviour right because it wraps the
            // live sequence), so the escaped wrapper is dropped here.
            label = strip_escaped_osc8(&label);
            // A new step has not been copied yet.
            state.url_copied_at = None;
            state.login_waiting = Some(label);
            None
        }
    }
}
/// Remove literal `\x1b]8;;<url>\x07` wrapper text a caller put around a URL.
///
/// `sanitize_block` escapes raw control bytes, so an `OSC 8` sequence a caller
/// embedded arrives as the four characters `\`, `x`, `1`, `b` and a trailing
/// `\x07`. That is not a hyperlink, it is text - and it sits immediately before
/// the URL, which is enough to stop `linkify_urls` (it wants a whitespace or
/// opening-bracket boundary). Dropping the pair leaves the bare URL, which the
/// modal's own renderer links correctly (gh #178) and which is what the copy
/// path already captured separately.
fn strip_escaped_osc8(text: &str) -> String {
    const OPEN: &str = "\\x1b]8;;";
    const CLOSE: &str = "\\x1b]8;;\\x07";
    if !text.contains(OPEN) {
        return text.to_string();
    }
    let mut out = text.replace(CLOSE, "");
    // An opener's URI runs to the escaped BEL that followed it.
    while let Some(start) = out.find(OPEN) {
        let after = out[start + OPEN.len()..].to_string();
        let end = after.find("\\x07").map_or(0, |at| at + 4);
        out.replace_range(start..start + OPEN.len() + end, "");
    }
    out
}
