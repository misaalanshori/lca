//! gh #573 - every `unsafe` block in `lca-tui`'s engine carries a SAFETY
//! rationale that names *why* the call is sound, and the three reader/full-loop
//! sleeps say what mechanism they are.
//!
//! The claim, re-measured: the issue names three `GetConsoleMode` lines in
//! `sys.rs` (271/272/310) and two `thread::sleep`s (472/991). Today those are
//! lines **422/423/467** and three sleeps at `598`, `635`, `1046`, so the issue's
//! line numbers are stale by ~150 and the sleep count by one. What survives the
//! re-measurement is the *shape* of the claim: `GetConsoleMode` really was the
//! undocumented set, and a comment that restates the code (`// SAFETY: we call
//! GetConsoleMode`) is not a SAFETY comment - it has to name what is
//! initialised, who owns the handle, and what the failure path does.
//!
//! **This row checks comments, not counts.** Its test is: the block has a
//! `SAFETY:` line within four lines above it, the comment is at least 8 words,
//! and it names an identifier that appears in the block itself - so
//! `// SAFETY: safe` cannot pass.
//!
//! Verifies: gh #573 acceptance 1-2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file of `lca-tui`'s engine and widget trees, minus the test
/// modules (whose own allowances are a separate, crate-level matter).
fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for tree in ["crates/lca-tui/src/engine", "crates/lca-tui/src/widgets"] {
        let dir = repo().join(tree);
        walk(&dir, &mut out);
    }
    assert!(out.len() > 10, "the engine tree parsed {} files", out.len());
    out
}

fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.to_string_lossy().to_string();
        if name.ends_with("_tests.rs") || name.contains("/tests/") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {name}: {err}"));
        out.push((name, text));
    }
}

/// Words in a chunk of comment text, for the "at least 8" floor.
fn words(text: &str) -> usize {
    text.split(|c: char| c.is_whitespace() || c == '-')
        .filter(|w| !w.is_empty())
        .count()
}

// Verifies: gh #573 - the exhaustive form: every `unsafe` usage in the tree has
// an adjacent, substantive SAFETY comment that names something in the block.
#[test]
fn every_unsafe_block_states_why_it_is_sound() {
    let mut problems = Vec::new();
    for (path, text) in sources() {
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue; // prose about `unsafe`, not an `unsafe` usage
            }
            let Some(position) = line.find("unsafe") else {
                continue;
            };
            let tail = line[position + "unsafe".len()..].trim_start();
            // Only real usages: `unsafe {`, `unsafe fn`, `unsafe impl`, `unsafe extern`.
            if !(tail.starts_with('{')
                || tail.starts_with("fn ")
                || tail.starts_with("impl ")
                || tail.starts_with("extern "))
            {
                continue;
            }
            // The rationale: the contiguous `//` block above the usage, which is
            // where a multi-line SAFETY note lives (rustfmt wraps them).
            let mut comment = String::new();
            let mut found = false;
            let mut cursor = index;
            while cursor > 0 {
                cursor -= 1;
                let above = lines[cursor].trim();
                if above.is_empty() || !above.starts_with("//") {
                    break;
                }
                if above.contains("SAFETY:") {
                    found = true;
                }
                comment.insert_str(0, above.trim_start_matches('/').trim());
                comment.insert(0, ' ');
            }
            if !found {
                problems.push(format!("{path}:{} — no SAFETY line\n   {line}", index + 1));
                continue;
            }
            if words(&comment) < 8 {
                problems.push(format!(
                    "{path}:{} — SAFETY comment under 8 words: {comment}",
                    index + 1
                ));
                continue;
            }
            // It must name an identifier the block uses (the "not theatre" test).
            let block: String = lines[index..(index + 10).min(lines.len())].join(" ");
            let names_an_identifier = block
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|word| word.len() > 2)
                .filter(|word| {
                    ![
                        "libc", "unsafe", "self", "let", "mut", "std", "fn", "use", "impl",
                    ]
                    .contains(word)
                })
                .any(|word| comment.contains(word));
            if !names_an_identifier {
                problems.push(format!(
                    "{path}:{} — SAFETY comment names nothing in the block: {comment}",
                    index + 1
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "`unsafe` blocks without a soundness rationale:\n{}",
        problems.join("\n")
    );
}

// Verifies: gh #573 - the three `thread::sleep`s each say what they are. Two are
// the gap between asynchronous cancellation attempts (bounded by a 500 ms
// deadline), one is a child-process poll interval (bounded by 250 ms); the
// comments name the mechanism, and the row also checks the bounds are still
// there, because "justify the sleep" is worthless if the deadline went away.
#[test]
fn every_reader_loop_sleep_says_what_it_is_and_what_bounds_it() {
    let terminal = std::fs::read_to_string(repo().join("crates/lca-tui/src/engine/terminal.rs"))
        .expect("read terminal.rs");
    let sleeps = terminal.matches("std::thread::sleep").count();
    assert_eq!(
        sleeps, 3,
        "the three reader/full-loop sleeps are still there (found {sleeps}); a new one \
         needs its own justification"
    );
    for (index, line) in terminal.lines().enumerate() {
        if !line.contains("std::thread::sleep") {
            continue;
        }
        // The gap between the sleep and its rationale is the loop body it sits in
        // (`while … {` then `cancel_blocking_read(..)`), so read a generous window.
        let above: Vec<&str> = terminal
            .lines()
            .rev()
            .skip(terminal.lines().count() - index)
            .take(14)
            .collect();
        let comment = above
            .iter()
            .filter(|line| line.trim_start().starts_with("//"))
            .map(|line| line.trim_start_matches('/').trim())
            .collect::<Vec<_>>()
            .join(" ");
        // The comment is *about* the sleep, so look for the mechanism words in the
        // whole rationale (which may be wrapped in any order).
        let normalized = comment.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("asynchronous")
                || normalized.contains("poll")
                || normalized.contains("interval")
                || normalized.contains("gap"),
            "the sleep at line {} does not say what mechanism it is: {comment}",
            index + 1
        );
    }
    // And the bounds that make a sleep safe to keep.
    assert_eq!(
        terminal.matches("Duration::from_millis(500)").count(),
        2,
        "both cancellation loops keep their 500 ms deadline"
    );
    assert!(
        terminal.contains("Duration::from_millis(250)"),
        "the tmux probe keeps pi's 250 ms budget"
    );
}
