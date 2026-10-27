//! Gh #398 gap 2: a multi-step login keeps its trail instead of clobbering it.
//!
//! Every step used to **clear** the one before it, so by the time a login failed
//! the user had lost the URL they were told to visit and the code they were told
//! to paste - the two things needed to work out which step went wrong. pi keeps
//! the whole trail (`showPrompt` appends; `replaceInputWithSubmittedText` swaps
//! the answered input for a static `> value` line).
//!
//! The rows are written in the order of the harm they prevent, which is the
//! brief's order: the **masked-history** row comes first, because "append every
//! step" would print a secret into the transcript if a flow ever collected a key
//! through the same prompt. Visible-history, the `> value` marker, the failure
//! path, and "the newest prompt is still visible" follow.
//!
//! Verifies: gh #398 gap 2, FR-UI-5, NFR-24.

use super::*;
use super::{chat, strip};

/// A URL long enough to wrap several rows at 80 columns.
fn sign_in_url() -> String {
    format!(
        "https://accounts.example.test/o/oauth2/v2/auth?response_type=code&client_id={}\
         &redirect_uri=https%3A%2F%2Flocalhost%3A8080%2Fcallback&scope=openid%20email",
        "7".repeat(60)
    )
}

/// The first step of a real flow: the CLI reports the sign-in URL.
fn waiting(chat: &mut Chat, url: &str) {
    chat.apply_login_next(LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{url}"),
    });
}

/// The second step: the flow asks for the pasted callback.
fn ask_for_code(chat: &mut Chat, masked: bool) {
    chat.apply_login_next(LoginNext::Secret {
        provider: "example".into(),
        label: "Paste the callback URL:".into(),
        masked,
    });
}

/// The whole modal as plain text.
fn frame(chat: &Chat, width: u16, height: u16) -> String {
    strip(&chat.viewport(width, height, 0)).join("\n")
}

// ---------------------------------------------------------------------------
// The safety row: a secret never becomes history. Written first on purpose.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 2 design point 3 - **no secret survives into the trail**.
// The fix ("append every step") is exactly what would leak a key into the
// scrollback if a flow collected one through the same prompt, so the value is
// dropped where the step becomes history rather than redacted at paint time -
// there is then nothing for any later renderer change to un-redact.
#[test]
fn a_secret_step_stays_masked_after_the_flow_advances() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, true);
    let secret = "sk-live-51H-do-not-print-me";
    for character in secret.chars() {
        chat.handle_key(&character.to_string());
    }
    assert_eq!(
        chat.world.secret.as_ref().expect("the field is open").input,
        secret,
        "the field took the value"
    );

    // The flow advances: this is where a naive append would print it.
    chat.apply_login_next(LoginNext::Waiting {
        label: "waiting for browser sign-in… (esc cancels)".into(),
    });

    // Nowhere: not the modal, not the trail, not the stored entry.
    let modal = frame(&chat, 80, 24);
    assert!(
        !modal.contains(secret),
        "the secret reached the modal after advancing:\n{modal}"
    );
    assert!(
        !chat.world.login_history.iter().any(|entry| matches!(
            entry,
            crate::state::LoginHistoryEntry::Answer { plain: Some(plain), .. } if plain == secret
        )),
        "the secret is stored in the clear in the trail"
    );
    // Masked, not silently dropped and not shown in the clear.
    assert!(
        modal.contains(&"*".repeat(secret.chars().count())),
        "the answered step is on the trail, masked:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 design point 3 - the strongest form: the secret is not
// merely unrendered, it is **absent from the entry**, so spilling the trail to
// the transcript at the end of the flow cannot carry it either.
#[test]
fn a_secret_is_absent_from_the_transcript_the_flow_spills() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, true);
    let secret = "sk-live-abcdef";
    for character in secret.chars() {
        chat.handle_key(&character.to_string());
    }
    // A failure ends the flow and spills the trail into the transcript.
    chat.apply_login_next(LoginNext::Message(
        "could not sign in: the code was rejected".into(),
    ));
    let transcript = strip(&chat.viewport(80, 40, 0)).join("\n");
    assert!(
        !transcript.contains(secret),
        "the secret reached the transcript:\n{transcript}"
    );
    assert!(
        transcript.contains("could not sign in"),
        "the failure is still reported:\n{transcript}"
    );
    // Not merely "the secret is missing": the masked step must actually be
    // **there**, or this row would pass on a trail that recorded nothing.
    assert!(
        transcript.contains("Paste the callback URL:"),
        "the step that answered is on the trail:\n{transcript}"
    );
    assert!(
        transcript.contains(&"*".repeat(secret.chars().count())),
        "and it is masked, not blank:\n{transcript}"
    );
    assert!(
        transcript.contains("accounts.example.test"),
        "with the step before it intact:\n{transcript}"
    );
}

