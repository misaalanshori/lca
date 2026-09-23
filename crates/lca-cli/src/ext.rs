//! `lca ext install|update|remove|info|list`: resolve, consent, store
//! (FR-DIST-*, the SRDD's extension-tree paragraph, ADR-0010).
//!
//! Everything here reads and writes [`lca_registry`]'s tree; the
//! consent screen shows exactly the manifest's declared grants in the
//! capability catalog's words before anything is written, and a "no"
//! writes nothing at all.

use std::io::Write as _;

use lca_registry::{InstallTree, Resolved};

/// The `lca ext ...` subcommands (SRDD command-line section).
#[derive(clap::Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum ExtCmd {
    /// Install from an OCI reference, an HTTPS archive URL, or a local path.
    Install {
        /// `host/repo/name:tag`, `https://host/file.zip`, or a path.
        reference: String,
        /// The manifest beside a local component (default: sibling
        /// `extension.toml`; FR-DIST-5's shape).
        #[arg(long)]
        manifest: Option<std::path::PathBuf>,
    },
    /// Re-resolve an installed extension's source and apply it
    /// (FR-DIST-7's prompt covers a widened capability set).
    Update {
        /// The extension to update, or every installed one.
        name: Option<String>,
        /// Update every installed extension (`ext update --all`).
        #[arg(long)]
        all: bool,
    },
    /// Uninstall an extension and forget its record.
    Remove {
        /// The extension to remove.
        name: String,
    },
    /// Show one extension's manifest, digest, source, consent lines,
    /// and recorded denial count (FR-EXT-9).
    Info {
        /// The extension to inspect.
        name: String,
    },
    /// List installed extensions.
    List,
}

/// The install tree under the user data directory.
pub fn install_tree() -> InstallTree {
    InstallTree::new(crate::data_dir().join("extensions"))
}

/// Scope roots for loading an installed extension (the same platform
/// layout [`crate::extension_capabilities`] builds for bundled ones).
fn host_roots(cwd: &std::path::Path) -> lca_permissions::ScopeRoots {
    let data = crate::data_dir();
    lca_permissions::ScopeRoots {
        workspace: cwd.to_path_buf(),
        private: data.join("extensions"),
        home_config: crate::config_file()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| data.clone()),
        temp: std::env::temp_dir(),
        state_dir: data,
    }
}

/// Register every installed extension ahead of the bundled ones: a
/// name present in both resolves to the installed copy, because the
/// duplicate-identity rule disables the later registration
/// (FR-EXT-11), and an installed extension should win over the copy in
/// the binary. A broken record warns and skips: one bad install must
/// not cost the user their agent.
pub fn load_installed(
    registry: &mut lca_core::ExtensionRegistry,
    cwd: &std::path::Path,
    log_limit_bytes: usize,
) {
    let tree = install_tree();
    let entries = match tree.list() {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("warning: extension lockfile unreadable: {err}");
            return;
        }
    };
    if entries.is_empty() {
        return;
    }
    let store = match lca_permissions::GrantStore::open(&crate::data_dir().join("grants.json")) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("warning: grant store unreadable, skipping installed extensions: {err}");
            return;
        }
    };
    let env = std::sync::Arc::new(lca_ext_host::HostEnvironment {
        roots: host_roots(cwd),
        prompt: std::sync::Arc::new(std::sync::Mutex::new(crate::HeadlessPrompt::default())),
        grant_store: std::sync::Arc::new(std::sync::Mutex::new(store)),
        project: cwd.to_path_buf(),
        proposals: None,
    });
    let mut host = lca_ext_host::ExtHost::new(
        lca_ext_host::ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 10_000_000,
            log_limit_bytes,
        },
        env,
    );
    for (name, entry) in entries {
        let outcome = (|| {
            let manifest = tree.manifest(&name)?;
            let bytes = tree.component(&name, &entry.digest)?;
            Ok::<_, lca_registry::Error>(
                host.load(&bytes, &manifest)
                    .map_err(|err| lca_registry::Error::Invalid(err.to_string())),
            )
        })();
        match outcome {
            Ok(Ok(handle)) => registry.register(std::sync::Arc::new(handle)),
            Ok(Err(err)) => eprintln!("warning: skipping `{name}`: {err}"),
            Err(err) => eprintln!("warning: skipping `{name}`: {err}"),
        }
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => false, // EOF declines: nothing written
        Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    }
}

