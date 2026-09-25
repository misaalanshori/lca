//! The pull-based stream driver: events leave as the host yields body
//! chunks, not after the whole response (`docs/deferred_workplan.md` C1).
//! Deterministic: a fake capability view scripts the chunks, so no network
//! and no timing are involved.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use lca_protocol::{CapabilityError, CompletionRequest, ProviderCap, StreamEvent};
use openai_compatible::{Settings, StreamDriver};

/// A capability view whose body arrives in scripted chunks and counts how
/// many chunks the driver asked for; `fail_after` can inject a read error.
struct ChunkedCap {
    chunks: Mutex<VecDeque<Option<Vec<u8>>>>,
    reads: AtomicUsize,
    fail_after: Option<usize>,
}

impl ProviderCap for ChunkedCap {
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
        Ok(200)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_after.is_some_and(|limit| read > limit) {
            return Err(CapabilityError::Io("connection reset".into()));
        }
        Ok(self.chunks.lock().unwrap().pop_front().flatten())
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
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

fn cap(chunks: Vec<Option<Vec<u8>>>) -> ChunkedCap {
    ChunkedCap {
        chunks: Mutex::new(VecDeque::from(chunks)),
        reads: AtomicUsize::new(0),
        fail_after: None,
    }
}

fn text_event(text: &str) -> Vec<u8> {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n").into_bytes()
}

#[test]
fn the_driver_yields_an_event_before_the_body_ends() {
    let cap = cap(vec![
        Some(text_event("Hello")),
        Some(text_event(", world")),
        Some(b"data: [DONE]\n\n".to_vec()),
        None,
    ]);
    let mut driver = StreamDriver::open(&cap, &Settings::default(), &CompletionRequest::default())
        .expect("open");

    let first = driver.next_event().expect("an event").expect("ok");
    assert_eq!(
        first,
        StreamEvent::TextDelta {
            delta: "Hello".into()
        }
    );
    // The point of C1: one chunk read got the first event out, the rest of
    // the body is still unread.
    assert_eq!(
        cap.reads.load(Ordering::SeqCst),
        1,
        "the first event did not drain the body"
    );

    let second = driver.next_event().expect("an event").expect("ok");
    assert_eq!(
        second,
        StreamEvent::TextDelta {
            delta: ", world".into()
        }
    );
    assert!(driver.next_event().is_none(), "the stream ends");
    assert!(driver.next_event().is_none(), "and stays ended");
}

#[test]
fn a_mid_stream_read_error_becomes_a_typed_failure() {
    let mut cap = cap(vec![Some(text_event("Hello"))]);
    cap.fail_after = Some(1);
    let mut driver = StreamDriver::open(&cap, &Settings::default(), &CompletionRequest::default())
        .expect("open");
    let _ = driver.next_event().expect("first event").expect("ok");
    let failure = driver
        .next_event()
        .expect("an error, not a silent end")
        .expect_err("the read error surfaces");
    assert!(
        failure.message.contains("connection reset"),
        "{}",
        failure.message
    );
    assert!(driver.next_event().is_none(), "the stream ends after it");
}
