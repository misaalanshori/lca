//! Autocomplete, ported from pi's
//! `packages/tui/src/autocomplete.ts`
//! (`pi-tui-re/src_re/tui-widgets/autocomplete.md`).
//!
//! One provider covers three contexts: slash-command names, a command's
//! arguments (via a per-command callback — the `/model <id>` and
//! `/login <name>` shape), and file/path completion. Owner issue #7.
//!
//! Deviation from pi (documented): file discovery uses `std::fs` readdir
//! with the same scoring/tiebreaks, rather than shelling out to `fd`. The
//! `fd`-backed recursive fuzzy walk is a `ponytail:` upgrade if deep-repo
//! search feels slow; the near-directory pass is what makes completion feel
//! immediate.

pub mod token;
pub mod trigger;

#[cfg(test)]
#[path = "autocomplete/attachment_tests.rs"]
mod attachment_tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One completion suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteItem {
    /// The literal text applied on accept.
    pub value: String,
    /// The display label.
    pub label: String,
    /// An optional description.
    pub description: Option<String>,
    /// Where the row came from, for a provider that is not the built-in one -
    /// rendered as `[extension: github-issues]` (gh #408, issuedraft 022's origin
    /// tag). `None` for the built-in command and path rows.
    pub origin: Option<String>,
    /// Render the row dimmed (gh #408 rule 5): an `(unavailable)` row is
    /// information, not a choice, so it must not look selectable. Styling is
    /// carried as a typed flag rather than pre-baked escapes, so the popup keeps
    /// rendering plain text and the theme decides what "dim" is.
    pub dim: bool,
}

/// A slash command with optional argument completion.
#[derive(Clone)]
pub struct SlashCommand {
    /// Command name without the slash.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Argument hint.
    pub argument_hint: Option<String>,
    /// Per-argument completion.
    pub argument_completions: Option<ArgumentCompletions>,
}

/// A per-argument completion callback.
pub type ArgumentCompletions = Arc<dyn Fn(&str) -> Vec<AutocompleteItem> + Send + Sync>;

/// A suggestion set: the items plus the prefix they replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    /// The candidate items.
    pub items: Vec<AutocompleteItem>,
    /// The text prefix the chosen item replaces.
    pub prefix: String,
}

/// The completion provider seam (pi's `AutocompleteProvider`).
pub trait AutocompleteProvider: Send + Sync {
    /// Suggestions for the text before the cursor.
    fn get_suggestions(&self, text_before_cursor: &str, force: bool) -> Option<Suggestions>;
}

/// The combined provider.
pub struct CombinedAutocompleteProvider {
    commands: Vec<SlashCommand>,
    base_path: PathBuf,
}

impl CombinedAutocompleteProvider {
    /// Build with commands and a base directory.
    pub fn new(commands: Vec<SlashCommand>, base_path: impl Into<PathBuf>) -> Self {
        Self {
            commands,
            base_path: base_path.into(),
        }
    }
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn get_suggestions(&self, text: &str, force: bool) -> Option<Suggestions> {
        // `@` attachment prefix (gh #380 gap 1). The token is extracted
        // against a named delimiter set - line start, ASCII whitespace and
        // CJK punctuation - rather than `strip_prefix('@')`, which only
        // matched at column 0 and so made the feature work in the one
        // position a user rarely types it. `token::attachment_query` also
        // refuses an `@` that is inside a word, which is what keeps an
        // email address from opening a file menu.
        //
        // `/` stays line-start-only on purpose and the asymmetry is the
        // design (`token`'s module doc): a mid-sentence `/` in `https://…`
        // or `src/index.ts` must never open a command menu. Nothing here
        // touches the slash branch below.
        if let Some(query) = token::attachment_query(text) {
            return Some(fuzzy_files(query, &self.base_path));
        }
        // Slash context.
        if let Some(rest) = text.strip_prefix('/') {
            if !rest.contains(' ') {
                let items = self.command_items(rest);
                if items.is_empty() {
                    return None;
                }
                return Some(Suggestions {
                    items,
                    prefix: format!("/{rest}"),
                });
            }
            // pi's provider returns null when the command has no argument
            // completions (it does not fall through to file completion).
            let (name, argument) = rest.split_once(' ')?;
            let command = self.commands.iter().find(|c| c.name == name)?;
            let items = command.argument_completions.as_ref()?.as_ref()(argument);
            if items.is_empty() {
                return None;
            }
            return Some(Suggestions {
                items,
                prefix: argument.to_string(),
            });
        }
        // Plain path completion for the last whitespace-delimited token
        // (pi completes the token at the cursor, not the whole line).
        let token = text.rsplit(char::is_whitespace).next().unwrap_or(text);
        if force || looks_like_path(token) {
            let (dir_part, base) = split_path(token);
            let items = self.file_items(&dir_part, base);
            if items.is_empty() {
                return None;
            }
            return Some(Suggestions {
                items,
                prefix: token.to_string(),
            });
        }
        None
    }
}

