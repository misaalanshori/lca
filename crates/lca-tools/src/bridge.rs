//! One bridge shape for native handles: run blocking capability work on
//! the blocking pool while forwarding its stream pushes into the
//! caller's sink with real backpressure.

use std::sync::Arc;

use lca_protocol::{EventSink, StreamEvent};

/// Where the work half of [`bridge_stream`] pushes events.
struct Bridge(tokio::sync::mpsc::UnboundedSender<StreamEvent>);

impl EventSink for Bridge {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.send(event).is_ok()
    }
}

/// How a [`bridge_stream`] call failed.
#[derive(Debug)]
pub enum BridgeError<E> {
    /// The work itself answered with this error.
    Work(E),
    /// The work panicked on the blocking pool; the caller decides what
    /// that means for its extension (FR-EXT-3's rule for native handles
    /// is the caller's business, since native code cannot be isolated).
    Panicked,
}

/// Run `work` on a blocking thread, forwarding every `EventSink` push
/// it makes into `sink` until the work ends. This is the native twin of
/// the host's provider-stream bridge (ADR-0014's blocking-region rule):
/// dropping the returned future closes the bridge, `work` sees its
/// pushes fail, and the stream stops (FR-CONC-3).
pub async fn bridge_stream<E: Send + 'static>(
    work: impl FnOnce(Arc<dyn EventSink>) -> Result<(), E> + Send + 'static,
    sink: &dyn EventSink,
) -> Result<(), BridgeError<E>> {
    let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
    let mut join = tokio::task::spawn_blocking(move || work(Arc::new(Bridge(stx))));
    let mut joined: Option<Result<Result<(), E>, tokio::task::JoinError>> = None;
    loop {
        if joined.is_some() {
            while let Some(event) = srx.recv().await {
                let _ = sink.push(event);
            }
            break;
        }
        tokio::select! {
            maybe = srx.recv() => match maybe {
                None => break,
                Some(event) => if !sink.push(event) {
                    // Receiver gone: drain whatever the work still
                    // produces; its own push-false check ends the loop.
                    while srx.recv().await.is_some() {}
                    break;
                },
            },
            result = &mut join => joined = Some(result),
        }
    }
    // Exited on a closed bridge: the work is done, collect its answer
    // (it completed before the join handle was polled above).
    let joined = match joined {
        Some(joined) => joined,
        None => join.await,
    };
    match joined {
        Ok(result) => result.map_err(BridgeError::Work),
        Err(_) => Err(BridgeError::Panicked),
    }
}
