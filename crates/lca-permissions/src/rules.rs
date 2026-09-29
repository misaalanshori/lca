//! The allow/deny rules engine (ADR-0039): the rule types, glob matching,
//! and the session/project/global precedence order. Extracted from `lib.rs`
//! to keep the crate root under the house line ceiling; the behavior is
//! unchanged and `tests/rules.rs` proves the move.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An allow/deny rule's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleDecision {
    /// Never run a matching action (no prompt).
    Allow,
    /// Always refuse a matching action (no prompt).
    Deny,
}

/// Where a rule lives (permission-UX plan §3.2): a session rule is gone on
/// exit, a project rule is scoped to one repository, a global rule is the
/// user's default across every project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// In-memory only, this run.
    Session,
    /// Stored for this project.
    Project,
    /// Stored for every project (the global defaults).
    Global,
}

/// One rule as shown by the interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleView {
    /// Where the rule lives.
    pub scope: RuleScope,
    /// Allow or deny.
    pub decision: RuleDecision,
    /// The glob it matches.
    pub pattern: String,
}

/// A set of allow/deny globs (global or per-project).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    /// Patterns that auto-approve a matching action.
    #[serde(default)]
    pub allow: BTreeSet<String>,
    /// Patterns that refuse a matching action.
    #[serde(default)]
    pub deny: BTreeSet<String>,
}

/// What a rule set says about one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuleMatch {
    /// A deny rule matched; refuse without prompting.
    Deny,
    /// An allow rule matched; run without prompting.
    Allow,
    /// No rule matched.
    None,
}

/// The rule decision for one action value: deny first, then allow, across
/// session, project, and global scopes (ADR-0039). A deny rule at any scope
/// beats every allow rule.
pub(crate) fn decide(
    value: &str,
    session: &RuleSet,
    project: Option<&RuleSet>,
    global: &RuleSet,
) -> RuleMatch {
    let matches_any = |set: &BTreeSet<String>| set.iter().any(|rule| wildcard_match(rule, value));
    if matches_any(&session.deny)
        || project.is_some_and(|rules| matches_any(&rules.deny))
        || matches_any(&global.deny)
    {
        return RuleMatch::Deny;
    }
    if matches_any(&session.allow)
        || project.is_some_and(|rules| matches_any(&rules.allow))
        || matches_any(&global.allow)
    {
        return RuleMatch::Allow;
    }
    RuleMatch::None
}

/// Shell-style matching: `*` matches any run of characters, everything else
/// is exact. Exact patterns stay exact, so `git status` never becomes
/// `git push` (threat model: broad patterns are the weak point).
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    let (mut pi, mut vi) = (0usize, 0usize);
    let (mut star, mut backtrack) = (None, 0usize);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            backtrack = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            backtrack += 1;
            vi = backtrack;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}
