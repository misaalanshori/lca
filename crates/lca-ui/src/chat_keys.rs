//! The picker key handling, split from `chat.rs` (the 1,200-line ceiling).
//! Each picker owns the keyboard while open (FR-UI-16/17, R1/R2/R9, S8).

use super::chat::Chat;

/// Which way the fullscreen viewport was asked to jump (gh #375 gap 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportScroll {
    /// `tui.altScreen.top` - the transcript's first row.
    Top,
    /// `tui.altScreen.bottom` - the live bottom.
    Bottom,
    /// `tui.altScreen.pageUp` - one page towards the top (gh #375 gap 5).
    PageUp,
    /// `tui.altScreen.pageDown` - one page towards the live bottom.
    PageDown,
}

/// Rows a page scroll overlaps so a reader keeps their place (pi's
/// `PAGE_SCROLL_OVERLAP`).
pub const PAGE_SCROLL_OVERLAP: usize = 4;

/// One page's scroll distance for a window of `window` rows: the whole window
/// less [`PAGE_SCROLL_OVERLAP`], never less than one row - pi's
/// `Math.max(1, viewportHeight - PAGE_SCROLL_OVERLAP)`.
pub fn page_scroll_rows(window: usize) -> u16 {
    window
        .saturating_sub(PAGE_SCROLL_OVERLAP)
        .max(1)
        .min(u16::MAX as usize) as u16
}

/// The scroll a viewport-scroll key asks for, given the current scroll, the
/// content extent and the window height (gh #375 gaps 4 + 5).
///
/// One place decides what each target means, so the four cases cannot drift from
/// pi's arithmetic: `top` is content-less-window, `bottom` is the live end, and a
/// page is [`page_scroll_rows`] clamped to the scrollable range, so repeated
/// presses stop at the edge rather than running past it.
pub fn scroll_target(target: ViewportScroll, current: u16, content: usize, window: usize) -> u16 {
    let max = content.saturating_sub(window).min(u16::MAX as usize) as u16;
    match target {
        ViewportScroll::Bottom => 0,
        ViewportScroll::Top => max,
        ViewportScroll::PageUp => current.saturating_add(page_scroll_rows(window)).min(max),
        ViewportScroll::PageDown => current.saturating_sub(page_scroll_rows(window)),
    }
}

impl Chat {
    /// Whether a picker owns the keyboard (the modal is `modal_active`'s
    /// own question): the overlay test the viewport draws with, and the
    /// gate every contextual key asks first - gh #9's copy key among
    /// them.
    ///
    /// Gh #373 gap 5: the answer is **which surface is on top**, not which fields are
    /// set. A picker with a permission prompt above it is present but not in charge,
    /// and the old field test could not say so - which is how a prompt over a picker
    /// painted nothing while taking every key.
    pub(super) fn picker_open(&self) -> bool {
        self.top_layer() == Some(crate::layer_stack::Layer::Picker)
    }

    /// Whether a picker is raised at all, covered or not.
    ///
    /// Different question from [`Self::picker_open`], and the difference is the bug: a
    /// painter asking this must also ask [`Self::picker_covered`] (or composite in
    /// [`Self::layers`] order) to avoid drawing the wrong thing on top.
    pub(super) fn picker_raised(&self) -> bool {
        self.layers().contains(crate::layer_stack::Layer::Picker)
    }

    /// Whether this key is pi's message-copy key with the editor owning
    /// the keyboard (gh #9, pi 1.0.0's `app.message.copy`): while a
    /// picker or the transcript search has the keyboard, the key is
    /// theirs, not a copy request. Modals keep the keyboard - except
    /// the OAuth wait (gh #200): a waiting screen is not a question,
    /// and the copy target there is the sign-in URL itself.
    pub fn message_copy_key(&self, data: &str) -> bool {
        // Gh #398 gap 1: the MCP sign-in screen is the *other* place a wrapped
        // authorization URL appears, and over SSH it is the only way to reach
        // the provider. It owns the keyboard like a picker, but the copy key
        // has an unambiguous target there (the URL it is showing), so it is
        // allowed through - the same exception the OAuth wait already gets.
        if self.mcp_sign_in_url().is_some() {
            return self.keybindings.matches(data, "app.message.copy");
        }
        // Gh #373 gap 5: one question - who is on top - rather than three predicates
        // whose orders could disagree. The OAuth wait stays the exception it always
        // was: a waiting screen is not a question, and its copy target is the URL.
        match self.top_layer() {
            Some(crate::layer_stack::Layer::Panel) | None => {}
            Some(_) if self.world.login_waiting.is_none() => return false,
            Some(_) => {}
        }
        self.keybindings.matches(data, "app.message.copy")
    }

