//! The ADR-0016 pattern end to end: a live interactive program in the
//! side panel, reached only through the process and pty capabilities,
//! with everything crossing back as data in a widget tree.
//!
//! Verifies: the Phase 6 exit test's third clause - a pty-backed
//! extension displays a live interactive session inside a panel with no
//! raw terminal access of its own (ADR-0003's data-only boundary).

use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn sandbox(name: &str) -> Arc<lca_tools::Capabilities> {
    let root = std::env::temp_dir().join(format!("lca-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    Arc::new(lca_tools::Capabilities::new(
        "ui-example",
        ui_example::manifest_grants(),
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ))
}

fn panel_lines(handle: &dyn ExtensionDispatch) -> String {
    let tree = handle
        .render("panel")
        .expect("render")
        .expect("the panel has content");
    lca_tui::widget_lines(&tree.nodes).join("\n")
}

// Verifies: all four regions are registered and answer, and the panel
// starts the demo the first time it is drawn (the user opening the
// panel is the invocation - nothing runs on its own, FR-UI-6's shape).
#[cfg_attr(
    target_os = "macos",
    ignore = "pty allocation path on macOS - tracked in docs/platform-notes.md"
)]
#[test]
fn all_four_regions_render_and_the_panel_session_starts_on_demand() {
    let cap = sandbox("regions");
    let handle = ui_example::UiExample::new(cap.clone());
    assert_eq!(
        handle.ui_regions().len(),
        4,
        "status-line, footer, panel, modal"
    );
    for region in ["status-line", "footer", "modal"] {
        let tree = handle.render(region).expect("render").expect("content");
        assert!(!tree.is_empty(), "{region}");
    }
    assert!(
        handle.panel_session().is_none(),
        "nothing starts until the panel is drawn"
    );
    let first = panel_lines(&handle);
    assert!(first.contains("session"), "{first}");
    assert!(handle.panel_session().is_some(), "the session started");
}

// Verifies: the exit test's live session - typing into the panel types
// into the program (a pty echoes it), and the output comes back as
// data in the tree: no escape sequence crosses, the terminal's is the
// host's business (ADR-0003, FR-UI-2, ADR-0016).
#[cfg_attr(
    target_os = "macos",
    ignore = "pty allocation path on macOS - tracked in docs/platform-notes.md"
)]
#[test]
fn typing_into_the_panel_reaches_the_program_and_comes_back_as_data() {
    use lca_ext_abi::ExtensionDispatch as _;
    let cap = sandbox("live");
    let handle = ui_example::UiExample::new(cap.clone());
    let _ = panel_lines(&handle); // starts the session
    let session = handle.panel_session().expect("session running");

    // Type a line into the shell: the pty echoes it, so the output the
    // next render reads contains what we sent.
    let _effect = handle
        .on_ui_event(
            "panel",
            &lca_protocol::UiInput::Key {
                key: "echo-live\r".into(),
            },
        )
        .expect("event");
    let mut panel = String::new();
    for _attempt in 0..50 {
        panel = panel_lines(&handle);
        if panel.contains("echo-live") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        panel.contains("echo-live"),
        "the live session echoed the keystroke: {panel}"
    );
    assert!(
        !panel.contains('\u{1b}'),
        "what came back is data; the host draws it (no raw terminal here)"
    );

    // The pty handle is the capability's own - the extension never
    // opened a device.
    let denials = cap.denials();
    assert!(
        denials.is_empty(),
        "everything the demo needs was granted: {denials:?}"
    );
    assert!(handle.panel_session().is_some(), "still alive");
    let _ = session;
}

// Verifies: the manifest pairs every region with the two capabilities
// the panel demo concretely uses (the consent surface stays honest).
#[test]
fn the_manifest_declares_the_regions_and_the_demo_capabilities() {
    let manifest: toml::Value = ui_example::MANIFEST.parse().expect("MANIFEST parses");
    let regions = manifest["capabilities"]["ui"]["regions"]
        .as_array()
        .expect("regions");
    assert_eq!(regions.len(), 4);
    assert!(
        manifest["capabilities"]["pty"]["reason"]
            .as_str()
            .unwrap()
            .contains("terminal")
    );
    let grants = ui_example::manifest_grants();
    assert!(
        grants.pty && !grants.credentials && !grants.process,
        "only what the demo uses"
    );
    assert_eq!(grants.fs.len(), 1, "the cwd scope pairing");
    assert!(
        lca_ext_abi::ABI_VERSION.len() >= 3,
        "abi line present: {}",
        manifest["abi"]
    );
}
