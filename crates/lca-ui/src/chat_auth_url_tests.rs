//! Gh #398 gap 1: the sign-in URL over SSH/tmux.
//!
//! A 250-character OAuth URL wraps across three or four rows. Mouse-copy over
//! SSH or in tmux grabs the `\n`s at the soft-wrap seams and the padding the
//! modal draws around the text, so what lands in the browser is a broken URL.
//! `OSC 8` click-through is unavailable exactly where that matters. The URL is
//! the only way to finish the login, so the user currently has to reconstruct it
//! by hand.
//!
//! Two halves, and the tests are split the same way:
//!
//! 1. **the copy is the URL and nothing else** - byte equality against the
//!    original, because "looks right" is exactly the assertion that misses a
//!    trailing `\n` or a `│` fragment;
//! 2. **the copy is discoverable and its result visible in place** - a short
//!    hint naming the live key binding, and `Copied URL to clipboard` where the
//!    hint was, so a reader who pressed the key knows it worked.
//!
//! The clickable label is gated on `OSC 8` availability (the brief's rule: never
//! render "click to open" on a terminal that cannot click), so the rows come in
//! pairs - one with hyperlinks advertised, one without.
//!
//! Verifies: gh #398 gap 1, FR-UI-5 (the SSH path is a first-class renderer),
//! NFR-24.

use super::*;
use super::{chat, options, strip};

/// A URL long enough to wrap several rows at 80 columns, with the `&`/`=`/`%`
/// that make the pad-corruption case visible.
fn long_url() -> String {
    format!(
        "https://accounts.example.test/o/oauth2/v2/auth?response_type=code&client_id={}\
         &redirect_uri=https%3A%2F%2Flocalhost%3A8080%2Fcallback&scope=openid%20email%20profile\
         &state=xyzzy&code_challenge=abc123&code_challenge_method=S256",
        "7".repeat(80)
    )
}

/// A chat with the sign-in URL shown on the waiting screen, as the CLI drives
/// it (`LoginNext::Waiting`).
fn waiting_with(url: &str) -> Chat {
    let mut chat = chat();
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{url}"),
    });
    chat
}

// Alias so the import list stays short; `LoginNext` lives in `crate::state`.
use crate::state::LoginNext as lca_ui_LoginNext;

/// The whole modal as plain text, every row joined with `\n`.
fn plain_frame(chat: &Chat, width: u16) -> String {
    strip(&chat.viewport(width, 24, 0)).join("\n")
}

// ---------------------------------------------------------------------------
// Half 1: the copied string is the URL and nothing else.
// ---------------------------------------------------------------------------

// Verifies: gh #398 gap 1, the central row - **byte equality**, not "looks
// right". A URL that wraps across rows must reach the clipboard as the original
// string, with no `\n` at the wrap seams, no box padding, and no truncation
// marker. This is the defect: mouse-copy over SSH produced a broken URL, and a
// substring assertion would have passed on the broken one.
#[test]
fn the_copy_target_is_the_url_byte_for_byte() {
    let url = long_url();
    let chat = waiting_with(&url);
    let copied = chat
        .message_copy_text()
        .expect("the waiting screen has a copy target");
    assert_eq!(
        copied.as_bytes(),
        url.as_bytes(),
        "the copy target is the exact URL bytes - no newline, no padding, no ANSI"
    );
    assert!(!copied.contains('\n'), "no wrap newline survives the copy");
    assert!(
        !copied.contains('\u{1b}'),
        "no escape sequence reaches the clipboard"
    );
}

// Verifies: gh #398 gap 1 - a query string's `&`, `=` and `%` survive. Those
// are the characters a pad-corrupted or line-broken copy mangles, and they are
// what makes the URL invalid rather than merely ugly.
#[test]
fn a_query_string_survives_the_copy_unchanged() {
    let url = long_url();
    let chat = waiting_with(&url);
    let copied = chat.message_copy_text().expect("a copy target");
    for needle in ["&", "=", "%2F", "%20", "code_challenge_method=S256"] {
        assert!(
            copied.contains(needle),
            "`{needle}` survives the copy: {copied}"
        );
    }
    assert_eq!(copied, url);
}

// Verifies: gh #398 gap 1 - the raw `OSC 8` wrapper a caller may put in the
// label never reaches the clipboard. The CLI sends the URL pre-wrapped
// (`login.rs`'s `\x1b]8;;{page}\x07…`), and `sanitize_block` turns that into
// literal `\x1b]8;;` text; the copy path must see the URL, not wrapper bytes.
#[test]
fn an_explicit_osc8_wrapper_is_not_part_of_the_copy() {
    let url = "https://accounts.example.test/auth?code=42&state=zz";
    let mut chat = chat();
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!(
            "waiting for browser sign-in… (esc cancels)\n\n\x1b]8;;{url}\x07{url}\x1b]8;;\x07"
        ),
    });
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some(url),
        "wrapper bytes are never the copy target"
    );
}