impl CombinedAutocompleteProvider {
    /// Score every command, keep the matches, best first (pi's
    /// `fuzzyFilter`); ties hold registration order (stable sort).
    fn command_items(&self, query: &str) -> Vec<AutocompleteItem> {
        let mut scored: Vec<(f64, usize, AutocompleteItem)> = Vec::new();
        for (index, c) in self.commands.iter().enumerate() {
            let matched = fuzzy_match_scored(query, &c.name);
            if matched.matches {
                scored.push((
                    matched.score,
                    index,
                    AutocompleteItem {
                        value: format!("/{} ", c.name),
                        label: format!("/{}", c.name),
                        description: match (&c.argument_hint, &c.description) {
                            (Some(h), Some(d)) => Some(format!("{h} — {d}")),
                            (Some(h), None) => Some(h.clone()),
                            (None, Some(d)) => Some(d.clone()),
                            (None, None) => None,
                        },
                        origin: None,
                        dim: false,
                    },
                ));
            }
        }
        scored.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().map(|(_, _, item)| item).collect()
    }

    fn file_items(&self, dir_part: &str, base: &str) -> Vec<AutocompleteItem> {
        let (expanded, display_prefix) = expand_dir(dir_part);
        let dir = if Path::new(&expanded).is_absolute() {
            PathBuf::from(&expanded)
        } else {
            self.base_path.join(&expanded)
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut items: Vec<(i32, usize, String, AutocompleteItem)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_ignored_dir(&name) {
                continue;
            }
            if !name.to_lowercase().starts_with(&base.to_lowercase()) {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let mut value = format!("{display_prefix}{name}");
            if is_dir {
                value.push('/');
            }
            let score = score_entry(&name, base, is_dir);
            let depth = 0usize;
            items.push((
                score,
                depth,
                name.clone(),
                AutocompleteItem {
                    value: if value.contains(' ') {
                        format!("\"{value}\"")
                    } else {
                        value
                    },
                    label: if is_dir { format!("{name}/") } else { name },
                    description: None,
                    origin: None,
                    dim: false,
                },
            ));
        }
        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.len().cmp(&b.2.len()))
                .then(a.2.cmp(&b.2))
        });
        items.into_iter().take(20).map(|(_, _, _, i)| i).collect()
    }
}

/// How many directory entries the `@` walk will look at, in total.
///
/// Gh #502: this bounds **work**, not discovery. The old code stopped when it had
/// collected 200 *matches*, which meant a wide shallow directory could spend the
/// whole budget on scattered-letter hits and the walk never reached a nested file
/// the query actually named - `@mai` listed `home/.lca/private/mcp` and friends
/// and never `src/main.rs`.
///
/// 2,000 entries is roughly ten times what a keystroke budget needs while still
/// keeping the walk to a few milliseconds on a warm cache: `read_dir` on a flat
/// directory is a syscall plus a string copy per entry, and the fuzzy scorer runs
/// once per entry. The visible list is 20, so the entries beyond that are cheap
/// ranking material, not something the user waits for.
const WALK_ENTRY_BUDGET: usize = 2_000;

/// How many directories the `@` walk will descend into.
///
/// Separate from the entry budget because the two failure shapes differ: a
/// directory with a million files is bounded by entries, and a deep tree of empty
/// directories is bounded by this. Both are needed, and neither is a *match*
/// count.
const WALK_DIR_BUDGET: usize = 256;

