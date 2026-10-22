//! Turning what a user typed into a rule that actually matches (gh #261).
//!
//! # The defect this fixes
//!
//! `/permissions deny read muffin-recipe.txt` was accepted, confirmed as
//! `deny rule read muffin-recipe.txt`, and then did nothing. Two reasons, and
//! both had to go:
//!
//! 1. **The verb was stored as part of the glob.** A rule is matched against
//!    `Action::match_value`, which for a read is the *resolved absolute path*
//!    (`/home/me/proj/muffin-recipe.txt`) - never the word `read`. So the stored
//!    pattern `read muffin-recipe.txt` matched no action at all, and neither did
//!    `read /home/me/proj/muffin-recipe.txt`.
//! 2. **A bare relative path cannot match an absolute value.** Even with the verb
//!    stripped, `muffin-recipe.txt` is an exact-exact comparison against
//!    `/home/me/proj/muffin-recipe.txt`, which is false. The workaround people
//!    found was `*muffin-recipe.txt`, which works because `*` swallows the
//!    directory - and which is exactly what a literal path should have compiled
//!    to.
//!
//! # What a rule means now
//!
//! A verb-qualified rule declares *which kind of action* it is about, and its
//! path is compiled into the form the matcher actually compares:
//!
//! | typed | stored | because |
//! |---|---|---|
//! | `read muffin-recipe.txt` | `<dir>/*muffin-recipe.txt` | the value is an absolute path |
//! | `read /etc/hostname` | `/etc/hostname` | already absolute and exact |
//! | `read /etc/*` | `/etc/*` | a glob passes through untouched |
//! | `write out/*.txt` | `<dir>/*out/*.txt` | same rule, write actions |
//! | `git push*` | `git push*` | no verb: a shell rule, unchanged |
//!
//! The relative form is anchored with a leading `*` rather than being joined to
//! the project directory, so a rule written in one project keeps working from a
//! subdirectory and after the project moves. That is deliberately *broader* than
//! "this exact file in this exact directory" in one respect: it also matches the
//! same relative tail anywhere on disk. The alternative - baking the absolute
//! working directory in - is narrower but breaks the moment the user runs from a
//! subdirectory or renames the checkout, and a rule that silently stops matching
//! is the very failure this module exists to remove. Broad-in-a-known-way beats
//! narrow-in-a-surprising-way; the confirmation line states the pattern, and the
//! `read`/`write` verbs keep the blast radius to one action kind.
//!
//! # What is refused
//!
//! A rule that cannot match anything is refused at entry, with the accepted forms
//! named. Storing a dead rule is how the original bug happened, so the entry
//! point is where it gets caught: an unknown verb, an empty path, or a path
//! consisting only of `*` (which would be every file on the machine dressed up as
//! a targeted rule).
//!
//! # Precedence is untouched
//!
//! Deny still beats allow at any scope (ADR-0039). This module only decides what
//! string lands in the rule set; `rules::decide` still evaluates deny
//! first, so a compiled deny cannot lose to an allow it should beat.

use std::path::Path;

/// The action kind a rule is about, when the user named one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleVerb {
    /// `read <path>`: matches [`crate::Action::ReadPath`].
    Read,
    /// `write <path>`: matches [`crate::Action::WritePath`].
    Write,
}

/// What a compiled rule is about, for the confirmation line and the docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRule {
    /// The action kind named by the verb, or `None` for a bare glob.
    pub verb: Option<RuleVerb>,
    /// The pattern that will be matched, in the form the matcher compares.
    pub pattern: String,
    /// The words the user typed, for an unambiguous confirmation.
    pub typed: String,
}

/// How a rule is written so the confirmation can describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleRejection {
    /// A word one keystroke from an action kind, used with a path.
    UnknownVerb(String, String),
    /// A verb with nothing after it.
    EmptyPattern(RuleVerb),
    /// A path that is only wildcards: every file, not a rule.
    MatchesEverything(String),
}

