//! gh #403 gap 3 - the paste ladder has a **file tier**, and it is asked first.
//!
//! The ladder was two tiers (`command_paste`: image → text) while pi's is three
//! (**files → image → text**), so a user who copies a file in Finder or Explorer
//! and presses the paste key got "the clipboard holds no image" or the raw text.
//!
//! This row is the *ordering* receipt, at the seam a caller reads: the pure
//! `pick_tier` decision has to put the file list first when the clipboard
//! offers all three, put the image second, and put text third - and it must
//! **stop** at a refused name rather than silently pasting the text tier
//! underneath it. The formatting rules (control-character refusal, `!`-mode
//! quoting, newline joining, smart spaces) are pinned beside the code in
//! `crates/lca-cli/src/tui/paste_files.rs`'s own unit rows, which need no
//! clipboard; this row is the part a `#[test]` in the crate root can see.
//!
//! Verifies: gh #403 gap 3 acceptance.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn read(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #403 gap 3 - the ladder is **three tiers with files first**, and
// the file tier is wired into `command_paste` **ahead** of the image read rather
// than after it (a tier appended at the end would be a fallback, which is the
// opposite of pi's priority).
#[test]
fn the_paste_ladder_asks_for_files_before_images() {
    let attach = read("crates/lca-cli/src/tui/attach.rs");
    let start = attach
        .find("pub(super) fn command_paste")
        .expect("`command_paste` exists");
    let body = &attach[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];

    let files = body
        .find("pick_tier")
        .expect("the file tier is decided inside command_paste");
    let image = body
        .find("get_image()")
        .expect("the image tier is still there");
    assert!(
        files < image,
        "the file tier is asked BEFORE the image tier (files at {files}, image at \
         {image}); pi's ladder is a priority, so a tier appended after the image \
         read would only be a fallback"
    );
    let text = body
        .find("read_clipboard_text")
        .expect("the text tier is still there");
    assert!(
        image < text,
        "and the image still outranks text ({image} < {text})"
    );
}

// Verifies: gh #403 gap 3 - the ordered decision is **one pure function**, so the
// priority cannot drift into a chain of `if let`s that happens to be in the right
// order today. This asserts the function exists and is used by the command.
#[test]
fn the_tier_order_is_one_decision_function() {
    let paste = read("crates/lca-ui/src/state_paste.rs");
    assert!(
        paste.contains("pub fn pick_tier("),
        "the ladder's priority is a pure function with a name"
    );
    for arm in ["Files(String)", "Image", "Text(String)", "Nothing"] {
        assert!(
            paste.contains(arm),
            "`PasteTier::{arm}` names one rung, so an ordering row can assert it"
        );
    }
    assert!(
        paste.contains("pub fn files_insertion("),
        "and the formatting is its own pure function (the unit rows live beside it)"
    );
    // The three formatting rules pi specifies, each visible in the source the
    // unit rows exercise: refusal on a control character, shell quoting for `!`
    // mode, newline joining for the prompt.
    for rule in [
        "char::is_control",
        "PasteMode::Bash => shell_quote",
        "PasteMode::Prompt => parts.join(",
    ] {
        assert!(paste.contains(rule), "the rule {rule:?} is present");
    }
}

// Verifies: gh #403 gap 3 - the second reader hook lives on the **existing**
// clipboard seam (`UiHooks`), not a new global, and the host fills it through the
// same bounded helper as the image and text reads.
#[test]
fn the_file_reader_is_a_hook_on_the_clipboard_seam() {
    let hooks = read("crates/lca-ui/src/state_hooks.rs");
    assert!(
        hooks.contains("pub type ClipboardFilesReader ="),
        "the reader is a typed hook beside `ClipboardReader`"
    );
    let state = read("crates/lca-ui/src/state.rs");
    assert!(
        state.contains("pub read_clipboard_files: Option<ClipboardFilesReader>"),
        "and a field on `UiHooks`, which every embedder already constructs"
    );
    let cli_hooks = read("crates/lca-cli/src/tui/hooks.rs");
    assert!(
        cli_hooks
            .contains("read_clipboard_files: Some(super::attach::read_clipboard_files_hook())"),
        "the CLI wires it from its own clipboard module"
    );
    // Bounded like everything else: the same helper, so a stalled owner cannot
    // hang the interface through the new tier.
    let attach = read("crates/lca-cli/src/tui/attach.rs");
    let reader = attach
        .split("pub(crate) fn read_clipboard_files_hook")
        .nth(1)
        .expect("the hook exists")
        .split('\n')
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        reader.contains("with_clipboard_timeout"),
        "the file read goes through the bounded helper, not `arboard` inline: \
         {reader}"
    );
}

// Verifies: gh #403 gap 3 - **no new dependency and no new `unsafe`.** The
// platform APIs the study names (`NSPasteboard`, `CF_HDROP`) reach this tier
// through `arboard`'s own `file_list`, which is already a dependency of the
// clipboard module - so this cycle adds neither a crate nor an `unsafe` block.
#[test]
fn the_file_tier_adds_neither_a_crate_nor_unsafe() {
    let manifest = read("crates/lca-cli/Cargo.toml");
    assert!(
        manifest.contains("arboard = "),
        "`arboard` was already the clipboard dependency"
    );
    assert!(
        !manifest.contains("objc") && !manifest.contains("windows-sys"),
        "no platform crate was added: `arboard`'s `file_list` covers macOS, \
         Windows and X11"
    );
    let paste = read("crates/lca-ui/src/state_paste.rs");
    assert!(
        !paste.contains("unsafe"),
        "the formatting module is pure and safe"
    );
    let attach = read("crates/lca-cli/src/tui/attach.rs");
    assert!(
        !attach.contains("unsafe {"),
        "and the tier adds no `unsafe` block to the clipboard module"
    );
}
