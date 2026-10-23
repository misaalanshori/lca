//! `/model` search ranking (gh #396 gap 1).
//!
//! Split from `chat_pickers.rs` for the 1,200-line ceiling and because the
//! ranking rules are their own thing.
//!
//! # The defect
//!
//! `refilter` matched `label.to_lowercase().contains(needle)`. Two failures
//! follow from that one line:
//!
//! 1. **A proxy outranks the real thing.** `openrouter/openai/gpt-5` has the
//!    direct entry's whole name *inside* it, so a substring match on the label
//!    could not tell them apart - and the longer, proxied path is exactly the one
//!    a user does not want. `openai/gpt-5` is the model; the other is somebody
//!    else reselling it.
//! 2. **No typo tolerance.** `4ogpt` is a transposition of `gpt-4o`, and
//!    `contains` cannot see it. pi's `fuzzyMatch` can.
//!
//! # pi's shape, and what is ported
//!
//! `getModelSelectorSearchText` (`model-search.ts`) builds the search text as
//! **provider first**:
//!
//! ```text
//! {provider} {provider}/{id} {provider} {id}{name}
//! ```
//!
//! The provider leading is the whole trick: a query naming a provider scores
//! against the real entry at position 0, while the proxied entry only contains
//! that provider later in a longer string. That is why the direct row wins on
//! score rather than by a hand-written exception.
//!
//! `getModelSelectorSearchText` also keeps the **bare id out of the leading
//! position** deliberately - its own comment says "so keep the bare model ID out
//! of the leading position" - which is the same defect this cycle fixes.
//!
//! # Ranking rules, in order
//!
//! 1. An exact match on the model id (`gpt-5` == `gpt-5`) ranks first.
//! 2. A prefix match on the id ranks next.
//! 3. Everything else: the fuzzy score over the full identity.
//! 4. Ties keep the caller's order (a stable sort), so the unfiltered order
//!    survives a query that matches everything equally.
//!
//! Rule 1 and 2 exist because pi's `fuzzyMatch` alone ranks `gpt-5` inside
//! `openrouter/openai/gpt-5` highly - the characters are all there and closer
//! together than in a short id. Scoring the *kind* of match before the fuzzy
//! distance is what makes "the model I named" beat "a longer path containing it".

/// One `/model` row, with the pieces its search needs (gh #396).
///
/// `ModelRow` was a `(id, label)` tuple. The label is display-only (G2), and the
/// label is exactly what could not tell a proxy from the real thing - it embeds
/// the id, so `contains` cannot separate them. Carrying the provider as its own
/// field is what makes the identity searchable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// The raw model id (what selection saves and what the provider is called
    /// with). May already be `provider/model`.
    pub id: String,
    /// The display label, provider badge included. Never searched as the
    /// identity - see [`ModelEntry::search_text`].
    pub label: String,
    /// The provider that will bill the call (`openai`, `openrouter`, …).
    ///
    /// Empty when the host cannot tell, which is not an error: the entry still
    /// searches on its id alone.
    pub provider: String,
}

impl ModelEntry {
    /// A row whose provider is unknown: it searches on its id.
    #[must_use]
    pub fn without_provider(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            provider: String::new(),
        }
    }

    /// The text a query is scored against - pi's `getModelSelectorSearchText`
    /// shape, provider first.
    ///
    /// A blank provider is skipped rather than emitted as a leading space, so an
    /// entry with no provider scores exactly as it did before this change.
    #[must_use]
    pub fn search_text(&self) -> String {
        if self.provider.is_empty() {
            return self.id.clone();
        }
        // pi's exact order: provider, provider/id, provider, id. The repetition is
        // pi's and it is load-bearing: it lets a multi-token query `openai gpt-5`
        // match both halves of an unqualified id.
        format!("{p} {p}/{id} {p} {id}", p = self.provider, id = self.id)
    }

    /// How well this entry matches `query`, or `None` when it does not.
    ///
    /// Lower is better, matching pi's `totalScore` convention.
    #[must_use]
    pub fn score(&self, query: &str) -> Option<f64> {
        let tokens: Vec<String> = query
            .split(|c: char| c.is_whitespace() || c == '/')
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        if tokens.is_empty() {
            return Some(0.0);
        }
        let id = self.id.to_lowercase();
        let text = self.search_text().to_lowercase();
        let mut total = 0.0;
        for token in &tokens {
            // The kind of match comes before the fuzzy distance (rules 1-2):
            // an exact id beats a prefix beats a fuzzy hit, whatever the fuzzy
            // scores say. Without this, `gpt-5` ranks a proxied path above the
            // direct id because its characters sit closer together.
            if id == *token {
                total += EXACT_BONUS;
                continue;
            }
            // A prefix on the id, or a provider-prefixed id ending in the token
            // (`openai/gpt-5` for `gpt-5`). Both name this model directly.
            if id.starts_with(token.as_str()) || id.ends_with(&format!("/{token}")) {
                total += PREFIX_BONUS;
                continue;
            }
            let matched = lca_tui::widgets::autocomplete::fuzzy_match_scored(token, &text);
            if !matched.matches {
                // Multi-token fallback: a `/`-qualified query is split, so
                // `openai/gpt-5` arrives as `openai` + `gpt-5` and each half must
                // match somewhere in the identity. The id check above already
                // covered the direct case; this covers the rest.
                return None;
            }
            total += matched.score;
        }
        Some(total)
    }
}

