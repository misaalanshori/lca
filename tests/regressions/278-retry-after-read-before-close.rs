//! GitHub #278: the `retry-after` header was read **after** the response
//! handle was closed, so it was never read at all.
//!
//! `extensions/anthropic` did this on a 429/529:
//!
//! ```rust
//! let _ = cap.net_close_response(handle);      // drops the response
//! ...
//! if matches!(status, 429 | 500 | 502 | 503 | 529)
//!     && let Ok(headers) = cap.net_response_headers(handle)   // NotFound
//! ```
//!
//! `net_close_response` removes the handle from the host's table, so
//! `net_response_headers` answers `NotFound` and the `&&` chain short-circuits.
//! The server's backoff intent was silently dropped and the host retried on
//! its own exponential schedule, ignoring what the server asked for.
//!
//! Two sibling extensions had the same class of defect, reached differently:
//!
//! * `openrouter` never read the headers at all on its chat error path, so a
//!   429's delay was lost whether or not the ordering was right;
//! * `openai-compatible`, the default provider, likewise never read them.
//!
//! All three now read the header while the handle is live, before the close,
//! and forward the value verbatim as the trailing `retry-after: <value>`
//! token the host's retry engine parses (gh #52). The host accepts seconds
//! (including a float) and an IMF-fixdate, and clamps the result with
//! `provider.retry_max_delay_ms` — so a hostile `retry-after: 86400` cannot
//! wedge a session, and that half needed no change.
//!
//! **Why the pre-existing anthropic test did not catch this:** its
//! `ThrottledCap` double returned headers unconditionally, so reading them
//! after a close still "worked". A double that cannot fail the way the real
//! host fails cannot catch an ordering bug. Every row here uses a double that
//! models the lifecycle — closing removes the response — which is the guard
//! that would have caught the defect.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #278,
//! gh #278 acceptance 1–3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{CapabilityError, CompletionRequest, ProviderCap};

/// One extension's HTTP surface under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Anthropic,
    OpenRouter,
    OpenAiCompatible,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Anthropic => "anthropic",
            Target::OpenRouter => "openrouter",
            Target::OpenAiCompatible => "openai-compatible",
        }
    }

    /// Drive this extension's error path; the `StreamFailure` message.
    fn open(self, cap: &LifecycleCap) -> String {
        match self {
            Target::Anthropic => {
                match anthropic::MessagesDriver::open(cap, "https://mock/v1/messages", &[], b"{}") {
                    Err(err) => err.message,
                    Ok(_) => panic!("anthropic: the 429 must surface"),
                }
            }
            Target::OpenRouter => match openrouter::ChatDriver::open(
                cap,
                "https://mock/v1/chat/completions",
                &[],
                b"{}",
            ) {
                Err(err) => err.message,
                Ok(_) => panic!("openrouter: the 429 must surface"),
            },
            Target::OpenAiCompatible => {
                let settings = openai_compatible::Settings {
                    model: "test-model".to_string(),
                    ..openai_compatible::Settings::default()
                };
                let request = CompletionRequest {
                    model: "test-model".to_string(),
                    ..CompletionRequest::default()
                };
                match openai_compatible::StreamDriver::open(cap, &settings, &request) {
                    Err(err) => err.message,
                    Ok(_) => panic!("openai-compatible: the 429 must surface"),
                }
            }
        }
    }
}

/// A capability that models the host's real handle lifecycle: `close`
/// removes the response, so a header read afterwards is `NotFound`.
///
/// This is the whole point of the guard. The shipped tests used doubles whose
/// `net_response_headers` answered unconditionally, so an ordering defect was
/// invisible to them.
struct LifecycleCap {
    reads: std::sync::Mutex<u32>,
    closed: std::sync::atomic::AtomicBool,
    retry_after: Option<&'static str>,
    status: u16,
    /// Records the order the extension called `headers`/`close`, so a row can
    /// assert on sequence rather than only on the outcome.
    order: std::sync::Mutex<Vec<&'static str>>,
}

