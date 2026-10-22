//! The footer's `tok/s` reading: the last finished turn's generation speed.
//!
//! The owner asked for it ("can we add tok/s to the footer?"), and FR-UI-20's
//! annotation defines it: **output tokens over streaming time, measured from
//! the provider's own usage, excluding tool runs**.
//!
//! Gh #464 is why this module holds no clock. The first implementation opened
//! an `Instant` window in `Chat::on_turn_event` - the interface thread - and a
//! live session reported 5,263,158 tok/s. The interface drains a bounded
//! channel in a burst, so a finished response's deltas all arrived within
//! microseconds of each other; the meter was measuring the interface's
//! scheduling, not the model's generation. The span is now stamped in
//! `lca-core` where the stream is consumed and arrives here on the call's own
//! `Usage` extras ([`lca_protocol::GENERATION_US_EXTRA`]). This module adds
//! up what arrived and divides.
//!
//! Three rules the reading obeys, each pinned by a row below:
//!
//! 1. **One span, one token count, same thing.** A call contributes to the
//!    numerator only if it stamped a span, so a provider that reported tokens
//!    without streaming (or a call that failed mid-stream) cannot inflate the
//!    rate. Reasoning tokens are in both halves (the provider's `output`
//!    includes them and the stamp spans their deltas) - the "both" rule, never
//!    mixed.
//! 2. **An impossible number is a bug, not data.** Above
//!    [`MAX_PLAUSIBLE_TOK_S`] the cell renders nothing. Clamping would still
//!    be a lie; hiding it says "not measured".
//! 3. **No reading before a measurement, and none for a turn that measured
//!    nothing.** A turn that only ran tools, or failed before streaming,
//!    leaves the previous reading alone rather than inventing a zero or
//!    clearing a good number.

/// The ceiling above which a reading is a broken measurement rather than a
/// model's speed (gh #464).
///
/// The fastest production models stream well under this; a reading past it can
/// only come from a span that measured something other than generation (a
/// burst drain, a zero-length stamp). 100k leaves two orders of magnitude of
/// headroom over real hardware while catching the class of bug the receipt
/// showed - which was six orders out.
pub const MAX_PLAUSIBLE_TOK_S: u64 = 100_000;

/// The streaming time and output tokens accumulated for the turn in flight.
#[derive(Debug, Default)]
pub struct StreamMeter {
    /// Generation microseconds banked for this turn, summed over its calls.
    generation_us: u64,
    /// Output tokens of the calls that carried a span (rule 1).
    output: u64,
}

/// What a finished turn measured, which decides what the cell does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The turn streamed nothing measurable (tools only, an empty answer, a
    /// provider that reported tokens without streaming). The previous reading
    /// stands: it is still the last thing actually measured.
    Unmeasured,
    /// The turn measured, and the measurement is impossible - the class of bug
    /// gh #464 is. The cell clears rather than showing a number that could only
    /// be a broken span.
    Impossible,
    /// A believable speed, in output tokens per second.
    Rate(u64),
}

impl StreamMeter {
    /// The turn ended: publish the reading and reset for the next turn.
    pub fn finish_turn(&mut self, footer: &mut crate::footer::Footer) {
        match self.reading() {
            // Rule 3: leave a good previous reading alone.
            Reading::Unmeasured => {}
            // Rule 2: a wrong number is worse than no number.
            Reading::Impossible => footer.tok_s = None,
            Reading::Rate(rate) => footer.tok_s = Some(rate),
        }
        self.generation_us = 0;
        self.output = 0;
    }

    /// One provider call's usage. Only a call that stamped its generation span
    /// contributes, so numerator and denominator always describe the same set
    /// of tokens.
    pub fn note_usage(&mut self, usage: &lca_protocol::Usage) {
        let Some(micros) = generation_us(usage) else {
            return;
        };
        self.generation_us = self.generation_us.saturating_add(micros);
        self.output = self.output.saturating_add(usage.output);
    }

    /// What this turn measured.
    pub fn reading(&self) -> Reading {
        // A zero span is a division by zero, a burst with no measurable
        // duration, or a provider that streamed text but billed no output
        // tokens. None of those is a speed, and none of them is a *wrong*
        // speed either.
        if self.generation_us == 0 || self.output == 0 {
            return Reading::Unmeasured;
        }
        let rate = (self.output as f64 * 1_000_000.0) / self.generation_us as f64;
        if !rate.is_finite() || rate > MAX_PLAUSIBLE_TOK_S as f64 {
            return Reading::Impossible;
        }
        Reading::Rate(rate.round() as u64)
    }
}

