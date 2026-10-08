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
