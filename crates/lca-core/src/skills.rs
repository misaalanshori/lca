//! Host-side skills (FR-CTX-2, ADR-0030): the three-source merge.
//!
//! Skills are prompt-assembly data, so the host reads them (ADR-0030's
//! reader table): the workspace's `.lca/skills/<name>/SKILL.md`, the
//! user's skills directory, and every installed extension package's
//! `resources/skills/<name>/SKILL.md`. Project beats user beats extension,
//! first name wins; every injected skill names its source so a model can
//! weigh a project instruction above a packaged one.
//!
//! The `SKILL.md` format is the standard Claude one: leading `key: value`
//! header lines (the first is the name, a `match:` line lists
//! comma-separated trigger words), then a `---` line, then the body.
//!
//! ponytail: the parser duplicates `extensions/skills`'s (the extension
//! remains a working context-transform example, no longer registered by
//! default). Move it to a shared crate if a third consumer appears.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lca_protocol::{ChatMessage, ContentBlock, MessageRole};

/// The three skill sources, highest precedence first.
#[derive(Debug, Clone, Default)]
pub struct SkillsRoots {
    /// The workspace root; `.lca/skills` inside it is the project source.
    pub project: PathBuf,
    /// The user's skills directory (`<config>/skills`).
    pub user: PathBuf,
    /// The extension install tree; each package may carry
    /// `resources/skills/<name>/SKILL.md`.
    pub extensions: PathBuf,
}

/// Where a skill came from, for attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// The workspace's `.lca/skills`.
    Project,
    /// The user's skills directory.
    User,
    /// A package's `resources/skills`, named.
    Extension(String),
}

impl SkillSource {
    /// The attribution phrase: "project", "user", or "extension `name`".
    pub fn label(&self) -> String {
        match self {
            SkillSource::Project => "project".to_string(),
            SkillSource::User => "user".to_string(),
            SkillSource::Extension(name) => format!("extension `{name}`"),
        }
    }
}

/// One parsed skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The `name:` header, else the directory name.
    pub name: String,
    /// Lowercase trigger words from `match:`.
    pub match_words: Vec<String>,
    /// The instruction body after the `---` line.
    pub body: String,
    /// Where it came from.
    pub source: SkillSource,
}

/// Parse one `SKILL.md` (the standard Claude format).
pub fn parse_skill(fallback_name: &str, source: SkillSource, text: &str) -> Skill {
    let mut name = fallback_name.to_string();
    let mut match_line = String::new();
    let mut body_start = 0usize;
    let mut consumed = 0usize;
    for line in text.split('\n') {
        let trimmed = line.trim();
        body_start = consumed;
        consumed += line.len() + 1;
        if trimmed.is_empty() || trimmed == "---" {
            body_start = consumed;
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            match key.trim() {
                "name" => name = value.trim().to_string(),
                "match" => match_line = value.to_string(),
                _ => {} // unknown header keys are reserved, not errors
            }
        } else {
            body_start = consumed - line.len() - 1;
            break;
        }
    }
    let body = text.get(body_start..).unwrap_or("").trim().to_string();
    let match_words = match_line
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    Skill {
        name,
        match_words,
        body,
        source,
    }
}

/// Every `SKILL.md` under one directory's immediate subdirectories.
fn skills_in(dir: &Path, source: impl Fn(&str) -> SkillSource) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let text = std::fs::read_to_string(dir.join(&name).join("SKILL.md")).ok()?;
            Some(parse_skill(&name, source(&name), &text))
        })
        .collect()
}

/// Collect the three sources with precedence (project > user > extension),
/// first name wins, sorted by name within a source for determinism.
pub fn collect(roots: &SkillsRoots) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut add = |found: Vec<Skill>, seen: &mut HashSet<String>| {
        for skill in found {
            if seen.insert(skill.name.clone()) {
                skills.push(skill);
            }
        }
    };

    if !roots.project.as_os_str().is_empty() {
        add(
            skills_in(&roots.project.join(".lca/skills"), |_| SkillSource::Project),
            &mut seen,
        );
    }
    if !roots.user.as_os_str().is_empty() {
        add(skills_in(&roots.user, |_| SkillSource::User), &mut seen);
    }
    if !roots.extensions.as_os_str().is_empty() {
        let mut names: Vec<String> = std::fs::read_dir(&roots.extensions)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        for name in names {
            let dir = roots
                .extensions
                .join(&name)
                .join("resources")
                .join("skills");
            let source = SkillSource::Extension(name.clone());
            add(skills_in(&dir, |_| source.clone()), &mut seen);
        }
    }
    skills
}

/// Whether a skill matches: any trigger word appears in the latest user
/// message (case-insensitive containment).
pub fn skill_matches(skill: &Skill, latest_user: &str) -> bool {
    let haystack = latest_user.to_lowercase();
    skill.match_words.iter().any(|word| haystack.contains(word))
}

/// The injection: every matched skill becomes one appended system message,
/// each naming its source. Appended, never edited in place, so the stable
/// cache region is untouched by construction (Phase 4 exit clause 4).
pub fn injection(matched: &[&Skill]) -> Option<ChatMessage> {
    if matched.is_empty() {
        return None;
    }
    let mut text = String::new();
    for skill in matched {
        text.push_str("[skill ");
        text.push_str(&skill.name);
        text.push_str(" from ");
        text.push_str(&skill.source.label());
        text.push_str("]\n");
        text.push_str(&skill.body);
        text.push_str("\n\n");
    }
    Some(ChatMessage::text(
        MessageRole::System,
        text.trim_end().to_string(),
    ))
}

/// The whole host-side transform: find the latest user message, select the
/// matches, append the attributed injection.
pub fn transform(mut messages: Vec<ChatMessage>, skills: &[Skill]) -> Vec<ChatMessage> {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let matched: Vec<&Skill> = skills
        .iter()
        .filter(|skill| skill_matches(skill, &latest_user))
        .collect();
    if let Some(injection) = injection(&matched) {
        messages.push(injection);
    }
    messages
}
