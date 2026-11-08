//! The image attach path (ADR-0029): staging a file into the session's
//! content-addressed store, and the assembly that turns a user record's
//! attachments into typed image blocks.

use lca_protocol::{FORMAT_VERSION, MessageRole, Record};
use lca_session::SessionStore;

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];

// Verifies: ADR-0029 and D8 - a staged image is content-addressed, stored
// owner-only, and its stub names the hash and sniffed media type.
#[test]
fn staging_an_image_writes_an_owner_only_content_addressed_file() {
    let store = SessionStore::new(scratch("store"));
    let project = scratch("project");
    let session = store.create_session(&project, "test").expect("create");
    let source = project.join("shot.png");
    std::fs::write(&source, PNG).expect("write source");

    let staged = lca_core::stage_image(&session, &source).expect("stage");
    assert!(staged.stub.contains("image/png"), "stub: {}", staged.stub);
    assert!(staged.stub.contains(&staged.hash[..8]));
    let file = session.dir().join("attachments").join(&staged.hash);
    assert_eq!(std::fs::read(&file).expect("read"), PNG);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "attachment files are owner-only");
    }

    // Content-addressed: the same bytes resolve to the same hash and file.
    let again = lca_core::stage_image(&session, &source).expect("stage again");
    assert_eq!(staged.hash, again.hash);
    let count = std::fs::read_dir(session.dir().join("attachments"))
        .expect("dir")
        .count();
    assert_eq!(count, 1, "one file for identical content");
}

// Verifies: ADR-0029 and D8 - a file whose bytes are not a recognized image is
// refused, never stored as opaque text (the media type is sniffed, not named).
#[test]
fn staging_refuses_a_non_image() {
    let store = SessionStore::new(scratch("reject-store"));
    let project = scratch("reject-project");
    let session = store.create_session(&project, "test").expect("create");
    let source = project.join("notes.txt");
    std::fs::write(&source, b"just text, no magic bytes").expect("write");
    assert!(lca_core::stage_image(&session, &source).is_err());
    assert!(
        !session.dir().join("attachments").exists(),
        "nothing was written for a refused file"
    );
}

// Verifies: ADR-0029 - assembly turns a user record's image attachment into a
// typed `ContentBlock::Image` after its text, and a hash the resolver cannot
// resolve is skipped without failing the turn.
#[test]
fn assembly_appends_an_image_block_for_a_user_attachment() {
    let records = vec![
        Record::SessionStart {
            v: FORMAT_VERSION,
            ts: 1,
            agent_version: "0.1.3".to_string(),
            abi_version: "0.5".to_string(),
            working_dir: "/tmp".to_string(),
        },
        Record::User {
            v: FORMAT_VERSION,
            ts: 2,
            id: "u1".to_string(),
            parent: None,
            content: "what is this?".to_string(),
            attachments: vec!["hash-a".to_string(), "hash-missing".to_string()],
            queue: None,
        },
    ];
    let assembled = lca_core::assemble_with(&records, "system", &|hash| {
        (hash == "hash-a").then(|| lca_core::Attachment {
            media_type: "image/png".to_string(),
            bytes: PNG.to_vec(),
        })
    });
    let user = assembled
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("the user message");
    assert_eq!(
        user.content,
        vec![
            lca_protocol::ContentBlock::Text {
                text: "what is this?".to_string()
            },
            lca_protocol::ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: PNG.to_vec()
            },
        ],
        "text first, then the resolved image; the missing hash is skipped"
    );
}

// Verifies: gh #113 - assembly emits image blocks for a tool result's
// persisted image hashes (same resolver shape as user attachments);
// an unresolvable hash is skipped without failing the turn.
#[test]
fn assembly_emits_image_blocks_for_tool_result_images() {
    let records = vec![
        Record::SessionStart {
            v: FORMAT_VERSION,
            ts: 1,
            agent_version: "0.1.3".to_string(),
            abi_version: "0.5".to_string(),
            working_dir: "/tmp".to_string(),
        },
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 2,
            id: "r1".to_string(),
            parent: None,
            call_id: "call-1".to_string(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("[image image/png, 12 bytes]".to_string()),
            attachment: None,
            truncated: false,
            duration_ms: None,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
            images: vec!["hash-a".to_string(), "hash-missing".to_string()],
        },
    ];
    let assembled = lca_core::assemble_with(&records, "system", &|hash| {
        (hash == "hash-a").then(|| lca_core::Attachment {
            media_type: "image/png".to_string(),
            bytes: PNG.to_vec(),
        })
    });
    let result = assembled
        .messages
        .iter()
        .find(|message| message.role == lca_protocol::MessageRole::Tool)
        .expect("the tool result message");
    assert_eq!(
        result.content,
        vec![
            lca_protocol::ContentBlock::Text {
                text: "[image image/png, 12 bytes]".to_string()
            },
            lca_protocol::ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: PNG.to_vec()
            },
        ],
        "text first, then the resolved image; the missing hash is skipped"
    );
}