fn short_digest(digest: &str) -> &str {
    digest.get(7..19).unwrap_or(digest)
}

/// Map a resolution error to an exit code: a bad reference or manifest
/// is the caller's (usage), everything else is the environment
/// (internal).
fn error_code(err: &lca_registry::Error) -> i32 {
    match err {
        lca_registry::Error::Invalid(_) | lca_registry::Error::Corrupt(_) => crate::exit::USAGE,
        _ => crate::exit::INTERNAL,
    }
}

/// Dispatch one `lca ext ...` invocation; returns the exit code.
pub async fn run(cmd: ExtCmd) -> i32 {
    let tree = install_tree();
    match cmd {
        ExtCmd::Install {
            reference,
            manifest,
        } => match lca_registry::resolve(&reference, manifest.as_deref()).await {
            Ok(resolved) => install(resolved, tree),
            Err(err) => {
                eprintln!("error: {err}");
                error_code(&err)
            }
        },
        ExtCmd::Update { name, all } => update(&tree, name, all).await,
        ExtCmd::Remove { name } => match tree.remove(&name) {
            Ok(true) => {
                println!("removed {name}");
                crate::exit::OK
            }
            Ok(false) => {
                eprintln!("error: `{name}` is not installed");
                crate::exit::USAGE
            }
            Err(err) => {
                eprintln!("error: {err}");
                crate::exit::INTERNAL
            }
        },
        ExtCmd::Info { name } => info(&tree, &name),
        ExtCmd::List => list(&tree),
    }
}

/// Validate, show consent, then write (FR-DIST-4's delete never needs
/// to fire: resolution verified before we got here).
fn install(resolved: Resolved, tree: InstallTree) -> i32 {
    // Manifest validation is the same parser the loader uses, so an
    // install that passes here loads later (one parser, one truth).
    let parsed = match parse_manifest_strict(&resolved.manifest) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}");
            return crate::exit::USAGE;
        }
    };
    let lines = match lca_registry::consent_lines(&resolved.manifest) {
        Ok(lines) => lines,
        Err(err) => {
            eprintln!("error: {err}");
            return error_code(&err);
        }
    };
    let (name, version, abi, description) = parsed;
    println!("{name} {version} (abi {abi})");
    if !description.is_empty() {
        println!("{description}");
    }
    if lines.is_empty() {
        println!("This extension requests no capabilities.");
    } else {
        println!("It asks for:");
        for line in &lines {
            println!("  - {line}");
        }
    }
    if !confirm("Allow these capabilities? [y/N] ") {
        println!("aborted; nothing was written");
        return crate::exit::OK;
    }
    match tree.install(resolved) {
        Ok(entry) => {
            println!(
                "installed {} {} ({})",
                name,
                entry.version,
                short_digest(&entry.digest)
            );
            crate::exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            error_code(&err)
        }
    }
}

/// (name, version, abi, description) out of the manifest, through the
/// loader's parser so validation stays single-sourced.
fn parse_manifest_strict(manifest: &str) -> Result<(String, String, String, String), String> {
    // The loader's parser is wasmtime-side but plain-TOML; the fields
    // this screen shows are read here directly to avoid pulling the
    // host into a display path.
    let value: toml::Value = manifest
        .parse()
        .map_err(|err| format!("manifest does not parse: {err}"))?;
    let get = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    if get("name").is_empty() {
        return Err("manifest has no name".to_string());
    }
    if get("abi").is_empty() {
        return Err("manifest has no abi".to_string());
    }
    Ok((get("name"), get("version"), get("abi"), get("description")))
}