// Verifies: gh #398 design point 5 - **the copy works at any step**, not just
// the first. The multi-step flow itself is gap 2 (out of scope), but the copy
// target must be whichever URL the current step shows.
#[test]
fn a_url_shown_by_a_later_step_is_still_the_copy_target() {
    let mut chat = chat();
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: "discovering login options… (esc cancels)".to_string(),
    });
    assert_eq!(
        chat.message_copy_text(),
        None,
        "no URL shown yet, so nothing URL-shaped is offered"
    );
    let second = "https://accounts.example.test/second-step?code=99";
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{second}"),
    });
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some(second),
        "the URL the *current* step shows is the target"
    );
}

// ---------------------------------------------------------------------------
// Half 2: discoverable, with the result visible in place.
// ---------------------------------------------------------------------------

// Verifies: gh #398 design point 3 - **in-place feedback**. After the copy the
// confirmation appears in the modal where the hint was, and the URL is still on
// screen. A toast alone would be a separate notice the reader has to hunt for,
// and the URL disappearing would be worse than no feedback.
#[test]
fn the_copy_confirmation_appears_in_place_and_keeps_the_url() {
    let url = long_url();
    let mut chat = waiting_with(&url);
    // The seeded clipboard writer records what the loop would have copied.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_hook = seen.clone();
    chat.world.options.hooks.copy_to_clipboard = Some(std::sync::Arc::new(move |text: &str| {
        seen_hook
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(text.to_string());
        true
    }));
    assert!(
        chat.message_copy_key("\x18"),
        "the busy wait owns the keyboard, and the copy key still reaches it"
    );

    // A tall pane: the MCP sign-in body is long (URL, hint, paste prompt), and a
    // 24-row overlay clips the bottom - the login-wait rows use the same height.
    let before = strip(&chat.viewport(80, 40, 0)).join("\n");
    assert!(before.contains("accounts.example.test"), "the URL is shown");

    chat.note_url_copied(true);
    let after = strip(&chat.viewport(80, 40, 0)).join("\n");
    assert!(
        after.contains("Copied URL to clipboard"),
        "the confirmation replaces the hint in place:\n{after}"
    );
    assert!(
        after.contains("accounts.example.test"),
        "the URL is not lost from the modal:\n{after}"
    );
}

// Verifies: gh #398 design point 3 - the confirmation is **transient**, riding
// the same mechanism the existing copy feedback uses. A permanent "Copied" would
// claim a state that is no longer true, and gh #398's own words are that this is
// the same transient mechanism, not a new one.
#[test]
fn the_copy_confirmation_fades_like_every_other_copy_notice() {
    let url = long_url();
    let mut chat = waiting_with(&url);
    chat.note_url_copied(true);
    assert!(plain_frame(&chat, 80).contains("Copied URL to clipboard"));
    // `tick` is the loop's own expiry pass (it calls `expire_toasts`); drive the
    // clock past the toast window rather than sleeping.
    chat.world.url_copied_at = Some(
        std::time::Instant::now() - std::time::Duration::from_secs(crate::chat::TOAST_SECS + 1),
    );
    chat.tick();
    let after = plain_frame(&chat, 80);
    assert!(
        !after.contains("Copied URL to clipboard"),
        "the confirmation fades:\n{after}"
    );
    assert!(after.contains("to copy"), "the hint returns:\n{after}");
}

// Verifies: gh #398 design point 2 / the #410 lesson - **the hint names the live
// binding**, read from the registry rather than hardcoded. A rebind must change
// the hint, which is the whole reason the project keeps one source for bindings.
#[test]
fn the_copy_hint_names_the_live_binding() {
    let url = long_url();
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("app.message.copy".to_string(), vec!["ctrl+g".to_string()]);
    let mut opts = options();
    opts.keybinding_overrides = overrides;
    let mut chat = Chat::new(
        opts,
        std::sync::Arc::new(
            lca_tui::engine::keybindings::KeybindingsManager::with_user_bindings(
                std::collections::BTreeMap::from([(
                    "app.message.copy".to_string(),
                    vec!["ctrl+g".to_string()],
                )]),
            ),
        ),
    );
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{url}"),
    });
    let frame = plain_frame(&chat, 80);
    assert!(
        frame.contains("ctrl+g") || frame.contains("Ctrl+G"),
        "the hint follows a rebind:\n{frame}"
    );
    assert!(
        !frame.contains("ctrl+x") && !frame.contains("Ctrl+X"),
        "and does not still say the default:\n{frame}"
    );
}