impl std::fmt::Display for RuleRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleRejection::UnknownVerb(verb, suggestion) => write!(
                f,
                "`{verb}` is not an action kind - did you mean `{suggestion}`? Rules look \
                 like `/permissions deny read <path>`, `/permissions deny write <path>`, \
                 or a bare glob such as `/permissions deny git push*` (verbs: read, \
                 write)"
            ),
            RuleRejection::EmptyPattern(verb) => {
                let verb = match verb {
                    RuleVerb::Read => "read",
                    RuleVerb::Write => "write",
                };
                write!(
                    f,
                    "`{verb}` needs a path: `/permissions deny {verb} <path>`"
                )
            }
            RuleRejection::MatchesEverything(pattern) => write!(
                f,
                "`{pattern}` matches every file; name something, e.g. \
                 `/permissions deny read secrets.txt`"
            ),
        }
    }
}

impl std::error::Error for RuleRejection {}

/// Words that name an action kind. Kept as one list so the parser, the docs and
/// the refusal message cannot drift apart.
pub const VERBS: &[(&str, RuleVerb)] = &[("read", RuleVerb::Read), ("write", RuleVerb::Write)];

/// The verb `word` is one slip away from, if any.
///
/// One substitution, one insertion, one deletion, or one transposition - the
/// four ways a typo happens. Deliberately not general edit distance: a name two
/// edits from `read` (`reede`? `bread`?) is more likely a real program than a
/// typo, and refusing it would be worse than storing a rule that does not match,
/// because it would block work the user meant to allow.
fn near_miss(word: &str) -> Option<&'static str> {
    let lowered = word.to_ascii_lowercase();
    let word: Vec<char> = lowered.chars().collect();
    for (verb, _) in VERBS {
        let candidate: Vec<char> = verb.chars().collect();
        // The first letter has to survive. A mistyped verb is a slip *inside* a
        // word you were already typing; `bread`, `dread`, `tread` share their
        // shape with `read` but are different words, and my own row caught the
        // false positive (`bread --toast` refused as one insertion away). Real
        // typos - a transposition, a doubled letter, a dropped letter, a wrong
        // letter after the first - all keep the first character.
        if word.first() != candidate.first() {
            continue;
        }
        if is_one_edit(&word, &candidate) {
            return Some(verb);
        }
    }
    None
}

/// Whether two short words are one edit apart (Damerau-Levenshtein distance 1).
fn is_one_edit(left: &[char], right: &[char]) -> bool {
    if left == right {
        return false;
    }
    match left.len().abs_diff(right.len()) {
        0 => {
            // Same length: one substitution, or one adjacent transposition.
            let differences: Vec<usize> = (0..left.len())
                .filter(|index| left[*index] != right[*index])
                .collect();
            match differences.as_slice() {
                [_single] => true,
                [first, second] => {
                    // A transposition must be adjacent and swap exactly these.
                    second - first == 1
                        && left[*first] == right[*second]
                        && left[*second] == right[*first]
                }
                _ => false,
            }
        }
        // One insertion or one deletion.
        1 => {
            let (shorter, longer) = if left.len() < right.len() {
                (left, right)
            } else {
                (right, left)
            };
            let mut i = 0;
            let mut j = 0;
            let mut skipped = false;
            while i < shorter.len() && j < longer.len() {
                if shorter[i] == longer[j] {
                    i += 1;
                    j += 1;
                } else if skipped {
                    return false;
                } else {
                    skipped = true;
                    j += 1;
                }
            }
            true
        }
        _ => false,
    }
}

