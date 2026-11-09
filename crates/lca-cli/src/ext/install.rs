//! `lca ext install|update`: resolve, verify, consent, and write (FR-DIST-*
//! and its extension-tree paragraph, ADR-0010).
//!
//! Split out of `ext/mod.rs` at gate 11's 1,200-line ceiling (gh #543). The
//! seam is the **write path**: resolution, the manifest parser the loader shares
//! (`parse_manifest_strict` - one parser, one truth), the capability consent
//! screen in the catalog's own words, the tree write, and the `--offline`
//! refusal that precedes any of it. `manage.rs` holds what *reads and toggles*
//! an installed tree; `mod.rs` holds the command vocabulary and dispatch.

use lca_registry::{InstallTree, Resolved};

use super::config::config;
use super::manage::{
    clear_state, info, list, not_installed, resolve_installed, set_enabled, update,
};
use crate::ext::{ExtCmd, StateCmd};

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
            fuel_per_call: lca_ext_host::DEFAULT_FUEL_PER_CALL,
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
pub(super) fn confirm(prompt: &str) -> bool {
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

pub(super) fn short_digest(digest: &str) -> &str {
    digest.get(7..19).unwrap_or(digest)
}

/// Map a resolution error to an exit code: a bad reference or manifest
/// is the caller's (usage), everything else is the environment
/// (internal).
pub(super) fn error_code(err: &lca_registry::Error) -> i32 {
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
        ExtCmd::Config { name, filters } => config(&tree, &name, &filters),
        ExtCmd::State { cmd } => match cmd {
            StateCmd::Clear { name } => clear_state(&name),
        },
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

pub(super) fn parse_manifest_strict(
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
