//! GitHub #342: `RotatingWriter` opened `debug.log`/`debug-wire.log`
//! with plain `OpenOptions`, so umask 022 made them 0644 — any other
//! local user could read prompts, payloads, and paths. New opens now use
//! `mode(0o600)` and repair a pre-existing 0644 file on the next open.
//!
//! Verifies: the produced log is owner-only on Unix, including a
//! pre-existing world-readable file being repaired. Windows is the
//! documented profile-ACL shape (named skip).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The POSIX-mode assertion is Unix-only.
#[cfg(unix)]
mod unix {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use lca_debug::logfiles::{DebugPaths, RotatingWriter};

    fn mode_of(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
    }

    // Verifies: gh #342 acceptance 1 and 2 - a fresh pair of logs is
    // owner-only, and a pre-existing 0644 file is repaired on the next
    // open (the end state, not just new files).
    #[test]
    fn the_debug_logs_are_owner_only_and_repair_an_old_world_readable_file() {
        let root = lca_testkit::scratch_path("regression-342-modes");
        let _ = std::fs::remove_dir_all(&root);
        let paths = DebugPaths::in_dir(&root);
        std::fs::create_dir_all(&root).expect("mkdir logs");

        // A world-readable legacy file, as an earlier version left it.
        std::fs::write(&paths.wire, "legacy line\n").expect("plant");
        std::fs::set_permissions(&paths.wire, std::fs::Permissions::from_mode(0o644))
            .expect("chmod 644");

        let mut writer = RotatingWriter::open(&paths.wire).expect("open");
        writer.write_all(b"new line\n").expect("write");
        writer.flush().expect("flush");

        assert_eq!(mode_of(&paths.wire), 0o600, "the legacy log is repaired");

        // A fresh file (the internal log) is created owner-only.
        let mut internal = RotatingWriter::open(&paths.log).expect("open");
        internal.write_all(b"internal line\n").expect("write");
        internal.flush().expect("flush");
        assert_eq!(mode_of(&paths.log), 0o600, "a fresh log is owner-only");

        let _ = std::fs::remove_dir_all(&root);
    }
}

// Windows: `%USERPROFILE%` subdirectories inherit the profile ACL
// (user + SYSTEM + Administrators). Asserting that needs ACL surgery the
// crate does not carry, so the mode check skips with a named reason; the
// writer still runs.
#[cfg(windows)]
#[test]
fn the_debug_logs_are_owner_only_and_repair_an_old_world_readable_file() {
    use std::io::Write;

    use lca_debug::logfiles::{DebugPaths, RotatingWriter};

    let root = lca_testkit::scratch_path("regression-342-modes");
    let _ = std::fs::remove_dir_all(&root);
    let paths = DebugPaths::in_dir(&root);
    std::fs::create_dir_all(&root).expect("mkdir logs");
    std::fs::write(&paths.wire, "legacy line\n").expect("plant");
    let mut writer = RotatingWriter::open(&paths.wire).expect("open");
    writer.write_all(b"new line\n").expect("write");
    writer.flush().expect("flush");
    assert!(
        paths.wire.exists(),
        "the profile-ACL shape is documented, not asserted"
    );
    eprintln!("skip: Windows uses the inherited profile ACL (no POSIX mode bits)");
    let _ = std::fs::remove_dir_all(&root);
}
