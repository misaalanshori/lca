//! The reference `ui` extension (Phase 6 exit): registers for all four
//! regions, and its side panel runs a live interactive program through
//! the `pty` capability - the ADR-0016 tmux-shaped pattern, where the
//! only thing that ever runs unsandboxed is the spawned program and
//! everything crossing back is data in a widget tree (ADR-0003: no raw
//! terminal access of its own, FR-UI-2 at the host, not here).
//!
//! The modal demonstrates the 0.6 vocabulary's interactive half (gh
//! #172): a `button` widget whose `click-widget` input answers with a
//! notice, alongside the styled vocabulary the conformance extension
//! probes exhaustively.
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

/// The panel's rolling screen (gh #301).
pub mod panel_screen;

/// The manifest this form ships with (single source for
/// [`manifest_grants`]; a test keeps them in step).
pub const MANIFEST: &str = include_str!("../extension.toml");

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The grants the manifest declares: the four regions plus the demo's
/// process and terminal (capability catalog's sentences are what the
/// consent screen shows; this is the parsed grant set).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
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

/// Plan mode's badge state (gh #409's first slice).
///
/// **Extension state, not host state.** A `plan_mode` flag in the host would be
/// the core growing a feature that belongs to an extension (ADR-0013's
/// taxonomy), and the `ui` world exists exactly so an extension can hold state
/// and be pulled for it. This struct is that state, and `render` reads it.
///
/// The badge needs no push: the host's `RegionRenderer` calls `render(region)`
/// per frame (`crates/lca-cli/src/tui/mod.rs`), so mutating this on a key is
/// enough for the next frame to show it. `lca:host/ui.request-redraw()` stays
/// the documented push for a guest whose async work lands outside a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanBadge {
    /// Whether plan mode is on.
    pub active: bool,
    /// `(current, total)` when the extension has parsed a plan's steps.
    /// `None` while the mode is on but no plan exists yet, because a
    /// half-formed `0/0` would be worse than no fraction.
    pub step: Option<(u32, u32)>,
}

impl PlanBadge {
    /// The one line the `status-line` region shows, or `None` when plan mode is
    /// off - "nothing at all" rather than an empty segment, because the region
    /// is shared and an empty segment still costs a separator.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        if !self.active {
            return None;
        }
        Some(match self.step {
            Some((current, total)) => format!("plan {current}/{total}"),
            None => "plan".to_string(),
        })
    }
}

/// The badge's widget tree, in the arena shape the `ui` world uses (node0 is
/// the root, children are indices).
///
/// The vocabulary is the existing one on purpose (gh #409): a `text` node with
/// the `accent` role. A new widget kind here would be an ABI change, and the
/// issue labels that possibility for the 0.7 batch rather than this slice. The
/// words carry the meaning so a plain theme still reads "plan 2/5" (NFR-28).
#[must_use]
pub fn plan_badge_nodes(badge: &PlanBadge) -> Vec<lca_protocol::Widget> {
    match badge.text() {
        Some(text) => vec![lca_protocol::Widget::Text {
            content: text,
            role: "accent".to_string(),
        }],
        None => Vec::new(),
    }
}

/// The program the panel session runs: a real interactive shell, so
/// the demo is the tmux-shaped case rather than a scripted pipe.
pub fn demo_program() -> &'static str {
    if cfg!(windows) { "cmd.exe" } else { "sh" }
}

pub use crate::panel_screen::PanelScreen;

/// The live session a panel holds: whatever handle the pty capability
/// handed back, alive for as long as the extension handle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSession {
    /// The capability's pty handle.
    pub handle: u32,
}

/// The panel's widget tree: the session header, then the screen's rows, then
/// what the session has to say about itself (gh #301).
///
/// One builder for both delivery modes, so the native panel and the WASM panel
/// can only differ in what they *know* - a live screen or nothing - never in how
/// they render it.
#[must_use]
pub fn panel_nodes(
    session: Option<&PanelSession>,
    screen: Option<&PanelScreen>,
) -> Vec<lca_protocol::Widget> {
    use lca_protocol::Widget;
    let text = |content: &str, role: &str| Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    };
    let Some(session) = session else {
        return vec![
            Widget::Column(vec![1]),
            Widget::KeyValue(vec![(
                "session".to_string(),
                "not available in this delivery mode".to_string(),
            )]),
        ];
    };
    let mut nodes = vec![Widget::Column(Vec::new())];
    nodes.push(Widget::KeyValue(vec![
        ("session".to_string(), format!("pty {}", session.handle)),
        (
            "keys".to_string(),
            "typed here reach the program".to_string(),
        ),
    ]));
    let rows = screen.map(PanelScreen::rows).unwrap_or_default();
    for row in &rows {
        nodes.push(text(row, "default"));
    }
    if let Some(code) = screen.and_then(PanelScreen::ended) {
        // Gh #301: a session that has exited says so. Freezing on its last
        // output would be indistinguishable from one that has gone quiet.
        nodes.push(text(&panel_screen::ended_label(code), "muted"));
    } else if rows.is_empty() {
        // Nothing has run yet: say what the panel is for rather than showing a
        // bare header.
        nodes.push(text(
            "type here: the session answers in this panel",
            "muted",
        ));
    }
    // Arena rule: node0 is the root and children are indices.
    let last = nodes.len() as u32;
    if let Widget::Column(children) = &mut nodes[0] {
        *children = (1..last).collect();
    }
    nodes
}

