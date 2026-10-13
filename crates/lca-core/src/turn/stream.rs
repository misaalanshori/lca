//! Provider streaming (gh #45's observation buffer): one attempt's
//! events through the accumulator, replayed to `hooks-stream` in order
//! once the stream closes. Split from `turn/mod.rs` for the workspace
//! file ceiling (gate 11).

use lca_protocol::{StreamEvent, TurnEvent, Usage};
use lca_provider::{CompletionRequest, ToolCallAccumulator};
use lca_tools::CancelFlag;

use super::{CallFail, CallResponse};
use crate::{Agent, TurnSink};

impl Agent<'_> {
    /// Replay one closed stream to `hooks-stream` observers (gh #45),
    /// in arrival order, labeled with the producing provider+model.
    /// Observation only: nothing here steers the turn.
    async fn replay_stream(&self, model: &str, events: Vec<StreamEvent>) {
        for event in &events {
            let (kind, data) = match event {
                StreamEvent::TextDelta { delta } => ("text-delta", delta.clone()),
                StreamEvent::ReasoningDelta { delta } => ("reasoning-delta", delta.clone()),
                StreamEvent::ToolCallStart { call_id, name } => {
                    ("tool-call-start", format!("{name} {call_id}"))
                }
                StreamEvent::ToolCallArgDelta { call_id, delta } => {
                    ("tool-call-arg-delta", format!("{call_id} {delta}"))
                }
                StreamEvent::ToolCallEnd { call_id } => ("tool-call-end", call_id.clone()),
                StreamEvent::Usage { usage } => (
                    "usage",
                    format!("input {} output {}", usage.input, usage.output),
                ),
                StreamEvent::Error { message, .. } => ("error", message.clone()),
                StreamEvent::VendorEvent { kind, payload } => {
                    ("vendor-event", format!("{kind} {payload}"))
                }
            };
            self.config
                .extensions
                .observe_stream_event(&self.config.provider, model, kind, &data)
                .await;
        }
    }

    /// One completion call, with retry (FR-CORE-6) and cancellation
    /// (FR-CONC-3: dropping the producer stops the in-flight stream).
    ///
    /// Gh #52 (pi's `getRetryDelayMs`): a server-requested delay rides
    /// the error text as a `retry-after` / `retry-after-ms` token the
    /// provider guests append from the HTTP response headers; the sleep
    /// honors it, clamped to `retry_max_delay`. No token (or an
    /// unparseable one) falls back to exponential backoff.
    /// ponytail: deferred-response handles (pi's poll-a-response-id
    /// later) are a noted deferral, not a gap here: they need a new
    /// provider-world export plus a polling loop, and the 0.6 freeze
    /// forbids the export. Retry-After plus the cap cover the
    /// throttle case today.
    pub(super) async fn provider_call(
        &self,
        request: CompletionRequest,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<CallResponse, CallFail> {
        let mut attempt = 0u32;
        loop {
            match self.stream_once(request.clone(), sink, cancel).await {
                Ok(response) => {
                    if attempt > 0 {
                        sink.on_event(TurnEvent::RetryFinished { success: true });
                    }
                    return Ok(response);
                }
                Err(CallFail::Cancelled) => return Err(CallFail::Cancelled),
                Err(CallFail::Provider {
                    message,
                    class,
                    retryable,
                }) => {
                    // Gh #202: capacity names retry even when the
                    // provider flagged the failure non-retryable.
                    let retry = retryable || lca_protocol::is_capacity_error(&message);
                    if retry && attempt < self.config.retry_limit {
                        let backoff = self
                            .config
                            .retry_base_delay
                            .saturating_mul(1u32 << attempt.min(6));
                        let mut delay = retry_after_delay_ms(&message, lca_session::now_ms())
                            .unwrap_or(backoff);
                        // Gh #52: the max-delay cap (pi's
                        // `maxAgentDelayMs`); zero disables it.
                        if !self.config.retry_max_delay.is_zero() {
                            delay = delay.min(self.config.retry_max_delay);
                        }
                        sink.on_event(TurnEvent::RetryScheduled {
                            attempt: attempt + 1,
                            max: self.config.retry_limit,
                            delay_ms: delay.as_millis() as u64,
                            error: message.clone(),
                        });
                        // Gh #258: the backoff (class and delay, never
                        // the error text — providers echo requests).
                        lca_debug::core::event(
                            "retry.attempt",
                            &format!(
                                "attempt={} max={} delay_ms={} retryable={retry}",
                                attempt + 1,
                                self.config.retry_limit,
                                delay.as_millis()
                            ),
                        );
                        if !delay.is_zero() {
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                _ = cancel.wait_cancelled() => return Err(CallFail::Cancelled),
                            }
                        } else if cancel.is_cancelled() {
                            return Err(CallFail::Cancelled);
                        }
                        attempt += 1;
                        continue;
                    }
                    // FR-CORE-7: the user sees that retries were tried and
                    // gave up, not just the raw transport error. Any
                    // attempt past zero scheduled a retry (by flag or by
                    // the gh #202 capacity override), so it owns the
                    // finish event and the exhausted note either way.
                    let message = if attempt > 0 {
                        sink.on_event(TurnEvent::RetryFinished { success: false });
                        format!("{message} (retries exhausted after {attempt})")
                    } else {
                        message
                    };
                    return Err(CallFail::Provider {
                        message,
                        class,
                        retryable,
                    });
                }
            }
        }
    }

    async fn stream_once(
        &self,
        request: CompletionRequest,
        sink: &mut dyn TurnSink,
        cancel: &CancelFlag,
    ) -> Result<CallResponse, CallFail> {
        sink.on_event(TurnEvent::MessageStarted { role: "assistant" });
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        // The replay labels events with the producing model; the
        // request itself moves into the producer below.
        let model = request.model.clone();
        let producer = self.provider.stream(request, tx);
        tokio::pin!(producer);

        let mut text = String::new();
        let mut reasoning = String::new();
        // gh #41: the latest thinking signature (one reasoning run
        // carries one; a later block replaces an earlier one, matching
        // the transcript order replay resends).
        let mut signature: Option<String> = None;
        let mut acc = ToolCallAccumulator::default();
        let mut usage = Usage::default();
        let mut failure: Option<(String, &'static str, bool)> = None;
        // Gh #45's stream observation buffers the normalized events
        // and replays them in order once the stream closes:
        // observation never steers or stalls a live stream.
        let mut stream_log: Vec<StreamEvent> = Vec::new();
        let mut natural_end = false;

        loop {
            tokio::select! {
                biased;
                _ = cancel.wait_cancelled() => return Err(CallFail::Cancelled),
                item = rx.recv() => match item {
                    Some(StreamEvent::TextDelta { delta }) => {
                        stream_log.push(StreamEvent::TextDelta { delta: delta.clone() });
                        text.push_str(&delta);
                        sink.on_event(TurnEvent::TextDelta(delta));
                    }
                    Some(StreamEvent::ReasoningDelta { delta }) => {
                        stream_log.push(StreamEvent::ReasoningDelta { delta: delta.clone() });
                        reasoning.push_str(&delta);
                        sink.on_event(TurnEvent::ReasoningDelta(delta));
                    }
                    Some(event @ (StreamEvent::ToolCallStart { .. }
                    | StreamEvent::ToolCallArgDelta { .. }
                    | StreamEvent::ToolCallEnd { .. })) => {
                        stream_log.push(event.clone());
                        acc.handle(event)
                    }
                    Some(StreamEvent::VendorEvent { kind, payload })
                        if kind == lca_protocol::THINKING_SIGNATURE_KIND =>
                    {
                        stream_log.push(StreamEvent::VendorEvent {
                            kind: kind.clone(),
                            payload: payload.clone(),
                        });
                        note_signature(&mut signature, &payload);
                    }
                    Some(StreamEvent::Usage { usage: reported }) => {
                        stream_log.push(StreamEvent::Usage { usage: reported.clone() });
                        usage = reported;
                    }
                    Some(StreamEvent::Error { message, retryable }) => {
                        stream_log.push(StreamEvent::Error { message: message.clone(), retryable });
                        failure = Some((message, "transport", retryable));
                        break;
                    }
                    Some(StreamEvent::VendorEvent { kind, payload }) => {
                        stream_log.push(StreamEvent::VendorEvent { kind: kind.clone(), payload: payload.clone() });
                        tracing::debug!(%kind, %payload, "vendor event");
                    }
                    None => break,
                },
                produced = &mut producer => {
                    // The producer finished; drain whatever it buffered.
                    if let Err(err) = produced {
                        failure = Some((err.message, err.class, err.retryable));
                    }
                    while let Some(item) = rx.recv().await {
                        match item {
                            StreamEvent::TextDelta { delta } => {
                                stream_log.push(StreamEvent::TextDelta { delta: delta.clone() });
                                text.push_str(&delta);
                                sink.on_event(TurnEvent::TextDelta(delta));
                            }
                            StreamEvent::ReasoningDelta { delta } => {
                                stream_log.push(StreamEvent::ReasoningDelta { delta: delta.clone() });
                                reasoning.push_str(&delta);
                                sink.on_event(TurnEvent::ReasoningDelta(delta));
                            }
                            event @ (StreamEvent::ToolCallStart { .. }
                            | StreamEvent::ToolCallArgDelta { .. }
                            | StreamEvent::ToolCallEnd { .. }) => {
                                stream_log.push(event.clone());
                                acc.handle(event)
                            }
                            StreamEvent::Usage { usage: reported } => {
                                stream_log.push(StreamEvent::Usage { usage: reported.clone() });
                                usage = reported;
                            }
                            StreamEvent::Error { message, retryable } => {
                                stream_log.push(StreamEvent::Error { message: message.clone(), retryable });
                                failure = Some((message, "transport", retryable));
                            }
                            StreamEvent::VendorEvent { kind, payload } => {
                                stream_log.push(StreamEvent::VendorEvent { kind: kind.clone(), payload: payload.clone() });
                                if kind == lca_protocol::THINKING_SIGNATURE_KIND {
                                    note_signature(&mut signature, &payload);
                                }
                                tracing::debug!(%kind, %payload, "vendor event");
                            }
                        }
                    }
                    natural_end = failure.is_none();
                    break;
                }
            }
        }

        self.replay_stream(&model, stream_log).await;
        if let Some((message, class, retryable)) = failure {
            return Err(CallFail::Provider {
                message,
                class: class.to_string(),
                retryable,
            });
        }
        let (mut calls, protocol_errors) = acc.finish(natural_end);
        // Pi-name aliases dispatch as their canonical tool (gh #119):
        // one seam, before records, permission, and dispatch see the name.
        for call in &mut calls {
            call.name = lca_tools::canonical_tool_name(&call.name).to_string();
        }
        Ok(CallResponse {
            text,
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            signature,
            calls,
            protocol_errors,
            usage,
        })
    }
}

