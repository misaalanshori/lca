//! `lca ext install|update|remove|info|list`: resolve, consent, store
//! (FR-DIST-*, the SRDD's extension-tree paragraph, ADR-0010).
//!
//! Everything under this module reads and writes [`lca_registry`]'s tree; the
//! consent screen shows exactly the manifest's declared grants in the
//! capability catalog's words before anything is written, and a "no"
//! writes nothing at all.
//!
//! **Gh #543 made this a directory without changing its public path.** It was
//! `src/ext.rs`; the barrel (`lca_cli::ext::{run, install_tree, load_installed,
//! …}`) and the `#[path]`-free submodules are all reachable exactly as before,
//! because a `ext.rs`/`ext/mod.rs` file *is* the module `ext`. The parts:
//! `install.rs` (resolve → verify → consent → write), `manage.rs` (read/update/
//! remove/list/toggle an installed tree), `config.rs` (per-project resource
//! selection, gh #59).

mod config;
mod install;
mod manage;

// The per-project resource-selection seam (gh #59): `read_all_selections` is
// read crate-wide; `config` is dispatched through `install.rs`'s router rather
// than from here. The test seam is imported by the rows themselves.
pub(crate) use config::read_all_selections;

/// The `lca ext ...` subcommands (SRDD command-line section).
#[derive(clap::Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum ExtCmd {
    /// Install an extension from a registry, a URL, or a local file.
    Install {
        /// A registry reference (`host/repo/name:tag`), an https archive
        /// URL, or a path to a component.
        reference: String,
        /// The manifest next to a local component (defaults to
        /// `extension.toml` beside it).
        #[arg(long)]
        manifest: Option<std::path::PathBuf>,
        /// Answer yes to the capability prompt (for scripts).
        #[arg(long)]
        yes: bool,
    },
    /// Re-fetch an installed extension and apply the new version.
    Update {
        /// The extension to update.
        name: Option<String>,
        /// Update every installed extension.
        #[arg(long)]
        all: bool,
        /// Answer yes to the capability prompt (for scripts).
        #[arg(long)]
        yes: bool,
    },
    /// Uninstall an extension.
    Remove {
        /// The extension to remove.
        name: String,
    },
    /// Show an extension's manifest, source, and digest.
    Info {
        /// The extension to inspect.
        name: String,
    },
    /// Enable an extension for the current project (FR-PROV-9).
    Enable {
        /// The extension to enable (`builtin:` prefix accepted, gh #139).
        name: String,
    },
    /// Disable an extension for the current project (FR-PROV-9).
    Disable {
        /// The extension to disable (`builtin:` prefix accepted, gh #139).
        name: String,
    },
    /// Manage an extension's `state` bag (ADR-0030).
    State {
        /// The state operation.
        #[command(subcommand)]
        cmd: StateCmd,
    },
    /// Show or set an installed extension's per-project resource
    /// selection (gh #59): `+path` includes, `-path` excludes,
    /// `!glob` excludes matches. No filters prints the current set.
    Config {
        /// The installed extension to configure.
        name: String,
        /// The new filter list, replacing the current one. Leading
        /// dashes are values here, not flags (`-skills/x` filters).
        #[arg(allow_hyphen_values = true)]
        filters: Vec<String>,
    },
    /// List installed extensions.
    List,
}

/// `lca ext state ...` subcommands.
#[derive(clap::Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum StateCmd {
    /// Delete an extension's state bag.
    Clear {
        /// The extension whose state to clear.
        name: String,
    },
}

// The module's public surface, in one place: `mod.rs` is the barrel, and these
// are the names other modules already wrote as `crate::ext::<name>`. The split
// moved definitions; the paths are the same ones `ext.rs` offered.
pub(crate) use install::installed_environment;
pub use install::{confirm_with, install_tree, load_extra, load_installed, run};
pub(crate) use manage::{builtin_names, not_installed};
pub use manage::{set_extension_enabled, update_all};

#[cfg(test)]
mod tests {
    // Gh #543: the module became a directory; each name is reached through the
    // file that now holds it, and `mod.rs` is still the barrel.
    use super::config::{config_in, read_selection};
    use super::install::{installed_environment, parse_manifest_strict};
    use super::manage::{clear_state_in, set_enabled_in, state_bytes, update};

    const VALID: &str = r#"name = "word-count"
version = "1.0.0"
abi = "0.5"
worlds = ["tool"]
description = "Counts words."

[capabilities.fs]
workspace = "read"
"#;

