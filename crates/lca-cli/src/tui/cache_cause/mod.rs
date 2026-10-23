//! Why a prompt-cache miss happened (gh #402).
//!
//! ADR-0017 already records *that* a boundary narrowed and already measures *how
//! much* was re-billed. What a user cannot see is **what they did** to cause it:
//! the only signal today is a toast reading
//! `[host] cache-boundary-narrowed: stable region diverged at message 7 (boundary
//! 9 -> 7)`, which is wire vocabulary, not a person's vocabulary.
//!
//! # Correlate, do not re-derive
//!
//! This module never asks "why might a cache miss happen" and answers from
//! inference. It reads two things the measurement already produced:
//!
//! 1. [`lca_session::CacheMiss`] carries `idle_ms` and `model_changed` - the two
//!    causes the scan itself detects.
//! 2. A `cache-boundary-narrowed` extension event records that the *prefix* moved,
//!    written by `lca-core`'s cross-turn comparison at the moment it narrowed.
//!
//! A miss whose assistant record is preceded (in log order) by a
//! `cache-boundary-narrowed` event is attributed to a boundary change; the rest
//! fall back to the two measurable causes. Anything left is
//! [`Cause::Unknown`] - named as unknown rather than guessed, because an invented
//! cause is undebuggable and would make the notice lie about the one thing it is
//! for.
//!
//! Split from `display.rs` for the 1,200-line ceiling (gate 11).

use lca_protocol::Record;

/// What made the prompt cache miss (gh #402).
///
/// The variants are ordered as they are reported: a boundary change explains the
/// miss more specifically than an idle gap does, and a model switch is a
/// different *kind* of fact (see [`Cause::is_defect`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Cause {
    /// A skill matched and its instructions entered the prompt (gh #299's
    /// `[skill instructions]` wrapper grew the stable prefix).
    SkillMatched,
    /// Project instructions changed mid-session (gh #217's delta).
    InstructionsChanged,
    /// The stable region diverged: a context-transform extension rewrote content
    /// the provider had cached, and the boundary narrowed to end before it.
    BoundaryDiverged,
    /// The prompt cache expired while the session sat idle (TTL).
    IdleExpired,
    /// The active model changed. **Counted on purpose** (FR-CACHE-2): a switch
    /// resets nothing, and the re-billed tokens are real cost, not a fault.
    ModelSwitched,
    /// A counted miss with no attributable signal. Reported honestly.
    Unknown,
}

impl Cause {
    /// The user-facing phrase for this cause.
    pub(crate) fn phrase(self) -> &'static str {
        match self {
            Cause::SkillMatched => "a skill matched and its instructions joined the prompt",
            Cause::InstructionsChanged => "project instructions changed",
            Cause::BoundaryDiverged => "an extension rewrote context the cache had stored",
            Cause::IdleExpired => "the prompt cache expired while the session sat idle",
            Cause::ModelSwitched => "you switched models",
            Cause::Unknown => "cause unknown",
        }
    }

    /// Whether this cause is something to fix, or cost that is working as
    /// designed (FR-CACHE-2).
    ///
    /// A model switch is deliberate and its cost is real; wording it as a defect
    /// would make the notice cry wolf on the one case the design intends. A skill
    /// matching and a mid-session instruction edit are *normal* too - they are
    /// changes the user asked for - so they are framed as consequences rather
    /// than faults. Only a transform diverging and an unexplained miss are
    /// framed as worth investigating.
    pub(crate) fn is_defect(self) -> bool {
        matches!(self, Cause::BoundaryDiverged | Cause::Unknown)
    }
}

/// Which part of the prompt a skill injection or instruction delta landed in.
///
/// The distinction matters because gh #299's fix put both markers in a **User**
/// message rather than a `System` one, precisely so they stay in the conversational
/// stream. Reading the marker is therefore reading a message the model saw.
fn marker_cause(text: &str) -> Option<Cause> {
    if text.starts_with("[skill instructions]") {
        Some(Cause::SkillMatched)
    } else if text.starts_with("[instructions changed]") {
        Some(Cause::InstructionsChanged)
    } else {
        None
    }
}