// gh #41: the latest thinking signature (one reasoning run carries
// one; a later block replaces an earlier one, matching the transcript
// order replay resends).
fn note_signature(signature: &mut Option<String>, payload: &serde_json::Value) {
    if let Some(bytes) = payload.get("signature").and_then(|value| value.as_str()) {
        *signature = Some(bytes.to_string());
    }
}

/// A server-requested retry delay parsed from a provider error message
/// (gh #52, pi's `getRetryDelayMs`): guests append the HTTP
/// `retry-after` / `retry-after-ms` response header as a trailing
/// `retry-after: <value>` token. Seconds (a float; `retry-after-ms`
/// is millis) first, then an HTTP-date minus now; anything else is
/// `None` and the caller falls back to exponential backoff (pi's NaN
/// branch). `now_ms` is a parameter so tests pin the date math.
fn retry_after_delay_ms(message: &str, now_ms: u64) -> Option<std::time::Duration> {
    let lower = message.to_ascii_lowercase();
    // `retry-after-ms` first: the shorter marker is its prefix.
    let (raw, is_ms) = lower
        .find("retry-after-ms:")
        .map(|index| (lower[index + 16..].trim_start(), true))
        .or_else(|| {
            lower
                .find("retry-after:")
                .map(|index| (lower[index + 12..].trim_start(), false))
        })?;
    if is_ms {
        // Already millis (pi's `retry-after-ms`): clamp negatives to
        // due-now the way a sleep does; only non-finite falls back.
        return first_token(raw).parse::<f64>().ok().and_then(|ms| {
            ms.is_finite()
                .then(|| std::time::Duration::from_millis(ms.max(0.0) as u64))
        });
    }
    // Seconds are one token; an HTTP-date has spaces, so try the
    // token first and the line remainder as a date second.
    if let Ok(secs) = first_token(raw).parse::<f64>()
        && let Some(delay) = millis_from_secs(secs)
    {
        return Some(delay);
    }
    let date_text = raw
        .trim_end_matches(['\"', '\'', '}', ']', ')', '.', ','])
        .trim_end();
    let date_ms = http_date_ms(date_text)?;
    Some(std::time::Duration::from_millis(
        date_ms.saturating_sub(now_ms),
    ))
}

