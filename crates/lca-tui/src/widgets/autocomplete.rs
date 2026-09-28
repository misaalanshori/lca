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
        // `@` attachment prefix.
        if let Some(query) = text.strip_prefix('@') {
            return Some(self.fuzzy_files(query, &self.base_path));
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
            // A command with argument completions answers here; a command
            // without them (or one whose argument has no matches) falls
            // through to file completion for the last token, the way pi's
            // provider chain does.
            if let Some((name, argument)) = rest.split_once(' ')
                && let Some(command) = self.commands.iter().find(|c| c.name == name)
                && let Some(completions) = &command.argument_completions
            {
                let items = completions(argument);
                if !items.is_empty() {
                    return Some(Suggestions {
                        items,
                        prefix: argument.to_string(),
                    });
                }
            }
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
    fn command_items(&self, query: &str) -> Vec<AutocompleteItem> {
        self.commands
            .iter()
            .filter(|c| fuzzy_match(&c.name, query))
            .map(|c| AutocompleteItem {
                value: format!("/{} ", c.name),
                label: format!("/{}", c.name),
                description: match (&c.argument_hint, &c.description) {
                    (Some(h), Some(d)) => Some(format!("{h} — {d}")),
                    (Some(h), None) => Some(h.clone()),
                    (None, Some(d)) => Some(d.clone()),
                    (None, None) => None,
                },
            })
            .collect()
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

    fn fuzzy_files(&self, query: &str, base: &Path) -> Suggestions {
        let mut items = Vec::new();
        collect_files(base, base, query, 0, &mut items);
        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.len().cmp(&b.2.len()))
        });
        let items: Vec<AutocompleteItem> =
            items.into_iter().take(20).map(|(_, _, _, i)| i).collect();
        Suggestions {
            items,
            prefix: format!("@{query}"),
        }
    }
}

fn collect_files(
    root: &Path,
    dir: &Path,
    query: &str,
    depth: usize,
    out: &mut Vec<(i32, usize, String, AutocompleteItem)>,
) {
    if depth > 6 || out.len() > 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // D10: skip the heavy build/vendor directories in the fuzzy walk.
        if is_ignored_dir(&name) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if fuzzy_match(&rel, query) {
            let score = score_entry(&name, query, is_dir);
            out.push((
                score,
                depth,
                rel.clone(),
                AutocompleteItem {
                    value: format!("@{rel}"),
                    label: name.clone(),
                    description: Some(rel.clone()),
                },
            ));
        }
        if is_dir {
            collect_files(root, &path, query, depth + 1, out);
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

/// Subsequence fuzzy match.
fn fuzzy_match(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut chars = text
        .to_lowercase()
        .chars()
        .collect::<Vec<_>>()
        .into_iter()
        .peekable();
    for q in query.to_lowercase().chars() {
        loop {
            match chars.next() {
                Some(c) if c == q => break,
                Some(_) => {}
                None => return false,
            }
        }
    }
    true
}

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

    #[test]
    fn fuzzy_match_is_subsequence() {
        assert!(fuzzy_match("model", "mdl"));
        assert!(fuzzy_match("model", ""));
        assert!(!fuzzy_match("model", "xyz"));
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
