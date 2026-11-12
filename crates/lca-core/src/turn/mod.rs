//! The turn loop (S2): one model call per round, tools run sequentially
//! between rounds, cancellation that keeps every record already written.
//! Split out of `lib.rs`; the loop body reads as the calls in order
//! (steer drain, assemble, provider call, persist, tool dispatch).
//!
//! Gh #548 phase 2 partitioned the file along its own seams, and renames
//! nothing a caller can see - `lib.rs` is unchanged and an inherent `impl` may
//! be spread across files, so every method keeps the path it had:
//!
//! | file | what it owns |
//! |---|---|
//! | this file | the loop, the cancellation and steer-queue endings, the call's accumulated shape (`CallResponse`/`CallFail`), and the window/usage helpers the front ends share |
//! | [`execute`] | one tool call through mutation, verdict, permission, dispatch, composition |
//! | [`stream`] | one provider attempt's events, accumulated and replayed |
//! | `request` | assembling one request: transform chain, skills merge, the three stable-prefix steps, and the protocol refusal |
//! | `persist` | writing the response and the compaction decisions around it |
//!
//! [`turn_body`](Agent::turn_body) is **the loop**, deliberately one function.
//! Its phases share long-lived state across `await` points - the round
//! counters, the nested-completion server, `active_seen`, the cancellation
//! flag - so extracting them into named functions is a change to the hot path
//! (each boundary needs its own behaviour-pinned rows and an order mutation),
//! not a move. It is a separate change with its own bar; do not split it while
//! moving code.

use lca_protocol::{
    FORMAT_VERSION, Record, StopReason, ToolCall, TurnEvent, TurnOutcome, TurnStatus, Usage,
};

pub(super) mod execute;
pub(super) mod stream;
// Plain `mod`, not `pub(super)`: the two boundaries are this module's alone, and
// the structural guards reach them through the ordinary `mod` syntax that
// `crate::module_sources` walks (#548 phase 2).
mod persist;
mod request;
use self::execute::{NestedGuard, NestedServer};
use lca_provider::ProtocolError;
use lca_session::ViewMode;
use lca_tools::CancelFlag;

use super::{Agent, TurnSink};

/// One completion call's accumulated result.
pub(super) struct CallResponse {
    pub(super) text: String,
    pub(super) reasoning: Option<String>,
    /// The latest thinking signature the stream carried (gh #41).
    pub(super) signature: Option<String>,
    pub(super) calls: Vec<ToolCall>,
    pub(super) protocol_errors: Vec<ProtocolError>,
    pub(super) usage: Usage,
    /// The provider reported that it hit its output-token limit (gh #390 gap 1),
    /// so this response is a prefix of what the model was producing.
    pub(super) truncated: bool,
}

/// Why one completion call did not produce a response.
pub(super) enum CallFail {
    Cancelled,
    Provider {
        message: String,
        class: String,
        retryable: bool,
    },
}

