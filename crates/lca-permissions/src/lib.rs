//! The permission layer: the split grant store from ADR-0006 and the
//! authorize flow from `docs/flows.md`.
//!
//! The project file holds proposals with no force. Only the user grant
//! store, keyed by the canonical project path, is consulted when an action
//! runs (FR-PERM-11). Approving copies into that store behind a prompt that
//! shows what is being added, and a changed proposal set re-prompts with
//! the difference (FR-PERM-10).

#![forbid(unsafe_code)]

mod net;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A project's proposal set: pattern to human note.
pub type Proposals = BTreeMap<String, String>;

/// Something sensitive the model or an extension wants to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run a shell command.
    Shell {
        /// The exact command string.
        command: String,
        /// Working directory.
        cwd: PathBuf,
    },
    /// Write a file outside the workspace (FR-TOOL-3).
    WritePath {
        /// The exact target path.
        path: PathBuf,
    },
}

impl Action {
    /// Exactly what the interface shows while waiting (FR-UI-4).
    pub fn display(&self) -> String {
        match self {
            Action::Shell { command, cwd } => format!("{command} (in {})", cwd.display()),
            Action::WritePath { path } => format!("write {}", path.display()),
        }
    }

    /// The value approvals match against.
    fn match_value(&self) -> String {
        match self {
            Action::Shell { command, .. } => command.clone(),
            Action::WritePath { path } => path.display().to_string(),
        }
    }

    /// The pattern an always-approval records: the exact thing shown.
    pub fn suggested_pattern(&self) -> String {
        self.match_value()
    }
}

/// What the user chose at a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allow this call only.
    Once,
    /// Allow this call and persist its pattern.
    Always,
    /// Refuse the call.
    Denied,
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
    /// Show the proposal difference; true applies the new set.
    fn review_proposals(&mut self, diff: &ProposalDiff) -> bool;
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
}

/// The user grant store: one JSON file outside every project directory.
pub struct GrantStore {
    path: PathBuf,
    data: StoreData,
}

impl GrantStore {
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
            path: path.to_path_buf(),
            data,
        })
    }

    /// Whether an action is already granted for this project.
    pub fn is_allowed(&self, project_dir: &Path, action: &Action) -> bool {
        let Some(entry) = self.data.projects.get(&canonical_key(project_dir)) else {
            return false;
        };
        let value = action.match_value();
        entry
            .patterns
            .iter()
            .chain(entry.proposal_patterns.iter())
            .any(|pattern| wildcard_match(pattern, &value))
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
    pub fn is_trusted(&self, project_dir: &Path) -> bool {
        self.data
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

    /// Difference between the project's proposals and the approved set
    /// (FR-PERM-10).
    pub fn proposal_diff(&self, project_dir: &Path, proposals: &Proposals) -> ProposalDiff {
        let approved = self
            .data
            .projects
            .get(&canonical_key(project_dir))
            .map(|entry| &entry.approved_proposals)
            .cloned()
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

/// Run the authorize flow for one action: review a changed proposal set
/// first (FR-PERM-10), then consult the store, then prompt if needed
/// (FR-TOOL-3's approval path; a hook denial is upstream of this and never
/// reaches the prompt, FR-CORE-10).
pub fn authorize(
    store: &mut GrantStore,
    project_dir: &Path,
    action: &Action,
    proposals: Option<&Proposals>,
    prompt: &mut dyn PermissionPrompt,
) -> Result<Outcome, Error> {
    let mut reviewed = false;
    if let Some(proposals) = proposals {
        let diff = store.proposal_diff(project_dir, proposals);
        if !diff.is_empty() {
            reviewed = true;
            if prompt.review_proposals(&diff) {
                store.apply_proposals(project_dir, proposals)?;
            }
        }
    }

    if store.is_allowed(project_dir, action) {
        return Ok(Outcome {
            allowed: true,
            prompted: false,
            reviewed,
            stored_pattern: None,
        });
    }

    match prompt.ask(action) {
        Decision::Denied => Ok(Outcome {
            allowed: false,
            prompted: true,
            reviewed,
            stored_pattern: None,
        }),
        Decision::Once => Ok(Outcome {
            allowed: true,
            prompted: true,
            reviewed,
            stored_pattern: None,
        }),
        Decision::Always => {
            let pattern = action.suggested_pattern();
            store.approve_pattern(project_dir, pattern.clone())?;
            Ok(Outcome {
                allowed: true,
                prompted: true,
                reviewed,
                stored_pattern: Some(pattern),
            })
        }
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
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Error::Corrupt {
            path: PathBuf::from("<memory>"),
            source,
        }
    }
}

// ---------------------------------------------------------------------------
// Filesystem scopes (ADR-0005, capability catalog `fs`)
// ---------------------------------------------------------------------------

pub use net::{
    LocalPattern, NetPattern, PatternError, is_local_address, normalize_ip, parse_local_pattern,
    parse_net_pattern,
};

/// The fixed scope vocabulary a manifest may name (ADR-0005). The manifest
/// names a scope and a mode; it never carries a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsMode {
    /// Read-only access to the scope.
    Read,
    /// Read and write access.
    ReadWrite,
}

/// Why a scope resolution was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeViolationKind {
    /// The scope name is not in the vocabulary.
    UnknownScope,
    /// The scope is in the vocabulary but not in the granted set
    /// (deny by default, NFR-13).
    NotGranted,
    /// The grant's mode does not cover this operation.
    ModeRefused,
    /// The resolution left the scope (FR-PERM-12): traversal, absolute
    /// path, or a symlink out.
    Escape,
    /// The resolution would enter the agent's own state directory, where
    /// sessions, the extension tree, and the credential store live
    /// (capability catalog, platform notes).
    StateDirectory,
}

/// One granted scope: a vocabulary name plus a mode (the manifest's
/// `capabilities.fs` entry, as approved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeGrant {
    /// Which scope.
    pub scope: &'static str,
    /// What may be done there.
    pub mode: FsMode,
}