    /// The authorization URL the MCP sign-in screen is showing, if any
    /// (gh #398 gap 1): the `/mcp` modal's URL is a second wrapped-URL trap
    /// over SSH, and it is the same bytes the login wait copies.
    pub fn mcp_sign_in_url(&self) -> Option<String> {
        let manager = self.mcp_manager.as_ref()?;
        match &manager.screen {
            crate::chat_mcp::McpScreen::SignIn { url: Some(url), .. } if !url.is_empty() => {
                Some(url.clone())
            }
            _ => None,
        }
    }

    /// Whether this key is pi's jump-to-bottom with the fullscreen
    /// viewport owning the keyboard (gh #35): `tui.altScreen.bottom`
    /// (End) acts only in fullscreen - main-screen mode keeps End on the
    /// editor, whose scrollback has nothing to jump to - and only when
    /// no modal, picker, or the transcript search has the keyboard,
    /// mirroring pi's `shouldDeferViewportInputToOverlay`.
    pub fn alt_screen_bottom(&self, data: &str) -> bool {
        self.viewport_owns(data) && self.keybindings.matches(data, "tui.altScreen.bottom")
    }

    /// Whether the fullscreen **viewport** owns this key at all (gh #35).
    ///
    /// One gate for every viewport-scroll action, so a new one cannot be wired
    /// without the fullscreen / modal / picker / search checks. Mirrors pi's
    /// `shouldDeferViewportInputToOverlay`.
    fn viewport_owns(&self, _data: &str) -> bool {
        // Gh #373 gap 5: the viewport owns a scroll key only when **nothing** is above
        // it. Asking the stack is the same question the old three predicates asked,
        // now with one answer instead of three that had to all be false.
        self.screen_mode && self.top_layer().is_none()
    }

    /// The viewport-scroll target this key names, if the viewport owns it.
    ///
    /// Gh #375 gap 4: one gate for every viewport-scroll action, so a new one
    /// cannot be wired without the fullscreen / modal / picker / search checks.
    /// `tui.altScreen.top` was declared but never acted on, which is how `Home`
    /// came to be claimed by two actions whose matchers disagreed.
    ///
    /// Returns `ViewportScroll::Top` for `tui.altScreen.top`,
    /// `ViewportScroll::Bottom` for `tui.altScreen.bottom`, and the paging
    /// variants for `tui.altScreen.pageUp`/`pageDown`. Named without intra-doc
    /// links because the module is private, so a link to a variant is a
    /// `private_intra_doc_links` error under `-D warnings`.
    pub fn viewport_scroll(&self, data: &str) -> Option<ViewportScroll> {
        if !self.viewport_owns(data) {
            return None;
        }
        if self.keybindings.matches(data, "tui.altScreen.top") {
            return Some(ViewportScroll::Top);
        }
        if self.keybindings.matches(data, "tui.altScreen.bottom") {
            return Some(ViewportScroll::Bottom);
        }
        if self.keybindings.matches(data, "tui.altScreen.pageUp") {
            return Some(ViewportScroll::PageUp);
        }
        if self.keybindings.matches(data, "tui.altScreen.pageDown") {
            return Some(ViewportScroll::PageDown);
        }
        None
    }

    /// The text a copy request names when nothing is selected (gh #9,
    /// pi 1.0.0): the sign-in URL on a waiting login screen - what the
    /// person is actually waiting on - else the last assistant message
    /// as plain text (its stored source, escape sequences stripped).
    /// What reaches the clipboard, and with what honesty, is the loop's
    /// ladder in `run`.
    pub fn message_copy_text(&self) -> Option<String> {
        // Gh #398 gap 1: the waiting screen's URL is captured when the step
        // arrives (`apply_login_next`), not re-parsed here. The label is prose
        // with the URL inside it, and the clipboard needs the URL alone.
        if let Some(url) = self.world.auth_url.as_deref() {
            return Some(url.to_string());
        }
        // Gh #398 gap 1: the MCP sign-in screen's URL, before the last
        // assistant message (which is what a reader is *not* trying to copy
        // while a sign-in is waiting).
        if let Some(url) = self.mcp_sign_in_url() {
            return Some(url);
        }
        self.transcript
            .last_assistant_text()
            .map(lca_tui::engine::text::strip_terminal_sequences)
    }

    /// The first `http(s)://` URL in a block of text: the waiting login
    /// screen carries the sign-in URL inside a sentence, and that URL is
    /// what the copy key copies there (gh #9). The stored label passed
    /// through `sanitize_block`, which escapes the real OSC 8 wrapper
    /// (gh #178) into literal `\x1b` / `\x07` text - so a backslash
    /// ends the URL too, else the copy carries wrapper bytes (gh #200).
    /// Raw ESC/BEL end it as well, for labels that never sanitized.
    pub(crate) fn http_url_of(text: &str) -> Option<String> {
        Self::http_url(text)
    }

    fn http_url(text: &str) -> Option<String> {
        let start = text.find("https://").or_else(|| text.find("http://"))?;
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || c == '`' || c == '\\' || c == '\u{1b}' || c == '\u{7}'
            })
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(|c: char| ",.;)]}\"".contains(c));
        (!url.is_empty()).then(|| url.to_string())
    }
}
