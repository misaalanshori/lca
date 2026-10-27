//! Applying the CLI's login steps and the label cleanup that goes with them
//! (ceiling split from `state.rs`, gate 11).
//!
//! `apply_login_next` is the one place a login step becomes interface state, so
//! it is also the one place that captures the step's sign-in URL for the copy
//! path (gh #398 gap 1) and the one place a finished step is appended to the
//! modal's history (gh #398 gap 2). Keeping it beside `state.rs` inside one
//! module keeps the "one step owns the screen" rule readable as a unit.

use super::state::{
    GrantPrompt, LoginHistoryEntry, LoginNext, PickerPrompt, SecretPrompt, SwitchPrompt, UiState,
    sanitize_block,
};

/// Apply the CLI's next login step: one modal at a time, and a
/// [`LoginNext::Secret`] mid-flow is the next field of a multi-field
/// login, not a refusal. Returns a message for history when the step
/// says something (gh #239: the dock catch-all is gone, so login
/// progress lands in the transcript).
pub fn apply_login_next(state: &mut UiState, next: LoginNext) -> Option<String> {
    // Gh #398 gap 2: a step that is still on screen is finished by the arrival
    // of the next one, so it becomes history before the new step takes over.
    // Appending here - at the one place a step enters the interface - is what
    // makes the trail complete rather than best-effort: no step can clear
    // another without first being recorded.
    promote_pending_step(state);
    // One step owns the screen at a time: entering one clears the others.
    // `Waiting` is the exception that must not clear a manual-callback
    // secret field, so it only sets the waiting label.
    match next {
        LoginNext::Message(text) => {
            // Gh #398 gap 2: a `Message` ends the flow. The trail is left in
            // place for the caller to spill as its **own** transcript entry
            // ([`take_login_trail`]) rather than concatenated onto this text:
            // the trail is what happened, this message is what it amounted to,
            // and a reader wants them as two lines.
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

/// Record an answered step, so it stays on the flow's trail (gh #398 gap 2).
///
/// Called from the one place an answer exists - the Enter branch of the masked
/// field's key handler - because the handler **takes** the prompt before handing
/// the next step to [`apply_login_next`]. Without this call the answered step
/// would be gone before the promotion had anything to promote, which is how the
/// first version of this change lost every typed value.
///
/// A masked step records asterisks and stores `None` as its plain text, so the
/// secret is absent from the entry rather than merely unrendered.
pub fn record_login_answer(state: &mut UiState, prompt: &SecretPrompt, typed: &str) {
    if typed.is_empty() {
        // Nothing was entered, so nothing was answered - a step that collected
        // nothing must not read as a step that did.
        return;
    }
    state.login_history.push(LoginHistoryEntry::Answer {
        prompt: prompt.label.clone(),
        value: if prompt.masked {
            "*".repeat(typed.chars().count())
        } else {
            typed.to_string()
        },
        plain: (!prompt.masked).then(|| typed.to_string()),
    });
}

/// Turn the trail into transcript text and clear it, so the finished flow can be
/// read back above the message that ends it (gh #398 gap 2).
///
/// Public because the transcript belongs to [`crate::Chat`], not to this
/// module: the state layer says *what* happened, the chat decides where it goes.
///
/// `None` when there is nothing to spill, so a `/login` that fails on its very
/// first step does not prepend an empty block to its own error.
///
/// The rows are the **plain** text, not the painted ones: this is history, and a
/// themed row belongs to the modal that drew it. `trail_rows` paints; this builds
/// the same two shapes for a surface that has its own roles.
pub fn take_login_trail(state: &mut UiState) -> Option<String> {
    if state.login_history.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for entry in std::mem::take(&mut state.login_history) {
        match entry {
            LoginHistoryEntry::Message(text) => lines.extend(text.lines().map(str::to_string)),
            LoginHistoryEntry::Answer { prompt, value, .. } => {
                lines.push(format!(">{prompt}"));
                lines.push(format!("> {value}"));
            }
        }
    }
    Some(lines.join("\n"))
}

/// Record the step that is about to be replaced, so the flow reads as a
/// transcript instead of only ever showing its current line (gh #398 gap 2).
///
/// Called at the top of [`apply_login_next`], which is the single point where a
/// step enters the interface - so a step cannot be cleared without first being
/// recorded, and no call site has to remember to record it.
///
/// **A secret never becomes history.** A masked field is dropped here rather
/// than redacted at paint time: the value never enters the entry at all, so no
/// later change to the renderer, the scrollback, or a log can leak what was
/// never stored. That is the whole safety argument, and it is one line.
///
/// A `Waiting` step is recorded as its label - the sign-in URL or a `waiting…`
/// line - because that *is* what the step showed, and it is what a user pointing
/// at a failed login needs to read back.
fn promote_pending_step(state: &mut UiState) {
    if let Some(waiting) = state.login_waiting.take() {
        state
            .login_history
            .push(LoginHistoryEntry::Message(waiting));
    }
    // A masked field is discarded without being recorded; an unmasked one is a
    // base URL, a model id, or a pasted callback URL, which the user needs back.
    if let Some(secret) = state.secret.take() {
        state.login_history.push(LoginHistoryEntry::Answer {
            prompt: secret.label,
            value: if secret.masked {
                "*".repeat(secret.input.chars().count())
            } else {
                secret.input.clone()
            },
            plain: (!secret.masked).then_some(secret.input),
        });
    }
}

/// Finish the flow the way a cancel does: hand back what happened and clear the
/// modal's trail (gh #398 gap 2).
///
/// One path for "this login is over", so a cancel and a failure cannot leave the
/// user with different amounts of the story.
pub fn end_login(state: &mut UiState) -> Option<String> {
    // The step that was live when the flow ended is part of what happened, so it
    // joins the trail before the spill - otherwise cancelling a `Waiting` step
    // would lose exactly the URL the user was looking at.
    promote_pending_step(state);
    state.secret = None;
    state.auth_url = None;
    state.url_copied_at = None;
    take_login_trail(state)
}

impl crate::chat::Chat {
    /// Apply the CLI's next login step (the host's [`LoginNext`]).
    pub fn apply_login_next(&mut self, next: LoginNext) {
        // gh #233: every login/logout step can change credentials, so
        // what providers offer may have changed with it.
        self.invalidate_models();
        // Gh #398 gap 2: a step that ends the flow hands the whole trail back, so
        // it lands in the transcript above the step's own message - what happened,
        // then what it amounted to. The transcript is append-only and survives
        // scrolling, which is the difference between "the user saw it" and "the
        // user can still read it".
        let ends_flow = matches!(next, LoginNext::Message(_));
        let message = apply_login_next(&mut self.world, next);
        if ends_flow && let Some(trail) = take_login_trail(&mut self.world) {
            self.say(trail);
        }
        if let Some(message) = message {
            self.say(message);
        }
    }
}