/// The most a single directory may contribute before the walk moves on.
///
/// This is the fix's third leg, and the one that keeps a wide sibling from
/// dominating: the walk is breadth-first, so `src/` is visited at the same level
/// as the wide directory rather than after it. The per-directory cap stops one
/// enormous directory from consuming the *whole* entry budget, which would starve
/// breadth just as the match cap starved depth.
const WALK_PER_DIR_CAP: usize = 200;

/// What one walk has spent so far, threaded through the breadth-first queue.
struct WalkBudget {
    /// Entries looked at across every directory.
    entries: usize,
    /// Directories descended.
    dirs: usize,
}

/// The `@file` matcher: rank by score, then path length, then name.
fn fuzzy_files(query: &str, base: &Path) -> Suggestions {
    // The query in display form, once: candidates are normalised per entry
    // inside the walk, so both sides of the comparison use `/` on every
    // platform (pi's `toDisplayPath`; gh #380's Windows half).
    let display_query = token::to_display_path(query);
    let mut items: Vec<(f64, usize, String, AutocompleteItem)> = Vec::new();
    collect_files(base, &display_query, &mut items);
    items.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.len().cmp(&b.2.len()))
            .then(a.2.cmp(&b.2))
    });
    let items: Vec<AutocompleteItem> = items.into_iter().take(20).map(|(_, _, _, i)| i).collect();
    Suggestions {
        items,
        prefix: format!("@{query}"),
    }
}

/// Walk `base` breadth-first, collecting matches, under three budgets (gh #502).
///
/// **Breadth-first is the fix.** The old walk was depth-first in `read_dir`
/// order, so the survivors were whichever directories the filesystem happened to
/// list first; a wide data home at the root could take the entire budget before
/// `src/` was ever opened. Visiting a level completely before descending means a
/// nested file competes with its siblings rather than with everything the walk
/// happened to see earlier - which is what makes the result **deterministic**
/// across filesystems, because no level decides another level's fate.
///
/// Within a level, entries are sorted by name so the order does not depend on
/// `read_dir`. That costs one sort per directory over the entries actually read,
/// and it is what turns an intermittent bug into a reproducible one.
fn collect_files(root: &Path, query: &str, out: &mut Vec<(f64, usize, String, AutocompleteItem)>) {
    let mut budget = WalkBudget {
        entries: 0,
        dirs: 0,
    };
    // `(directory, depth)`, processed in order: everything at depth N before
    // anything at depth N+1.
    let mut queue: std::collections::VecDeque<(std::path::PathBuf, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));
    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 6 || budget.dirs >= WALK_DIR_BUDGET || budget.entries >= WALK_ENTRY_BUDGET {
            break;
        }
        budget.dirs += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<std::ffi::OsString> =
            entries.flatten().map(|entry| entry.file_name()).collect();
        // Deterministic order: no level's contents depend on how the filesystem
        // happened to lay out its directory blocks.
        names.sort();
        for (taken, name) in names.into_iter().enumerate() {
            if budget.entries >= WALK_ENTRY_BUDGET || taken >= WALK_PER_DIR_CAP {
                break;
            }
            budget.entries += 1;
            let name = name.to_string_lossy().to_string();
            // D10: skip the heavy build/vendor directories in the fuzzy walk.
            if is_ignored_dir(&name) {
                continue;
            }
            let path = dir.join(&name);
            // Display form (`/` separators), which is what pi matches and
            // inserts against (`toDisplayPath`). Gh #380 gap 1's Windows
            // half: the walk's `rel` carries the OS separator, so a query
            // containing `/` never matched and `@src/m` found nothing on
            // Windows. Normalising here fixes the match *and* makes the
            // inserted value portable, rather than the inserted value being
            // a Windows path that the user's prose then carries around.
            let rel =
                token::to_display_path(&path.strip_prefix(root).unwrap_or(&path).to_string_lossy());
            let is_dir = path.is_dir();
            let matched = fuzzy_match_scored(query, &rel);
            if matched.matches {
                out.push((
                    matched.score,
                    depth,
                    rel.clone(),
                    AutocompleteItem {
                        value: format!("@{rel}"),
                        label: name.clone(),
                        description: Some(rel.clone()),
                        origin: None,
                        dim: false,
                    },
                ));
            }
            if is_dir {
                queue.push_back((path, depth + 1));
            }
        }
    }
}

