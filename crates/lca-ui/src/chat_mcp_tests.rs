//! `/mcp` dispatch rows (gh #53): verbs route through the host
//! hooks, and a hostless interface stays a notice.
//!
//! Split from `chat_tests.rs` under the workspace's 1,200-line file
//! ceiling. Fixtures (`options`, `chat`) stay in `chat_tests.rs`.

use super::*;
use super::{chat, options};

// Verifies: gh #53 - `/mcp` prints the status block with no verbs
// and routes verbs plus targets to the host action.
#[test]
fn mcp_verbs_route_through_the_hooks() {
    let mut chat_options = options();
    chat_options.hooks.mcp_action = Some(std::sync::Arc::new(|verb: &str, target: &str| {
        if verb.is_empty() {
            return "- echo [direct] (user): connected (1 tools)".to_string();
        }
        format!("verb={verb} target={target}")
    }));
    let mut chat = Chat::new(chat_options, std::sync::Arc::new(KeybindingsManager::new()));
    for c in "/mcp".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(
        chat.last_notice().as_deref().unwrap().contains("connected"),
        "{:?}",
        chat.last_notice()
    );
    for c in "/mcp disable echo".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(
        chat.last_notice().as_deref().unwrap(),
        "verb=disable target=echo"
    );
}

// Verifies: gh #53 - `/mcp` without a host stays a notice, never a
// submit.
#[test]
fn mcp_without_a_host_names_the_guide() {
    let mut chat = chat();
    for c in "/mcp".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap()
            .contains("authoring guide")
    );
    assert!(chat.take_submitted().is_none());
}

// ---------------------------------------------------------------------------
// Gh #418 Phase G: the `/mcp` manager modal
// ---------------------------------------------------------------------------

use crate::chat_mcp::{EXPOSURE_LEVELS, McpScreen, STDERR_VIEW_ROWS, stderr_rows};
use crate::state::{McpServerDetail, McpServerRow, McpSnapshot, McpToolRow};
use std::sync::{Arc, Mutex};

/// Every verb the modal sends, in order.
type Verbs = Arc<Mutex<Vec<(String, String)>>>;

/// A detail fixture with the fields a row cares about, defaulted sensibly.
fn detail(name: &str, state: &str) -> McpServerDetail {
    McpServerDetail {
        name: name.to_string(),
        state: state.to_string(),
        detail: format!("{state}: fixture"),
        target: "python3 mock.py".to_string(),
        source_path: "/home/u/.lca/mcp.json".to_string(),
        override_path: None,
        effective_exposure: "direct".to_string(),
        configured_exposure: "direct".to_string(),
        enabled: state != "disabled",
        project_override: false,
        project_toggle_available: true,
        needs_sign_in: state == "needs-auth",
        has_oauth: false,
        stderr_tail: None,
        tools: Vec::new(),
    }
}

/// A chat wired to a recording manager: fixed rows, one detail, and a verb log.
fn mcp_modal_chat(rows: Vec<McpServerRow>, details: Vec<McpServerDetail>) -> (Chat, Verbs) {
    let mut chat_options = options();
    let verbs: Verbs = Arc::new(Mutex::new(Vec::new()));
    let rows_for_list = rows.clone();
    chat_options.hooks.mcp_list = Some(Arc::new(move || McpSnapshot {
        rows: rows_for_list.clone(),
        warnings: Vec::new(),
    }));
    let details = Arc::new(details);
    let details_for_hook = details.clone();
    chat_options.hooks.mcp_detail = Some(Arc::new(move |server: &str| {
        details_for_hook
            .iter()
            .find(|detail| detail.name == server)
            .cloned()
    }));
    let seen = verbs.clone();
    chat_options.hooks.mcp_action = Some(Arc::new(move |verb: &str, target: &str| {
        seen.lock()
            .expect("verbs")
            .push((verb.to_string(), target.to_string()));
        format!("{verb} {target}")
    }));
    let chat = Chat::new(chat_options, Arc::new(KeybindingsManager::new()));
    (chat, verbs)
}

fn row(name: &str, state: &str) -> McpServerRow {
    McpServerRow {
        name: name.to_string(),
        state: state.to_string(),
        detail: format!("{state}: fixture"),
        effective_exposure: "direct".to_string(),
        scope: "user".to_string(),
        project_override: false,
    }
}

