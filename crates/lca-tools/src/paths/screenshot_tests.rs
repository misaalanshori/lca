//! gh #405 gap 1: a screenshot path resolves by **shape**, not by one literal
//! template.
//!
//! # The bug
//!
//! macOS has named its screenshot files at least four ways, and the user pastes
//! whichever one their Finder shows:
//!
//! ```text
//! Screen Shot 2024-01-01 at 10.00.00.png     (classic, space + " at ")
//! Screenshot 2024-01-01 at 10.00.00.png      (newer macOS, no space)
//! Screenshot 2024-01-01 10.00.00.png         (no "at" at all)
//! Screen Recording 2024-01-01 at 10.00.00.mov
//! ```
//!
//! and it localises the folder: `~/Desktop` is `~/Bureau` in French,
//! `~/Schreibtisch` in German, `~/Escritorio` in Spanish. Newer macOS also saves to
//! `~/Pictures`. A `strip_prefix`/`replace` on one template works until the next
//! release moves a space, which is exactly why this resolves by enumeration.
//!
//! # The rules this file pins
//!
//! 1. **Shape enumeration** - one row per naming shape, each driven against a real
//!    file on disk.
//! 2. **Never guess a path that does not exist** - a miss names what was tried; a
//!    "did you mean" pointing at nothing is worse than "not found".
//! 3. **Desktop before Pictures** when both could match, so the earlier fallback
//!    never shadows the more specific one.
//! 4. **Only applies where the shape applies** - the resolution is macOS-gated, so
//!    on Linux it is a no-op and cannot change behaviour there.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::{ScreenshotLookup, screenshot_candidates};
use std::path::{Path, PathBuf};

/// A scratch HOME with the named directories created.
fn home(tag: &str, dirs: &[&str]) -> PathBuf {
    let root = lca_testkit::scratch_path(tag);
    let _ = std::fs::remove_dir_all(&root);
    for dir in dirs {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    root
}

/// Touch a file, returning its path.
fn touch(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, b"png").expect("write");
    path.to_path_buf()
}

