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

/// Wait until **every** needle is on screen at once, then return that frame.
///
/// The fold-aware wait gh #413 asked for: a selector paints its rows over
/// several frames, so waiting for one label and asserting another races the
/// render. The timeout is the same loud panic as [`Tmux::wait_for`] and reports
/// the last frame it saw.
#[cfg(unix)]
fn wait_for_all(session: &Tmux, needles: &[&str], timeout: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let pane = session.capture();
        if needles.iter().all(|needle| pane.contains(needle)) {
            return pane;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "not every needle {:?} appeared in the pane at once:\n{pane}",
                needles
            );
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
    // Gh #397: the thinking row now opens a stepped wizard whose second step
    // offers the model's allowed levels. This sandbox has no model catalog, so
    // `models.thinking_levels` is what declares them - the same table a user
    // would write, and the documented file-only form. Without it the wizard still
    // opens (the model step is real) but offers no level, which would make this
    // row unable to prove what it exists to prove: that a `/settings` edit
    // reaches the file.
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "# my setup\ncompaction.threshold = 0.7\n\n[models.thinking_levels]\n\"test-model\" = [\"off\", \"minimal\", \"low\", \"medium\", \"high\"]\n",
    )
    .expect("write config");

    let session = Tmux::new("settings-selector");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    // The inventoried box is taller than a default pane: size up so every
    // section paints (the box clips to the viewport, gh #82).
    session.resize(120, 55);

    // 1. The selector opens: label, current value, winning source.
    // The needle is a curated label, never a bare word:
    // `wait_for("thinking")` matched footer/status text before the
    // selector opened (macOS CI).
    session.send(&["/settings", "Enter"]);
    // harness: fold-aware wait, gh #413. The selector paints its rows over
    // several frames, so waiting for the *first* label and then asserting the
    // second raced the render (third hit 2026-10-17: `Theme` on screen,
    // `Permissions` one frame behind, under full-suite load). Wait for the
    // screen the assertion actually reads: both curated labels present at
    // once. The assertion below is unchanged.
    let pane = wait_for_all(
        &session,
        &["Theme", "Permissions"],
        std::time::Duration::from_secs(20),
    );
    assert!(
        pane.contains("Theme") && pane.contains("Permissions"),
        "the curated rows are listed:\n{pane}"
    );
    assert!(
        pane.contains("default"),
        "the winning source is on the row:\n{pane}"
    );
    // The selector settles with the first row selected: every Down
    // below counts from row 0, so a re-sorted list fails here,
    // loudly, instead of silently retargeting.
    session.wait_for("> Theme", std::time::Duration::from_secs(10));

    // 2. The `thinking` row opens the existing thinking sub-picker
    // (Display carries eight rows before Model starts), and choosing a
    // level persists it. Each Down is followed by its frame: the
    // selected-row marker moves before the next key is sent, so a slow
    // frame never eats a step.
    for marker in [
        "> Color",
        "> Display mode",
        "> Quiet startup",
        "> Completion rows",
        "> Editor padding",
        "> Output margin",
        "> Code border",
        "> Thinking effort",
    ] {
        session.send(&["Down"]);
        session.wait_for(marker, std::time::Duration::from_secs(10));
    }
    session.send(&["Enter"]);
    // **Changed by gh #397's brief**: the thinking row now opens the stepped
    // wizard (model, then level) rather than the flat level picker, so this
    // section walks two steps. The property this row pins is unchanged - a
    // `/settings` edit reaches the config file and the footer - so only the
    // keystrokes changed.
    //
    // Both steps print `Step N/2`, so the *cursor marker* is what says which step
    // is live; a bare `Thinking level:` wait matches the unmarked row of the
    // other step and races.
    session.wait_for("> Model:", std::time::Duration::from_secs(10));
    session.send(&["Enter"]); // advance to the thinking level
    session.wait_for("> Thinking level:", std::time::Duration::from_secs(10));
    // Step one changes nothing, so only the level is committed - and a sandbox
    // whose mock model declares no levels offers none. In that case the wizard
    // still opens (the model step is a real choice) but there is nothing to pick,
    // so the row's assertion is that the wizard is walkable, not that a level
    // exists. The mock here does declare levels, so walk them.
    let pane = session.capture();
    let levels = ["off", "minimal", "low", "medium", "high"];
    let start = levels
        .into_iter()
        .find(|level| pane.contains(&format!("> Thinking level: {level}")));
    if let Some(start) = start {
        let from = levels
            .iter()
            .position(|level| *level == start)
            .expect("a level");
        for level in &levels[from + 1..] {
            session.send(&["Down"]);
            session.wait_for(
                &format!("> Thinking level: {level}"),
                std::time::Duration::from_secs(10),
            );
        }
    }
    session.send(&["Enter"]); // finish: commit the walked steps
    // The notice row sits under the tall box (it wins over the dock),
    // so wait for the durable outcome - the re-read row - never the
    // transient notice.
    session.wait_for("Thinking effort = high", std::time::Duration::from_secs(10));

    // 3. The selector is back on screen after its sub-picker closed
    //    (pi's submenu shape - the list is never lost), and it answers
    //    with the new value and its new source. The notice fades; the
    //    restored selector persists - so wait for the durable outcome,
    //    never the notice.
    let pane = session.wait_for("user file", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("Thinking effort = high"),
        "...and the new value:\n{pane}"
    );

    // 4. The running UI reflects it: the `thinking: high` notice above
    // is transient - the footer wears the level durably. The sectioned
    // box is tall enough to cover the footer row while open (the box
    // wins over the dock), so close it first, then read the footer.
    session.send(&["Escape"]);
    wait_for_gone(
        &session,
        SETTINGS_HEADER,
        std::time::Duration::from_secs(10),
    );
    let pane = session.wait_for("\u{2022} high", std::time::Duration::from_secs(10));
    assert!(
        pane.lines()
            .any(|row| row.contains("high") && row.contains("ctx")),
        "the footer shows the new level:\n{pane}"
    );

    // A fresh open reads the same answer (the write is on disk).
    session.send(&["/settings", "Enter"]);
    let pane = session.wait_for("user file", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("thinking"),
        "a fresh open shows it too:\n{pane}"
    );
    // Gh #397: the wizard's commit leaves the selector open on the opener row, so
    // this escape closes it exactly as before. Wait for the *box* to go, not just
    // the header line: a windowed overlay can keep the frame while scrolling the
    // header out, and the old wait then reported "gone" with the picker still
    // eating `/exit` (that is how this row failed once).
    session.send(&["Escape"]);
    // The header line is what says the list is live: main-screen rendering
    // appends, so an emptied box frame from an earlier repaint stays in the
    // captured scrollback (waiting for `╰` never fires).
    wait_for_gone(
        &session,
        SETTINGS_HEADER,
        std::time::Duration::from_secs(10),
    );

    // 5. The write itself, with the file's other content intact.
    // Gh #397: the wizard leaves the selector open on the opener row (pi's
    // submenu shape), so the escape above closes the selector as before - but a
    // stale wizard would swallow `/exit` into its own key arm. Assert it is gone
    // rather than relying on that, so a future change that leaves one open fails
    // here and not as a confusing "no session-end record".
    assert!(
        !session.capture().contains("Step 1/2") && !session.capture().contains("Step 2/2"),
        "no wizard is left open before /exit"
    );
    session.send(&["/exit", "Enter"]);
    // Gh #397: a `/settings`-only session has no *messages*, and a quit without
    // messages writes no session log - so there is no `session-end` marker to
    // wait for (the helper says so in its own doc). The dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
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
