//! Trigger-owned autocomplete: the decorator chain (gh #408, in-process half).
//!
//! pi's seam is an `AutocompleteProvider` that also declares
//! `triggerCharacters?: string[]` (`packages/tui/src/autocomplete.ts:271`), and an
//! extension wraps the existing provider to own one of those characters
//! (`examples/extensions/github-issue-autocomplete.ts`). This module is that shape
//! in Rust: a provider that owns some prefixes, handles them asynchronously, and
//! **delegates every other token down the chain**.
//!
//! Delegation, not a callback registry, is what keeps `/` and `@` intact. A registry
//! would need an ordering rule and would get it wrong the first time somebody adds a
//! prefix; here the built-in owner of a token is asked first, by construction.
//!
//! # Why a bounded wait rather than a tick
//!
//! [`AutocompleteProvider::get_suggestions`] is synchronous, and `editor.rs` is
//! outside this cycle's boundary, so nothing on the render loop can be woken when a
//! query resolves. A query is therefore given a hard budget and the
//! caller receives rows, a failure row, or a timeout row - never a hang. `pi` gets
//! away with resolving later because its editor calls `requestRender`; our renderer
//! already repaints on `dirty`, so the honest equivalent is "answer within the
//! budget, or say you could not".
//!
//! # Abort
//!
//! Every query is stamped with a generation. A second call bumps the generation, so
//! a result computed for an older one is discarded rather than applied over newer
//! text. That is `pi`'s `AbortController` discipline expressed against a sync trait:
//! the [`AbortSignal`] a query holds reports aborted the moment the chain moves on.

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod trigger_tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{AutocompleteItem, AutocompleteProvider, Suggestions};

/// Why a trigger prefix was refused at registration (gh #408 rule 2).
///
/// A trigger that can hijack command or path completion is the bug this whole seam
/// could introduce, so refusal is a named outcome rather than a silently ignored
/// argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerRefusal {
    /// `/` owns command completion; letting another provider claim it would make
    /// `/model` depend on registration order.
    Slash,
    /// Whitespace ends a token, so it can never start one.
    Whitespace,
    /// A prefix is exactly one character.
    NotSingle,
}

impl std::fmt::Display for TriggerRefusal {
    /// The copy tells the user what to do, not what the subsystem called it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slash => f.write_str("`/` is reserved for commands; pick another trigger"),
            Self::Whitespace => f.write_str("a whitespace character cannot start a token"),
            Self::NotSingle => f.write_str("a trigger is exactly one character"),
        }
    }
}

/// Validate a trigger prefix, returning the rejection reason (gh #408 rule 2).
pub fn validate_trigger(prefix: &str) -> Result<char, TriggerRefusal> {
    let mut chars = prefix.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return Err(TriggerRefusal::NotSingle);
    };
    if first == '/' {
        return Err(TriggerRefusal::Slash);
    }
    if first.is_whitespace() {
        return Err(TriggerRefusal::Whitespace);
    }
    Ok(first)
}

/// The abort token a query holds (pi's `AbortSignal`).
///
/// It reports aborted as soon as the chain starts a newer query, so a slow query can
/// stop early instead of finishing work whose result will be thrown away.
#[derive(Debug, Clone)]
pub struct AbortSignal {
    /// Shared with the chain: the chain stores its current generation here.
    generation: Arc<AtomicU64>,
    /// The generation this query was started for.
    mine: u64,
}

impl AbortSignal {
    /// Whether a newer query has superseded this one.
    #[must_use]
    pub fn aborted(&self) -> bool {
        self.generation.load(Ordering::SeqCst) != self.mine
    }
}

/// What a query produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    /// Rows for the popup.
    Rows(Vec<AutocompleteItem>),
    /// A failure the user should see as one row. The copy says what the user can do.
    Failed(String),
}

/// A handle to an in-flight query: the query owns its own thread or task and sends
/// its result down this channel when it lands.
pub struct QueryHandle {
    /// The result channel. Dropping the handle abandons the query; the query's own
    /// `send` then fails, which is how it learns nobody is waiting.
    pub rx: Receiver<QueryResult>,
}