// ---------------------------------------------------------------------------
// Visible history: a completed step remains after the next prompt appears.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 2 design point 1 - a completed step is history, not a
// replacement. This is the defect itself: before, the second step cleared the
// first and the URL the user was told to visit was gone.
#[test]
fn a_completed_step_stays_visible_when_the_next_prompt_opens() {
    let mut chat = chat();
    let url = sign_in_url();
    waiting(&mut chat, &url);
    ask_for_code(&mut chat, false);

    let modal = frame(&chat, 80, 24);
    assert!(
        modal.contains("accounts.example.test"),
        "the URL from the first step is still on screen:\n{modal}"
    );
    assert!(
        modal.contains("Paste the callback URL:"),
        "and the live prompt is beneath it:\n{modal}"
    );
    // Order: the trail above, the live step below (a transcript reads downward).
    let url_at = modal.find("accounts.example.test");
    let prompt_at = modal.find("Paste the callback URL:");
    assert!(
        url_at < prompt_at,
        "history is above the live step:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 - a **three**-step flow keeps all of it, which is the
// shape the issue names (URL -> code -> workspace).
#[test]
fn a_three_step_flow_keeps_every_step() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, false);
    for character in "http://localhost:8080/callback?code=abc".chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
    chat.apply_login_next(LoginNext::Secret {
        provider: "example".into(),
        label: "Workspace slug:".into(),
        masked: false,
    });

    let modal = frame(&chat, 80, 30);
    assert!(
        modal.contains("accounts.example.test"),
        "step 1 (the URL) survived:\n{modal}"
    );
    assert!(
        modal.contains("localhost:8080/callback?code=abc"),
        "step 2 (the pasted code) survived:\n{modal}"
    );
    assert!(
        modal.contains("Workspace slug:"),
        "step 3 is the live prompt:\n{modal}"
    );
}

// ---------------------------------------------------------------------------
// A submitted value is displayed as input, not prose.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 2 design point 2 - a submitted value renders with the
// input marker, so the user can tell what they typed from what the provider
// said. Asserting the **marker** rather than only the text is the point: the
// text alone does not distinguish input from prose.
#[test]
fn a_submitted_value_carries_the_input_marker() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, false);
    for character in "http://localhost:8080/callback?code=abc".chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
    chat.apply_login_next(LoginNext::Secret {
        provider: "example".into(),
        label: "Workspace slug:".into(),
        masked: false,
    });

    let modal = frame(&chat, 80, 30);
    assert!(
        modal.contains("> http://localhost:8080/callback?code=abc"),
        "the submitted value is marked as input:\n{modal}"
    );
}