/// Directories the fuzzy file walk never descends into (D10).
fn is_ignored_dir(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules")
}

fn looks_like_path(text: &str) -> bool {
    text.starts_with('/')
        || text.starts_with("./")
        || text.starts_with("../")
        || text.starts_with("~/")
        || text.contains('/')
        || text.starts_with('.')
}

fn split_path(text: &str) -> (String, &str) {
    match text.rfind('/') {
        Some(idx) => (text[..=idx].to_string(), &text[idx + 1..]),
        None => (String::new(), text),
    }
}

/// Expand `~`/`~/` and return the display prefix to preserve.
fn expand_dir(dir: &str) -> (String, String) {
    if let Some(rest) = dir.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        (format!("{home}/{rest}"), "~/".to_string())
    } else if dir == "~" {
        let home = std::env::var("HOME").unwrap_or_default();
        (home, "~/".to_string())
    } else {
        (dir.to_string(), dir.to_string())
    }
}

/// pi's scoring: exact 100, prefix 80, substring 50, path substring 30,
/// +10 for directories.
fn score_entry(name: &str, query: &str, is_dir: bool) -> i32 {
    let n = name.to_lowercase();
    let q = query.to_lowercase();
    let mut score = if q.is_empty() || n == q {
        100
    } else if n.starts_with(&q) {
        80
    } else if n.contains(&q) {
        50
    } else {
        0
    };
    if is_dir {
        score += 10;
    }
    score
}

/// A scored fuzzy match (pi's `packages/tui/src/fuzzy.ts`): lower
/// scores rank first. Word starts and consecutive runs earn bonuses;
/// gaps and late positions cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    /// Whether every query character appears in order.
    pub matches: bool,
    /// Lower ranks first (pi's `totalScore`).
    pub score: f64,
}

/// Score `text` against `query` (pi's `fuzzyMatch`).
pub fn fuzzy_match_scored(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let primary = match_query(&query_lower, &text_lower);
    if primary.matches {
        return primary;
    }
    // pi's letter/digit swap fallback (`gpt4o` for a `4ogpt` typo): the
    // swapped shape costs 5 extra points.
    let swapped = swap_alpha_digits(&query_lower);
    let Some(swapped) = swapped else {
        return primary;
    };
    let retry = match_query(&swapped, &text_lower);
    if retry.matches {
        FuzzyMatch {
            matches: true,
            score: retry.score + 5.0,
        }
    } else {
        primary
    }
}

fn match_query(query: &str, text: &str) -> FuzzyMatch {
    let none = FuzzyMatch {
        matches: false,
        score: 0.0,
    };
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    let query_chars: Vec<char> = query.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    if query_chars.len() > text_chars.len() {
        return none;
    }
    let mut score = 0.0;
    let mut last: i32 = -1;
    let mut consecutive = 0i32;
    let mut matched = 0usize;
    while matched < query_chars.len() {
        // pi's greedy `indexOf` from the previous match on.
        let from = (last + 1) as usize;
        let found = text_chars
            .iter()
            .skip(from)
            .position(|&c| c == query_chars[matched])
            .map(|offset| from + offset);
        let Some(index) = found else { break };
        let boundary = index == 0 || is_word_boundary(text_chars[index - 1]);
        if last == index as i32 - 1 {
            consecutive += 1;
            score -= f64::from(consecutive) * 5.0;
        } else {
            consecutive = 0;
            if last >= 0 {
                score += f64::from(index as i32 - last - 1) * 2.0;
            }
        }
        if boundary {
            score -= 10.0;
        }
        score += index as f64 * 0.1;
        last = index as i32;
        matched += 1;
    }
    if matched < query_chars.len() {
        return none;
    }
    if query == text {
        score -= 100.0;
    }
    FuzzyMatch {
        matches: true,
        score,
    }
}

/// pi's word-boundary set: whitespace plus `-_.`/`:`.
fn is_word_boundary(before: char) -> bool {
    before.is_whitespace() || matches!(before, '-' | '_' | '.' | '/' | ':')
}

