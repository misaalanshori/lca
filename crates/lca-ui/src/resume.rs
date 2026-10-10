//! The `/resume` session picker and its search grammar (R2), ported from
//! pi's `session-selector-search.ts` (`pi-tui-re/src_re/agent-components/
//! selectors-small.md` §6).
//!
//! The grammar, verbatim:
//! - `re:<pattern>` (or `re:/<pattern>/`) is a case-insensitive regex;
//! - `"quoted phrase"` is an exact case-insensitive substring;
//! - bare whitespace-separated terms are an AND of case-insensitive
//!   substrings;
//! - an empty query matches everything, newest first (the caller's order).
//!
//! An invalid regex does not match anything and does not abort the search
//! (pi surfaces the error; we keep the row count honest instead).

use regex::Regex;

/// One session row the picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// The session id (its directory name), the resume key.
    pub id: String,
    /// The session title.
    pub title: String,
    /// Message count.
    pub messages: usize,
    /// A human age (`now`, `5m`, `3h`, …), for the row's right side.
    pub age: String,
    /// Live bookmark names in this session (gh #75): the filter
    /// matches them, the row shows them.
    pub labels: Vec<String>,
}

/// One parsed query term.
#[derive(Debug, Clone)]
enum Term {
    /// `re:<pattern>`; `None` when the pattern did not compile.
    Regex(Option<Regex>),
    /// A case-insensitive substring (quoted phrase or bare word).
    Substring(String),
}

/// Split a query into terms, honoring quoted phrases and the `re:` prefix.
///
/// An unclosed quote falls back to plain whitespace tokenization (pi's
/// graceful fallback), so a half-typed `"foo` still searches for `"foo`
/// rather than matching nothing.
fn parse_query(query: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut buffer = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' if !quoted => {
                quoted = true;
            }
            '"' if quoted => {
                quoted = false;
                if !buffer.is_empty() {
                    terms.push(Term::Substring(buffer.to_lowercase()));
                    buffer.clear();
                }
            }
            c if c.is_whitespace() && !quoted => {
                if !buffer.is_empty() {
                    terms.push(term_for(&buffer));
                    buffer.clear();
                }
            }
            c => buffer.push(c),
        }
    }
    if !buffer.is_empty() {
        // An unclosed quote is not special: treat the remainder as one term.
        terms.push(term_for(&buffer));
    }
    terms
}

fn term_for(token: &str) -> Term {
    let Some(pattern) = token.strip_prefix("re:") else {
        return Term::Substring(token.to_lowercase());
    };
    // `re:/foo/` and `re:foo` are both accepted; the slashes are optional.
    let pattern = pattern
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or(pattern);
    Term::Regex(Regex::new(&format!("(?i){pattern}")).ok())
}

/// Whether one session matches the query (all terms must match).
pub fn session_matches(entry: &SessionEntry, query: &str) -> bool {
    let terms = parse_query(query);
    if terms.is_empty() {
        return true;
    }
    let haystack =
        format!("{} {} {}", entry.title, entry.id, entry.labels.join(" ")).to_lowercase();
    terms.iter().all(|term| match term {
        Term::Substring(needle) => haystack.contains(needle.as_str()),
        Term::Regex(Some(regex)) => regex.is_match(&haystack),
        Term::Regex(None) => false,
    })
}

/// The indices of the entries that match, in the caller's order (newest
/// first when the caller lists newest first).
pub fn filter_sessions(entries: &[SessionEntry], query: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| session_matches(entry, query))
        .map(|(index, _)| index)
        .collect()
}

/// The open `/resume` picker.
#[derive(Debug, Clone)]
pub struct ResumePicker {
    /// Every session, newest first.
    pub entries: Vec<SessionEntry>,
    /// The typed search query.
    pub query: String,
    /// The indices into `entries` that match, in order.
    pub matches: Vec<usize>,
    /// The highlighted row (an index into `matches`).
    pub selected: usize,
    /// The match awaiting delete confirmation (gh #75): `Some`
    /// means `y` trashes, `n` keeps. An index into `matches`.
    pub confirming: Option<usize>,
}

impl ResumePicker {
    /// Open the picker over a session list (empty query, newest first).
    pub fn new(entries: Vec<SessionEntry>) -> ResumePicker {
        let matches = (0..entries.len()).collect();
        ResumePicker {
            entries,
            query: String::new(),
            matches,
            selected: 0,
            confirming: None,
        }
    }

    /// Re-filter after a query change, keeping the selection in range.
    /// A pending delete confirm dies with the old view (gh #75):
    /// the locked row may point anywhere now.
    pub fn refilter(&mut self) {
        self.confirming = None;
        self.matches = filter_sessions(&self.entries, &self.query);
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    /// The highlighted entry, when any.
    pub fn selected_entry(&self) -> Option<&SessionEntry> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.entries.get(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<SessionEntry> {
        vec![
            SessionEntry {
                id: "s1".into(),
                title: "Fix the parser".into(),
                messages: 4,
                age: "5m".into(),
                labels: Vec::new(),
            },
            SessionEntry {
                id: "s2".into(),
                title: "Node CVE triage".into(),
                messages: 2,
                age: "3h".into(),
                labels: Vec::new(),
            },
            SessionEntry {
                id: "s3".into(),
                title: "Docs pass".into(),
                messages: 9,
                age: "2d".into(),
                labels: Vec::new(),
            },
        ]
    }

    #[test]
    fn an_empty_query_keeps_every_session_in_order() {
        assert_eq!(filter_sessions(&entries(), ""), vec![0, 1, 2]);
    }

    #[test]
    fn bare_terms_are_an_and_of_substrings() {
        assert_eq!(filter_sessions(&entries(), "node"), vec![1]);
        assert_eq!(filter_sessions(&entries(), "node triage"), vec![1]);
        assert_eq!(
            filter_sessions(&entries(), "node parser"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn quoted_phrases_match_the_exact_substring() {
        assert_eq!(filter_sessions(&entries(), "\"node cve\""), vec![1]);
        assert_eq!(
            filter_sessions(&entries(), "\"cve node\""),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn an_unclosed_quote_still_searches() {
        assert_eq!(filter_sessions(&entries(), "\"node"), vec![1]);
    }

    #[test]
    fn a_regex_term_is_case_insensitive() {
        assert_eq!(filter_sessions(&entries(), "re:/^fix/"), vec![0]);
        assert_eq!(filter_sessions(&entries(), "re:doc"), vec![2]);
    }

    #[test]
    fn an_invalid_regex_matches_nothing_without_panicking() {
        assert_eq!(filter_sessions(&entries(), "re:("), Vec::<usize>::new());
    }

    #[test]
    fn refilter_keeps_the_selection_in_range() {
        let mut picker = ResumePicker::new(entries());
        picker.selected = 2;
        picker.query = "node".into();
        picker.refilter();
        assert_eq!(picker.matches, vec![1]);
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.selected_entry().map(|e| e.id.as_str()), Some("s2"));
    }
}