/// The first whitespace-delimited token, stripped of JSON wrapping.
fn first_token(raw: &str) -> &str {
    raw.split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['\"', '\'', '{', '['])
        .trim_end_matches(['\"', '\'', ',', '}', ']'])
}

/// Finite seconds to a `Duration`, clamped at zero; non-finite
/// falls back to backoff.
fn millis_from_secs(secs: f64) -> Option<std::time::Duration> {
    if !secs.is_finite() {
        return None;
    }
    Some(std::time::Duration::from_millis(
        secs.max(0.0).mul_add(1000.0, 0.0) as u64,
    ))
}

/// Parse an IMF-fixdate (`Mon, 12 Oct 2026 00:01:00 GMT`) to epoch
/// millis. Hand-rolled (Howard Hinnant's days-from-civil): the one
/// header format worth honoring without a date crate.
/// ponytail: IMF-fixdate only; another date shape falls back to backoff.
fn http_date_ms(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let weekday = parts.next()?;
    if !weekday.ends_with(',') {
        return None;
    }
    let day: u64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" | "jan" => 1,
        "Feb" | "feb" => 2,
        "Mar" | "mar" => 3,
        "Apr" | "apr" => 4,
        "May" | "may" => 5,
        "Jun" | "jun" => 6,
        "Jul" | "jul" => 7,
        "Aug" | "aug" => 8,
        "Sep" | "sep" => 9,
        "Oct" | "oct" => 10,
        "Nov" | "nov" => 11,
        "Dec" | "dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let time = parts.next()?;
    let mut clock = time.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 || !parts.next()?.eq_ignore_ascii_case("GMT") {
        return None;
    }
    if !(1..=31).contains(&day) {
        return None;
    }
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let yoe = (shifted - era * 400) as u64;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as u64 * 146097 + doe - 719468;
    Some(days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Verifies: gh #52 - a `retry-after` token in the provider error
    // (seconds, pi's `retry-after` / `retry-after-ms`) sets the delay.
    #[test]
    fn retry_after_seconds_are_honored() {
        assert_eq!(
            retry_after_delay_ms("provider returned HTTP 429: busy retry-after: 2", 0),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            retry_after_delay_ms("provider returned HTTP 503: busy retry-after-ms: 1500", 0),
            Some(Duration::from_millis(1500))
        );
    }

    // Verifies: gh #52 - an HTTP-date `retry-after` sleeps until the
    // named instant (pi's `Date.parse` branch); a past date is due now.
    #[test]
    fn retry_after_http_date_is_honored() {
        // 2026-10-12T00:01:00Z: 60s after the 00:00:00Z fixture now.
        let now_ms = 1_791_763_200_000u64; // 2026-10-12T00:00:00Z
        assert_eq!(
            retry_after_delay_ms(
                "provider returned HTTP 429: busy retry-after: Mon, 12 Oct 2026 00:01:00 GMT",
                now_ms
            ),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            retry_after_delay_ms(
                "provider returned HTTP 429: busy retry-after: Mon, 12 Oct 2026 00:00:00 GMT",
                now_ms + 5_000
            ),
            Some(Duration::ZERO)
        );
    }

    // Verifies: gh #52 - an unparseable `retry-after` (pi's NaN branch)
    // is not a delay: the caller falls back to exponential backoff.
    #[test]
    fn unparseable_retry_after_falls_back_to_backoff() {
        assert_eq!(
            retry_after_delay_ms("provider returned HTTP 429: busy retry-after: soon", 0),
            None
        );
        assert_eq!(
            retry_after_delay_ms("provider returned HTTP 429: busy", 0),
            None
        );
    }
}
