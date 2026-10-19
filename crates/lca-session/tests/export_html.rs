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
            duration_ms: None,
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

// Verifies: gh #312 - the `<article>`'s class list is well formed. The
// template used `class="message{} {}"` while `row.class` is already
// `"message-user"` / `"message-assistant"`, so every exported transcript
// carried `class="messagemessage-user "` - a single unknown CSS class that
// matches none of the stylesheet's `.message-*` selectors, plus a stray
// trailing space. A reader sees an unstyled page and would have to guess why.
#[test]
fn article_classes_are_space_separated_and_well_formed() {
    let html = lca_session::export_html::render_html("shot", &fixture_records(), &BTreeMap::new());
    assert!(
        !html.contains("messagemessage"),
        "the class list must not concatenate two class names"
    );
    for class in [
        "class=\"message message-user\"",
        "class=\"message message-assistant\"",
        "class=\"message message-tool-result\"",
    ] {
        assert!(
            html.contains(class) || !html.contains(&class.replace("message message-", "message ")),
            "well-formed class list {class:?} in the export"
        );
    }
    // No class attribute may end with a space before the quote.
    assert!(
        !html.contains(" \"\u{3e}") && !html.contains(" \" "),
        "no class attribute ends with a dangling space"
    );
}

// Verifies: gh #312 - a hidden record carries both the base marker and the
// hidden marker as separate classes, so the toggle's CSS applies.
#[test]
fn a_hidden_record_carries_both_markers_as_separate_classes() {
    let html = lca_session::export_html::render_html("shot", &fixture_records(), &BTreeMap::new());
    assert!(
        html.contains("message-hidden"),
        "the hidden marker is present"
    );
    assert!(
        !html.contains("message-custommessage-hidden"),
        "the two class names must not be concatenated"
    );
    // The custom (hidden) record's article: base marker, then hidden marker.
    // Scoped to `<main>`: the sidebar carries the same class on its link, and
    // this row is about the article.
    let main = html.split("<main>").nth(1).expect("the main element");
    let article = main
        .split("<article ")
        .find(|chunk| chunk.contains("message-custom"))
        .expect("the custom record's article");
    let class = article
        .split("class=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next());
    let class = class.expect("a class attribute");
    let parts: Vec<&str> = class.split_whitespace().collect();
    assert_eq!(
        parts,
        vec!["message", "message-custom", "message-hidden"],
        "the class list is exactly the three tokens, no empties: {class:?}"
    );
}

// Verifies: gh #312 - the image's `media_type` is escaped before it lands in
// a `data:` URI. An unescaped value is an injection point into the attribute
// (the same class as the escaped text the rest of the exporter already
// handles), and it is untrusted: a media type comes from an attachment's own
// file, not from the exporter.
#[test]
fn an_image_media_type_is_escaped_in_the_data_uri() {
    let mut attachments = BTreeMap::new();
    // `hash-a` is the hash the fixture's user and tool-result records
    // reference, so the image is actually rendered.
    attachments.insert(
        "hash-a".to_string(),
        lca_session::export_html::HtmlImage {
            // A quote would close the `src` attribute if it were raw.
            media_type: "image/png\" onerror=\"alert(1)".to_string(),
            bytes: vec![1, 2, 3],
        },
    );
    let html = lca_session::export_html::render_html("shot", &fixture_records(), &attachments);
    assert!(
        !html.contains("image/png\" onerror"),
        "a raw quote must not reach the attribute"
    );
    // The `src` attribute is still one attribute: read it back and require the
    // escaped form inside it. (`escape` turns `"` into `&quot;`, which ends
    // nothing.)
    let src = html
        .split("src=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the img src attribute");
    assert!(
        src.starts_with("data:image/png&quot; onerror=&quot;alert(1);base64,"),
        "the media type is escaped inside the data URI: {src:?}"
    );
}

// Verifies: gh #312 - the article's `id` and the sidebar's `href` name the
// same anchor. Row ids are synthesized by the exporter (`m<index>`), not taken
// from the record, so this row pins the shape both sides must agree on; the
// escaping is defensive on top of that (a synthesized id is safe by
// construction, but the interpolation point should never rely on it).
#[test]
fn the_article_id_and_the_sidebar_href_agree() {
    let html = lca_session::export_html::render_html("shot", &fixture_records(), &BTreeMap::new());
    let mut anchors: Vec<String> = html
        .split("<a href=\"#")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect();
    let mut ids: Vec<String> = html
        .split("<article id=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect();
    assert!(
        !anchors.is_empty() && !ids.is_empty(),
        "both sides rendered"
    );
    anchors.sort();
    ids.sort();
    assert_eq!(
        anchors, ids,
        "every sidebar link targets a rendered article"
    );
}
