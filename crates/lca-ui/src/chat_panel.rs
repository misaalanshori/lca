//! The side panel's visibility predicate (gh #373 gap 6), split from
//! `chat_overlays.rs` at the workspace's 1,200-line file ceiling.
//!
//! One predicate answers for the paint and for the keyboard, which is the whole
//! safety property: a panel hidden by its own breakpoint that still consumed
//! keystrokes would make the app feel possessed. Both the key gate and the
//! painter therefore ask this module, never `world.panel_open` directly.

use super::chat::Chat;
use super::chat_overlays::{HINT_LOGIN, modal_title_line, sensitive_read_name};
use super::ext_widgets::widget_lines;
use crate::render::overlay_box;

impl Chat {
    /// Whether the extension panel is actually on screen this frame (gh #373 gap 6).
    ///
    /// One predicate for the paint and for the key gate, for the same reason the
    /// `has_overlay` guard is one: two answers to one question is how a hidden panel
    /// goes on swallowing keystrokes.
    pub fn panel_shown(&self, width: u16, height: u16) -> bool {
        self.world.panel_open
            && self
                .panel_visible
                .as_ref()
                .is_none_or(|visible| visible(width, height))
    }

    /// Whether the panel is shown, against the size the last frame painted at.
    ///
    /// Input dispatch has no size parameter, and a key must not be judged against a
    /// different size than the frame the reader is looking at. Before the first frame
    /// there is no frame to disagree with, so the predicate is answered at 0x0 - which
    /// a breakpoint predicate reports as hidden, the safe default.
    pub(crate) fn panel_shown_now(&self) -> bool {
        let (width, height) = self
            .last_frame_size
            .get()
            .unwrap_or((self.world.size.0, self.world.size.1));
        self.panel_shown(width, height)
    }

    /// Paint the drawer tab over the margin (gh #207): `◀` opens, `▶`
    /// closes - the visible face of the Alt+X binding. One cell only,
    /// accent on hover, so an idle frame stays quiet.
    pub(super) fn paint_drawer_tab(&self, viewport: &mut [String], width: u16, height: u16) {
        let Some((col, row)) = self.drawer_rect(width, height) else {
            return;
        };
        let Some(line) = viewport.get_mut(row as usize) else {
            return;
        };
        let glyph = if self.panel_shown(width, height) {
            "▶"
        } else {
            "◀"
        };
        let role = if self.drawer_hover {
            crate::theme::Role::Accent
        } else {
            crate::theme::Role::Border
        };
        // gh #238: the glyph splices at the seam, so the row behind
        // cannot tint it and its accent cannot leak right.
        *line =
            crate::render::splice_segment(line, col as usize, 1, &(self.theme.role(role))(glyph));
    }
}

impl Chat {
    /// The raised layers, bottom first (gh #373 gap 5).
    ///
    /// **Derived, not stored.** The fifteen pickers, the modal surfaces, the panel and
    /// the search readout are the state; this is the *order over* that state, computed
    /// from it on every ask. That is deliberate: a stored stack would need a push at
    /// every one of the ~40 open paths and a pop at every close, and one missed pop is
    /// a stranded layer - the bug this cycle exists to remove, reintroduced as a
    /// maintenance hazard. Deriving makes drift unrepresentable, and costs about twenty
    /// `Option` tests per call.
    ///
    /// The order is the composite order, and it is the order the **key** path already
    /// used, which is the fix: `handle_modal_key` runs before `handle_picker_key`, so a
    /// moment above a picker was already in charge of keys while `compose_pickers`
    /// refused to paint it. One order, asked by every consumer, ends that.
    ///
    /// Rungs, bottom first:
    ///
    /// 1. `Search` — dock chrome, under everything.
    /// 2. `Panel` — the side panel (composited, never a keyboard owner).
    /// 3. `Wizard` — a stepped settings modal, drawn above the list it came from.
    /// 4. `Picker` — a searchable list.
    /// 5. `Modal` — a question: permission, login step, host dialog, extension modal.
    pub(crate) fn layers(&self) -> crate::layer_stack::LayerStack {
        let mut stack = crate::layer_stack::LayerStack::new();
        for (present, layer) in [
            (self.search.is_some(), crate::layer_stack::Layer::Search),
            (self.world.panel_open, crate::layer_stack::Layer::Panel),
            (
                self.picker_raised_by_fields(),
                crate::layer_stack::Layer::Picker,
            ),
            (
                self.modal_raised_by_fields(),
                crate::layer_stack::Layer::Modal,
            ),
        ] {
            if present {
                stack.raise(layer);
            }
        }
        stack
    }