// Verifies: gh #398 design point 4 - **no clickable label where clicking cannot
// work**. `Cmd/Ctrl+click` is a lie on a terminal that does not forward OSC 8,
// and a stale hint is the failure mode this project keeps closing.
//
// The capability is forced off with `LCA_HYPERLINKS=0`, which is the documented
// override and the only route a test has (detection is `lca-tui`'s and reads the
// environment).
#[test]
fn the_click_label_is_absent_when_hyperlinks_are_off() {
    let url = long_url();
    let off = hyperlink_gated_frame(&url, false);
    assert!(
        !off.contains("click to open"),
        "no click affordance on a terminal that cannot click:\n{off}"
    );
    assert!(
        off.contains("to copy"),
        "the copy hint is still there - that path works everywhere:\n{off}"
    );
}

// Verifies: gh #398 design point 4, the other half - **the clickable label is
// present when the terminal advertises OSC 8**, as pi's `AuthUrlComponent`
// renders it (a short label that is itself the link).
#[test]
fn the_click_label_appears_when_hyperlinks_are_supported() {
    let url = long_url();
    let on = hyperlink_gated_frame(&url, true);
    assert!(
        on.contains("click to open"),
        "the short clickable label is shown:\n{on}"
    );
    // pi's `auth-url.ts`: the *label* is the link, so the short word opens the
    // full URL - that is the half which saves a reader from mouse-selecting
    // three wrapped rows. Asserted on the raw frame, because the escaped form
    // is exactly what `strip` removes.
    let label_link = format!("\x1b]8;;{url}\x07Ctrl+click to open\x1b]8;;\x07");
    let raw = hyperlink_gated_raw(&url, true);
    assert!(
        raw.contains(&label_link) || raw.contains(&label_link.replace("Ctrl+click", "Cmd+click")),
        "the label itself carries the link: {:?}",
        raw.lines().find(|l| l.contains("click to open"))
    );
}

/// Render the waiting modal with the hyperlink gate forced on or off.
///
/// `lca-tui` caches the capability per process (it reads the environment once),
/// so a row cannot flip it with an env var and expect a different frame. The
/// `Chat` carries the answer as a field - that is the same value the production
/// path reads - so overriding it here renders exactly what a terminal with that
/// capability would see.
fn hyperlink_gated_frame(url: &str, hyperlinks: bool) -> String {
    strip(&hyperlink_gated_rows(url, hyperlinks)).join("\n")
}

/// The same frame **with** its escape sequences, for the rows that must see the
/// raw `OSC 8` bytes (a stripped frame cannot show a link).
fn hyperlink_gated_raw(url: &str, hyperlinks: bool) -> String {
    hyperlink_gated_rows(url, hyperlinks).join("\n")
}

/// The frame as rows, with the hyperlink gate forced to `hyperlinks`.
fn hyperlink_gated_rows(url: &str, hyperlinks: bool) -> Vec<String> {
    let mut chat = chat();
    chat.hyperlinks = hyperlinks;
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!("waiting for browser sign-in… (esc cancels)\n\n{url}"),
    });
    chat.viewport(80, 24, 0)
}

// Verifies: gh #398 test policy - **a modal with no URL shows no copy
// affordance**. The "discovering login options…" step is a waiting screen with
// nothing to copy, and offering the hint there would send a reader to a key that
// copies the last assistant message instead.
#[test]
fn a_waiting_step_without_a_url_offers_no_copy_hint() {
    let mut chat = chat();
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: "discovering login options… (esc cancels)".to_string(),
    });
    let frame = plain_frame(&chat, 80);
    assert!(
        !frame.contains("to copy"),
        "no URL means no copy hint:\n{frame}"
    );
    assert!(
        !frame.contains("click to open"),
        "and no click label:\n{frame}"
    );
}

