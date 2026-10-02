//! Per-turn throughput metering for the footer's `tok/s` reading (the
//! owner's "can we add tok/s to the footer?" ask).
//!
//! The rate is generation speed, not wall-clock: the window opens on the
//! turn's first content delta, closes while a tool runs, and reopens at
//! the next delta, so a two-minute shell command never lands in the
//! number. The token count is the provider's own `Usage`, summed over the
//! turn's calls - measured, never estimated from characters.

use std::time::{Duration, Instant};

/// The streaming time and output tokens of the turn in flight.
#[derive(Debug, Default)]
pub struct StreamMeter {
    /// The currently open window, when the turn is streaming right now.
    window: Option<Instant>,
    /// Streaming time already banked for this turn.
    streamed: Duration,
    /// Output tokens seen for this turn (its per-call `Usage` events).
    turn_output: u64,
}

impl StreamMeter {
    /// The first content delta opened the window; a later delta finds it
    /// open and does nothing (the window also re-opens after a tool run).
    pub fn open(&mut self) {
        if self.window.is_none() {
            self.window = Some(Instant::now());
        }
    }

    /// Bank the open window: a tool run's latency is not generation time.
    pub fn close(&mut self) {
        if let Some(at) = self.window.take() {
            self.streamed += at.elapsed();
        }
    }

    /// One provider call's output side (the turn's calls sum).
    pub fn note_usage(&mut self, output: u64) {
        self.turn_output = self.turn_output.saturating_add(output);
    }

    /// The turn ended: bank the window, publish the reading onto the
    /// footer, and reset for the next turn. A turn with nothing measured
    /// (tools only, an immediate error) leaves the previous reading alone
    /// rather than inventing a number or clearing a good one.
    pub fn finish_turn(&mut self, footer: &mut crate::footer::Footer) {
        self.close();
        if self.streamed.as_secs_f64() > 0.0 && self.turn_output > 0 {
            footer.tok_s =
                Some((self.turn_output as f64 / self.streamed.as_secs_f64()).round() as u64);
        }
        self.streamed = Duration::ZERO;
        self.turn_output = 0;
    }
}
