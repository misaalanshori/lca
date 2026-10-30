//! Gh #323: four engine-robustness defects, guarded so they cannot come back.
//!
//! The behavioural rows live beside the code (`engine/lru.rs`, `engine/text.rs`,
//! `engine/keys.rs`, `engine/sys.rs`). This file is the NFR-24 guard, and it covers
//! what a behavioural row cannot see because it measures the *current* code:
//!
//! 1. **one eviction policy in the codebase**, not three. Two hand-rolled caches
//!    evicted `keys().next()` - `HashMap` hash order, so the victim was arbitrary.
//!    A row proving the shared type behaves still passes if a new cache hand-rolls
//!    its own eviction, so the guard names the pattern instead.
//! 2. **the Kitty flag is not process-global.** A row proves two managers *can*
//!    disagree; this proves no new static was introduced that would make them agree
//!    again, which is the `thread_local` trap the brief names.
//! 3. **the Windows halves report rather than swallow.** `SetConsoleMode`'s return is
//!    checked and a non-key console record's kind reaches the loop. Both are
//!    Windows-only at the OS call, so the *shape* is what a Linux row can hold.
//!
//! # What is deliberately not guarded here
//!
//! `extensions/**`, `wit/**`, `lca-ui/**` and `lca-tui/src/widgets/**` are outside
//! this cycle's boundary, and one of them - `widgets/mermaid.rs`'s parse cache - is a
//! third instance of the eviction pattern this file's first check names. It is
//! reported rather than fixed or asserted, because asserting it would fail on a file
//! this cycle may not edit.

use std::path::PathBuf;

