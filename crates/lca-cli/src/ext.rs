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
        // Install context: no session exists, so the system temp dir,
        // explicitly (gh #160) - never a process-global.
        temp: std::env::temp_dir(),
        state_dir: data,
    }
}

/// The environment the extensions in [`install_tree`] run against.
///
/// One seam, one store: `grants` is the session's own
/// `Arc<Mutex<GrantStore>>`, passed through untouched, so exactly one
/// instance manages `grants.json` in a running process (ADR-0022, gh #29
/// QA-007). This function used to re-open the file here, which made a
/// second writer whose saves clobbered the prompt's.
pub(crate) fn installed_environment(
    cwd: &std::path::Path,
    prompt: lca_permissions::SharedPrompt,
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
) -> std::sync::Arc<lca_ext_host::HostEnvironment> {
    std::sync::Arc::new(lca_ext_host::HostEnvironment {
        roots: host_roots(cwd),
        prompt: std::sync::Arc::new(std::sync::Mutex::new(prompt)),
        dialogs,
        grant_store: grants.clone(),
        project: cwd.to_path_buf(),
        proposals: None,
    })
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
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
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
    let env = installed_environment(cwd, prompt, dialogs, grants);
    let mut host = lca_ext_host::ExtHost::new(
        lca_ext_host::ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 10_000_000,
            log_limit_bytes,
        },
        env,
    );
    for (name, entry) in entries {
        // A data-only package (ADR-0030) carries no component: the host
        // reads its `resources/` directly (skills), so there is nothing to
        // register. A missing component file is that case, not an error.
        let manifest = match tree.manifest(&name) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!("warning: skipping `{name}`: {err}");
                continue;
            }
        };
        let Ok(bytes) = tree.component(&name, &entry.digest) else {
            continue;
        };
        match host.load(&bytes, &manifest) {
            Ok(handle) => registry.register(std::sync::Arc::new(handle)),
            Err(err) => eprintln!("warning: skipping `{name}`: {err}"),
        }
    }
}

/// Load one-run `-e` components (gh #70): local `.wasm` files resolve
/// beside their manifests and register like installed extensions (same
/// host, same consent). Directories are data-only packages (their
/// skills ride `--skill`, resolved in `run`); anything else refuses
/// out loud - remote references belong to `lca ext install`.
pub fn load_extra(
    registry: &mut lca_core::ExtensionRegistry,
    cwd: &std::path::Path,
    log_limit_bytes: usize,
    prompt: lca_permissions::SharedPrompt,
    dialogs: lca_permissions::SharedDialogs,
    grants: &std::sync::Arc<std::sync::Mutex<lca_permissions::GrantStore>>,
    paths: &[std::path::PathBuf],
) {
    if paths.is_empty() {
        return;
    }
    let mut host = crate::project_ext::guest_host(cwd, log_limit_bytes, prompt, dialogs, grants);
    for path in paths {
        // Resolve from the working directory, like `@file` does.
        let full = if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        };
        if full.is_dir() {
            // Data-only by design (ADR-0030): no component to link.
            // Skills already merged via `--skill` in `run`.
            continue;
        }
        if !full.is_file() {
            eprintln!(
                "warning: -e {}: no such file (remote references belong to `lca ext install`)",
                path.display()
            );
            continue;
        }
        let resolved = match lca_registry::resolve_local(&full, None) {
            Ok(resolved) => resolved,
            Err(err) => {
                eprintln!("warning: -e {}: {err}", path.display());
                continue;
            }
        };
        match host.load(&resolved.component, &resolved.manifest) {
            Ok(handle) => registry.register(std::sync::Arc::new(handle)),
            Err(err) => eprintln!("warning: -e {}: {err}", path.display()),
        }
    }
}

/// The consent answer, read from standard input: one line, then Enter.
///
/// This is the seam [`confirm_with`] is tested through - both callers share
/// this one function, so the line rule below covers the install grant and
/// `ext update`'s capability widening alike.
fn confirm(prompt: &str) -> bool {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    confirm_with(prompt, &mut stdin, &mut stdout)
}

