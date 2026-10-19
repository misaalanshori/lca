//! The permission layer: the split grant store from ADR-0006 and the
//! authorize flow from `docs/flows.md`.
//!
//! The project file holds proposals with no force. Only the user grant
//! store, keyed by the canonical project path, is consulted when an action
//! runs (FR-PERM-11). Approving copies into that store behind a prompt that
//! shows what is being added, and a changed proposal set re-prompts with
//! the difference (FR-PERM-10).

#![forbid(unsafe_code)]

mod action;
mod authorize;
mod net;
mod rules;
mod scope;
pub mod sensitive;
pub mod shell;
mod trust;

pub use action::Action;
pub use authorize::authorize;
use rules::RuleMatch;
pub use rules::{RuleDecision, RuleScope, RuleSet, RuleView, wildcard_match};
// The `fs` vocabulary and its path resolution (ADR-0005), which is its own
// file so this one stays under the workspace's 1,200-line ceiling.
pub use scope::{
    FsMode, OAuthSettings, ScopeGrant, ScopeRoots, ScopeViolation, ScopeViolationKind,
};

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A project's proposal set: pattern to human note.
pub type Proposals = BTreeMap<String, String>;

/// How permission prompts are answered for this process (ADR-0042).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// Ask the user: the default.
    #[default]
    Ask,
    /// Answer every prompt "always, for this exact pattern" without asking,
    /// and record it like a human answer. Explicit deny rules still deny:
    /// yolo answers prompts, it does not overrule the user's own words.
    Yolo,
}

impl PermissionMode {
    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Ask => "ask",
            PermissionMode::Yolo => "yolo",
        }
    }

    /// Parse the config spelling.
    pub fn parse(text: &str) -> Option<PermissionMode> {
        match text {
            "ask" => Some(PermissionMode::Ask),
            "yolo" => Some(PermissionMode::Yolo),
            _ => None,
        }
    }
}

/// What the user chose at a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allow this call only.
    Once,
    /// Allow this call and persist its pattern.
    Always,
    /// Allow this call and trust the project folder for this session:
    /// later commands that provably stay inside the workspace run without a
    /// prompt (permission-UX plan §3.1). Never persisted.
    TrustFolder,
    /// Refuse the call.
    Denied,
}

/// Session-only grants. Never serialized: a `GrantStore` lives for the
/// process, so these vanish when it exits.
#[derive(Debug, Default)]
struct SessionGrants {
    /// Canonical project keys trusted for this session.
    trust: BTreeSet<String>,
    /// Canonical project keys refused for this session (gh #80): the
    /// trust prompt does not fire here. Refusal never overrides trust;
    /// it only suppresses the ask.
    distrust: BTreeSet<String>,
    /// Rules attached for this session.
    rules: RuleSet,
    /// Ad hoc `net` grants for this process only (`--allow-host`), keyed by
    /// project like the persisted set. Never written to `path`, so the flag
    /// dies with the run that set it (gh #29, QA-004).
    net_patterns: BTreeMap<String, BTreeSet<String>>,
}

/// The difference between the approved proposal set and the project's
/// current one (FR-PERM-10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProposalDiff {
    /// Proposals present now but never approved.
    pub added: Proposals,
    /// Proposals approved earlier but gone from the project now.
    pub removed: Proposals,
}

impl ProposalDiff {
    /// Whether anything changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// The approval prompts. The TUI implements this with modals; headless mode
/// implements it by denying (exit code 4, `docs/headless.md`).
pub trait PermissionPrompt: Send {
    /// Ask about one action.
    fn ask(&mut self, action: &Action) -> Decision;
    /// Ask about one action with tool hints attached (#78): the prompt
    /// may show them; the decision path never reads them (a hint is
    /// not a bypass, ADR-0045). The default keeps old behavior.
    fn ask_annotated(
        &mut self,
        action: &Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        let _ = annotations;
        self.ask(action)
    }
    /// Show the proposal difference; true applies the new set.
    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool;
}

/// A swappable prompt slot. Every capability engine the host builds (WASM and
/// native) gets one for its whole life; the interface installs the current
/// turn's real prompt into it, so an extension's own `process`/`pty` commands
/// reach the same modal the model's commands do (capability catalog: each
/// command still asks for approval). With nothing installed it denies, which
/// is the right answer headless and between turns.
#[derive(Default, Clone)]
pub struct SharedPrompt {
    inner: SharedPromptSlot,
}

/// The interior of [`SharedPrompt`]: an optional prompt, swappable at runtime.
type SharedPromptSlot = std::sync::Arc<
    std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>>>,
>;

impl SharedPrompt {
    /// Route subsequent asks to `prompt`.
    pub fn set(&self, prompt: std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt);
    }

