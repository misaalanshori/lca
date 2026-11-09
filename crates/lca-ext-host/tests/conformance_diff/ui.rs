//! The `ui` world in both delivery modes: regions, effects, and a scripted dialog verdict (FR-UI-2/6, gh #124).
//!
//! Gh #552 split `tests/conformance_diff.rs` (it was 1,180 lines, 20 under gate
//! 11's ceiling) at the world groups it already had, following the pattern its
//! two earlier siblings established (`args.rs`, `serve_http.rs`). The dual-mode
//! harness - `Fixture`, `both_modes`, `both_modes_keeping_wasm` - stays in the
//! barrel and is reached through `super`: it is what every row calls, and a copy
//! per file would recompile the ~6 s WASM fixture once more per row
//! (`tests/regressions/449`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_ext_host::{ExtHost, ExtensionLimits, HostEnvironment};
use lca_protocol::ToolCall;

use super::{FIXTURE, Fixture, MANIFEST};
// Verifies: the Phase 6 conformance cases - both modes return the same
// tree for every region (NFR-25 over ADR-0003's arena), the hostile
// span crosses byte for byte so the host is the one that must defang
// it (FR-UI-2), and interaction effects agree (FR-UI-6's vocabulary).
#[tokio::test]
async fn ui_regions_and_effects_agree_across_modes() {
    let fixture = Fixture::new("diff-ui");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(wasm.ui_regions(), native.ui_regions());
    assert_eq!(wasm.ui_regions().len(), 4, "all four regions granted");

    for region in ["status-line", "footer", "panel", "modal"] {
        let wasm_tree = wasm.render(region).expect("wasm render");
        let native_tree = native.render(region).expect("native render");
        assert_eq!(wasm_tree, native_tree, "tree divergence in {region}");
        let tree = wasm_tree.expect("a scripted tree");
        assert!(!tree.is_empty(), "{region} has content");
        if region == "status-line" {
            assert_eq!(tree.nodes.len(), 1, "a single span needs no arena");
        }
    }
    // The hostile fixture crossed intact, escape byte and all: the
    // HOST must defang it on the way out (lca-tui's sanitizer test is
    // the other half of FR-UI-2).
    let footer = wasm.render("footer").expect("footer").expect("tree");
    let hostile = footer
        .nodes
        .iter()
        .find_map(|node| match node {
            lca_protocol::Widget::Text { content, .. } if content.contains("NOT A PROMPT") => {
                Some(content.clone())
            }
            _ => None,
        })
        .expect("the hostile span crossed the boundary");
    assert!(
        hostile.contains('\u{1b}'),
        "the real escape byte survived the ABI: {hostile:?}"
    );

    // The freeze gate's coverage claim: every widget case crosses the
    // boundary in both modes (the footer is the vocabulary page).
    use lca_protocol::Widget as W;
    let footer_nodes = &footer.nodes;
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Column(_))),
        "column"
    );
    assert!(footer_nodes.iter().any(|w| matches!(w, W::Row(_))), "row");
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Spinner { .. })),
        "spinner"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Progress { .. })),
        "progress"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Image { .. })),
        "image"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Vendor(_))),
        "the reserved vendor case"
    );
    let modal = wasm.render("modal").expect("modal").expect("tree");
    assert!(
        modal.nodes.iter().any(|w| matches!(w, W::Boxed { .. })),
        "boxed"
    );
    // The 0.6 vocabulary (gh #172): every new widget case crosses the
    // boundary in both modes, same as the old ones above.
    assert!(
        footer_nodes
            .iter()
            .any(|w| matches!(w, W::StyledText { .. })),
        "styled-text"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Markdown { .. })),
        "markdown"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Button { .. })),
        "button"
    );
    assert!(
        footer_nodes.iter().any(|w| matches!(w, W::Table { .. })),
        "table"
    );
    assert!(
        footer_nodes
            .iter()
            .any(|w| matches!(w, W::ScrollContainer { .. })),
        "scroll-container"
    );
    let panel = wasm.render("panel").expect("panel").expect("tree");
    assert!(
        panel.nodes.iter().any(|w| matches!(w, W::KeyValue(_))),
        "keyvalue"
    );

    // Events: the modal branch answers, other regions do nothing.
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        native
            .on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Key { key: "q".into() })
            .expect("event"),
        lca_protocol::UiEffect::CloseModal
    );
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::Submit { text: "hi".into() }
        )
        .expect("event"),
        lca_protocol::UiEffect::ShowNotice("heard: hi".to_string())
    );
    assert_eq!(
        wasm.on_ui_event("panel", &lca_protocol::UiInput::Cancel)
            .expect("event"),
        lca_protocol::UiEffect::None,
        "only the modal region answers"
    );
    // The 0.6 mouse inputs (gh #172): a button click names its widget
    // in both modes, and the relative forms cross intact.
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
        )
        .expect("event"),
        native
            .on_ui_event(
                "modal",
                &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
            )
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event(
            "modal",
            &lca_protocol::UiInput::ClickWidget { id: "ok".into() }
        )
        .expect("event"),
        lca_protocol::UiEffect::ShowNotice("clicked: ok".to_string())
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Click { col: 3, row: 1 })
            .expect("event"),
        native
            .on_ui_event("modal", &lca_protocol::UiInput::Click { col: 3, row: 1 })
            .expect("event")
    );
    assert_eq!(
        wasm.on_ui_event("modal", &lca_protocol::UiInput::Scroll { delta: -1 })
            .expect("event"),
        lca_protocol::UiEffect::None
    );
}

