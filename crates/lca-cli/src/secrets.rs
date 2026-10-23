//! Split from `lib.rs` for the workspace file ceiling (gate 11,
//! #158). Behaviour unchanged.

use std::path::Path;
use std::sync::Arc;

use lca_permissions::GrantStore;

use super::provider_needs;
use crate::config_dir;
use crate::{CliFlags, data_dir};
/// Whether the configured provider looks ready to answer: a stored
/// credential for its namespace (`<data>/credentials/<name>.json`, whose
/// value is the extension identity per FR-PERM-6/7), or - for the bundled
/// `openai-compatible` provider - one of its documented environment keys.
/// When it is not ready the interface starts with no model and says how to
/// sign in, instead of offering a model whose first turn will fail with a
/// transport error.
pub(crate) fn provider_ready(name: &str, data: &Path) -> bool {
    if name == "openai-compatible"
        && ["OPENAI_API_KEY", "OPENCODE_API_KEY"]
            .iter()
            .any(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()))
    {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(data.join("credentials").join(format!("{name}.json")))
    else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.as_object().is_some_and(|object| {
        object
            .values()
            .any(|value| value.as_str().is_some_and(|text| !text.is_empty()))
    })
}

/// Why a secret write or an ad hoc grant write failed, typed for the
/// login flow rather than collapsed to a string at the seam.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SecretError {
    /// The credential store refused the write.
    #[error("credentials: {0}")]
    Credentials(String),
    /// The grant store's lock is poisoned (a bug elsewhere; fail closed).
    #[error("the grant store is locked")]
    GrantStoreLocked,
    /// The grant store refused the write.
    #[error("grant store: {0}")]
    GrantStore(String),
}

/// Store one secret in a provider's credential namespace (the `/login`
/// flow): the same atomic, owner-only writer extensions use. The namespace
/// is the provider identity, never guest input (FR-PERM-6).
pub(crate) fn store_provider_secret(
    data: &Path,
    cwd: &Path,
    provider: &str,
    key: &str,
    value: &str,
    grants: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> Result<(), SecretError> {
    secret_capabilities(data, cwd, provider, grants)
        .credentials_set(key, value)
        .map_err(|err| SecretError::Credentials(err.to_string()))
}

/// The capability engine a credential is written through: a credentials-
/// only grant, the platform scope roots, and `grants` - the session's own
/// handle, passed through untouched, so storing a secret cannot clobber a
/// grant and a grant cannot clobber a secret (gh #29 review: exactly one
/// `GrantStore` instance manages `grants.json` in a running process).
/// Extracted so the guard can see which handle it carries.
fn secret_capabilities(
    data: &Path,
    cwd: &Path,
    provider: &str,
    grants: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
) -> lca_tools::Capabilities {
    // No session here (a credential write precedes or outlives any
    // session): the system temp dir, explicitly, never a global.
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: std::env::temp_dir(),
        state_dir: data.to_path_buf(),
    };
    lca_tools::Capabilities::new(
        provider,
        lca_tools::CapabilityGrants {
            credentials: true,
            ..Default::default()
        },
        roots,
        Arc::new(std::sync::Mutex::new(
            lca_permissions::SharedPrompt::default(),
        )),
        grants.clone(),
        cwd.to_path_buf(),
        None,
    )
}

