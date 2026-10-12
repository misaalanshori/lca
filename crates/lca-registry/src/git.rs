//! Git package sources (gh #59, ADR-0010's fourth kind): a `git+<url>`
//! reference clones into a scratch dir, pins the commit, and resolves
//! the checkout with the same shape rules as an archive (one top-level
//! `extension.toml`, at most one top-level `*.wasm`, `resources/**`).
//! The lockfile keeps the typed ref (branches advance on update) and
//! the pin separately, mirroring OCI's ref-plus-digest split.
//!
//! The `git` CLI does the transport (no new dependency); tests use
//! local-path repos, so they run fully offline.

use std::path::Path;

use super::{Error, Resolved, manifest_declares_resources, read_resource_dir};

/// Split a `git+...` source into its URL and optional `#rev`.
/// `None` when the source is not a git source at all.
pub fn parse_git_source(source: &str) -> Option<(String, Option<String>)> {
    let rest = source.strip_prefix("git+")?;
    if rest.is_empty() {
        return None;
    }
    match rest.rsplit_once('#') {
        Some((url, rev)) if !url.is_empty() && !rev.is_empty() => {
            Some((url.to_string(), Some(rev.to_string())))
        }
        _ => Some((rest.to_string(), None)),
    }
}

fn git(args: &[&str], dir: &Path) -> Result<String, Error> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| Error::Fetch(format!("git clone needs the `git` command on PATH: {err}")))?;
    if !out.status.success() {
        return Err(Error::Fetch(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Clone `url`, check out `rev` (HEAD when absent), and resolve the
/// checkout. The scratch dir is removed on every path out, success or
/// failure, so a failed clone leaves nothing behind.
pub fn resolve_git(source: &str) -> Result<Resolved, Error> {
    let (url, rev) = parse_git_source(source)
        .ok_or_else(|| Error::Invalid(format!("not a git source: {source}")))?;
    let scratch = std::env::temp_dir().join(format!(
        "lca-git-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    let done = |result: Result<Resolved, Error>| {
        let _ = std::fs::remove_dir_all(&scratch);
        result
    };
    // A full clone (no `--depth`): a pinned SHA may point anywhere in
    // history, and correctness beats one saved round-trip here.
    if let Err(err) = std::process::Command::new("git")
        .args(["clone", "--quiet", &url, &scratch.display().to_string()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| Error::Fetch(format!("git clone needs the `git` command on PATH: {err}")))
        .and_then(|out| {
            if out.status.success() {
                Ok(out)
            } else {
                Err(Error::Fetch(format!(
                    "git clone failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )))
            }
        })
    {
        return done(Err(err));
    }
    let checkout = rev.as_deref().unwrap_or("HEAD");
    if let Err(err) = git(&["checkout", "--quiet", checkout], &scratch) {
        return done(Err(err));
    }
    let commit = match git(&["rev-parse", "HEAD"], &scratch) {
        Ok(sha) => sha,
        Err(err) => return done(Err(err)),
    };
    let resolved = match resolve_checkout(&scratch) {
        Ok(mut resolved) => {
            resolved.source = source.to_string();
            resolved.commit = commit;
            resolved
        }
        Err(err) => return done(Err(err)),
    };
    done(Ok(resolved))
}

/// Read a git checkout with the archive's shape rules: one top-level
/// `extension.toml`, at most one top-level `*.wasm` (absent means a
/// data-only package, which must declare `resources`), `resources/**`
/// below it. Anything else a repo carries (README, CI configs,
/// `.gitignore`) is ignored - unlike an archive, a checkout is not
/// an exact file list. `.git` itself never leaks into the bag (only
/// files match, and only `resources/` is walked).
fn resolve_checkout(dir: &Path) -> Result<Resolved, Error> {
    let manifest_path = dir.join("extension.toml");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|_| {
        Error::Invalid("the git checkout carries no top-level extension.toml".to_string())
    })?;
    let mut component_path = None;
    let mut entries: Vec<String> = std::fs::read_dir(dir)
        .map_err(|err| Error::Io(err.to_string()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    for name in entries {
        if name == "extension.toml" {
            continue;
        }
        if name.ends_with(".wasm") {
            if component_path.is_some() {
                return Err(Error::Invalid(
                    "the git checkout carries more than one top-level component".to_string(),
                ));
            }
            component_path = Some(dir.join(&name));
        }
        // Anything else (README, configs): a checkout is not an exact
        // file list like an archive is.
    }
    let resources = read_resource_dir(&dir.join("resources"))?;
    match component_path {
        Some(path) => {
            let component = std::fs::read(&path).map_err(|err| Error::Io(err.to_string()))?;
            Ok(Resolved {
                digest: Resolved::digest_of(&component),
                source: String::new(),
                manifest,
                component,
                commit: String::new(),
                resources,
            })
        }
        None if manifest_declares_resources(&manifest) => Ok(Resolved {
            digest: Resolved::digest_of_package(&manifest, &resources),
            source: String::new(),
            manifest,
            component: Vec::new(),
            commit: String::new(),
            resources,
        }),
        None => Err(Error::Invalid(
            "the git checkout carries extension.toml and no component".to_string(),
        )),
    }
}
