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
    /// One-run `--skill` paths (gh #70): files or directories, highest
    /// precedence (explicit beats discovered).
    pub extra: Vec<PathBuf>,
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
    /// A `--skill` path (gh #70).
    Cli,
}

impl SkillSource {
    /// The attribution phrase: "project", "user", or "extension `name`".
    pub fn label(&self) -> String {
        match self {
            SkillSource::Project => "project".to_string(),
            SkillSource::User => "user".to_string(),
            SkillSource::Extension(name) => format!("extension `{name}`"),
            SkillSource::Cli => "command line".to_string(),
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

/// One `--skill` path as skills (gh #70): a directory reads like
/// any skill dir; a `.md` file reads as one skill named by its stem
/// (`SKILL.md` takes its parent's name). Anything else contributes
/// nothing (bad paths refuse up front, in `run`).
fn skills_at(path: &Path) -> Vec<Skill> {
    if path.is_dir() {
        // The directory is usually the skill itself (`SKILL.md`
        // inside, named by the directory); otherwise it reads as a
        // container of skills, like every discovery dir.
        if let Ok(text) = std::fs::read_to_string(path.join("SKILL.md"))
            && let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
        {
            return vec![parse_skill(&name, SkillSource::Cli, &text)];
        }
        return skills_in(path, |_| SkillSource::Cli);
    }
    if !path.is_file() || !path.extension().is_some_and(|ext| ext == "md") {
        return Vec::new();
    }
    let name = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    } else {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    };
    let (Some(name), Ok(text)) = (name, std::fs::read_to_string(path)) else {
        return Vec::new();
    };
    if name.is_empty() {
        return Vec::new();
    }
    vec![parse_skill(&name, SkillSource::Cli, &text)]
}

/// Collect the sources with precedence (gh #70: command line first,
/// then project > user > extension), first name wins, sorted by name
/// within a source for determinism.
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
    // gh #70: explicit `--skill` paths win over everything
    // discovered. Bad paths never reach here (`run` refuses them up
    // front), so a file that vanished mid-run reads like any other
    // unreadable skill: skipped, like `skills_in` skips.
    for path in &roots.extra {
        add(skills_at(path), &mut seen);
    }

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

/// One fingerprinted spot: a path plus its mtime millis (`None` when
/// the path is absent) and, for directories, the child count (an
/// add/remove changes the count even on coarse-mtime filesystems).
/// Sorted by path, so equality is order-free.
type Fingerprint = Vec<(PathBuf, Option<u128>, Option<usize>)>;

/// The scan cache (gh #153): per-round `collect` walks the same trees
/// the turn just walked. The fingerprint (mtimes plus child counts,
/// no file reads) decides freshness; a stale cache re-scans off the
/// runtime threads via `spawn_blocking`.
#[derive(Debug, Default)]
pub struct SkillCache {
    fingerprint: Fingerprint,
    skills: Vec<Skill>,
}

impl SkillCache {
    /// An empty cache: the first collect always scans.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the cache matches the trees right now (blocking stat
    /// calls: tests and sync callers; the async turn uses
    /// [`SkillCache::collect_cached`], whose freshness probe runs on
    /// `tokio::fs`).
    pub fn is_fresh(&self, roots: &SkillsRoots) -> bool {
        self.fingerprint == fingerprint_sync(roots)
    }

    /// The cached skills when fresh, else a re-scan: the turn's
    /// per-round entry point. Freshness probes run on `tokio::fs`; the
    /// scan itself runs on the blocking pool, never a runtime thread.
    pub async fn collect_cached(
        cache: &std::sync::Mutex<SkillCache>,
        roots: &SkillsRoots,
    ) -> Vec<Skill> {
        let fingerprint = fingerprint_async(roots).await;
        {
            let guard = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.fingerprint == fingerprint {
                return guard.skills.clone();
            }
        }
        let owned = roots.clone();
        let skills = tokio::task::spawn_blocking(move || collect(&owned))
            .await
            .unwrap_or_default();
        if let Ok(mut guard) = cache.lock() {
            guard.fingerprint = fingerprint;
            guard.skills = skills.clone();
        }
        skills
    }
}
fn candidate_dirs(roots: &SkillsRoots) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for path in &roots.extra {
        dirs.push(path.clone());
    }
    if !roots.project.as_os_str().is_empty() {
        dirs.push(roots.project.join(".lca/skills"));
    }
    if !roots.user.as_os_str().is_empty() {
        dirs.push(roots.user.clone());
    }
    if !roots.extensions.as_os_str().is_empty() {
        // Package skill dirs, mirroring `collect`'s traversal minus
        // the file reads (a blocking `read_dir` here would defeat
        // the cache; the sync twin below walks the same list).
        if let Ok(entries) = std::fs::read_dir(&roots.extensions) {
            let mut names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                if roots.disabled.iter().any(|disabled| disabled == &name) {
                    continue;
                }
                dirs.push(
                    roots
                        .extensions
                        .join(&name)
                        .join("resources")
                        .join("skills"),
                );
            }
        }
    }
    dirs
}

