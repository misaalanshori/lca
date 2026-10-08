//! Invocation tests (phase 4A): stdin, `@file`, tool/session/resource
//! flags. Split out for the file ceiling.

use super::*;

// Verifies: gh #71 - `@`-tokens split from positionals; stdin and file
// text prepend the first message in pi's order; the rest ride along.
#[test]
fn at_tokens_split_and_stdin_prepends_first() {
    let planned = plan_messages(
        None,
        Some("review"),
        &["@a.txt".to_string(), "extra".to_string()],
        Some("DIFF"),
        "<file>\n</file>\n",
    );
    assert_eq!(planned.files, vec!["a.txt".to_string()]);
    assert_eq!(
        planned.first.as_deref(),
        Some("DIFF<file>\n</file>\nreview")
    );
    assert_eq!(planned.rest, vec!["extra".to_string()]);
}

// Verifies: gh #71 - pi's shape: any token starting with `@` is a
// path, even a lone `@` (it resolves to the cwd and the expander
// refuses directories out loud).
#[test]
fn at_splitting_follows_pi() {
    let planned = plan_messages(None, None, &["@".to_string(), "hi".to_string()], None, "");
    assert_eq!(planned.files, vec!["".to_string()]);
    assert_eq!(planned.first.as_deref(), Some("hi"));
    assert!(planned.rest.is_empty());
}

// Verifies: gh #71 - text inlines in pi's `<file name>` shape; images
// return paths for staging; empties skip; missing paths and
// directories refuse out loud.
#[test]
fn at_files_expand_text_and_images() {
    let root = lca_testkit::scratch_path("lca-at-files");
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(root.join("a.txt"), "hello\n").expect("write");
    std::fs::write(root.join("empty.txt"), "").expect("write");
    std::fs::create_dir_all(root.join("dir")).expect("mkdir");
    // A minimal PNG (signature + IHDR) sniffs as an image.
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.resize(40, 0);
    std::fs::write(root.join("img.png"), &png).expect("write");

    let (text, images) = expand_at_files(
        &[
            "a.txt".to_string(),
            "empty.txt".to_string(),
            "img.png".to_string(),
        ],
        &root,
    )
    .expect("expands");
    assert!(text.contains("<file name=\""), "pi shape: {text:?}");
    assert!(text.contains("hello"), "content inlines: {text:?}");
    assert!(!text.contains("empty"), "empties skip");
    assert_eq!(images.len(), 1, "the png stages");
    assert!(images[0].ends_with("img.png"));

    assert!(
        expand_at_files(&["missing.txt".to_string()], &root).is_err(),
        "missing refuses"
    );
    assert!(
        expand_at_files(&["dir".to_string()], &root).is_err(),
        "dirs refuse"
    );
    let _ = std::fs::remove_dir_all(&root);
}
