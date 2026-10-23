//! gh #484 - every extension that reads the environment declares what it reads.
//!
//! # The class, and why this row is the deliverable
//!
//! #461 found `openai-compatible` reading eight environment variables and
//! declaring **none** under `[capabilities.env]`. A WASM component is instantiated
//! with an empty `WasiCtx`, so `std::env::var` inside it answers `NotPresent`
//! unless the manifest declares the name and the host forwards the value at load
//! (#170). So the `.wasm` build behaved as if nothing were configured while the
//! compiled-in one read the host directly - a divergence invisible on either side.
//!
//! The audit that found that one found the same shape in four more manifests
//! (`anthropic`, `antigravity`, `llama`, `openrouter`). Auditing by hand is how
//! *this* issue came to exist, so the guard here is not "these four are fixed" - it
//! is **every** `extensions/*/extension.toml`, checked against that extension's own
//! source. A fifth instance cannot ship without turning this red.
//!
//! # The distinction the first audit missed
//!
//! Not every env read needs a grant. A read inside
//! `#[cfg(not(target_arch = "wasm32"))]` is *native-only by construction*: the
//! sandboxed build never executes it, so a grant would forward a secret the guest
//! has no code to read. Those four providers are all in that shape, and their
//! manifests say so deliberately - `anthropic`'s source puts it plainly: *"Native
//! only reads the environment - the sandbox must not see the host's."*
//!
//! So "the literal reads must be declared" is too blunt: it would force a broad
//! disclosure (`ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, ...) into manifests whose
//! guests deliberately cannot use them. The rule this row enforces is the honest
//! one:
//!
//! **an env read that the `.wasm` build can execute must be declared.**
//!
//! A read that cannot run on `wasm32` is documented instead (see each provider's
//! page), and this row records that it is gated rather than quietly ignoring it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Every extension directory that has both a manifest and a `src/` tree.
fn extension_names() -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("extensions");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("extensions dir")
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_dir() && path.join("extension.toml").is_file() && path.join("src").is_dir()
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The declared `keys` list, read out of a manifest's text.
///
/// A text scan rather than a TOML parse: `lca-regressions` has no `toml`
/// dependency and the shape being read is one array of quoted names.
fn declared_keys(manifest: &str) -> Vec<String> {
    const MARKER: &str = "keys = [";
    let Some(start) = manifest.find(MARKER) else {
        return Vec::new();
    };
    let rest = &manifest[start + MARKER.len()..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };
    rest[..end]
        .split(',')
        .filter_map(|item| {
            let item = item.trim().trim_matches('"').trim();
            (!item.is_empty() && item.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
                .then(|| item.to_string())
        })
        .collect()
}

/// One env read found in an extension's source: the name, and whether the
/// **sandboxed** build can execute it.
#[derive(Debug, PartialEq, Eq)]
struct EnvRead {
    name: String,
    /// `true` when the read can run on `wasm32`, i.e. when it needs a grant.
    reaches_the_guest: bool,
}

/// Every literal env read in one extension's `src/`, with its cfg gating.
///
/// Gating is read by looking **upward** from the read to the enclosing `cfg`
/// attribute, which is how this crate writes it everywhere: an attribute on the
/// block or the statement. A read under `#[cfg(not(target_arch = "wasm32"))]` is
/// native-only; under `#[cfg(target_arch = "wasm32")]` it is guest-only and needs
/// a grant; with no attribute it runs in both and needs a grant.
fn env_reads(provider: &str) -> Vec<EnvRead> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("extensions").join(provider).join("src");
    let mut found: Vec<EnvRead> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![src];
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // A simple line scan with a short memory of recent cfg attributes. The
        // crate's style puts the attribute on its own line or inline, so two
        // lines of context is enough and avoids needing a parser here.
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some(name) = literal_env_name(line) else {
                continue;
            };
            // Look back for the nearest `cfg(...)` attribute that is not closed on
            // its own line by a completed block - the crate writes them directly
            // above the item or as a block wrapper.
            let mut gating: Option<&str> = None;
            for back in (index.saturating_sub(6)..index).rev() {
                let candidate = lines[back];
                if candidate.contains("target_arch = \"wasm32\"") {
                    gating = Some(candidate);
                    break;
                }
                // A blank line or an unrelated statement ends the search: the
                // attribute applies to what immediately follows it.
                if candidate.trim().is_empty() && back + 2 < index {
                    break;
                }
            }
            let native_only = gating
                .map(|gate| gate.contains("not(target_arch"))
                .unwrap_or(false);
            found.push(EnvRead {
                name,
                reaches_the_guest: !native_only,
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup();
    found
}

/// The env variable name read on this line, if any.
fn literal_env_name(line: &str) -> Option<String> {
    for opener in ["env::var(\"", "env_var(\""] {
        if let Some(index) = line.find(opener) {
            let rest = &line[index + opener.len()..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

// Verifies: gh #484 - **the deliverable.** Every env read a `.wasm` build can
// execute is declared in that extension's manifest.
//
// Reading the source rather than a checklist is the point: this row would have
// caught `openai-compatible` before #461 did, and it catches the fifth instance
// the day it is written.
#[test]
fn every_guest_visible_env_read_is_declared() {
    let mut failures: Vec<String> = Vec::new();
    for provider in extension_names() {
        let manifest = read(&format!("extensions/{provider}/extension.toml"));
        let declared = declared_keys(&manifest);
        for read in env_reads(&provider) {
            if read.reaches_the_guest && !declared.contains(&read.name) {
                failures.push(format!(
                    "`{provider}` reads `{}` in a build the sandbox runs, but its \
                     extension.toml declares no `[capabilities.env]` entry for it - \
                     the `.wasm` build will see nothing (gh #484)",
                    read.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "an env read the guest can reach must be declared:\n  {}",
        failures.join("\n  ")
    );
}

// Verifies: gh #484 - **declaring more than is read is also a defect.**
//
// An `env` grant is a consent screen that names every variable; a name nobody
// reads is a disclosure with no purpose. This is the other half of the audit, and
// the reason `openai-compatible`'s own row pairs a "declared" check with an
// "exactly" check.
//
// **One legitimate exception: a probe that takes the name as an argument.** The
// `conformance` extension exists to exercise the *import* rather than any
// particular variable - it calls `cap.env_get(name)` with the name supplied by
// whoever is driving it, so its manifest's `keys` list is the only place the names
// exist in that crate. That shape is detected by the source calling an
// `env_get`-style function rather than `env::var`, and it is skipped here because
// the previous row would already have nothing to compare against.
#[test]
fn no_manifest_declares_a_name_its_source_never_reads() {
    let mut failures: Vec<String> = Vec::new();
    for provider in extension_names() {
        let manifest = read(&format!("extensions/{provider}/extension.toml"));
        let declared = declared_keys(&manifest);
        if declared.is_empty() || takes_the_name_as_an_argument(&provider) {
            continue;
        }
        let reads: Vec<String> = env_reads(&provider).into_iter().map(|r| r.name).collect();
        for name in declared {
            if !reads.contains(&name) {
                failures.push(format!(
                    "`{provider}` declares `{name}` but no source in that crate reads \
                     it - a grant with no reader is a disclosure with no purpose"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a declared name must be read:\n  {}",
        failures.join("\n  ")
    );
}

/// Whether an extension reads the environment through a call that takes the
/// **name as an argument** (`cap.env_get(name)`), which is the probe shape: the
/// manifest is then the only place its variable names appear.
fn takes_the_name_as_an_argument(provider: &str) -> bool {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("extensions").join(provider).join("src");
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.lines().any(|line| {
                let line = line.trim_start();
                !line.starts_with("//")
                    && (line.contains(".env_get(") || line.contains("env_get(&"))
            }) {
                return true;
            }
        }
    }
    false
}

// Verifies: gh #484 - **a provider that reads nothing declares no `env` table.**
//
// A blanket `[capabilities.env] keys = []` is noise: it says "this extension
// wants environment access" on an install consent screen and then names nothing,
// which is worse than silence.
#[test]
fn a_provider_that_reads_nothing_declares_nothing() {
    for provider in extension_names() {
        let manifest = read(&format!("extensions/{provider}/extension.toml"));
        // A probe that takes the name as an argument declares its variables in the
        // manifest and nowhere else, so it is not "reads nothing".
        if env_reads(&provider).is_empty() && !takes_the_name_as_an_argument(&provider) {
            assert!(
                !manifest.contains("[capabilities.env]"),
                "`{provider}` reads no environment variable, so it must not declare \
                 the capability at all"
            );
        }
    }
}

// Verifies: gh #484 - **the native-only reads are gated, not granted.**
//
// This is the distinction the first audit table missed. `anthropic`,
// `antigravity`, `llama` and `openrouter` read their variables inside
// `#[cfg(not(target_arch = "wasm32"))]`, so the sandboxed build cannot execute
// them; a grant would forward a secret the guest has no code to read, which is a
// disclosure with no reader (the boundary the previous row already calls a
// defect).
//
// The row records the shape so a reader can see *why* those four manifests carry
// no `[capabilities.env]` block - and fails if one of them ever moves a read out
// from under the gate without adding the declaration.
#[test]
fn native_only_reads_are_gated_so_a_grant_would_have_no_reader() {
    let mut native_only: Vec<(&str, Vec<String>)> = Vec::new();
    for provider in extension_names() {
        let manifest = read(&format!("extensions/{provider}/extension.toml"));
        let declared = declared_keys(&manifest);
        let reads = env_reads(&provider);
        let gated: Vec<String> = reads
            .iter()
            .filter(|read| !read.reaches_the_guest && !declared.contains(&read.name))
            .map(|read| read.name.clone())
            .collect();
        if !gated.is_empty() {
            native_only.push((
                // Leaked for the assertion message only; this is a test.
                Box::leak(provider.clone().into_boxed_str()),
                gated,
            ));
        }
    }
    // The four from gh #484, each with exactly its own pair (or single).
    let expected: [(&str, &[&str]); 4] = [
        ("anthropic", &["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"]),
        (
            "antigravity",
            &["ANTIGRAVITY_CLIENT_ID", "ANTIGRAVITY_CLIENT_SECRET"],
        ),
        ("llama", &["LLAMA_API_KEY", "LLAMA_BASE_URL"]),
        ("openrouter", &["OPENROUTER_API_KEY"]),
    ];
    for (provider, names) in expected {
        let found = native_only
            .iter()
            .find(|(name, _)| *name == provider)
            .map(|(_, names)| names.clone())
            .unwrap_or_default();
        let mut found_sorted = found.clone();
        found_sorted.sort();
        let mut want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        want.sort();
        assert_eq!(
            found_sorted, want,
            "`{provider}` reads these outside the sandbox and declares none: expected \
             the native-only pair, found {found:?} - a read that moved out from under \
             the `cfg` gate needs a declaration instead"
        );
    }
}

// Verifies: gh #484 - no new dependency; the audit is a text scan.
//
// This suite already carries `regex` and `toml_edit` (other guards use them), so
// the check is not "the name is absent from the workspace" but "this file's own
// imports do not reach for one" - a scan over `read_dir` and `lines()` needs no
// crate, and adding one here would be a dependency for a few loops.
#[test]
fn the_audit_needs_no_parser_or_regex_crate() {
    let source = read("tests/regressions/484-every-env-read-is-declared.rs");
    let imports: Vec<&str> = source
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("use "))
        .collect();
    for line in &imports {
        for unwanted in ["regex", "toml::", "walkdir"] {
            assert!(
                !line.contains(unwanted),
                "the audit needs no `{unwanted}`: {line}"
            );
        }
    }
}
