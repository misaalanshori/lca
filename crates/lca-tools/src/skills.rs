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
    /// Extension names disabled for this project (FR-PROV-9): a disabled
    /// package contributes no skills, per the threat model's promise that
    /// disabling removes its skill pack.
    pub disabled: Vec<String>,
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
    /// The one-line `description:` header (the routing line the catalog
    /// advertises; empty when the file has none).
    pub description: String,
    /// Lowercase trigger words from `match:`.
    pub match_words: Vec<String>,
    /// False when `disable-model-invocation: true`: explicit `/skill:`
    /// invocation only, never the catalog, the model paths, or matched
    /// injection (pi parity).
    pub model_invocable: bool,
    /// The instruction body after the `---` line.
    pub body: String,
    /// Where it came from.
    pub source: SkillSource,
}

/// Parse one `SKILL.md` (the standard Claude format).
pub fn parse_skill(fallback_name: &str, source: SkillSource, text: &str) -> Skill {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut match_line = String::new();
    let mut model_invocable = true;
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
                "description" => description = value.trim().to_string(),
                "match" => match_line = value.to_string(),
                "disable-model-invocation" => {
                    model_invocable = value.trim() != "true";
                }
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
        description,
        match_words,
        model_invocable,
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
            if roots.disabled.iter().any(|disabled| disabled == &name) {
                continue;
            }
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

/// The skill catalog: one advertised line per model-invocable skill -
/// name, one-line description, source attribution. Bodies never ride it
/// (gh #43: advertise by default, load bodies on demand). The model's
/// way in names the loader: the `skill` tool, or `/skill:name`.
pub fn catalog(skills: &[Skill]) -> String {
    let mut out =
        String::from("Available skills (load one with the `skill` tool or `/skill:name`):\n");
    for skill in skills.iter().filter(|skill| skill.model_invocable) {
        out.push_str("- ");
        out.push_str(&skill.name);
        if !skill.description.is_empty() {
            out.push_str(" — ");
            out.push_str(&skill.description);
        }
        out.push_str(" (from ");
        out.push_str(&skill.source.label());
        out.push_str(")\n");
    }
    out
}

/// A skill's full body by name, through the same files the merge reads:
/// precedence first, so a project body shadows a user one with its name.
/// Explicit invocation always loads, restricted or not - the flag gates
/// the model's paths, never `/skill:`.
pub fn load_body(roots: &SkillsRoots, name: &str) -> Option<String> {
    collect(roots)
        .into_iter()
        .find(|skill| skill.name == name)
        .map(|skill| skill.body)
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
/// matches, append the attributed injection - but only when the caller
/// opted into matched injection (gh #43: the default is the catalog,
/// not full text). Restricted skills never inject, opted in or not.
pub fn transform(
    mut messages: Vec<ChatMessage>,
    skills: &[Skill],
    inject_matched: bool,
) -> Vec<ChatMessage> {
    if !inject_matched {
        return messages;
    }
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
        .filter(|skill| skill.model_invocable && skill_matches(skill, &latest_user))
        .collect();
    if let Some(injection) = injection(&matched) {
        messages.push(injection);
    }
    messages
}
