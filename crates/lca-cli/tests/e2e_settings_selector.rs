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

    // 1. The selector opens: key, current value, winning source.
    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("thinking", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("ui.theme") && pane.contains("permissions.mode"),
        "the curated keys are listed:\n{pane}"
    );
    assert!(
        pane.contains("default"),
        "the winning source is on the row:\n{pane}"
    );

    // 2. The `thinking` row opens the existing thinking sub-picker (the
    // third row of the curated list), and choosing a level persists it.
    session.send(&["Down"]);
    session.send(&["Down"]);
    session.send(&["Enter"]);
    session.wait_for("No reasoning", std::time::Duration::from_secs(10));
    for _ in 0..5 {
        // unset -> off -> minimal -> low -> medium -> high
        session.send(&["Down"]);
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    session.send(&["Enter"]);
    session.wait_for("thinking: high", std::time::Duration::from_secs(10));

    // 3. The running UI reflects it: the footer wears the level.
    let pane = session.capture();
    assert!(
        pane.lines()
            .any(|row| row.contains("high") && row.contains("ctx")),
        "the footer shows the new level:\n{pane}"
    );

    // 4. The selector is back on screen after its sub-picker closed
    //    (pi's submenu shape - the list is never lost), and it answers
    //    with the new value and its new source.
    let pane = session.capture();
    assert!(
        pane.contains("user file"),
        "the restored selector shows the new source:\n{pane}"
    );
    assert!(
        pane.contains("thinking = high"),
        "...and the new value:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    // A fresh open reads the same answer (the write is on disk).
    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("user file", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("thinking"),
        "a fresh open shows it too:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

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