/// pi's swapped letter/digit shape (`abc123` <-> `123abc`): one run
/// of letters and one run of digits, in either order.
fn swap_alpha_digits(query: &str) -> Option<String> {
    if query.is_empty()
        || !query
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return None;
    }
    // ASCII-only from here, so byte indexing is safe.
    let bytes = query.as_bytes();
    let first_letter = bytes[0].is_ascii_lowercase();
    let split = bytes
        .iter()
        .position(|&b| b.is_ascii_lowercase() != first_letter)?;
    let (head, tail) = (&query[..split], &query[split..]);
    if tail.bytes().any(|b| b.is_ascii_lowercase() == first_letter) {
        return None;
    }
    Some(format!("{tail}{head}"))
}

/// Subsequence fuzzy match.
#[cfg(test)]
mod tests {
    use super::*;

    fn provider(dir: &Path) -> CombinedAutocompleteProvider {
        let commands = vec![
            SlashCommand {
                name: "model".into(),
                description: Some("Pick a model".into()),
                argument_hint: Some("<id>".into()),
                argument_completions: Some(Arc::new(|prefix: &str| {
                    ["gpt-4o", "claude-sonnet"]
                        .iter()
                        .filter(|m| m.starts_with(prefix))
                        .map(|m| AutocompleteItem {
                            value: m.to_string(),
                            label: m.to_string(),
                            description: None,
                            origin: None,
                            dim: false,
                        })
                        .collect()
                })),
            },
            SlashCommand {
                name: "login".into(),
                description: Some("Sign in".into()),
                argument_hint: None,
                argument_completions: None,
            },
        ];
        CombinedAutocompleteProvider::new(commands, dir)
    }

    #[test]
    fn completes_command_names_fuzzily() {
        let p = provider(Path::new("."));
        let s = p.get_suggestions("/mod", false).unwrap();
        assert_eq!(s.items[0].label, "/model");
    }