// ---------------------------------------------------------------------------
// The trail survives a failure - the whole point.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 2 design point 4 - when a step errors, the completed
// steps **stay**. Pinned on the failure path, not only the happy one, because a
// failure is the only time the trail matters.
#[test]
fn a_failed_step_keeps_the_trail_and_names_the_error() {
    let mut chat = chat();
    let url = sign_in_url();
    waiting(&mut chat, &url);
    ask_for_code(&mut chat, false);
    for character in "http://localhost:8080/callback?code=wrong".chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
    chat.apply_login_next(LoginNext::Message(
        "could not sign in: `state` did not match".into(),
    ));

    let screen = strip(&chat.viewport(80, 40, 0)).join("\n");
    assert!(
        screen.contains("accounts.example.test"),
        "the URL the user was told to visit survived the failure:\n{screen}"
    );
    assert!(
        screen.contains("localhost:8080/callback?code=wrong"),
        "the code the user pasted survived it too:\n{screen}"
    );
    assert!(
        screen.contains("could not sign in"),
        "and the error is named:\n{screen}"
    );
    // The modal is gone: the flow ended, so nothing is left owning the keyboard.
    assert!(
        !chat.world.modal_active(),
        "a finished flow does not keep the modal"
    );
    // The trail is its **own** transcript entry, above the step's message: what
    // happened, then what it amounted to. One concatenated notice would make the
    // trail unsearchable and would put the error inside it.
    let notices = chat.transcript.notices();
    let trail_at = notices
        .iter()
        .position(|notice| notice.contains("accounts.example.test"))
        .expect("the trail is a notice of its own");
    let error_at = notices
        .iter()
        .position(|notice| notice.contains("could not sign in"))
        .expect("the error is a notice");
    assert!(
        trail_at < error_at,
        "the trail comes before the error: {notices:?}"
    );
    assert!(
        !notices[error_at].contains("accounts.example.test"),
        "and the error notice is only the error: {notices:?}"
    );
}

// Verifies: gh #398 gap 2 - a cancel also keeps its trail, so "cancelled" and
// "hung" do not look the same.
#[test]
fn a_cancelled_flow_keeps_its_trail() {
    let mut chat = chat();
    let url = sign_in_url();
    waiting(&mut chat, &url);
    chat.handle_key("\x1b");

    let screen = strip(&chat.viewport(80, 30, 0)).join("\n");
    assert!(
        screen.contains("accounts.example.test"),
        "the step that was cancelled is still readable:\n{screen}"
    );
    assert!(
        screen.contains("login cancelled"),
        "and the cancel is named:\n{screen}"
    );
}

// ---------------------------------------------------------------------------
// The newest prompt stays reachable: no hidden input.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 2 design point 5 - appending history must not push the
// live prompt below the fold. A long flow on a short terminal windows the trail
// from its **newest** end and never trims the current step, so the fix cannot
// trade a lost trail for a hidden input.
#[test]
fn the_newest_prompt_stays_visible_when_the_trail_is_long() {
    let mut chat = chat();
    // Five steps, each with a label long enough to wrap.
    for index in 0..5 {
        waiting(
            &mut chat,
            &format!(
                "https://accounts.example.test/step{index}?state=long-enough-to-wrap-at-eighty-columns-because-it-has-to-be"
            ),
        );
    }
    chat.apply_login_next(LoginNext::Secret {
        provider: "example".into(),
        label: "Paste the callback URL:".into(),
        masked: false,
    });
    // A leading space is the typed input, so the row is unmistakable.
    for character in " http://localhost:8080/callback".chars() {
        chat.handle_key(&character.to_string());
    }

    let modal = frame(&chat, 80, 24);
    assert!(
        modal.contains("Paste the callback URL:"),
        "the live prompt is on screen:\n{modal}"
    );
    assert!(
        modal.contains("callback"),
        "with the text being typed in it:\n{modal}"
    );
    // The window is announced rather than silently truncated.
    assert!(
        modal.contains("earlier step"),
        "the elided trail is announced:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 - a short trail is never elided (the note is for
// overflow only), so the common one- and two-step flow reads in full.
#[test]
fn a_short_trail_is_shown_whole() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, false);

    let modal = frame(&chat, 80, 24);
    assert!(
        !modal.contains("earlier step"),
        "nothing is elided when it all fits:\n{modal}"
    );
}

// ---------------------------------------------------------------------------
// What this change must not alter.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 1 (no regression) - the byte-exact copy still works
// while the trail is on screen, and still targets the *current* step's URL.
#[test]
fn the_url_copy_is_unaffected_by_the_trail() {
    let mut chat = chat();
    let url = sign_in_url();
    waiting(&mut chat, &url);
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some(url.as_str()),
        "the copy target is the URL and nothing else"
    );
}

