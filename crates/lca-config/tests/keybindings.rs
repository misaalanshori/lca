//! `keybindings.toml` parsing (gh #66): action → key or list of keys.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

// Verifies: gh #66 - a string value binds one key, a list binds several,
// an empty list disables.
#[test]
fn string_list_and_empty_list_shapes_parse() {
    let dir = scratch("lca-kb-shapes");
    let path = dir.join("keybindings.toml");
    write(
        &path,
        "app.clear = \"ctrl+x\"\n\
         tui.editor.cursorLeft = [\"left\", \"ctrl+b\"]\n\
         tui.altScreen.pageUp = []\n",
    );
    let map = lca_config::load_keybindings_file(&path).expect("parses");
    assert_eq!(map["app.clear"], vec!["ctrl+x".to_string()]);
    assert_eq!(
        map["tui.editor.cursorLeft"],
        vec!["left".to_string(), "ctrl+b".to_string()]
    );
    assert!(map["tui.altScreen.pageUp"].is_empty(), "empty disables");
}

// Verifies: gh #66 - a non-string entry names the action.
#[test]
fn a_non_string_entry_names_the_action() {
    let dir = scratch("lca-kb-badtype");
    let path = dir.join("keybindings.toml");
    write(&path, "app.clear = 42\n");
    let err = lca_config::load_keybindings_file(&path).expect_err("rejects");
    assert!(err.to_string().contains("app.clear"), "{err}");
}

// Verifies: gh #66 - broken TOML names the file.
#[test]
fn broken_toml_names_the_file() {
    let dir = scratch("lca-kb-badtoml");
    let path = dir.join("keybindings.toml");
    write(&path, "app.clear = [\n");
    let err = lca_config::load_keybindings_file(&path).expect_err("rejects");
    assert!(err.to_string().contains("keybindings.toml"), "{err}");
}