/// One registered trigger's query.
///
/// `start` returns immediately with a handle, so a slow query never runs on the
/// caller's thread. The [`AbortSignal`] is there for a query that can stop early.
pub trait TriggerQuery: Send + Sync {
    /// Begin a query for `token` (the text after the trigger character).
    fn start(&self, token: &str, signal: AbortSignal) -> QueryHandle;
}

/// One registered trigger.
pub struct RegisteredTrigger {
    /// The character that starts it.
    pub prefix: char,
    /// Where the rows come from, e.g. `extension: github-issues`. Rendered as
    /// `[extension: github-issues]` (issuedraft 022's origin tag).
    pub origin: String,
    /// The query.
    pub query: Arc<dyn TriggerQuery>,
}

/// How much work the chain has done, for the "unchanged keystrokes cost nothing"
/// row. A count, not a clock: the assertion must not depend on machine speed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChainStats {
    /// Queries actually started.
    pub queries: u64,
    /// Calls answered from the cache without starting a query.
    pub cache_hits: u64,
}

/// A decorator that owns trigger prefixes over an inner provider.
pub struct TriggerChain {
    /// The provider that owns everything the chain does not, asked second.
    inner: Arc<dyn AutocompleteProvider>,
    /// Registered triggers, at most one per character.
    triggers: Vec<RegisteredTrigger>,
    /// How long a query may take before the row says it timed out.
    budget: Duration,
    /// Rows by `(prefix, token)`: an unchanged token costs no query and no new rows,
    /// which is what makes repeated keystrokes inside one token free.
    cache: Mutex<HashMap<(char, String), Vec<AutocompleteItem>>>,
    /// The current generation; bumped per query, read by [`AbortSignal`].
    generation: Arc<AtomicU64>,
    /// Counters.
    stats: Mutex<ChainStats>,
}

impl TriggerChain {
    /// Wrap `inner` with a query budget.
    #[must_use]
    pub fn new(inner: Arc<dyn AutocompleteProvider>, budget: Duration) -> Self {
        Self {
            inner,
            triggers: Vec::new(),
            budget,
            cache: Mutex::new(HashMap::new()),
            generation: Arc::new(AtomicU64::new(0)),
            stats: Mutex::new(ChainStats::default()),
        }
    }

    /// Register a trigger, or say why the prefix is not allowed.
    ///
    /// A duplicate prefix replaces the earlier registration rather than stacking:
    /// two providers on one character is the collision this API exists to prevent.
    pub fn register(
        &mut self,
        prefix: &str,
        origin: impl Into<String>,
        query: Arc<dyn TriggerQuery>,
    ) -> Result<(), TriggerRefusal> {
        let character = validate_trigger(prefix)?;
        let trigger = RegisteredTrigger {
            prefix: character,
            origin: origin.into(),
            query,
        };
        match self.triggers.iter_mut().find(|t| t.prefix == character) {
            Some(existing) => *existing = trigger,
            None => self.triggers.push(trigger),
        }
        Ok(())
    }

    /// The registered prefixes, in registration order.
    #[must_use]
    pub fn prefixes(&self) -> Vec<char> {
        self.triggers.iter().map(|t| t.prefix).collect()
    }

    /// The work counters.
    #[must_use]
    pub fn stats(&self) -> ChainStats {
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }

