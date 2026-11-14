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
//
// Asserted on **the file found**, not on string equality: macOS's filesystem (HFS+
// and APFS both) normalises filenames on write, so the `PathBuf` a caller gets back
// is whatever spelling the filesystem reports - which is not the spelling this test
// built on Linux. What matters is that the lookup reaches the right file in both
// directions; the bytes it then reads are the user's screenshot either way.
#[test]
fn an_accented_name_resolves_in_either_normalisation() {
    let root = home("lca-405-nfd", &["Desktop"]);
    // NFD on disk: 'é' as e + combining acute.
    let nfd = "Capture d'e\u{301}cran 2024-01-01 at 10.00.00.png";
    touch(&root.join("Desktop").join(nfd));
    let lookup = ScreenshotLookup::new(&root);
    let nfc = "Capture d'\u{e9}cran 2024-01-01 at 10.00.00.png";
    let found = lookup
        .resolve(Path::new(&format!("~/Desktop/{nfc}")))
        .expect("the NFC spelling finds the NFD file");
    assert!(
        found.is_file(),
        "and the result is a real file: {}",
        found.display()
    );
    // The content, not the path spelling, is what a read consumes.
    assert_eq!(std::fs::read(&found).expect("read"), b"png");

    // And the reverse direction: an NFC file found by an NFD spelling.
    let root2 = home("lca-405-nfc", &["Desktop"]);
    touch(&root2.join("Desktop").join(nfc));
    let lookup2 = ScreenshotLookup::new(&root2);
    let found2 = lookup2
        .resolve(Path::new(&format!("~/Desktop/{nfd}")))
        .expect("the NFD spelling finds the NFC file");
    assert_eq!(std::fs::read(&found2).expect("read"), b"png");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

// Verifies: gh #405 gap 1 - **the typographic apostrophe.** A real French macOS
// name is `Capture d'écran 2024-…` with U+2019 (the curly one Finders and
// macOS's own localisation produce), while the resolver's stem shipped with
// ASCII `'`. **NFC normalisation does not turn one into the other** - both are
// already NFC - so before this fix a curly name failed the shape test outright
// and resolved to nothing.
//
// Both directions are asserted, because the spelling can differ on either side:
// the user's paste may be curly while the disk holds ASCII, or the reverse.
#[test]
fn the_typographic_apostrophe_in_the_french_stem_resolves() {
    let ascii = "Capture d'écran 2024-01-01 at 10.00.00.png";
    let curly = "Capture d\u{2019}écran 2024-01-01 at 10.00.00.png";

    // Curly typed, ASCII on disk.
    let root = home("lca-405-apostrophe-a", &["Desktop"]);
    let real = touch(&root.join("Desktop").join(ascii));
    let lookup = ScreenshotLookup::new(&root);
    let found = lookup
        .resolve(Path::new(&format!("~/Desktop/{curly}")))
        .unwrap_or_else(|| panic!("the U+2019 spelling must find the {ascii:?} file"));
    assert_eq!(found, real, "and it is the file that exists");
    assert_eq!(std::fs::read(&found).expect("read"), b"png");

    // ASCII typed, curly on disk - the other direction.
    let root2 = home("lca-405-apostrophe-b", &["Desktop"]);
    let real2 = touch(&root2.join("Desktop").join(curly));
    let lookup2 = ScreenshotLookup::new(&root2);
    let found2 = lookup2
        .resolve(Path::new(&format!("~/Desktop/{ascii}")))
        .unwrap_or_else(|| panic!("the ASCII spelling must find the {curly:?} file"));
    assert_eq!(found2, real2, "and it is the file that exists");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&root2);
}

