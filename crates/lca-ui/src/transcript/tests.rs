//! Transcript tests: every rendering and state contract, kept with the
//! module they assert (`#[cfg(test)] mod tests` in `mod.rs`).

use super::*;
use lca_tui::engine::text::strip_terminal_sequences;

fn plain() -> Theme {
    Theme::plain()
}

fn strip(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_terminal_sequences(l)).collect()
}

#[test]
fn user_prompts_render_with_a_marker() {
    let mut t = Transcript::new();
    t.push_user("hello there");
    let out = strip(&t.render(40, &plain()));
    let row = out
        .iter()
        .find(|l| l.contains("› "))
        .expect("the marker row");
    assert!(row.contains("hello there"), "{out:?}");
}

// Verifies: R3 - a fenced block in a real answer carries the syntax
// roles pi's palette defines: comment, keyword, and string are three
// different colors inside one block.
#[test]
fn a_fenced_rust_block_carries_the_syntax_roles() {
    let theme = Theme::colored();
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("```rust\n// hi\nfn main() { let s = \"x\"; }\n```");
    t.finish_assistant();
    let joined = t.render(80, &theme).join("\n");
    // syntaxComment #6a9955, syntaxKeyword #569cd6, syntaxString #ce9178
    assert!(joined.contains("38;2;106;153;85"), "comment: {joined}");
    assert!(joined.contains("38;2;86;156;214"), "keyword: {joined}");
    assert!(joined.contains("38;2;206;145;120"), "string: {joined}");
    assert!(
        joined.contains("38;2;220;220;170"),
        "`main(` is a function: {joined}"
    );
}

// Verifies: FR-UI-22 - the key toggles the newest run that has
// reasoning, not merely the newest assistant message: a tool report
// after a thinking run must not swallow the key while a
// `ctrl+t to expand` marker is on screen.
#[test]
fn the_thinking_toggle_skips_a_run_without_reasoning() {
    let mut t = Transcript::new();
    t.append_reasoning("one\ntwo\nthree\nfour");
    t.append_text("the answer");
    t.finish_assistant();
    // A later assistant message with no reasoning of its own.
    t.append_text("just a tool report");
    t.finish_assistant();

    let before = strip(&t.render(60, &plain()));
    assert!(
        before.iter().any(|l| l.contains("+1 lines")),
        "the snippet is collapsed: {before:?}"
    );
    t.toggle_thinking_expanded();
    let after = strip(&t.render(60, &plain()));
    assert!(
        after.iter().any(|l| l.contains("four")),
        "the run with reasoning expanded: {after:?}"
    );
    assert!(
        !after.iter().any(|l| l.contains("+1 lines")),
        "the marker is gone: {after:?}"
    );
    // And once there is no reasoning anywhere, the key is a no-op.
    let mut empty = Transcript::new();
    empty.append_text("no thinking here");
    empty.finish_assistant();
    let before = empty.render(60, &plain());
    empty.toggle_thinking_expanded();
    assert_eq!(
        empty.render(60, &plain()),
        before,
        "no reasoning, nothing to expand"
    );
}

// Verifies: cycle 9 - a heading reaches the transcript in `mdHeading`
// (it used to come out in the body color, because the markdown bold
// carried a color of its own), and a blockquote paints its text in
// `mdQuote` + italic instead of coloring only the border glyph.
#[test]
fn headings_and_quotes_carry_their_roles() {
    let theme = Theme::colored();
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("## Title\n\n> quoted");
    t.finish_assistant();
    let rows = t.render(60, &theme);
    let joined = rows.join("\n");
    assert!(
        joined.contains("38;2;240;198;116"),
        "mdHeading #f0c674 on the heading: {joined}"
    );
    let quote = rows
        .iter()
        .find(|l| l.contains('\u{2502}'))
        .expect("the quote border row");
    assert!(
        quote.contains("\x1b[3m"),
        "the quote's text is italic: {quote:?}"
    );
    assert!(
        quote.matches("38;2;128;128;128").count() >= 1,
        "the quote text is mdQuote-colored: {quote:?}"
    );
}

