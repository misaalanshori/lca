//! gh #461 - the provider's environment reads must reach the WASM guest.
//!
//! #157 was a workaround, not a fix: `lca-cli` carried `openai_ad_hoc_host()`,
//! a hardcoded special case that existed because a WASM provider had no
//! legitimate way to read one base-URL environment variable. A WASM component is
//! instantiated with an empty `WasiCtx`, so `std::env::var` inside it answers
//! `NotPresent`, while a native-linked extension reads the host environment
//! directly. The same provider therefore worked compiled-in and silently behaved
//! as if nothing were configured as a `.wasm`.
//!
//! #170 built the capability that replaces it: a manifest lists variable
//! **names**, and the host copies the values that are present into the context at
//! load. #157's workaround was then deleted, which is the point where this issue
//! starts - deleting the workaround is only correct if the capability actually
//! took over its job.
//!
//! It had not. The shipped `openai-compatible` manifest declared no
//! `[capabilities.env]` at all, so after the deletion the sandboxed provider
//! still answered *"no API key is configured. Set OPENAI_API_KEY ..."* with the
//! variable set and granted. Measured directly: `with_env_vars({OPENAI_API_KEY:
//! "granted-key"})` against the shipped manifest produced exactly that refusal,
//! and the same fixture with the `[capabilities.env]` block present reports the
//! key instead.
//!
//! So this guard is about the **declaration**, and there are three things it has
//! to keep true:
//!
//! 1. **The names are declared**, so the capability has something to forward.
//! 2. **The list is the minimum set** - the names this crate reads by literal,
//!    each justified in the manifest comment. An `env` grant is a consent screen;
//!    a broad list is a broad disclosure (ADR-0048's rule, and the reason a
//!    wildcard is refused at parse).
//! 3. **The workaround is gone and stays gone.** A grep-class row, so it cannot
//!    quietly return.
//!
//! The parity rows live in `crates/lca-ext-host/tests/provider.rs`
//! (`the_providers_env_reads_reach_the_guest`,
//! `a_granted_env_key_is_read_by_the_guest`); this file pins the manifest shape
//! those rows depend on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

fn provider_manifest() -> String {
    read("extensions/openai-compatible/extension.toml")
}

/// The declared `keys` list, read out of the manifest **text**.
///
/// Deliberately not a TOML parse: this crate has no `toml` dependency and the
/// brief forbids adding one for a test. The shape being read is a single array
/// of quoted names, so a text scan is exact here and needs no parser.
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

// Verifies: gh #461 - **the declaration exists.** Without it the host forwards
// nothing and every `std::env::var` in the guest answers `NotPresent`, which is
// the state the shipped provider was in after #157's workaround was deleted.
#[test]
fn the_provider_declares_the_environment_it_reads() {
    let manifest = provider_manifest();
    let keys = declared_keys(&manifest);
    assert!(
        !keys.is_empty(),
        "the provider reads the environment, so it must declare it: {manifest}"
    );
    // The two that endpoint discovery and authentication actually need.
    for required in ["OPENAI_BASE_URL", "OPENAI_API_KEY"] {
        assert!(
            keys.iter().any(|key| key == required),
            "`{required}` is read by the provider and must be granted: {keys:?}"
        );
    }
}

// Verifies: gh #461 design point 1 - **the minimum set.**
//
// The assertion is bidirectional against the source: every name the crate reads
// by *literal* is declared, and nothing else is. A future broad grant shows up
// here as a diff rather than as a consent screen nobody reads.
//
// The per-preset key names (`GROQ_API_KEY`, `OPENROUTER_API_KEY`, …) are
// deliberately outside this check: they come from the `provider-presets.toml`
// resource at run time, so the manifest cannot enumerate them without either a
// broad grant or a list that goes stale whenever a preset is added. The
// consequence is documented instead (see the next row).
#[test]
fn the_declared_set_is_exactly_the_literals_the_crate_reads() {
    let mut literals: Vec<String> = Vec::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(root.join("extensions/openai-compatible/src")).expect("src dir")
    {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read source");
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for opener in ["env::var(\"", "env_var(\""] {
                let mut rest = trimmed;
                while let Some(index) = rest.find(opener) {
                    rest = &rest[index + opener.len()..];
                    if let Some(end) = rest.find('"') {
                        let name = &rest[..end];
                        if !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                        {
                            literals.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    literals.sort();
    literals.dedup();
    assert!(
        !literals.is_empty(),
        "the scan must find the crate's literal env reads"
    );

    let mut declared = declared_keys(&provider_manifest());
    declared.sort();
    declared.dedup();
    assert_eq!(
        declared, literals,
        "the declared set is exactly the names the crate reads by literal - \
         adding a read means declaring it, and declaring more than is read is a \
         disclosure nobody needs"
    );
}

// Verifies: gh #461 design point 2 - **the consequence of the preset gap is
// documented**, not left for a user to discover.
//
// A named profile's key resolves from its stored credential under the sandbox and
// from the environment only in the native build. That is a real behavioural
// difference between delivery modes, so it belongs in the provider's page rather
// than only in a manifest comment.
#[test]
fn the_sandbox_limitation_on_preset_keys_is_documented() {
    let docs = read("docs/providers/openai-compatible.md");
    assert!(
        docs.contains("capabilities.env"),
        "the provider page names the grant: {docs:.0}"
    );
    let lower = docs.to_lowercase();
    assert!(
        lower.contains("sandbox") && lower.contains("preset"),
        "and states the sandbox's limitation on preset key names"
    );
}

// Verifies: gh #461 - **the workaround is gone and stays gone.**
//
// `openai_ad_hoc_host` and the `api.openai.com` literal were the coupling
// #157 described. `scripts/docs-consistency.sh` greps for the latter in
// `lca-cli`; this row pins the symbol too, so neither can return unnoticed.
#[test]
fn the_hardcoded_workaround_is_gone() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(root.join("crates/lca-cli/src")).expect("src dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("fn openai_ad_hoc_host"),
            "the workaround must not return: {}",
            path.display()
        );
        assert!(
            !text.contains("api.openai.com"),
            "and neither must the hardcoded host literal: {}",
            path.display()
        );
    }
}

// Verifies: gh #461 - no new dependency; the declaration is manifest data.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("extensions/openai-compatible/Cargo.toml");
    for unwanted in ["dotenv", "envy", "config"] {
        assert!(
            !manifest.contains(&format!("\n{unwanted} =")),
            "the environment is forwarded by the host, not read by a new crate"
        );
    }
}