impl ScopeGrant {
    /// Parse a manifest entry against the vocabulary.
    pub fn parse(name: &str, mode: FsMode) -> Result<ScopeGrant, ScopeViolationKind> {
        match name {
            "workspace" | "private" | "home-config" | "temp" => Ok(ScopeGrant {
                scope: match name {
                    "workspace" => "workspace",
                    "private" => "private",
                    "home-config" => "home-config",
                    _ => "temp",
                },
                mode,
            }),
            _ => Err(ScopeViolationKind::UnknownScope),
        }
    }
}

/// A refused resolution, carrying enough detail to record the attempt
/// (capability catalog: path-escape attempts are recorded separately).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeViolation {
    /// Why it was refused.
    pub kind: ScopeViolationKind,
    /// The scope the guest named.
    pub scope: String,
    /// The path the guest supplied.
    pub path: String,
}

impl std::fmt::Display for ScopeViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.kind {
            ScopeViolationKind::UnknownScope => "unknown scope name",
            ScopeViolationKind::NotGranted => "scope not granted",
            ScopeViolationKind::ModeRefused => "grant mode refuses this operation",
            ScopeViolationKind::Escape => "resolution left the scope",
            ScopeViolationKind::StateDirectory => "resolution enters the agent state directory",
        };
        write!(
            f,
            "fs scope `{}` refused path `{}`: {why}",
            self.scope, self.path
        )
    }
}

impl std::error::Error for ScopeViolation {}

/// Where each vocabulary scope resolves (capability catalog's `fs` table).
#[derive(Debug, Clone)]
pub struct ScopeRoots {
    /// The project root the agent was opened in.
    pub workspace: PathBuf,
    /// A per-extension directory under the user data directory.
    pub private: PathBuf,
    /// The platform configuration directory.
    pub home_config: PathBuf,
    /// A per-session temporary directory.
    pub temp: PathBuf,
    /// The agent's own state directory: sessions, extension tree,
    /// credential store. Refused under every scope.
    pub state_dir: PathBuf,
}

impl ScopeRoots {
    fn root_for(&self, scope: &str) -> Option<&Path> {
        match scope {
            "workspace" => Some(&self.workspace),
            "private" => Some(&self.private),
            "home-config" => Some(&self.home_config),
            "temp" => Some(&self.temp),
            _ => None,
        }
    }

    /// Resolve a guest-supplied path inside a granted scope.
    ///
    /// `write` requests a write; a `read` grant refuses it. The resolution
    /// is canonical as far as the deepest existing ancestor, so parent
    /// traversal and symlinks (including one created after the grant) are
    /// caught before anything opens (FR-PERM-12), and the state directory
    /// is refused under every scope.
    pub fn resolve(
        &self,
        grants: &[ScopeGrant],
        scope: &str,
        rel: &str,
        write: bool,
    ) -> Result<PathBuf, ScopeViolation> {
        let violation = |kind: ScopeViolationKind| ScopeViolation {
            kind,
            scope: scope.to_string(),
            path: rel.to_string(),
        };

        let root = self
            .root_for(scope)
            .ok_or_else(|| violation(ScopeViolationKind::UnknownScope))?;
        let grant = grants
            .iter()
            .find(|grant| grant.scope == scope)
            .ok_or_else(|| violation(ScopeViolationKind::NotGranted))?;
        if write && grant.mode == FsMode::Read {
            return Err(violation(ScopeViolationKind::ModeRefused));
        }
        if rel.contains('\0') || Path::new(rel).is_absolute() {
            return Err(violation(ScopeViolationKind::Escape));
        }

        let joined = root.join(rel);
        let resolved = canonicalize_deepest(&joined);

        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let canonical_state =
            std::fs::canonicalize(&self.state_dir).unwrap_or_else(|_| self.state_dir.clone());
        if resolved.starts_with(&canonical_state) {
            return Err(violation(ScopeViolationKind::StateDirectory));
        }
        if !resolved.starts_with(&canonical_root) {
            return Err(violation(ScopeViolationKind::Escape));
        }
        Ok(resolved)
    }
}

/// Canonicalize as far as the path exists, expanding every symlink met on
/// the way, then re-append the not-yet-existing tail. A dangling link is
/// expanded through `read_link`, so a link planted after the grant still
/// reveals where it points (capability catalog: symlink-after-grant).
/// ponytail: symlink expansion is capped at40 hops; a loop degrades to
/// the pop path below rather than hanging.
fn canonicalize_deepest(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    let mut expansions = 0usize;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&current) {
            let mut out = canonical;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let is_symlink = std::fs::symlink_metadata(&current)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink
            && expansions < 40
            && let Ok(link) = std::fs::read_link(&current)
        {
            let mut expanded = if link.is_absolute() {
                link.clone()
            } else {
                current
                    .parent()
                    .map(|parent| parent.join(&link))
                    .unwrap_or_else(|| link.clone())
            };
            for part in tail.iter().rev() {
                expanded.push(part);
            }
            current = expanded;
            tail.clear();
            expansions += 1;
            continue;
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => {
                let mut out = current;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return out;
            }
        }
    }
}

impl From<ScopeViolation> for lca_protocol::CapabilityError {
    fn from(violation: ScopeViolation) -> lca_protocol::CapabilityError {
        match violation.kind {
            ScopeViolationKind::NotGranted | ScopeViolationKind::UnknownScope => {
                lca_protocol::CapabilityError::NotGranted(violation.to_string())
            }
            _ => lca_protocol::CapabilityError::Permission(violation.to_string()),
        }
    }
}
