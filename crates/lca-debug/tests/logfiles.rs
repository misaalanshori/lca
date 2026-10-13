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