    fn current(&self) -> Option<std::sync::Arc<std::sync::Mutex<dyn PermissionPrompt>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl PermissionPrompt for SharedPrompt {
    fn ask(&mut self, action: &Action) -> Decision {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .ask(action),
            None => Decision::Denied,
        }
    }

    /// gh #418 Phase C#C5 (gh #303): forward the tool hints.
    ///
    /// The trait default drops them and calls `ask`, which is right for a
    /// prompt with no annotated surface but wrong for this slot: it is the
    /// object every engine holds, so the default silently flattened every
    /// hinted action on its way to the real prompt. Forwarding keeps the
    /// hints a *display* concern - the decision still comes from the
    /// installed prompt's own rules (a hint is not a bypass, ADR-0045).
    fn ask_annotated(
        &mut self,
        action: &Action,
        annotations: Option<&lca_protocol::ToolAnnotations>,
    ) -> Decision {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .ask_annotated(action, annotations),
            None => Decision::Denied,
        }
    }

    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .review_proposals(diff),
            None => false,
        }
    }
}

/// One host-rendered question an extension asks (gh #124, gh #172): the
/// answers cross back as plain data, and the chrome is always the
/// host's own (spoof-proof by construction).
///
/// The swappable slot below mirrors [`SharedPrompt`]: with nothing
/// installed every question answers its denied value (`false`/`None`,
/// notify drops), which is the right answer headless and between
/// sessions.
pub trait DialogPrompt: Send {
    /// Yes/No buttons; `false` is no or dismissed.
    fn confirm(&mut self, title: &str, message: &str) -> bool;
    /// A picker over options; `None` is dismissed.
    fn select(&mut self, title: &str, options: &[String]) -> Option<String>;
    /// One line of text; `None` is dismissed.
    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String>;
    /// A transient notice; fire and forget.
    fn notify(&mut self, message: &str, level: &str);
}

/// A swappable dialog slot (the [`SharedPrompt`] pattern for questions).
#[derive(Default, Clone)]
pub struct SharedDialogs {
    /// The installed prompter, when any.
    inner: SharedDialogsSlot,
}

/// The interior of [`SharedDialogs`].
type SharedDialogsSlot =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>>>>;

impl SharedDialogs {
    /// Route subsequent questions to `prompt`.
    pub fn set(&self, prompt: std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(prompt);
    }

    /// The installed prompter, when any.
    fn current(&self) -> Option<std::sync::Arc<std::sync::Mutex<dyn DialogPrompt>>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl DialogPrompt for SharedDialogs {
    fn confirm(&mut self, title: &str, message: &str) -> bool {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .confirm(title, message),
            None => false,
        }
    }

    fn select(&mut self, title: &str, options: &[String]) -> Option<String> {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .select(title, options),
            None => None,
        }
    }

    fn input(&mut self, label: &str, placeholder: Option<&str>) -> Option<String> {
        match self.current() {
            Some(prompt) => prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .input(label, placeholder),
            None => None,
        }
    }

    fn notify(&mut self, message: &str, level: &str) {
        if let Some(prompt) = self.current() {
            prompt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .notify(message, level);
        }
    }
}

/// Result of one authorize call.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Whether the action may run.
    pub allowed: bool,
    /// Whether the action prompt was shown.
    pub prompted: bool,
    /// Whether a proposal review was shown.
    pub reviewed: bool,
    /// The pattern persisted, when the user chose always.
    pub stored_pattern: Option<String>,
    /// Whether a deny rule refused the action (no prompt was shown).
    pub denied_by_rule: bool,
    /// Whether an allow rule allowed the action (gh #259): no prompt
    /// was shown, but the caller records it — a rule decision belongs
    /// in the log even when nobody was asked.
    pub allowed_by_rule: bool,
    /// Whether yolo mode answered for the user (ADR-0042). The caller
    /// records these like a human "always" answer, so the audit trail is
    /// complete even though no prompt was shown.
    pub yolo: bool,
}