/// Build the tree for one region. `session` is `None` in a mode that
/// cannot keep one (the WASM guest's fresh instance per call); every
/// other region is identical across modes by construction.
pub fn render_region(
    region: &str,
    session: Option<&PanelSession>,
) -> Option<Vec<lca_protocol::Widget>> {
    render_region_with(region, session, &PlanBadge::default())
}

/// [`render_region`] with plan-mode state (gh #409): the status line is the
/// badge's slot, and every other region is identical whether or not plan mode is
/// on.
pub fn render_region_with(
    region: &str,
    session: Option<&PanelSession>,
    badge: &PlanBadge,
) -> Option<Vec<lca_protocol::Widget>> {
    use lca_protocol::Widget;
    let text = |content: &str, role: &str| Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    };
    let nodes = match region {
        // Gh #409: the plan-mode badge rides the existing status-line region.
        // While plan mode is on it leads, so the mode is the first thing read;
        // while it is off the line is exactly what it was before this slice.
        "status-line" => vec![match badge.text() {
            // Gh #409: plan mode leads, so the mode is the first thing read. The
            // mode word is in the text, not only the colour (NFR-28).
            Some(plan) => text(&format!("{plan} | ui-example ok"), "accent"),
            None => text("ui-example ok", "accent"),
        }],
        "footer" => vec![
            Widget::Column(vec![1, 2]),
            text("press m in a region for the modal, q to close it", "muted"),
            text("all four regions granted", "muted"),
        ],
        // Gh #301: the panel is a screen, so its tree is built from what the
        // session has already produced. `None` here is the mode that cannot keep
        // a handle (the WASM guest), which is also the mode with no screen.
        "panel" => return Some(panel_nodes(session, None)),
        "modal" => vec![
            Widget::Boxed {
                title: Some("ui-example".to_string()),
                border: None,
                background: None,
                child: 1,
            },
            Widget::Column(vec![2, 3, 4]),
            text(
                "A modal is user-dismissible and cannot appear mid-turn on its own.",
                "default",
            ),
            Widget::Progress {
                label: "example".to_string(),
                fill: 0.5,
            },
            // Gh #172: the clickable demo button - clicking it answers
            // with a notice, which the mouse receipt drives.
            Widget::Button {
                id: "demo-ok".to_string(),
                label: "OK".to_string(),
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
            // Gh #172: the demo button answers with a notice.
            UiInput::ClickWidget { id } if id == "demo-ok" => {
                UiEffect::ShowNotice("demo ok clicked".to_string())
            }
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
        /// Gh #301: what the session has produced, kept so a frame with no new
        /// bytes can draw it again. Behind the same mutex as the session because
        /// the two are read together and started together.
        screen: Mutex<PanelScreen>,
        /// Gh #409: plan-mode state, held here because it is the extension's
        /// (ADR-0013's taxonomy). The host pulls `render` per frame, so this
        /// needs no notification when it changes.
        badge: Mutex<PlanBadge>,
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
                screen: Mutex::new(PanelScreen::new()),
                badge: Mutex::new(PlanBadge::default()),
            }
        }

        /// The engine, for tests that inspect recorded denials.
        pub fn capabilities(&self) -> &Arc<lca_tools::Capabilities> {
            &self.cap
        }

        /// The live panel session, when one has started.
        pub fn panel_session(&self) -> Option<u32> {
            lock(&self.session).map(|session| session.handle)
        }

        /// What the panel has to draw, for a test or a host that wants it
        /// without going through the widget tree.
        pub fn panel_screen(&self) -> &Mutex<PanelScreen> {
            &self.screen
        }

        /// End the panel session and release the pty, if one is running.
        ///
        /// The one teardown both nets use: `on_session_close` calls it on the
        /// ordinary path, `Drop` on the abnormal one, and it is **safe to call
        /// twice** - the handle is taken out of the `Option` first, so the second
        /// call finds `None` and issues no `pty_kill`. That is the property that
        /// makes two nets free rather than a source of double-kill errors, and
        /// `panel_lifecycle.rs` pins it.
        ///
        /// Takes `&self` rather than `&mut self` because `on_session_close` gets
        /// one: the mutex is what makes the take exclusive.
        pub fn end_panel_session(&self) -> Option<u32> {
            let ended = lock(&self.session).take();
            if let Some(session) = ended {
                // A failure here is reported by the capability engine's own
                // denial/error record; there is nothing this extension can do
                // about it and nothing a user could act on, so it is not raised
                // as an effect. The handle is gone from this extension either
                // way, which is the part this code owns.
                let _ = self.cap.pty_kill(session.handle);
            }
            ended.map(|session| session.handle)
        }
    }

    impl Drop for UiExample {
        /// Gh #301: the second net. `on_session_close` is the ordinary path, but
        /// a panic, an early return, or a session that never closes cleanly all
        /// skip it - and the pty child is a real process.
        ///
        /// Worth being precise about what this does and does not buy: the host
        /// *does* reap the child eventually, because `Capabilities` owns the
        /// handle table and `PtyChild::drop` kills the tree. But the engine is an
        /// `Arc` shared with the session's other extensions, so "eventually" is
        /// "when the whole session's registry goes" - measured, not assumed: a
        /// dropped `UiExample` whose engine is still alive leaves a live `sh`
        /// behind (see the report for the probe). This closes that window.
        ///
        /// Idempotent for the same reason [`UiExample::end_panel_session`] is.
        fn drop(&mut self) {
            self.end_panel_session();
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
            if region != "panel" {
                return Ok(render_region_with(region, None, &lock(&self.badge))
                    .map(|nodes| WidgetTree { nodes }));
            }
            // The panel starts the demo session on its first render:
            // the user opening the panel IS the invocation (FR-UI-6's
            // shape - nothing starts on its own).
            let mut session = lock(&self.session);
            if session.is_none() {
                let args: Vec<String> = Vec::new();
                let spawned = self
                    .cap
                    .pty_spawn(demo_program(), &args, "workspace", 24, 80);
                if let Ok(handle) = spawned {
                    *session = Some(PanelSession { handle });
                }
            }
            // The live read: whatever the program has produced since the last
            // frame, folded into the rolling screen as data (the host draws it;
            // nothing here touches a terminal).
            //
            // Gh #301: the read and the fold happen on **every** panel frame, and
            // an empty read is a normal answer rather than a reason to draw
            // nothing. `Ok(None)` is the session's end, which the screen records
            // so the panel can say so instead of freezing on its last output.
            let mut screen = lock(&self.screen);
            if let Some(live) = session.as_ref() {
                match self.cap.pty_read(live.handle, 4096) {
                    Ok(Some(chunk)) => screen.observe(&chunk, None),
                    Ok(None) => screen.observe(&[], Some(0)),
                    // A refusal or an I/O error: the panel keeps what it has
                    // rather than blanking, and the engine's own record carries
                    // the reason.
                    Err(_) => {}
                }
            }
            let nodes = panel_nodes(session.as_ref(), Some(&screen));
            drop(screen);
            drop(session);
            Ok(Some(WidgetTree { nodes }))
        }

        fn on_ui_event(&self, region: &str, input: &UiInput) -> Result<UiEffect, DispatchError> {
            // Gh #409: `p` in the **panel** toggles plan mode.
            //
            // Not the status line, and that is a host contract rather than a
            // choice here: `footer` and `status-line` are display-only
            // (`chat_mouse.rs`: "their rows pack native and extension statuses
            // together, so a cell cannot name one owner"), so no key ever
            // reaches a handler for them. The panel is the region this extension
            // already owns and already receives keys in, so the toggle lives
            // there and the resulting state shows up in the badge.
            //
            // The badge still needs no notification: the host pulls `render`
            // per frame, so the next pull in `status-line` sees the new value.
            if region == "panel"
                && let UiInput::Key { key } = input
                && key == "p"
            {
                let mut badge = lock(&self.badge);
                *badge = if badge.active {
                    PlanBadge::default()
                } else {
                    PlanBadge {
                        active: true,
                        step: badge.step,
                    }
                };
                return Ok(UiEffect::None);
            }
            // Keys the modal and status branches do not handle reach the
            // live session: typing into the panel types into the program.
            if region == "panel"
                && let UiInput::Key { key } = input
                && let Some(session) = *lock(&self.session)
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

        /// Gh #301: the ordinary teardown net - the panel's child is a real
        /// process, so closing the session ends it rather than leaving it to the
        /// engine's own `Drop` whenever that finally runs.
        ///
        /// **The dispatch is not wired for a `ui`-only extension**, and that is a
        /// host finding rather than something this crate can fix:
        /// `ExtensionRegistry::on_session_close` filters on `World::Hooks`, so a
        /// handle declaring only `World::Ui` is never called. The method is here
        /// because it is the correct half of the contract and the interface the
        /// ABI defines; `Drop` is what actually runs (see the report).
        fn on_session_close(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
            self.end_panel_session();
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
            "lca:host/log@0.6.0": generate,
            "lca:host/ui@0.6.0": generate,
            "lca:host/ui-dialogs@0.6.0": generate,
            // ADR-0030's bags: every world imports them.
            "lca:host/resources@0.6.0": generate,
            "lca:host/state@0.6.0": generate,
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
            W::StyledText { content, style } => WasmWidget::StyledText((
                content,
                lca::ext::types::TextStyle {
                    fg: style.fg,
                    bg: style.bg,
                    bold: style.bold,
                    dim: style.dim,
                    italic: style.italic,
                    underline: style.underline,
                },
            )),
            W::Markdown { source } => WasmWidget::Markdown(source),
            W::Button { id, label } => WasmWidget::Button((id, label)),
            W::Table { headers, rows } => WasmWidget::Table((headers, rows)),
            W::ScrollContainer {
                max_height,
                children,
            } => WasmWidget::ScrollContainer((max_height, children)),
            W::Image { media_type, bytes } => WasmWidget::Image((media_type, bytes)),
            W::Boxed {
                title,
                border,
                background,
                child,
            } => WasmWidget::Boxed((title, border, background, child)),
            W::Row(children) => WasmWidget::Row(children),
            W::Column(children) => WasmWidget::Column(children),
            W::Spinner { frames } => WasmWidget::Spinner(frames),
            W::Progress { label, fill } => WasmWidget::Progress((label, fill)),
            W::KeyValue(pairs) => WasmWidget::Keyvalue(pairs),
            W::Vendor(kind) => WasmWidget::Vendor(kind),
        }
    }

    /// The state key plan mode lives under (gh #409).
    ///
    /// The `state` bag is the right home for it and the only one that survives
    /// this delivery mode: a WASM guest here is instantiated per call (the panel
    /// docs say so), so a `static` would be reset every time. ADR-0030's bag is
    /// exactly "the extension's own mutable, non-secret data", which is what a
    /// mode toggle is.
    const PLAN_KEY: &str = "plan-mode";

    /// Read plan mode from the bag. A missing key, a refused read, or a value
    /// that is not the two bytes this writes all mean "off" - the bag is storage
    /// the extension owns, so an unexpected value is a state to recover from, not
    /// an error to show.
    fn read_plan() -> PlanBadge {
        match lca::host::state::read(PLAN_KEY) {
            Some(bytes) if bytes.len() == 2 => PlanBadge {
                active: bytes[0] == b'1',
                step: None,
            },
            // A missing key and an unexpected value both mean "off": the bag is
            // storage this extension owns, so an unknown value is a state to
            // recover from rather than an error to show.
            _ => PlanBadge::default(),
        }
    }

    /// Persist plan mode. A failed write leaves the mode off for the next call,
    /// which is the safe direction: better to lose a toggle than to show a mode
    /// the extension cannot prove is on.
    fn write_plan(badge: &PlanBadge) {
        let value = if badge.active { b"1\0" } else { b"0\0" };
        let _ = lca::host::state::write(PLAN_KEY, value);
    }

    pub struct UiExampleWasm;

    impl RenderGuest for UiExampleWasm {
        fn render(region: String) -> Option<Vec<WasmWidget>> {
            render_region_with(&region, None, &read_plan())
                .map(|nodes| nodes.into_iter().map(to_wit).collect())
        }
    }

    impl InteractionGuest for UiExampleWasm {
        fn handle(region: String, input: WasmInput) -> WasmEffect {
            // Gh #409: `p` toggles plan mode in the status line. The toggle is
            // the whole mechanism - the host pulls `render` per frame, so the
            // next pull reads the new value from the bag. No redraw request, and
            // in particular none per frame: the #389 footer lesson.
            // The panel owns keys; `status-line`/`footer` are display-only in
            // the host, so a toggle there would never be delivered.
            if region == "panel"
                && let WasmInput::Key(key) = &input
                && key == "p"
            {
                let mut badge = read_plan();
                badge.active = !badge.active;
                write_plan(&badge);
                return WasmEffect::None;
            }
            let input = match input {
                WasmInput::Key(key) => lca_protocol::UiInput::Key { key },
                WasmInput::Submit(text) => lca_protocol::UiInput::Submit { text },
                WasmInput::Cancel => lca_protocol::UiInput::Cancel,
                WasmInput::ClickWidget(id) => lca_protocol::UiInput::ClickWidget { id },
                WasmInput::Click((col, row)) => lca_protocol::UiInput::Click { col, row },
                WasmInput::Scroll(delta) => lca_protocol::UiInput::Scroll { delta },
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