// Verifies: gh #124 - a conformance tool asking `ui.confirm` hears the
// same verdict in both modes: the WASM twin crosses the real host
// import, the native twin answers through its injected prompter, and
// the Fixture scripts both sides alike.
#[test]
fn the_dialog_verdict_agrees_across_modes() {
    use lca_permissions::{DialogPrompt, SharedDialogs};

    struct Scripted(bool);
    impl DialogPrompt for Scripted {
        fn confirm(&mut self, _title: &str, _message: &str) -> bool {
            self.0
        }
        fn select(&mut self, _title: &str, _options: &[String]) -> Option<String> {
            None
        }
        fn input(&mut self, _label: &str, _placeholder: Option<&str>) -> Option<String> {
            None
        }
        fn notify(&mut self, _message: &str, _level: &str) {}
    }

    fn asking(handle: &Arc<dyn ExtensionDispatch>) -> String {
        // Tools run off the loop thread (gh #124), so no guard is set
        // here - exactly like the turn worker calling in.
        let call = ToolCall {
            call_id: "dialog-1".to_string(),
            name: "conformance".to_string(),
            arguments: "{\"mode\":\"ask-confirm\"}".to_string(),
            parent_call_id: None,
        };
        let handle = handle.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(handle.execute_tool(&call))
        })
        .join()
        .expect("tool thread")
        .expect("tool ran")
        .content
    }

    fn ask(wasm: &Arc<dyn ExtensionDispatch>, native: &Arc<dyn ExtensionDispatch>, verdict: &str) {
        let wasm_text = asking(wasm);
        let native_text = asking(native);
        assert_eq!(wasm_text, native_text, "the verdict agrees");
        assert!(
            wasm_text.contains(verdict),
            "the scripted verdict crossed: {wasm_text}"
        );
    }

    // gh #449: one load, not one per verdict. `SharedDialogs` is a live
    // slot the store holds by handle (`build_store` clones the Arc), and
    // the host reads `current()` per call - so installing the second
    // scripted prompter on the *same* guest exercises exactly what a
    // second load did, against the same component. The previous shape
    // paid `Component::new` twice (~6 s each in a test build) to prove
    // something the slot iteration already proves.
    let fixture = Fixture::new("dialog");
    let dialogs = SharedDialogs::default();
    let mut host = ExtHost::new(
        ExtensionLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel_per_call: 100_000_000,
            log_limit_bytes: 4096,
        },
        Arc::new(HostEnvironment {
            roots: fixture.roots.clone(),
            prompt: fixture.prompt.clone(),
            dialogs: dialogs.clone(),
            grant_store: fixture.store.clone(),
            project: fixture.root.join("project"),
            proposals: None,
            // (kept alphabetical like the constructor)
        }),
    );
    let wasm: Arc<dyn ExtensionDispatch> =
        Arc::new(host.load(FIXTURE, MANIFEST).expect("load wasm mode"));
    let engine = fixture.capabilities();
    let native: Arc<dyn ExtensionDispatch> =
        Arc::new(conformance::NativeConformance::new(engine).with_dialogs(dialogs.clone()));
    for verdict in [false, true] {
        dialogs.set(Arc::new(Mutex::new(Scripted(verdict))));
        ask(&wasm, &native, &format!("confirm: {verdict}"));
    }
}