// Verifies: gh #547 - the sync writer loses a create race to its own twin and
// keeps the winner's file, instead of failing one caller for another's success.
//
// The shape: N callers stage the same bytes at the same moment. The path is the
// content's hash, so every writer would produce byte-identical content and only
// one of them can win `create_new`. Before this, `exists()`-then-`create_new`
// let a caller that saw `!exists()` fail on `AlreadyExists` - a TOCTOU window
// whose width was the whole reason CI could not see it (`store_image_bytes_async`
// already ignored the miss, so the two twins disagreed).
//
// Threads, not tasks: the row is about a filesystem race between concurrent
// writers, and a barrier gives them the same start line.
#[cfg(unix)]
#[test]
fn concurrent_stores_of_the_same_bytes_all_succeed_with_one_file() {
    use std::sync::{Arc, Barrier};

    let store = SessionStore::new(scratch("race-store"));
    let project = scratch("race-project");
    let session = store.create_session(&project, "test").expect("create");
    let bytes: Vec<u8> = PNG.to_vec();
    const WRITERS: usize = 8;
    let barrier = Arc::new(Barrier::new(WRITERS));
    let session = Arc::new(session);
    let bytes = Arc::new(bytes);

    let handles: Vec<_> = (0..WRITERS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let session = Arc::clone(&session);
            let bytes = Arc::clone(&bytes);
            std::thread::spawn(move || {
                barrier.wait();
                lca_core::store_image_bytes(&session, &bytes)
            })
        })
        .collect();

    let hashes: Vec<String> = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .expect("the writer thread")
                .expect("every caller of the same content succeeds (gh #547)")
        })
        .collect();
    assert_eq!(hashes.len(), WRITERS);
    assert!(
        hashes.windows(2).all(|pair| pair[0] == pair[1]),
        "identical bytes hash identically: {hashes:?}"
    );

    let dir = session.dir().join("attachments");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .expect("attachments dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(files.len(), 1, "one file for one content hash: {files:?}");
    assert_eq!(std::fs::read(&files[0]).expect("read"), bytes.to_vec());

    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&files[0])
        .expect("meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the losing writers did not loosen the mode");
}

// Verifies: gh #547 - a pre-existing file for the same hash is left alone.
//
// Not just "the second call succeeds": the *first* file must survive untouched,
// so a caller cannot silently rewrite content another caller already published.
// The content is identical by construction (same hash), so the assertion that
// can actually fail is the inode plus the mode - an unconditional `create_new`
// that reported the miss as success while truncating would pass a content
// comparison.
#[cfg(unix)]
#[test]
fn a_pre_existing_identical_attachment_is_not_rewritten() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let store = SessionStore::new(scratch("keep-store"));
    let project = scratch("keep-project");
    let session = store.create_session(&project, "test").expect("create");
    let hash = lca_core::store_image_bytes(&session, PNG).expect("first store");
    let file = session.dir().join("attachments").join(&hash);
    let before = std::fs::metadata(&file).expect("meta");
    assert_eq!(before.permissions().mode() & 0o777, 0o600);
    let (ino, mtime) = (before.ino(), before.mtime_nsec());

    // A month later in mtime terms: any rewrite would move both.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let second = lca_core::store_image_bytes(&session, PNG).expect("second store");
    assert_eq!(second, hash, "the same content, the same hash");
    let after = std::fs::metadata(&file).expect("meta");
    assert_eq!(after.ino(), ino, "the same file, not a replacement");
    assert_eq!(
        after.mtime_nsec(),
        mtime,
        "untouched: an identical store does not rewrite"
    );
    assert_eq!(after.permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read(&file).expect("read"), PNG);
}

// Verifies: gh #547 - the two twins agree. The async store and the sync store
// write the same content-addressed file with the same owner-only mode, and
// either order works: whichever lands second reports success and leaves the
// first file in place.
#[cfg(unix)]
#[tokio::test]
async fn the_sync_and_async_stores_agree_on_the_same_file() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let store = SessionStore::new(scratch("twins-store"));
    let project = scratch("twins-project");
    let session = store.create_session(&project, "test").expect("create");

    let sync_hash = lca_core::store_image_bytes(&session, PNG).expect("sync store");
    let file = session.dir().join("attachments").join(&sync_hash);
    let sync_meta = std::fs::metadata(&file).expect("meta");

    let async_hash = lca_core::store_image_bytes_async(&session, PNG)
        .await
        .expect("the async twin succeeds over a file the sync twin wrote");
    assert_eq!(async_hash, sync_hash, "one hash for one content");
    let after = std::fs::metadata(&file).expect("meta");
    assert_eq!(
        after.ino(),
        sync_meta.ino(),
        "the async twin did not replace the sync twin's file"
    );
    assert_eq!(after.permissions().mode() & 0o777, 0o600, "one mode");

    // And the other direction: an async-first store, then the sync writer.
    let other = SessionStore::new(scratch("twins-other"));
    let other_session = other.create_session(&project, "test").expect("create");
    let first = lca_core::store_image_bytes_async(&other_session, PNG)
        .await
        .expect("async first");
    let second = lca_core::store_image_bytes(&other_session, PNG)
        .expect("the sync twin succeeds over a file the async twin wrote");
    assert_eq!(first, second);
    assert_eq!(
        std::fs::read(other_session.dir().join("attachments").join(&second)).expect("read"),
        PNG
    );
}
