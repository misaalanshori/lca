//! `lca ext install|update|remove|info|list`: resolve, consent, store
//! (FR-DIST-*, the SRDD's extension-tree paragraph, ADR-0010).
//!
//! Everything here reads and writes [`lca_registry`]'s tree; the
//! consent screen shows exactly the manifest's declared grants in the
//! capability catalog's words before anything is written, and a "no"
//! writes nothing at all.

use lca_registry::{InstallTree, Resolved};

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
        private: data.join("private"),
        home_config: crate::config_dir(),
        temp: crate::session_temp(),
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
    prompt: lca_permissions::SharedPrompt,
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
        prompt: std::sync::Arc::new(std::sync::Mutex::new(prompt)),
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
    use std::io::{IsTerminal, Write as _};
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    if !std::io::stdin().is_terminal() {
        // Piped input (tests, scripts): read a line. Nothing on stdin is a
        // decline, so an unattended install never writes without consent.
        let mut line = String::new();
        return match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => false,
            Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        };
    }
    // Interactive: one key, no Enter needed. Raw mode also works when a
    // previous process left the console raw, where `read_line` would wait
    // forever for a newline that never completes.
    let _ = crossterm::terminal::enable_raw_mode();
    let raw = RawModeGuard;
    let answer = loop {
        match crossterm::event::read() {
            Ok(crossterm::event::Event::Key(key))
                if key.kind == crossterm::event::KeyEventKind::Press =>
            {
                match key.code {
                    crossterm::event::KeyCode::Char('y' | 'Y') => break true,
                    crossterm::event::KeyCode::Char('n' | 'N')
                    | crossterm::event::KeyCode::Esc
                    | crossterm::event::KeyCode::Enter => break false,
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    drop(raw);
    println!("{}", if answer { "y" } else { "n" });
    answer
}

/// Turns raw mode back off when dropped, including on the error paths.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
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
            yes,
        } => match lca_registry::resolve(&reference, manifest.as_deref()).await {
            Ok(resolved) => install(resolved, tree, yes),
            Err(err) => {
                eprintln!("error: {err}");
                error_code(&err)
            }
        },
        ExtCmd::Update { name, all, yes } => update(&tree, name, all, yes).await,
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
fn install(resolved: Resolved, tree: InstallTree, yes: bool) -> i32 {
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
    if !yes && !confirm("Allow these capabilities? [y/N] ") {
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
///
/// `lca_ext_host::Manifest::parse` is the authority: it checks the
/// identifier rules, the ABI line, every capability declaration, the
/// credential namespace, and oauth-requires-net. An install that passes
/// here loads later, and - critically - a manifest whose `name` is not a
/// legal identifier is refused before `InstallTree::install` ever joins it
/// onto the filesystem.
fn parse_manifest_strict(manifest: &str) -> Result<(String, String, String, String), String> {
    let parsed = lca_ext_host::Manifest::parse(manifest).map_err(|err| err.to_string())?;
    if !parsed.abi_in_window() {
        return Err(format!(
            "manifest targets ABI {}, outside this host's supported window",
            parsed.abi
        ));
    }
    // `description` is display-only and not part of the loader's contract.
    let description = manifest
        .parse::<toml::Value>()
        .ok()
        .and_then(|value| {
            value
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    Ok((parsed.name, parsed.version, parsed.abi, description))
}

async fn update(tree: &InstallTree, name: Option<String>, all: bool, yes: bool) -> i32 {
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
        // Refuse a rename: installing under a different name would leave the
        // old lockfile entry (and its tree) orphaned.
        match parse_manifest_strict(&resolved.manifest) {
            Ok((resolved_name, ..)) if resolved_name == name => {}
            Ok((resolved_name, ..)) => {
                eprintln!(
                    "error: {name}: the update's manifest is named `{resolved_name}`; refusing to install under a different name"
                );
                code = crate::exit::USAGE;
                continue;
            }
            Err(err) => {
                eprintln!("error: {name}: {err}");
                code = crate::exit::USAGE;
                continue;
            }
        }
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
                if yes || confirm(&format!("Apply the new capabilities for {name}? [y/N] ")) {
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

#[cfg(test)]
mod tests {
    use super::parse_manifest_strict;

    const VALID: &str = r#"name = "word-count"
version = "1.0.0"
abi = "1.0"
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
        assert_eq!(abi, "1.0");
        assert_eq!(description, "Counts words.");
    }

    // Verifies: the install screen cannot be shown for a name that would
    // escape the install tree (FR-DIST-5; `InstallTree::install` refuses too).
    #[test]
    fn a_traversal_name_is_refused() {
        let err = parse_manifest_strict(&VALID.replace("word-count", "../../escape"))
            .expect_err("traversal refused");
        assert!(
            err.contains("name"),
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
abi = "1.0"
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
        let err = parse_manifest_strict(&VALID.replace("abi = \"1.0\"", "abi = \"9.9\""))
            .expect_err("out-of-window ABI refused");
        assert!(
            err.contains("outside this host's supported window"),
            "{err}"
        );
    }
}