    // Verifies: FR-DIST-5's consent path validates through the loader's
    // parser - a valid manifest yields the identity the screen shows.
    #[test]
    fn a_valid_manifest_yields_identity() {
        let (name, version, abi, description) =
            parse_manifest_strict(VALID).expect("valid manifest parses");
        assert_eq!(name, "word-count");
        assert_eq!(version, "1.0.0");
        assert_eq!(abi, "0.5");
        assert_eq!(description, "Counts words.");
    }

    // Verifies: the install screen cannot be shown for a name that would
    // escape the install tree (FR-DIST-5; `InstallTree::install` refuses too).
    #[test]
    fn a_traversal_name_is_refused() {
        let err = parse_manifest_strict(&VALID.replace("word-count", "../../escape"))
            .expect_err("traversal refused");
        assert!(
            err.to_string().contains("name"),
            "the refusal names the identifier: {err}"
        );
    }

    // Verifies: manifest rules the loader enforces are enforced here too,
    // before the consent screen: oauth needs net, and the credential
    // namespace is the extension name (FR-PERM-6).
    #[test]
    fn invalid_capability_declarations_are_refused() {
        let oauth_without_net = r#"name = "provider-x"
version = "1.0.0"
abi = "0.5"
worlds = ["provider"]

[capabilities.oauth]
redirect_path = "/callback"
"#;
        assert!(parse_manifest_strict(oauth_without_net).is_err());

        let wrong_namespace = VALID.replace(
            "[capabilities.fs]\nworkspace = \"read\"",
            "[capabilities.credentials]\nnamespace = \"other\"",
        );
        assert!(parse_manifest_strict(&wrong_namespace).is_err());
    }

    // Verifies: FR-EXT-8's window is checked at install, not only at load -
    // an artifact this host can never run is refused up front.
    #[test]
    fn an_out_of_window_abi_is_refused() {
        let err = parse_manifest_strict(&VALID.replace("abi = \"0.5\"", "abi = \"9.9\""))
            .expect_err("out-of-window ABI refused");
        assert!(
            err.to_string()
                .contains("outside this host's supported window"),
            "{err}"
        );
        assert!(
            err.to_string().contains("0.5") && err.to_string().contains("0.6"),
            "the refusal names the accepted lines: {err}"
        );
    }