/// Fingerprint one tree: its own stat plus every entry's name and
/// stat (file content changes move file mtimes; adds/removes move
/// the dir mtime and count).
fn fingerprint_sync(roots: &SkillsRoots) -> Fingerprint {
    let mut fingerprint = Vec::new();
    for dir in candidate_dirs(roots) {
        fingerprint.push((dir.clone(), mtime_sync(&dir), count_sync(&dir)));
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                let path = dir.join(&name);
                fingerprint.push((path.clone(), mtime_sync(&path), count_sync(&path)));
                // Skill bodies are one level down (`<name>/SKILL.md`).
                if path.is_dir()
                    && let Ok(inner) = std::fs::read_dir(&path)
                {
                    let mut files: Vec<String> = inner
                        .filter_map(|entry| entry.ok())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect();
                    files.sort();
                    for file in files {
                        let child = path.join(&file);
                        fingerprint.push((child.clone(), mtime_sync(&child), None));
                    }
                }
            }
        }
    }
    fingerprint
}

fn mtime_sync(path: &Path) -> Option<u128> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|age| age.as_millis())
}

fn count_sync(path: &Path) -> Option<usize> {
    if !path.is_dir() {
        return None;
    }
    Some(
        std::fs::read_dir(path)
            .map(|entries| entries.count())
            .unwrap_or(0),
    )
}

/// The async twin of [`fingerprint_sync`], on `tokio::fs`: same
/// paths, same shape, no blocking call. The two must not drift: both
/// walk `candidate_dirs` plus two levels, dir counts included.
async fn fingerprint_async(roots: &SkillsRoots) -> Fingerprint {
    let mut fingerprint = Vec::new();
    for dir in candidate_dirs_async(roots).await {
        fingerprint.push((
            dir.clone(),
            mtime_async(&dir).await,
            count_async(&dir).await,
        ));
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            let mut names = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            for name in names {
                let path = dir.join(&name);
                fingerprint.push((
                    path.clone(),
                    mtime_async(&path).await,
                    count_async(&path).await,
                ));
                if count_async(&path).await.is_some()
                    && let Ok(mut inner) = tokio::fs::read_dir(&path).await
                {
                    let mut files = Vec::new();
                    while let Ok(Some(entry)) = inner.next_entry().await {
                        files.push(entry.file_name().to_string_lossy().into_owned());
                    }
                    files.sort();
                    for file in files {
                        let child = path.join(&file);
                        fingerprint.push((child.clone(), mtime_async(&child).await, None));
                    }
                }
            }
        }
    }
    fingerprint
}

/// The async twin of [`candidate_dirs`]: same list, but the
/// extensions packages list through `tokio::fs`.
async fn candidate_dirs_async(roots: &SkillsRoots) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for path in &roots.extra {
        dirs.push(path.clone());
    }
    if !roots.project.as_os_str().is_empty() {
        dirs.push(roots.project.join(".lca/skills"));
    }
    if !roots.user.as_os_str().is_empty() {
        dirs.push(roots.user.clone());
    }
    if !roots.extensions.as_os_str().is_empty()
        && let Ok(mut entries) = tokio::fs::read_dir(&roots.extensions).await
    {
        let mut names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        for name in names {
            if roots.disabled.iter().any(|disabled| disabled == &name) {
                continue;
            }
            dirs.push(
                roots
                    .extensions
                    .join(&name)
                    .join("resources")
                    .join("skills"),
            );
        }
    }
    dirs
}

async fn mtime_async(path: &Path) -> Option<u128> {
    tokio::fs::metadata(path)
        .await
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|age| age.as_millis())
}

async fn count_async(path: &Path) -> Option<usize> {
    // `is_dir` would block: list instead, and read absence as not-a-dir.
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => {
            let mut count = 0;
            while let Ok(Some(_)) = entries.next_entry().await {
                count += 1;
            }
            Some(count)
        }
        Err(_) => None,
    }
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