// Verifies: gh #398 gap 1 and gh #178 together - a label the CLI pre-wrapped in
// `OSC 8` renders as a *real* link, not as the escaped wrapper text.
//
// `sanitize_block` escapes control bytes, so the sequence `login.rs` sends
// (`\x1b]8;;{page}\x07…`) arrived in the modal as the literal characters
// `\x1b]8;;`, and the URL was printed twice behind visible garbage. Worse, the
// literal backslash before `http` stopped `linkify_urls` (it wants a whitespace
// boundary), so there was no link at all. Dropping the escaped wrapper leaves
// the bare URL, which the renderer links and every wrapped row re-opens.
#[test]
fn a_pre_wrapped_cli_label_renders_as_a_real_link() {
    let url = long_url();
    let mut chat = chat();
    chat.hyperlinks = true;
    chat.apply_login_next(lca_ui_LoginNext::Waiting {
        label: format!(
            "waiting for browser sign-in… (esc cancels)\n\n\x1b]8;;{url}\x07{url}\x1b]8;;\x07"
        ),
    });
    let raw = chat.viewport(80, 24, 0).join("\n");
    assert!(
        !raw.contains("\\x1b]8;;"),
        "the escaped wrapper is not printed as text: {raw:?}"
    );
    assert!(
        raw.contains(&format!("\x1b]8;;{url}\x07")),
        "the URL is a real OSC 8 link"
    );
    let plain = strip(&chat.viewport(80, 24, 0)).join("\n");
    let occurrences = plain.matches(url.as_str()).count();
    assert!(
        occurrences <= 1,
        "the URL is not printed twice in the same modal (found {occurrences})"
    );
}

// ---------------------------------------------------------------------------
// The second wrapped-URL surface: `/mcp` sign-in (gh #398 gap 1).
// ---------------------------------------------------------------------------

/// A chat with the `/mcp` sign-in screen open and showing `url`.
///
/// The MCP modal is the other place a 3-5 row authorization URL appears, and
/// over SSH it is the only route to the provider - so it is the same defect.
fn mcp_signin_with(url: &str) -> Chat {
    use crate::chat_mcp::{McpManager, McpScreen};
    let mut chat = chat();
    chat.mcp_manager = Some(McpManager {
        screen: McpScreen::SignIn {
            server: "remote".to_string(),
            url: Some(url.to_string()),
            pasted: String::new(),
            status: String::new(),
        },
        ..Default::default()
    });
    chat
}

// Verifies: gh #398 gap 1 - **the MCP sign-in URL is a copy target and the key
// reaches it**, even though the modal owns the keyboard. This was found live: a
// real 5-row wrapped URL on that screen ignored Ctrl+X entirely, so an SSH user
// had no route to the provider at all.
#[test]
fn the_mcp_sign_in_url_is_copied_by_the_same_key() {
    let url = long_url();
    let chat = mcp_signin_with(&url);
    assert!(
        chat.message_copy_key("\x18"),
        "the copy key reaches the MCP sign-in screen"
    );
    assert_eq!(
        chat.message_copy_text().as_deref(),
        Some(url.as_str()),
        "and its target is the exact URL"
    );
    assert!(chat.mcp_sign_in_url().is_some());
}

// Verifies: gh #398 gap 1 - the MCP sign-in screen names the copy key and shows
// the confirmation in place, so the two wrapped-URL surfaces behave the same
// way. Two surfaces with two different conventions is how a user learns not to
// trust either.
#[test]
fn the_mcp_sign_in_screen_names_the_copy_key_and_confirms_in_place() {
    let url = long_url();
    let mut chat = mcp_signin_with(&url);
    chat.hyperlinks = false;
    let before = plain_frame(&chat, 80);
    assert!(
        before.contains("to copy"),
        "the hint names the copy key:\n{before}"
    );
    assert!(
        !before.contains("click to open"),
        "and no click label where clicking cannot work:\n{before}"
    );

    chat.note_url_copied(true);
    let after = plain_frame(&chat, 80);
    assert!(
        after.contains("Copied URL to clipboard"),
        "the confirmation shows in place:\n{after}"
    );
    assert!(
        after.contains("accounts.example.test"),
        "and the URL is still on screen:\n{after}"
    );
}

// Verifies: gh #398 gap 1 - a sign-in screen with **no URL yet** offers no copy
// hint, the same rule the login wait follows. The status text is not a URL, and
// a hint there would send a reader to a key that copies something else.
#[test]
fn an_mcp_sign_in_screen_without_a_url_offers_no_hint() {
    use crate::chat_mcp::{McpManager, McpScreen};
    let mut chat = chat();
    chat.mcp_manager = Some(McpManager {
        screen: McpScreen::SignIn {
            server: "remote".to_string(),
            url: None,
            pasted: String::new(),
            status: "starting".to_string(),
        },
        ..Default::default()
    });
    assert_eq!(chat.mcp_sign_in_url(), None);
    let frame = plain_frame(&chat, 80);
    assert!(!frame.contains("to copy"), "no hint:\n{frame}");
}
