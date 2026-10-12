//! Session transcript export (gh #57): the HTML sibling of the JSON
//! export. Pi's shape (sidebar, hidden-message toggle), self-contained
//! (inline style, script, and images), offline by construction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::Record;
use lca_session::SessionStore;
use std::collections::BTreeMap;

fn fixture_records() -> Vec<Record> {
    vec![
        Record::SessionStart {
            v: 1,
            ts: 1,
            agent_version: "0.6.0".to_string(),
            abi_version: "0.6".to_string(),
            working_dir: "/tmp".to_string(),
        },
        Record::User {
            v: 1,
            ts: 2,
            id: "u1".to_string(),
            parent: None,
            content: "look at <shot.png> & tell me".to_string(),
            attachments: vec!["hash-a".to_string()],
            queue: None,
        },
        Record::ToolResult {
            v: 1,
            ts: 3,
            id: "r1".to_string(),
            parent: None,
            call_id: "call-1".to_string(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("[image image/png, 4 bytes]".to_string()),
            attachment: None,
            truncated: false,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
            images: vec!["hash-a".to_string()],
        },
        Record::CustomMessage {
            v: 1,
            ts: 4,
            id: "c1".to_string(),
            parent: None,
            custom_type: "settle-append".to_string(),
            content: "a hidden note".to_string(),
            display: false,
            details: None,
        },
    ]
}

// Verifies: gh #57 - the HTML export renders every record's text
// (the text content equals the transcript), inlines attachments,
// and marks display-false records hidden behind the toggle.
#[test]
fn html_export_renders_text_images_and_hidden() {
    let mut attachments = BTreeMap::new();
    attachments.insert(
        "hash-a".to_string(),
        lca_session::export_html::HtmlImage {
            media_type: "image/png".to_string(),
            bytes: vec![0x89, b'P', b'N', b'G'],
        },
    );
    let html = lca_session::export_html::render_html("shot", &fixture_records(), &attachments);
    for text in [
        "look at &lt;shot.png&gt; &amp; tell me",
        "[image image/png, 4 bytes]",
        "a hidden note",
        "data:image/png;base64",
        "toggle-hidden",
        "sidebar",
    ] {
        assert!(html.contains(text), "export carries {text:?}");
    }
    assert!(
        html.contains("message-hidden"),
        "display-false records hide behind the toggle"
    );
}

// Verifies: gh #57 - `export --html` writes the self-contained file
// next to the JSON export, through the same store path the command
// uses.
#[test]
fn html_export_writes_beside_the_json_export() {
    let root = lca_testkit::scratch_path("lca-export-html");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let session = store.create_session(&project, "shot").expect("session");
    for record in fixture_records().into_iter().skip(1) {
        store.append(&session, record).expect("append");
    }
    let path = store
        .export_html(&session, lca_session::ExportOptions { audit: false })
        .expect("export");
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("export.html")
    );
    let html = std::fs::read_to_string(&path).expect("read");
    assert!(
        html.contains("look at &lt;shot.png&gt;"),
        "the transcript made it"
    );
}
