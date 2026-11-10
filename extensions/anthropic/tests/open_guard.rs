//! Gh #577's follow-up (finding F1): `MessagesDriver::open` acquires a host handle
//! from `net_request` and then makes **fallible** calls - `net_response_status`, and
//! `net_read_body` inside the non-2xx path - before the driver exists. Its `Drop`
//! only runs once there is a driver, so an `Err` in that window returned with the
//! handle still in the capability engine's table: one leaked response per failed
//! Anthropic request, bounded only by the process's lifetime.
//!
//! The fix is the shared [`lca_protocol::ResponseGuard`] (lifted out of
//! `lca-wire-openai` in this cycle, because seven extensions have the same shape).
//! These rows drive the real `MessagesDriver::open` over a fake `ProviderCap` that
//! **counts** closes, because "closed exactly once" is the property; "closed at
//! least once" would pass on a path that closes twice.
//!
//! The order matters too, and it is the delicate half: gh #278 made this path read
//! the response headers **before** closing, because the host drops a handle from its
//! table on close and the server's `retry-after` was being lost. A guard must not
//! close underneath that read. Two rows pin it: the throttle case still carries its
//! delay, and the double models the lifecycle (a header read after a close is
//! `NotFound`) so the ordering cannot pass by accident.
//!
//! Verifies: gh #577 (the mechanism), gh #52 and gh #278 (the ordering that must
//! survive), finding F1 of the #576/#577 cycle.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lca_protocol::{CapabilityError, ProviderCap};

/// What the fake answers on the status call.
#[derive(Clone, Copy)]
enum StatusCall {
    /// Answer this status.
    Answer(u16),
    /// Fail the call itself (the other early `?`).
    Fail,
}

/// A `ProviderCap` with one response: it answers a status, then serves at most one
/// body chunk, then fails or ends. It counts closes and models the host's handle
/// lifecycle, so a read after a close is `NotFound` - the only double shape that can
/// catch an ordering bug (gh #278's lesson).
struct CountingCap {
    status: StatusCall,
    body_error: Option<CapabilityError>,
    body: Mutex<Vec<Vec<u8>>>,
    closes: AtomicUsize,
    closed: AtomicBool,
    retry_after: Option<&'static str>,
}

impl CountingCap {
    fn new(status: StatusCall, body_error: Option<CapabilityError>) -> CountingCap {
        CountingCap {
            status,
            body_error,
            body: Mutex::new(vec![
                br#"{"type":"error","error":{"type":"overloaded_error","message":"try later"}}"#
                    .to_vec(),
            ]),
            closes: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            retry_after: None,
        }
    }

    fn throttled(retry_after: &'static str) -> CountingCap {
        CountingCap {
            retry_after: Some(retry_after),
            ..CountingCap::new(StatusCall::Answer(429), None)
        }
    }

    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl ProviderCap for CountingCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Ok(7)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        match self.status {
            StatusCall::Answer(status) => Ok(status),
            StatusCall::Fail => Err(CapabilityError::Timeout("status timed out".to_string())),
        }
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        if self.is_closed() {
            return Err(CapabilityError::NotFound("unknown handle 7".to_string()));
        }
        if let Some(err) = &self.body_error {
            return Err(err.clone());
        }
        let mut body = self.body.lock().expect("body");
        if body.is_empty() {
            return Ok(None);
        }
        Ok(Some(body.remove(0)))
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn net_response_headers(&self, _handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        // The host's behaviour: a closed handle is gone from the table.
        if self.is_closed() {
            return Err(CapabilityError::NotFound("unknown handle 7".to_string()));
        }
        Ok(self
            .retry_after
            .map(|value| vec![("retry-after".to_string(), value.to_string())])
            .unwrap_or_default())
    }

    fn credentials_get(&self, _key: &str) -> Option<String> {
        None
    }

    fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
        Ok(())
    }
}

// Verifies: gh #577's follow-up - **a failing status call closes the handle.**
//
// `net_response_status` is the first `?` after the request. On the pre-fix tree this
// row reads **zero** closes: `open` returns `Err` and no driver ever exists to run
// its `Drop`. That zero is the receipt for this cycle.
#[test]
fn a_failing_status_call_closes_the_response_handle() {
    let cap = CountingCap::new(StatusCall::Fail, None);
    let outcome = anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}");
    assert!(outcome.is_err(), "the status failure surfaces");
    assert_eq!(
        cap.closes(),
        1,
        "the handle acquired before the failure is closed exactly once"
    );
}