/// The preset id a provider's login last stored (E5), so the footer names
/// `opencode-go` rather than the extension. `None` for a provider set up
/// directly (env vars, a custom endpoint) that never chose a preset.
pub(crate) fn stored_provider_preset(data: &Path, provider: &str) -> Option<String> {
    let path = data.join("credentials").join(format!("{provider}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("preset")
        .and_then(|preset| preset.as_str())
        .filter(|preset| !preset.is_empty())
        .map(str::to_string)
}

/// The host in a URL authority, or `None` when it is one of the
/// provider's manifest-declared default endpoints (gh #157): the seven
/// lines of provider knowledge this used to hardcode now arrive as
/// [`provider_needs::ProviderNeeds`].
pub(crate) fn ad_hoc_host_from_authority(
    rest: &str,
    needs: Option<&provider_needs::ProviderNeeds>,
) -> Option<String> {
    provider_needs::ad_hoc_host_from_authority(rest, needs)
}

/// The endpoint host that still needs an ad hoc `net` grant for this project
/// (FR-PERM-16). `None` when none is needed: no non-default endpoint is
/// configured, or the host is already granted. A poisoned store reads as
/// granted so a broken lock never nags.
pub(crate) fn ungranted_host(
    store: &std::sync::Mutex<GrantStore>,
    cwd: &Path,
    host: Option<String>,
) -> Option<String> {
    ungranted_host_in(store, cwd, host, &[])
}

/// [`ungranted_host`], also told which `net-local` ranges the provider's own
/// manifest declares (gh #456).
///
/// A host inside a declared local range needs **no ad hoc grant**: the manifest
/// asked for it, the user approved that grant at install, and the capability
/// layer already lets the request out. Before #456 the CLI did not know about
/// the local grant at all, so it asked for a redundant second approval - and on
/// the native path, which dropped the grant entirely, the request was refused
/// outright regardless of the answer.
///
/// The declared ranges are consulted with the same matcher the capability layer
/// uses, so "covered here" and "covered there" cannot disagree.
pub(crate) fn ungranted_host_in(
    store: &std::sync::Mutex<GrantStore>,
    cwd: &Path,
    host: Option<String>,
    declared_local: &[lca_permissions::LocalPattern],
) -> Option<String> {
    let host = host?;
    // A manifest-declared local range covers the host on its own. Both
    // matchers are consulted because they answer different questions: a
    // literal address parses to a /32 or /128 CIDR, which `matches_name`
    // deliberately does not answer (it handles `localhost` and `.local`), and
    // a name like `localhost` is not an `IpAddr` for `matches_ip`. The
    // capability layer applies the same pair, so "covered here" and "covered
    // there" cannot disagree (gh #456).
    if declared_local.iter().any(|range| range.matches_name(&host))
        || host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_some_and(|ip| declared_local.iter().any(|range| range.matches_ip(ip)))
    {
        return None;
    }
    let covered = store
        .lock()
        .map(|store| {
            store
                .net_patterns(cwd)
                .iter()
                .any(|pattern| pattern == &host)
        })
        .unwrap_or(true);
    (!covered).then_some(host)
}

/// Persist an ad hoc `net` grant the user approved at login (FR-PERM-16).
/// It writes through the shared grant-store handle so the running session's
/// capability engines and the turn loop see it and cannot clobber it.
pub(crate) fn store_ad_hoc_grant(
    store: &std::sync::Arc<std::sync::Mutex<GrantStore>>,
    cwd: &Path,
    host: &str,
) -> Result<(), SecretError> {
    let mut store = store.lock().map_err(|_| SecretError::GrantStoreLocked)?;
    store
        .approve_net_pattern(cwd, host)
        .map_err(|err| SecretError::GrantStore(err.to_string()))
}

/// FR-PROV-9's disable knob (FR-PERM-19's storage): every handle the
/// grant store has disabled for this project leaves the registry.
pub(crate) fn apply_enablement(
    registry: &mut lca_core::ExtensionRegistry,
    disabled_here: impl Fn(&str) -> bool,
) {
    for name in registry.registered_names() {
        if disabled_here(&name) {
            registry.set_enabled(&name, false);
        }
    }
}

/// The capability environment for a bundled extension: the platform's scope
/// roots, the shared grant store, and the caller's prompt. Every engine and
/// the turn loop share one `Arc<Mutex<GrantStore>>` so a grant written by
/// one path is visible to (and never clobbered by) another.
// #92: bundled extensions and the (unconditional) MCP manager call
// this; without any caller it would be dead code, and dead code with
// a warning is a gate failure.
pub(crate) fn extension_capabilities(
    cwd: &Path,
    name: &str,
    grants: lca_tools::CapabilityGrants,
    prompt: lca_permissions::SharedPrompt,
    store: std::sync::Arc<std::sync::Mutex<GrantStore>>,
    resources: lca_tools::ResourceSource,
    temp: &Path,
) -> std::sync::Arc<lca_tools::Capabilities> {
    use std::sync::{Arc, Mutex};

    let data = data_dir();
    let roots = lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("private"),
        home_config: config_dir(),
        temp: temp.to_path_buf(),
        state_dir: data.clone(),
    };
    let mut engine = lca_tools::Capabilities::new(
        name,
        grants,
        roots,
        Arc::new(Mutex::new(prompt)),
        store,
        // The grant project is the workspace, not the data dir: shell
        // "allow always" patterns and mid-session ad hoc `net` grants
        // (FR-PERM-16, FR-PERM-18) are keyed by project, and the consent
        // flows write them for `cwd`. Keying the engine by the data dir
        // made those grants project-agnostic and hid every ad hoc grant
        // attached after startup.
        cwd.to_path_buf(),
        None,
    );
    // The extension's own `resources/` bag (ADR-0032): the compiled-in
    // table for a bundled extension, an installed package's directory for
    // a downloaded one. Without this `resource_read` finds nothing and the
    // picker has no presets to show.
    engine.set_resources(resources);
    Arc::new(engine)
}

/// The capability engine for one bundled provider (gh #157): the whole grant
/// set comes from the provider's own manifest, so every provider wires through
/// this one function. The caller supplies the extension's own pieces (its
/// manifest and its resource source); the host adds the session's ad hoc grants.
///
/// Gh #456: this used to build the grants as a partial literal and let
/// `..Default::default()` absorb the rest, which silently dropped `net_local`
/// (and `net_declared`). A provider declaring `[capabilities.net-local]` - which
/// #294 had just given `openai-compatible` for its `ollama`/`lmstudio` presets -
/// therefore worked as a `.wasm` and failed as a built-in, with
/// `permission denied: 127.0.0.1 matches no granted local range`.
///
/// It now calls the manifest's [one mapping](lca_ext_host::Manifest::capability_grants),
/// which is the same call the WASM host makes: one source of truth for the
/// native/WASM grant set, so the two delivery modes cannot diverge again
/// without a compile error (an exhaustive literal) or a failing parity row
/// (`tests/regressions/456-*`).
#[cfg(feature = "bundled-openai-compat")]
pub(crate) fn provider_capabilities(
    cwd: &Path,
    provider: &str,
    manifest: &lca_ext_host::Manifest,
    resources: lca_tools::ResourceSource,
    prompt: lca_permissions::SharedPrompt,
    store: std::sync::Arc<std::sync::Mutex<GrantStore>>,
    temp: &Path,
) -> std::sync::Arc<lca_tools::Capabilities> {
    let mut grants = manifest.capability_grants();
    grants.adhoc_net = store
        .lock()
        .map(|store| {
            store
                .net_patterns(cwd)
                .iter()
                .filter_map(|pattern| lca_permissions::parse_net_pattern(pattern).ok())
                .collect()
        })
        .unwrap_or_default();
    extension_capabilities(cwd, provider, grants, prompt, store, resources, temp)
}

/// The host-side skill sources (FR-CTX-2, ADR-0030): the workspace's
/// `.lca/skills`, the user skills dir, and the extension install tree.
pub(crate) fn skills_roots(cwd: &Path, flags: &CliFlags) -> lca_tools::skills::SkillsRoots {
    // A package disabled for this project contributes no skills (FR-PROV-9):
    // the merge reads the install tree directly, so it must apply the same
    // enablement filter the registry does.
    let disabled = lca_permissions::GrantStore::open(&data_dir().join("grants.json"))
        .map(|store| store.disabled_extensions(cwd))
        .unwrap_or_default();
    // gh #70: `--no-skills` empties discovery (explicit `--skill` paths
    // still load - pi's shape).
    let empty = std::path::PathBuf::new();
    // Gh #59: per-package resource selections narrow which skill
    // dirs load. Unparseable rules fall back to unfiltered (the
    // `ext config` seam validates on write; hand edits that break
    // parsing must not break discovery).
    let resource_selection = crate::ext::read_all_selections(cwd)
        .into_iter()
        .filter_map(|(name, rules)| {
            lca_tools::skills::parse_resource_selection(&rules)
                .ok()
                .map(|selection| (name, selection))
        })
        .collect();
    lca_tools::skills::SkillsRoots {
        project: if flags.no_skills {
            empty.clone()
        } else {
            cwd.to_path_buf()
        },
        user: if flags.no_skills {
            empty.clone()
        } else {
            data_dir().join("skills")
        },
        extensions: if flags.no_skills {
            empty
        } else {
            data_dir().join("extensions")
        },
        disabled,
        extra: flags.skill.clone(),
        resource_selection,
    }
}

/// The host-side prompt template sources (gh #58): the workspace's
/// `.lca/prompts` and the user prompts dir. Packages and explicit
/// paths stay out (user/project covers the workflow).
pub(crate) fn prompt_roots(cwd: &Path) -> lca_tools::prompts::PromptRoots {
    lca_tools::prompts::PromptRoots {
        user: data_dir().join("prompts"),
        project: cwd.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `/login` secret is stored through the same writer extensions use,
    // in the provider's own namespace, owner-only on Unix (B2).
    #[test]
    fn store_provider_secret_writes_the_namespace_credential() {
        let root = lca_testkit::scratch_path("lca-login");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let grants = std::sync::Arc::new(std::sync::Mutex::new(GrantStore::empty()));
        store_provider_secret(
            &root,
            &root,
            "openai-compatible",
            "api_key",
            "sk-x",
            &grants,
        )
        .expect("store the secret");
        let path = root.join("credentials").join("openai-compatible.json");
        let text = std::fs::read_to_string(&path).expect("read the credential file");
        assert!(text.contains("sk-x"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "owner-only credential file");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // The ad hoc host is the base URL's host, unless it is a
    // manifest-declared default (gh #157, FR-PERM-16: only a
    // non-default endpoint needs the grant). The old literal cases run
    // as manifest rows.
    #[test]
    fn the_ad_hoc_host_is_the_non_default_endpoint_host() {
        let needs = provider_needs::test_needs();
        assert_eq!(
            ad_hoc_host_from_authority("llm.example.com:8443/v1", Some(&needs)),
            Some("llm.example.com".to_string())
        );
        assert_eq!(
            ad_hoc_host_from_authority("user@internal.local/v1", Some(&needs)),
            Some("internal.local".to_string())
        );
        assert_eq!(
            ad_hoc_host_from_authority("api.acme.test/v1", Some(&needs)),
            None,
            "the manifest's declared default needs no grant"
        );
        assert_eq!(ad_hoc_host_from_authority("", Some(&needs)), None);
        assert_eq!(
            ad_hoc_host_from_authority("api.acme.test/v1", None),
            Some("api.acme.test".to_string()),
            "unknown needs never count as default: consent is the safe direction"
        );
    }

    // Verifies: gh #157 - the manifest drives the ad hoc grant: the
    // declared env override and stored base URL resolve through
    // `provider_ad_hoc_host` with no host literals.
    #[test]
    fn the_manifest_drives_the_ad_hoc_grant() {
        let needs = provider_needs::test_needs();
        assert_eq!(
            needs.env_base_url.as_deref(),
            Some("ACME_BASE_URL"),
            "the env override is declared, not hardcoded"
        );
        assert!(
            provider_needs::is_default_host(Some(&needs), "api.acme.test"),
            "the declared default is covered"
        );
        assert!(
            !provider_needs::is_default_host(Some(&needs), "llm.example.com"),
            "anything else needs consent"
        );
        assert!(
            !provider_needs::is_default_host(None, "api.acme.test"),
            "unknown needs consent too"
        );
    }

    // The approved grant is persisted for this project, so the next run's
    // capability environment picks it up (ADR-0022).
    #[test]
    fn the_ad_hoc_grant_is_persisted_for_the_project() {
        let root = lca_testkit::scratch_path("lca-adhoc");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        store_ad_hoc_grant(&store, &project, "llm.example.com").expect("store");
        let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(
            reread.net_patterns(&project),
            vec!["llm.example.com".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (the check behind the startup notice and
    // `/login`'s grant prompt). A non-default endpoint is ungranted until the
    // ad hoc grant is stored; no non-default endpoint never needs one. This
    // is the path an env-var key takes, which never runs the login prompt.
    #[test]
    fn a_non_default_endpoint_is_ungranted_until_the_grant_is_stored() {
        let root = lca_testkit::scratch_path("lca-ungranted");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        assert_eq!(
            ungranted_host(&store, &project, None),
            None,
            "the default endpoint needs no grant"
        );
        assert_eq!(
            ungranted_host(&store, &project, Some("opencode.ai".to_string())),
            Some("opencode.ai".to_string()),
            "a non-default endpoint needs a grant"
        );
        store_ad_hoc_grant(&store, &project, "opencode.ai").expect("grant");
        assert_eq!(
            ungranted_host(&store, &project, Some("opencode.ai".to_string())),
            None,
            "the stored grant satisfies the check"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "bundled-openai-compat")]
    #[test]
    fn the_engine_honors_a_grant_attached_for_the_workspace() {
        let root = lca_testkit::scratch_path("lca-engine-project");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        let caps = extension_capabilities(
            &project,
            "openai-compatible",
            openai_compatible::manifest_grants(),
            lca_permissions::SharedPrompt::default(),
            store.clone(),
            openai_compatible::resources(),
            &root.join("tmp"),
        );
        store_ad_hoc_grant(&store, &project, "127.0.0.1").expect("grant");
        // Reaching the socket layer (and failing to connect) proves the grant
        // was honored; a permission refusal means it was not.
        let err = caps
            .net_request("GET", "http://127.0.0.1:9/", &[], None)
            .expect_err("nothing listens on port 9");
        assert!(
            !matches!(
                err,
                lca_protocol::CapabilityError::Permission(_)
                    | lca_protocol::CapabilityError::NotGranted(_)
            ),
            "the ad hoc grant was honored, not refused: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (the ad hoc net grant and an engine-persisted
    // `always` pattern share one store, so neither save clobbers the other
    // - deferred plan E1). Before the single-owner wiring, the login seam
    // opened its own handle and its save dropped the engine's pattern.
    #[test]
    fn one_grant_store_holds_the_login_grant_and_an_engine_pattern() {
        let root = lca_testkit::scratch_path("lca-shared");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
        ));
        // The login flow writes the ad hoc net grant ...
        store_ad_hoc_grant(&store, &project, "llm.example.com").expect("net grant");
        // ... then the engine (or the turn loop) persists an `always`.
        store
            .lock()
            .expect("store")
            .approve_pattern(&project, "cargo test")
            .expect("pattern");
        let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(
            reread.net_patterns(&project),
            vec!["llm.example.com".to_string()]
        );
        assert!(reread.is_allowed(
            &project,
            &lca_permissions::Action::Shell {
                command: "cargo test".to_string(),
                cwd: project.clone(),
            }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: FR-PERM-16 (an approved ad hoc net grant is persisted and
    // project-scoped: it survives a restart, and another project never sees
    // it). Automates the manual tmux check B1 carried.
    #[test]
    fn an_ad_hoc_grant_survives_a_restart_and_stays_project_scoped() {
        let root = lca_testkit::scratch_path("lca-adhoc-persist");
        let _ = std::fs::remove_dir_all(&root);
        let project_a = root.join("a");
        let project_b = root.join("b");
        std::fs::create_dir_all(&project_a).expect("mkdir");
        std::fs::create_dir_all(&project_b).expect("mkdir");
        let path = root.join("grants.json");
        // The login flow's handle is dropped here: the store on disk is all
        // that survives a restart.
        {
            let store = std::sync::Arc::new(std::sync::Mutex::new(
                lca_permissions::GrantStore::open(&path).expect("open"),
            ));
            store_ad_hoc_grant(&store, &project_a, "llm.example.com").expect("grant");
        }

        let reloaded = lca_permissions::GrantStore::open(&path).expect("reopen");
        assert_eq!(
            reloaded.net_patterns(&project_a),
            vec!["llm.example.com".to_string()]
        );
        assert!(
            reloaded.net_patterns(&project_b).is_empty(),
            "the grant never leaks to another project"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: E5 - the login preset id is readable after a restart, so the
    // footer names `opencode-go` rather than the extension.
    #[test]
    fn stored_provider_preset_reads_the_persisted_id() {
        let root = lca_testkit::scratch_path("lca-provider-preset");
        let grants = std::sync::Arc::new(std::sync::Mutex::new(GrantStore::empty()));
        store_provider_secret(
            &root,
            &root,
            "openai-compatible",
            "preset",
            "opencode-go",
            &grants,
        )
        .expect("store");
        assert_eq!(
            stored_provider_preset(&root, "openai-compatible").as_deref(),
            Some("opencode-go")
        );
        assert_eq!(stored_provider_preset(&root, "other"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}

// Verifies: gh #29 review finding 1 - QA-007's acceptance criterion at
// the credentials-write path: exactly one `GrantStore` instance
// manages `grants.json` in a running process, so storing a secret runs
// on the session's own handle. A grant written around that write is
// never clobbered, and the credential survives a grant written after
// it. Red before the handle is threaded through: the engine opened the
// file itself, so neither the pointer nor the live view matches.
#[test]
fn the_credentials_write_shares_the_session_handle_and_clobbers_nothing() {
    let root = lca_testkit::scratch_path("gh29fix-secret-grant");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    // The session's handle: what the prompt writes goes through this Arc.
    let grants = std::sync::Arc::new(std::sync::Mutex::new(
        lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open"),
    ));
    let cap = secret_capabilities(&root, &project, "openai-compatible", &grants);

    assert!(
        std::sync::Arc::ptr_eq(&cap.grant_store(), &grants),
        "the credentials write runs on the session's own handle, not a second one"
    );
    // A grant attached after the engine was built is visible through it
    // without reloading the file.
    grants
        .lock()
        .unwrap()
        .approve_net_pattern(&project, "granted-first.example")
        .expect("grant");
    assert!(
        cap.grant_store()
            .lock()
            .unwrap()
            .net_patterns(&project)
            .iter()
            .any(|pattern| pattern == "granted-first.example"),
        "the one store: a grant written around a secret store is visible, not clobbered"
    );

    // Grant first, secret second: the secret lands and the grant stays.
    cap.credentials_set("api_key", "s3cret-one").expect("store");
    assert_eq!(
        cap.credentials_get("api_key").expect("read"),
        Some("s3cret-one".to_string()),
        "the credential was written"
    );
    let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("reopen");
    assert!(
        reread
            .net_patterns(&project)
            .iter()
            .any(|pattern| pattern == "granted-first.example"),
        "storing a secret left the grant on disk"
    );

    // Secret first, grant second: the credential stays too.
    cap.credentials_set("api_key", "s3cret-two").expect("store");
    grants
        .lock()
        .unwrap()
        .approve_net_pattern(&project, "granted-second.example")
        .expect("grant");
    assert_eq!(
        cap.credentials_get("api_key").expect("read"),
        Some("s3cret-two".to_string()),
        "writing a grant left the credential alone"
    );
    let reread = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("reopen");
    let patterns = reread.net_patterns(&project);
    for pattern in ["granted-first.example", "granted-second.example"] {
        assert!(
            patterns.iter().any(|seen| seen == pattern),
            "both grants survived: {patterns:?}"
        );
    }
}
