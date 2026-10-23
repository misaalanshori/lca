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
//! Gh #456 retargeted these rows. They used to point at the loopback mock's own
//! `127.0.0.1` and rely on discovery being *refused* (the native path dropped
//! the manifest's `net-local` grant) so the consent modal fired. #456 fixed that
//! divergence - #294's `[capabilities.net-local] addresses = ["localhost",
//! "127.0.0.1"]` now reaches the native path - so a loopback endpoint is
//! **covered by the manifest** and needs no endpoint consent at all. The rows
//! below split on that boundary now:
//!
//! - a **covered** endpoint (loopback) lists and runs with no ask at all, in
//!   this session and in a fresh process, with nothing persisted;
//! - an **uncovered** endpoint (an internet host, which `net-local` can never
//!   reach) still asks, and a denial still explains itself.
//!
//! Every spawn uses `write_grants(false)`, so the harness's auto-approve does
//! not run: the manager's scratch-HOME, env-only shape.

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

/// An endpoint the bundled manifest does **not** cover (gh #456).
///
/// An internet host name on the mock's port. `net-local` can never cover it -
/// FR-PERM-14 confines that grant to the canonical local ranges - so the request
/// is refused until the user adds an ad-hoc `net` grant, which is the consent
/// journey these rows drive. `.example` is reserved by RFC 6761 and never
/// resolves, so discovery fails for a name nothing answers: the empty-list shape
/// the picker turns into an ask. The port is the live mock's, so the row fails
/// on consent, never on the network.
#[cfg(unix)]
fn uncovered_url(mock: &common::Mock) -> String {
    format!("http://uncovered.example:{}/v1", mock.addr.port())
}

