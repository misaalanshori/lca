//! GitHub issue #23 (open, decision: print it): pi prints
//! `escape interrupt · ctrl+c/ctrl+d clear/exit · / commands …` at
//! startup; LCA relied on `/help` and new users stalled without it. The
//! interface prints one dim key-hint line on its first frame (our
//! wording, pi's shape); headless `lca -p` never prints it (the scripting
//! contract).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_ui::{Chat, UiOptions};

fn chat() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: "auto".to_string(),
            theme_dir: std::path::PathBuf::new(),
            themes: lca_ui::theme::THEMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            initial_lines: Vec::new(),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            keybinding_overrides: Default::default(),
            keybinding_error: None,
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

// The first frame carries one key-hint line: the interrupt and clear
// keys under their default names, and the slash-command pointer.
#[test]
fn the_first_frame_carries_the_key_hint() {
    let chat = chat();
    let text: String = chat
        .viewport(100, 30, 0)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("escape") && text.contains("interrupt"),
        "the interrupt key is named:\n{text}"
    );
    assert!(
        text.contains("ctrl+c") && text.contains("clear"),
        "the clear key is named:\n{text}"
    );
    assert!(
        text.contains("/ commands") || text.contains("/commands"),
        "slash commands are pointed at:\n{text}"
    );
}

/// The debug binary under test, sibling of this test executable
/// (`target/<profile>/deps` → `target/<profile>/lca`, with the Windows
/// `.exe` tried first).
fn test_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("this test's path");
    let deps = exe.parent().expect("deps dir");
    let profile = deps.parent().expect("profile dir");
    let exe = profile.join("lca.exe");
    let binary = if exe.is_file() {
        exe
    } else {
        profile.join("lca")
    };
    assert!(
        binary.is_file(),
        "expected the built binary at {} (run `cargo build -p lca-cli` or the full suite first)",
        binary.display()
    );
    binary
}

// Headless `lca -p` never prints the hint: scripts get the answer, not
// interface chrome (the scripting contract).
#[test]
fn headless_never_prints_the_hint() {
    let root = lca_testkit::scratch_path("lca-gh23-headless");
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir_all(&home).expect("mkdir");
    std::fs::create_dir_all(&project).expect("mkdir");
    let output = std::process::Command::new(test_binary())
        .args(["-p", "hello"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("LCA_UPDATE_CHECK", "false")
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("spawn lca -p");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("clear/exit") && !combined.contains("/ commands"),
        "no hint outside the interface:\n{combined}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