/// Reads a consent answer from `reader`, writing `prompt` to `writer` once
/// per line asked (the gh #24 seam; the reader is injectable so the rule
/// can be pinned without a terminal).
///
/// The platform's own line discipline does the echo and requires Enter, so
/// a keystroke is only an answer once its line is finished: a line with no
/// terminator means end of input, not agreement. Both `\n` and `\r` end a
/// line - a console a previous process left in raw mode has no line
/// discipline and delivers Enter as `\r` alone, so waiting for `\n` would
/// hang the one prompt that must never hang (in that mode Ctrl+C is a
/// literal byte, which leaves no way out). `\r\n` is one terminator, not
/// two. Trimmed, case-insensitive `y`/`yes` confirms; `n`/`no` and the
/// empty line decline; end of input declines (an unattended run never
/// writes without consent); anything else is asked again.
pub fn confirm_with(
    prompt: &str,
    reader: &mut impl std::io::BufRead,
    writer: &mut impl std::io::Write,
) -> bool {
    // The `\n` half of a `\r\n` pair, dropped when the next line starts.
    // It is carried across lines instead of being checked at the end of
    // the one before it: checking needs a peek, and a peek on an idle
    // terminal is a blocking wait for a keystroke nobody is going to type
    // - the hang this whole fix exists to prevent.
    let mut drop_lf = false;
    loop {
        let _ = write!(writer, "{prompt}");
        let _ = writer.flush();
        // No line came back: input ended - or failed - before any
        // terminator, so half an answer was submitted and nothing is
        // granted. An unattended run never writes without consent.
        let Some(line) = read_answer_line(reader, &mut drop_lf) else {
            return false;
        };
        match String::from_utf8_lossy(&line)
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "y" | "yes" => return true,
            "n" | "no" | "" => return false,
            // Anything else is not an answer: ask again, the way the old
            // key loop ignored a key it did not know.
            _ => {}
        }
    }
}

/// One answer line: the bytes before its terminator, consumed with it.
/// `None` when the input ends without one - that is a half-line nobody
/// submitted, not an answer.
///
/// Enter arrives as a bare `\r` on a console left in raw mode (no line
/// discipline to translate it) and as `\r\n` from a canonical one, so both
/// end a line, and the pair is one terminator. The `\n` half is consumed
/// here - on the line *after* the `\r` that announced it, via `drop_lf` -
/// because reading ahead for it at this end would block.
fn read_answer_line(reader: &mut impl std::io::BufRead, drop_lf: &mut bool) -> Option<Vec<u8>> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let byte = next_byte(reader)?;
        if *drop_lf {
            *drop_lf = false;
            if byte == b'\n' {
                continue;
            }
        }
        match byte {
            b'\n' => return Some(line),
            b'\r' => {
                *drop_lf = true;
                return Some(line);
            }
            byte => line.push(byte),
        }
    }
}

/// The next byte, or `None` once the input is exhausted. A read that
/// fails ends the line the same way: there is no answer to read, and the
/// caller declines.
fn next_byte(reader: &mut impl std::io::BufRead) -> Option<u8> {
    let buffer = reader.fill_buf().ok()?;
    let &byte = buffer.first()?;
    reader.consume(1);
    Some(byte)
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
pub async fn run(cmd: ExtCmd, offline: bool) -> i32 {
    // gh #71: `--offline` refuses installs and updates that need the
    // network, before any request; an existing local path stays usable.
    if offline {
        let remote = match &cmd {
            ExtCmd::Install { reference, .. } => !std::path::Path::new(reference).exists(),
            ExtCmd::Update { .. } => true,
            _ => false,
        };
        if remote {
            eprintln!("error: --offline refuses remote extension installs and updates");
            return crate::exit::USAGE;
        }
    }
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
        ExtCmd::Remove { name } => {
            // R5: accept the name, the source ref, or a digest prefix.
            let name = match resolve_installed(&tree, &name) {
                Ok(Some(name)) => name,
                Ok(None) => {
                    not_installed(&tree, &name);
                    return crate::exit::USAGE;
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    return crate::exit::USAGE;
                }
            };
            // ADR-0030: state is wiped on uninstall (it is the extension's
            // own scratch, not something a reinstall should inherit).
            let removed = tree.remove(&name);
            match removed {
                Ok(true) => {
                    let _ = std::fs::remove_dir_all(crate::data_dir().join("state").join(&name));
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
            }
        }
        ExtCmd::Info { name } => info(&tree, &name),
        ExtCmd::Enable { name } => set_enabled(&name, true),
        ExtCmd::Disable { name } => set_enabled(&name, false),
        ExtCmd::State { cmd } => match cmd {
            StateCmd::Clear { name } => clear_state(&name),
        },
        ExtCmd::List => list(&tree),
    }
}

/// Delete one extension's `state` bag (ADR-0030).
fn clear_state(name: &str) -> i32 {
    clear_state_in(&crate::data_dir(), name)
}

/// The testable core of [`clear_state`]: the data dir is injected.
fn clear_state_in(data: &std::path::Path, name: &str) -> i32 {
    let dir = data.join("state").join(name);
    if !dir.exists() {
        println!("{name}: no state to clear");
        return crate::exit::OK;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            println!("cleared {name} state");
            crate::exit::OK
        }
        Err(err) => {
            eprintln!("error: cannot clear state: {err}");
            crate::exit::INTERNAL
        }
    }
}

