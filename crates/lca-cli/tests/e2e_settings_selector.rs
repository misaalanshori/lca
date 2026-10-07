//! The `/settings` selector (gh #30, EFG-030 + PG-032) end to end:
//! the acceptance criterion verbatim - open `/settings`, change a
//! setting, `config.toml` carries the change, and the running UI
//! reflects it - with the winning source moving from `default` to
//! `user file` on the way (FR-CFG-2's column, kept from the dump).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

#[cfg(unix)]
/// Wait until the pane no longer shows the needle (an overlay
/// closed): the inverse of `wait_for`, same loud timeout. Typed
/// input after an overlay closes must not land in the overlay, so
/// the close is awaited, never slept through.
#[cfg(unix)]
fn wait_for_gone(session: &Tmux, needle: &str, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let pane = session.capture();
        if !pane.contains(needle) {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("`{needle}` never left the pane:\n{pane}");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// The `/settings` selector's header line: present exactly while the
/// selector overlay is open, so its absence proves a close landed.
#[cfg(unix)]
const SETTINGS_HEADER: &str = "key = value [source]";

#[cfg(unix)]
#[test]
fn settings_changes_a_setting_and_config_toml_carries_it() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("settings-selector");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // A config file whose comments and unrelated keys must survive the
    // write (the one persist seam, QA-015).
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "# my setup\ncompaction.threshold = 0.7\n",
    )
    .expect("write config");

    let session = Tmux::new("settings-selector");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));

    // 1. The selector opens: key, current value, winning source. The
    // needle is a curated key, never a bare word: `wait_for("thinking")`
    // matched footer/status text before the selector opened (macOS CI).
    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("ui.theme", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("ui.theme") && pane.contains("permissions.mode"),
        "the curated keys are listed:\n{pane}"
    );
    assert!(
        pane.contains("default"),
        "the winning source is on the row:\n{pane}"
    );
    // The selector settles with the first row selected: every Down
    // below counts from row 0, so a re-sorted list fails here,
    // loudly, instead of silently retargeting.
    session.wait_for("> ui.theme", std::time::Duration::from_secs(10));

    // 2. The `thinking` row opens the existing thinking sub-picker (the
    // third row of the curated list), and choosing a level persists it.
    // Each Down is followed by its frame: the selected-row marker
    // moves before the next key is sent, so a slow frame never eats
    // a step.
    session.send(&["Down"]);
    session.wait_for("> ui.thinking", std::time::Duration::from_secs(10));
    session.send(&["Down"]);
    session.wait_for("> thinking", std::time::Duration::from_secs(10));
    session.send(&["Enter"]);
    session.wait_for("No reasoning", std::time::Duration::from_secs(10));
    // The sub-picker opens on the current level (unset on a fresh
    // sandbox); anchor there, then step to high one marked frame at
    // a time: unset -> off -> minimal -> low -> medium -> high.
    session.wait_for("> unset", std::time::Duration::from_secs(10));
    for level in ["off", "minimal", "low", "medium", "high"] {
        session.send(&["Down"]);
        session.wait_for(&format!("> {level}"), std::time::Duration::from_secs(10));
    }
    session.send(&["Enter"]);
    session.wait_for("thinking: high", std::time::Duration::from_secs(10));

    // 3. The running UI reflects it: wait for the footer outcome (the
    // `thinking: high` notice above is transient - the footer wears
    // the level durably), then assert the same rows as before.
    let pane = session.wait_for("\u{2022} high", std::time::Duration::from_secs(10));
    assert!(
        pane.lines()
            .any(|row| row.contains("high") && row.contains("ctx")),
        "the footer shows the new level:\n{pane}"
    );

    // 4. The selector is back on screen after its sub-picker closed
    //    (pi's submenu shape - the list is never lost), and it answers
    //    with the new value and its new source. The notice fades; the
    //    restored selector persists - so wait for the durable outcome,
    //    never the notice.
    let pane = session.wait_for("user file", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("thinking = high"),
        "...and the new value:\n{pane}"
    );
    session.send(&["Escape"]);
    wait_for_gone(
        &session,
        SETTINGS_HEADER,
        std::time::Duration::from_secs(10),
    );

    // A fresh open reads the same answer (the write is on disk).
    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("user file", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("thinking"),
        "a fresh open shows it too:\n{pane}"
    );
    session.send(&["Escape"]);
    wait_for_gone(
        &session,
        SETTINGS_HEADER,
        std::time::Duration::from_secs(10),
    );

    // 5. The write itself, with the file's other content intact.
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    let text = std::fs::read_to_string(sandbox.state_dir().join("config.toml")).expect("config");
    assert!(
        text.contains("thinking = \"high\""),
        "config.toml carries the change:\n{text}"
    );
    assert!(
        text.contains("# my setup") && text.contains("compaction.threshold = 0.7"),
        "the comment-preserving writer kept the rest of the file:\n{text}"
    );
}