impl Outcome {
    /// Convenience: allowed.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Convenience: denied.
    pub fn denied(&self) -> bool {
        !self.allowed
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreData {
    #[serde(default = "store_version")]
    version: u32,
    /// Global rules, the user's defaults across every project.
    #[serde(default)]
    rules: RuleSet,
    #[serde(default)]
    projects: BTreeMap<String, ProjectEntry>,
}

fn store_version() -> u32 {
    1
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectEntry {
    #[serde(default)]
    trusted: bool,
    /// Patterns approved directly during sessions (decision: always).
    #[serde(default)]
    patterns: BTreeSet<String>,
    /// Patterns copied in from an approved proposal set; replaced wholesale
    /// when the set changes and the user approves the difference.
    #[serde(default)]
    proposal_patterns: BTreeSet<String>,
    /// The proposal set the user approved, for differencing.
    #[serde(default)]
    approved_proposals: Proposals,
    /// Hash of `approved_proposals` (FR-PERM-10).
    #[serde(default)]
    approved_hash: Option<String>,
    /// Per-project extension enablement (FR-PERM-19).
    #[serde(default)]
    extensions: BTreeMap<String, bool>,
    /// Ad hoc `net` grants attached when the host was named
    /// (FR-PERM-16, ADR-0022): host-pattern vocabulary only, never an
    /// fs path, never a bare wildcard.
    #[serde(default)]
    net_patterns: BTreeSet<String>,
    /// Project rules (allow/deny globs).
    #[serde(default)]
    rules: RuleSet,
}

/// The user grant store: one JSON file outside every project directory.
pub struct GrantStore {
    /// How prompts are answered this process (ADR-0042). Session state:
    /// the grant *file* never learns it, and losing it cannot loosen the
    /// deny-by-default posture.
    mode: PermissionMode,
    path: PathBuf,
    data: StoreData,
    /// Session-only grants (never written to `path`).
    session: SessionGrants,
    /// Process-wide distrust (`-na`); never persisted, like the mode.
    force_untrusted: bool,
}

impl GrantStore {
    /// The mode prompts are answered in for this process (ADR-0042).
    pub fn permission_mode(&self) -> PermissionMode {
        self.mode
    }

    /// Set the mode for this process. Config decides it at startup; nothing
    /// persists it, and the default stays deny-by-default (`Ask`).
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    /// An empty in-memory store: grants nothing (NFR-13, deny by default).
    /// Used when the store file cannot be read, so the agent starts
    /// fail-closed rather than panicking.
    pub fn empty() -> GrantStore {
        GrantStore {
            mode: PermissionMode::default(),
            path: PathBuf::new(),
            data: StoreData::default(),
            session: SessionGrants::default(),
            force_untrusted: false,
        }
    }

    /// Open (or create) a store file.
    pub fn open(path: &Path) -> Result<GrantStore, Error> {
        let data = match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str::<StoreData>(&text).map_err(|source| Error::Corrupt {
                    path: path.to_path_buf(),
                    source,
                })?
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(source) => {
                return Err(Error::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Ok(GrantStore {
            mode: PermissionMode::default(),
            path: path.to_path_buf(),
            data,
            session: SessionGrants::default(),
            force_untrusted: false,
        })
    }

    /// The rule decision for one action: deny first, then allow, across
    /// session, project, and global scopes (ADR-0039).
    fn rule_decision(&self, project_dir: &Path, action: &Action) -> RuleMatch {
        let project_rules = self
            .data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| &entry.rules);
        rules::decide(
            &action.match_value(),
            &self.session.rules,
            project_rules,
            &self.data.rules,
        )
    }

    /// Add a rule at one scope (ADR-0039). Global/project rules persist;
    /// session rules do not.
    pub fn add_rule(
        &mut self,
        project_dir: &Path,
        scope: RuleScope,
        decision: RuleDecision,
        pattern: impl Into<String>,
    ) -> Result<(), Error> {
        let pattern = pattern.into();
        if pattern.trim().is_empty() {
            return Err(Error::Pattern("empty rule".to_string()));
        }
        let set = match scope {
            RuleScope::Session => &mut self.session.rules,
            RuleScope::Project => {
                &mut self
                    .data
                    .projects
                    .entry(canonical_key(project_dir))
                    .or_default()
                    .rules
            }
            RuleScope::Global => &mut self.data.rules,
        };
        match decision {
            RuleDecision::Allow => {
                set.allow.insert(pattern);
            }
            RuleDecision::Deny => {
                set.deny.insert(pattern);
            }
        }
        if scope == RuleScope::Session {
            Ok(())
        } else {
            self.save()
        }
    }

    /// Remove a rule at one scope. Returns whether it existed.
    pub fn remove_rule(
        &mut self,
        project_dir: &Path,
        scope: RuleScope,
        decision: RuleDecision,
        pattern: &str,
    ) -> Result<bool, Error> {
        let set = match scope {
            RuleScope::Session => &mut self.session.rules,
            RuleScope::Project => match self.data.projects.get_mut(&canonical_key(project_dir)) {
                Some(entry) => &mut entry.rules,
                None => return Ok(false),
            },
            RuleScope::Global => &mut self.data.rules,
        };
        let removed = match decision {
            RuleDecision::Allow => set.allow.remove(pattern),
            RuleDecision::Deny => set.deny.remove(pattern),
        };
        if !removed {
            return Ok(false);
        }
        if scope == RuleScope::Session {
            Ok(true)
        } else {
            self.save()?;
            Ok(true)
        }
    }

    /// Every rule visible for this project, session first.
    pub fn rules(&self, project_dir: &Path) -> Vec<RuleView> {
        let mut out = Vec::new();
        let mut push = |scope: RuleScope, set: &RuleSet| {
            for pattern in &set.deny {
                out.push(RuleView {
                    scope,
                    decision: RuleDecision::Deny,
                    pattern: pattern.clone(),
                });
            }
            for pattern in &set.allow {
                out.push(RuleView {
                    scope,
                    decision: RuleDecision::Allow,
                    pattern: pattern.clone(),
                });
            }
        };
        push(RuleScope::Session, &self.session.rules);
        if let Some(entry) = self.data.projects.get(&canonical_key(project_dir)) {
            push(RuleScope::Project, &entry.rules);
        }
        push(RuleScope::Global, &self.data.rules);
        out
    }

    /// Drop every session rule (not trust).
    pub fn clear_session_rules(&mut self) {
        self.session.rules = RuleSet::default();
    }

    /// Trust the project folder for this session only (never persisted).
    pub fn trust_for_session(&mut self, project_dir: &Path) {
        self.session.trust.insert(canonical_key(project_dir));
    }

    /// Whether the folder is trusted for this session.
    pub fn is_trusted_for_session(&self, project_dir: &Path) -> bool {
        self.session.trust.contains(&canonical_key(project_dir))
    }

    /// Whether the folder is trusted at all (persisted or session).
    /// Forced distrust (`-na`) denies both reads until lifted.
    pub fn is_trusted_here(&self, project_dir: &Path) -> bool {
        !self.force_untrusted
            && (self.is_trusted(project_dir) || self.is_trusted_for_session(project_dir))
    }

    /// Whether an action is already granted for this project: a rule, the
    /// folder's trust plus the workspace-scoped analyzer, or a stored pattern.
    pub fn is_allowed(&self, project_dir: &Path, action: &Action) -> bool {
        match self.rule_decision(project_dir, action) {
            RuleMatch::Deny => return false,
            RuleMatch::Allow => return true,
            RuleMatch::None => {}
        }
        if self.is_trusted_here(project_dir) {
            let root =
                std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
            let inside = |path: &Path| {
                let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                path == root || path.starts_with(&root)
            };
            match action {
                Action::Shell { command, cwd } => {
                    if shell::workspace_scoped(command, cwd, project_dir) {
                        return true;
                    }
                }
                Action::WritePath { path } | Action::ReadPath { path } => {
                    // Gh #215: folder trust never covers credential
                    // filenames — a sensitive read falls through to the
                    // prompt (or yolo/log) below, even in-workspace.
                    if inside(path)
                        && !matches!(action, Action::ReadPath { .. }
                            if sensitive::is_sensitive(path))
                    {
                        return true;
                    }
                }
                Action::Net { .. } => {}
            }
        }
        let Some(entry) = self.data.projects.get(&canonical_key(project_dir)) else {
            return false;
        };
        match action {
            // The ad hoc `net` vocabulary is its own, host-shaped set
            // (ADR-0022); it never consults the shell/path wildcards.
            Action::Net { host } => entry
                .net_patterns
                .iter()
                .filter_map(|pattern| parse_net_pattern(pattern).ok())
                .any(|pattern| pattern.matches_host(host)),
            _ => {
                let value = action.match_value();
                entry
                    .patterns
                    .iter()
                    .chain(entry.proposal_patterns.iter())
                    .any(|pattern| wildcard_match(pattern, &value))
            }
        }
    }

    /// Whether a deny rule refuses this action (no prompt).
    pub fn rule_denied(&self, project_dir: &Path, action: &Action) -> bool {
        self.rule_decision(project_dir, action) == RuleMatch::Deny
    }

    /// Persist a directly approved pattern for this project (FR-PERM-8).
    pub fn approve_pattern(
        &mut self,
        project_dir: &Path,
        pattern: impl Into<String>,
    ) -> Result<(), Error> {
        let key = canonical_key(project_dir);
        self.data
            .projects
            .entry(key)
            .or_default()
            .patterns
            .insert(pattern.into());
        self.save()
    }

    /// Project trust state, stored here rather than in the project (FR-PERM-19).
    /// Forced distrust (`-na`) reads back denied until lifted.
    pub fn is_trusted(&self, project_dir: &Path) -> bool {
        !self.force_untrusted
            && self
                .data
                .projects
                .get(&canonical_key(project_dir))
                .is_some_and(|entry| entry.trusted)
    }

    /// Record trust state for this project (FR-PERM-19).
    pub fn set_trusted(&mut self, project_dir: &Path, trusted: bool) -> Result<(), Error> {
        self.data
            .projects
            .entry(canonical_key(project_dir))
            .or_default()
            .trusted = trusted;
        self.save()
    }

    /// Per-project extension enablement (FR-PERM-19).
    pub fn extension_enabled(&self, project_dir: &Path, name: &str) -> Option<bool> {
        self.data
            .projects
            .get(&canonical_key(project_dir))
            .and_then(|entry| entry.extensions.get(name).copied())
    }

    /// Record per-project extension enablement (FR-PERM-19).
    pub fn set_extension_enabled(
        &mut self,
        project_dir: &Path,
        name: &str,
        enabled: bool,
    ) -> Result<(), Error> {
        self.data
            .projects
            .entry(canonical_key(project_dir))
            .or_default()
            .extensions
            .insert(name.to_string(), enabled);
        self.save()
    }

    /// The extension names this project has explicitly disabled (FR-PROV-9).
    /// Used by the host-side skills merge so a disabled package's skill pack
    /// falls out with it, the same way its registered handle does.
    pub fn disabled_extensions(&self, project_dir: &Path) -> Vec<String> {
        self.data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| {
                entry
                    .extensions
                    .iter()
                    .filter(|(_, enabled)| !**enabled)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The ad hoc `net` patterns approved for this project, in the
    /// order the store lists them (FR-PERM-16's persistence; ADR-0022).
    /// A pattern that no longer parses is skipped rather than failing
    /// every later request over one corrupt line.
    /// The ad hoc `net` grants approved for this project: the persisted
    /// set (FR-PERM-16) plus any granted for this process only, so every
    /// reader - the live ad hoc check, the endpoint host check, the
    /// provider's startup snapshot - sees one truth (`--allow-host`).
    pub fn net_patterns(&self, project_dir: &Path) -> Vec<String> {
        let key = canonical_key(project_dir);
        let mut patterns: Vec<String> = self
            .data
            .projects
            .get(&key)
            .map(|entry| {
                entry
                    .net_patterns
                    .iter()
                    .filter(|pattern| parse_net_pattern(pattern).is_ok())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(session) = self.session.net_patterns.get(&key) {
            for pattern in session {
                if parse_net_pattern(pattern).is_ok() && !patterns.contains(pattern) {
                    patterns.push(pattern.clone());
                }
            }
        }
        patterns
    }

    /// Attach an ad hoc `net` grant for this process only: `--allow-host`
    /// is a one-run grant, so it joins the session set and never reaches
    /// `path` (gh #29, QA-004). The pattern is validated with the same
    /// vocabulary the persisted grant uses.
    pub fn attach_session_net_pattern(
        &mut self,
        project_dir: &Path,
        pattern: &str,
    ) -> Result<(), Error> {
        parse_net_pattern(pattern).map_err(|err| Error::Pattern(err.to_string()))?;
        self.session
            .net_patterns
            .entry(canonical_key(project_dir))
            .or_default()
            .insert(pattern.to_string());
        Ok(())
    }

    /// Whether `pattern` is granted for this process only (`--allow-host`),
    /// which is what the consent check records rather than re-persists.
    pub fn session_net_pattern(&self, project_dir: &Path, pattern: &str) -> bool {
        self.session
            .net_patterns
            .get(&canonical_key(project_dir))
            .is_some_and(|patterns| patterns.contains(pattern))
    }

    /// Record one ad hoc `net` grant for this project (FR-PERM-16).
    pub fn approve_net_pattern(&mut self, project_dir: &Path, pattern: &str) -> Result<(), Error> {
        parse_net_pattern(pattern).map_err(|err| Error::Pattern(err.to_string()))?;
        self.data
            .projects
            .entry(canonical_key(project_dir))
            .or_default()
            .net_patterns
            .insert(pattern.to_string());
        self.save()
    }

    // --- the grants view's read/write surface (S8) ------------------------

    /// The ad hoc patterns approved for this project (S8's revocable group).
    pub fn patterns(&self, project_dir: &Path) -> Vec<String> {
        self.data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| entry.patterns.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The patterns copied in from an approved proposal set (S8): shown as
    /// install consent, edited only by re-approving the whole set.
    pub fn proposal_patterns(&self, project_dir: &Path) -> Vec<String> {
        self.data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| entry.proposal_patterns.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The project's extension enablement as `(name, enabled)` pairs, the
    /// install-consent group's data (S8).
    pub fn extensions(&self, project_dir: &Path) -> Vec<(String, bool)> {
        self.data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| {
                entry
                    .extensions
                    .iter()
                    .map(|(name, enabled)| (name.clone(), *enabled))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remove one ad hoc pattern (the grants view's revoke path, S8).
    pub fn revoke_pattern(&mut self, project_dir: &Path, pattern: &str) -> Result<(), Error> {
        self.data
            .projects
            .entry(canonical_key(project_dir))
            .or_default()
            .patterns
            .remove(pattern);
        self.save()
    }

    /// Remove one ad hoc `net` pattern (the grants view's revoke path, S8).
    pub fn revoke_net_pattern(&mut self, project_dir: &Path, pattern: &str) -> Result<(), Error> {
        self.data
            .projects
            .entry(canonical_key(project_dir))
            .or_default()
            .net_patterns
            .remove(pattern);
        self.save()
    }

    /// Difference between the project's proposals and the approved set
    /// (FR-PERM-10).
    pub fn proposal_diff(&self, project_dir: &Path, proposals: &Proposals) -> ProposalDiff {
        let key = canonical_key(project_dir);
        let entry = self.data.projects.get(&key);
        // ADR-0006: the hash of the approved set is the change detector. A
        // match means nothing changed, so no prompt is needed; only a
        // mismatch computes the difference to show.
        let hash = proposal_hash(proposals);
        if let Some(entry) = entry
            && entry.approved_hash.as_deref() == Some(hash.as_str())
        {
            return ProposalDiff::default();
        }
        let approved = entry
            .map(|entry| entry.approved_proposals.clone())
            .unwrap_or_default();
        ProposalDiff {
            added: proposals
                .iter()
                .filter(|(pattern, note)| approved.get(pattern.as_str()) != Some(*note))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            removed: approved
                .iter()
                .filter(|(pattern, _)| !proposals.contains_key(pattern.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// Apply an approved proposal set: its patterns become grants and the
    /// snapshot updates so the next change prompts with a fresh difference.
    pub fn apply_proposals(
        &mut self,
        project_dir: &Path,
        proposals: &Proposals,
    ) -> Result<(), Error> {
        let key = canonical_key(project_dir);
        let entry = self.data.projects.entry(key).or_default();
        entry.approved_proposals = proposals.clone();
        entry.approved_hash = Some(proposal_hash(proposals));
        entry.proposal_patterns = proposals.keys().cloned().collect();
        self.save()
    }

    /// Persist an extension's project-specific enablement change.
    #[allow(clippy::expect_used)] // serializing a `serde_json::Value` into JSON is infallible.
    pub fn save(&mut self) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec_pretty(&self.data).expect("store serializes");
        let temp = self.path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&temp).map_err(|source| Error::Io {
                path: temp.clone(),
                source,
            })?;
            file.write_all(&bytes).map_err(|source| Error::Io {
                path: temp.clone(),
                source,
            })?;
            file.flush().map_err(|source| Error::Io {
                path: temp.clone(),
                source,
            })?;
        }
        std::fs::rename(&temp, &self.path).map_err(|source| Error::Io {
            path: self.path.clone(),
            source,
        })
    }
}

/// Canonical key for a project: the working copy's canonical path
/// (ADR-0006: two checkouts are two projects).
pub fn canonical_key(project_dir: &Path) -> String {
    std::fs::canonicalize(project_dir)
        .unwrap_or_else(|_| project_dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Hash of a proposal set: any single-byte change re-prompts (ADR-0006).
pub fn proposal_hash(proposals: &Proposals) -> String {
    let mut hasher = Sha256::new();
    for (pattern, note) in proposals {
        hasher.update(pattern.as_bytes());
        hasher.update([0]);
        hasher.update(note.as_bytes());
        hasher.update([0xff]);
    }
    format!("{:x}", hasher.finalize())
}

/// Errors this crate returns.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem failure with the path involved.
    #[error("grant store I/O error at {path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The store file exists but does not parse; refusing to overwrite it.
    #[error("grant store {path} is corrupt: {source}")]
    Corrupt {
        /// The path.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: serde_json::Error,
    },
    /// A pattern value failed validation.
    #[error("invalid pattern: {0}")]
    Pattern(String),
}

// ---------------------------------------------------------------------------
// Filesystem scopes (ADR-0005, capability catalog `fs`)
// ---------------------------------------------------------------------------

pub use net::{
    LocalPattern, NetPattern, PatternError, is_local_address, normalize_ip, parse_local_pattern,
    parse_net_pattern,
};

#[cfg(test)]
mod dialog_tests {
    use super::*;

    // Verifies: gh #124 - with nothing installed every question answers
    // its denied value, which is the headless contract (no modal, no
    // hang, notify silently dropped).
    #[test]
    fn an_empty_slot_denies_every_question() {
        let mut slot = SharedDialogs::default();
        assert!(!slot.confirm("t", "m"));
        assert_eq!(slot.select("t", &["a".to_string()]), None);
        assert_eq!(slot.input("l", None), None);
        slot.notify("hello", "info");
    }

    struct Scripted {
        answers: Vec<String>,
    }

    impl DialogPrompt for Scripted {
        fn confirm(&mut self, _title: &str, _message: &str) -> bool {
            true
        }
        fn select(&mut self, _title: &str, options: &[String]) -> Option<String> {
            options.first().cloned()
        }
        fn input(&mut self, _label: &str, placeholder: Option<&str>) -> Option<String> {
            Some(placeholder.unwrap_or("typed").to_string())
        }
        fn notify(&mut self, message: &str, _level: &str) {
            self.answers.push(message.to_string());
        }
    }

    // Verifies: gh #124 - an installed prompter answers, so the live
    // TUI side and the headless side share one seam.
    #[test]
    fn an_installed_prompter_answers() {
        let mut slot = SharedDialogs::default();
        slot.set(std::sync::Arc::new(std::sync::Mutex::new(Scripted {
            answers: Vec::new(),
        })));
        assert!(slot.confirm("t", "m"));
        assert_eq!(
            slot.select("t", &["a".to_string(), "b".to_string()]),
            Some("a".to_string())
        );
        assert_eq!(slot.input("l", Some("ph")), Some("ph".to_string()));
    }
}
