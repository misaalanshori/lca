//! The reference `ui` extension (Phase 6 exit): registers for all four
//! regions, and its side panel runs a live interactive program through
//! the `pty` capability - the ADR-0016 tmux-shaped pattern, where the
//! only thing that ever runs unsandboxed is the spawned program and
//! everything crossing back is data in a widget tree (ADR-0003: no raw
//! terminal access of its own, FR-UI-2 at the host, not here).
//!
//! # Delivery modes
//!
//! Both modes share every function here. The one thing a WASM guest
//! cannot keep between calls is the pty handle: the host gives each
//! call a fresh instance (the Phase 2 trap-isolation rule), so the
//! guest passes `None` for the panel state and the panel shows its
//! placeholder while the native handle - which lives as long as the
//! session - keeps the session running. The exit test's live session is
//! therefore the native-linked path; the statelessness is documented
//! rather than hidden.
//!
//! # Unsafe-code exemption
//!
//! The `wasm32` half carries generated `wit-bindgen` export shims; the
//! module holds the allowance.

#![deny(unsafe_code)]

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// The grants the manifest declares: the four regions plus the demo's
/// process and terminal (capability catalog's sentences are what the
/// consent screen shows; this is the parsed grant set).
#[cfg(not(target_arch = "wasm32"))]
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("the manifest's own scope parses"),
        ],
        fs_declared: true,
        pty: true,
        ..Default::default()
    }
}

/// The four regions, in catalog order.
pub const REGIONS: [&str; 4] = ["status-line", "footer", "panel", "modal"];

/// The program the panel session runs: a real interactive shell, so
/// the demo is the tmux-shaped case rather than a scripted pipe.
pub fn demo_program() -> &'static str {
    if cfg!(windows) { "cmd.exe" } else { "sh" }
}

/// The live session a panel holds: whatever handle the pty capability
/// handed back, alive for as long as the extension handle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSession {
    /// The capability's pty handle.
    pub handle: u32,
}

/// Build the tree for one region. `session` is `None` in a mode that
/// cannot keep one (the WASM guest's fresh instance per call); every
/// other region is identical across modes by construction.
pub fn render_region(
    region: &str,
    session: Option<&PanelSession>,
) -> Option<Vec<lca_protocol::Widget>> {
    use lca_protocol::Widget;
    let text = |content: &str, role: &str| Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    };
    let nodes = match region {
        "status-line" => vec![text("ui-example ok", "accent")],
        "footer" => vec![
            Widget::Column(vec![1, 2]),
            text("press m in a region for the modal, q to close it", "muted"),
            text("all four regions granted", "muted"),
        ],
        "panel" => match session {
            Some(session) => vec![
                Widget::Column(vec![1, 2]),
                Widget::KeyValue(vec![
                    ("session".to_string(), format!("pty {}", session.handle)),
                    (
                        "keys".to_string(),
                        "typed here reach the program".to_string(),
                    ),
                ]),
                text("type here: the session answers in this panel", "muted"),
            ],
            None => vec![
                Widget::Column(vec![1]),
                Widget::KeyValue(vec![(
                    "session".to_string(),
                    "not available in this delivery mode".to_string(),
                )]),
            ],
        },
        "modal" => vec![
            Widget::Boxed {
                title: Some("ui-example".to_string()),
                child: 1,
            },
            Widget::Column(vec![2, 3]),
            text(
                "A modal is user-dismissible and cannot appear mid-turn on its own.",
                "default",
            ),
            Widget::Progress {
                label: "example".to_string(),
                fill: 0.5,
            },
        ],
        _ => return None,
    };
    Some(nodes)
}

