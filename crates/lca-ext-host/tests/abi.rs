//! The ABI policy's machine-checkable half (docs/abi-versioning.md).

use lca_ext_abi::ABI_VERSION;
use lca_ext_host::{Manifest, SUPPORTED_ABI_WINDOW};

fn manifest_with_abi(abi: &str) -> Manifest {
    Manifest::parse(&format!(
        "name = \"window-test\"\nversion = \"1.0.0\"\nabi = \"{abi}\"\n\
         worlds = [\"tool\"]\ndescription = \"x\"\n"
    ))
    .expect("parses")
}

// Verifies: NFR-18 (the ABI line is `major.minor` semver - the shape
// the manifest and every window check parse) and NFR-19 (the host
// loads the current minor and the previous one, nothing else).
#[test]
fn the_window_accepts_the_current_and_previous_minor_only() {
    // The declared line is major.minor, exactly.
    let (major, minor) = ABI_VERSION
        .split_once('.')
        .expect("ABI_VERSION is major.minor");
    let major: u64 = major.parse().expect("major is a number");
    let minor: u64 = minor.parse().expect("minor is a number");
    assert!(
        SUPPORTED_ABI_WINDOW.contains('.'),
        "the window announces itself as a range: {SUPPORTED_ABI_WINDOW}"
    );

    // Current line: loads.
    assert!(
        manifest_with_abi(ABI_VERSION).abi_in_window(),
        "the current line {ABI_VERSION} must load"
    );

    // The previous minor: loads (the rebuild cycle abi-versioning
    // promises). During0.x the minor is the breaking position, so the
    // previous minor is the one before it.
    if minor > 0 {
        let previous = format!("{major}.{}", minor - 1);
        assert!(
            manifest_with_abi(&previous).abi_in_window(),
            "the previous line {previous} must load (NFR-19)"
        );
    }

    // The next minor (built against a newer host than this one):
    // refused - a new host line does not run on an old host.
    let next = format!("{major}.{}", minor + 1);
    assert!(
        !manifest_with_abi(&next).abi_in_window(),
        "the future line {next} must not load here"
    );

    // A different major: refused (NFR-18's semver meaning: a new major
    // is breaking, and this host is not it).
    assert!(
        !manifest_with_abi(&format!("{}.{}", major + 1, minor)).abi_in_window(),
        "a new major never loads on an older host"
    );
}

// Verifies: NFR-20's discipline (a change that removes or alters an
// export increments the major) rests on the changelog being written
// for the live line in the same change as the WIT edit
// (docs/abi-versioning.md's "Making a change"): the current version
// must appear in the ABI changelog or the entry is missing its own
// version bump.
#[test]
fn the_abi_changelog_carries_the_live_version() {
    let changelog = include_str!("../../../wit/CHANGELOG.md");
    assert!(
        changelog.contains(ABI_VERSION),
        "wit/CHANGELOG.md has no entry for the live ABI {ABI_VERSION}: \
         an ABI change without its changelog entry is incomplete \
         (docs/abi-versioning.md, NFR-18/NFR-20)"
    );
}
