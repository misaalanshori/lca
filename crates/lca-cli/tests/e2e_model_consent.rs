//! gh #31 review finding: `/model`'s live `GET /models` discovery ran
//! outside gh #29's endpoint consent, so an env-configured ungranted host
//! failed the discovery silently and the picker reported the misleading
//! "no models are offered by the active provider".
//!
//! The fix runs the same `endpoint_consent` before the ask, off the
//! interface's thread (the loop must keep painting for the modal), and
//! hands the discovered rows to the picker once the answer lands. These
//! rows drive that journey end to end: ask first, list after, never ask
//! twice - and, on a deny, say what is actually wrong.
//!
//! The endpoint is the loopback mock with **no** net grant
//! (`write_grants(false)`, so the harness's auto-approve does not run):
//! exactly the manager's scratch-HOME, env-only shape.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so their imports and helpers are too: an
// ungated glob import or `fn` is an unused import/function on Windows,
// which clippy denies (the same rule `e2e_terminal_consent.rs` carries).
#[cfg(unix)]
use common::*;
#[cfg(unix)]
use std::time::Duration;

/// The receipt's environment: an endpoint, a key, and no model chosen -
/// which is what makes `list_models` reach for live discovery.
#[cfg(unix)]
fn discovery_env() -> &'static [(&'static str, &'static str)] {
    &[("OPENAI_MODEL", "")]
}

// Verifies: gh #31 review (allow half) - the first `/model` asks for the
// ungranted endpoint before anything is listed, and the answer the picker
// opens with names the service rather than the crate. A second `/model`
// in the same run does not ask again (the grant is persisted), and a
// fresh process over the same home lists without asking at all.
#[cfg(unix)]
#[test]
fn an_ungranted_endpoint_prompts_before_the_model_list_and_lists_after() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("model-consent-allow");
    sandbox.write_grants(false);

    let session = Tmux::new("model-consent-allow");
    session.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    session.wait_for("[session in", Duration::from_secs(25));

    // 1. The ask comes first: the modal names the exact host (FR-PERM-16).
    session.send(&["/model", "Enter"]);
    let pane = session.wait_for("permission required", Duration::from_secs(20));
    assert!(
        pane.contains("connect to 127.0.0.1"),
        "the consent names the endpoint host:\n{pane}"
    );

    // 2. Allow: discovery runs (the grant landed first), the rows arrive,
    //    and the picker opens with the service in every label.
    session.send(&["a"]);
    let pane = session.wait_for("zen-free (", Duration::from_secs(25));
    assert!(
        pane.contains("zen-free (127.0.0.1)") && pane.contains("zen-lite (127.0.0.1)"),
        "each row names the service that will bill the call:\n{pane}"
    );
    assert!(
        !pane.contains("(openai-compatible)"),
        "the crate name is nowhere in the rows:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(Duration::from_millis(400));

    // 3. The grant persists: the second listing offers the model again
    //    and asks nothing. The row may be showing in the picker or
    //    already selected into the footer, so the assertion is the model
    //    plus the absence of a second ask - not a particular widget.
    session.send(&["/model", "Enter"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let pane = loop {
        let pane = session.capture();
        if pane.contains("zen-free") || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("zen-free"),
        "the second listing offers the model again:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "the consent is per host: the second listing asked nothing:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(Duration::from_millis(300));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));

    // 4. A fresh process over the same home: still no ask, still the
    //    model on offer - the grant outlived the run that made it.
    let again = Tmux::new("model-consent-fresh");
    again.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    again.wait_for("[session in", Duration::from_secs(25));
    again.send(&["/model", "Enter"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let pane = loop {
        let pane = again.capture();
        if pane.contains("zen-free") || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("zen-free"),
        "a fresh process lists the model again:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "a fresh process lists with no re-prompt:\n{pane}"
    );
    again.send(&["Escape"]);
    std::thread::sleep(Duration::from_millis(300));
    again.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));
}

// Verifies: gh #31 review (the `once` half of the one answer mapping) -
// `o` lists the models now, covers the turn for the rest of this run, and
// says what it granted; a fresh process's first *request* asks again,
// because `once` is never persisted. (The picker may still list from the
// stored list in that fresh process - a cache is not a request, so there
// is nothing to consent to - which is why the proof is on the turn.)
#[cfg(unix)]
#[test]
fn the_once_answer_lists_now_and_asks_again_in_a_fresh_process() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("once-turn-ok"))]));
    let sandbox = sandbox("model-consent-once");
    sandbox.write_grants(false);

    let session = Tmux::new("model-consent-once");
    session.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    session.wait_for("[session in", Duration::from_secs(25));

    // 1. `once` at the picker consent: the discovery request that follows
    //    is covered, so the list opens - it used to stall with nothing,
    //    because `once` had granted nothing and the request was denied
    //    silently.
    session.send(&["/model", "Enter"]);
    session.wait_for("permission required", Duration::from_secs(20));
    session.send(&["o"]);
    let pane = session.wait_for("zen-free (", Duration::from_secs(25));
    assert!(
        pane.contains("zen-free (127.0.0.1)"),
        "the picker lists after `once`:\n{pane}"
    );
    assert!(
        pane.contains("allowed for this session"),
        "the flow says what `once` granted:\n{pane}"
    );
    // Pick the highlighted row, so the turn below has a model to ask for.
    session.send(&["Enter"]);
    std::thread::sleep(Duration::from_millis(400));

    // 2. The rest of this run: the turn needs no consent (the session set
    //    covers it) and the reply lands.
    session.send(&["hello", "Enter"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    let pane = loop {
        let pane = session.capture();
        if pane.contains("once-turn-ok") || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("once-turn-ok"),
        "the turn ran without a second ask:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "the session grant covered the turn:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));

    // 3. The distinguishing step: a fresh process's first request asks
    //    again, so `once` never became `always`.
    let again = Tmux::new("model-consent-once-fresh");
    again.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    again.wait_for("[session in", Duration::from_secs(25));
    again.send(&["hello", "Enter"]);
    let pane = again.wait_for("permission required", Duration::from_secs(20));
    assert!(
        pane.contains("connect to 127.0.0.1"),
        "a fresh process asks again - `once` was not persisted:\n{pane}"
    );
    again.send(&["d"]);
    std::thread::sleep(Duration::from_millis(400));
    again.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));
}

// Verifies: gh #31 review (deny half) - refusing the endpoint says what
// is wrong ("not granted - approve it or run /login"), not "no models":
// the empty list is a consequence, not the diagnosis.
#[cfg(unix)]
#[test]
fn a_denied_endpoint_reports_not_granted_rather_than_no_models() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("model-consent-deny");
    sandbox.write_grants(false);

    let session = Tmux::new("model-consent-deny");
    session.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    session.wait_for("[session in", Duration::from_secs(25));

    session.send(&["/model", "Enter"]);
    session.wait_for("permission required", Duration::from_secs(20));
    session.send(&["d"]);

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let pane = loop {
        let pane = session.capture();
        if pane.contains("is not granted for this project") || std::time::Instant::now() > deadline
        {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("is not granted for this project") && pane.contains("127.0.0.1"),
        "the denial names the host and the fix:\n{pane}"
    );
    assert!(
        !pane.contains("no models are offered"),
        "an ungranted endpoint is not reported as an empty provider:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));
}