/// Every cause recorded in `records` before the assistant record `miss_record`,
/// most specific first.
///
/// Walks in log order rather than by timestamp: the log is append-only and
/// ordered, and two records in the same millisecond would tie on `ts`.
fn causes_before(records: &[Record], miss_record: &str) -> Vec<Cause> {
    let mut found: Vec<Cause> = Vec::new();
    for record in records {
        // The miss's own record ends the window: anything at or after it did not
        // cause this miss.
        if record.id() == Some(miss_record) {
            break;
        }
        match record {
            // gh #299: the injection is a `CustomMessage` whose content carries
            // the wrapper, so the marker is readable from the log.
            Record::CustomMessage { content, .. } => {
                if let Some(cause) = marker_cause(content) {
                    found.push(cause);
                }
            }
            // gh #217's instruction delta is appended as a `CustomMessage` too,
            // but older sessions carry it as a plain `User` record.
            Record::User { content, .. } => {
                if let Some(cause) = marker_cause(content) {
                    found.push(cause);
                }
            }
            Record::ExtensionEvent { event, .. } if event == "cache-boundary-narrowed" => {
                found.push(Cause::BoundaryDiverged);
            }
            _ => {}
        }
    }
    // Most specific wins: a skill match or an instruction delta explains a
    // boundary move better than "something diverged". Later occurrences win over
    // earlier ones only within a kind, which `dedup` below makes irrelevant.
    for preferred in [
        Cause::SkillMatched,
        Cause::InstructionsChanged,
        Cause::BoundaryDiverged,
    ] {
        if found.contains(&preferred) {
            return vec![preferred];
        }
    }
    found
}

/// Attribute one counted miss to a cause (gh #402).
///
/// Precedence, and why:
///
/// 1. **A recorded prompt change** (skill, instructions, divergence) - it is the
///    specific thing that moved the prefix, read from the log rather than inferred.
/// 2. **A model switch** - FR-CACHE-2 counts it; carried by the miss itself.
/// 3. **An idle gap past the cache TTL** - carried by the miss itself as `idle_ms`.
/// 4. **[`Cause::Unknown`]** - nothing in the log explains it, so say so.
///
/// A skill match and a model switch can coincide; the prompt change wins because
/// it is the more actionable fact, and the notice is once-per-cause so the switch
/// would otherwise be masked permanently.
pub(crate) fn attribute(
    records: &[Record],
    miss: &lca_session::CacheMiss,
    cache_ttl_ms: u64,
) -> Cause {
    if let Some(cause) = causes_before(records, &miss.record_id).into_iter().next() {
        return cause;
    }
    if miss.model_changed {
        return Cause::ModelSwitched;
    }
    if cache_ttl_ms > 0 && miss.idle_ms >= cache_ttl_ms {
        return Cause::IdleExpired;
    }
    Cause::Unknown
}

/// The notice text for one attributed miss (gh #402).
///
/// Two shapes, and the difference is the point:
///
/// - a **fault-shaped** cause reads as a report, with the remedy pointing at
///   `/session` for the numbers;
/// - a **counted-by-design** one says so, so a deliberate switch is not dressed
///   up as a bug (FR-CACHE-2).
pub(crate) fn notice_text(
    cause: Cause,
    missed_tokens: u64,
    missed_cost: f64,
    miss_count: u64,
) -> String {
    let cost = if missed_cost > 0.0 {
        format!("${missed_cost:.4}")
    } else {
        "cost unknown".to_string()
    };
    // Once the same cause has been reported, later occurrences add up rather than
    // repeating: the count is what tells a user a cause keeps happening.
    let repeat = if miss_count > 1 {
        format!(" ({miss_count} times this session)")
    } else {
        String::new()
    };
    if cause.is_defect() {
        format!(
            "prompt cache miss: {} - {missed_tokens} tokens re-billed ({cost}){repeat} - see /session",
            cause.phrase()
        )
    } else {
        format!(
            "prompt cache: {} - {missed_tokens} tokens re-billed, counted as real cost ({cost}){repeat}",
            cause.phrase()
        )
    }
}

#[cfg(test)]
mod tests;