impl Agent<'_> {
    /// The loop skeleton: cancel check, steer drain, assemble, call,
    /// persist, tool rounds.
    pub(super) async fn turn_body(
        &mut self,
        input: &str,
        queue: Option<lca_protocol::SubmitMode>,
        attachments: &[String],
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> TurnOutcome {
        let ts = lca_session::now_ms();
        let turn_record_id = lca_session::new_record_id();
        if let Err(err) = self.store.append(
            self.session,
            Record::User {
                v: FORMAT_VERSION,
                ts,
                id: turn_record_id.clone(),
                parent: None,
                content: input.to_string(),
                attachments: attachments.to_vec(),
                // ADR-0038's marker: the record says how the message was
                // submitted, wherever it finally lands in the log.
                queue: queue.map(|mode| mode.marker().to_string()),
            },
        ) {
            return self.fail(
                StopReason::Error,
                format!("cannot write to the session log: {err}"),
            );
        }

        // The run boundaries headless `--mode json` and `--mode rpc`
        // reconstruct from (pi's agent/turn/message taxonomy, our names).
        sink.on_event(TurnEvent::TurnStarted);
        sink.on_event(TurnEvent::MessageStarted { role: "user" });
        sink.on_event(TurnEvent::MessageEnded { role: "user" });

        // `pre-turn` fires once, after the user record and before any provider
        // or compaction work (SRDD hook points; `docs/flows.md`).
        self.config.extensions.on_pre_turn().await;

        // Gh #77's nested server lives for exactly this turn: the
        // `tools` import sends here from blocking threads while the
        // turn serves on its own task. The guard clears the slot on
        // every exit, so no request outlives its turn.
        let (nested_tx, nested_rx) = tokio::sync::mpsc::unbounded_channel();
        self.config.extensions.install_nested(nested_tx);
        // Gh #77's registry surface reaches every handle before the
        // first provider call, so the `tools` import serves from any
        // thread the turn's tools run on.
        for handle in self.config.extensions.handles() {
            handle.set_tools_view(self.config.extensions.as_tools_view());
        }
        // #49: the turn's nested-completion backend reaches every
        // handle with an empty completion slot (compaction keeps its
        // own); per-call and turn-end drains below attribute its
        // usage to this turn's cost.
        if let Some(backend) = &self.nested_backend {
            let shared: std::sync::Arc<dyn std::any::Any + Send + Sync> =
                std::sync::Arc::new(lca_tools::SharedBackend(backend.clone()));
            for handle in self.config.extensions.handles() {
                handle.set_completion_backend(&shared);
            }
        }
        let _nested_guard = NestedGuard {
            registry: self.config.extensions.clone(),
        };
        let mut nested = NestedServer {
            rx: nested_rx,
            counts: std::collections::HashMap::new(),
            records: std::collections::HashMap::new(),
        };
        // The active set this turn has recorded: the first request
        // carries the opening set implicitly (the declaration
        // itself); only mid-turn changes write entries (gh #77).
        let mut active_seen = self.config.extensions.active_revision();
        // Gh #45's settle summary counts rounds and tool calls.
        let mut tool_calls = 0u32;
        let mut settle_continued = false;

        let mut turn_usage = Usage::default();
        let mut rounds = 0u32;
        // Gh #36 phase 3: one overflow recovery per turn - compact and
        // retry once, then surface if still capped.
        let mut overflow_recovered = false;
        loop {
            if cancel.is_cancelled() {
                return self.cancelled(turn_usage, sink);
            }

            // ADR-0038: steered messages join the turn's input at this
            // model-call boundary (never mid-stream).
            if let Err(outcome) = self.drain_steer(sink) {
                return outcome;
            }

            let request = match self
                .assemble_request(input, &turn_record_id, rounds == 0, sink, &mut active_seen)
                .await
            {
                Ok(request) => request,
                Err(outcome) => return outcome,
            };

            let mut response = match self.provider_call(request, sink, cancel).await {
                Ok(response) => response,
                Err(CallFail::Cancelled) => return self.cancelled(turn_usage, sink),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
                    // Gh #36 phase 3: the error is the signal - a capped
                    // generation compacts and retries. The partial attempt
                    // text is not persisted (LCA never persists failed
                    // attempts). A failed recovery compacts nothing and
                    // retries nothing: the error surfaces below, once.
                    //
                    // Gh #277: nothing is emitted here. This branch used to
                    // send `Error` and `TurnEnded` before attempting the
                    // recovery, so a *recovered* turn carried on streaming
                    // after `TurnEnded` and an unrecovered one emitted both
                    // events twice. The recovery is marked by
                    // `CompactionStarted`/`CompactionEnded { reason:
                    // "overflow" }`, which `compact_before_turn` emits; the
                    // turn's end is marked below, when the turn really ends.
                    if !overflow_recovered && super::compact::is_overflow_error(&message) {
                        overflow_recovered = true;
                        let records = match self.store.read_with(self.session, ViewMode::Display) {
                            Ok(outcome) => outcome.records,
                            Err(_) => {
                                return self.fail(
                                    StopReason::Error,
                                    format!(
                                        "cannot re-read the log after the capped turn: {message}"
                                    ),
                                );
                            }
                        };
                        if self
                            .compact_before_turn(&records, &turn_record_id, sink, "overflow")
                            .await
                        {
                            continue;
                        }
                    }
                    sink.on_event(TurnEvent::Error {
                        message: message.clone(),
                        class: class.clone(),
                        retryable,
                    });
                    sink.on_event(TurnEvent::TurnEnded {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                    });
                    return TurnOutcome {
                        status: TurnStatus::Error,
                        stop_reason: StopReason::Error,
                        usage: turn_usage,
                        error: Some(message),
                    };
                }
            };

            // Gh #125/#260: a provider that reports no cost gets the model
            // catalog's math; a reported cost is never overwritten, and a
            // model the catalog does not list stays tokens-only.
            if response.usage.cost == 0.0
                && let Some(rates) =
                    super::pricing::rates_for(&self.config.provider, &self.config.model)
            {
                // Gh #274: the four buckets are persisted alongside the
                // total, because `lca_session::cache` attributes a cache
                // miss to a dollar figure per bucket (ADR-0017). With only
                // `cost` set, that attribution was always `$0.00`.
                let priced = super::pricing::PricedUsage::new(rates, &response.usage);
                response.usage.cost = priced.total;
                response.usage.cost_input = priced.cost_input;
                response.usage.cost_cache_read = priced.cost_cache_read;
                response.usage.cost_cache_write = priced.cost_cache_write;
            }
            accumulate_usage(&mut turn_usage, &response.usage);
            // Gh #390 gap 1: **a truncation outranks everything else**, and it
            // ends the turn here - before any tool call runs.
            //
            // Two things were wrong without this. The half-streamed call's
            // arguments are a JSON prefix (`{"path": "a.txt", "content": "hel`),
            // so running it either fails on the parse or, worse, runs against a
            // value the model never finished writing; and because nothing marked
            // the response as cut, the loop re-requested and re-attempted the
            // same incomplete call - a live session showed the same
            // `write a.txt error` card repeating until it was interrupted.
            //
            // It also comes *before* `reject_incomplete`: the open-call protocol
            // error is a *consequence* of the cut, not an independent fault, and
            // reporting both would be the double-report this issue warns about.
            if response.truncated {
                if let Err(outcome) = self.persist_response(&response, sink).await {
                    return outcome;
                }
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Length,
                });
                return TurnOutcome {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Length,
                    usage: turn_usage,
                    error: None,
                };
            }
            if let Err(outcome) = self.reject_incomplete(&response, sink, &turn_usage) {
                return outcome;
            }
            if let Err(outcome) = self.persist_response(&response, sink).await {
                return outcome;
            }

            if response.calls.is_empty() {
                // Gh #45's actionable settle: `turn_end` handlers run
                // first, then `agent_before_settle` gets the last word.
                // Appends inject as context messages the next request
                // sees; one continuation runs one more request, then the
                // turn settles regardless (a handler cannot loop it).
                if !settle_continued {
                    let mut decision = self
                        .config
                        .extensions
                        .settle_phase(false, rounds, tool_calls, "ok")
                        .await;
                    let late = self
                        .config
                        .extensions
                        .settle_phase(true, rounds, tool_calls, "ok")
                        .await;
                    crate::registry::ExtensionRegistry::merge_append(&mut decision, late.append);
                    decision.continue_once |= late.continue_once;
                    if let Some(append) = decision.append
                        && let Err(err) = self.store.append(
                            self.session,
                            Record::CustomMessage {
                                v: FORMAT_VERSION,
                                ts: lca_session::now_ms(),
                                id: lca_session::new_record_id(),
                                parent: None,
                                custom_type: "settle-append".to_string(),
                                content: append,
                                display: false,
                                details: None,
                            },
                        )
                    {
                        return self.fail(
                            StopReason::Error,
                            format!("cannot record the settle append: {err}"),
                        );
                    }
                    if decision.continue_once {
                        settle_continued = true;
                        continue;
                    }
                }
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Stop,
                });
                return TurnOutcome {
                    status: TurnStatus::Ok,
                    stop_reason: StopReason::Stop,
                    usage: turn_usage,
                    error: None,
                };
            }

            // FR-CORE-9: bound the tool rounds. `0` (the default since
            // 2026-10-02) means unlimited - the mechanism is unchanged and
            // a configured value still stops the turn here.
            rounds += 1;
            if self.config.max_iterations > 0 && rounds > self.config.max_iterations {
                let message = format!(
                    "iteration limit of {} tool-call rounds reached; send another message to continue",
                    self.config.max_iterations
                );
                sink.on_event(TurnEvent::Error {
                    message: message.clone(),
                    class: "iteration-limit".to_string(),
                    retryable: false,
                });
                sink.on_event(TurnEvent::TurnEnded {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::IterationLimit,
                });
                return TurnOutcome {
                    status: TurnStatus::Error,
                    stop_reason: StopReason::IterationLimit,
                    usage: turn_usage,
                    error: Some(message),
                };
            }

            // Sequential execution inside one turn (FR-CONC-2).
            for call in &response.calls {
                tool_calls += 1;
                if let Err(outcome) = self
                    .run_tool_call(call, sink, cancel, &mut nested, &mut turn_usage)
                    .await
                {
                    return outcome;
                }
                if cancel.is_cancelled() {
                    return self.cancelled(turn_usage, sink);
                }
            }
        }
    }

    /// The cancellation exit: the turn ends `Ok` with `Cancelled`, keeping
    /// every record already written.
    fn cancelled(&self, usage: Usage, sink: &mut dyn TurnSink) -> TurnOutcome {
        sink.on_event(TurnEvent::TurnEnded {
            status: TurnStatus::Ok,
            stop_reason: StopReason::Cancelled,
        });
        TurnOutcome {
            status: TurnStatus::Ok,
            stop_reason: StopReason::Cancelled,
            usage,
            error: None,
        }
    }

    /// Drain the steering queue into user records at this boundary
    /// (ADR-0038): each steered message extends the message list the way any
    /// user message does, so the stable cache prefix keeps its value.
    /// Gh #76: `OneAtATime` drains the first waiting message and leaves
    /// the rest queued, in order; `All` flushes everything (historical).
    #[allow(clippy::result_large_err)]
    fn drain_steer(&self, sink: &mut dyn TurnSink) -> Result<(), TurnOutcome> {
        let steered: Vec<lca_protocol::QueuedMessage> = {
            let mut queue = super::lock(&self.config.steer);
            match self.config.steering_mode {
                lca_protocol::QueueMode::All => std::mem::take(&mut *queue),
                lca_protocol::QueueMode::OneAtATime => {
                    if queue.is_empty() {
                        Vec::new()
                    } else {
                        vec![queue.remove(0)]
                    }
                }
            }
        };
        for message in steered {
            let marker = message.mode.marker().to_string();
            // Gh #258: the drain (counts and markers, never text).
            lca_debug::core::event("steer.drain", &format!("marker={marker}"));
            if let Err(err) = self.store.append(
                self.session,
                Record::User {
                    v: FORMAT_VERSION,
                    ts: lca_session::now_ms(),
                    id: lca_session::new_record_id(),
                    parent: None,
                    content: message.text.clone(),
                    attachments: Vec::new(),
                    queue: Some(marker.clone()),
                },
            ) {
                return Err(self.fail(
                    StopReason::Error,
                    format!("cannot write to the session log: {err}"),
                ));
            }
            sink.on_event(TurnEvent::UserInjected {
                text: message.text,
                mode: marker,
            });
        }
        Ok(())
    }
}