    /// The layer the user is looking at and what takes the next key (gh #373 gap 5).
    ///
    /// `None` means **the editor has the keyboard** - the one fact that used to be
    /// expressed as three predicates all being false.
    ///
    /// Public because an embedder driving `Chat` from its own event loop needs exactly
    /// this to decide what a key means, and there is no other honest answer: the
    /// surface fields are `pub(crate)` and every predicate is derived from here.
    pub fn top_layer(&self) -> Option<crate::layer_stack::Layer> {
        self.layers().top()
    }

    /// Whether something is raised above the picker (gh #373 gap 5).
    ///
    /// The paint gate's question, and distinct from [`Self::top_layer`]: a picker can be
    /// raised and covered, in which case it is not painted and not in charge.
    pub(crate) fn picker_covered(&self) -> bool {
        self.layers().is_covered(crate::layer_stack::Layer::Picker)
    }

    /// Whether any picker's field is set, covered or not.
    fn picker_raised_by_fields(&self) -> bool {
        self.theme_picker.is_some()
            || self.thinking_picker.is_some()
            || self.model_picker.is_some()
            || self.tree_picker.is_some()
            || self.trust_picker.is_some()
            || self.resume_picker.is_some()
            || self.grants_picker.is_some()
            || self.fork_picker.is_some()
            || self.scoped_models_picker.is_some()
            || self.extension_hub.is_some()
            || self.extension_manager.is_some()
            || self.mcp_manager.is_some()
            || self.install_consent.is_some()
            || self.settings_picker.is_some()
            // The stepped wizard paints through `compose_pickers` and takes keys
            // through `handle_picker_key`, so it is a picker-rung layer.
            || self.world.wizard.is_some()
    }

    /// Whether any modal-shaped surface is up.
    ///
    /// **The union of what `handle_modal_key` dispatches on and what `compose_modals`
    /// paints**, which is the only definition that cannot strand a surface. The old
    /// `UiState::modal_active` was neither: it omitted `bug_review` and
    /// `switch_confirm`, both of which take keys and paint, so a `/bug` inspection
    /// screen was invisible to every predicate that asked about modals - including the
    /// keyboard-owner and the padding decision. Deriving the layer from the union fixes
    /// that as a side effect of asking one question.
    pub(crate) fn modal_raised_by_fields(&self) -> bool {
        // `wizard` is deliberately **not** here: `compose_pickers` paints it, and it
        // takes keys through `handle_picker_key`, so it belongs to the picker rung.
        // Listing it as a modal put it in a rung whose painter does not draw it, which
        // is exactly the "one surface, two answers" fault this cycle removes - caught
        // by the wizard's own paint row.
        self.world.modal_active()
            || self.world.dialog.is_some()
            // Painted by `compose_modals` and dispatched by `handle_modal_key`, but
            // absent from `modal_active`.
            || self.world.bug_review.is_some()
            || self.world.switch_confirm.is_some()
    }
}