// Verifies: gh #398 gap 2 constraint 6 - the consent/countdown surface is
// untouched: a permission modal still renders its countdown, and a login step
// arriving does not disturb it.
#[test]
fn the_permission_countdown_is_untouched() {
    let mut chat = chat();
    chat.world.permission = Some(crate::state::PermissionModal {
        action: "net: connect to example.test".into(),
        respond: None,
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    });
    let modal = frame(&chat, 80, 24);
    assert!(
        modal.contains("auto-approves in"),
        "the countdown still renders:\n{modal}"
    );
    assert!(
        !modal.contains("login"),
        "and a login modal is not stacked over it:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 - a new `/login` starts from a clean modal. Two flows'
// steps reading as one would point the user at the wrong attempt, which is worse
// than no trail.
#[test]
fn a_finished_flow_leaves_no_trail_behind() {
    let mut chat = chat();
    waiting(&mut chat, &sign_in_url());
    ask_for_code(&mut chat, false);
    chat.handle_key("\x1b");
    assert!(
        chat.world.login_history.is_empty(),
        "the finished flow took its trail with it"
    );
    // A fresh login shows only its own step.
    waiting(&mut chat, "https://accounts.example.test/second-login");
    let modal = frame(&chat, 80, 24);
    assert!(
        !modal.contains("step0"),
        "the previous login's steps are gone:\n{modal}"
    );
    assert!(
        modal.contains("second-login"),
        "and the new one is shown:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 - the ad hoc `net` grant is a **step of the flow**, not
// a separate screen, so the trail above it is what lets the user check the host
// being granted against what the flow collected.
#[test]
fn the_grant_consent_keeps_the_trail_above_it() {
    let mut chat = chat();
    let url = sign_in_url();
    waiting(&mut chat, &url);
    ask_for_code(&mut chat, false);
    for character in "https://api.example.test/v1".chars() {
        chat.handle_key(&character.to_string());
    }
    chat.handle_key("\r");
    chat.apply_login_next(LoginNext::Grant {
        provider: "example".into(),
        host: "api.example.test".into(),
        prompt: "example's endpoint is api.example.test; add it as an ad hoc grant?".into(),
    });

    let modal = frame(&chat, 80, 24);
    assert!(
        modal.contains("accounts.example.test"),
        "the URL from step 1 is still above the consent:\n{modal}"
    );
    assert!(
        modal.contains("api.example.test/v1"),
        "and the base URL the user typed, so the host is checkable:\n{modal}"
    );
    assert!(
        modal.contains("Allow [y]"),
        "with the live consent beneath:\n{modal}"
    );
}

// Verifies: gh #398 gap 2 - answering the ad hoc grant **ends** the flow (the CLI
// sends no further `Message`), so the trail must be spilled here too. Without
// that call the whole multi-step flow was orphaned at the last step - the one
// place a user is most likely to look, because the grant names the host the flow
// just collected.
#[test]
fn answering_the_grant_spills_the_trail_to_the_transcript() {
    for denial in [true, false] {
        let mut chat = chat();
        waiting(&mut chat, &sign_in_url());
        ask_for_code(&mut chat, false);
        for character in "https://api.example.test/v1".chars() {
            chat.handle_key(&character.to_string());
        }
        chat.handle_key("\r");
        chat.apply_login_next(LoginNext::Grant {
            provider: "example".into(),
            host: "api.example.test".into(),
            prompt: "add it as an ad hoc grant?".into(),
        });
        chat.handle_key(if denial { "n" } else { "y" });

        let screen = strip(&chat.viewport(80, 30, 0)).join("\n");
        assert!(
            screen.contains("accounts.example.test"),
            "the flow's trail survived the grant answer (denial={denial}):\n{screen}"
        );
        assert!(
            screen.contains("https://api.example.test/v1"),
            "including the base URL the grant was about (denial={denial}):\n{screen}"
        );
        assert!(
            chat.world.login_history.is_empty(),
            "and the modal keeps no orphaned trail"
        );
    }
}
