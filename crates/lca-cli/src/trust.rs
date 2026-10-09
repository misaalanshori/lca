//! Startup trust overrides (gh #80, FR-PERM-28): the CLI flags plus
//! the configured fallback applied onto ADR-0039 before the config
//! merge. Split from `lib.rs` under the workspace's 1,200-line file
//! ceiling.

use std::collections::BTreeMap;
use std::path::Path;

use lca_config::{Config, LoadInput};
use lca_permissions::GrantStore;

use super::{CliFlags, config_file};

/// Apply the CLI trust overrides plus the configured fallback onto
/// ADR-0039 (gh #80, FR-PERM-28). Runs before the config merge so the
/// project-file gate sees the trust `-a` (and an `always` fallback)
/// grants for the session:
/// - `-a` trusts for the session; `-na` forces process-wide distrust.
/// - else the stored decision stands and `default` (the merged
///   `trust.default_project`) is the fallback: `always` trusts for
///   the session, `never` refuses the ask.
///
/// An explicit trust answer mid-run still overrides the startup
/// default (the `/trust` hook lifts forced distrust when told to).
pub fn apply_startup_trust(store: &mut GrantStore, cwd: &Path, flags: &CliFlags, default: &str) {
    if flags.approve {
        store.trust_for_session(cwd);
        return;
    }
    if flags.no_approve {
        store.set_force_untrusted(true);
        store.distrust_for_session(cwd);
        return;
    }
    if store.is_trusted(cwd) {
        return;
    }
    match default {
        "always" => store.trust_for_session(cwd),
        "never" => store.distrust_for_session(cwd),
        _ => {}
    }
}

/// The configured trust fallback ahead of the merge (gh #80): peeks at
/// the user file only, since a project file cannot set it (FR-CFG-7).
/// An unreadable user file falls back to `ask` here - the full merge
/// reports it loudly right after.
pub fn trust_default_fallback() -> String {
    lca_config::Config::load(&lca_config::LoadInput {
        user_file: config_file().exists().then(config_file),
        env: lca_config::collect_env(),
        ..Default::default()
    })
    .map(|peek| peek.trust_default_project().to_string())
    .unwrap_or_else(|_| "ask".to_string())
}

/// [`load_config_flags`] without command-line flags.
pub fn load_config(
    cwd: &Path,
    grants: &GrantStore,
    headless: bool,
    yolo: bool,
) -> anyhow::Result<Config> {
    load_config_flags(cwd, grants, headless, yolo, &CliFlags::default())
}

/// [`load_config`] with the command line's flag-layer values (`--models`
/// and friends, gh #8): the flags join the same precedence the `--yolo`
/// flag already rides, so `lca config` and `/settings` name the source.
pub fn load_config_flags(
    cwd: &Path,
    grants: &GrantStore,
    headless: bool,
    yolo: bool,
    cli: &CliFlags,
) -> anyhow::Result<Config> {
    let project_file = cwd.join(".lca").join("config.toml");
    let user_file = config_file().exists().then(config_file);
    let mut flags: BTreeMap<String, String> = cli.layer();
    if yolo {
        // The flag is the loudest layer (FR-CFG-1's precedence): it beats
        // a config file that says `ask`.
        flags.insert("permissions.mode".to_string(), "yolo".to_string());
    }
    let input = LoadInput {
        flags,
        env: lca_config::collect_env(),
        project_file: project_file.is_file().then_some(project_file),
        // gh #80: session trust counts (a `-a` run and an `always`
        // fallback trust before this load). No caller holds session
        // trust at load time today except through `apply_startup_trust`,
        // so mid-run `/reload` newly picking up a trusted file is the
        // only behavior delta - and it beats the restart ADR-0039 noted.
        trusted: grants.is_trusted_here(cwd),
        user_file,
        headless,
    };
    Ok(Config::load(&input)?)
}