/// Compile what the user typed into a rule.
///
/// `typed` is the whole `<decision> ...` payload after the decision word - i.e.
/// `read muffin-recipe.txt` from `/permissions deny read muffin-recipe.txt`.
/// `project_dir` is only used to recognise a path the user typed in full, never
/// to pin a relative one (see the module docs).
///
/// # Errors
///
/// [`RuleRejection`], naming the accepted forms, when the input could not match
/// anything.
pub fn compile(typed: &str, project_dir: &Path) -> Result<CompiledRule, RuleRejection> {
    let typed = typed.trim();
    let (first, rest) = match typed.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest.trim()),
        None => (typed, ""),
    };
    // The verb matches case-insensitively: `Read x` is the same intent as
    // `read x`, and treating it as a bare glob would store a rule that matches
    // nothing a read action carries - the silent no-op again, from a shift key.
    let found = VERBS
        .iter()
        .find(|(word, _)| word.eq_ignore_ascii_case(first));
    let Some((_, verb)) = found else {
        // No verb. That is the normal shape for a shell or net rule
        // (`git push*`, `example.com`), and for every rule stored before this
        // cycle, so the pattern passes through unchanged.
        if typed.is_empty() {
            return Err(RuleRejection::EmptyPattern(RuleVerb::Read));
        }
        // gh #261: one exception, because it is the typo this issue is made of.
        // `raed muffin-recipe.txt` looks exactly like `git push*` structurally -
        // word, space, rest - but the word is one keystroke from a real verb. Such
        // a rule would be stored as a *bare glob* and match nothing a read action
        // carries, which is the silent no-op all over again. Refusing it with
        // `did you mean read?` costs a real user nothing (`raed` is not a program
        // anyone denies) and saves exactly the person this bug was filed by.
        if !rest.is_empty()
            && let Some(suggestion) = near_miss(first)
        {
            return Err(RuleRejection::UnknownVerb(
                first.to_string(),
                suggestion.to_string(),
            ));
        }
        return Ok(CompiledRule {
            verb: None,
            pattern: typed.to_string(),
            typed: typed.to_string(),
        });
    };
    if rest.is_empty() {
        return Err(RuleRejection::EmptyPattern(*verb));
    }
    let pattern = compile_path(rest, project_dir, *verb)?;
    Ok(CompiledRule {
        verb: Some(*verb),
        pattern,
        typed: typed.to_string(),
    })
}

/// Compile the path half of a verb-qualified rule.
///
/// The action's match value is an absolute path, so a relative rule gets a
/// leading `*` to span the directory. Anything already anchored (absolute, or
/// already starting with a wildcard) passes through, which is what keeps the
/// `*muffin-recipe.txt` workaround working byte-for-byte.
fn compile_path(path: &str, project_dir: &Path, verb: RuleVerb) -> Result<String, RuleRejection> {
    let path = path.trim();
    if path.is_empty() {
        return Err(RuleRejection::EmptyPattern(verb));
    }
    // Only wildcards is not a rule about anything.
    if path
        .chars()
        .all(|c| c == '*' || c == '?' || c.is_whitespace())
    {
        return Err(RuleRejection::MatchesEverything(path.to_string()));
    }
    // Already a glob the user anchored: leave it alone. `*muffin-recipe.txt`,
    // `/etc/*`, and `**/x` all keep their meaning.
    if path.starts_with('*') || path.starts_with('?') {
        return Ok(path.to_string());
    }
    // An absolute path is compared as written; no directory to span.
    if Path::new(path).is_absolute() {
        return Ok(path.to_string());
    }
    // A relative path: `*` spans the directory the action resolved against, so
    // the rule keeps working from a subdirectory and after the checkout moves.
    // Recognising that the user *may* have typed a path already relative to the
    // project (with a `./` or `../`) changes nothing here - the tail is what
    // matters once the leading `*` is in place.
    let tail = path.trim_start_matches("./");
    if tail.is_empty() {
        return Err(RuleRejection::EmptyPattern(verb));
    }
    let _ = project_dir;
    Ok(format!("*{tail}"))
}

#[cfg(test)]
#[path = "rules_compile_tests.rs"]
mod tests;
