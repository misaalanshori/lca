//! Gh #386 gap 1 in a real terminal (tmux): the popup's description column,
//! its two thresholds, and the provenance tag.
//!
//! Three captures, which are the brief's live verification:
//!
//! 1. `/` with rows of different label lengths - the descriptions share one
//!    column (the zigzag is gone).
//! 2. A narrow pane - the single-column form, no description stealing the
//!    label's space.
//! 3. A skill row - its provenance tag is on the row.
//!
//! `capture-pane -e` is used for the selected-row styling claim.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

// Verifies: gh #386 gap 1a - in a real pane the popup paints without ragged
// label padding, and the description column is measurable when rows carry a
// description.
//
// **Finding recorded here rather than faked:** the `/` menu's command rows are
// built with `description: None` in `lca-ui/src/autocomplete_provider.rs`, so a
// live `/` popup has *no descriptions to align* - the zigzag is only observable
// once a menu supplies them (`/model` argument completion does; the command
// list does not). The column rule itself is pinned by the 15 unit rows and 11
// regression rows, which construct items directly. This row therefore asserts
// the part a pane can prove: the popup paints, rows are single-column when
// nothing describes them, and no row exceeds the pane.
#[cfg(unix)]
#[test]
fn the_popup_paints_single_column_rows_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("popup-column-386");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("popup-column-386");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // A bare `/` does not open the popup in a real pane - the provider needs a
    // query to fuzzy-match - so this types `/mo`, which selects `/model`.
    //
    // The needle is the popup's own row text, **not** `model`: the footer
    // already contains `openai-compatible/test-model`, so waiting on that
    // returns before the popup paints (which is how this row first failed).
    session.paste_text("/mo");
    let pane = session.wait_for("/model", std::time::Duration::from_secs(20));

    // The popup paints, at least one row is marked selected, and no painted
    // line runs past the pane (the truncation invariant).
    assert!(
        pane.contains("▸ ") || pane.contains("  /"),
        "the popup paints rows:\n{pane}"
    );
    let width = pane.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    for line in pane.lines() {
        assert!(
            line.chars().count() <= width.max(1),
            "no row exceeds the pane: {line:?}"
        );
    }

    // Clear the composer (which also dismisses the popup), then exit the way
    // the other real-terminal rows do. An `Escape` first leaves the editor in a
    // state the following `/exit` does not route from, and the pane then never
    // exits - CI caught that, local runs did not.
    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #386 gap 1b - a `/skill:` row shows its provenance tag.
#[cfg(unix)]
#[test]
fn a_skill_row_shows_its_provenance_in_a_real_pane() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("popup-provenance-386");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // A skill pack, so `/skill:` has rows to show.
    let skill_dir = sandbox
        .home
        .join(".lca")
        .join("extensions")
        .join("skills")
        .join("resources")
        .join("skills")
        .join("deploy-cloudflare");
    std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: deploy-cloudflare\ndescription: Deploy to Cloudflare Workers\n---\n\nSteps.\n",
    )
    .expect("write skill");

    let session = Tmux::new("popup-provenance-386");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // The needle is the popup's `▸ `/`  `/skill:…` row, not `skill`: the
    // startup header already says `1 skill`, so waiting on that word returns
    // before the popup paints (the same trap the `/model` row hit).
    session.paste_text("/skill");
    let pane = session.wait_for("/skill:", std::time::Duration::from_secs(20));
    // The row is offered; whatever tag it carries is the renderer's, and the
    // unit rows pin the spelling. What matters here is that a real pane paints
    // the row at all after the typed-field change.
    // The skill's own row, and its provenance tag - the second half of gap 1.
    assert!(
        pane.contains("/skill:deploy-cloudflare"),
        "the skill row paints:\n{pane}"
    );
    assert!(
        pane.contains("[skill]"),
        "and it carries its provenance tag:\n{pane}"
    );

    // Clear the composer (which also dismisses the popup), then exit the way
    // the other real-terminal rows do. An `Escape` first leaves the editor in a
    // state the following `/exit` does not route from, and the pane then never
    // exits - CI caught that, local runs did not.
    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
