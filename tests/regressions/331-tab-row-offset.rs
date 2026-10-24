//! gh #331 — one subtraction, not four. NFR-24's guard.
//!
//! The defect was four sites that each turned a terminal height into a transcript
//! window, and three of them had forgotten that the fullscreen tab bar takes a row.
//! The behaviour rows (`crates/lca-ui/src/chat_offset_tests.rs`) prove the four
//! symptoms are gone; this file guards the *design*, because the symptoms can all be
//! green while a fifth copy of the arithmetic waits to be written.
//!
//! Two properties:
//!
//! 1. **There is exactly one function that does the subtraction**, and it takes the
//!    tab rows off too. No other call site in the crate may compute
//!    `height - dock` - a `grep` is the acceptance test the brief asked for, and it
//!    goes from four hits to one.
//! 2. **The tab term is conditional on the same fact the renderer uses.** The
//!    scrollback renderer has no tab bar above the transcript (it is the dock's last
//!    row there), so subtracting it again would be the mirror of the original bug in
//!    the *default* renderer. The subtraction must therefore go through
//!    `tab_top`, which answers zero off fullscreen - not through `tab_bar_alt`
//!    directly.

use std::path::Path;

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The non-test source of the UI crate, so a scan does not trip on fixtures.
fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-ui/src");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path.display().to_string(), text));
            }
        }
    }
    out
}