/// One interaction: `m` opens the modal, `q` closes it, submissions
/// surface as notices - the FR-UI-6 vocabulary in miniature. Keys not
/// handled here are what the panel session consumes.
pub fn handle_event(region: &str, input: &lca_protocol::UiInput) -> lca_protocol::UiEffect {
    use lca_protocol::{UiEffect, UiInput};
    match region {
        "modal" => match input {
            UiInput::Key { key } if key == "q" => UiEffect::CloseModal,
            UiInput::Submit { text } => UiEffect::ShowNotice(format!("modal said: {text}")),
            UiInput::Cancel => UiEffect::CloseModal,
            _ => UiEffect::None,
        },
        _ => match input {
            UiInput::Key { key } if key == "m" => UiEffect::OpenModal,
            _ => UiEffect::None,
        },
    }
}

// ---------------------------------------------------------------------------
// Native delivery mode: the handle that keeps the session alive
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::sync::Mutex;

    use lca_ext_abi::{DeliveryMode, DispatchFuture, ExtensionDispatch, World};
    use lca_protocol::{DispatchError, UiEffect, UiInput, WidgetTree};

    /// The native reference handle: the panel's pty session lives in
    /// this struct for as long as the extension is registered.
    pub struct UiExample {
        cap: ArcSession,
        session: Mutex<Option<PanelSession>>,
    }

    /// The engine behind the demo's process and pty grants.
    pub type ArcSession = Arc<lca_tools::Capabilities>;

    impl UiExample {
        /// Build from the engine whose manifest grants carry ui,
        /// process, and pty.
        pub fn new(cap: Arc<lca_tools::Capabilities>) -> UiExample {
            UiExample {
                cap,
                session: Mutex::new(None),
            }
        }

        /// The engine, for tests that inspect recorded denials.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }

        /// The live panel session, when one has started.
        pub fn panel_session(&self) -> Option<u32> {
            self.session
                .lock()
                .expect("session")
                .map(|session| session.handle)
        }
    }

    use std::sync::Arc;

    impl ExtensionDispatch for UiExample {
        fn name(&self) -> &str {
            "ui-example"
        }

        fn delivery(&self) -> DeliveryMode {
            DeliveryMode::Native
        }

        fn worlds(&self) -> Vec<World> {
            vec![World::Ui]
        }

        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, DispatchError> {
            Err(DispatchError::MissingWorld {
                extension: "ui-example".to_string(),
                world: "tool",
            })
        }

        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::ToolResult, DispatchError>> {
            Box::pin(std::future::ready(Err(DispatchError::MissingWorld {
                extension: "ui-example".to_string(),
                world: "tool",
            })))
        }

        fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, DispatchError> {
            Ok(Vec::new())
        }

        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }

        fn ui_regions(&self) -> Vec<String> {
            REGIONS.iter().map(|region| region.to_string()).collect()
        }

        fn render(&self, region: &str) -> Result<Option<WidgetTree>, DispatchError> {
            // The panel starts the demo session on its first render:
            // the user opening the panel IS the invocation (FR-UI-6's
            // shape - nothing starts on its own).
            if region == "panel" {
                let mut session = self.session.lock().expect("session");
                if session.is_none() {
                    let args: Vec<String> = Vec::new();
                    let spawned = self
                        .cap
                        .pty_spawn(demo_program(), &args, "workspace", 24, 80);
                    if let Ok(handle) = spawned {
                        *session = Some(PanelSession { handle });
                    }
                }
            }
            let session = self.session.lock().expect("session");
            // The live read: whatever the program has produced since
            // the last frame, as data (the host draws it; nothing here
            // touches a terminal).
            if region == "panel"
                && let Some(live) = session.as_ref()
                && let Ok(Some(output)) = self.cap.pty_read(live.handle, 4096)
            {
                {
                    let text = String::from_utf8_lossy(&output);
                    // Arena rule: node0 is the root and children are
                    // indices, so a column links the header and lines.
                    let mut nodes = vec![lca_protocol::Widget::Column(Vec::new())];
                    nodes.push(lca_protocol::Widget::KeyValue(vec![(
                        "session".to_string(),
                        format!("pty {}", live.handle),
                    )]));
                    for line in text.lines().take(20) {
                        nodes.push(lca_protocol::Widget::Text {
                            content: line.to_string(),
                            role: "default".to_string(),
                        });
                    }
                    let last = nodes.len() as u32;
                    if let lca_protocol::Widget::Column(children) = &mut nodes[0] {
                        *children = (1..last).collect();
                    }
                    return Ok(Some(WidgetTree { nodes }));
                }
            }
            drop(session);
            Ok(
                render_region(region, self.session.lock().expect("session").as_ref())
                    .map(|nodes| WidgetTree { nodes }),
            )
        }

        fn on_ui_event(&self, region: &str, input: &UiInput) -> Result<UiEffect, DispatchError> {
            // Keys the modal and status branches do not handle reach the
            // live session: typing into the panel types into the program.
            if region == "panel"
                && let UiInput::Key { key } = input
                && let Some(session) = *self.session.lock().expect("session")
            {
                let _ = self.cap.pty_write(session.handle, key.as_bytes());
                return Ok(UiEffect::None);
            }
            Ok(handle_event(region, input))
        }

        fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> DispatchFuture<'a, Result<lca_protocol::HookAction, DispatchError>> {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }

        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> DispatchFuture<'a, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::UiExample;

