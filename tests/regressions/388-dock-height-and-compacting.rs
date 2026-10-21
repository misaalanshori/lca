//! GitHub #388 gap 2: the dock's height is constant, and `Compacting` is a
//! first-class separator state.
//!
//! **The property is row-count invariance across the transition**, not "an
//! idle status exists" — the latter passes while the height still changes,
//! which is precisely the defect. The dock used to gain a `running...` row the
//! moment a turn started and keep it forever, so every turn moved the whole
//! transcript by a row: the instability the anti-jitter status exists to stop.
//!
//! The behavioural rows live beside the code (`crates/lca-ui/src/dock_tests.rs`
//! and the updated `chat_overlay_tests.rs`). This file is the NFR-24 named
//! guard: it pins the *mechanism* from outside, so a refactor cannot quietly
//! restore the conditional row or move `Compacting` back out of the separator.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #388 gap 2 - the footer's status row is **unconditional**. A
// conditional row is the defect: it appears with the first turn and makes the
// dock grow by one.
#[test]
fn the_status_row_is_reserved_not_conditional() {
    // The footer became a directory in gh #389 (the git discovery and cache
    // moved beside it); the row contract is still this file's.
    let footer = read("crates/lca-ui/src/footer/mod.rs");
    let render = footer
        .find("pub fn render(&self, width: u16, theme: &Theme)")
        .expect("Footer::render exists");
    let body = &footer[render..];
    let end = body.find("\n    }\n").expect("render has a body");
    let body = &body[..end];

    // The old shape, which must not come back.
    assert!(
        !body.contains("if !self.statuses.is_empty() {"),
        "the status row must not be conditional on having something to say - \
         that is what made the dock grow by a row when a turn started"
    );
    // And the reserved row is really pushed, unconditionally.
    assert!(
        body.contains("let joined = self.statuses.join"),
        "the status row is built every frame"
    );
    assert!(
        body.contains("lines.push(if joined.is_empty()"),
        "and pushed every frame, empty or not, so the height is constant"
    );
}

// Verifies: gh #388 gap 2 - `Compacting` is a `SeparatorState` carrying its
// reason, not something handled beside the separator. Being a state is what
// gives a compacting interface a status row at all.
#[test]
fn compacting_is_a_separator_state_with_its_reason() {
    let separator = read("crates/lca-ui/src/separator.rs");
    assert!(
        separator.contains("Compacting {\n        /// Why the compaction started.\n        reason: CompactionReason,"),
        "`Compacting` carries a `CompactionReason`"
    );
    assert!(
        separator.contains("pub enum CompactionReason"),
        "the three reasons are one enum"
    );
    for word in ["manual", "threshold", "overflow"] {
        assert!(
            separator.contains(&format!("\"{word}\"")),
            "the `{word}` reason has a displayed word"
        );
    }
    // The state is rendered by the separator itself, in the row a turn uses.
    let render = separator
        .find("fn render_inner(")
        .expect("the separator renders its own row");
    assert!(
        separator[render..].contains("SeparatorState::Compacting { reason }"),
        "the separator paints the compacting row, rather than a caller \
         substituting a `Working` label"
    );
}

// Verifies: gh #388 gap 2 - the separator agrees with `WorkKind` instead of
// forming a second opinion. Both surfaces must call a compaction "working".
#[test]
fn the_separator_and_the_badge_share_one_opinion() {
    let run_status = read("crates/lca-ui/src/run_status.rs");
    assert!(
        run_status.contains("crate::status::work_state(chat.turn_running, chat.compacting"),
        "the badge/taskbar decision still reads `compacting` through `work_state`"
    );
    let status = read("crates/lca-ui/src/status.rs");
    let work_state = status
        .find("pub fn work_state(")
        .expect("work_state exists");
    let body = &status[work_state..];
    let end = body.find("\n}\n").expect("work_state has a body");
    assert!(
        body[..end].contains("compact"),
        "`work_state` counts a compaction as work, which the separator must agree with"
    );
}