// Verifies: R1 - "Tool name/args lines get the roles pi gives them",
// read off pi's own renderers: the name is `toolTitle` **plus bold**
// (`renderers/read.ts`, `tool-execution.ts`), a path argument is
// `accent` (`renderToolPath`), a shell command rides inside the bold
// title (`renderers/bash.ts` `formatShellCall`), an unknown tool's
// JSON arguments stay plain, and the editor's `!` run titles itself in
// `bashMode` the way pi's `BashExecutionComponent` does.
#[test]
fn tool_card_titles_and_arguments_carry_the_roles_pi_gives() {
    let theme = Theme::colored();
    // Match on the stripped row: the marker is styled, so "> " is not
    // contiguous in the raw bytes.
    let header_of = |t: &Transcript| {
        t.render(80, &theme)
            .into_iter()
            .find(|row| strip_terminal_sequences(row).trim_start().starts_with("> "))
            .expect("the card header row")
    };

    // read: bold toolTitle name, accent path, plain status text.
    let mut t = Transcript::new();
    t.start_tool("read", r#"{"path":"src/main.rs"}"#);
    t.finish_tool(ToolStatus::Ok, Some("1  fn main() {}".into()));
    let header = header_of(&t);
    assert!(
        header.contains("\x1b[1;38;2;212;212;212mread\x1b[22;39m"),
        "the name is toolTitle + bold: {header:?}"
    );
    assert!(
        header.contains("\x1b[38;2;138;190;183msrc/main.rs\x1b[39m"),
        "the path argument is accent: {header:?}"
    );

    // A model-requested shell call: name and command in one bold title.
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"ls -la"}"#);
    t.finish_tool(ToolStatus::Ok, Some("total 0".into()));
    let header = header_of(&t);
    assert!(
        header.contains("\x1b[1;38;2;212;212;212mshell ls -la\x1b[22;39m"),
        "the command rides in the bold title: {header:?}"
    );

    // The editor's own `!` run: bashMode, bold (pi's bash component).
    let mut t = Transcript::new();
    t.start_manual_tool("bash", r#"{"command":"ls -la"}"#);
    t.finish_tool(ToolStatus::Ok, Some("total 0".into()));
    let header = header_of(&t);
    assert!(
        header.contains("\x1b[1;38;2;181;189;104mbash ls -la\x1b[22;39m"),
        "the editor's run titles itself in bashMode: {header:?}"
    );

    // An unknown tool: bold name, plain JSON arguments (pi's card
    // fallback prints them unstyled).
    let mut t = Transcript::new();
    t.start_tool("frobnicate", r#"{"target":"x"}"#);
    t.finish_tool(ToolStatus::Ok, Some("ok".into()));
    let header = header_of(&t);
    assert!(
        header.contains("\x1b[1;38;2;212;212;212mfrobnicate\x1b[22;39m"),
        "the name is still toolTitle + bold: {header:?}"
    );
    assert!(
        header.contains(r#"{"target":"x"}"#) && !header.contains("38;2;138;190;183"),
        "an unknown tool's arguments are plain: {header:?}"
    );
}

// Verifies: R1 - the user prompt renders as a full-width
// `userMessageBg` band: every row painted on the background role, each
// exactly the render width, with the marker row inside it.
#[test]
fn the_user_message_is_a_full_width_band() {
    let theme = Theme::colored();
    let width = 40usize;
    let mut t = Transcript::new();
    t.push_user("hello there");
    let rows = t.render(width as u16, &theme);
    let bg = "\x1b[48;2;52;53;65m"; // #343541, pi's dark `userMessageBg`
    assert!(
        rows.iter().all(|r| r.contains(bg)),
        "every band row is on the user background: {rows:?}"
    );
    for row in &rows {
        assert_eq!(
            lca_tui::engine::text::visible_width(row),
            width,
            "the band fills the row: {row:?}"
        );
    }
    assert!(
        rows.iter()
            .any(|r| r.contains("› ") && r.contains("hello there")),
        "the marker sits inside the band: {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r.ends_with("\x1b[49m")),
        "the band closes its own channel, so the row beside it is clean"
    );
    // Assistant text stays on the default background (pi's choice -
    // the contrast is the point).
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("an answer");
    t.finish_assistant();
    assert!(
        t.render(width as u16, &theme)
            .iter()
            .all(|r| !r.contains(bg)),
        "assistant text is not banded"
    );
}

// Verifies: R1 - a tool card's background is its state: pending while
// the call is in flight, the quiet success tint when it settled, the
// error tint when it did not (pi's `updateDisplay`).
#[test]
fn tool_cards_carry_the_states_background() {
    let theme = Theme::colored();
    let paint = |status: ToolStatus, result: Option<&str>| {
        let mut t = Transcript::new();
        t.start_tool("read", r#"{"path":"a.rs"}"#);
        t.finish_tool(status, result.map(str::to_string));
        t.render(60, &theme)
    };
    let pending = paint(ToolStatus::Running, Some("partial"));
    let ok = paint(ToolStatus::Ok, Some("done"));
    let failed = paint(ToolStatus::Error, Some("boom"));
    let refused = paint(ToolStatus::Denied, Some("denied"));
    // #282832, #283228, #3c2828
    for (rows, expected) in [
        (&pending, "\x1b[48;2;40;40;50m"),
        (&ok, "\x1b[48;2;40;50;40m"),
        (&failed, "\x1b[48;2;60;40;40m"),
        (&refused, "\x1b[48;2;60;40;40m"),
    ] {
        assert!(
            rows.iter().all(|r| r.contains(expected)),
            "every card row carries {expected}: {rows:?}"
        );
        assert!(
            rows.iter()
                .all(|r| { lca_tui::engine::text::visible_width(r) == 60 }),
            "the card fills each row: {rows:?}"
        );
    }
    assert!(
        pending.iter().any(|r| r.contains("…")),
        "the pending state carries its own symbol, not just a color (NFR-28): {pending:?}"
    );
    assert!(ok.iter().any(|r| r.contains("ok")), "{ok:?}");
    assert!(failed.iter().any(|r| r.contains("error")), "{failed:?}");
    assert!(
        refused.iter().any(|r| r.contains("denied")),
        "a refusal names itself: {refused:?}"
    );
}

// Verifies: FR-UI-5 - the plain theme paints no background at all, so
// an 80-column colorless terminal reads the same bands as text.
#[test]
fn the_plain_theme_paints_no_band() {
    let mut t = Transcript::new();
    t.push_user("hi");
    t.start_tool("read", r#"{"path":"a.rs"}"#);
    t.finish_tool(ToolStatus::Ok, Some("ok".into()));
    let rows = strip(&t.render(50, &plain()));
    assert!(rows.iter().any(|r| r.contains("› hi")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("> read")), "{rows:?}");
    let raw = t.render(50, &plain());
    assert!(
        raw.iter().all(|r| !r.contains('\x1b')),
        "the plain theme emits no SGR at all: {raw:?}"
    );
}

#[test]
fn assistant_markdown_renders() {
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("# Title\n\n- a\n- b");
    t.finish_assistant();
    let out = strip(&t.render(40, &plain()));
    assert!(out.iter().any(|l| l.trim() == "Title"), "{out:?}");
    assert!(
        out.iter().any(|l| l.trim_start().starts_with("- a")),
        "{out:?}"
    );
}

// Verifies: FR-UI-22 (R6) - a thinking run shows a short snippet by
// default: the first few non-empty lines, then a count of the rest.
#[test]
fn reasoning_shows_a_snippet_by_default() {
    let mut t = Transcript::new();
    t.append_reasoning("one\ntwo\nthree\nfour\nfive");
    t.append_text("the answer");
    t.finish_assistant();
    let lines = strip(&t.render(40, &plain()));
    // Quiet indented lines, no cute symbols (W2, issue #14)
    let shown: Vec<&String> = lines
        .iter()
        .filter(|l| {
            l.starts_with("  ")
                && (l.contains("one")
                    || l.contains("two")
                    || l.contains("three")
                    || l.contains("… +"))
        })
        .collect();
    assert_eq!(
        shown.len(),
        4,
        "three lines plus the continuation: {lines:?}"
    );
    assert!(shown[0].contains("one"), "{lines:?}");
    assert!(shown[2].contains("three"), "{lines:?}");
    assert!(
        shown[3].contains("… +2 lines") && shown[3].contains("ctrl+t to expand"),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("four")), "{lines:?}");
}

// Verifies: FR-UI-22 (R6) - the toggle expands the run it is on, in
// place, to the full block; the answer below it is untouched.
#[test]
fn the_thinking_toggle_expands_the_run_in_place() {
    let mut t = Transcript::new();
    t.append_reasoning("one\ntwo\nthree\nfour\nfive");
    t.append_text("the answer");
    t.finish_assistant();
    t.toggle_thinking_expanded();
    let lines = strip(&t.render(40, &plain()));
    for expected in ["one", "two", "three", "four", "five"] {
        assert!(
            lines.iter().any(|l| l.contains(expected)),
            "{expected} after expanding: {lines:?}"
        );
    }
    assert!(
        !lines.iter().any(|l| l.contains("+2 lines")),
        "no marker once expanded: {lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("the answer")));
}

// Verifies: W3 (issue #16) - during streaming, reasoning follows the tail:
// the visible window displays the latest lines as they arrive.
#[test]
fn reasoning_follows_tail_while_streaming() {
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_reasoning("line 1\nline 2\nline 3\nline 4\nline 5\nline 6");
    // While streaming:
    let streaming_lines = strip(&t.render(40, &plain()));
    assert!(
        streaming_lines.iter().any(|l| l.contains("earlier lines")),
        "header indicates earlier lines in tail mode: {streaming_lines:?}"
    );
    assert!(
        streaming_lines.iter().any(|l| l.contains("line 6")),
        "latest line visible: {streaming_lines:?}"
    );
    assert!(
        streaming_lines.iter().any(|l| l.contains("line 5")),
        "latest line visible: {streaming_lines:?}"
    );
    assert!(
        streaming_lines.iter().any(|l| l.contains("line 4")),
        "latest line visible: {streaming_lines:?}"
    );
    assert!(
        !streaming_lines.iter().any(|l| l.contains("line 1")),
        "earliest line hidden in tail mode: {streaming_lines:?}"
    );

    // When streaming finishes: settled view shows head snippet + expansion hint
    t.finish_assistant();
    let finished_lines = strip(&t.render(40, &plain()));
    assert!(
        finished_lines.iter().any(|l| l.contains("line 1")),
        "head line visible: {finished_lines:?}"
    );
    assert!(
        finished_lines.iter().any(|l| l.contains("… +3 lines")),
        "expansion hint present: {finished_lines:?}"
    );
}

// Verifies: FR-UI-22 (R6) - `full` and `hidden` are settings values:
// each renders its shape with no per-run toggle involved.
#[test]
fn thinking_visibility_full_and_hidden_are_settings() {
    let mut full = Transcript::new();
    full.set_thinking_visibility(ThinkingVisibility::Full);
    full.append_reasoning("alpha\nbeta\ngamma\ndelta");
    full.append_text("answer");
    full.finish_assistant();
    let lines = strip(&full.render(40, &plain()));
    assert!(lines.iter().any(|l| l.contains("delta")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("+1 lines")), "{lines:?}");

    let mut hidden = Transcript::new();
    hidden.set_thinking_visibility(ThinkingVisibility::Hidden);
    hidden.append_reasoning("alpha\nbeta");
    hidden.append_text("answer");
    hidden.finish_assistant();
    let lines = strip(&hidden.render(40, &plain()));
    assert!(lines.iter().any(|l| l.contains("Thinking…")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("alpha")), "{lines:?}");
}

// Verifies: FR-UI-22 (R6) - the toggle is per run (pi's
// thinkingVisibilityOverrides): expanding the latest run leaves the
// earlier one on the configured default.
#[test]
fn the_thinking_toggle_only_changes_the_latest_run() {
    let mut t = Transcript::new();
    t.append_reasoning("first run");
    t.append_text("answer one");
    t.finish_assistant();
    t.append_reasoning("second run");
    t.append_text("answer two");
    t.finish_assistant();
    t.toggle_thinking_expanded();
    let lines = strip(&t.render(40, &plain()));
    // The second run's reasoning is now shown as-is; the first run's
    // still shows as a snippet (one line, so no marker).
    assert_eq!(
        lines.iter().filter(|l| l.contains("second run")).count(),
        1,
        "{lines:?}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("first run")).count(),
        1,
        "{lines:?}"
    );
}

// Verifies: R15 - the per-entry render cache never serves stale lines.
#[test]
fn the_render_cache_reflects_appends_and_finishes() {
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("first");
    let a = strip(&t.render(40, &plain()));
    assert!(a.iter().any(|l| l.contains("first")));
    t.append_text(" second");
    let b = strip(&t.render(40, &plain()));
    assert!(b.iter().any(|l| l.contains("first second")), "{b:?}");
    t.start_tool("read", r#"{"path":"a"}"#);
    t.finish_tool(ToolStatus::Ok, Some("ok".into()));
    t.toggle_tools_expanded();
    let c = strip(&t.render(40, &plain()));
    assert!(c.iter().any(|l| l.contains("ok")), "{c:?}");
}

#[test]
fn tool_cards_name_the_tool_not_the_call_id() {
    let mut t = Transcript::new();
    t.start_tool("read", r#"{"path":"a.rs"}"#);
    t.finish_tool(ToolStatus::Ok, Some("ok".into()));
    let out = strip(&t.render(60, &plain()));
    let header = out
        .iter()
        .find(|l| l.contains("> read"))
        .expect("the card header row");
    assert!(header.contains("ok"), "{out:?}");
    assert!(!header.contains("call_"));
}

// Verifies: R8 - a tool card collapses to one line and expands on demand.
#[test]
fn a_tool_card_collapses_and_expands() {
    let mut t = Transcript::new();
    t.start_tool("read", r#"{"path":"a.rs"}"#);
    t.finish_tool(
        ToolStatus::Ok,
        Some("line one\nline two\nline three".into()),
    );
    let collapsed = strip(&t.render(60, &plain()));
    assert!(
        collapsed.iter().any(|l| l.contains("> read a.rs ok")),
        "{collapsed:?}"
    );
    assert!(collapsed.iter().any(|l| l.contains("ctrl+o to expand")));
    assert!(!collapsed.iter().any(|l| l.contains("line two")));
    t.toggle_tools_expanded();
    let expanded = strip(&t.render(60, &plain()));
    assert!(expanded.iter().any(|l| l.contains("line two")));
}

// Verifies: R8 - per-tool one-line argument summaries.
#[test]
fn tool_arguments_render_as_a_one_line_summary() {
    assert_eq!(format_tool_args("read", r#"{"path":"a.rs"}"#), "a.rs");
    assert_eq!(
        format_tool_args("bash", r#"{"command":"ls -la"}"#),
        "ls -la"
    );
    assert_eq!(
        format_tool_args("grep", r#"{"pattern":"foo","path":"."}"#),
        "/foo/"
    );
    assert_eq!(format_tool_args("list", r#"{"path":"."}"#), ".");
    assert_eq!(format_tool_args("other", "raw"), "raw");
}

// Verifies: R8 - a command card shows a bounded preview when collapsed.
#[test]
fn a_shell_card_previews_its_output() {
    let mut t = Transcript::new();
    t.start_tool("shell", r#"{"command":"ls"}"#);
    let output: String = (1..=8).map(|i| format!("line {i}\n")).collect();
    t.finish_tool(ToolStatus::Ok, Some(output));
    let out = strip(&t.render(60, &plain()));
    assert!(out.iter().any(|l| l.contains("> shell ls ok")), "{out:?}");
    assert!(out.iter().any(|l| l.contains("line 1")), "{out:?}");
    assert!(out.iter().any(|l| l.contains("line 5")), "{out:?}");
    assert!(!out.iter().any(|l| l.contains("line 6")), "{out:?}");
    assert!(out.iter().any(|l| l.contains("3 more lines")), "{out:?}");
}

// Verifies: R8 - a read card stays one line when collapsed.
#[test]
fn a_read_card_stays_one_line() {
    let mut t = Transcript::new();
    t.start_tool("read", r#"{"path":"a.rs"}"#);
    t.finish_tool(ToolStatus::Ok, Some("1  fn main() {}\n2  more\n".into()));
    let out = strip(&t.render(60, &plain()));
    assert!(out.iter().any(|l| l.contains("> read a.rs ok")), "{out:?}");
    assert!(out.iter().any(|l| l.contains("ctrl+o to expand")));
    assert!(!out.iter().any(|l| l.contains("fn main")), "{out:?}");
}

#[test]
fn entries_are_separated_by_a_blank_line() {
    let mut t = Transcript::new();
    t.push_user("q");
    t.append_text("a");
    t.finish_assistant();
    let out = t.render(40, &plain());
    assert!(out.iter().any(|l| l.is_empty()));
}

#[test]
fn streaming_marker_disappears_when_finished() {
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("partial");
    let streaming = strip(&t.render(40, &plain()));
    assert!(streaming.iter().any(|l| l.contains("▍")));
    t.finish_assistant();
    let done = strip(&t.render(40, &plain()));
    assert!(!done.iter().any(|l| l.contains("▍")));
}

// Verifies: FR-CORE-11 - a steered user message ends the assistant
// message before it, so no stale streaming marker survives the
// boundary (a turn split by a steer has more than one assistant entry).
#[test]
fn a_steer_finalizes_the_assistant_before_it() {
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("first call");
    t.push_user("steer");
    t.append_text("second call");
    let out = strip(&t.render(60, &plain()));
    let streaming = out.iter().filter(|l| l.contains('▍')).count();
    assert_eq!(streaming, 1, "only the live assistant streams:\n{out:?}");
}

// Verifies: FR-UI-13 - an image renders with its media type and
// dimensions, never silently dropped.
#[test]
fn image_entries_render_a_placeholder() {
    let mut t = Transcript::new();
    t.push_image(
        ImageInfo {
            media_type: "image/png".into(),
            bytes: 1024,
            width: Some(10),
            height: Some(20),
            alt: Some("a chart".into()),
        },
        Vec::new(),
    );
    let out = strip(&t.render(60, &plain()));
    assert!(
        out.iter()
            .any(|l| l.contains("image/png") && l.contains("10×20") && l.contains("a chart")),
        "{out:?}"
    );
}

// Verifies: TUI-10 M1 row 27 - the user's own message renders as markdown
// with pi's preserve options (`user-message.ts`): emphasis shows as
// emphasis instead of asterisks, and the authored `1)` marker survives.
#[test]
fn user_prompts_render_as_markdown_with_the_authored_markers() {
    let mut t = Transcript::new();
    t.push_user("**bold** and 1) first");
    let out = strip(&t.render(60, &plain()));
    let row = out
        .iter()
        .find(|l| l.contains("› "))
        .expect("the marker row");
    assert!(row.contains("bold"), "emphasis rendered: {out:?}");
    assert!(!row.contains("**"), "no raw asterisks: {out:?}");
    assert!(row.contains("1) first"), "the authored marker: {out:?}");

    // A list in a user message renders as a list, inside the band.
    let mut t = Transcript::new();
    t.push_user("- one");
    let out = strip(&t.render(60, &plain()));
    assert!(out.iter().any(|l| l.contains("- one")), "{out:?}");
}

// Verifies: TUI-10 M4/M6 - a mermaid diagram in an answer paints through
// the theme's diagram roles (borderMuted borders, accent edges), not as
// a code frame.
#[test]
fn a_mermaid_diagram_paints_through_the_theme() {
    let theme = Theme::colored();
    let mut t = Transcript::new();
    t.begin_assistant();
    t.append_text("```mermaid\nflowchart TD\n  A[One] --> B[Two]\n```");
    t.finish_assistant();
    let joined = t.render(70, &theme).join("\n");
    assert!(joined.contains('┌'), "art box: {joined}");
    // borderMuted #808080, accent #8aa7b7-family: pi's roles as LCA paints them
    assert!(joined.contains("38;2;"), "colored: {joined}");
    assert!(!joined.contains('╭'), "not the code frame: {joined}");
}