/// The total bytes in one extension's `state` bag (0 when absent).
fn state_bytes(data: &std::path::Path, name: &str) -> u64 {
    let dir = data.join("state").join(name);
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.metadata().ok())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Enable or disable an extension for the current project (FR-PROV-9,
/// SRDD's per-project enable/disable). The grant store is the same one
/// the loader reads through `extension_enabled`.
fn set_enabled(name: &str, enabled: bool) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: cannot read the working directory: {err}");
            return crate::exit::INTERNAL;
        }
    };
    set_enabled_in(&crate::data_dir(), &cwd, name, enabled)
}

/// The testable core of [`set_enabled`]: the data dir and project are
/// injected so a test never touches the real store or the process cwd.
fn set_enabled_in(data: &std::path::Path, cwd: &std::path::Path, name: &str, enabled: bool) -> i32 {
    // gh #139: `builtin:` is pi's spelling for a first-party extension,
    // not a second record — the loader reads the plain name.
    let name = name.strip_prefix("builtin:").unwrap_or(name);
    let mut store = match lca_permissions::GrantStore::open(&data.join("grants.json")) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return crate::exit::INTERNAL;
        }
    };
    if let Err(err) = store.set_extension_enabled(cwd, name, enabled) {
        eprintln!("error: {err}");
        return crate::exit::INTERNAL;
    }
    println!(
        "{} {name} for this project",
        if enabled { "enabled" } else { "disabled" }
    );
    crate::exit::OK
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
/// Why a manifest failed the install-time validation pass (the loader's
/// parser is the authority; this wraps it and the host window check).
#[derive(Debug, thiserror::Error)]
pub(crate) enum ManifestError {
    /// The loader's own parse/validation failure.
    #[error("{0}")]
    Load(#[from] lca_ext_host::LoadError),
    /// The declared ABI line is outside this host's window (FR-EXT-8).
    #[error("manifest targets ABI {declared}, outside this host's supported window ({window})")]
    Abi {
        /// The declared `major.minor`.
        declared: String,
        /// The accepted window, named so the message says what to build for.
        window: &'static str,
    },
}

fn parse_manifest_strict(
    manifest: &str,
) -> Result<(String, String, String, String), ManifestError> {
    let parsed = lca_ext_host::Manifest::parse(manifest)?;
    if !parsed.abi_in_window() {
        return Err(ManifestError::Abi {
            declared: parsed.abi.clone(),
            window: lca_ext_host::SUPPORTED_ABI_WINDOW,
        });
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
            Some(name) => match resolve_installed(tree, &name) {
                Ok(Some(name)) => vec![name],
                Ok(None) => {
                    not_installed(tree, &name);
                    return crate::exit::USAGE;
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    return crate::exit::USAGE;
                }
            },
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

fn info(tree: &InstallTree, query: &str) -> i32 {
    let name = match resolve_installed(tree, query) {
        Ok(Some(name)) => name,
        Ok(None) => {
            if builtin_names().contains(&query) {
                println!("{query}: built into this binary (native, unsandboxed)");
                println!("denials:  {}", tree.denial_count(query));
                return crate::exit::OK;
            }
            not_installed(tree, query);
            return crate::exit::USAGE;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return crate::exit::USAGE;
        }
    };
    match tree.entry(&name) {
        Ok(Some(entry)) => {
            let manifest = tree.manifest(&name).unwrap_or_default();
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
            println!("denials:  {}", tree.denial_count(&name));
            // ADR-0030: the state bag is visible (and clearable).
            let state_bytes = state_bytes(&crate::data_dir(), &name);
            if state_bytes > 0 {
                println!("state:    {state_bytes} bytes (`lca ext state clear {name}`)");
            }
            crate::exit::OK
        }
        Ok(None) => {
            // The resolver returned a name from the lockfile, so this is
            // only reachable on a race (removed between calls).
            eprintln!("error: `{name}` is not installed");
            crate::exit::USAGE
        }
        Err(err) => {
            eprintln!("error: {err}");
            crate::exit::INTERNAL
        }
    }
}

/// Resolve a user-typed extension query against the installed tree (R5):
/// the exact name, the source reference, the full digest, or a digest
/// prefix (full or short). An ambiguous prefix is an error naming the
/// matches; no match returns `Ok(None)` so the caller can print what does
/// exist.
fn resolve_installed(tree: &InstallTree, query: &str) -> Result<Option<String>, String> {
    let entries = tree.list().map_err(|err| err.to_string())?;
    // Exact name wins outright, before any fuzzy matching.
    if let Some((name, _)) = entries.iter().find(|(name, _)| name == query) {
        return Ok(Some(name.clone()));
    }
    let query = query.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let matches: Vec<&String> = entries
        .iter()
        .filter(|(_, entry)| {
            entry.source == query
                || entry.digest == query
                || short_digest(&entry.digest) == query
                || (!query.is_empty() && entry.digest.starts_with(query))
        })
        .map(|(name, _)| name)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [name] => Ok(Some((*name).clone())),
        many => Err(format!(
            "`{query}` is ambiguous; it matches {}. Use the exact name.",
            many.iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The "not installed" report (R5): name what does exist instead of a flat
/// refusal, so the owner's four-command transcript succeeds on attempt one.
fn not_installed(tree: &InstallTree, query: &str) {
    let mut known: Vec<String> = tree
        .list()
        .map(|entries| entries.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default();
    known.extend(builtin_names().iter().map(|name| (*name).to_string()));
    known.sort();
    known.dedup();
    if known.is_empty() {
        eprintln!(
            "error: `{query}` is not installed; nothing is installed yet \
             (install one with `lca ext install <ref>`)"
        );
    } else {
        eprintln!(
            "error: `{query}` is not installed. Known: {} (see `lca ext list`)",
            known.join(", ")
        );
    }
}

/// The first-party native extensions this build carries (labels only:
/// they have no lockfile record by design, ADR-0013).
pub(crate) fn builtin_names() -> Vec<&'static str> {
    let mut names = vec!["hooks-example"];
    if cfg!(feature = "bundled-openai-compat") {
        names.push("openai-compatible");
    }
    if cfg!(feature = "bundled-compaction-default") {
        names.push("compaction-default");
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
    use super::{clear_state_in, installed_environment, state_bytes};

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
            super::set_enabled_in(&root, &project, "builtin:compaction-default", false),
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
            super::set_enabled_in(&root, &project, "skills", false),
            crate::exit::OK
        );
        let store = lca_permissions::GrantStore::open(&root.join("grants.json")).expect("open");
        assert_eq!(store.extension_enabled(&project, "skills"), Some(false));
        assert_eq!(
            super::set_enabled_in(&root, &project, "skills", true),
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

        let code = super::update(&tree, Some("demo-pack".to_string()), false, true).await;
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
        let cap = crate::secret_capabilities(&root, &project, "openai-compatible", &grants);

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
}
