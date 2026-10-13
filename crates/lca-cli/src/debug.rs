//! Debug tracing setup (gh #258 §5): `--debug` / `--debug-wire`
//! plus `LCA_LOG` route the instrumented domains to the two rotating
//! files. Stable builds omit the feature: the flags then name the
//! missing feature instead of silently doing nothing.

use std::path::Path;

/// Start debug tracing when asked. Returns whether tracing is live;
/// a setup failure (or flags on a feature-less build) is a message
/// for the caller to warn with, never fatal.
pub fn init_debug_trace(debug: bool, wire: bool, data_dir: &Path) -> Result<bool, String> {
    if !debug && !wire {
        return Ok(false);
    }
    init_inner(debug, wire, data_dir)
}

#[cfg(feature = "debug-trace")]
fn init_inner(debug: bool, wire: bool, data_dir: &Path) -> Result<bool, String> {
    let filter = std::env::var("LCA_LOG").ok().or_else(|| {
        // Flag-derived scoping (the subscriber holds the per-file
        // split under an explicit filter too): debug-only quiets the
        // wire file, wire-only quiets the internal file, both keeps
        // the defaults.
        match (debug, wire) {
            (true, true) => None,
            (true, false) => Some(lca_debug::INTERNAL_FILTER.to_string()),
            (false, true) => Some(lca_debug::WIRE_FILTER.to_string()),
            (false, false) => None,
        }
    });
    let config = lca_debug::DebugConfig {
        paths: lca_debug::DebugPaths::in_dir(&data_dir.join("logs")),
        filter,
    };
    lca_debug::init(&config).map_err(|err| err.to_string())?;
    Ok(true)
}

#[cfg(not(feature = "debug-trace"))]
fn init_inner(_debug: bool, _wire: bool, _data_dir: &Path) -> Result<bool, String> {
    Err(
        "debug tracing is not in this build (stable builds omit the debug-trace feature)"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #258 - silence by default (no flags, no tracing).
    #[test]
    fn silence_by_default() {
        let root = lca_testkit::scratch_path("debug-wire-off");
        assert!(
            !init_debug_trace(false, false, &root).expect("never fails"),
            "no flags, no tracing"
        );
    }

    // Verifies: gh #258 - on a feature-less build the flags name the
    // missing feature instead of silently doing nothing.
    #[cfg(not(feature = "debug-trace"))]
    #[test]
    fn flags_without_the_feature_say_so() {
        let root = lca_testkit::scratch_path("debug-wire-missing");
        let err = init_debug_trace(true, false, &root).expect_err("unsupported");
        assert!(err.contains("debug-trace"), "{err}");
    }

    // Verifies: gh #258 - with the feature, `--debug` goes live and
    // the files appear on first emission.
    #[cfg(feature = "debug-trace")]
    #[test]
    fn debug_flag_goes_live() {
        let root = lca_testkit::scratch_path("debug-wire-live");
        // A global subscriber per process: this is the only test here
        // that installs one.
        assert!(init_debug_trace(true, true, &root).expect("init"));
        lca_debug::core::event("turn.start", "id=test");
        let log = root.join("logs").join("debug.log");
        assert!(log.is_file(), "internal file appears on emission");
    }
}