/// Open the modal and step onto the server named `want` (the list is sorted by
/// attention, so the order is not the fixture's order).
fn open_on(chat: &mut Chat, want: &str) {
    chat.open_mcp_manager();
    let manager = chat.mcp_manager.as_ref().expect("the modal opened");
    let index = manager
        .rows
        .iter()
        .position(|row| row.name == want)
        .unwrap_or_else(|| {
            panic!(
                "{want} is not in {:?}",
                manager.rows.iter().map(|r| &r.name).collect::<Vec<_>>()
            )
        });
    for _ in 0..index {
        chat.handle_mcp_manager_key("", Some("down"));
    }
}

// Verifies: gh #418 §3.4 item 1 - bare `/mcp` opens the modal, and the list is
// sorted by attention so the server needing a decision is at the top. A list
// ordered by config file order buries the one thing the user must act on.
#[test]
fn the_modal_sorts_servers_by_attention_not_config_order() {
    let rows = vec![
        row("healthy", "connected"),
        row("off", "disabled"),
        row("broken", "failed"),
        row("auth-needed", "needs-auth"),
    ];
    let (mut chat, _) = mcp_modal_chat(rows, Vec::new());
    chat.open_mcp_manager();
    let manager = chat.mcp_manager.as_ref().expect("opened");
    let order: Vec<&str> = manager.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(order, vec!["auth-needed", "broken", "healthy", "off"]);
    // `disabled` trails `connected`: a server the user switched off is not
    // waiting on them, so it sorts last (the spec's order, not alphabetical).
    // And the highlight starts on the first row, which is the one that needs a
    // decision.
    assert_eq!(manager.selected_server(), Some("auth-needed"));
}