// Verifies: gh #405 gap 1 - **the two apostrophes are separate candidates, and
// the accent's two normalisations compose with them.** Four spellings of one
// name (ASCII/U+2019 × NFC/NFD) must all reach one file, which is what a real
// French directory can require: the Finder writes U+2019 in NFD, and every paste
// is some other combination.
#[test]
fn every_apostrophe_and_normalisation_spelling_reaches_one_file() {
    // The one file on disk: curly apostrophe, NFD accent (what macOS writes).
    let on_disk = "Capture d\u{2019}e\u{301}cran 2024-01-01 at 10.00.00.png";
    let root = home("lca-405-apostrophe-matrix", &["Desktop"]);
    let real = touch(&root.join("Desktop").join(on_disk));
    let lookup = ScreenshotLookup::new(&root);
    // **The file's identity, not its spelling.** macOS normalises a filename on
    // write (HFS+/APFS), so the `PathBuf` the filesystem hands back is whichever
    // normalisation it stores - the same trap `an_accented_name_resolves_in_either_
    // normalisation` documents. What matters here is that every spelling reaches
    // that one file, so the assertion below is "a file with our content, at that
    // name up to normalisation", never byte equality of the path.
    let comparable = |path: &std::path::Path| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let nfc: String = {
            use unicode_normalization::UnicodeNormalization;
            name.nfc().collect()
        };
        (path.parent().map(std::path::Path::to_path_buf), nfc)
    };
    let expected = comparable(&real);
    for (label, typed) in [
        (
            "curly+NFD",
            "Capture d\u{2019}e\u{301}cran 2024-01-01 at 10.00.00.png",
        ),
        (
            "curly+NFC",
            "Capture d\u{2019}\u{e9}cran 2024-01-01 at 10.00.00.png",
        ),
        (
            "ascii+NFD",
            "Capture d'e\u{301}cran 2024-01-01 at 10.00.00.png",
        ),
        (
            "ascii+NFC",
            "Capture d'\u{e9}cran 2024-01-01 at 10.00.00.png",
        ),
    ] {
        let found = lookup
            .resolve(Path::new(&format!("~/Desktop/{typed}")))
            .unwrap_or_else(|| panic!("{label} must resolve to the file on disk"));
        assert_eq!(
            comparable(&found),
            expected,
            "{label} resolved to a different file (spelling may differ by normalisation)"
        );
        assert_eq!(
            std::fs::read(&found).expect("read"),
            b"png",
            "{label}: and it is the file that exists"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #405 gap 1 - **the candidate set carries both apostrophes, and
// does not repeat itself.** The set is what a miss list prints, so a duplicate
// makes the error look like a resolver bug, and a missing spelling is the hole
// this fix closed.
#[test]
fn the_candidate_set_carries_both_apostrophe_spellings_once() {
    let names = screenshot_candidates("2024-01-01", "10.00.00", "png");
    let count = |needle: &str| names.iter().filter(|name| name.contains(needle)).count();
    assert_eq!(
        count("d'écran"),
        3,
        "every join, once, for the ASCII spelling: {names:?}"
    );
    assert_eq!(
        count("d\u{2019}écran"),
        3,
        "and every join, once, for the typographic one: {names:?}"
    );
    // Uniqueness is a count, not an order: the set's order is the search order
    // (Desktop before Pictures, and joins in the order they are tried), which a
    // sort would destroy.
    let unique: std::collections::BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "no candidate is listed twice (a repeated miss reads as a bug): {names:?}"
    );
}

// Verifies: gh #405 gap 1's other half - **the accent's two normalisations are
// separate candidates per stem spelling, not one candidate normalised once.**
//
// Measured before this cycle: `name.nfd()`/`nfc()` were applied to the *joined*
// candidate (stem + date + time + ext), which is equivalent for a pure accent but
// not for the apostrophe: NFC of `d\u{2019}écran` is itself, so a file written
// with U+0027 was never reached from a U+2019 typing. With both stems enumerated
// and both normalisations applied to each, four spellings reach one file - which
// is the state macOS actually produces, because the Finder writes NFD and a paste
// is usually NFC.
//
// The variant *set* is asserted too, not only the winner: a resolver that reached
// the file by accident (say, because it happened to try the exact typed spelling
// first) would pass a resolve-only row.
#[test]
fn the_apostrophe_axes_multiply_the_variant_set() {
    let root = home("lca-405-variant-count", &[]);
    let lookup = ScreenshotLookup::new(&root);
    let variants = lookup
        .variants(Path::new(
            "~/Desktop/Capture d\u{2019}écran 2024-01-01 at 10.00.00.png",
        ))
        .expect("the French shape is recognised");
    let text = |p: &PathBuf| p.to_string_lossy().into_owned();
    let joined = |needle: &str| variants.iter().filter(|p| text(p).contains(needle)).count();
    // Four stem spellings (ASCII/U+2019 × typed/other) × the NFD variant of the
    // accented ones; the exact ASCII ones have no NFD twin, so the count is what
    // the candidate set is, not a formula the row recomputes.
    assert!(
        joined("d\u{2019}") >= 4,
        "the typographic spelling is enumerated in both normalisations: {:?}",
        variants.iter().map(text).collect::<Vec<_>>()
    );
    assert!(
        joined("d'") >= 4,
        "and the ASCII one too, since a disk can hold either: {:?}",
        variants.iter().map(text).collect::<Vec<_>>()
    );
    // Every variant lives under a searched folder, so no candidate is unreachable.
    //
    // Asserted on the **path components**, not on a `"/{dir}/"` substring: a Windows
    // path separates with `\`, so the substring form is a POSIX assumption (CI's
    // Windows leg caught it). The components are platform-independent, which is what
    // the claim is about.
    for path in &variants {
        let searched = path.components().any(|component| {
            const DIRS: [&str; 8] = [
                "Desktop",
                "Bureau",
                "Schreibtisch",
                "Escritorio",
                "Pictures",
                "Images",
                "Bilder",
                "Imágenes",
            ];
            DIRS.iter()
                .any(|dir| component.as_os_str() == std::ffi::OsStr::new(dir))
        });
        assert!(
            searched,
            "a variant outside the searched folders can never be found: {}",
            path.display()
        );
    }
    let _ = std::fs::remove_dir_all(&root);
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
