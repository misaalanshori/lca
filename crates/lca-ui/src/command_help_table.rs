//! The `/help` description table (gh #418 Phase G split), beside the router it
//! describes.
//!
//! The `/help` guard (`chat_command_guards::every_routed_command_has_a_help_line`)
//! reads the router's arms from `chat_commands.rs` and asks this table for each
//! one's line, so the two halves stay checked against each other even though they
//! now live in different files.

/// One-line descriptions for the interface's own commands.
pub(crate) fn command_help(command: &str) -> &'static str {
    match command {
        "/help" => "list commands and keys",
        "/hotkeys" => "list every key binding",
        "/fullscreen" => "toggle terminal scrollback and app-owned screen (fullscreen)",
        "/theme" => "pick a theme with live preview",
        "/thinking" => "set the reasoning level",
        "/tree" => "browse the session's entry tree and branch in place",
        "/rename" => "rename this session (usage: /rename <name>)",
        "/name" => "rename this session, pi's spelling (usage: /name <name>)",
        "/resume" => "search and reopen a session",
        "/fork" => "fork a branch at a message (picker, or /fork <n>)",
        "/clone" => "duplicate this session at its tip and switch (usage: /clone [name])",
        "/label" => "bookmark a message (usage: /label [message-index] <name>)",
        "/labels" => "list every bookmark",
        "/jump" => "branch at a bookmark and switch (usage: /jump <name>)",
        "/scoped-models" => "choose the quick-cycle rotation (Ctrl+P)",
        "/changelog" => "show the latest released changes",
        "/reload" => {
            "re-run discovery without restarting (settings, extensions, prompts, themes, keys)"
        }
        "/exit" => "leave the interface",
        "/login" => "sign in to a provider",
        "/logout" => "clear the provider's stored key",
        "/usage" => "show the provider's usage, when it has one",
        "/model" => "list or switch the session's model",
        "/compact" => "summarize the session to free context",
        "/attach" => "attach an image to the next message",
        "/paste" => "paste the clipboard image to the next message",
        "/ext" => "manage extensions (enable/disable, compaction strategy)",
        "/extensions" => "browse and install extensions from the hub",
        "/export" => "write the session out (add html for the transcript page)",
        "/import" => "import JSONL lines as a new session",
        "/bug" => "write a local diagnostic bundle",
        "/trust" => "trust the project folder (auto-approve in-workspace commands)",
        "/permissions" => "manage allow/deny rules (session, project, global)",
        "/grants" => "review this project's grants and rules",
        "/mcp" => {
            "manage MCP servers: open the manager, or run a verb (login, reconnect, enable, exposure)"
        }
        "/settings" => "show the merged configuration and where each value came from",
        "/session" => "show this session's tokens, cache, and cost",
        _ => "",
    }
}