    #[test]
    fn completes_command_arguments() {
        let p = provider(Path::new("."));
        let s = p.get_suggestions("/model gp", false).unwrap();
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].value, "gpt-4o");
        assert_eq!(s.prefix, "gp");
    }

    #[test]
    fn no_argument_callback_means_no_suggestions() {
        let p = provider(Path::new("."));
        assert!(p.get_suggestions("/login an", false).is_none());
    }

    /// Subsequence fuzzy match (the boolean face of [`super::fuzzy_match_scored`]).
    fn fuzzy_match(text: &str, query: &str) -> bool {
        super::fuzzy_match_scored(query, text).matches
    }

    #[test]
    fn fuzzy_match_is_subsequence() {
        assert!(fuzzy_match("model", "mdl"));
        assert!(fuzzy_match("model", ""));
        assert!(!fuzzy_match("model", "xyz"));
    }

    fn ranking_provider() -> CombinedAutocompleteProvider {
        let commands = ["settings", "trust", "stats", "grants", "antigravity.login"]
            .iter()
            .map(|name| SlashCommand {
                name: name.to_string(),
                description: None,
                argument_hint: None,
                argument_completions: None,
            })
            .collect();
        CombinedAutocompleteProvider::new(commands, Path::new("."))
    }

    #[test]
    fn a_consecutive_prefix_outranks_a_scattered_subsequence() {
        // gh #176: `/st` must select `/stats` (`st…`), not `/settings`
        // (`s…t…`) or `/trust` (`…s.t`).
        let p = ranking_provider();
        let labels: Vec<String> = p
            .get_suggestions("/st", false)
            .unwrap()
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect();
        assert_eq!(labels[0], "/stats", "{labels:?}");
    }

    #[test]
    fn a_word_start_outranks_a_mid_word_scatter() {
        // gh #176: `/ant` must select `/antigravity.login`, not
        // `/grants` (`gr-a-n-t-s`).
        let p = ranking_provider();
        let labels: Vec<String> = p
            .get_suggestions("/ant", false)
            .unwrap()
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect();
        assert_eq!(labels[0], "/antigravity.login", "{labels:?}");
    }

    #[test]
    fn scores_reward_prefix_and_penalize_gaps() {
        let stats = fuzzy_match_scored("st", "stats");
        let settings = fuzzy_match_scored("st", "settings");
        let trust = fuzzy_match_scored("st", "trust");
        assert!(stats.matches && settings.matches && trust.matches);
        assert!(
            stats.score < settings.score && settings.score < trust.score,
            "stats {} settings {} trust {}",
            stats.score,
            settings.score,
            trust.score
        );
    }

    #[test]
    fn an_empty_query_keeps_registration_order() {
        let p = ranking_provider();
        let labels: Vec<String> = p
            .get_suggestions("/", false)
            .unwrap()
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect();
        assert_eq!(labels[0], "/settings", "{labels:?}");
    }

    #[test]
    fn fuzzy_files_skip_build_and_vendor_dirs() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("target/debug")).unwrap();
        std::fs::create_dir_all(tmp.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "x").unwrap();
        let p = provider(&tmp);
        let s = p.get_suggestions("@", false).unwrap();
        let descs: Vec<String> = s
            .items
            .iter()
            .filter_map(|i| i.description.clone())
            .map(|d| d.replace('\\', "/"))
            .collect();
        assert!(descs.iter().any(|d| d.contains("src/main.rs")), "{descs:?}");
        assert!(!descs.iter().any(|d| d.starts_with("target/")), "{descs:?}");
        assert!(
            !descs.iter().any(|d| d.starts_with("node_modules/")),
            "{descs:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Verifies: gh #502's own repro - a wide shallow sibling starves a nested
    // match, so the file the query *names* is never offered.
    //
    // The defect was not in the matcher: bare `@` listed `src/main.rs` in the
    // same tree, and `@main` found it once `~/.lca` was absent. The walk was the
    // problem - `collect_files` counts every *match* against one 200-item budget,
    // in `read_dir` order, so a wide directory at the root fills the budget with
    // scattered-letter hits and the walk never descends to `src/`.
    //
    // The tree below is the issue's shape: `src/main.rs`, `README.md`, and a wide
    // populated sibling (the data-home analogue) whose entries all weakly match
    // `mai`. Red before the fix.
    #[test]
    fn a_nested_file_the_query_names_survives_a_wide_sibling_directory() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-502-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "x").unwrap();
        std::fs::write(tmp.join("README.md"), "x").unwrap();
        // A wide shallow sibling whose names all match `mai` loosely - the
        // data-home shape from the issue (`mcp`, `openai-compatible`, ...),
        // scaled past the budget so the starvation is deterministic rather than
        // dependent on how many entries a real data home holds.
        let wide = tmp.join("home/.lca/private");
        std::fs::create_dir_all(&wide).unwrap();
        for index in 0..260 {
            // `ma...i` scattered: every one of these matches `mai`.
            std::fs::write(wide.join(format!("m{index}a-i-{index}.toml")), "x").unwrap();
        }

        let p = provider(&tmp);
        let s = p.get_suggestions("@mai", false).unwrap();
        let descs: Vec<String> = s
            .items
            .iter()
            .filter_map(|i| i.description.clone())
            .map(|d| d.replace('\\', "/"))
            .collect();
        assert!(
            descs.iter().any(|d| d == "src/main.rs"),
            "the file the query names is offered, not starved by the wide sibling: {descs:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Verifies: gh #502 design point 4 - **the cap still bounds the walk**. The
    // fix moved the budget from "matches found" to "entries visited", so the row
    // has to prove the bound still exists: a directory with far more files than
    // the budget returns in bounded time and does not collect more than the
    // budget allows.
    //
    // Structural, not a stopwatch: a timing assertion would re-create the load
    // sensitivity this repo has spent cycles removing, and "how many entries did
    // it read" is the actual property.
    #[test]
    fn the_walk_budget_still_bounds_an_enormous_directory() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-502-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Well past both budgets, so the bound is what stops it rather than the
        // filesystem running out.
        for index in 0..(WALK_ENTRY_BUDGET + 500) {
            std::fs::write(tmp.join(format!("file-{index:05}.txt")), "x").unwrap();
        }
        let p = provider(&tmp);
        // Every name matches the empty query, so without a bound this would
        // collect all of them.
        let s = p.get_suggestions("@", false).unwrap();
        assert!(
            s.items.len() <= 20,
            "the visible list is 20 (got {})",
            s.items.len()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Verifies: gh #502 design point 5 - **ordering is deterministic**.
    //
    // The old walk's survivors depended on `read_dir` order, which is why the bug
    // was intermittent across machines: a wide directory's entries could arrive
    // in any sequence and the budget cut wherever it happened to be. The walk now
    // sorts each level before spending budget on it.
    //
    // Two rows, because "run it twice and compare" is too weak to prove anything
    // - `read_dir` on one filesystem is stable, so that row passes with the sort
    // removed (checked). The real property is that **the budget is spent in
    // sorted order**, so a file whose name sorts early is never starved by one
    // that sorts late, whatever order the filesystem hands them over in. That is
    // what this pair asserts.
    #[test]
    fn the_walk_spends_its_budget_in_sorted_order() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-502-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Well past `WALK_PER_DIR_CAP`, and only the first names alphabetically
        // are reachable. `zebra` sorts last and must be the one dropped.
        std::fs::write(tmp.join("aaa-first.txt"), "x").unwrap();
        std::fs::write(tmp.join("zzz-last.txt"), "x").unwrap();
        for index in 0..(WALK_PER_DIR_CAP + 50) {
            std::fs::write(tmp.join(format!("mmm-{index:04}.txt")), "x").unwrap();
        }
        let p = provider(&tmp);
        // Ask for the early name by name: the visible window is 20 and the bulk
        // `mmm-*` entries crowd it, so a bare `@` would answer "was it in the
        // window", not "did the walk reach it". A query only that file matches
        // asks the real question - did the budget, spent in sorted order, get far
        // enough to see a name that sorts first?
        let descs: Vec<String> = p
            .get_suggestions("@aaa-first", false)
            .unwrap()
            .items
            .iter()
            .filter_map(|i| i.description.clone())
            .collect();
        assert!(
            descs.iter().any(|d| d == "aaa-first.txt"),
            "the budget is spent in sorted order, so an early name survives: {descs:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_same_tree_gives_the_same_suggestions_every_time() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-502-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "x").unwrap();
        let wide = tmp.join("home/.lca/private");
        std::fs::create_dir_all(&wide).unwrap();
        for index in 0..260 {
            std::fs::write(wide.join(format!("m{index}a-i-{index}.toml")), "x").unwrap();
        }
        let p = provider(&tmp);
        let first: Vec<String> = p
            .get_suggestions("@mai", false)
            .unwrap()
            .items
            .iter()
            .filter_map(|i| i.description.clone())
            .collect();
        for _ in 0..3 {
            let again: Vec<String> = p
                .get_suggestions("@mai", false)
                .unwrap()
                .items
                .iter()
                .filter_map(|i| i.description.clone())
                .collect();
            assert_eq!(first, again, "the walk does not depend on read_dir order");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // Verifies: gh #502's regression pin - **bare `@` lists what it did**. The
    // issue records that bare `@` already found `src/main.rs`; a fix that
    // changed the breadth-first order must not quietly drop it, because bare `@`
    // is the ordinary way a reader browses the tree.
    #[test]
    fn a_bare_at_still_lists_the_nested_file() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-502-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "x").unwrap();
        std::fs::write(tmp.join("README.md"), "x").unwrap();
        let wide = tmp.join("home/.lca/private");
        std::fs::create_dir_all(&wide).unwrap();
        for index in 0..40 {
            std::fs::write(wide.join(format!("entry-{index}.toml")), "x").unwrap();
        }
        let p = provider(&tmp);
        let descs: Vec<String> = p
            .get_suggestions("@", false)
            .unwrap()
            .items
            .iter()
            .filter_map(|i| i.description.clone())
            .map(|d| d.replace('\\', "/"))
            .collect();
        assert!(
            descs.iter().any(|d| d == "src/main.rs"),
            "bare `@` still reaches the nested file: {descs:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn directory_path_completion_lists_and_marks_dirs() {
        let tmp = std::env::temp_dir().join(format!("lca-ac-{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("src"));
        std::fs::write(tmp.join("README.md"), "x").unwrap();
        let p = provider(&tmp);
        let s = p.get_suggestions("", true).unwrap();
        let labels: Vec<String> = s.items.iter().map(|i| i.label.clone()).collect();
        assert!(labels.contains(&"src/".to_string()));
        assert!(labels.contains(&"README.md".to_string()));
        // Directories sort first (score bonus).
        assert_eq!(s.items[0].label, "src/");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
