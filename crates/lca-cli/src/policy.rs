//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

use lca_config::Config;
use lca_permissions::{GrantStore, PermissionPrompt, ProposalDiff};
/// FR-PROV-6's report, shared by both front ends and by the mid-session
/// identity commands: no enabled provider answers the configured name,
/// so there is no model, and the install command is the way out.
///
/// The message names the escape route explicitly: disabling the only
/// provider leaves `lca` unable to start at all, so "install one" is not
/// the only answer - re-enabling is the common one.
pub(crate) fn no_model_message(provider: &str) -> String {
    format!(
        "No model is available: no enabled provider answers `{provider}`. \
         Check the name (`lca ext list`), re-enable one that is disabled \
         (`lca ext enable <name>`), or install a provider with \
         `lca ext install <reference>`."
    )
}

/// The explicitly configured `shell` default timeout (gh #40): `Some`
/// when `tool.timeout_seconds` came from a flag, env, or file, `None`
/// when it is just the built-in default. Callers add their own fallback
/// for the unset case - none where the user can cancel (interactive),
/// the historical backstop where nobody can (headless).
pub fn configured_tool_timeout(config: &Config) -> Option<std::time::Duration> {
    if config.source_of("tool.timeout_seconds") == lca_config::MergeSource::Default {
        None
    } else {
        Some(std::time::Duration::from_secs(
            config.tool_timeout_seconds(),
        ))
    }
}

/// Load merged configuration for `cwd`, honoring project-file trust
/// (FR-CFG-1, FR-PERM-9).
/// One line naming yolo mode, unmissable on purpose (ADR-0042).
pub const YOLO_BANNER: &str = "YOLO MODE: every permission prompt is auto-approved as \"always\" and recorded in the \
session log; explicit deny rules still deny. /settings shows permissions.mode.";

/// Apply `permissions.mode` to the grant store (ADR-0042) and return the
/// banner when yolo is on. One call, both surfaces: the interface and the
/// capability engines share this store, so every permission check
/// (extension `process` calls included) answers the same way.
pub fn apply_permission_mode(config: &Config, grants: &mut GrantStore) -> Option<&'static str> {
    let mode = config
        .permissions_mode()
        .and_then(lca_permissions::PermissionMode::parse)
        .unwrap_or_default();
    grants.set_permission_mode(mode);
    matches!(mode, lca_permissions::PermissionMode::Yolo).then_some(YOLO_BANNER)
}

/// Headless mode cannot prompt: it denies and remembers that approval was
/// needed, which becomes exit code 4 (`docs/headless.md`).
#[derive(Default, Clone)]
pub struct HeadlessPrompt {
    needed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HeadlessPrompt {
    /// Whether any action asked for approval.
    pub fn needed_approval(&self) -> bool {
        self.needed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl PermissionPrompt for HeadlessPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        self.needed.store(true, std::sync::atomic::Ordering::SeqCst);
        lca_permissions::Decision::Denied
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #40 (the timeout default is mode-dependent): an
    // explicit `tool.timeout_seconds` surfaces as the default, while
    // the built-in default surfaces as none - the caller adds its own
    // fallback (backstop headless, none interactive).
    #[test]
    fn an_explicit_tool_timeout_is_the_default_and_the_builtin_is_none() {
        let plain = Config::load(&lca_config::LoadInput::default()).expect("load");
        assert_eq!(
            configured_tool_timeout(&plain),
            None,
            "the built-in 120s default is not an explicit setting"
        );
        let mut flags = std::collections::BTreeMap::new();
        flags.insert("tool.timeout_seconds".to_string(), "60".to_string());
        let set = Config::load(&lca_config::LoadInput {
            flags,
            ..Default::default()
        })
        .expect("load");
        assert_eq!(
            configured_tool_timeout(&set),
            Some(std::time::Duration::from_secs(60))
        );
    }

    // Verifies: FR-PERM-26 (ADR-0042) - `--yolo` reaches the config through the flag
    // layer (which beats a file that says ask), lands on the shared grant
    // store, and returns the banner the interface and headless print.
    #[test]
    fn the_yolo_flag_sets_the_mode_and_banners() {
        let root = lca_testkit::scratch_path("lca-yolo-flag");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");

        // A file that says ask, and a flag that says yolo: the flag wins.
        let file = root.join("config.toml");
        std::fs::write(&file, "permissions.mode = \"ask\"\n").expect("write");
        let mut flags = std::collections::BTreeMap::new();
        flags.insert("permissions.mode".to_string(), "yolo".to_string());
        let config = Config::load(&lca_config::LoadInput {
            flags,
            user_file: Some(file),
            ..Default::default()
        })
        .expect("load");
        assert_eq!(config.permissions_mode(), Some("yolo"));

        let mut grants = GrantStore::open(&root.join("grants.json")).expect("open");
        let banner = apply_permission_mode(&config, &mut grants).expect("banner");
        assert!(banner.contains("YOLO MODE"), "{banner}");
        assert_eq!(
            grants.permission_mode(),
            lca_permissions::PermissionMode::Yolo
        );

        // Ask mode is quiet and leaves the store in the default.
        let quiet = Config::defaults();
        assert!(apply_permission_mode(&quiet, &mut grants).is_none());
        assert_eq!(
            grants.permission_mode(),
            lca_permissions::PermissionMode::Ask
        );
    }

    // Verifies: FR-PROV-6 - the report names every way back, including
    // re-enabling. Disabling the only provider leaves `lca` unable to start
    // until one is enabled again, so "install a provider" alone is advice
    // that cannot be followed from inside the stuck state.
    #[test]
    fn the_no_model_report_names_the_way_back() {
        let report = no_model_message("openai-compatible");
        assert!(report.contains("No model is available"), "{report}");
        assert!(
            report.contains("lca ext enable"),
            "re-enabling is the common escape: {report}"
        );
        assert!(report.contains("lca ext install"), "{report}");
    }
}