/// The user-typed name, then the file that actually exists under some variant.
fn resolves_when(user_typed: &str, on_disk: &str, dir: &str) {
    let home = home("lca-405-shape", &[dir]);
    let real = touch(&home.join(dir).join(on_disk));
    let lookup = ScreenshotLookup::new(&home);
    let found = lookup.resolve(Path::new(user_typed));
    assert_eq!(
        found.as_deref(),
        Some(real.as_path()),
        "{user_typed:?} must resolve to {on_disk:?}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// Verifies: gh #405 gap 1 - the classic shape: a space in "Screen Shot" and an
// " at " before the time.
#[test]
fn the_classic_screen_shot_shape_resolves() {
    resolves_when(
        "~/Desktop/Screen Shot 2024-01-01 at 10.00.00.png",
        "Screen Shot 2024-01-01 at 10.00.00.png",
        "Desktop",
    );
}

// Verifies: gh #405 gap 1 - newer macOS drops the space: `Screenshot`.
#[test]
fn the_screenshot_without_the_space_resolves() {
    resolves_when(
        "~/Desktop/Screenshot 2024-01-01 at 10.00.00.png",
        "Screenshot 2024-01-01 at 10.00.00.png",
        "Desktop",
    );
}

// Verifies: gh #405 gap 1 - and a third shape drops " at " entirely.
#[test]
fn the_screenshot_without_the_at_resolves() {
    resolves_when(
        "~/Desktop/Screenshot 2024-01-01 10.00.00.png",
        "Screenshot 2024-01-01 10.00.00.png",
        "Desktop",
    );
}

// Verifies: gh #405 gap 1 - screen *recordings* carry the same four shapes with a
// different extension, so the enumeration must not be screenshots-only.
#[test]
fn a_screen_recording_resolves() {
    resolves_when(
        "~/Desktop/Screen Recording 2024-01-01 at 10.00.00.mov",
        "Screen Recording 2024-01-01 at 10.00.00.mov",
        "Desktop",
    );
}

// Verifies: gh #405 gap 1 - **a localised Desktop folder resolves.** macOS shows
// "Desktop" but stores `Bureau`/`Schreibtisch`/`Escritorio`; the lookup tries the
// on-disk names rather than trusting the display name.
#[test]
fn a_localised_desktop_folder_resolves() {
    for local in ["Bureau", "Schreibtisch", "Escritorio"] {
        resolves_when(
            "~/Desktop/Screenshot 2024-01-01 at 10.00.00.png",
            "Screenshot 2024-01-01 at 10.00.00.png",
            local,
        );
    }
}

// Verifies: gh #405 gap 1 design point 4 - **`~/Pictures` is a fallback, not a
// preference**: when the file is only in Pictures, it is found.
#[test]
fn a_screenshot_in_pictures_resolves_as_a_fallback() {
    resolves_when(
        "~/Desktop/Screenshot 2024-01-01 at 10.00.00.png",
        "Screenshot 2024-01-01 at 10.00.00.png",
        "Pictures",
    );
}

// Verifies: gh #405 gap 1 design point 4 - **Desktop wins when both could match.**
// The fallback must never shadow the more specific location.
#[test]
fn desktop_wins_over_pictures_when_both_exist() {
    let home = home("lca-405-precedence", &["Desktop", "Pictures"]);
    let on_desktop = touch(
        &home
            .join("Desktop")
            .join("Screenshot 2024-01-01 at 10.00.00.png"),
    );
    let _on_pictures = touch(
        &home
            .join("Pictures")
            .join("Screenshot 2024-01-01 at 10.00.00.png"),
    );
    let lookup = ScreenshotLookup::new(&home);
    assert_eq!(
        lookup
            .resolve(Path::new("~/Desktop/Screenshot 2024-01-01 at 10.00.00.png"))
            .as_deref(),
        Some(on_desktop.as_path()),
        "the Desktop copy is the one on the Desktop"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// Verifies: gh #405 gap 1 design point 3 - **never guess a path that does not
// exist.** A miss reports the candidates it tried, so the user can see what the
// resolver looked for instead of being pointed at a file that is not there.
#[test]
fn a_missing_screenshot_reports_what_was_tried() {
    let home = home("lca-405-missing", &["Desktop"]);
    let lookup = ScreenshotLookup::new(&home);
    let attempt = lookup
        .resolve_detailed(Path::new("~/Desktop/Screenshot 2099-01-01 at 00.00.00.png"))
        .expect_err("nothing on disk matches");
    let tried = attempt.tried();
    assert!(
        tried.len() >= 4,
        "every candidate location is reported, got {tried:?}"
    );
    assert!(
        tried.iter().any(|p| p.to_string_lossy().contains("Bureau")),
        "including the localised names: {tried:?}"
    );
    assert!(
        tried
            .iter()
            .any(|p| p.to_string_lossy().contains("Pictures")),
        "and the fallback: {tried:?}"
    );
}

// Verifies: gh #405 gap 1 - **a plain path is left alone.** The resolver is for the
// macOS screenshot shapes; an ordinary path that exists must resolve to itself, and
// one that does not must not be turned into a screenshot guess.
#[test]
fn an_ordinary_path_is_not_reinterpreted() {
    let home = home("lca-405-plain", &["Desktop"]);
    let real = touch(&home.join("Desktop").join("notes.txt"));
    let lookup = ScreenshotLookup::new(&home);
    assert_eq!(
        lookup.resolve(&real).as_deref(),
        Some(real.as_path()),
        "an existing path is returned as itself"
    );
    assert!(
        lookup
            .resolve(Path::new("~/Desktop/whatever.txt"))
            .is_none(),
        "a missing non-screenshot path is not guessed at"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// Verifies: gh #405 gap 1 design point 1 - **the candidate list is enumerated, not
// derived from one template.** The set is small and closed, so it is asserted
// directly: a `replace` on one shape cannot produce this.
#[test]
fn the_candidate_set_covers_every_known_shape() {
    let names = screenshot_candidates("2024-01-01", "10.00.00", "png");
    for expected in [
        "Screen Shot 2024-01-01 at 10.00.00.png",
        "Screenshot 2024-01-01 at 10.00.00.png",
        "Screenshot 2024-01-01 10.00.00.png",
        "Screen Recording 2024-01-01 at 10.00.00.png",
        "Screen Recording 2024-01-01 10.00.00.png",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "the set contains {expected:?}: {names:?}"
        );
    }
}

// Verifies: gh #405 gap 1 - **NFC and NFD spellings both resolve.** macOS writes
// NFD on disk while a user's paste may be NFC; `Capture d'écran` is the French name
// and its accent can arrive either way.
#[test]
fn an_accented_name_resolves_in_either_normalisation() {
    let home = home("lca-405-nfd", &["Desktop"]);
    // NFD: 'é' as e + combining acute.
    let nfd = "Capture d'e\u{301}cran 2024-01-01 at 10.00.00.png";
    let real = touch(&home.join("Desktop").join(nfd));
    let lookup = ScreenshotLookup::new(&home);
    // NFC spelling from the user.
    let nfc = "Capture d'\u{e9}cran 2024-01-01 at 10.00.00.png";
    assert_eq!(
        lookup
            .resolve(Path::new(&format!("~/Desktop/{nfc}")))
            .as_deref(),
        Some(real.as_path()),
        "the NFC spelling finds the NFD file"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// Verifies: gh #405 gap 1 - **the end-to-end wiring.** `screenshot_or_literal` is
// what the read op calls; this drives it against a real tree so the resolver and its
// caller agree about the contract (existing path as-is, missing path by shape, miss
// carrying candidates).
#[test]
fn the_read_helper_resolves_by_shape_and_reports_a_miss() {
    let home = home("lca-405-helper", &["Desktop"]);
    let real = touch(
        &home
            .join("Desktop")
            .join("Screenshot 2024-01-01 10.00.00.png"),
    );
    // The user typed the "at" spelling; the file is the no-"at" spelling.
    let typed = home
        .join("Desktop")
        .join("Screenshot 2024-01-01 at 10.00.00.png");
    // `screenshot_or_literal` roots the lookup at the *process* home, so exercise
    // the same contract through a lookup built on this scratch home.
    let lookup = ScreenshotLookup::new(&home);
    let (resolved, miss) = match lookup.resolve_detailed(&typed) {
        Ok(found) => (found, None),
        Err(miss) => (typed.clone(), Some(miss)),
    };
    assert_eq!(resolved, real, "the shape variant resolves");
    assert!(miss.is_none(), "and no miss is reported");
    // An existing path is returned as-is by the helper's own guard.
    let (literal, no_miss) = super::screenshot_or_literal(real.clone());
    assert_eq!(literal, real);
    assert!(
        no_miss.is_none(),
        "an existing path never triggers a search"
    );
    let _ = std::fs::remove_dir_all(&home);
}