// Verifies: gh #577's follow-up - **an error while reading the error body closes it.**
//
// The 500 puts `open` on the non-2xx path; `net_read_body` then fails there, so the
// function returns before building the driver. Pre-fix: zero closes.
#[test]
fn an_error_during_the_error_body_read_closes_the_response_handle() {
    let cap = CountingCap::new(
        StatusCall::Answer(500),
        Some(CapabilityError::Timeout("read timed out".to_string())),
    );
    let outcome = anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}");
    match outcome {
        Err(failure) => assert!(
            failure.retryable,
            "a timeout is a transport failure a retry could heal: {failure:?}"
        ),
        Ok(_) => panic!("the 500 surfaces"),
    }
    assert_eq!(cap.closes(), 1, "closed exactly once, on the failure path");
}

// Verifies: gh #577's follow-up - **the control: a successful open closes exactly once,
// on drop.** The guard must not close twice (the manual close plus `Drop`), which a
// "closed at least once" assertion would happily accept.
#[test]
fn a_successful_open_closes_the_handle_exactly_once_on_drop() {
    let cap = CountingCap::new(StatusCall::Answer(200), None);
    let driver = anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}");
    let mut driver = driver.expect("a 200 opens");
    assert_eq!(cap.closes(), 0, "still owned while the driver is alive");
    // Drain it, so the body read also exercises the live handle path.
    while let Some(event) = driver.next_event() {
        if event.is_err() {
            break;
        }
    }
    drop(driver);
    assert_eq!(
        cap.closes(),
        1,
        "closed exactly once, when the driver drops"
    );
}

// Verifies: gh #278 (must not regress) - **the `retry-after` read still happens before
// the close.** The double answers `NotFound` for a header read on a closed handle, which
// is what the real host does; if the guard closed first, the delay would vanish from
// the message and this row would fail on its first assertion.
#[test]
fn the_retry_after_header_is_still_read_before_the_guard_closes() {
    let cap = CountingCap::throttled("7");
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 429 surfaces"),
    };
    assert!(err.retryable, "a 429 stays retryable");
    assert!(
        err.message.contains("retry-after: 7"),
        "the server delay survives the guard: {}",
        err.message
    );
    assert_eq!(cap.closes(), 1, "and the handle is closed exactly once");
}

// Verifies: gh #278 - **the non-2xx classification is unchanged.** The guard is a
// lifetime fix, not a behaviour change: a 500 still reads `transport` and retryable,
// an error body still shapes the message, and the close count stays one.
#[test]
fn a_server_error_still_reports_its_own_class() {
    let cap = CountingCap::new(StatusCall::Answer(500), None);
    let err = match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}") {
        Err(err) => err,
        Ok(_) => panic!("the 500 surfaces"),
    };
    assert_eq!(err.class, "transport", "a 5xx is a transport failure");
    assert!(err.retryable, "a 500 is worth retrying");
    assert!(
        err.message.contains("try later"),
        "the error body's own words ride the message: {}",
        err.message
    );
    assert_eq!(cap.closes(), 1);
}

// ---------------------------------------------------------------------------
// The shape, across every extension that opens a response
// ---------------------------------------------------------------------------

/// Every `.rs` file under `extensions/<name>/src`, in walk order.
///
/// `tests/` is skipped: a test double implements `net_close_response` on purpose,
/// and a fixture that *quotes* the pre-fix shape must not fail the row.
fn extension_sources() -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("extensions/");
    let mut out = Vec::new();
    walk(root, &mut out);
    out.retain(|path| !path.components().any(|part| part.as_os_str() == "tests"));
    out.sort();
    out
}

// Verifies: gh #577 (findings F1 and the six siblings) - **every extension that
// opens a response takes ownership of the handle on the line after the request.**
//
// The defect is a shape, not one call site: `net_request(..)?` followed by a
// fallible read, with the close left to a driver's `Drop` that does not exist yet.
// So the row walks the extension sources rather than asserting on one file - a
// new provider that copies the old shape fails here instead of leaking quietly.
//
// `.net_request_asking(` is excluded on purpose: `webfetch` and `conformance` ask
// the user for an ad-hoc grant and close through their own scoped path, which is a
// different contract (gh #302), and the pattern below does not match it.
#[test]
fn every_extension_that_opens_a_response_takes_the_guard_first() {
    /// How many lines after the request a guard may appear: the guard belongs on the
    /// next statement, and a little slack keeps this a rule about shape rather than
    /// formatting (`.net_request(..).map_err(..)?` spans three lines).
    const SLACK: usize = 6;

    let mut offenders = Vec::new();
    for path in extension_sources() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains(".net_request(") || line.contains(".net_request_asking(") {
                continue;
            }
            let window = &lines[index..lines.len().min(index + SLACK + 1)];
            if !window
                .iter()
                .any(|near| near.contains("ResponseGuard::new"))
            {
                offenders.push(format!(
                    "{}:{}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display(),
                    index + 1
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these response handles are opened without a guard, so an early error leaks \
         them: {offenders:?}"
    );
}
