//! Gh #398 gap 2 in a real terminal (tmux): a multi-step login keeps its trail.
//!
//! The unit rows prove the rule; this row proves **what a terminal receives**,
//! because the whole issue is about what a user can still read after a step has
//! moved on. It drives the real `/login` flow of the bundled
//! `openai-compatible` extension's `custom` preset, which declares three fields
//! (`base-url`, `api-key`, `model`) - a genuine multi-step flow, not a mock of
//! one.
//!
//! Three things are captured:
//!
//! 1. step 1 (the base URL) is **still on screen** when step 3 (the model id)
//!    opens - the defect was that each step cleared the last;
//! 2. the answered steps carry pi's `> value` **input marker**, so what the user
//!    typed is distinguishable from what the provider said;
//! 3. the **API key step's value never appears** anywhere in the pane, while the
//!    step itself is readable as asterisks. That is the risk this change
//!    introduces, so it is asserted before the visible-history rows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// The three values the flow is driven with. The key is deliberately memorable
/// so a leak is unmistakable.
#[cfg(unix)]
const BASE_URL: &str = "https://api.trail-test.example/v1";
#[cfg(unix)]
const API_KEY: &str = "sk-trail-secret-9931";
#[cfg(unix)]
const MODEL: &str = "trail-model-1";

// Verifies: gh #398 gap 2 - in a real pane the completed steps stay on screen,
/// carry the input marker, and a secret step is masked rather than printed.
#[cfg(unix)]
#[test]
fn a_multi_step_login_keeps_its_trail_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    // No turn is ever run: this row is about the login modal.
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("login-trail-398");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("login-trail-398");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // `/login <provider>` opens the option picker (the argument is a single
    // token, so `custom` cannot ride on the same line). The `custom` preset is
    // last, and it is the one that declares three fields - a genuine multi-step
    // flow rather than a mock of one.
    session.send(&["C-u"]);
    session.send_literal("/login openai-compatible");
    session.send(&["Enter"]);
    session.wait_for("Sign in with:", std::time::Duration::from_secs(20));
    // Walk to the last option. `End` is not bound in the picker, so this is
    // Downs; the list is stable for a given extension build.
    for _ in 0..24 {
        session.send(&["Down"]);
    }
    session.send(&["Enter"]);
    // Step 1: the base URL.
    session.wait_for("Base URL", std::time::Duration::from_secs(20));
    session.send_literal(BASE_URL);
    session.send(&["Enter"]);

    // Step 2: the API key, masked.
    session.wait_for("API key", std::time::Duration::from_secs(20));
    session.send_literal(API_KEY);
    session.send(&["Enter"]);

    // Step 3: the model id. By now steps 1 and 2 have completed.
    let pane = session.wait_for("Model id", std::time::Duration::from_secs(20));

    // --- the safety assertion, first ---
    assert!(
        !pane.contains(API_KEY),
        "the API key reached the pane's scrollback:\n{pane}"
    );
    // ...and the step is not silently missing: it is on the trail, masked.
    assert!(
        pane.contains(&"*".repeat(API_KEY.len())),
        "the key step is on the trail, masked:\n{pane}"
    );

    // --- visible history ---
    assert!(
        pane.contains(BASE_URL),
        "the first step (the base URL) is still on screen:\n{pane}"
    );
    assert!(
        pane.contains("Model id"),
        "and the live prompt is beneath it:\n{pane}"
    );
    // The input marker: the base URL was typed, so it renders as input.
    assert!(
        pane.contains(&format!("> {BASE_URL}")),
        "the answered step carries the input marker:\n{pane}"
    );
    // Ordering: the trail above, the live step below.
    let first = pane.find(BASE_URL).expect("the base URL is on screen");
    let live = pane.find("Model id").expect("the live prompt is on screen");
    assert!(first < live, "history reads above the live step:\n{pane}");

    // Finish the flow. The endpoint is one the manifest does not cover, so an ad
    // hoc grant consent follows - and that consent is where the whole trail
    // matters most, because it names the host the flow just collected.
    session.send_literal(MODEL);
    session.send(&["Enter"]);
    session.wait_for("ad hoc grant", std::time::Duration::from_secs(20));
    let consent = session.capture();
    assert!(
        consent.contains(BASE_URL),
        "the consent shows the base URL the grant is about:\n{consent}"
    );
    assert!(
        !consent.contains(API_KEY),
        "and still never the key:\n{consent}"
    );
    session.send(&["n"]);
    let after = session.wait_until_painted(std::time::Duration::from_secs(15));
    assert!(
        after.contains(BASE_URL) || after.contains("trail-test"),
        "the trail reached the transcript, which survives the modal:\n{after}"
    );

    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(20));
}
