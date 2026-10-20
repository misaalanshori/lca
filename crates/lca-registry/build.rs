//! Build the embedded model catalog (gh #260).
//!
//! At build time this produces a stripped, gzipped `models.dev` catalog and
//! hands its path to `include_bytes!` through `LCA_MODELS_CATALOG_GZIP`. The
//! data is a build artifact: never committed, never fetched at run time
//! (FR-CFG-3 - the only outbound request the agent makes unasked is the
//! update check).
//!
//! The decision logic lives in `build_support.rs`, which the test suite
//! `include!`s too, so "a CI build never fetches" is a function with an
//! assertion rather than a line nothing can call.
//!
//! ## Where the bytes come from, highest precedence first
//!
//! 1. `LCA_MODELS_CATALOG=<path>`: a local `api.json`-shaped file, stripped
//!    through `scripts/sync-models-catalog.py`.
//! 2. **Nothing, when the build is in CI** (`CI` or `GITHUB_ACTIONS` set):
//!    the checked-in `resources/models_stub.json`, with a warning that names
//!    the coverage cost. A release build must be a function of its commit,
//!    and the network is not.
//! 3. `$CARGO_TARGET_DIR/models-catalog.json.gz` (else `<workspace>/target/`):
//!    the cached development fetch, reused verbatim.
//! 4. The network, through the same script, which then fills that cache.
//! 5. `resources/models_stub.json`.
//!
//! ## Why the CI arm exists (NFR-17)
//!
//! `.github/workflows/publish.yml`'s reproducible double build runs two
//! independent `CARGO_TARGET_DIR`s (`target/repro-a`, `target/repro-b`) on one
//! fresh checkout and compares the two binary hashes. Neither sees a cache, so
//! without this arm both would fetch `models.dev` across a network boundary,
//! and any upstream or CDN difference between the two responses would change a
//! compressed blob inside the binary - failing the gate for a reason that has
//! nothing to do with the commit. Under CI the catalog is therefore the stub,
//! deterministically.
//!
//! `LCA_MODELS_CATALOG=<path>` remains the way a release that wants the full
//! catalog gets it: pinned input, same bytes both times. The file may be the
//! raw `api.json` or the already-stripped gzip, so a release can commit the
//! ~134 KiB artifact and pin it, and no step of its build touches a network.
//!
//! ## The developer path
//!
//! A local, non-CI build still fetches once and caches. `LCA_MODELS_CATALOG_OFFLINE=1`
//! skips the network and the cache; `LCA_MODELS_CATALOG_REFRESH=1` refetches
//! over the cached copy.
//!
//! Every failure warns and lands on a lower rung. A missing network is never a
//! build failure: `cargo check` and the suite run offline (acceptance 6).

use std::process::Command;

include!("build_support.rs");

fn main() {
    for key in [
        "LCA_MODELS_CATALOG",
        "LCA_MODELS_CATALOG_OFFLINE",
        "LCA_MODELS_CATALOG_REFRESH",
        "CARGO_TARGET_DIR",
        "CI",
        "GITHUB_ACTIONS",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| PathBuf::from(".").into()),
    );
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.clone());
    let sync = workspace.join("scripts/sync-models-catalog.py");
    let stub = manifest_dir.join("resources/models_stub.json");
    let cache = cache_path(std::env::var_os("CARGO_TARGET_DIR").as_deref(), &workspace);
    println!("cargo:rerun-if-changed={}", sync.display());

    // The artifact path is always OUT_DIR, so a byte-identical catalog still
    // produces a byte-identical `include_bytes!` input.
    let Some(out_dir) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        println!("cargo:warning=OUT_DIR is unset; the model catalog stays on the stub");
        return;
    };
    let artifact = out_dir.join("models_catalog.json.gz");

    let ci = in_ci(
        std::env::var_os("CI").as_deref(),
        std::env::var_os("GITHUB_ACTIONS").as_deref(),
    );
    let offline = flag(std::env::var_os("LCA_MODELS_CATALOG_OFFLINE").as_deref());
    let refresh = flag(std::env::var_os("LCA_MODELS_CATALOG_REFRESH").as_deref());

    // A pinned input the build cannot use is a warning, not a silent downgrade
    // to the stub: a release that pinned something needs to know.
    let pinned_path = std::env::var_os("LCA_MODELS_CATALOG").map(PathBuf::from);
    let pinned = match &pinned_path {
        Some(path) => strip(&sync, path, &artifact) && within_ceiling(&artifact),
        None => false,
    };
    if pinned_path.is_some() && !pinned {
        println!("cargo:warning=cannot use LCA_MODELS_CATALOG; falling through to the next source");
    }

    let origin = choose(pinned, offline, ci, refresh, within_ceiling(&cache));
    println!("cargo:warning={}", warning(origin, ci));

    let embed_from = match origin {
        Origin::Pinned => Some(artifact.clone()),
        Origin::Stub => Some(stub.clone()),
        Origin::Cache => Some(cache.clone()),
        Origin::Network => fetch(&sync, &artifact).then(|| {
            if std::fs::copy(&artifact, &cache).is_err() {
                println!(
                    "cargo:warning=cannot cache the catalog at {}; the next build will refetch",
                    cache.display()
                );
            }
            artifact.clone()
        }),
    };

    // A rung that produced nothing usable falls to the stub, and says so.
    let embed_from = match embed_from.filter(|path| path.is_file()) {
        Some(path) => path,
        None => {
            if origin != Origin::Stub {
                println!(
                    "cargo:warning=no usable catalog; the model catalog stays on the checked-in stub"
                );
            }
            stub.clone()
        }
    };
    println!(
        "cargo:rustc-env=LCA_MODELS_CATALOG_GZIP={}",
        embed_from.display()
    );
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