// Verifies: gh #31 review's consent contract at its new boundary (gh #456) - a
// **manifest-covered** endpoint lists with no ask at all, the picker names the
// service in every row rather than the crate, a second listing asks nothing
// either, and a fresh process over the same home lists without asking - because
// the coverage is the manifest's, not a stored grant.
//
// SANCTIONED CHANGE (gh #456). This row used to assert the *ask* for a loopback
// endpoint: discovery was refused (the native path dropped the manifest's
// `net-local`), listed nothing, and the picker raised a consent. #456 made the
// native path honour the grant, so that ask cannot fire for a loopback host and
// asserting it would pin the divergence. The consent journey itself is still
// covered, for an uncovered host, by
// `a_denied_endpoint_reports_not_granted_rather_than_no_models`.
#[cfg(unix)]
#[test]
fn a_covered_endpoint_lists_without_a_consent_and_names_the_service() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("covered-turn-ok"))]));
    let sandbox = sandbox("model-consent-allow");
    sandbox.write_grants(false);

    let session = Tmux::new("model-consent-allow");
    session.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    session.wait_for("[session in", Duration::from_secs(25));
    // gh #446 (the #447 family): this row failed on macOS in 0.82 s - far too
    // fast for the consent round-trip it exercises - because the keystrokes
    // below arrived before the input thread was reading. Wait for an echoed
    // keystroke: that can only come through a live input path, and it is the
    // readiness `[session in` was wrongly taken to imply.
    session.wait_for_input_ready(Duration::from_secs(15));

    // 1. No ask: the endpoint is inside the manifest's declared `net-local`
    //    grant, so the capability layer carries it and the CLI adds no second,
    //    redundant consent (gh #456).
    session.send(&["/model", "Enter"]);
    // Both rows are asserted from one frame, so read a settled one: the picker
    // fills its rows over several paints (gh #447).
    let pane = session.wait_settled_for("zen-free (", Duration::from_secs(25));
    assert!(
        !pane.contains("permission required"),
        "a covered endpoint is never asked about:\n{pane}"
    );
    assert!(
        pane.contains("zen-free (127.0.0.1)") && pane.contains("zen-lite (127.0.0.1)"),
        "each row names the service that will bill the call:\n{pane}"
    );
    assert!(
        !pane.contains("(openai-compatible)"),
        "the crate name is nowhere in the rows:\n{pane}"
    );
    // 2. Selecting a row closes the picker and lands the model (the pick is
    //    the confirmation). Live rows rather than a frozen list.
    session.send(&["Enter"]);
    session.wait_until_painted(Duration::from_secs(15));
    std::thread::sleep(Duration::from_millis(400));

    // 3. The turn runs against that endpoint with no consent anywhere in the
    //    path - which is the whole of #456's user-visible acceptance: the
    //    manifest's local grant reaches the native provider.
    session.send(&["hello", "Enter"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let pane = loop {
        let pane = session.capture();
        if pane.contains("covered-turn-ok") || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("covered-turn-ok"),
        "the turn ran against the covered endpoint:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "with no consent prompt at any point:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(25));
}

// Verifies: gh #31 review's **coverage** contract, retargeted by gh #456 - a
// manifest-covered endpoint is covered for every listing in the run *and* in a
// fresh process over the same home, with no grant stored anywhere.
//
// SANCTIONED CHANGE (gh #456). This row used to answer `once` at the picker
// consent and then prove `once` is never persisted. After #456 the bundled
// manifest's `net-local` grant reaches the native path, so the picker consent
// never fires for a loopback endpoint and there is no `once` answer to give.
// The persistence question still exists for uncovered hosts and is covered by
// the deny row below; what is worth pinning here is the stronger property that
// replaced it: manifest coverage needs nothing persisted at all.
#[cfg(unix)]
#[test]
fn a_manifest_covered_endpoint_is_covered_in_a_fresh_process_too() {
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

    // 1. No ask: the endpoint is inside the manifest's declared grant.
    session.send(&["/model", "Enter"]);
    let pane = session.wait_settled_for("zen-free (", Duration::from_secs(25));
    assert!(
        pane.contains("zen-free (127.0.0.1)"),
        "the picker lists a covered endpoint:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "and asked nothing:\n{pane}"
    );
    // Pick the highlighted row, so the turn below has a model to ask for.
    session.send(&["Enter"]);
    session.wait_until_painted(Duration::from_secs(15));
    std::thread::sleep(Duration::from_millis(400));

    // 2. The turn runs on that endpoint without a consent.
    session.send(&["hello", "Enter"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let pane = loop {
        let pane = session.capture();
        if pane.contains("once-turn-ok") || std::time::Instant::now() > deadline {
            break pane;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    assert!(
        pane.contains("once-turn-ok"),
        "the turn ran against the covered endpoint:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "with no consent prompt at any point:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(25));

    // 3. A fresh process over the same home lists it too, asking nothing:
    //    the coverage came from the manifest, so nothing was persisted.
    let again = Tmux::new("model-consent-once-fresh");
    again.spawn(&sandbox, Some(&mock), true, discovery_env(), &[]);
    again.wait_for("[session in", Duration::from_secs(25));
    again.send(&["/model", "Enter"]);
    let pane = again.wait_settled_for("zen-free (", Duration::from_secs(25));
    assert!(
        pane.contains("zen-free (127.0.0.1)"),
        "a fresh process lists it too:\n{pane}"
    );
    assert!(
        !pane.contains("permission required"),
        "and asks nothing:\n{pane}"
    );
    again.send(&["Enter"]);
    again.wait_until_painted(Duration::from_secs(15));
    std::thread::sleep(Duration::from_millis(400));
    again.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(25));
}

// Verifies: gh #31 review (deny half) - refusing the endpoint says what is
// wrong ("not granted - approve it or run /login"), not "no models": the empty
// list is a consequence, not the diagnosis.
//
// SANCTIONED CHANGE (gh #456): the endpoint is now a host the manifest does not
// cover (an internet name), because a loopback one became manifest-covered when
// #456 made the native path honour #294's `net-local` grant. This is the one row
// that still exercises the consent journey, and it does so at the boundary the
// modal actually exists for.
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
    let endpoint = uncovered_url(&mock);
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", ""), ("OPENAI_BASE_URL", &endpoint)],
        &[],
    );
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
        pane.contains("is not granted for this project") && pane.contains("uncovered.example"),
        "the denial names the host and the fix:\n{pane}"
    );
    assert!(
        !pane.contains("no models are offered"),
        "an ungranted endpoint is not reported as an empty provider:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), Duration::from_secs(15));
}
