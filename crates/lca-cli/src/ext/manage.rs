//! `lca ext update|remove|info|list|enable|disable|config`, and the per-project
//! enable flag they share (FR-PROV-9, ADR-0030, gh #3, gh #59).
//!
//! Split out of `ext/mod.rs` at gate 11's 1,200-line ceiling (gh #543). The seam
//! is **an installed tree**: what is in it, what a query resolves to, updating
//! one (including the rename refusal and the capability-widening prompt gh #50
//! shares with `lca update --extensions`), removing it with its state bag
//! (ADR-0030), listing it, and toggling it per project. Nothing here writes a
//! newly resolved component - that is `install.rs`.

use lca_registry::InstallTree;

use super::install::{confirm, error_code, parse_manifest_strict, short_digest};

/// Delete one extension's `state` bag (ADR-0030).
pub(super) fn clear_state(name: &str) -> i32 {
    clear_state_in(&crate::data_dir(), name)
}

/// The testable core of [`clear_state`]: the data dir is injected.
pub(super) fn clear_state_in(data: &std::path::Path, name: &str) -> i32 {
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
pub(super) fn state_bytes(data: &std::path::Path, name: &str) -> u64 {
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
pub(super) fn set_enabled(name: &str, enabled: bool) -> i32 {
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
pub(super) fn set_enabled_in(
    data: &std::path::Path,
    cwd: &std::path::Path,
    name: &str,
    enabled: bool,
) -> i32 {
    match set_extension_enabled(data, cwd, name, enabled) {
        Ok(notice) => {
            println!("{notice}");
            crate::exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            crate::exit::INTERNAL
        }
    }
}

/// Toggle one extension without printing (gh #3, the TUI manager):
/// persist per project in the grant store; the caller applies it live.
/// Returns the notice to show.
pub fn set_extension_enabled(
    data: &std::path::Path,
    cwd: &std::path::Path,
    name: &str,
    enabled: bool,
) -> Result<String, String> {
    // gh #139: `builtin:` is pi's spelling for a first-party extension,
    // not a second record — the loader reads the plain name.
    let name = name.strip_prefix("builtin:").unwrap_or(name);
    let mut store = lca_permissions::GrantStore::open(&data.join("grants.json"))
        .map_err(|err| format!("cannot open the grant store: {err}"))?;
    store
        .set_extension_enabled(cwd, name, enabled)
        .map_err(|err| err.to_string())?;
    Ok(format!(
        "{} {name} for this project",
        if enabled { "enabled" } else { "disabled" }
    ))
}

/// `lca update --extensions`: every installed extension, yes to
/// capability widening alike (gh #50 reuses the non-interactive
/// `ext update --all` path).
pub async fn update_all(tree: &InstallTree) -> i32 {
    update(tree, None, true, true).await
}

pub(super) async fn update(tree: &InstallTree, name: Option<String>, all: bool, yes: bool) -> i32 {
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

pub(super) fn info(tree: &InstallTree, query: &str) -> i32 {
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
            if !entry.commit.is_empty() {
                println!("commit:   {}", entry.commit);
            }
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
pub(super) fn resolve_installed(tree: &InstallTree, query: &str) -> Result<Option<String>, String> {
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
pub(crate) fn not_installed(tree: &InstallTree, query: &str) {
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

pub(super) fn list(tree: &InstallTree) -> i32 {
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
    // Gh #250: name the bundled extensions outright, marked so they
    // read apart from installed packages - the old count-only
    // summary hid which ones ship with the binary.
    for name in builtin_names() {
        println!("{name} (built-in, abi {})", lca_session::ABI_VERSION);
    }
    if entries.is_empty() && builtin_names().is_empty() {
        println!("nothing installed");
    }
    crate::exit::OK
}