/// The repository root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a file from the repository root.
fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The body of `fn <name>` in `src`, from its `{` to the matching `}`.
///
/// `name` must end where a declaration does, so `render` cannot match
/// `render_region`. A guard that inspects the wrong function passes vacuously.
fn function_body(src: &str, name: &str) -> String {
    let needle = format!("fn {name}");
    let start = src
        .match_indices(&needle)
        .find(|(at, _)| matches!(src[at + needle.len()..].chars().next(), Some('(' | '<')))
        .map(|(at, _)| at)
        .unwrap_or_else(|| panic!("`{name}` is declared"));
    let open = start
        + src[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{name}` has a body"));
    let mut depth = 0usize;
    let mut end = src.len();
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > open, "`{name}`'s body is closed");
    src[open..=end].to_string()
}

/// The `.rs` files under `dir`, recursively.
fn rust_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

// Verifies: gh #323 defect 1 - the engine has **one** eviction policy.
//
// Three caches existed and two of them evicted `keys().next()`, which is `HashMap`
// hash order: the victim was arbitrary, so a hot entry could be dropped by the next
// cold insert exactly when the cache was working hardest. `LruCache` now states the
// policy once. This row fails if a cache in the engine grows its own eviction
// again - which a behavioural row cannot catch, because the second copy would behave
// correctly for as long as nobody adds a third.
#[test]
fn the_engine_has_one_cache_eviction_policy() {
    let engine = repo_root().join("crates/lca-tui/src/engine");
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_files(&engine) {
        let rel = file
            .strip_prefix(repo_root())
            .unwrap_or(&file)
            .display()
            .to_string()
            .replace('\\', "/");
        // The policy's own implementation is where `keys().next()` is allowed - it
        // does not call it, but its eviction is the definition being shared.
        if rel.ends_with("engine/lru.rs") || rel.ends_with("engine/lru_tests.rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let code: String = src
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // The arbitrary-eviction idiom: take whatever the map yields first.
        if code.contains("keys().next()") {
            offenders.push(format!("{rel}: evicts `keys().next()` (hash order)"));
        }
    }
    assert!(
        offenders.is_empty(),
        "gh #323: every cache must use `engine::lru::LruCache` rather than picking \
         its own victim:\n  {}",
        offenders.join("\n  ")
    );

    // And the shared type is actually reachable from the two caches that exist.
    let text = read("crates/lca-tui/src/engine/text.rs");
    assert!(
        text.contains("lru::LruCache"),
        "the width cache uses the shared policy"
    );
    let lru = read("crates/lca-tui/src/engine/lru.rs");
    assert!(
        lru.contains("fn evict_least_recent"),
        "and the policy has exactly one eviction implementation"
    );
    assert!(
        lru.contains("min_by_key(|(_, entry)| entry.stamp)"),
        "whose victim is the lowest recency stamp - a comparison, not hash order"
    );
}

// Verifies: gh #323 defect 2 - the Kitty flag is not process-global in the matcher.
//
// The row that matters (`two_managers_disagree_about_kitty_simultaneously`) proves
// two managers can disagree; this one proves nothing re-introduces the agreement.
// A `thread_local!` would pass that row and fail real use, so the ban is on the
// *ambient read being the only answer*, not on the static's existence - the static
// is the documented fallback for a match with no terminal attached.
#[test]
fn the_key_matcher_reads_a_parameter_not_only_a_global() {
    let keys = read("crates/lca-tui/src/engine/keys.rs");
    assert!(
        keys.contains("pub fn matches_key_with_kitty("),
        "there is an explicit-state entry point"
    );
    let inner = function_body(&keys, "matches_key_inner");
    assert!(
        !inner.contains("is_kitty_protocol_active()"),
        "and the matcher body reads its parameter, never the ambient flag:\n{inner}"
    );
    assert!(
        inner.contains("kitty_protocol"),
        "the parameter is what the mode-aware branches use:\n{inner}"
    );

    let bindings = read("crates/lca-tui/src/engine/keybindings.rs");
    assert!(
        bindings.contains("fn set_kitty_state("),
        "a manager attaches its own terminal's cell"
    );
    let matches = function_body(&bindings, "matches");
    assert!(
        matches.contains("kitty_state"),
        "and `matches` consults that cell:\n{matches}"
    );
    assert!(
        !matches.contains("is_kitty_protocol_active()"),
        "not the process-wide flag:\n{matches}"
    );
    // The signature is unchanged on purpose: 44 call sites live in `lca-ui`, which
    // this cycle must not edit.
    assert!(
        bindings.contains("pub fn matches(&self, data: &str, action: &str) -> bool"),
        "the public signature stays, so no caller had to change"
    );
}

// Verifies: gh #323 defect 2, the terminal half - negotiation reaches the matcher
// through the terminal's own cell, and a real terminal no longer writes the
// process-global flag.
//
// That write is exactly how one pane's teardown used to deafen another pane's
// matcher: both read one static, so whichever negotiated last won.
#[test]
fn a_terminal_reports_its_own_kitty_cell_and_leaves_the_global_alone() {
    let terminal = read("crates/lca-tui/src/engine/terminal.rs");
    assert!(
        terminal.contains("fn kitty_state(&self)"),
        "the terminal exposes its cell"
    );
    let code: String = terminal
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("set_kitty_protocol_active"),
        "and never writes the process-global flag: one pane must not change another's \
         key interpretation"
    );
    assert!(
        terminal.contains("kitty: std::sync::Arc<AtomicBool>"),
        "the cell is an `Arc`, so a manager and the reader thread share one value"
    );
}

// Verifies: gh #323 defect 3 - every `SetConsoleMode` return is checked, and a
// refusal becomes a reported error rather than a silent no-op.
//
// Windows-only at the OS call, so this row holds the **shape**: no bare
// `unsafe { SetConsoleMode(..) };` statement survives, and the refusal builder
// carries the handle name and the code. The call itself is untested on this box and
// the report says so.
#[test]
fn a_refused_console_mode_is_checked_and_reported() {
    let sys = read("crates/lca-tui/src/engine/sys.rs");
    let code: String = sys
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // A checked call is `if SetConsoleMode(..) == 0` or a `!= 0` test; a bare
    // statement ends `};` right after the call's closing paren.
    assert!(
        !code.contains("SetConsoleMode(stdin, new_mode) };")
            && !code.contains(
                "SetConsoleMode(stdout, stdout_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };"
            ),
        "the mode-change results must not be discarded"
    );
    assert!(
        code.contains("if unsafe { SetConsoleMode(stdin, new_mode) } == 0"),
        "the stdin call's return is checked"
    );
    assert!(
        code.contains(
            "== 0\n            {\n                return Err(set_mode_error(\"stdout\"));"
        ),
        "and so is the stdout call's"
    );
    assert!(
        sys.contains("fn set_mode_refusal(which: &str, code: i32)"),
        "a refusal is built with its handle name and OS code"
    );
    assert!(
        sys.contains("refused raw mode (os error {code})"),
        "and the message carries the code, which is the only actionable part"
    );
    // Not a panic: a production path may not abort on a degraded console.
    let refusal = function_body(&sys, "set_mode_refusal");
    assert!(
        !refusal.contains("panic!") && !refusal.contains("unwrap"),
        "reporting must not become a panic:\n{refusal}"
    );
}

// Verifies: gh #323 defect 4 - a non-key console record's **kind** reaches the loop
// instead of being swallowed.
//
// The defect: `wait_stdin` consumed every non-key record with `ReadConsoleInputW` and
// looped, so `WINDOW_BUFFER_SIZE_EVENT` was read and thrown away and the loop learned
// the new size only on its next timer tick. The fix reports the outcome, so the shape
// a Linux row can hold is: an outcome type with a resize case, a classification
// predicate, and a caller that acts rather than discards.
#[test]
fn a_non_key_console_record_reaches_the_loop() {
    let sys = read("crates/lca-tui/src/engine/sys.rs");
    assert!(
        sys.contains("pub(crate) enum WaitOutcome"),
        "the wait reports what it found rather than a bare bool"
    );
    assert!(
        sys.contains("fn console_record_meaning(event_type: u16) -> bool"),
        "the record classification is its own testable predicate"
    );
    let classify = function_body(&sys, "console_record_meaning");
    assert!(
        classify.contains("WINDOW_BUFFER_SIZE_EVENT_TYPE"),
        "which keys on the buffer-size record:\n{classify}"
    );
    // The Windows backend asks the predicate rather than hard-coding the constant,
    // so the row above covers the real path and not a copy of it.
    //
    // There are **two** `fn wait_stdin`: the dispatching one near the top and the
    // console backend inside `mod windows_backend`. `function_body` finds the first,
    // which is the unix/`cfg` dispatcher and legitimately knows nothing about console
    // records - the guard's first version asserted against it and failed. The
    // backend's is the one with the `super::WaitOutcome` return type.
    let backend_at = sys
        .find("fn wait_stdin(timeout_ms: i32) -> io::Result<super::WaitOutcome>")
        .expect("the Windows backend's wait_stdin is declared");
    let wait = function_body(&sys[backend_at..], "wait_stdin");
    assert!(
        wait.contains("console_record_meaning"),
        "and the console backend consults it:\n{wait}"
    );

    let terminal = read("crates/lca-tui/src/engine/terminal.rs");
    let reader = function_body(&terminal, "reader_loop");
    assert!(
        !reader.contains("Ok(true)") && !reader.contains("Ok(false)"),
        "the reader no longer matches a bool, so a resize cannot fall into a \
         catch-all:\n{reader}"
    );
    assert!(
        reader.contains("outcome.is_key()"),
        "it distinguishes a key from everything else:\n{reader}"
    );
}
