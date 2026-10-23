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
    /// The session's own last-write time, epoch milliseconds (gh #395).
    ///
    /// The tree's ordering key, and it has to be the *raw* timestamp rather than
    /// the rendered `age`: "3h" cannot order two sessions an hour apart, and a
    /// recursive sort compares these numbers at every level.
    pub modified_ms: u64,
    /// The parent session's id from this session's `fork-point` record
    /// (gh #395), or `None` for a root.
    ///
    /// **This is the actual linkage.** ADR-0046's design records a fork as a child
    /// directory whose log opens with a `fork-point` naming the parent; there is no
    /// `parentSessionPath` field here. `Store::rebuild_index` already reads that
    /// record into `SessionSummary::parent_session`, so this is the same value the
    /// store has always had - it simply never reached the picker.
    ///
    /// A parent that no longer exists is still a *value*; it is the tree builder
    /// that treats a dangling parent as a root, so a deleted parent never hides a
    /// resumable session.
    pub parent: Option<String>,
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

/// One node of the shape the picker paints (gh #395): the entry index, its depth,
/// and whether it is the last child of its parent.
///
/// The prefix is *derived* at paint time rather than stored, because the connector
/// depends on the ancestors' sibling states - a row's own `last` is not enough to
/// know whether its parent's column continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeNode {
    /// Index into the picker's `entries`.
    pub entry: usize,
    /// Nesting depth (0 for a root).
    pub depth: usize,
    /// Whether this is the last child of its parent (draws `└─` instead of `├─`).
    pub last: bool,
}

/// The entry index a count of subtree activity: the newest `modified_ms` anywhere
/// below it, the node itself included (gh #395).
///
/// A parent sorting by its own last write would sink a family whose work happened
/// one level deeper - which, after two forks, is exactly where the user's recent
/// work is.
fn subtree_activity(entries: &[SessionEntry], children: &[Vec<usize>], node: usize) -> u64 {
    let mut newest = entries.get(node).map_or(0, |entry| entry.modified_ms);
    for child in &children[node] {
        newest = newest.max(subtree_activity(entries, children, *child));
    }
    newest
}

/// Build the tree rows (gh #395): roots in subtree-activity order, each followed by
/// its descendants depth-first.
///
/// # The two guards
///
/// **A dangling parent is a root.** A session whose `parent` names a session that is
/// not in the list is still resumable, so it must get a row - at the top, ordered by
/// its own activity. Hiding it would lose work silently.
///
/// **A cycle is broken, not looped.** Two sessions naming each other, or one naming
/// itself, would recurse forever. `visited` makes every session appear exactly once:
/// the first cycle member reached becomes a root and the rest hang off it. That is a
/// *shape* degradation, never a hang - which is the trade the picker must make,
/// because a hanging picker is worse than the flat list it replaced.
fn build_tree(entries: &[SessionEntry]) -> Vec<TreeNode> {
    let len = entries.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); len];
    // A parent is only a parent if it is in the list *and* it is not this node:
    // otherwise the edge is dropped and the child is a root.
    let mut is_child = vec![false; len];
    for (index, entry) in entries.iter().enumerate() {
        let Some(parent) = entry.parent.as_deref() else {
            continue;
        };
        if parent == entry.id {
            continue;
        }
        if let Some(parent_index) = entries.iter().position(|other| other.id == parent) {
            children[parent_index].push(index);
            is_child[index] = true;
        }
    }

    // Break any remaining cycle: walk each node's ancestor chain; a node that
    // reaches itself is inside a cycle, and the first such node reached in document
    // order is promoted to a root so the chain below it terminates.
    for index in 0..len {
        if !is_child[index] {
            continue;
        }
        let mut seen = vec![false; len];
        let mut cursor = index;
        loop {
            if seen[cursor] {
                // `index` is in a cycle; cut the edge that points at it.
                let parent_id = entries[index].parent.clone();
                if let Some(parent_index) = parent_id
                    .as_deref()
                    .and_then(|id| entries.iter().position(|other| other.id == id))
                {
                    children[parent_index].retain(|child| *child != index);
                }
                is_child[index] = false;
                break;
            }
            seen[cursor] = true;
            let Some(parent) = entries[cursor].parent.as_deref() else {
                break;
            };
            match entries.iter().position(|other| other.id == parent) {
                Some(parent_index) if parent_index != cursor => cursor = parent_index,
                _ => break,
            }
        }
    }

    // Order every sibling list by subtree activity, then by id so equal activity is
    // deterministic - the list must not reshuffle between opens.
    let activity: Vec<u64> = (0..len)
        .map(|index| subtree_activity(entries, &children, index))
        .collect();
    for list in children.iter_mut() {
        list.sort_by(|a, b| {
            activity[*b]
                .cmp(&activity[*a])
                .then_with(|| entries[*a].id.cmp(&entries[*b].id))
        });
    }

    // Roots: every node with no parent edge, in the same order.
    let mut roots: Vec<usize> = (0..len).filter(|index| !is_child[*index]).collect();
    roots.sort_by(|a, b| {
        activity[*b]
            .cmp(&activity[*a])
            .then_with(|| entries[*a].id.cmp(&entries[*b].id))
    });

    let mut out = Vec::with_capacity(len);
    // Depth-first, so a block is contiguous. An explicit stack rather than
    // recursion: the `visited` set is the cycle guard, and an iterative walk cannot
    // overflow on a pathological chain either.
    let mut stack: Vec<(usize, usize, bool)> = roots
        .into_iter()
        .rev()
        .map(|root| (root, 0usize, true))
        .collect();
    let mut visited = vec![false; len];
    while let Some((node, depth, last)) = stack.pop() {
        if visited[node] {
            continue;
        }
        visited[node] = true;
        out.push(TreeNode {
            entry: node,
            depth,
            last,
        });
        let kids = &children[node];
        for (position, child) in kids.iter().enumerate().rev() {
            let is_last = position + 1 == kids.len();
            stack.push((*child, depth + 1, is_last));
        }
    }
    // A node made unreachable by a broken cycle still deserves a row.
    for (index, seen) in visited.iter().enumerate() {
        if !seen {
            out.push(TreeNode {
                entry: index,
                depth: 0,
                last: true,
            });
        }
    }
    out
}