// ---------------------------------------------------------------------------
// WASM delivery mode: the same functions, stateless panel (see the
// crate docs: a fresh instance per call cannot keep a handle)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // generated wit-bindgen export shims
mod wasm_mode {
    use super::*;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "ui",
        export_macro_name: "export_ui",
        with: {
            "lca:host/log@0.2.0": generate,
            "lca:host/ui@0.2.0": generate,
        },
    });

    use exports::lca::ext::interaction::{
        Effect as WasmEffect, Guest as InteractionGuest, Input as WasmInput,
    };
    use exports::lca::ext::render::{Guest as RenderGuest, Widget as WasmWidget};

    fn to_wit(widget: lca_protocol::Widget) -> WasmWidget {
        use lca_protocol::Widget as W;
        match widget {
            W::Text { content, role } => WasmWidget::Text((content, role)),
            W::Image { media_type, bytes } => WasmWidget::Image((media_type, bytes)),
            W::Boxed { title, child } => WasmWidget::Boxed((title, child)),
            W::Row(children) => WasmWidget::Row(children),
            W::Column(children) => WasmWidget::Column(children),
            W::Spinner { frames } => WasmWidget::Spinner(frames),
            W::Progress { label, fill } => WasmWidget::Progress((label, fill)),
            W::KeyValue(pairs) => WasmWidget::Keyvalue(pairs),
            W::Vendor(kind) => WasmWidget::Vendor(kind),
        }
    }

    pub struct UiExampleWasm;

    impl RenderGuest for UiExampleWasm {
        fn render(region: String) -> Option<Vec<WasmWidget>> {
            render_region(&region, None).map(|nodes| nodes.into_iter().map(to_wit).collect())
        }
    }

    impl InteractionGuest for UiExampleWasm {
        fn handle(region: String, input: WasmInput) -> WasmEffect {
            let input = match input {
                WasmInput::Key(key) => lca_protocol::UiInput::Key { key },
                WasmInput::Submit(text) => lca_protocol::UiInput::Submit { text },
                WasmInput::Cancel => lca_protocol::UiInput::Cancel,
            };
            match handle_event(&region, &input) {
                lca_protocol::UiEffect::None => WasmEffect::None,
                lca_protocol::UiEffect::CloseModal => WasmEffect::CloseModal,
                lca_protocol::UiEffect::OpenModal => WasmEffect::OpenModal,
                lca_protocol::UiEffect::ShowNotice(text) => WasmEffect::ShowNotice(text),
                lca_protocol::UiEffect::InsertText(text) => WasmEffect::InsertText(text),
                lca_protocol::UiEffect::SubmitPrompt(text) => WasmEffect::SubmitPrompt(text),
            }
        }
    }

    export_ui!(UiExampleWasm);
}