// Verifies: gh #331 - exactly one place turns a terminal height into a transcript
// window, and it is named for what it returns.
//
// The brief's acceptance test for the design: "no call site may compute
// `height.saturating_sub(dock.len()...)` itself". Before this change four functions
// did (`click_at`, `window_height`, `clamp_scroll`, and `viewport` in a longer
// spelling); now the arithmetic lives only in `transcript_window`.
//
// **Matched by shape, not by one spelling.** The first version of this row looked for
// `dock.len()`, and a mutation that wrote `self.sections(width).1.len()` slipped
// through - same arithmetic, different name. So the test is: a height, saturating-
// subtracted by the length of something obtained from `sections(`. That is the
// operation the bug is about, whatever the local is called.
#[test]
fn the_window_subtraction_lives_in_exactly_one_function() {
    let mut offenders = Vec::new();
    for (path, text) in sources() {
        // The **body** of `transcript_window`, as a line range - not "the file
        // contains it", which every line of `chat_render.rs` satisfies and which made
        // the first two versions of this row pass every mutation.
        let lines: Vec<&str> = text.lines().collect();
        let window_body: Option<(usize, usize)> = lines
            .iter()
            .position(|l| l.contains("fn transcript_window("))
            .map(|start| {
                let end = lines[start..]
                    .iter()
                    .position(|l| l.trim() == "}")
                    .map(|offset| start + offset)
                    .unwrap_or(lines.len());
                (start, end)
            });
        for (index, line) in lines.iter().enumerate() {
            let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if !compact.contains("height") || !compact.contains(".saturating_sub(") {
                continue;
            }
            // The subtracted term, up to two lines back and two forward. Both
            // directions are needed because a local can be bound above the subtraction
            // (`let d = self.sections(width).1;`) exactly as often as the call chain
            // runs past it.
            let lo = index.saturating_sub(3);
            let hi = (index + 3).min(lines.len());
            let window_of_lines: String = lines[lo..hi]
                .iter()
                .map(|l| l.chars().filter(|c| !c.is_whitespace()).collect::<String>())
                .collect();
            // Structural, not nominal: the *operation* is "saturating-subtract the
            // length of the second half of `sections()` from a height". Matching local
            // names was whack-a-mole - `dock.len()`, then `d.len()`, then `.1.len()`
            // each needed a patch, and the next name would have needed another. So the
            // test is: this line subtracts *something's* `.len()`, and a `sections(`
            // call appears nearby (the dock it is the length of).
            let subtracts_a_len =
                compact.contains("saturating_sub(") && window_of_lines.contains(".len()");
            let takes_the_dock = subtracts_a_len
                && (window_of_lines.contains("sections(") || window_of_lines.contains("dock"));
            if takes_the_dock {
                // `yolo_badge_cells` legitimately subtracts the dock to place a frame
                // row; it is not a transcript window and is named here so the row
                // does not have to pretend otherwise.
                let is_badge = lines[..index]
                    .iter()
                    .rposition(|l| l.contains("fn yolo_badge_cells("))
                    .map(|at| {
                        let after = lines[at + 1..]
                            .iter()
                            .position(|l| l.trim() == "}")
                            .map(|o| at + 1 + o)
                            .unwrap_or(lines.len());
                        index <= after
                    })
                    .unwrap_or(false);
                let in_window = window_body.is_some_and(|(a, b)| index >= a && index <= b);
                if !in_window && !is_badge {
                    offenders.push(format!("{path}:{}: {}", index + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the window's subtraction is `transcript_window`'s alone; these sites compute \
         it themselves (which is how #331 got four symptoms instead of one):\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #331 - the subtraction is conditional on the renderer's own fact.
//
// `transcript_window` must reach the tab rows through `tab_top` (which answers 0 off
// fullscreen), not through `tab_bar_alt`/`tab_bar_main` directly. Reaching for the
// fullscreen bar unconditionally would shift scrollback mode - the default renderer -
// by one row, which is the mirror image of the bug this cycle fixed.
#[test]
fn the_tab_term_comes_from_the_renderer_aware_helper() {
    let source = read("crates/lca-ui/src/chat_render.rs");
    let start = source
        .find("pub(crate) fn transcript_window(")
        .expect("the one function exists");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("tab_top("),
        "the window reaches the tab rows via `tab_top`, which is zero off fullscreen: {body}"
    );
    assert!(
        !body.contains("tab_bar_alt(") && !body.contains("tab_bar_main("),
        "and not via the bar itself - the fullscreen bar is not the scrollback row: {body}"
    );
    // `tab_top` is where the mode check lives, and it says so.
    let tabs = read("crates/lca-ui/src/chat_tabs.rs");
    let top = tabs.find("pub(crate) fn tab_top(").expect("tab_top exists");
    let top_body = &tabs[top..];
    let top_body = &top_body[..top_body.find("\n    }\n").unwrap_or(top_body.len())];
    assert!(
        top_body.contains("screen_mode"),
        "`tab_top` is the mode-aware helper, so its body must check the mode: {top_body}"
    );
}

// Verifies: gh #331 - the offset is subtracted once, never added back.
//
// The brief's second warning: "no `+1` anywhere - an offset added in one place and
// subtracted in another is the bug returning under a new name." The hit-tests take
// the tab rows off the frame row; none of them may add an offset back to compensate.
#[test]
fn the_row_offset_is_subtracted_and_never_added_back() {
    for path in [
        "crates/lca-ui/src/chat_render.rs",
        "crates/lca-ui/src/chat_mouse.rs",
    ] {
        let source = read(path);
        for (index, line) in source.lines().enumerate() {
            let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            // `+ tab_top` / `+ tab_rows` is the tell: one site putting back what
            // another took off. The drawer tab legitimately *adds* `tab_top` because
            // it converts a transcript row into a frame row for the painter, so that
            // one site is named and allowed.
            let adds = compact.contains("+self.tab_top(")
                || compact.contains("+.tab_top(")
                || compact.contains("saturating_add(self.tab_top(");
            if adds {
                assert!(
                    path.ends_with("chat_mouse.rs"),
                    "only `drawer_rect` may add the tab rows back (it converts a \
                     transcript row into the frame row the painter splices on); \
                     {path}:{} adds it: {line}",
                    index + 1
                );
                assert!(
                    source.contains("fn drawer_rect("),
                    "and it really is the drawer's conversion: {line}"
                );
            }
        }
    }
}

// Verifies: gh #331 - `scroll_extent` and `window_height` are the same number.
//
// The bug survived because `viewport` was right and the hit-tests were wrong: a
// frame-level test passed while every click was off. Two functions computing "the
// window" is how that happened, so the guard is that both delegate.
#[test]
fn every_window_reader_delegates_to_the_one_function() {
    let source = read("crates/lca-ui/src/chat_render.rs");
    for name in ["fn scroll_extent(", "fn window_height(", "fn clamp_scroll("] {
        let start = source.find(name).unwrap_or_else(|| panic!("{name} exists"));
        let body = &source[start..];
        let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
        assert!(
            body.contains("transcript_window(") || body.contains("scroll_extent("),
            "{name} must read the window from the one function, not compute it: {body}"
        );
    }
    // And `viewport` - the renderer whose arithmetic was already right - reads it too,
    // so the frame and the hit-test cannot disagree again.
    let start = source.find("pub fn viewport(").expect("viewport exists");
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    assert!(
        body.contains("transcript_window("),
        "the renderer and the hit-test share one window: {body}"
    );
}
