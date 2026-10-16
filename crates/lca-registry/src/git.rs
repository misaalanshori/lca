//! Git package sources (gh #59, ADR-0010's fourth kind): a `git+<url>`
//! reference clones into a scratch dir, pins the commit, and resolves
//! the checkout with the same shape rules as an archive (one top-level
//! `extension.toml`, at most one top-level `*.wasm`, `resources/**`).
//! The lockfile keeps the typed ref (branches advance on update) and
//! the pin separately, mirroring OCI's ref-plus-digest split.
//!
//! The `git` CLI does the transport (no new dependency); tests use
//! local-path repos, so they run fully offline.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Error, Resolved, manifest_declares_resources, read_resource_dir};

/// The git-subprocess ceiling: a clone/checkout that has not finished by
/// then is killed, so a stalled transport cannot wedge `lca ext install`
/// forever. Named (not inlined) so the guard test and the docs agree.
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Split a `git+...` source into its URL and optional `#rev`.
/// `None` when the source is not a valid git source: not `git+`-prefixed,
/// an empty url, or a url/rev git would read as an option (gh #349).
pub fn parse_git_source(source: &str) -> Option<(String, Option<String>)> {
    let rest = source.strip_prefix("git+")?;
    if rest.is_empty() {
        return None;
    }
    let (url, rev) = match rest.rsplit_once('#') {
        Some((url, rev)) if !url.is_empty() && !rev.is_empty() => (url, Some(rev)),
        _ => (rest, None),
    };
    // gh #349: a url or rev that git reads as an option (`-`-prefixed
    // after trimming) or is empty never reaches git as a positional. The
    // clone also passes `--` (see `resolve_git`); this validation is the
    // gate that covers checkout, where `--` cannot be used.
    if !is_usable_git_value(url) || rev.is_some_and(|rev| !is_usable_git_value(rev)) {
        return None;
    }
    Some((url.to_string(), rev.map(str::to_string)))
}

/// Whether a url/rev is safe to hand to git as a positional argument: not
/// empty, and not option-shaped. `-`-prefixed after trimming because git
/// strips nothing before deciding "is this a flag".
fn is_usable_git_value(value: &str) -> bool {
    !value.is_empty() && !value.trim_start().starts_with('-')
}

/// Run `git` with `args`, in `dir` when given (the clone inherits the
/// process cwd; the in-scratch commands run in the checkout), returning
/// stdout on success.
///
/// Two properties are load-bearing here (gh #349):
///
/// * **The pipes are drained while the child runs.** A reader thread per
///   pipe is spawned before the wait loop, because a child that writes
///   more than the OS pipe buffer (~64 KiB) blocks in `write()` until
///   someone reads; a poll-then-read shape would deadlock a perfectly
///   healthy git. `Command::output` happens to do this, but the timeout
///   below needs `spawn`, so the drain is explicit.
/// * **A stalled transport cannot wedge the install forever.** The wait
///   loop enforces [`GIT_TIMEOUT`] and kills the child on expiry. This is
///   the one behavior the old `.output()` path lacked.
///
/// ponytail: on the timeout path the reader threads are detached, not
/// joined — a killed git can leave a remote helper holding the pipe, and
/// blocking the install on that would defeat the timeout. A leaked
/// short-lived reader thread is the cheaper failure for a one-shot CLI.
fn git(args: &[&str], dir: Option<&Path>) -> Result<String, Error> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The clone inherits the process cwd (a relative local-path url is
    // resolved against it); the in-scratch commands run there.
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|err| Error::Fetch(format!("git needs the `git` command on PATH: {err}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Drain both pipes on their own threads (see the doc comment).
    let out_thread = std::thread::spawn(move || read_all(stdout));
    let err_thread = std::thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let Some(status) = status else {
        return Err(Error::Fetch(format!(
            "git {} timed out after {}s and was stopped",
            args.join(" "),
            GIT_TIMEOUT.as_secs()
        )));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(Error::Fetch(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

/// Read a child pipe to EOF (or empty when it was not piped).
fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buffer);
    }
    buffer
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
    //
    // `--` before the positionals: the belt next to the `parse_git_source`
    // braces (gh #349), so a url we failed to anticipate still cannot be
    // read as a clone option.
    if let Err(err) = git(
        &[
            "clone",
            "--quiet",
            "--",
            &url,
            &scratch.display().to_string(),
        ],
        None,
    ) {
        return done(Err(err));
    }
    let checkout = rev.as_deref().unwrap_or("HEAD");
    // No `--` here on purpose: `git checkout -- <rev>` reads `<rev>` as a
    // pathspec (the path-restore form), so `git checkout --quiet -- v1.0`
    // would fail on every ordinary rev. The `-`-reject in
    // `parse_git_source` is what makes this position safe.
    if let Err(err) = git(&["checkout", "--quiet", checkout], Some(&scratch)) {
        return done(Err(err));
    }
    let commit = match git(&["rev-parse", "HEAD"], Some(&scratch)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // gh #349: the parse gate rejects anything git would read as an
    // option (or an empty positional), and keeps the ordinary grammar.
    #[test]
    fn parse_git_source_rejects_option_shaped_and_empty_values() {
        // Option-injected url: never a valid source.
        for source in [
            "git+--upload-pack=touch /tmp/x",
            "git+-c",
            "git+ -u", // leading space before the dash
            "git+https://h/r.git#--force",
            "git+https://h/r.git#-b",
            "git+https://h/r.git# -x",
        ] {
            assert!(
                parse_git_source(source).is_none(),
                "{source:?} must be refused"
            );
        }
        assert!(
            parse_git_source("git+").is_none(),
            "an empty url is refused"
        );
    }

    #[test]
    fn parse_git_source_accepts_ordinary_sources() {
        assert_eq!(
            parse_git_source("git+https://host/r.git"),
            Some(("https://host/r.git".to_string(), None))
        );
        assert_eq!(
            parse_git_source("git+https://host/r.git#v1"),
            Some(("https://host/r.git".to_string(), Some("v1".to_string())))
        );
        assert_eq!(
            parse_git_source("git+/tmp/scratch/repo"),
            Some(("/tmp/scratch/repo".to_string(), None))
        );
        assert_eq!(parse_git_source("not-git"), None);
    }

    // gh #349 AC2: a git command emitting more than the ~64 KiB pipe
    // buffer completes instead of deadlocking. `git log -p` over a blob
    // larger than the buffer is the cheap real invocation.
    #[test]
    fn a_git_command_larger_than_the_pipe_buffer_completes() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skip: git is not installed");
            return;
        }
        let root = lca_testkit::scratch_path("lca-registry-git-bigout");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        };
        run(&["init", "-q"]);
        // ~256 KiB of incompressible-enough text: four times the buffer.
        let big = "0123456789abcdef".repeat(16 * 1024);
        std::fs::write(root.join("big.txt"), &big).expect("write");
        run(&["add", "."]);
        run(&["commit", "-qm", "big"]);
        // `git show` streams the blob; the old poll-then-read shape would
        // block past the buffer. The new runner must return it whole.
        let shown =
            git(&["show", "HEAD:big.txt"], Some(&root)).expect("no timeout, output returned");
        assert!(shown.len() >= big.len(), "the full stream came back");
        let _ = std::fs::remove_dir_all(&root);
    }
}
