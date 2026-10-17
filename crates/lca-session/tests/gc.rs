//! Attachment reachability (gh #311): the GC's mark set and the export's
//! attachment map must be the same walker, and gc must keep every hash a
//! resolved record list references while still sweeping true orphans.
//!
//! Split from `log.rs` for the workspace file ceiling (gate 11).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{ExportOptions, SessionStore};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

/// A tool result carrying only image attachment hashes (gh #113/#311):
/// no text spill, so `attachment` is `None` and the record must still be
/// reachable.
fn image_result_record(id: &str, images: &[&str]) -> Record {
    Record::ToolResult {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        call_id: format!("{id}-call"),
        status: lca_protocol::ToolResultStatus::Ok,
        content: Some("[image attachment]".to_string()),
        attachment: None,
        truncated: false,
        duration_ms: None,
        exit_code: None,
        nested: Vec::new(),
        full_output_path: None,
        images: images.iter().map(|hash| hash.to_string()).collect(),
    }
}

/// A tool result whose oversized text spilled to `attachments/<hash>`.
fn attachment_record(id: &str, hash: &str) -> Record {
    Record::ToolResult {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        call_id: format!("{id}-call"),
        status: lca_protocol::ToolResultStatus::Ok,
        content: Some("[full output in an attachment]".to_string()),
        attachment: Some(hash.to_string()),
        truncated: true,
        duration_ms: None,
        exit_code: None,
        nested: Vec::new(),
        full_output_path: None,
        images: Vec::new(),
    }
}

fn write_attachment(session: &lca_session::Session, hash: &str, bytes: &str) {
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(hash), bytes).expect("write attachment");
}

// Verifies: gh #311 - the GC's mark set must see EVERY hash-bearing record
// shape. A tool result that returned images (gh #113) stores its payload
// in `images` with `attachment` absent, so the old walker never matched it
// at all and gc deleted live images.
#[test]
fn gc_keeps_tool_result_images_and_still_sweeps_orphans() {
    let store = store("gc-tool-images");
    let project = scratch("gc-tool-images-project");
    let session = store.create_session(&project, "test").expect("create");
    let image_a = "1".repeat(64);
    let image_b = "2".repeat(64);
    let spill = "3".repeat(64);
    let user = "4".repeat(64);
    let orphan = "5".repeat(64);
    for (hash, body) in [
        (&image_a, "image a"),
        (&image_b, "image b"),
        (&spill, "text spill"),
        (&user, "user attachment"),
        (&orphan, "nobody references me"),
    ] {
        write_attachment(&session, hash, body);
    }

    // images only (no text spill): the shape the old walker missed.
    store
        .append(&session, image_result_record("r1", &[&image_a, &image_b]))
        .expect("append");
    // a text spill result (`attachment` Some, `images` empty).
    store
        .append(&session, attachment_record("r2", &spill))
        .expect("append");
    // a user message attachment.
    store
        .append(
            &session,
            Record::User {
                v: FORMAT_VERSION,
                ts: 2,
                id: "r3".into(),
                parent: None,
                content: "look".into(),
                attachments: vec![user.clone()],
                queue: None,
            },
        )
        .expect("append");

    let deleted = store.gc(&session).expect("gc");
    assert_eq!(
        deleted,
        vec![orphan.clone()],
        "only the truly unreferenced file is swept (gc is not neutered)"
    );
    for (hash, why) in [
        (&image_a, "a tool-result image"),
        (&image_b, "a second tool-result image"),
        (&spill, "a tool-result text spill"),
        (&user, "a user attachment"),
    ] {
        assert!(
            session.dir().join("attachments").join(hash).exists(),
            "{why} referenced by a record must survive gc"
        );
    }
    assert!(
        !session.dir().join("attachments").join(&orphan).exists(),
        "the orphan is gone"
    );
}

// Verifies: gh #311 AC3 - gc's mark set and the export's attachment map are
// the SAME walker, so they cannot drift apart again. One record set through
// both must yield identical hash sets; before the fix gc dropped the
// tool-result images the export kept.
#[test]
fn gc_and_export_reach_the_same_attachment_set() {
    let store = store("gc-export-equivalence");
    let project = scratch("gc-export-equivalence-project");
    let session = store.create_session(&project, "test").expect("create");
    let image = "a1".repeat(32);
    let spill = "b2".repeat(32);
    let user = "c3".repeat(32);
    let orphan = "d4".repeat(32);
    for (hash, body) in [
        (&image, "an image"),
        (&spill, "a text spill"),
        (&user, "a user attachment"),
        (&orphan, "an orphan"),
    ] {
        write_attachment(&session, hash, body);
    }
    store
        .append(&session, image_result_record("r1", &[&image]))
        .expect("append");
    store
        .append(&session, attachment_record("r2", &spill))
        .expect("append");
    store
        .append(
            &session,
            Record::User {
                v: FORMAT_VERSION,
                ts: 2,
                id: "r3".into(),
                parent: None,
                content: "look".into(),
                attachments: vec![user.clone()],
                queue: None,
            },
        )
        .expect("append");

    // What the export resolves before gc.
    let export_path = store
        .export(&session, ExportOptions::default())
        .expect("export");
    let exported: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&export_path).expect("read")).expect("json");
    let mut exported_hashes: Vec<String> = exported
        .get("attachments")
        .and_then(|v| v.as_object())
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    exported_hashes.sort();

    // gc then export again: nothing the first export listed may be gone,
    // and the second export must list exactly the same set.
    let deleted = store.gc(&session).expect("gc");
    assert_eq!(deleted, vec![orphan.clone()], "only the orphan is swept");
    assert!(
        !session.dir().join("attachments").join(&orphan).exists(),
        "the orphan is gone"
    );
    for hash in &exported_hashes {
        assert!(
            session.dir().join("attachments").join(hash).exists(),
            "gc kept every hash the export resolved: {hash}"
        );
    }
    let export_path = store
        .export(&session, ExportOptions::default())
        .expect("export again");
    let exported: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&export_path).expect("read")).expect("json");
    let mut after: Vec<String> = exported
        .get("attachments")
        .and_then(|v| v.as_object())
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    after.sort();
    assert_eq!(
        after, exported_hashes,
        "gc and export agree on reachability by construction"
    );
    assert_eq!(after.len(), 3, "image + spill + user, no orphan");
}
