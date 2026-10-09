//! Process-scoped trust overrides (gh #80, FR-PERM-28): session
//! refusal and forced distrust behind `-na`/`--no-approve` and the
//! `trust.default_project` fallback. Extracted from `lib.rs` to keep
//! the crate root under the house line ceiling; `tests/rules.rs`
//! proves the behavior.

use std::path::Path;

use super::{GrantStore, canonical_key};

impl GrantStore {
    /// Refuse the project folder for this session (gh #80,
    /// FR-PERM-28): the trust prompt does not fire here, and nothing
    /// persists. An explicit trust answer later still wins: refusal
    /// suppresses the ask, never the trust itself.
    pub fn distrust_for_session(&mut self, project_dir: &Path) {
        self.session.distrust.insert(canonical_key(project_dir));
    }

    /// Whether the folder carries a session refusal.
    pub fn is_refused_for_session(&self, project_dir: &Path) -> bool {
        self.session.distrust.contains(&canonical_key(project_dir))
    }

    /// Treat every project as untrusted for this process (gh #80,
    /// `-na`/`--no-approve`): stored and session trust read back
    /// denied until lifted. Process state, never persisted.
    pub fn set_force_untrusted(&mut self, forced: bool) {
        self.force_untrusted = forced;
    }

    /// Whether forced distrust is active.
    pub fn is_force_untrusted(&self) -> bool {
        self.force_untrusted
    }
}