impl LifecycleCap {
    fn new(status: u16, retry_after: Option<&'static str>) -> LifecycleCap {
        LifecycleCap {
            reads: std::sync::Mutex::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
            retry_after,
            status,
            order: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn throttled(retry_after: Option<&'static str>) -> LifecycleCap {
        LifecycleCap::new(429, retry_after)
    }

    fn calls(&self) -> Vec<&'static str> {
        self.order.lock().unwrap().clone()
    }
}

impl ProviderCap for LifecycleCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Ok(self.status)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut reads = self.reads.lock().unwrap();
        *reads += 1;
        if *reads == 1 {
            Ok(Some(
                br#"{"type":"error","error":{"message":"rate limited"}}"#.to_vec(),
            ))
        } else {
            Ok(None)
        }
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        self.order.lock().unwrap().push("close");
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn net_response_headers(&self, _handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        self.order.lock().unwrap().push("headers");
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CapabilityError::NotFound("unknown handle 1".into()));
        }
        Ok(self
            .retry_after
            .map(|value| vec![("retry-after".to_string(), value.to_string())])
            .unwrap_or_default())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        (key == "access").then(|| "sk-or-mock".to_string())
    }

    fn credentials_set(&self, _key: &str, _value: &str) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_delete(&self, _key: &str) -> Result<(), CapabilityError> {
        Ok(())
    }
}

/// The three extensions that had the defect. Listed once so a fourth cannot be
/// fixed without a row, and so the guard says which surfaces it covers.
const TARGETS: [Target; 3] = [
    Target::Anthropic,
    Target::OpenRouter,
    Target::OpenAiCompatible,
];

/// Acceptance 1, per extension: a 429 carrying `retry-after` reaches the error
/// text, so the host's retry engine schedules the server's delay.
#[test]
fn every_provider_forwards_a_throttles_retry_after() {
    for target in TARGETS {
        let cap = LifecycleCap::throttled(Some("7"));
        let message = target.open(&cap);
        assert!(
            message.contains("retry-after: 7"),
            "{}: the server's delay must ride the message: {message}",
            target.name()
        );
    }
}

/// Acceptance 2 and the ordering itself: headers are read **before** the close
/// on every one of them. Asserted on the call sequence, so a future refactor
/// that happens to keep the value by other means still has to satisfy it.
#[test]
fn every_provider_reads_headers_before_closing() {
    for target in TARGETS {
        let cap = LifecycleCap::throttled(Some("7"));
        let _ = target.open(&cap);
        let calls = cap.calls();
        let headers_at = calls.iter().position(|call| *call == "headers");
        let close_at = calls.iter().position(|call| *call == "close");
        assert_eq!(
            (headers_at, close_at),
            (Some(0), Some(1)),
            "{}: headers must be read, then the handle closed: {calls:?}",
            target.name()
        );
    }
}

/// Acceptance 2: the handle is still closed. Reading headers first must not
/// leak the response — the close is mandatory, not an optimisation.
#[test]
fn every_provider_still_closes_the_handle() {
    for target in TARGETS {
        let cap = LifecycleCap::throttled(Some("7"));
        let _ = target.open(&cap);
        assert!(
            cap.closed.load(std::sync::atomic::Ordering::SeqCst),
            "{}: the response handle is closed on the error path",
            target.name()
        );
    }
}

/// Design rule 3: no header means no token, so the host falls back to its
/// exponential schedule. Never a zero delay — a garbled or absent header must
/// not become "retry immediately".
#[test]
fn a_throttle_without_the_header_carries_no_delay() {
    for target in TARGETS {
        let cap = LifecycleCap::throttled(None);
        let message = target.open(&cap);
        assert!(
            !message.contains("retry-after"),
            "{}: no header, no token: {message}",
            target.name()
        );
    }
}

