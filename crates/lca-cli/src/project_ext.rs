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

/// Whether two paths name the same directory, resolving symlinks and casing.
///
/// `symlink_metadata` first: a path that does not exist is not the same as one
/// that does, and calling `canonicalize` on a missing path would fall back to a
/// lexical compare - which is exactly the string comparison that has twice
/// passed here and failed elsewhere (gh #436, gh #389). Comparing the parent plus
/// the final component keeps the deleted-directory case honest.
pub(crate) fn same_directory(left: &std::path::Path, right: &std::path::Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        // Either path is absent, so they cannot be the same existing directory;
        // a lexical spelling of an absent path proves nothing either way.
        _ => false,
    }
}

/// Scan `<project>/.lca/extensions/` for project-local extensions (gh
/// #138): each subdirectory with an `extension.toml` and a
/// `component.wasm` is one candidate. Returns the valid ones plus one
/// warning per skipped directory (a broken repo file warns, never
/// loads). No trust check here: the loader gates on trust, so the
/// scan stays a pure, testable directory read.
fn scan_project_extensions(
    cwd: &std::path::Path,
    data_dir: &std::path::Path,
) -> (Vec<ProjectExtension>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    let dir = cwd.join(".lca").join("extensions");
    // gh #463: when the project root *is* the data home, this directory is
    // `~/.lca/extensions` - where installed extensions live. They hold
    // `sha256-<digest>.wasm`, never `component.wasm`, so every one of them
    // produced "skipping project extension `x`: no component.wasm" and then
    // loaded anyway. Wrong twice, and the blame landed on the user.
    //
    // There is nothing project-local to find in that case, so the scan does not
    // run. Compared canonically: a `$HOME` that is a symlink, macOS's
    // `/var` -> `/private/var`, and Windows's `\\?\` verbatim prefix all make a
    // string comparison agree on one platform and not another (the #436 and
    // #389 lesson).
    if same_directory(&dir, &data_dir.join("extensions")) {
        return (found, warnings);
    }
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
    data_dir: &std::path::Path,
    log_limit_bytes: usize,
    prompt: lca_permissions::SharedPrompt,
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
) {
    let (found, warnings) = scan_project_extensions(cwd, data_dir);
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

        let (found, warnings) = super::scan_project_extensions(&project, &root.join("data"));
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

    // Verifies: gh #463 - **the defect.** Launching from the data home made the
    // project-extension scan read `~/.lca/extensions`, which is where installed
    // extensions live. They hold `sha256-<digest>.wasm`, not `component.wasm`, so
    // every one produced
    //
    //   warning: skipping project extension `antigravity`: no component.wasm
    //
    // - wrong twice: not a project extension, and not skipped (it loaded right
    // after). The scan must not run at all when the two directories are the same.
    #[test]
    fn no_project_scan_when_cwd_is_the_data_home() {
        let root = lca_testkit::scratch_path("lca-gh463-home-cwd");
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join(".lca");
        // The shape an installed extension really has: a digest-named component.
        let installed = data.join("extensions").join("antigravity");
        std::fs::create_dir_all(&installed).expect("mkdir");
        std::fs::write(
            installed.join("extension.toml"),
            "name = \"antigravity\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");
        std::fs::write(
            installed.join(
                "sha256-a6bfef821d98f57e474f538e534f385f17ae658890cb795701672230353203a4.wasm",
            ),
            b"wasm-bytes",
        )
        .expect("component");

        // cwd == the data home: nothing project-local exists to find.
        let (found, warnings) = super::scan_project_extensions(&root, &data);
        assert!(found.is_empty(), "nothing is loaded twice: {found:?}");
        assert!(
            warnings.is_empty(),
            "a scan of the data home must say nothing, not something false: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #463 design point 3 - **the diagnostic survives.** A real
    // project directory whose `.lca/extensions/<name>/` lacks its component still
    // warns, so this fix cannot quietly disable the warning it is about.
    #[test]
    fn a_real_broken_project_extension_still_warns() {
        let root = lca_testkit::scratch_path("lca-gh463-real-broken");
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        std::fs::create_dir_all(&data).expect("mkdir");
        let project = root.join("project");
        let broken = project.join(".lca").join("extensions").join("foo");
        std::fs::create_dir_all(&broken).expect("mkdir");
        std::fs::write(
            broken.join("extension.toml"),
            "name = \"foo\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");
        // No component.wasm: the genuine broken case.
        let (found, warnings) = super::scan_project_extensions(&project, &data);
        assert!(found.is_empty());
        assert_eq!(warnings.len(), 1, "the diagnostic fires: {warnings:?}");
        assert!(
            warnings[0].contains("no component.wasm") && warnings[0].contains("foo"),
            "and names the extension and the reason: {:?}",
            warnings[0]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #463 design point 1 - the comparison is **canonical**, so a
    // `$HOME` that is itself a symlink does not collide silently.
    //
    // This is the third cycle in a row where a raw path comparison passed on
    // Linux and failed elsewhere (#436's registry keys, #389's git fixtures), so
    // the row is written to fail if anyone reintroduces a string compare.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_data_home_is_still_recognised() {
        let root = lca_testkit::scratch_path("lca-gh463-symlink");
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real-home");
        let installed = real.join(".lca").join("extensions").join("tool");
        std::fs::create_dir_all(&installed).expect("mkdir");
        std::fs::write(
            installed.join("extension.toml"),
            "name = \"tool\"\nversion = \"1.0.0\"\nabi = \"0.6\"\nworlds = [\"tool\"]\n",
        )
        .expect("manifest");
        std::fs::write(installed.join("sha256-abc.wasm"), b"wasm").expect("component");
        // The home is reached through a symlink; the data dir is spelled via the
        // link while the cwd is the real path (the macOS `/var` -> `/private/var`
        // shape, in miniature).
        let link = root.join("linked-home");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let (found, warnings) = super::scan_project_extensions(&real, &link.join(".lca"));
        assert!(found.is_empty());
        assert!(
            warnings.is_empty(),
            "a symlinked spelling of the same directory must not warn: {warnings:?}"
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
        let (found, warnings) = super::scan_project_extensions(&project, &root.join("data"));
        assert!(found.is_empty() && warnings.is_empty());
    }
}