impl Chat {
    pub(super) fn compose_modals(&self, viewport: &mut [String], width: u16, height: u16) {
        // A host-rendered dialog owns the screen outright (gh #124):
        // nothing stacks under it while it is open.
        if let Some(modal) = &self.world.dialog {
            self.compose_host_dialog(viewport, width, height, modal);
        } else if let Some(label) = &self.world.login_waiting {
            // R4: a cancellable waiting state, so a slow OAuth callback is
            // visible and interruptible instead of freezing the app.
            // Gh #398 gap 1: when the step shows a sign-in URL, the body
            // carries the copy hint and (where OSC 8 reaches the terminal) the
            // short clickable label, so an SSH reader never has to mouse-select
            // a wrapped URL.
            let body = self.login_body(label);
            overlay_box(viewport, width, height, "login", &body, &self.theme);
        } else if let Some(picker) = &self.world.picker {
            let rows = 15usize.min(height.saturating_sub(7) as usize).max(1);
            let total = picker.options.len();
            let start = (picker.selected + 1)
                .saturating_sub(rows)
                .min(total.saturating_sub(rows));
            let end = (start + rows).min(total);
            // The trail is not merged into a picker's body: `picker_overlay`
            // windows `body.len() - 2` rows as *items*, so prepending history
            // would make finished steps scrollable as choices. In every flow that
            // reaches this picker it is the first step, so the trail is empty.
            // shortcut: a provider that re-offers the picker mid-flow loses the
            // trail above it; merge through a header row if that ever happens.
            let mut body = vec!["Sign in with:".to_string(), String::new()];
            for (index, option) in picker.options.iter().enumerate().take(end).skip(start) {
                let cur = if index == picker.selected { '>' } else { ' ' };
                let hint = if option.hint.is_empty() {
                    String::new()
                } else {
                    format!("   {}", option.hint)
                };
                body.push(format!(" {cur} {}{hint}", option.label));
            }
            if start > 0 || end < total {
                body.push(format!("   [{}/{}]", picker.selected + 1, total));
            }
            self.picker_overlay(
                viewport,
                width,
                height,
                "login",
                &body,
                HINT_LOGIN,
                &self.theme,
                Some(2 + picker.selected.saturating_sub(start)),
            );
        } else if let Some(grant) = &self.world.grant {
            let current = vec![
                grant.prompt.clone(),
                String::new(),
                format!("  connect to {}", grant.host),
                String::new(),
                "Allow [y] / Deny [n]".to_string(),
            ];
            // Gh #398 gap 2: the consent is a *step of the flow*, not a separate
            // screen - it names the host the flow just collected, so the trail
            // above it is what makes the host checkable against what the user
            // typed.
            let body = self.with_trail(current);
            overlay_box(
                viewport,
                width,
                height,
                &format!("ad hoc grant: {}", grant.provider),
                &body,
                &self.theme,
            );
        } else if let Some(review) = &self.world.bug_review {
            // Gh #411 gap 2: the inspection is the gate. The body is the host's
            // rendered summary (file list, sizes, scrub counts), so the screen
            // never re-reads the session and cannot drift from what will be
            // written.
            let mut body: Vec<String> = review.inspection.lines().map(str::to_string).collect();
            body.push(String::new());
            body.push("Write the bundle [y] / Cancel [n]".to_string());
            overlay_box(
                viewport,
                width,
                height,
                "bug bundle - review before sharing",
                &body,
                &self.theme,
            );
        } else if let Some(switch) = &self.world.switch_confirm {
            let body = vec![
                switch.prompt.clone(),
                String::new(),
                "Switch [y] / Stay [n]".to_string(),
            ];
            overlay_box(
                viewport,
                width,
                height,
                &format!("switch provider: {}", switch.provider),
                &body,
                &self.theme,
            );
        } else if let Some(secret) = &self.world.secret {
            // Gh #371 bug 3: the marker sits at the **typed value's** insertion
            // point, which for a masked field is the end of the asterisks - not
            // the end of the secret. The mask is built first and the caret is
            // appended to it, so the anchor follows what the reader can see and
            // the plaintext never reaches the frame (secrets hygiene, and what
            // #411's inspection gate established).
            let (shown, placeholder) = if secret.masked {
                (
                    "*".repeat(secret.input.chars().count()),
                    "(type the secret)",
                )
            } else {
                (secret.input.clone(), "(type a value)")
            };
            let current = vec![
                secret.label.clone(),
                String::new(),
                self.caret_row("  ", &shown, placeholder, crate::chat_focus::Field::Secret),
                String::new(),
                "Enter stores it; Esc cancels.".to_string(),
            ];
            // Gh #398 gap 2: the steps that already happened stay above this one.
            let body = self.with_trail(current);
            overlay_box(
                viewport,
                width,
                height,
                &format!("login: {}", secret.provider),
                &body,
                &self.theme,
            );
        } else if let Some(modal) = &self.world.permission {
            let mut body = vec![
                "Allow this action?".to_string(),
                String::new(),
                format!("  {}", modal.action),
                String::new(),
            ];
            // Gh #215: a sensitive read names its security boundary —
            // allowing it exposes the file's contents to the active
            // model provider.
            if let Some(name) = sensitive_read_name(&modal.action) {
                body.push(format!(
                    "  ⚠️ Sensitive file read requested ({name}) — allowing this exposes its contents to the active model provider."
                ));
                body.push(String::new());
            }
            if let Some(deadline) = modal.deadline {
                let seconds = deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_secs();
                body.push(format!(
                    "auto-approves in {seconds}s - press a key to decide"
                ));
                body.push(String::new());
            }
            body.push(
                "[o] once / [a] always this pattern / [t] trust this folder this session / [d] deny"
                    .to_string(),
            );
            overlay_box(
                viewport,
                width,
                height,
                "permission required",
                &body,
                &self.theme,
            );
        } else if self.world.modal_open {
            let trees = self
                .world
                .options
                .render_regions
                .as_ref()
                .map(|render| render("modal"))
                .unwrap_or_default();
            let mut body: Vec<String> = Vec::new();
            let mut title = String::from("extension");
            for (name, tree) in &trees {
                for line in widget_lines(
                    &tree.nodes,
                    &crate::ext_widgets::widget_ctx(
                        &self.theme,
                        "modal",
                        width as usize,
                        &self.world.ext_scroll,
                    ),
                ) {
                    // A bordered box paints its title (gh #172): the
                    // chrome reads through the escapes.
                    if title == "extension"
                        && let Some(name) = modal_title_line(&line)
                    {
                        title = name;
                    }
                    body.push(line);
                }
                if body.is_empty() {
                    body.push(name.clone());
                }
            }
            overlay_box(viewport, width, height, &title, &body, &self.theme);
        }
    }
}
