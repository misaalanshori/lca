//! Gh #312: the HTML transcript export emitted a malformed CSS class list and
//! interpolated an image's media type unescaped.
//!
//! - The `<article>` template read `class="message{} {}"` while `row.class` is
//!   already `"message-user"` / `"message-assistant"` / ..., so every exported
//!   transcript carried `class="messagemessage-user "`: one unknown token that
//!   matches none of the stylesheet's `.message-*` selectors, plus a trailing
//!   space when the row was not hidden. The page rendered unstyled.
//! - `image_tag` put `image.media_type` straight into `src="data:<type>;base64,…"`.
//!   Media types come from an attachment, so a value containing a quote closes
//!   the attribute and can inject a handler - the same class of bug the
//!   exporter already escapes text against.
//!
//! Verifies: gh #312 acceptance (well-formed classes, escaped media type).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::collections::BTreeMap;

use lca_protocol::Record;

/// One user record and one hidden custom record: a visible article and a
/// hidden one.
fn records() -> Vec<Record> {
    vec![
        Record::User {
            v: 1,
            ts: 1,
            id: "u1".to_string(),
            parent: None,
            content: "hello".to_string(),
            attachments: vec!["img".to_string()],
            queue: None,
        },
        Record::CustomMessage {
            v: 1,
            ts: 2,
            id: "c1".to_string(),
            parent: None,
            custom_type: "note".to_string(),
            content: "hidden".to_string(),
            display: false,
            details: None,
        },
    ]
}

fn png() -> BTreeMap<String, lca_session::export_html::HtmlImage> {
    let mut attachments = BTreeMap::new();
    attachments.insert(
        "img".to_string(),
        lca_session::export_html::HtmlImage {
            media_type: "image/png".to_string(),
            bytes: vec![0x89, b'P', b'N', b'G'],
        },
    );
    attachments
}

// Verifies: gh #312 - no class list concatenates two class names, and a
// visible article's list is exactly `message message-user` (no trailing space,
// no empty token).
#[test]
fn visible_articles_have_a_well_formed_class_list() {
    let html = lca_session::export_html::render_html("t", &records(), &png());
    assert!(
        !html.contains("messagemessage"),
        "two class names must never be joined"
    );
    let main = html.split("<main>").nth(1).expect("the main element");
    let article = main
        .split("<article ")
        .next()
        .map(str::to_string)
        .unwrap_or_default();
    let class = main
        .split("<article ")
        .nth(1)
        .and_then(|rest| rest.split("class=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .expect("a class attribute");
    assert_eq!(
        class, "message message-user",
        "exactly two tokens, nothing else (article began: {article:?})"
    );
}

// Verifies: gh #312 - a hidden record carries the base marker, its own class,
// and the hidden marker as three separate tokens. The toggle's CSS keys off
// `message-hidden` as its own class, so a concatenated name would leave the
// hidden messages visible.
#[test]
fn hidden_articles_carry_three_separate_class_tokens() {
    let html = lca_session::export_html::render_html("t", &records(), &png());
    let main = html.split("<main>").nth(1).expect("the main element");
    let class = main
        .split("<article ")
        .find(|chunk| chunk.contains("message-custom"))
        .and_then(|chunk| chunk.split("class=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .expect("the hidden record's class attribute");
    let tokens: Vec<&str> = class.split_whitespace().collect();
    assert_eq!(
        tokens,
        vec!["message", "message-custom", "message-hidden"],
        "three tokens, no empties: {class:?}"
    );
}

// Verifies: gh #312 - the image's media type is escaped inside the data URI.
// The value is escaped rather than dropped: a quote becomes `&quot;`, so the
// attribute stays one attribute and the type still reads back correctly.
#[test]
fn a_hostile_media_type_cannot_close_the_src_attribute() {
    let mut attachments = BTreeMap::new();
    attachments.insert(
        "img".to_string(),
        lca_session::export_html::HtmlImage {
            media_type: "image/png\" onerror=\"alert(1)".to_string(),
            bytes: vec![1, 2, 3],
        },
    );
    let html = lca_session::export_html::render_html("t", &records(), &attachments);
    assert!(
        !html.contains("\" onerror=\""),
        "a raw quote must not reach the attribute: the media type is escaped"
    );
    let src = html
        .split("src=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the img src attribute");
    assert!(
        src.starts_with("data:image/png&quot; onerror=&quot;alert(1);base64,"),
        "the escaped media type rides the data URI: {src:?}"
    );
}

// Verifies: gh #312 - an ordinary media type still renders as the familiar
// `data:image/png;base64,` prefix, so escaping did not change the common case.
#[test]
fn an_ordinary_media_type_is_unchanged() {
    let html = lca_session::export_html::render_html("t", &records(), &png());
    assert!(
        html.contains("src=\"data:image/png;base64,"),
        "the common shape is untouched by escaping"
    );
}