    /// The token the cursor is in: everything after the last boundary.
    ///
    /// Boundaries are whitespace and the opening wrappers pi allows in prose
    /// (`(@src`, `` `@src ``), so `"see (#12"` yields `"#12"`.
    #[must_use]
    pub fn trailing_token(text: &str) -> &str {
        let start = text
            .char_indices()
            .rev()
            .find(|(_, c)| {
                c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '`' | '"' | '\'')
            })
            .map_or(0, |(index, c)| index + c.len_utf8());
        &text[start..]
    }

    /// Which trigger owns `token`, longest prefix first, and the token's body.
    ///
    /// `@` is deliberately not claimed: the built-in provider owns it, and pi's
    /// whitespace rule for attachments must keep priority (gh #408 rule 4).
    fn owner<'a>(&'a self, token: &'a str) -> Option<(&'a RegisteredTrigger, &'a str)> {
        if token.starts_with('@') {
            return None;
        }
        let mut best: Option<&RegisteredTrigger> = None;
        for trigger in &self.triggers {
            if !token.starts_with(trigger.prefix) {
                continue;
            }
            // Longest wins. Registration only ever admits one character, so this
            // cannot tie today; it is written as a comparison so that admitting a
            // longer prefix later cannot silently change the rule.
            match best {
                None => best = Some(trigger),
                Some(current) if trigger.prefix.len_utf8() > current.prefix.len_utf8() => {
                    best = Some(trigger);
                }
                Some(_) => {}
            }
        }
        best.map(|trigger| (trigger, &token[trigger.prefix.len_utf8()..]))
    }

    /// Tag a row with where it came from.
    ///
    /// The tag is a typed field, never a string the editor has to parse back out
    /// (gh #408 design point 4); the popup renderer appends it as
    /// `[extension: name]`.
    fn tag_origin(item: &mut AutocompleteItem, origin: &str) {
        item.origin = Some(origin.to_string());
    }

    /// The one row that reports a failure or a timeout (gh #408 rule 5).
    ///
    /// Its `value` is the token itself, so accepting it re-inserts exactly what the
    /// user typed: the row is information, not a choice, and pressing Tab on it
    /// must not delete their text.
    fn unavailable(token: &str, reason: &str) -> Suggestions {
        Suggestions {
            items: vec![AutocompleteItem {
                value: token.to_string(),
                label: "(unavailable)".to_string(),
                description: Some(reason.to_string()),
                origin: None,
                dim: true,
            }],
            prefix: token.to_string(),
        }
    }

    /// Start a query for `(trigger, body)` and wait up to the budget.
    fn run(&self, trigger: &RegisteredTrigger, token: &str, body: &str) -> Option<Suggestions> {
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let signal = AbortSignal {
            generation: Arc::clone(&self.generation),
            mine,
        };
        if let Ok(mut stats) = self.stats.lock() {
            stats.queries += 1;
        }
        let handle = trigger.query.start(body, signal);
        let rows = match handle.rx.recv_timeout(self.budget) {
            Ok(QueryResult::Rows(rows)) => rows,
            // A failure is one dim row; it is never a modal and never silent.
            Ok(QueryResult::Failed(reason)) => return Some(Self::unavailable(token, &reason)),
            // Hitting the ceiling *is* the measurement: say so instead of hanging.
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                return Some(Self::unavailable(token, "the query took too long"));
            }
        };
        // Abort check: a newer keystroke has already asked a different question, so
        // this answer is about text the user no longer has.
        if self.generation.load(Ordering::SeqCst) != mine {
            return None;
        }
        if rows.is_empty() {
            // pi's example falls through to the inner provider when a trigger has
            // nothing to offer, so a `#` that matches no issue still completes paths.
            return self.inner.get_suggestions(token, false);
        }
        let mut rows = rows;
        for row in &mut rows {
            Self::tag_origin(row, &trigger.origin);
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert((trigger.prefix, body.to_string()), rows.clone());
        }
        Some(Suggestions {
            items: rows,
            prefix: token.to_string(),
        })
    }
}

impl AutocompleteProvider for TriggerChain {
    fn get_suggestions(&self, text: &str, force: bool) -> Option<Suggestions> {
        // `/` at line start is the command provider's, always (gh #408 rule 4).
        if text.starts_with('/') {
            return self.inner.get_suggestions(text, force);
        }
        let token = Self::trailing_token(text);
        let Some((trigger, body)) = self.owner(token) else {
            return self.inner.get_suggestions(text, force);
        };
        // An unchanged token costs no query: the answer for this exact
        // `(prefix, body)` pair is reused. `force` (Tab) is an explicit "ask again".
        let cached = (!force)
            .then(|| self.cache.lock().ok())
            .flatten()
            .and_then(|cache| cache.get(&(trigger.prefix, body.to_string())).cloned());
        if let Some(rows) = cached {
            if let Ok(mut stats) = self.stats.lock() {
                stats.cache_hits += 1;
            }
            return Some(Suggestions {
                items: rows,
                prefix: token.to_string(),
            });
        }
        self.run(trigger, token, body)
    }
}