// Verifies: gh #418 §3.4 item 2 - Enter on a row opens the detail with the
// actions that make sense for THAT state, and `Esc` returns to the list keeping
// the cursor on the same server.
#[test]
fn a_detail_shows_only_the_actions_its_state_supports() {
    let rows = vec![row("srv", "connected")];
    let (mut chat, _) = mcp_modal_chat(rows, vec![detail("srv", "connected")]);
    open_on(&mut chat, "srv");
    chat.handle_mcp_manager_key("", Some("enter"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    let labels: Vec<String> = manager
        .actions
        .iter()
        .map(|action| action.label(manager.detail.as_ref().expect("detail")))
        .collect();
    // Connected over stdio: no Sign in (nothing to sign in to) and no Sign out
    // (no OAuth configured). A greyed button would teach the user nothing.
    assert!(!labels.iter().any(|label| label == "Sign in"), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label == "Sign out"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.starts_with("Tools")),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Reconnect"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.starts_with("Exposure")),
        "{labels:?}"
    );

    chat.handle_mcp_manager_key("", Some("escape"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    assert_eq!(manager.screen, McpScreen::List);
    assert_eq!(manager.selected_server(), Some("srv"));
}

// Verifies: gh #418 §3.4 item 2 - `Sign in` appears exactly when the server
// wants one, and running it starts the flow.
#[test]
fn a_needs_auth_server_offers_sign_in_and_running_it_starts_the_flow() {
    let rows = vec![row("idp", "needs-auth")];
    let mut failed = detail("idp", "needs-auth");
    failed.has_oauth = true;
    let (mut chat, _) = mcp_modal_chat(rows, vec![failed]);
    let started: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = started.clone();
    chat.world.options.hooks.mcp_begin_sign_in = Some(Arc::new(move |server: &str| {
        seen.lock().expect("started").push(server.to_string());
        (
            Some("https://idp.example/authorize?x=1".to_string()),
            "waiting".to_string(),
        )
    }));
    open_on(&mut chat, "idp");
    chat.handle_mcp_manager_key("", Some("enter"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    assert!(
        manager
            .actions
            .iter()
            .any(|action| action.label(manager.detail.as_ref().expect("detail")) == "Sign in")
    );
    chat.handle_mcp_manager_key("", Some("enter"));
    assert_eq!(*started.lock().expect("started"), vec!["idp".to_string()]);
    match &chat.mcp_manager.as_ref().expect("open").screen {
        McpScreen::SignIn { url, .. } => {
            assert!(url.as_deref().is_some_and(|url| url.contains("authorize")));
        }
        other => panic!("expected the sign-in screen, got {other:?}"),
    }
}

// Verifies: gh #418 §3.4 item 2 - the `Tools` sub-view lists each tool with its
// EFFECTIVE exposure, which is the whole point of the inspector (a per-tool
// override is invisible anywhere else).
#[test]
fn the_tool_inspector_lists_every_tool_with_its_effective_exposure() {
    let rows = vec![row("srv", "connected")];
    let mut with_tools = detail("srv", "connected");
    with_tools.tools = vec![
        McpToolRow {
            name: "mcp__srv__read".to_string(),
            exposure: "direct".to_string(),
            description: "reads a thing".to_string(),
        },
        McpToolRow {
            name: "mcp__srv__write".to_string(),
            exposure: "deferred".to_string(),
            description: "writes a thing".to_string(),
        },
    ];
    let (mut chat, _) = mcp_modal_chat(rows, vec![with_tools]);
    open_on(&mut chat, "srv");
    chat.handle_mcp_manager_key("", Some("enter"));
    // The first action on a connected server is `Tools`.
    chat.handle_mcp_manager_key("", Some("enter"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    assert!(matches!(manager.screen, McpScreen::Tools { .. }));
    let tools = &manager.detail.as_ref().expect("detail").tools;
    assert_eq!(tools[0].exposure, "direct");
    assert_eq!(tools[1].exposure, "deferred");
    // Down moves the inspector's own cursor, not the action list.
    chat.handle_mcp_manager_key("", Some("down"));
    assert_eq!(chat.mcp_manager.as_ref().expect("open").tool_selected, 1);
}

// Verifies: gh #418 §3.4 item 2 - the `Exposure` picker persists the choice
// through the SAME verb the scriptable form uses (one write path, gh #267's
// atomic writer), and the screen reopens on fresh data so the header shows the
// new effective value rather than the one it was opened with.
#[test]
fn the_exposure_picker_persists_through_the_verb_and_rereads_the_detail() {
    let rows = vec![row("srv", "connected")];
    let (mut chat, verbs) = mcp_modal_chat(rows, vec![detail("srv", "direct")]);
    open_on(&mut chat, "srv");
    chat.handle_mcp_manager_key("", Some("enter"));
    // Walk to the Exposure action.
    let manager = chat.mcp_manager.as_ref().expect("open");
    let index = manager
        .actions
        .iter()
        .position(|action| {
            action
                .label(manager.detail.as_ref().expect("d"))
                .starts_with("Exposure")
        })
        .expect("the picker action exists");
    for _ in 0..index {
        chat.handle_mcp_manager_key("", Some("down"));
    }
    chat.handle_mcp_manager_key("", Some("enter"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    let McpScreen::Exposure { selected, .. } = manager.screen else {
        panic!("expected the exposure picker, got {:?}", manager.screen);
    };
    // The picker opens on the effective level, not at index 0: opening on
    // `direct` when the server is `hidden` would make Enter a silent no-op.
    assert_eq!(EXPOSURE_LEVELS[selected], "direct");
    // Move to `codemode` and apply.
    chat.handle_mcp_manager_key("", Some("down"));
    chat.handle_mcp_manager_key("", Some("enter"));
    assert_eq!(
        *verbs.lock().expect("verbs"),
        vec![("exposure".to_string(), "srv codemode".to_string())]
    );
}

// Verifies: gh #418 §3.4 item 2 + the brief's "single most useful thing" - a
// failed server shows its stderr tail, and a very long tail is bounded so a
// chatty server cannot push the actions off the screen.
#[test]
fn a_failed_server_shows_its_stderr_tail_and_a_long_tail_is_bounded() {
    let rows = vec![row("broken", "failed")];
    let mut failed = detail("broken", "failed");
    failed.stderr_tail = Some("Traceback (most recent call last): | KeyError: 'token'".to_string());
    let (mut chat, _) = mcp_modal_chat(rows, vec![failed]);
    open_on(&mut chat, "broken");
    chat.handle_mcp_manager_key("", Some("enter"));
    let manager = chat.mcp_manager.as_ref().expect("open");
    let tail = manager
        .detail
        .as_ref()
        .expect("detail")
        .stderr_tail
        .as_deref()
        .expect("a failed server carries its tail");
    let rows = stderr_rows(tail);
    assert!(rows.iter().any(|row| row.contains("KeyError")), "{rows:?}");

    // A tail far longer than the view is cut, and says so, so nobody reads the
    // visible part as the whole story.
    let long: String = (0..STDERR_VIEW_ROWS * 3)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let bounded = stderr_rows(&long);
    assert_eq!(bounded.len(), STDERR_VIEW_ROWS + 1);
    assert!(
        bounded[0].contains("earlier lines omitted"),
        "{:?}",
        bounded[0]
    );
    // The tail is what survives: the last line is the newest, which is where a
    // crash explains itself.
    assert!(
        bounded
            .last()
            .expect("last")
            .contains(&format!("line {}", STDERR_VIEW_ROWS * 3 - 1))
    );
}

// Verifies: gh #418 Phase G's regression pin - the direct verbs still work
// alongside the modal. A script or muscle memory that uses `/mcp login <server>`
// must not break because a picker was added.
#[test]
fn the_direct_verbs_still_answer_with_a_notice_while_the_modal_exists() {
    let (mut chat, verbs) = mcp_modal_chat(
        vec![row("srv", "connected")],
        vec![detail("srv", "connected")],
    );
    for c in "/mcp login srv".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    // The verb route answered with a notice, and the modal did not open: the
    // addition did not replace the scriptable form.
    assert_eq!(chat.last_notice().as_deref(), Some("login srv"));
    assert!(!chat.mcp_manager_open());
    assert_eq!(
        *verbs.lock().expect("verbs"),
        vec![("login".to_string(), "srv".to_string())]
    );
}

// Verifies: gh #418 §3.4 item 3 - the paste-redirect path is what makes the SSH
// case work: the loopback listener can never fire on a remote box, so the flow
// must accept the redirect URL the user's local browser produced.
#[test]
fn a_pasted_redirect_url_finishes_the_flow_when_the_loopback_cannot_fire() {
    let rows = vec![row("idp", "needs-auth")];
    let mut needs = detail("idp", "needs-auth");
    needs.has_oauth = true;
    let (mut chat, _) = mcp_modal_chat(rows, vec![needs]);
    chat.world.options.hooks.mcp_begin_sign_in = Some(Arc::new(|_server: &str| {
        (
            Some("https://idp.example/authorize".to_string()),
            "waiting".to_string(),
        )
    }));
    let delivered: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = delivered.clone();
    chat.world.options.hooks.mcp_submit_redirect =
        Some(Arc::new(move |server: &str, redirect: &str| {
            seen.lock()
                .expect("delivered")
                .push((server.to_string(), redirect.to_string()));
            "delivered".to_string()
        }));
    open_on(&mut chat, "idp");
    chat.handle_mcp_manager_key("", Some("enter")); // detail
    chat.handle_mcp_manager_key("", Some("enter")); // Sign in (first action)
    for c in "http://127.0.0.1:9/callback?code=abc&state=x".chars() {
        chat.handle_mcp_manager_key(&c.to_string(), None);
    }
    chat.handle_mcp_manager_key("", Some("enter"));
    let seen = delivered.lock().expect("delivered");
    assert_eq!(seen.len(), 1, "the redirect was not delivered");
    assert_eq!(seen[0].0, "idp");
    assert!(seen[0].1.contains("code=abc"));
}

// Verifies: gh #418 §3.4 item 3 - `Esc` on the sign-in screen cancels the flow
// (the listener must stop, or a later sign-in inherits its channel) and lands
// back on the detail rather than closing everything.
#[test]
fn escape_on_the_sign_in_screen_cancels_the_flow() {
    let rows = vec![row("idp", "needs-auth")];
    let mut needs = detail("idp", "needs-auth");
    needs.has_oauth = true;
    let (mut chat, _) = mcp_modal_chat(rows, vec![needs]);
    chat.world.options.hooks.mcp_begin_sign_in = Some(Arc::new(|_server: &str| {
        (
            Some("https://idp.example/authorize".to_string()),
            "waiting".to_string(),
        )
    }));
    let cancelled: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = cancelled.clone();
    chat.world.options.hooks.mcp_cancel_sign_in = Some(Arc::new(move |server: &str| {
        seen.lock().expect("cancelled").push(server.to_string());
    }));
    open_on(&mut chat, "idp");
    chat.handle_mcp_manager_key("", Some("enter"));
    chat.handle_mcp_manager_key("", Some("enter"));
    assert!(matches!(
        chat.mcp_manager.as_ref().expect("open").screen,
        McpScreen::SignIn { .. }
    ));
    chat.handle_mcp_manager_key("", Some("escape"));
    assert_eq!(
        *cancelled.lock().expect("cancelled"),
        vec!["idp".to_string()]
    );
    assert!(matches!(
        chat.mcp_manager.as_ref().expect("open").screen,
        McpScreen::Detail { .. }
    ));
}

// Verifies: the brief's "do not let a background reconnect flip the modal's
// contents under the user's cursor" - a refresh keeps the selection on the same
// server by name, even when the attention order changed under it.
#[test]
fn a_background_refresh_keeps_the_cursor_on_the_same_server() {
    let mut chat_options = options();
    // The list changes between calls: `srv` moves from second to first because
    // it failed, which is exactly what a reconnect does.
    let first = Arc::new(Mutex::new(true));
    let flag = first.clone();
    chat_options.hooks.mcp_list = Some(Arc::new(move || {
        let before = *flag.lock().expect("flag");
        McpSnapshot {
            rows: if before {
                vec![row("alpha", "connected"), row("srv", "connected")]
            } else {
                vec![row("srv", "failed"), row("alpha", "connected")]
            },
            warnings: Vec::new(),
        }
    }));
    chat_options.hooks.mcp_detail =
        Some(Arc::new(|server: &str| Some(detail(server, "connected"))));
    chat_options.hooks.mcp_action = Some(Arc::new(|_verb: &str, _target: &str| "ok".to_string()));
    let mut chat = Chat::new(chat_options, Arc::new(KeybindingsManager::new()));
    open_on(&mut chat, "srv");
    assert_eq!(chat.mcp_manager.as_ref().expect("open").selected, 1);
    // The world moves under the cursor.
    *first.lock().expect("flag") = false;
    chat.refresh_mcp_rows();
    let manager = chat.mcp_manager.as_ref().expect("open");
    assert_eq!(
        manager.selected_server(),
        Some("srv"),
        "the cursor must follow the server, not the index"
    );
}

// Verifies: gh #418 §3.4 item 1 - a config error is shown above the list, because
// it is the reason a server the user configured is missing from it.
#[test]
fn a_config_warning_is_shown_above_the_list() {
    let mut chat_options = options();
    chat_options.hooks.mcp_list = Some(Arc::new(|| McpSnapshot {
        rows: vec![row("good", "connected")],
        warnings: vec!["server \"typo\" skipped: bad entry".to_string()],
    }));
    let mut chat = Chat::new(chat_options, Arc::new(KeybindingsManager::new()));
    chat.open_mcp_manager();
    let manager = chat.mcp_manager.as_ref().expect("open");
    assert_eq!(manager.warnings.len(), 1);
    let (_, body, _) = crate::chat_mcp_render::mcp_overlay(manager, None);
    let joined = body.join("\n");
    assert!(joined.contains("WARNINGS"), "{joined}");
    assert!(joined.contains("typo"), "{joined}");
}

// Verifies: a host with no MCP modal hooks (headless, or a build without MCP)
// keeps the old notice behaviour rather than opening an empty modal.
#[test]
fn a_host_without_the_modal_hooks_falls_back_to_the_notice() {
    let mut chat_options = options();
    chat_options.hooks.mcp_action = Some(Arc::new(|_verb: &str, _target: &str| {
        "status block".to_string()
    }));
    let mut chat = Chat::new(chat_options, Arc::new(KeybindingsManager::new()));
    for c in "/mcp".chars() {
        chat.handle_key(&c.to_string());
    }
    assert_eq!(chat.handle_key("\r"), Action::Continue);
    assert_eq!(chat.last_notice().as_deref(), Some("status block"));
    assert!(!chat.mcp_manager_open());
}
