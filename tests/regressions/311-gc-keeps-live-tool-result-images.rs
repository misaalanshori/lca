//! GitHub #311: `SessionStore::gc` marked attachments reachable from
//! `Record::User.attachments` and `Record::ToolResult.attachment` only. A
//! tool that returned images (gh #113) stores them in
//! `Record::ToolResult.images` with `attachment` absent, so the record
//! never matched and `lca session gc` **permanently deleted live images** —
//! silent data loss on a documented user command, after which turn
//! assembly and export failed on the missing files.
//!
//! The root cause class was drift: the export walker had learned about
//! `images` (in #113) and the gc walker had not. Both now call one shared
//! `lca_session::attachments::referenced_attachments`.
//!
//! Verifies: end to end through the real `lca session gc` binary — a
//! session whose tool result carries only image hashes keeps those files,
//! an unreferenced file is still swept (gc is not neutered), and a
//! subsequent `lca export` still resolves them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// The CLI round trip is Unix-gated like its siblings (the harness spawns a
// POSIX shell). The walker itself is pinned portably by `lca-session`'s
// `gc_keeps_tool_result_images_and_still_sweeps_orphans` and
// `gc_and_export_reach_the_same_attachment_set`.
#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use lca_protocol::{Record, ToolResultStatus};

    /// The built `lca` binary, or `None` when this run did not build one
    /// (the harness-absence rule, testing plan §14).
    fn binary() -> Option<PathBuf> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        ["target/debug/lca", "target/release/lca"]
            .iter()
            .map(|relative| manifest.join(relative))
            .find(|path| path.is_file())
    }

    fn run(home: &Path, project: &Path, args: &[&str]) -> (String, String, i32) {
        let binary = binary().expect("the lca binary is built (harness-absence guard)");
        let out = Command::new(&binary)
            .args(args)
            .current_dir(project)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("LCA_UPDATE_CHECK", "false")
            .output()
            .expect("run lca");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// A one-pixel PNG, so the hash is a genuine image for the export's
    /// sniff gate (the parser only needs the magic).
    fn png_bytes() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png
    }

    // Verifies: gh #311 AC1 + AC2 + the export still working after gc.
    #[test]
    fn gc_keeps_live_tool_result_images_and_sweeps_true_orphans() {
        if binary().is_none() {
            eprintln!("skip: the lca binary is not built in this run (harness-absence rule)");
            return;
        }
        let root = lca_testkit::scratch_path("gh311-gc-images");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).expect("mkdir home");
        std::fs::create_dir_all(&project).expect("mkdir project");

        // Build the session through the public store API, exactly as a turn
        // would leave it: an image-bearing tool result, plus one file no
        // record references. The store treats the filename as an opaque
        // hash, so a fixed 64-hex fixture value stands in for sha256.
        let store = lca_session::SessionStore::new(home.join(".lca"));
        let session = store.create_session(&project, "gh311").expect("create");
        let image_hash = "a5".repeat(32);
        {
            let dir = session.dir().join("attachments");
            std::fs::create_dir_all(&dir).expect("mkdir attachments");
            std::fs::write(dir.join(&image_hash), png_bytes()).expect("write image");
        }
        let orphan = "ff".repeat(32);
        std::fs::write(
            session.dir().join("attachments").join(&orphan),
            b"unreferenced",
        )
        .expect("write orphan");

        store
            .append(
                &session,
                Record::ToolResult {
                    v: lca_protocol::FORMAT_VERSION,
                    ts: 2,
                    id: "r1".into(),
                    parent: None,
                    call_id: "r1-call".into(),
                    status: ToolResultStatus::Ok,
                    content: Some("[image attachment]".into()),
                    attachment: None,
                    truncated: false,
                    duration_ms: None,
                    exit_code: None,
                    nested: Vec::new(),
                    full_output_path: None,
                    images: vec![image_hash.clone()],
                },
            )
            .expect("append");

        // gc through the CLI.
        let (stdout, stderr, code) = run(&home, &project, &["session", "gc", session.id()]);
        assert_eq!(code, 0, "gc succeeds: {stdout}{stderr}");
        assert!(
            stdout.contains(&orphan) || stdout.trim() == orphan,
            "the orphan is reported as deleted: {stdout}"
        );
        assert!(
            !session.dir().join("attachments").join(&orphan).exists(),
            "the true orphan is gone"
        );
        assert!(
            session.dir().join("attachments").join(&image_hash).exists(),
            "the live tool-result image SURVIVES gc (this is the regression)"
        );

        // A later export must still resolve it.
        let (_, stderr, code) = run(&home, &project, &["export", session.id()]);
        assert_eq!(code, 0, "export still works after gc: {stderr}");
        let export =
            std::fs::read_to_string(session.dir().join("export.json")).expect("read export");
        assert!(
            export.contains(&image_hash),
            "the export lists the surviving image: {export}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
