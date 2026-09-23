//! The provider-world adapter: a `provider`-world handle answers the
//! core's [`Provider`] trait, so the turn loop
//! never learns which world or delivery mode produced the stream
//! (ADR-0019's one-interface rule applied to the model backend).

use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;
use lca_protocol::{CompletionRequest, EventSink, ModelInfo, StreamEvent};
use lca_provider::{BoxFuture, EventSender, Provider, ProviderError};

/// Wraps one provider-world handle as the core's `Provider`.
pub struct ExtensionProvider {
    handle: Arc<dyn ExtensionDispatch>,
}

impl ExtensionProvider {
    /// Adapt a handle that implements the `provider` world.
    pub fn new(handle: Arc<dyn ExtensionDispatch>) -> ExtensionProvider {
        ExtensionProvider { handle }
    }
}

/// Mid-buffer between the handle's synchronous `EventSink` pushes and
/// the core's bounded channel: the async side below drains it with real
/// backpressure, so no push ever blocks or drops.
struct Mid(tokio::sync::mpsc::UnboundedSender<StreamEvent>);

impl EventSink for Mid {
    fn push(&self, event: StreamEvent) -> bool {
        self.0.send(event).is_ok()
    }
}

impl Provider for ExtensionProvider {
    fn name(&self) -> &str {
        self.handle.name()
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        // A handle that cannot enumerate answers with an error; the
        // picker falls back to the configured model (FR-PROV-2).
        self.handle.provider_models().unwrap_or_default()
    }

    fn stream(
        &self,
        request: CompletionRequest,
        tx: EventSender,
    ) -> BoxFuture<Result<(), ProviderError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
            let sink = Mid(stx);
            let answered = {
                let dispatch = handle.stream_completion(request, &sink);
                tokio::pin!(dispatch);
                let mut done = None;
                let mut core_gone = false;
                while done.is_none() {
                    tokio::select! {
                        maybe = srx.recv() => {
                            if let Some(event) = maybe {
                                // The core's receiver disappearing leaves
                                // the dispatch running to its end with
                                // events drained into the void; dropping
                                // this whole future cancels instead
                                // (FR-CONC-3), through the handle's own
                                // sink-closed path.
                                if !core_gone && tx.send(event).await.is_err() {
                                    core_gone = true;
                                }
                            }
                        }
                        result = &mut dispatch => done = Some(result),
                    }
                }
                // The loop exits only when the dispatch answered; dropping
                // it here closes the bridge so the drain below terminates.
                done.expect("the loop waits for the dispatch future")
            };
            drop(sink);
            while let Some(event) = srx.recv().await {
                if tx.send(event).await.is_err() {
                    while srx.recv().await.is_some() {}
                    break;
                }
            }
            answered.map_err(|err| ProviderError {
                message: err.to_string(),
                class: "invalid",
                retryable: false,
            })
        })
    }
}
