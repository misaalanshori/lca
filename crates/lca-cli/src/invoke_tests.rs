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

fn tool_names(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

// Verifies: gh #67 - an allowlist replaces the selection; globs match;
// unknown names report without failing the run.
#[test]
fn tools_allowlist_replaces_and_globs() {
    let all = tool_names(&["read", "write", "grep", "ext-docs"]);
    let builtin = tool_names(&["read", "write", "grep"]);
    let (sel, unknown) = select_tools(&all, &all, &builtin, Some("read,grep"), None, false, false);
    assert!(sel.touched);
    assert_eq!(sel.extension, vec!["grep".to_string(), "read".to_string()]);
    assert!(unknown.is_empty());
    assert_eq!(sel.builtin.as_ref().unwrap().len(), 2);

    let (sel, _) = select_tools(&all, &all, &builtin, Some("gr*"), None, false, false);
    assert_eq!(sel.extension, vec!["grep".to_string()]);

    let (_, unknown) = select_tools(&all, &all, &builtin, Some("read,nope"), None, false, false);
    assert_eq!(unknown, vec!["nope".to_string()]);
}

// Verifies: gh #67 - `+`/`-` deltas modify the default; `--exclude`
// and `--no-builtin-tools` subtract after everything else.
#[test]
fn tools_deltas_and_excludes_subtract() {
    let all = tool_names(&["read", "write", "grep", "ext-docs"]);
    let builtin = tool_names(&["read", "write", "grep"]);
    let (sel, _) = select_tools(
        &all,
        &all,
        &builtin,
        Some("+ext-docs,-write"),
        None,
        false,
        false,
    );
    assert!(sel.extension.contains(&"ext-docs".to_string()));
    assert!(!sel.extension.contains(&"write".to_string()));
    assert!(
        sel.extension.contains(&"read".to_string()),
        "the rest stays"
    );

    let (sel, _) = select_tools(&all, &all, &builtin, None, Some("wr*"), false, false);
    assert!(!sel.extension.contains(&"write".to_string()));
    assert!(sel.builtin.as_ref().unwrap().contains("read"));

    let (sel, _) = select_tools(&all, &all, &builtin, None, None, true, false);
    assert_eq!(
        sel.extension,
        vec!["ext-docs".to_string()],
        "extensions stay"
    );
    assert!(sel.builtin.as_ref().unwrap().is_empty());
}

// Verifies: gh #67 - `--no-tools` empties everything; no flags touch
// nothing (the turn records no spurious change).
#[test]
fn no_tools_empties_and_no_flags_touch_nothing() {
    let all = tool_names(&["read", "ext-docs"]);
    let builtin = tool_names(&["read"]);
    let (sel, _) = select_tools(&all, &all, &builtin, None, None, false, true);
    assert!(sel.touched);
    assert!(sel.extension.is_empty());
    assert!(sel.builtin.as_ref().unwrap().is_empty());

    let (sel, unknown) = select_tools(&all, &all, &builtin, None, None, false, false);
    assert!(!sel.touched);
    assert!(sel.builtin.is_none());
    assert!(unknown.is_empty());
}

// Verifies: gh #70 - resource flags parse (repeatable paths, no-*
// switches, `-e` short).
#[test]
fn resource_flags_parse() {
    use clap::Parser;
    let cli = crate::Cli::parse_from([
        "lca",
        "-e",
        "a.wasm",
        "-e",
        "b.wasm",
        "--skill",
        "s/SKILL.md",
        "--theme",
        "t.toml",
        "--no-extensions",
        "--no-skills",
        "--no-themes",
    ]);
    assert_eq!(cli.extension.len(), 2);
    assert_eq!(cli.skill, vec!["s/SKILL.md".to_string()]);
    assert_eq!(cli.theme, vec!["t.toml".to_string()]);
    assert!(cli.no_extensions && cli.no_skills && cli.no_themes);
}

// Verifies: gh #70 - skill files, skill dirs, and theme files load
// with CLI precedence; bad paths refuse up front.
#[test]
fn skills_collect_extra_and_validate_paths() {
    let root = lca_testkit::scratch_path("lca-skills-extra");
    std::fs::create_dir_all(root.join("my-skill")).expect("mkdir");
    std::fs::write(root.join("my-skill/SKILL.md"), "# my skill\n").expect("write");
    std::fs::write(root.join("solo.md"), "# solo\n").expect("write");
    let roots = lca_tools::skills::SkillsRoots {
        project: std::path::PathBuf::new(),
        user: std::path::PathBuf::new(),
        extensions: std::path::PathBuf::new(),
        disabled: Vec::new(),
        extra: vec![root.join("my-skill"), root.join("solo.md")],
        resource_selection: Default::default(),
    };
    let skills = lca_tools::skills::collect(&roots);
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();
    assert!(names.contains(&"my-skill"), "dir loads: {names:?}");
    assert!(names.contains(&"solo"), "file loads: {names:?}");
    assert!(
        check_resource_paths("--skill", &[root.join("nope")], "md").is_err(),
        "missing refuses"
    );
    assert!(check_resource_paths("--skill", &[root.join("my-skill")], "md").is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

/// Write `@file` content whose first `prefix` bytes are `a` and whose
/// tail follows, so the 1 MiB cut can be aimed at a known char state.
fn write_padded(root: &std::path::Path, name: &str, prefix: usize, tail: &str) {
    let mut content = "a".repeat(prefix);
    content.push_str(tail);
    std::fs::write(root.join(name), &content).expect("write fixture");
}

/// The inlined body of the single `<file>` block, marker included.
fn inlined_body(text: &str) -> &str {
    let open = text.find('>').expect("open tag") + 1;
    let start = text[open..].find('\n').expect("tag newline") + open + 1;
    let end = text.find("\n</file>").expect("close tag");
    &text[start..end]
}

// Verifies: gh #263 - truncating an `@file` at the 1 MiB bound lands on a
// character boundary whatever the alignment: the cut aimed one byte into a
// 3-byte CJK char, two bytes into a 4-byte emoji, one byte into a 2-byte
// accent, and exactly on a boundary. The old direct
// `truncate(INPUT_MAX_BYTES)` panicked for the first three.
#[test]
fn at_file_truncation_never_splits_a_character() {
    let root = lca_testkit::scratch_path("lca-at-truncate-utf8");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let limit = INPUT_MAX_BYTES as usize;
    // A 3-byte CJK char occupying bytes limit-1..limit+2: byte `limit`
    // falls inside it. Fill is ASCII, so everything before is a boundary.
    write_padded(&root, "cjk.txt", limit - 1, "世界");
    // A 4-byte emoji occupying limit-2..limit+2: byte `limit` is inside.
    write_padded(&root, "emoji.txt", limit - 2, "🎉🎉");
    // A 2-byte accent occupying limit-1..limit+1: byte `limit` is inside.
    write_padded(&root, "accent.txt", limit - 1, "ééé");
    // Boundary-exact: byte `limit` is the start of the tail.
    write_padded(&root, "exact.txt", limit, "tail");

    // The safe cut each case must land on (the nearest boundary at or
    // below the limit).
    for (name, expected_cut) in [
        ("cjk.txt", limit - 1),
        ("emoji.txt", limit - 2),
        ("accent.txt", limit - 1),
        ("exact.txt", limit),
    ] {
        let (text, _images) = expand_at_files(&[name.to_string()], &root)
            .unwrap_or_else(|err| panic!("@file {name} must expand: {err}"));
        assert!(
            std::str::from_utf8(text.as_bytes()).is_ok(),
            "{name}: the inlined text is valid UTF-8"
        );
        let body = inlined_body(&text);
        let marker = "\n… (input truncated at 1 MiB)";
        assert!(
            body.ends_with(marker),
            "{name}: the marker still appends: {:?}",
            &body[body.len().saturating_sub(40)..]
        );
        let content = &body[..body.len() - marker.len()];
        assert_eq!(
            content.len(),
            expected_cut,
            "{name}: the cut lands on the nearest boundary at or below the bound"
        );
        assert!(
            content.is_char_boundary(content.len()),
            "{name}: the cut is a char boundary"
        );
        assert!(
            content.chars().all(|c| c == 'a'),
            "{name}: nothing after the cut survives (no partial char)"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #263 - the no-truncation path is untouched: a file under
// the bound inlines whole, marker-free.
#[test]
fn at_file_under_the_bound_is_untouched() {
    let root = lca_testkit::scratch_path("lca-at-no-truncate");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    std::fs::write(root.join("small.txt"), "世 🎉 é fine\n").expect("write");
    let (text, _images) = expand_at_files(&["small.txt".to_string()], &root).expect("expands");
    assert!(text.contains("世 🎉 é fine"), "content inlines whole");
    assert!(!text.contains("truncated"), "no marker on the happy path");
    let _ = std::fs::remove_dir_all(&root);
}
