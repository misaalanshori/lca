//! The `#`-issue trigger: a first-party provider proving the seam (gh #408).
//!
//! This is pi's `github-issue-autocomplete.ts` example, natively: typing `#12`
//! offers open issues from the repository `gh` resolves for the working directory.
//! It exists to prove the seam end to end — a real query, real abort, real failure
//! row — not because issue completion is more valuable than a file.
//!
//! # Why `lca-ui` and not an extension
//!
//! A WASM guest cannot register a trigger yet: `wit/` has no autocomplete host
//! function and [`lca_tui::widgets::autocomplete::AutocompleteProvider`] is a Rust
//! trait (gh #408's own ABI finding). So the first consumer must be in-process, and
//! `lca-tui` may not touch `std::process` or the network (ADR-0036: the engine knows
//! nothing about the world). `lca-ui` is the host, which is where a real capability
//! call belongs — the same layering that makes the WASM version a wire-up rather
//! than a rewrite.
//!
//! # Why line-oriented `gh` output rather than `--json`
//!
//! `lca-ui` has no JSON dependency and this cycle may not add one (`Cargo.toml` is
//! out of bounds). `gh issue list` prints tab-separated columns, which is exactly
//! the three fields a row needs, so the provider parses fields rather than a
//! document. That is a deliberate trade: a `--json` parse would be more robust to
//! future `gh` output changes, and a title containing a tab would shift the columns
//! (the title field is taken whole, so only a leading tab is a real limit).

use lca_tui::widgets::autocomplete::trigger::{
    AbortSignal, QueryHandle, QueryResult, TriggerQuery,
};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

#[cfg(test)]
#[path = "autocomplete_issues_tests.rs"]
mod tests;
use lca_tui::widgets::autocomplete::{AutocompleteItem, Origin};

/// How many issues one query lists (pi's `MAX_ISSUES`).
const MAX_ISSUES: u32 = 100;
/// How many rows the popup would ever show from this provider (pi's cap).
const MAX_SUGGESTIONS: usize = 20;
/// One issue as `gh` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The issue number.
    pub number: u64,
    /// `OPEN` or `CLOSED`, as `gh` spells it.
    pub state: String,
    /// The title.
    pub title: String,
}

impl Issue {
    /// Parse one tab-separated `gh issue list` line.
    ///
    /// `gh` prints five columns - number, state, title, labels, updated - and only
    /// the first three are a row's business. Taking the title as "the rest of the
    /// line" (the obvious reading) put the labels and the timestamp into it, which
    /// made every description lie and let a fuzzy query match on a date: typing
    /// `#999` offered `#53` because its timestamp contained three nines. So the
    /// columns are taken **by position** and the rest is dropped.
    ///
    /// Returns `None` for a blank line or a line whose first field is not a number,
    /// so a banner or a warning cannot become a row.
    #[must_use]
    pub fn parse_line(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let number: u64 = fields.next()?.trim().parse().ok()?;
        let state = fields.next()?.trim();
        let title = fields.next()?.trim();
        Some(Self {
            number,
            state: state.to_string(),
            title: title.to_string(),
        })
    }

    /// The row this issue offers: `#12` inserts, the title explains.
    #[must_use]
    pub fn to_item(&self) -> AutocompleteItem {
        AutocompleteItem {
            value: format!("#{}", self.number),
            label: format!("#{}", self.number),
            // pi's shape: the state leads, so open issues are recognisable.
            description: Some(format!("[{}] {}", self.state.to_lowercase(), self.title)),
            // A `#12` row comes from the host's issue board, which is the
            // agent's own surface, not an installed extension.
            origin: Origin::Builtin,
            dim: false,
        }
    }
}

