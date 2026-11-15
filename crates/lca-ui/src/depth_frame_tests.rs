//! Gh #600's frame-level rows: the rung is chosen once, and the frame says so.
//!
//! The arithmetic rows live beside the lowering (`theme::depth`). These drive
//! `Chat::render` - the production path - because a theme built at a rung and a
//! transcript painted from it are two different claims, and only the frame proves
//! both.

use lca_tui::engine::colors::ColorDepth;

/// A chat whose options name a rung, the way the host does (`tui/commands.rs`).
fn chat_at(depth: ColorDepth) -> crate::chat::Chat {
    let mut options = crate::chat::tests_support::options();
    options.plain = false;
    options.color_depth = depth;
    crate::chat::Chat::new(
        options,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    )
}

// Verifies: gh #600 - **a 256-colour session paints indices end to end.** The
// defect was a frame full of `38;2` bytes on a terminal that cannot render them;
// the row asserts the production render carries `38;5`/`48;5` and no `38;2`/`48;2`
// at all, on a document with a user band, a tool card and a notice - the three
// things a user sees painted.
#[test]
fn an_ansi256_session_paints_indices_on_every_row() {
    let mut chat = chat_at(ColorDepth::Ansi256);
    chat.transcript.push_user("hello");
    chat.transcript.finish_assistant();
    chat.transcript.start_tool("bash", "echo hi");
    chat.transcript
        .finish_tool_status(crate::transcript::ToolStatus::Ok);
    chat.say("a notice");
    let frame: String = chat.render(80).join("\n");
    assert!(
        frame.contains("38;5;") || frame.contains("48;5;"),
        "the palette channel is used somewhere: {frame}"
    );
    assert!(
        !frame.contains("38;2;") && !frame.contains("48;2;"),
        "and 24-bit bytes appear nowhere: {frame}"
    );
}

// Verifies: gh #600 - **a truecolor session is unchanged.** The same document at
// `Truecolor` still carries `38;2`/`48;2` and no palette index, so the tier did
// not cost the rung that already worked.
#[test]
fn a_truecolor_session_still_paints_bytes() {
    let mut chat = chat_at(ColorDepth::Truecolor);
    chat.transcript.push_user("hello");
    let frame: String = chat.render(80).join("\n");
    assert!(
        frame.contains("38;2;") || frame.contains("48;2;"),
        "{frame}"
    );
    assert!(
        !frame.contains("38;5;") && !frame.contains("48;5;"),
        "{frame}"
    );
}

// Verifies: gh #600 + FR-UI-5 - **`ui.color = never` beats the rung.** A `never`
// session handed a truecolor rung paints nothing, because `plain` is the user's
// statement about their eyes and the ladder puts it above everything.
#[test]
fn a_never_session_paints_nothing_at_any_depth() {
    for depth in [
        ColorDepth::Truecolor,
        ColorDepth::Ansi256,
        ColorDepth::Ansi16,
        ColorDepth::None,
    ] {
        let mut options = crate::chat::tests_support::options();
        options.plain = true;
        options.color_depth = depth;
        let mut chat = crate::chat::Chat::new(
            options,
            std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
        );
        chat.transcript.push_user("hello");
        let frame: String = chat.render(80).join("\n");
        assert!(
            !frame.contains("\x1b["),
            "a never session emits no SGR at all at {depth:?}: {frame}"
        );
    }
}