/// The generation span a usage record carries, in microseconds.
fn generation_us(usage: &lca_protocol::Usage) -> Option<u64> {
    usage
        .extras
        .get(lca_protocol::GENERATION_US_EXTRA)
        .and_then(|raw| raw.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::Usage;

    /// A usage record carrying a generation span, as `lca-core` stamps it.
    fn stamped(output: u64, micros: u64) -> Usage {
        let mut usage = Usage {
            output,
            ..Default::default()
        };
        usage.extras.insert(
            lca_protocol::GENERATION_US_EXTRA.to_string(),
            micros.to_string(),
        );
        usage
    }

    fn rate(output: u64, micros: u64) -> Reading {
        let mut meter = StreamMeter::default();
        meter.note_usage(&stamped(output, micros));
        meter.reading()
    }

    // Verifies: gh #464 - the reading is tokens over the stamped span. 40
    // tokens over 250 ms is 160 tok/s, whatever the interface was doing.
    #[test]
    fn the_reading_is_tokens_over_the_stamped_span() {
        assert_eq!(rate(40, 250_000), Reading::Rate(160));
        assert_eq!(rate(1_000, 1_000_000), Reading::Rate(1_000));
    }

    // Verifies: gh #464 - the receipt's case. When a burst drain is the only
    // thing that could have produced a span, the stamp is tiny and the rate
    // impossible; the cell shows nothing rather than 5,263,158.
    #[test]
    fn an_impossible_reading_renders_nothing() {
        // 30 tokens over 5.7 us is the arithmetic from the issue.
        assert_eq!(rate(30, 6), Reading::Impossible);
        // Just over the ceiling, and just at it, to pin where the line is.
        assert_eq!(
            rate(MAX_PLAUSIBLE_TOK_S + 1, 1_000_000),
            Reading::Impossible
        );
        assert_eq!(
            rate(MAX_PLAUSIBLE_TOK_S, 1_000_000),
            Reading::Rate(MAX_PLAUSIBLE_TOK_S),
            "the ceiling itself is a real reading"
        );
    }

    // Verifies: gh #464 - a call with no stamped span is not measured, and a
    // turn made only of such calls renders nothing. Absence is "not
    // measured", never zero.
    #[test]
    fn an_unstamped_call_contributes_nothing() {
        let mut meter = StreamMeter::default();
        meter.note_usage(&Usage {
            output: 500,
            ..Default::default()
        });
        assert_eq!(meter.reading(), Reading::Unmeasured);

        // And it cannot pad a stamped call's numerator either.
        meter.note_usage(&stamped(40, 250_000));
        assert_eq!(
            meter.reading(),
            Reading::Rate(160),
            "the unstamped 500 tokens stay out of the numerator"
        );
    }

    // Verifies: gh #464 - a zero-length span is a burst with no measurable
    // duration: no reading, never an infinite one.
    #[test]
    fn a_zero_span_produces_no_reading() {
        assert_eq!(rate(30, 0), Reading::Unmeasured);
    }

    // Verifies: gh #464 - a multi-call turn sums both halves, so a turn with
    // three streaming calls reports one speed for the whole turn.
    #[test]
    fn a_multi_call_turn_sums_tokens_and_spans() {
        let mut meter = StreamMeter::default();
        meter.note_usage(&stamped(100, 500_000));
        meter.note_usage(&stamped(300, 500_000));
        // 400 tokens over 1 s.
        assert_eq!(meter.reading(), Reading::Rate(400));
    }

    // Verifies: gh #464 and FR-UI-20's "no reading invented before the first
    // measurable turn" - a turn that measured nothing keeps the previous
    // reading rather than clearing a good number or inventing a zero.
    #[test]
    fn a_turn_without_measurement_keeps_the_previous_reading() {
        let mut meter = StreamMeter::default();
        let mut footer = crate::footer::Footer::default();
        meter.note_usage(&stamped(40, 250_000));
        meter.finish_turn(&mut footer);
        assert_eq!(footer.tok_s, Some(160));

        // A tools-only turn: no usage at all.
        meter.finish_turn(&mut footer);
        assert_eq!(footer.tok_s, Some(160), "no measurement, no new reading");

        // The meter resets, so the next measured turn stands alone.
        meter.note_usage(&stamped(50, 250_000));
        meter.finish_turn(&mut footer);
        assert_eq!(footer.tok_s, Some(200));
    }

    // Verifies: gh #464 rule 2 on the *clearing* side - an impossible
    // measurement clears the cell rather than leaving a stale reading that
    // would now be attributed to this turn.
    #[test]
    fn an_impossible_reading_clears_a_previous_one() {
        let mut meter = StreamMeter::default();
        let mut footer = crate::footer::Footer::default();
        meter.note_usage(&stamped(40, 250_000));
        meter.finish_turn(&mut footer);
        assert_eq!(footer.tok_s, Some(160));

        meter.note_usage(&stamped(30, 6));
        meter.finish_turn(&mut footer);
        assert_eq!(footer.tok_s, None, "the broken turn cleared the cell");
    }
}
