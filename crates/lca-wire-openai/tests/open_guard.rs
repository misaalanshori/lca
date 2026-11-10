//! Gh #577: a host handle acquired from a capability and followed by
//! **fallible** calls must not leak when one of them fails.
//!
//! `ResponseStreamDriver::open` gets its handle from `net_request`, then
//! `?`s on `net_response_status` and, in the non-2xx path, on
//! `net_read_body` - both **before** the driver exists. `Drop` closes the
//! handle, but `Drop` only runs once there is a driver to drop, and the
//! non-2xx path closed manually on its success path only. So an `Err` in
//! either of those two places returned with the handle still open in the
//! capability engine's table: a leak per failed request, bounded only by
//! the process's lifetime.
//!
//! These rows drive the real driver over a fake `ProviderCap` and count
//! `net_close_response`, because "closed exactly once" is the property;
//! "closed at least once" would pass on a path that closes twice.
//!
//! Verifies: gh #577.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::Mutex;

use lca_protocol::{CapabilityError, ProviderCap};
use lca_wire_openai::ResponseStreamDriver;

/// What the fake does on the status call.
#[derive(Clone, Copy)]
enum StatusCall {
    Answer(u16),
    Fail,
}

/// A `ProviderCap` that answers one status, then fails or ends the body,
/// and **counts** how many times the response was closed.
struct CountingCap {
    status: StatusCall,
    body_error: Option<CapabilityError>,
    body: Mutex<Vec<Vec<u8>>>,
    closed: Mutex<usize>,
}

impl CountingCap {
    fn new(status: StatusCall, body_error: Option<CapabilityError>) -> CountingCap {
        CountingCap {
            status,
            body_error,
            body: Mutex::new(vec![b"{\"error\":{\"message\":\"bad\"}}".to_vec()]),
            closed: Mutex::new(0),
        }
    }

    fn closes(&self) -> usize {
        *self.closed.lock().expect("closed")
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
        *self.closed.lock().expect("closed") += 1;
        Ok(())
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

// Verifies: gh #577 - **an early error during open closes the handle.**
//
// The 500 puts the driver on the non-2xx path (which read the body and built
// a status failure), and `net_read_body` then fails there, so `open` returns
// `Err` before any driver exists. On the pre-fix tree `net_close_response`
// is called **zero** times - measured, that is the receipt for this row.
#[test]
fn early_error_during_open_closes_the_response_handle() {
    let cap = CountingCap::new(
        StatusCall::Answer(500),
        Some(CapabilityError::Timeout("read timed out".to_string())),
    );
    let outcome = ResponseStreamDriver::open(&cap, "https://x.test/v1", &[], b"{}", &|| {});
    match outcome {
        Err(failure) => assert!(
            failure.retryable,
            "a timeout is a transport failure a retry could heal: {failure:?}"
        ),
        Ok(_) => panic!("a 500 with a failing body read must not open a driver"),
    }
    assert_eq!(
        cap.closes(),
        1,
        "the handle acquired before the failure is closed exactly once, not leaked \
         (gh #577: it used to be closed zero times)"
    );
}

// Verifies: gh #577 - **a failure on the status call closes the handle too.**
//
// The other early `?` in `open`: `net_request` handed out a handle, and
// `net_response_status` failed. Nothing else in the function has run, so the
// guard is what closes it.
#[test]
fn a_failing_status_call_closes_the_response_handle() {
    let cap = CountingCap::new(StatusCall::Fail, None);
    let outcome = ResponseStreamDriver::open(&cap, "https://x.test/v1", &[], b"{}", &|| {});
    assert!(
        outcome.is_err(),
        "a failing status read cannot open a driver"
    );
    assert_eq!(
        cap.closes(),
        1,
        "the handle is closed exactly once when the status call fails (gh #577)"
    );
}

// Verifies: gh #577 - **the success path still closes exactly once**, so the
// guard did not double-close what the driver's `Drop` already handled.
#[test]
fn a_successful_open_closes_the_handle_exactly_once_on_drop() {
    let cap = CountingCap::new(StatusCall::Answer(200), None);
    let driver = ResponseStreamDriver::open(&cap, "https://x.test/v1", &[], b"{}", &|| {})
        .expect("a 200 body opens");
    assert_eq!(
        cap.closes(),
        0,
        "nothing is closed while the driver is alive"
    );
    drop(driver);
    assert_eq!(
        cap.closes(),
        1,
        "dropping the driver closes the handle exactly once (the guard does not \
         double-close)"
    );
}

// Verifies: gh #577 - **the non-2xx path reports a status failure, not a
// timeout, when the body read succeeds** - the shape the guard must not change.
#[test]
fn a_non_two_hundred_status_still_reports_its_own_class() {
    let cap = CountingCap::new(StatusCall::Answer(500), None);
    let failure = ResponseStreamDriver::open(&cap, "https://x.test/v1", &[], b"{}", &|| {})
        .err()
        .expect("a 500 is a failure");
    assert_eq!(failure.class, "transport", "{failure:?}");
    assert!(failure.retryable, "500 is retryable: {failure:?}");
    assert_eq!(
        cap.closes(),
        1,
        "and the handle is closed exactly once on the way out"
    );
}