/// The window the compaction ratio uses: the model's published window when
/// one is known, else a conservative estimate.
///
/// An endpoint that publishes no context window (`opencode-go`'s `/models`
/// carries none) would otherwise never cross a threshold, and a long session
/// would grow until the provider rejects it. The estimate is a safety net,
/// not a claim: the footer still shows `ctx ?` when the window is unknown,
/// and `OPENAI_CONTEXT_WINDOW` (or a provider that reports one) overrides it.
const FALLBACK_CONTEXT_WINDOW: u32 = 128_000;

/// The effective window, shared with the front ends (gh #36 phase 1):
/// they derive the summarization budget from the same window the
/// trigger uses.
///
/// gh #260: this stays the two-argument form it always was, so no caller
/// changes shape. A model whose window the front end could not resolve keeps
/// the estimate here; the catalog fills that gap where the front end asks it
/// (see [`catalog_context_window`]).
pub fn effective_context_window(configured: u32) -> u32 {
    if configured > 0 {
        configured
    } else {
        FALLBACK_CONTEXT_WINDOW
    }
}

/// The catalog's context window for `provider`/`model`, when the model catalog
/// knows one (gh #260).
///
/// The front end resolves a window from the provider's own model listing first
/// and falls back here, so a provider that publishes none (an OpenAI-shaped
/// endpoint answering without `limit`) still gets the real number rather than
/// the footer's `ctx ?` and the compaction safety net.
///
/// **Not called from the turn loop.** The first lookup inflates and parses
/// ~1.5 MB (measured at ~460 ms), which inside a turn would sit in the
/// cancellation window and change what a cancel landing mid-turn interrupts.
/// The turn loop therefore keeps its cheap configured-or-fallback window, and
/// this is exposed for the front ends, which resolve once when the model is
/// chosen. Wiring it into `extensions/openai-compatible`'s window resolution
/// is the remaining step (see the cycle report).
#[must_use]
pub fn catalog_context_window(provider: &str, model: &str) -> Option<u32> {
    lca_registry::models::lookup(provider, model)?.context_window()
}

fn accumulate_usage(total: &mut Usage, per_call: &Usage) {
    total.input = total.input.saturating_add(per_call.input);
    total.output = total.output.saturating_add(per_call.output);
    total.cache_read = total.cache_read.saturating_add(per_call.cache_read);
    total.cache_write = total.cache_write.saturating_add(per_call.cache_write);
    total.cache_write_1h = total.cache_write_1h.saturating_add(per_call.cache_write_1h);
    total.cost += per_call.cost;
}

#[cfg(test)]
mod window_tests {
    use super::*;

    // A published window is used as-is; an unknown one falls back to a
    // conservative estimate so compaction still runs.
    #[test]
    fn an_unknown_window_still_compacts() {
        assert_eq!(effective_context_window(1_000_000), 1_000_000);
        assert_eq!(effective_context_window(0), FALLBACK_CONTEXT_WINDOW);
    }

    // Verifies: gh #260 - the catalog supplies the window a provider did not
    // publish, and an unknown model still reads as unknown (never a guess).
    #[test]
    fn the_catalog_supplies_an_unpublished_window() {
        assert_eq!(catalog_context_window("openai", "gpt-4o"), Some(128_000));
        assert_eq!(catalog_context_window("openai", "not-a-real-model"), None);
    }
}