/// Keep the rows a query asked for: numeric prefix first, then fuzzy over
/// `"<number> <title>"`, capped (pi's `filterIssues`).
#[must_use]
pub fn filter_issues(issues: &[Issue], query: &str) -> Vec<AutocompleteItem> {
    let query = query.trim();
    if query.is_empty() {
        return issues
            .iter()
            .take(MAX_SUGGESTIONS)
            .map(Issue::to_item)
            .collect();
    }
    // A numeric token matches numbers by prefix, which is what a reader typing an
    // issue number means - fuzzy would happily return #1234 for `12`.
    if query.chars().all(|c| c.is_ascii_digit()) {
        let numeric: Vec<AutocompleteItem> = issues
            .iter()
            .filter(|issue| issue.number.to_string().starts_with(query))
            .take(MAX_SUGGESTIONS)
            .map(Issue::to_item)
            .collect();
        if !numeric.is_empty() {
            return numeric;
        }
    }
    // A number that matched no issue means no issue: falling through to fuzzy here
    // is how `#999` starts offering `#53`. Text queries still get the matcher.
    if query.chars().all(|c| c.is_ascii_digit()) {
        return Vec::new();
    }
    // Otherwise rank by the shared fuzzy matcher, so `#redact` finds "redaction".
    let mut scored: Vec<(f64, &Issue)> = issues
        .iter()
        .filter_map(|issue| {
            let haystack = format!("{} {}", issue.number, issue.title);
            let matched = lca_tui::widgets::autocomplete::fuzzy_match_scored(query, &haystack);
            matched.matches.then_some((matched.score, issue))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, issue)| issue.to_item())
        .collect()
}

/// The `#` trigger's query: loads `gh issue list` once per session and filters
/// locally, so a keystroke never waits on the network (pi's cache-once shape).
pub struct IssueQuery {
    /// The working directory `gh` runs in.
    cwd: std::path::PathBuf,
    /// The issues once loaded. Shared with the loading thread, so two keystrokes
    /// racing the first load cannot run `gh` twice.
    cache: Arc<Mutex<Option<Arc<Vec<Issue>>>>>,
}

impl IssueQuery {
    /// A query rooted at `cwd`.
    #[must_use]
    pub fn new(cwd: impl Into<std::path::PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Run `gh issue list` in `cwd` and parse it.
    ///
    /// A failure is copy the user can act on, never an empty list: "no issues" and
    /// "`gh` is not installed" are different problems.
    fn load(cwd: &std::path::Path) -> Result<Vec<Issue>, String> {
        let output = Command::new("gh")
            .args([
                "issue",
                "list",
                "--state",
                "open",
                "--limit",
                &MAX_ISSUES.to_string(),
            ])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                Ok(text.lines().filter_map(Issue::parse_line).collect())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let tail = stderr.lines().next_back().unwrap_or("").trim();
                Err(if tail.is_empty() {
                    "could not list issues here; is this a GitHub repository?".to_string()
                } else {
                    format!("could not list issues: {tail}")
                })
            }
            // No `gh` on PATH is the common case, so say what to install.
            Err(_) => Err("install the GitHub CLI (`gh`) to complete issue numbers".to_string()),
        }
    }
}

impl TriggerQuery for IssueQuery {
    fn start(&self, token: &str, signal: AbortSignal) -> QueryHandle {
        // `get_suggestions` is synchronous, so `start` returns *immediately*: an
        // already-loaded cache answers inline, and a first load runs on its own
        // thread while the chain's budget bounds the caller's wait.
        let (tx, rx) = mpsc::channel();
        let token = token.to_string();
        if let Some(issues) = self.cache.lock().ok().and_then(|guard| guard.clone()) {
            // An aborted query answers nothing: the chain discards it either way, and
            // not computing it is the point of the signal.
            if !signal.aborted() {
                let _ = tx.send(QueryResult::Rows(filter_issues(&issues, &token)));
            }
            return QueryHandle { rx };
        }
        let cache = Arc::clone(&self.cache);
        let cwd = self.cwd.clone();
        std::thread::spawn(move || {
            // Another keystroke may have won the race; reuse its answer.
            let already = cache.lock().ok().and_then(|guard| guard.clone());
            let issues = match already {
                Some(issues) => issues,
                None => match Self::load(&cwd) {
                    Ok(issues) => {
                        let issues = Arc::new(issues);
                        if let Ok(mut guard) = cache.lock() {
                            *guard = Some(Arc::clone(&issues));
                        }
                        issues
                    }
                    Err(reason) => {
                        let _ = tx.send(QueryResult::Failed(reason));
                        return;
                    }
                },
            };
            if signal.aborted() {
                // Nobody is waiting for this answer any more.
                return;
            }
            let _ = tx.send(QueryResult::Rows(filter_issues(&issues, &token)));
        });
        QueryHandle { rx }
    }
}