    // Verifies: gh #139 (pi's `builtin:<name>` disable syntax). The
    // prefix is a spelling, not a second record: disabling
    // `builtin:compaction-default` writes the same flag the loader
    // reads for `compaction-default`.
    #[test]
    fn builtin_prefixed_disable_writes_the_plain_name_flag() {
        let root = lca_testkit::scratch_path("lca-ext-builtin-prefix");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        assert_eq!(
            set_enabled_in(&root, &project, "builtin:compaction-default", false),
            crate::exit::OK
        );
        let store = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(
            store.extension_enabled(&project, "compaction-default"),
            Some(false)
        );
        assert_eq!(
            store.extension_enabled(&project, "builtin:compaction-default"),
            None,
            "no second record under the prefixed spelling"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PROV-9 (per-project enable/disable through the CLI). The
    // store's own method was tested; the command that reaches it was not.
    #[test]
    fn disabling_then_enabling_writes_the_per_project_flag() {
        let root = lca_testkit::scratch_path("lca-ext-enable");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        assert_eq!(
            set_enabled_in(&root, &project, "skills", false),
            crate::exit::OK
        );
        let store = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(store.extension_enabled(&project, "skills"), Some(false));
        assert_eq!(
            set_enabled_in(&root, &project, "skills", true),
            crate::exit::OK
        );
        let store = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(store.extension_enabled(&project, "skills"), Some(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Cycle-7 driving defect: a data-only package's digest was over an
    // empty component, so v2 of a skill pack compared equal to v1 and
    // `ext update` answered "up to date" forever. The bag is the payload.
    //
    // Verifies: ADR-0030, FR-DIST-7.
    #[tokio::test]
    async fn a_data_only_update_applies_a_resource_change() {
        let root = lca_testkit::scratch_path("ext-data-only-update");
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join("pack");
        std::fs::create_dir_all(pkg.join("resources/skills/demo")).expect("mkdir");
        std::fs::write(
            pkg.join("extension.toml"),
            "name = \"demo-pack\"\nversion = \"1.0.0\"\nabi = \"0.5\"\nworlds = []\nresources = [\"skills\"]\n",
        )
        .expect("manifest");
        let skill = pkg.join("resources/skills/demo/SKILL.md");
        std::fs::write(&skill, "name: demo\nmatch: demo\n---\nv1\n").expect("skill v1");

        let tree = lca_registry::InstallTree::new(root.join("extensions"));
        let first = tree
            .install(lca_registry::resolve_local(&pkg, None).expect("resolve v1"))
            .expect("install v1");

        // v2 changes only the skill body.
        std::fs::write(&skill, "name: demo\nmatch: demo\n---\nv2\n").expect("skill v2");
        let second = lca_registry::resolve_local(&pkg, None).expect("resolve v2");
        assert_ne!(
            second.digest, first.digest,
            "the resources bag is part of a data-only package's identity"
        );

        let code = update(&tree, Some("demo-pack".to_string()), false, true).await;
        assert_eq!(code, crate::exit::OK);
        let installed = std::fs::read_to_string(
            root.join("extensions/demo-pack/resources/skills/demo/SKILL.md"),
        )
        .expect("read installed skill");
        assert!(installed.contains("v2"), "the update applied: {installed}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #29 (QA-007) - exactly one GrantStore instance manages
    // `grants.json` in a running process (ADR-0022): the environment
    // installed extensions run against carries the session's own handle, so
    // a grant written through the prompt is visible to every loaded
    // extension immediately, with no reload from disk. Red against the
    // extracted-but-not-yet-fixed builder, which opened the file itself.
    #[test]
    fn a_grant_added_via_the_prompt_is_visible_to_loaded_extensions_without_a_reload() {
        // No environment sandbox: this builder writes no file of its own,
        // and a test that moves `TMPDIR` under a sibling's scratch path
        // deletes that sibling when it drops.
        let root = lca_testkit::scratch_path("gh29-grant-handle");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        // The session's handle: what the prompt writes goes through this Arc.
        let grants = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        let env = installed_environment(
            &project,
            lca_permissions::SharedPrompt::default(),
            lca_permissions::SharedDialogs::default(),
            &grants,
        );

        // A grant attached mid-session, exactly as the prompt attaches it
        // (persisted through the shared handle).
        grants
            .lock()
            .unwrap()
            .approve_net_pattern(&project, "env-only.example")
            .expect("grant");
        assert!(
            std::sync::Arc::ptr_eq(&env.grant_store, &grants),
            "the extensions' store is the session's own handle, not a second one"
        );
        let seen = env.grant_store.lock().unwrap().net_patterns(&project);
        assert!(
            seen.iter().any(|pattern| pattern == "env-only.example"),
            "a grant added via the prompt is visible to loaded extensions without \
             reloading from disk: {seen:?}"
        );
    }

    // Verifies: ADR-0030 (state is cleared per namespace and only that one).
    #[test]
    fn clearing_state_removes_only_that_namespace() {
        let root = lca_testkit::scratch_path("ext-state-clear");
        std::fs::create_dir_all(root.join("state/alpha")).expect("mkdir");
        std::fs::write(root.join("state/alpha/counter"), b"1").expect("write");
        std::fs::create_dir_all(root.join("state/beta")).expect("mkdir");
        std::fs::write(root.join("state/beta/counter"), b"2").expect("write");

        assert_eq!(clear_state_in(&root, "alpha"), crate::exit::OK);
        assert!(!root.join("state/alpha").exists());
        assert!(
            root.join("state/beta/counter").exists(),
            "another extension's namespace is untouched"
        );
        // Clearing an absent bag is not an error.
        assert_eq!(clear_state_in(&root, "alpha"), crate::exit::OK);
        assert_eq!(state_bytes(&root, "beta"), 1);
    }

    // Verifies: gh #59 - resource selections persist per project and
    // round-trip; bad rules never reach the file.
    #[test]
    fn extension_resource_selection_round_trips() {
        let root = lca_testkit::scratch_path("ext-config-roundtrip");
        std::fs::create_dir_all(&root).expect("mkdir");
        assert!(
            read_selection(&root, "pack").is_empty(),
            "missing file means unfiltered"
        );
        assert_eq!(
            config_in(
                &root,
                "pack",
                &["+skills".to_string(), "-skills/drop".to_string()]
            ),
            crate::exit::OK
        );
        assert_eq!(
            read_selection(&root, "pack"),
            vec!["+skills".to_string(), "-skills/drop".to_string()]
        );
        // A second package's rules land beside the first.
        assert_eq!(
            config_in(&root, "other", &["!skills/tmp*/**".to_string()]),
            crate::exit::OK
        );
        assert_eq!(read_selection(&root, "pack").len(), 2, "untouched");
        assert_eq!(
            config_in(&root, "pack", &["bogus".to_string()]),
            crate::exit::USAGE,
            "a rule without a sigil refuses"
        );
        assert_eq!(
            read_selection(&root, "pack").len(),
            2,
            "the refused write changed nothing"
        );
    }
}