/// The value is forwarded verbatim, including the HTTP-date form the host's
/// parser understands (pi accepts both shapes). The extension must not try to
/// interpret it: `retry-after-ms` style local maths is the host's job.
#[test]
fn an_http_date_form_is_forwarded_not_reinterpreted() {
    let date = "Wed, 21 Oct 2026 07:28:00 GMT";
    for target in TARGETS {
        let cap = LifecycleCap::throttled(Some(date));
        let message = target.open(&cap);
        assert!(
            message.contains(&format!("retry-after: {date}")),
            "{}: the date rides verbatim: {message}",
            target.name()
        );
    }
}

/// Design rule 4's input half: an absurd value is passed through unchanged and
/// the **host** clamps it with `provider.retry_max_delay_ms`
/// (`crates/lca-core/src/turn/stream.rs`, `delay.min(retry_max_delay)`). The
/// extension must not pre-clamp, or the configured cap would stop being the one
/// knob.
#[test]
fn an_absurd_delay_is_forwarded_for_the_host_to_clamp() {
    for target in TARGETS {
        let cap = LifecycleCap::throttled(Some("86400"));
        let message = target.open(&cap);
        assert!(
            message.contains("retry-after: 86400"),
            "{}: the raw value reaches the host, which clamps it: {message}",
            target.name()
        );
    }
}

/// The positive control: a success response is untouched. A non-error status
/// must not read headers at all (nothing needs them) and must not gain a
/// `retry-after` token.
#[test]
fn a_successful_status_is_unchanged() {
    for target in TARGETS {
        let cap = LifecycleCap::new(200, Some("7"));
        match target {
            Target::Anthropic => {
                let driver =
                    anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}")
                        .expect("a 200 opens");
                drop(driver);
            }
            Target::OpenRouter => {
                let driver = openrouter::ChatDriver::open(
                    &cap,
                    "https://mock/v1/chat/completions",
                    &[],
                    b"{}",
                )
                .expect("a 200 opens");
                drop(driver);
            }
            Target::OpenAiCompatible => {
                let settings = openai_compatible::Settings {
                    model: "test-model".to_string(),
                    ..openai_compatible::Settings::default()
                };
                let request = CompletionRequest {
                    model: "test-model".to_string(),
                    ..CompletionRequest::default()
                };
                let driver = openai_compatible::StreamDriver::open(&cap, &settings, &request)
                    .expect("a 200 opens");
                drop(driver);
            }
        }
        assert!(
            !cap.calls().contains(&"headers"),
            "{}: a success path needs no header read",
            target.name()
        );
    }
}

/// A 401 is an auth failure, not a throttle: it must not carry a delay token
/// even when the server sent one, because the correct answer is to
/// re-authenticate, not to wait.
#[test]
fn an_auth_failure_carries_no_delay() {
    for target in TARGETS {
        let cap = LifecycleCap::new(401, Some("7"));
        let message = match target {
            Target::Anthropic => {
                match anthropic::MessagesDriver::open(&cap, "https://mock/v1/messages", &[], b"{}")
                {
                    Err(err) => err.message,
                    Ok(_) => panic!("anthropic: the 401 surfaces"),
                }
            }
            Target::OpenRouter => match openrouter::ChatDriver::open(
                &cap,
                "https://mock/v1/chat/completions",
                &[],
                b"{}",
            ) {
                Err(err) => err.message,
                Ok(_) => panic!("openrouter: the 401 surfaces"),
            },
            Target::OpenAiCompatible => {
                let settings = openai_compatible::Settings {
                    model: "test-model".to_string(),
                    ..openai_compatible::Settings::default()
                };
                let request = CompletionRequest {
                    model: "test-model".to_string(),
                    ..CompletionRequest::default()
                };
                match openai_compatible::StreamDriver::open(&cap, &settings, &request) {
                    Err(err) => err.message,
                    Ok(_) => panic!("openai-compatible: the 401 surfaces"),
                }
            }
        };
        assert!(
            !message.contains("retry-after: 7"),
            "{}: an auth failure is not a throttle: {message}",
            target.name()
        );
    }
}