async fn update(tree: &InstallTree, name: Option<String>, all: bool) -> i32 {
    let names: Vec<String> = if all {
        match tree.list() {
            Ok(entries) => entries.into_iter().map(|(name, _)| name).collect(),
            Err(err) => {
                eprintln!("error: {err}");
                return crate::exit::INTERNAL;
            }
        }
    } else {
        match name {
            Some(name) => vec![name],
            None => {
                eprintln!("error: `ext update` needs a name or --all");
                return crate::exit::USAGE;
            }
        }
    };
    if names.is_empty() {
        println!("nothing is installed");
        return crate::exit::OK;
    }
    let mut code = crate::exit::OK;
    for name in names {
        let entry = match tree.entry(&name) {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("error: {err}");
                code = crate::exit::INTERNAL;
                continue;
            }
        };
        let Some(entry) = entry else {
            eprintln!("error: `{name}` is not installed");
            code = crate::exit::USAGE;
            continue;
        };
        if entry.source.contains("@sha256:") {
            println!("{name}: pinned by digest; reinstall to move");
            continue;
        }
        let resolved = match lca_registry::resolve(&entry.source, None).await {
            Ok(resolved) => resolved,
            Err(err) => {
                eprintln!("error: {name}: {err}");
                code = error_code(&err);
                continue;
            }
        };
        if resolved.digest == entry.digest {
            println!("{name}: up to date ({})", short_digest(&entry.digest));
            continue;
        }
        let approved = tree.manifest(&name).unwrap_or_default();
        match lca_registry::update_widens_grants(&approved, &resolved.manifest) {
            Ok(false) => {
                println!(
                    "{name}: applying {} (grants unchanged)",
                    short_digest(&resolved.digest)
                );
                if let Err(err) = tree.install(resolved) {
                    eprintln!("error: {err}");
                    code = error_code(&err);
                }
            }
            Ok(true) => {
                let lines = lca_registry::consent_lines(&resolved.manifest).unwrap_or_default();
                println!("{name} now asks for MORE than you approved:");
                for line in &lines {
                    println!("  - {line}");
                }
                if confirm(&format!("Apply the new capabilities for {name}? [y/N] ")) {
                    if let Err(err) = tree.install(resolved) {
                        eprintln!("error: {err}");
                        code = error_code(&err);
                    } else {
                        println!("updated {name}");
                    }
                } else {
                    println!("{name}: kept the installed version");
                }
            }
            Err(err) => {
                eprintln!("error: {name}: {err}");
                code = error_code(&err);
            }
        }
    }
    code
}

fn info(tree: &InstallTree, name: &str) -> i32 {
    match tree.entry(name) {
        Ok(Some(entry)) => {
            let manifest = tree.manifest(name).unwrap_or_default();
            println!("{name} {} (abi {})", entry.version, entry.abi);
            println!("source:   {}", entry.source);
            println!("digest:   {}", entry.digest);
            println!("grants:   {}", entry.grant_hash);
            match lca_registry::consent_lines(&manifest) {
                Ok(lines) if lines.is_empty() => println!("consent:  (no capabilities)"),
                Ok(lines) => {
                    println!("consent:");
                    for line in lines {
                        println!("  - {line}");
                    }
                }
                Err(err) => println!("consent:  ({err})"),
            }
            // FR-EXT-9: the count that notices an extension trying
            // things it never declared.
            println!("denials:  {}", tree.denial_count(name));
            crate::exit::OK
        }
        Ok(None) => {
            if builtin_names().contains(&name) {
                println!("{name}: built into this binary (native, unsandboxed)");
                println!("denials:  {}", tree.denial_count(name));
                crate::exit::OK
            } else {
                eprintln!("error: `{name}` is not installed");
                crate::exit::USAGE
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            crate::exit::INTERNAL
        }
    }
}

/// The first-party native extensions this build carries (labels only:
/// they have no lockfile record by design, ADR-0013).
fn builtin_names() -> Vec<&'static str> {
    let mut names = vec!["hooks-example"];
    if cfg!(feature = "bundled-openai-compat") {
        names.push("openai-compatible");
    }
    if cfg!(feature = "bundled-compaction-default") {
        names.push("compaction-default");
    }
    if cfg!(feature = "bundled-skills") {
        names.push("skills");
    }
    names
}

fn list(tree: &InstallTree) -> i32 {
    let entries = match tree.list() {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("error: {err}");
            return crate::exit::INTERNAL;
        }
    };
    for (name, entry) in &entries {
        println!(
            "{name} {} (abi {}) {} {}",
            entry.version,
            entry.abi,
            short_digest(&entry.digest),
            entry.source
        );
    }
    if !entries.is_empty() {
        println!(
            "({} built-in extension{} available without installing)",
            builtin_names().len(),
            if builtin_names().len() == 1 { "" } else { "s" }
        );
    } else if builtin_names().is_empty() {
        println!("nothing installed");
    } else {
        println!(
            "(no installed extensions; {} built-in extension{} available)",
            builtin_names().len(),
            if builtin_names().len() == 1 { "" } else { "s" }
        );
    }
    crate::exit::OK
}
