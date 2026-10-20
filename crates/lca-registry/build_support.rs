// The build script's decision logic for the embedded model catalog, factored
// out of `build.rs` so the release pipeline's branches are testable
// (gh #260 fix-up).
//
// `build.rs` `include!`s this file and `tests/models_catalog.rs` `include!`s
// the same file, so the rule "a CI build never fetches" is an ordinary
// function with an assertion rather than a line inside a build script that
// nothing can call.
//
// Only `std` here: this has to compile as a build script and as a test
// module. `//` comments rather than `//!` because `include!` pastes items,
// and an inner doc comment has no module to attach to in both places.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The catalog size ceiling (issue #260 acceptance 2): a gzipped artifact over
/// this falls back rather than growing the binary past NFR-1.
pub const MAX_GZIP_BYTES: u64 = 200 * 1024;

/// Where a development build fetches the catalog from.
pub const SOURCE: &str = "https://models.dev/api.json";

/// The cache file's name inside the target directory.
pub const CACHE_FILE: &str = "models-catalog.json.gz";

/// Where the catalog's bytes come from, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `LCA_MODELS_CATALOG` named a local `api.json`-shaped file. This is how
    /// a release that wants the full catalog pins its input.
    Pinned,
    /// The cached development fetch inside the target directory.
    Cache,
    /// A fresh network fetch. Development builds only: see [`choose`].
    Network,
    /// The checked-in `resources/models_stub.json`.
    Stub,
}

/// Read an environment value as an affirmative flag.
///
/// Unset, empty, `0`, `false` and `no` are off; anything else is on. Used for
/// every switch here, so `CI=1`, `CI=true` and `GITHUB_ACTIONS=true` all mean
/// the same thing and `CI=false` means what it says.
pub fn flag(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        !matches!(value.as_str(), "" | "0" | "false" | "no")
    })
}

/// Whether this build is running in CI.
///
/// A CI or release build must never fetch the catalog. The pipeline's
/// reproducible double build (NFR-17, `.github/workflows/publish.yml`) runs
/// two independent `CARGO_TARGET_DIR`s on one fresh checkout and compares the
/// two binary hashes; if each build fetched `models.dev` over the network,
/// any upstream or CDN difference between the two fetches would change a
/// compressed blob inside a 24 MB binary and fail the gate for a reason that
/// has nothing to do with the commit.
pub fn in_ci(ci: Option<&OsStr>, github_actions: Option<&OsStr>) -> bool {
    flag(ci) || flag(github_actions)
}

/// Where a development build caches the fetched catalog.
///
/// `CARGO_TARGET_DIR` when it is set, else `<workspace>/target`. A relative
/// `CARGO_TARGET_DIR` resolves against the workspace root, which is where
/// cargo is invoked from in this repository and in the pipeline (its value is
/// literally `target/repro-a` there).
///
/// This follows the variable rather than hard-coding `<workspace>/target`
/// because a build that sets `CARGO_TARGET_DIR` puts all of its state
/// somewhere else: a cache left behind in `target/` would be invisible to the
/// very invocation that set it, so every such build would refetch.
pub fn cache_path(target_dir: Option<&OsStr>, workspace: &Path) -> PathBuf {
    let base = match target_dir {
        Some(dir) if !dir.is_empty() => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                workspace.join(dir)
            }
        }
        _ => workspace.join("target"),
    };
    base.join(CACHE_FILE)
}

/// Pick the source, in precedence order.
///
/// `pinned` is "the `LCA_MODELS_CATALOG` variable named a file"; the caller
/// tries that first and comes back here with `false` if the strip failed.
/// `cache_ok` is "a cached artifact exists and is inside the size ceiling".
///
/// Two rules carry NFR-17:
///
/// * `offline` or `ci` resolves to [`Origin::Stub`] and nothing else, even
///   when a usable cache is sitting right there. A cached artifact is
///   per-checkout state; a fresh CI checkout has none, so a rule that trusted
///   it would behave differently on the pipeline than on a developer's
///   machine. Making CI *deterministically* the stub is what makes the two
///   halves of the double build identical.
/// * `refresh` never reaches the network in CI, because the CI arm returns
///   before `refresh` is read.
pub fn choose(pinned: bool, offline: bool, ci: bool, refresh: bool, cache_ok: bool) -> Origin {
    if pinned {
        return Origin::Pinned;
    }
    if offline || ci {
        return Origin::Stub;
    }
    if cache_ok && !refresh {
        return Origin::Cache;
    }
    Origin::Network
}

/// The one `cargo:warning` line naming what a build embedded.
pub fn warning(origin: Origin, ci: bool) -> String {
    match origin {
        Origin::Pinned => "model catalog embedded from LCA_MODELS_CATALOG".to_string(),
        Origin::Cache => "model catalog embedded from the cached fetch".to_string(),
        Origin::Network => "model catalog fetched from models.dev and cached".to_string(),
        Origin::Stub if ci => {
            "CI build: the model catalog is the checked-in stub, not models.dev \
             (a release build never fetches; see NFR-17). Priced coverage is the \
             stub's 13 models across 5 providers. Set LCA_MODELS_CATALOG to pin \
             the full catalog."
                .to_string()
        }
        Origin::Stub => "model catalog embedded from the checked-in stub".to_string(),
    }
}
