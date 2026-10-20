//! Build the embedded model catalog (gh #260).
//!
//! At build time this produces a stripped, gzipped `models.dev` catalog and
//! hands its path to `include_bytes!` through `LCA_MODELS_CATALOG_GZIP`. The
//! data is a build artifact: never committed, never fetched at run time
//! (FR-CFG-3 - the only outbound request the agent makes unasked is the
//! update check).
//!
//! ## Where the bytes come from, highest precedence first
//!
//! 1. `LCA_MODELS_CATALOG=<path>`: a local `api.json`-shaped file, stripped
//!    through `scripts/sync-models-catalog.py`. This is how a release pins
//!    the input.
//! 2. `<workspace>/target/models-catalog.json.gz`: the cached fetch, reused
//!    verbatim. NFR-17 requires a tagged commit to build byte-identically,
//!    and the network is not reproducible - so the first build in a checkout
//!    fetches once and every later build (including the second half of the
//!    pipeline's double build) reuses the same bytes. Delete the file, or set
//!    `LCA_MODELS_CATALOG_REFRESH=1`, to refresh it.
//! 3. The network, through the same script, which then fills the cache.
//! 4. `resources/models_stub.json`, the checked-in fallback. Uncompressed on
//!    purpose: `models::embedded` sniffs the gzip magic, so the build needs no
//!    compression dependency of its own.
//!
//! `LCA_MODELS_CATALOG_OFFLINE=1` skips the network and the cache entirely
//! and goes straight to the stub.
//!
//! Every failure warns and lands on a lower rung. A missing network is never a
//! build failure: `cargo check` and the suite run offline (acceptance 6).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The catalog size ceiling (acceptance 2). A fetch that strips to more than
/// this falls back to the stub rather than growing the binary past NFR-1.
const MAX_GZIP_BYTES: u64 = 200 * 1024;

const SOURCE: &str = "https://models.dev/api.json";

fn main() {
    for key in [
        "LCA_MODELS_CATALOG",
        "LCA_MODELS_CATALOG_OFFLINE",
        "LCA_MODELS_CATALOG_REFRESH",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| PathBuf::from(".").into()),
    );
    let sync = manifest_dir.join("../../scripts/sync-models-catalog.py");
    let stub = manifest_dir.join("resources/models_stub.json");
    let cache = manifest_dir.join("../../target/models-catalog.json.gz");
    println!("cargo:rerun-if-changed={}", sync.display());

    // The artifact path is always OUT_DIR, so a byte-identical catalog still
    // produces a byte-identical `include_bytes!` input.
    let Some(out_dir) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        println!("cargo:warning=OUT_DIR is unset; the model catalog stays on the stub");
        return;
    };
    let artifact = out_dir.join("models_catalog.json.gz");

    if let Some(path) = std::env::var_os("LCA_MODELS_CATALOG").map(PathBuf::from) {
        if strip(&sync, &path, &artifact) && within_ceiling(&artifact) {
            embed(&artifact, "LCA_MODELS_CATALOG");
            return;
        }
        println!(
            "cargo:warning=cannot strip {}; falling back",
            path.display()
        );
    }

    if !offline() {
        if !refresh() && within_ceiling(&cache) {
            embed(&cache, "the cached fetch");
            return;
        }
        if fetch(&sync, &artifact) && within_ceiling(&artifact) {
            // Fill the cache for the next build; a failed copy is harmless.
            if std::fs::copy(&artifact, &cache).is_err() {
                println!(
                    "cargo:warning=cannot cache the catalog at {}; the next build will refetch",
                    cache.display()
                );
            }
            embed(&artifact, "models.dev");
            return;
        }
        println!("cargo:warning=no usable catalog from models.dev; staying on the stub");
    } else {
        println!(
            "cargo:warning=LCA_MODELS_CATALOG_OFFLINE is set; the model catalog stays on the stub"
        );
    }

    if !stub.is_file() {
        println!(
            "cargo:warning={} is missing; the catalog is empty",
            stub.display()
        );
        return;
    }
    embed(&stub, "the stub");
}

fn embed(path: &Path, origin: &str) {
    println!("cargo:rustc-env=LCA_MODELS_CATALOG_GZIP={}", path.display());
    match std::fs::metadata(path) {
        Ok(meta) => println!(
            "cargo:warning=model catalog embedded from {origin} ({} bytes)",
            meta.len()
        ),
        Err(err) => println!("cargo:warning=model catalog from {origin} is unreadable ({err})"),
    }
}

fn within_ceiling(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > 0 && meta.len() <= MAX_GZIP_BYTES => true,
        Ok(meta) => {
            println!(
                "cargo:warning={} is {} bytes, outside the 1..={MAX_GZIP_BYTES} ceiling",
                path.display(),
                meta.len()
            );
            false
        }
        Err(_) => false,
    }
}

fn offline() -> bool {
    flag("LCA_MODELS_CATALOG_OFFLINE")
}

fn refresh() -> bool {
    flag("LCA_MODELS_CATALOG_REFRESH")
}

fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        !matches!(value.as_str(), "" | "0" | "false" | "no")
    })
}

/// Strip a local `api.json`-shaped file into `out`.
fn strip(sync: &Path, input: &Path, out: &Path) -> bool {
    if !input.is_file() {
        println!(
            "cargo:warning=LCA_MODELS_CATALOG={} is not a file",
            input.display()
        );
        return false;
    }
    run(
        sync,
        [
            "--from",
            &input.to_string_lossy(),
            "--out",
            &out.to_string_lossy(),
        ],
    )
}

/// Fetch models.dev into `out` and gzip it.
fn fetch(sync: &Path, out: &Path) -> bool {
    run(sync, ["--source", SOURCE, "--out", &out.to_string_lossy()])
}

fn run<const N: usize>(sync: &Path, args: [&str; N]) -> bool {
    let status = Command::new("python3").arg(sync).args(args).status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            println!("cargo:warning=sync-models-catalog exited with {status}");
            false
        }
        Err(err) => {
            println!("cargo:warning=python3 is unavailable ({err})");
            false
        }
    }
}
