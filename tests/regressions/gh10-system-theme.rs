//! GitHub issue #10 (EFG-014, criterion 1, plus `#rgb`): adopt pi's
//! `system` theme shape with our TOML divergence. `ui.theme = "system"`
//! derives foreground/background roles from the detected terminal
//! palette (the OSC 11 / DSR / COLORFGBG parsers the engine already
//! carries) and repaints on a scheme change; a terminal that answers
//! nothing falls back to dark/light; `plain` keeps emitting zero SGR.
//! `oklch()`/`okhsl()` and `theme.style()` are mapped-for-parity, not
//! this cycle; the default stays `auto`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::colors::{ColorScheme, RgbColor};
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::theme::Role;
use lca_ui::theme::{Palette, Theme};
use lca_ui::{Chat, UiOptions};

fn chat_with_theme(theme: &str) -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: theme.to_string(),
            theme_extra_dirs: Vec::new(),
            themes: lca_ui::theme::THEMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            initial_lines: Vec::new(),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            open_resume_picker: false,
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: false,
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
            confirm_switch: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

// `system` without answers falls back to dark/light hues with the
// terminal-default foreground: the forgiving shape pi documents.
#[test]
fn system_without_answers_falls_back_to_scheme_hues() {
    let dark = Theme::system_theme(None, Some(ColorScheme::Dark));
    assert_eq!(dark.name, "system");
    assert_eq!(
        dark.palette.get(Role::Text),
        lca_ui::theme::Color::Default,
        "the foreground is the terminal's own"
    );
    assert_eq!(
        dark.palette.get(Role::Error),
        Palette::dark().get(Role::Error),
        "hues come from the scheme palette"
    );
    let light = Theme::system_theme(None, Some(ColorScheme::Light));
    assert_eq!(
        light.palette.get(Role::Error),
        Palette::light().get(Role::Error)
    );
}

// A reported background repaints the background roles and keeps the
// terminal-default foreground.
#[test]
fn a_reported_background_repaints_the_background_roles() {
    let background = RgbColor {
        r: 10,
        g: 20,
        b: 30,
    };
    let theme = Theme::system_theme(Some(background), None);
    assert_eq!(theme.name, "system");
    assert_eq!(
        theme.palette.get(Role::UserMessageBg),
        lca_ui::theme::Color::Rgb(10, 20, 30)
    );
    assert_eq!(theme.palette.get(Role::Text), lca_ui::theme::Color::Default);
}

// The interface applies a terminal background answer to a `system`
// theme and repaints: the band carries the reported background.
#[test]
fn the_interface_repaints_a_system_theme_on_a_background_answer() {
    let mut chat = chat_with_theme("system");
    assert_eq!(chat.theme.name, "system");
    chat.transcript.push_user("hello");
    chat.apply_terminal_background(RgbColor {
        r: 10,
        g: 20,
        b: 30,
    });
    assert_eq!(
        chat.theme.palette.get(Role::UserMessageBg),
        lca_ui::theme::Color::Rgb(10, 20, 30)
    );
    let raw: String = chat.render(80).join("\n");
    assert!(
        raw.contains("48;2;10;20;30"),
        "the reported background reaches the screen"
    );
}

// `#rgb` expands like `#rrggbb` in theme files; a malformed value stays
// out of the palette.
#[test]
fn three_digit_hex_expands_in_theme_files() {
    let palette = Palette::from(&[(Role::Text, "#abc")]);
    assert_eq!(
        palette.get(Role::Text),
        lca_ui::theme::Color::Rgb(0xaa, 0xbb, 0xcc)
    );
    let bad = Palette::from(&[(Role::Text, "#abcd")]);
    assert_eq!(
        bad.get(Role::Text),
        lca_ui::theme::Color::Default,
        "a 4-digit value is skipped, never half-parsed"
    );
}

// `ui.theme = "system"` resolves through the loader, and the default
// theme does not change (it stays `auto`).
#[test]
fn system_loads_by_name_and_auto_stays_the_default() {
    let dir = lca_testkit::scratch_path("lca-gh10-system");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let (theme, notice) = lca_ui::theme::load("system", Some(ColorScheme::Dark), &dir);
    assert_eq!(theme.name, "system");
    assert!(notice.is_none(), "a built-in needs no notice");
    assert!(
        !lca_ui::theme::THEMES
            .iter()
            .any(|name| *name == "auto" || *name == "default"),
        "built-ins never include the default aliases"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
