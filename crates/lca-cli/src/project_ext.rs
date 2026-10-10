//! Project-local extensions (gh #138): `.lca/extensions/<name>/`
//! holding `extension.toml` + `component.wasm`, loaded when the project
//! is trusted. Split from `ext.rs` at the file ceiling; behavior
//! unchanged.

/// One project-local extension: validated manifest text plus bytes.
#[derive(Debug)]
struct ProjectExtension {
    /// The manifest's own name (the registration identity).
    name: String,
    /// The manifest text ([`lca_ext_host::ExtHost::load`] parses it
    /// again; the scan's own parse validated it and read the name).
    manifest_text: String,
    /// The component bytes.
    wasm: Vec<u8>,
}

/// Scan `<project>/.lca/extensions/` for project-local extensions (gh
/// #138): each subdirectory with an `extension.toml` and a
/// `component.wasm` is one candidate. Returns the valid ones plus one
/// warning per skipped directory (a broken repo file warns, never
/// loads). No trust check here: the loader gates on trust, so the
/// scan stays a pure, testable directory read.
fn scan_project_extensions(cwd: &std::path::Path) -> (Vec<ProjectExtension>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    let dir = cwd.join(".lca").join("extensions");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return (found, warnings),
    };
    let mut dirs: Vec<_> = entries.flatten().filter(|e| e.path().is_dir()).collect();
    dirs.sort_by_key(|e| e.file_name());
    for entry in dirs {
        let path = entry.path();
        let manifest_text = match std::fs::read_to_string(path.join("extension.toml")) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let manifest = match lca_ext_host::Manifest::parse(&manifest_text) {
            Ok(manifest) => manifest,
            Err(err) => {
                warnings.push(format!(
                    "skipping project extension `{}`: {err}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                continue;
            }
        };
        match std::fs::read(path.join("component.wasm")) {
            Ok(wasm) => found.push(ProjectExtension {
                name: manifest.name.clone(),
                manifest_text,
                wasm,
            }),
            Err(_) => warnings.push(format!(
                "skipping project extension `{}`: no component.wasm",
                manifest.name
            )),
        }
    }
    (found, warnings)
}

/// Load project-local extensions (gh #138): the scan's candidates
/// register through the one-run host (same limits, same consent as
/// installed and `-e` loads), most-specific scope first so a repo's
/// own tool shadows a same-named installed one. Untrusted projects
/// ignore the directory entirely — silently, because the trust
/// prompt (which the directory itself triggers) is the UX, not a
/// per-run complaint. A `Some(false)` enablement flag skips, like
/// everywhere else.
pub fn load_project_local(
    registry: &mut lca_core::ExtensionRegistry,
    cwd: &std::path::Path,
    log_limit_bytes: usize,
    prompt: lca_permissions::SharedPrompt,
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
) {
    let (found, warnings) = scan_project_extensions(cwd);
    if found.is_empty() && warnings.is_empty() {
        return;
    }
    if !grants
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_trusted_here(cwd)
    {
        return;
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    let mut host = guest_host(cwd, log_limit_bytes, prompt, dialogs, grants);
    // Enablement is the registry's job (`apply_enablement` prunes
    // `Some(false)` after every load); the loader only gates trust.
    for candidate in found {
        match host.load(&candidate.wasm, &candidate.manifest_text) {
            Ok(handle) => registry.register(std::sync::Arc::new(handle)),
            Err(err) => eprintln!(
                "warning: skipping project extension `{}`: {err}",
                candidate.name
            ),
        }
    }
}

/// One WASM host for one-run loads (gh #70): the same limits and
/// environment installed extensions get, so consent answers come from
/// the same prompt and grant store (normal consent, preserved).
pub(crate) fn guest_host(
    cwd: &std::path::Path,
    log_limit_bytes: usize,
    prompt: lca_permissions::SharedPrompt,
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
) -> lca_ext_host::ExtHost {
    let env = crate::ext::installed_environment(cwd, prompt, dialogs, grants);
    lca_ext_host::ExtHost::new(
        lca_ext_host::ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: lca_ext_host::DEFAULT_FUEL_PER_CALL,
            log_limit_bytes,
        },
        env,
    )
}

#[cfg(test)]
mod tests {
    // Verifies: gh #138 (project-local extensions) — a
    // `.lca/extensions/<name>/` dir with a valid manifest and a
    // `component.wasm` scans; a bad manifest or a missing component
    // warns instead of loading.
    #[test]
    fn project_extension_layout_scans_valid_dirs_only() {
        let root = lca_testkit::scratch_path("lca-project-ext-scan");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        let dir = project.join(".lca").join("extensions");
        let good = dir.join("good-tool");
        std::fs::create_dir_all(&good).expect("mkdir");
        std::fs::write(
            good.join("extension.toml"),
            "name = \"good-tool\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");
        std::fs::write(good.join("component.wasm"), b"wasm-bytes").expect("component");
        let bad = dir.join("bad-tool");
        std::fs::create_dir_all(&bad).expect("mkdir");
        std::fs::write(bad.join("extension.toml"), "name = \"nope\"\n").expect("manifest");
        std::fs::write(bad.join("component.wasm"), b"wasm-bytes").expect("component");
        let nofile = dir.join("no-component");
        std::fs::create_dir_all(&nofile).expect("mkdir");
        std::fs::write(
            nofile.join("extension.toml"),
            "name = \"no-component\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");

        let (found, warnings) = super::scan_project_extensions(&project);
        assert_eq!(found.len(), 1, "only the valid dir: {found:?}");
        assert_eq!(found[0].name, "good-tool");
        assert_eq!(found[0].wasm, b"wasm-bytes");
        assert_eq!(
            warnings.len(),
            2,
            "bad manifest + missing component warn: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #138 — no `.lca/extensions` dir scans clean (absent
    // is the common case; it must cost nothing and warn nothing).
    #[test]
    fn project_extension_scan_without_the_dir_is_empty() {
        let root = lca_testkit::scratch_path("lca-project-ext-absent");
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let (found, warnings) = super::scan_project_extensions(&project);
        assert!(found.is_empty() && warnings.is_empty());
    }
}