/// The connector prefix for a tree row (gh #395): the ancestor columns, then `├─`
/// or `└─` for the row itself.
///
/// `ancestors_last` is one flag per ancestor level: `true` where that ancestor was
/// the last of its siblings, which means no sibling follows it and the column under
/// it is blank rather than `│`. Getting this wrong is the classic tree bug - a bar
/// drawn under a finished block makes two families look like one.
fn connector(ancestors_last: &[bool], last: bool) -> String {
    let mut out = String::new();
    for ancestor_is_last in ancestors_last {
        out.push_str(if *ancestor_is_last { "   " } else { "│  " });
    }
    out.push_str(if last { "└─ " } else { "├─ " });
    out
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
    /// The tree rows (gh #395), built **once** when the query is empty.
    ///
    /// Empty while a query is active, because an active search paints the flat
    /// scored list - a filtered tree spends rows on ancestors that did not match.
    /// Computed here rather than per frame: the build walks every entry's ancestry,
    /// and `#389`'s lesson is that no render path does I/O-shaped work per frame.
    tree: Vec<TreeNode>,
}

impl ResumePicker {
    /// Open the picker over a session list (empty query, newest first).
    pub fn new(entries: Vec<SessionEntry>) -> ResumePicker {
        let matches = (0..entries.len()).collect();
        let mut picker = ResumePicker {
            entries,
            query: String::new(),
            matches,
            selected: 0,
            confirming: None,
            tree: Vec::new(),
        };
        picker.rebuild();
        picker
    }

    /// Recompute `matches` (and the tree when the query is empty) from the current
    /// query (gh #395).
    ///
    /// One function so the two views can never disagree: with a query the matches
    /// are the scored set, without one they are the tree's flattened order - which
    /// is what makes "clear the query, the tree returns" a property rather than a
    /// coincidence.
    fn rebuild(&mut self) {
        if self.query.trim().is_empty() {
            self.tree = build_tree(&self.entries);
            self.matches = self.tree.iter().map(|node| node.entry).collect();
        } else {
            self.tree.clear();
            self.matches = filter_sessions(&self.entries, &self.query);
        }
    }

    /// The painted rows, connector prefixes included (gh #395).
    ///
    /// The connector needs one column per **ancestor below the root**: a depth-0
    /// row is a heading, not a boxed row, so it draws no column and a depth-1 child
    /// starts flush at `├─`. A depth-2 row under a non-last parent draws
    /// `│  └─`; under a last parent, `   └─`.
    ///
    /// `path[d]` holds the `last` flag of the node at depth `d` on the current
    /// walk, which is what makes the columns correct at every depth without
    /// re-deriving the parent from the list.
    pub fn paint_rows(&self) -> Vec<String> {
        if self.tree.is_empty() {
            return self
                .matches
                .iter()
                .filter_map(|index| self.entries.get(*index))
                .map(|entry| entry.title.clone())
                .collect();
        }
        let mut rows = Vec::with_capacity(self.tree.len());
        let mut path: Vec<bool> = Vec::new();
        for node in &self.tree {
            let Some(entry) = self.entries.get(node.entry) else {
                continue;
            };
            // Keep only this node's ancestors (depths 0..depth).
            path.truncate(node.depth);
            // A root is a heading: no column, no connector at all.
            let prefix = if node.depth == 0 {
                String::new()
            } else {
                // Columns come from ancestors at depth >= 1; the root draws none.
                let columns = &path[1.min(path.len())..];
                connector(columns, node.last)
            };
            rows.push(format!("{prefix}{}", entry.title));
            path.push(node.last);
        }
        rows
    }

    /// Re-filter after a query change, keeping the selection in range.
    /// A pending delete confirm dies with the old view (gh #75):
    /// the locked row may point anywhere now.
    pub fn refilter(&mut self) {
        self.confirming = None;
        self.rebuild();
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
                modified_ms: 0,
                parent: None,
            },
            SessionEntry {
                id: "s2".into(),
                title: "Node CVE triage".into(),
                messages: 2,
                age: "3h".into(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
            },
            SessionEntry {
                id: "s3".into(),
                title: "Docs pass".into(),
                messages: 9,
                age: "2d".into(),
                labels: Vec::new(),
                modified_ms: 0,
                parent: None,
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
