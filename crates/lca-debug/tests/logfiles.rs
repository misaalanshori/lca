//! Log files, rotation, and domain routing (gh #258 §5): exactly
//! two files, 5 MiB each with one spare, wire domains to the wire
//! file. Written before the writer — red first.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

use std::io::Write;

use lca_debug::{DebugPaths, RotatingWriter, FILE_CAP_BYTES};

// Verifies: gh #258 - the footprint is exactly two files with one
// spare each (20 MiB total, never more).
#[test]
fn footprint_is_two_files_plus_two_spares() {
    let root = lca_testkit::scratch_path("debug-paths");
    let paths = DebugPaths::in_dir(&root);
    assert_eq!(paths.log, root.join("debug.log"));
    assert_eq!(paths.wire, root.join("debug-wire.log"));
    assert_eq!(paths.spare_for(&paths.log), root.join("debug.log.1"));
    assert_eq!(paths.spare_for(&paths.wire), root.join("debug-wire.log.1"));
    assert_eq!(FILE_CAP_BYTES, 5 * 1024 * 1024);
}

// Verifies: gh #258 - a file past the cap rotates to its single
// spare (older spare clobbered), and writing continues in the fresh
// file. Small cap keeps this test instant.
#[test]
fn rotation_keeps_one_spare_and_continues() {
    let root = lca_testkit::scratch_path("debug-rotate");
    let path = root.join("debug.log");
    let mut writer = RotatingWriter::with_cap(&path, 64).expect("open");
    for chunk in ["aaa\n", "bbb\n"] {
        writer.write_all(chunk.as_bytes()).expect("write");
    }
    // Still under the cap: one file, no spare.
    assert!(path.exists());
    assert!(!root.join("debug.log.1").exists());
    // Past the cap exactly once: the full file becomes the spare,
    // writes continue in the fresh file.
    for _ in 0..5 {
        writer
            .write_all("0123456789abcdef\n".as_bytes())
            .expect("write");
    }
    writer.flush().expect("flush");
    let spare = std::fs::read_to_string(root.join("debug.log.1")).expect("spare");
    let live = std::fs::read_to_string(&path).expect("live");
    assert!(spare.contains("aaa\n"), "the rotated file keeps history");
    assert!(!live.contains("aaa\n"), "the live file starts fresh");
    assert!(live.contains("0123456789abcdef\n"), "writes continue");
}

// Verifies: gh #258 - rotation never creates a third file.
#[test]
fn rotation_never_creates_a_third_file() {
    let root = lca_testkit::scratch_path("debug-rotate-once");
    let path = root.join("debug.log");
    let mut writer = RotatingWriter::with_cap(&path, 16).expect("open");
    for _ in 0..50 {
        writer
            .write_all("0123456789abcdef\n".as_bytes())
            .expect("write");
    }
    writer.flush().expect("flush");
    let files: Vec<_> = std::fs::read_dir(&root)
        .expect("read")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(
        files.len(),
        2,
        "live plus one spare, nothing else: {files:?}"
    );
}

// ---------------------------------------------------------------------------
// gh #342: the logs were opened with the default mode (umask 022 → 0644),
// world-readable. Owner-only 0600 on Unix; the Windows shape is the profile
// ACL (documented, not testable here).
// ---------------------------------------------------------------------------

/// The Unix permission bits of a path.
#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

// Verifies: gh #342 acceptance 1 - a newly created log file is 0600.
// Windows has no POSIX mode bits; the profile ACL is the documented shape
// and its assertion would need ACL surgery the crate does not carry.
#[cfg(unix)]
#[test]
fn a_new_log_file_is_owner_only() {
    let root = lca_testkit::scratch_path("debug-mode-new");
    let path = root.join("debug.log");
    let mut writer = RotatingWriter::open(&path).expect("open");
    writer.write_all(b"line\n").expect("write");
    writer.flush().expect("flush");
    assert_eq!(mode_of(&path), 0o600, "a fresh log file is owner-only");
}

#[cfg(windows)]
#[test]
fn a_new_log_file_is_owner_only() {
    // Windows: `%USERPROFILE%` subdirectories inherit the profile ACL
    // (user + SYSTEM + Administrators), so other unprivileged users cannot
    // read the logs. Asserting that needs ACL surgery, which is out of
    // scope; the shape is documented in `logfiles.rs` and
    // `docs/environment-variables.md`.
    let root = lca_testkit::scratch_path("debug-mode-new");
    let path = root.join("debug.log");
    let mut writer = RotatingWriter::open(&path).expect("open");
    writer.write_all(b"line\n").expect("write");
    writer.flush().expect("flush");
    assert!(path.exists(), "the file exists (ACL is the platform shape)");
}

// Verifies: gh #342 acceptance 2 - a pre-existing world-readable file is
// repaired to 0600 on the next open (the end state matters, not just new
// files); rotation keeps both the live file and its spare owner-only.
#[cfg(unix)]
#[test]
fn a_preexisting_world_readable_file_is_repaired_and_rotation_keeps_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let root = lca_testkit::scratch_path("debug-mode-repair");
    let path = root.join("debug.log");
    // Plant a 0644 file as a previous version would have left it.
    std::fs::write(&path, "old\n").expect("plant");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
    assert_eq!(mode_of(&path), 0o644, "the fixture starts world-readable");

    // A tiny cap so the first write rotates, exercising every open site.
    let mut writer = RotatingWriter::with_cap(&path, 4).expect("open");
    writer.write_all(b"new line\n").expect("write");
    writer.flush().expect("flush");

    assert_eq!(mode_of(&path), 0o600, "the live file is repaired");
    let spare = root.join("debug.log.1");
    if spare.exists() {
        assert_eq!(mode_of(&spare), 0o600, "the rotated spare is owner-only");
    }
    // A direct repair (no rotation) also holds: reopen a 0644 file and
    // write once.
    let plain = root.join("debug-wire.log");
    std::fs::write(&plain, "old\n").expect("plant");
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
    let mut writer = RotatingWriter::open(&plain).expect("open");
    writer.write_all(b"more\n").expect("write");
    writer.flush().expect("flush");
    assert_eq!(mode_of(&plain), 0o600, "a reopened 0644 file is repaired");
}