impl From<(String, String)> for ModelEntry {
    /// A `(id, label)` pair becomes a provider-less entry.
    ///
    /// This is the shape every existing caller and test already builds, and it is
    /// a *lossy* conversion on purpose: a host that knows the provider should
    /// construct the struct so the ranking can use it. Conversion keeps the
    /// "provider unknown" case honest instead of inventing one from the label.
    fn from((id, label): (String, String)) -> Self {
        Self::without_provider(id, label)
    }
}

/// A query element matched the model id exactly.
///
/// Negative because lower ranks first, and large enough that no fuzzy score can
/// cross it: the longest id in a catalog is tens of characters, and pi's fuzzy
/// score accrues about 0.1 per character plus gap penalties.
const EXACT_BONUS: f64 = -10_000.0;

/// A query element matched the id as a prefix (or a `/`-qualified suffix).
///
/// Below [`EXACT_BONUS`] and above any realistic fuzzy total, so a prefix match
/// always outranks a mere containment.
const PREFIX_BONUS: f64 = -5_000.0;

/// Rank `entries` against `query`, best first.
///
/// An empty query returns every entry unchanged, so the caller's own order (the
/// unfiltered rule: current, then default, then alphabetical) is preserved
/// rather than re-sorted into catalog order.
#[must_use]
pub fn rank(entries: &[ModelEntry], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..entries.len()).collect();
    }
    let mut scored: Vec<(f64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.score(query).map(|score| (score, index)))
        .collect();
    // A stable sort on the score alone: equal scores keep the caller's order, so
    // the unfiltered rule still shows through a query that matches equally.
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, index)| index).collect()
}

/// Sort `indices` (into `entries`) by the unfiltered rule: current first, then
/// the default, then alphabetical by label.
///
/// Applied to the **whole list** when the picker opens and again when the query
/// clears, so the order a reader sees is defined rather than "whatever the
/// catalog arrived in" (gh #396 rule 4).
#[must_use]
pub fn default_order(
    entries: &[ModelEntry],
    current: Option<&str>,
    default: Option<&str>,
) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..entries.len()).collect();
    let rank_of = |index: &usize| -> (u8, String) {
        let index = *index;
        let entry = entries.get(index);
        let id = entry.map(|e| e.id.as_str()).unwrap_or_default();
        let is_current = current.is_some_and(|c| !c.is_empty() && id_matches(id, c));
        let is_default = default.is_some_and(|d| !d.is_empty() && id_matches(id, d));
        let bucket = if is_current {
            0
        } else if is_default {
            1
        } else {
            2
        };
        // Ties: the label, lowercased, so `GPT-4o` and `gpt-4o` adjacent. The id
        // is the second key because two providers can offer the same label.
        let sort_key = format!(
            "{}\u{0}{}",
            entry.map(|e| e.label.to_lowercase()).unwrap_or_default(),
            id.to_lowercase()
        );
        (bucket, sort_key)
    };
    indices.sort_by_key(rank_of);
    indices
}

/// Whether a row's `id` is the one an active-model string names.
///
/// The active cell may hold either the bare id or the `provider/id` form (a
/// provider switch rewrites it), so both directions are checked. Matching only
/// one spelling is how the `✓` marker went missing for qualified ids.
#[must_use]
fn id_matches(id: &str, active: &str) -> bool {
    id == active || id.ends_with(&format!("/{active}")) || active.ends_with(&format!("/{id}"))
}

#[cfg(test)]
#[path = "chat_model_search_tests.rs"]
mod tests;
