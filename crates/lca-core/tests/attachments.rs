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
            abi_version: "0.2".to_string(),
            working_dir: "/tmp".to_string(),
        },
        Record::User {
            v: FORMAT_VERSION,
            ts: 2,
            id: "u1".to_string(),
            content: "what is this?".to_string(),
            attachments: vec!["hash-a".to_string(), "hash-missing".to_string()],
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
