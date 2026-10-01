//! The `fs` capability's scope vocabulary and path resolution (ADR-0005,
//! the capability catalog's `fs` section): what a manifest may name, what
//! the host may grant beyond it, and how a resolved path is checked against
//! a scope - including the symlink and parent-traversal refusals.
//!
//! Split out of `lib.rs`, which held it from the ADR-0005 work onward; the
//! move is structural only, every item keeps its name and path through the
//! crate-root re-export.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

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

/// The `oauth` capability's manifest parameters (capability catalog:
/// `redirect_path`, and `timeout_seconds` defaulting to300).
#[derive(Debug, Clone)]
pub struct OAuthSettings {
    /// The redirect path the authorization server may use.
    pub redirect_path: String,
    /// Seconds to wait for the loopback callback.
    pub timeout_seconds: u64,
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
        // The state-directory exclusion protects the sessions, the extension
        // tree, and the credential store from every scope and every ad hoc
        // grant. The `private` scope is the one sanctioned exception: its own
        // root lives under the state directory (`<data>/private/<name>`) and
        // is confined by the `starts_with(root)` check below, so it cannot
        // reach those subtrees. Excluding all of the state directory made
        // `private` unusable (ADR-0005 / capability catalog's `fs` table).
        if scope != "private" && resolved.starts_with(&canonical_state) {
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
