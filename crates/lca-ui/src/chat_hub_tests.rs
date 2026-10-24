//! The `/extensions` hub journey: open, filter, navigate, consent,
//! approve, deny, close. Uses the `chat_tests` fixtures like the other
//! overlay journeys.

use super::{chat, options};
use crate::chat_hub::{HubData, InstallConsent};
use std::sync::Arc;

fn hub_chat() -> (
    crate::chat::Chat,
    Arc<std::sync::Mutex<Vec<InstallConsent>>>,
) {
    let mut opts = options();
    let confirmed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let confirm_host = confirmed.clone();
    opts.hooks.fetch_extensions = Some(Arc::new(move || {
        let rows = crate::chat_hub::fixture_rows();
        let installed = rows
            .iter()
            .filter(|row| row.installed)
            .cloned()
            .collect::<Vec<_>>();
        let available = rows
            .into_iter()
            .filter(|row| !row.installed)
            .collect::<Vec<_>>();
        HubData {
            installed,
            available,
            notice: None,
        }
    }));
    opts.hooks.confirm_install = Some(Arc::new(move |consent| {
        confirm_host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(consent);
    }));
    opts.hooks.poll_install = Some(Arc::new(|| None));
    let mut chat = chat();
    chat.world.options.hooks = opts.hooks;
    (chat, confirmed)
}

// Verifies: #227 - opening `/extensions` shows installed rows grouped
// ahead of catalog rows with provenance badges on each.
#[test]
fn hub_opens_with_sections_and_provenance() {
    let (mut chat, _) = hub_chat();
    chat.dispatch_extensions();
    assert!(chat.extension_hub.is_some(), "the hub opens");
    let hub = chat.extension_hub.as_mut().expect("hub");
    hub.drain_hub();
    assert!(!hub.loading, "rows settle the loader");
    assert_eq!(hub.matches.len(), 3);
    let first = hub.selected_row().expect("selection");
    assert!(first.installed, "installed rows lead");
}

// Verifies: #227 - typing filters to matching rows; Enter on an
// available row opens consent naming publisher, origin, artifact,
// and capabilities; approving fires the host install.
#[test]
fn hub_filter_enter_consent_and_approve() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    // Filter to the custom row.
    for c in "acme".chars() {
        chat.handle_extension_hub_key(&c.to_string(), None);
    }
    {
        let hub = chat.extension_hub.as_ref().expect("hub");
        assert_eq!(hub.matches.len(), 1);
        assert_eq!(
            hub.selected_row().map(|row| row.name.as_str()),
            Some("acme-deploy")
        );
    }
    // Enter opens consent with the row's provenance and capabilities.
    chat.handle_extension_hub_key("", Some("enter"));
    let consent = chat.install_consent.clone().expect("consent opens");
    assert_eq!(consent.name, "acme-deploy");
    assert!(consent.publisher.contains("[third-party]"));
    assert_eq!(consent.artifact_ref, "ghcr.io/acme/deploy:1.2.0");
    assert_eq!(consent.capabilities, vec!["process"]);
    // Approve fires the host install with the same data.
    chat.handle_install_consent_key("", Some("enter"));
    assert!(chat.install_consent.is_none(), "consent settles");
    let confirmed = confirmed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0], consent);
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap_or("")
            .contains("acme-deploy"),
        "progress shows: {:?}",
        chat.last_notice()
    );
}

// Verifies: #227 - denying (or Escaping) consent returns to the hub
// with nothing fired and nothing written.
#[test]
fn hub_deny_returns_without_firing() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    // Down to the first available row, then Enter opens consent.
    chat.handle_extension_hub_key("", Some("down"));
    chat.handle_extension_hub_key("", Some("enter"));
    assert!(chat.install_consent.is_some());
    chat.handle_install_consent_key("", Some("escape"));
    assert!(chat.install_consent.is_none(), "consent dismisses");
    assert!(chat.extension_hub.is_some(), "the hub stays open");
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "no install fires on deny"
    );
    // Esc closes the hub itself.
    chat.handle_extension_hub_key("", Some("escape"));
    assert!(chat.extension_hub.is_none(), "esc closes");
}

// Verifies: #227 - Enter on an installed row reports instead of
// installing: installs only flow for available rows.
#[test]
fn hub_enter_on_installed_reports() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    chat.handle_extension_hub_key("", Some("enter"));
    assert!(chat.install_consent.is_none(), "no consent for installed");
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap_or("")
            .contains("already installed"),
        "the notice says so: {:?}",
        chat.toasts
    );
}

// Verifies: gh #256 - one source, one label. An official row's
// consent publisher never names third-party/custom, and a custom
// row's never claims official — the live drive showed both
// contradictions at once (`Official (misaalanshori/lca) (Custom /
// Third-Party)`).
#[test]
fn provenance_labels_never_contradict() {
    for row in crate::chat_hub::fixture_rows() {
        let consent = crate::chat_hub::hub_install_consent(&row);
        if row.official {
            assert!(
                consent.publisher.contains("[official]"),
                "official row names its provenance: {}",
                consent.publisher
            );
            assert!(
                !consent.publisher.contains("Third-Party") && !consent.publisher.contains("Custom"),
                "official row contradicts itself: {}",
                consent.publisher
            );
        } else {
            assert!(
                !consent.publisher.contains("Official"),
                "custom row claims official: {}",
                consent.publisher
            );
        }
    }
}

// ===========================================================================
// Gh #329: the filter field takes raw bytes as text
// ===========================================================================
//
// The defect: the hub's fallthrough arm was
//
//     if let Some(text) = data.chars().find(|c| !c.is_control()) { query.push(text) }
//
// which reads `data` - the **raw input payload** - and keeps its first printable
// character. An ANSI sequence starts with ESC (a control char, skipped), and its
// second byte is `[` (printable), so `Left` (`\x1b[D`) typed `[`.
//
// Every other text field in `lca-ui` already routes through
// `chat_commands::{printable, paste_text}`, which *parse* the key instead of
// scanning bytes - that is why the picker search boxes are immune. The hub was the
// one field that re-derived text from raw bytes, so it is the one that broke.
//
// The rows below pin the shape, not the instance: an arrow key leaves the query
// alone, a **typed** `[` still enters (so the fix cannot be a blacklist of `[`),
// and a bracketed paste fills the query instead of typing its markers.

/// The escape sequences a terminal sends for the keys the issue names, exactly as
/// they arrive at this seam.
const NAVIGATION_SEQUENCES: &[(&str, &str)] = &[
    ("Left", "\x1b[D"),
    ("Right", "\x1b[C"),
    ("Up", "\x1b[A"),
    ("Down", "\x1b[B"),
    ("Home", "\x1b[H"),
    ("End", "\x1b[F"),
    ("PageUp", "\x1b[5~"),
    ("PageDown", "\x1b[6~"),
    ("Delete", "\x1b[3~"),
    ("Insert", "\x1b[2~"),
];

/// A hub with its catalog drained, so only the query is under test.
fn open_hub() -> crate::chat::Chat {
    let (mut chat, _confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    chat
}

fn query_of(chat: &crate::chat::Chat) -> String {
    chat.extension_hub
        .as_ref()
        .expect("hub stays open")
        .query
        .clone()
}

// Verifies: gh #329 - **a navigation key leaves the filter empty.** Six of the
// named keys, table-driven: each one used to append `[`, and a few presses made
// `[[[`, which matches nothing. Asserted on the exact query string, not on
// "contains no `[`" - a filter that had silently gained a space would pass that.
#[test]
fn a_navigation_key_does_not_type_into_the_filter() {
    let mut chat = open_hub();
    for (name, sequence) in NAVIGATION_SEQUENCES {
        chat.handle_extension_hub_key(
            sequence,
            lca_tui::engine::keys::parse_key(sequence).as_deref(),
        );
        assert_eq!(
            query_of(&chat),
            "",
            "{name} ({sequence:?}) must not touch the filter"
        );
    }
    // The issue's own symptom: several presses left `[[[`, matching nothing.
    assert!(
        chat.extension_hub.as_ref().expect("hub").matches.len() > 1,
        "and the catalog is not empty-filtered into nothing"
    );
}

// Verifies: gh #329 - **a typed `[` still enters the query.** This is the row that
// proves the fix is not a blacklist: a user filtering for `[draft]` types `[`, and
// a fix that special-cases the character would break exactly that user.
#[test]
fn a_typed_bracket_still_enters_the_filter() {
    let mut chat = open_hub();
    for c in ['[', 'd', 'r', 'a', 'f', 't', ']'] {
        chat.handle_extension_hub_key(&c.to_string(), None);
    }
    assert_eq!(
        query_of(&chat),
        "[draft]",
        "a genuinely typed bracket is text, not an escape fragment"
    );
}

// Verifies: gh #329 - **a whole bracketed paste fills the query with its payload.**
// `ESC[200~draft ESC[201~` arriving in one read used to type `[` (then digits from
// the `200~`), which is the case the issue calls out explicitly.
#[test]
fn a_whole_bracketed_paste_fills_the_filter() {
    let mut chat = open_hub();
    let pasted = "\x1b[200~draft\x1b[201~";
    chat.handle_extension_hub_key(pasted, None);
    assert_eq!(
        query_of(&chat),
        "draft",
        "the payload lands; the markers do not"
    );
}

// Verifies: gh #329 - **a fragmented paste never leaves a marker fragment behind.**
// The opener, the payload and the closer can arrive as three separate reads. Whatever
// the terminal does, the field must never contain `[`, `200~`, or a duplicated
// payload - the failure mode a byte-scanning arm produces.
#[test]
fn a_fragmented_bracketed_paste_leaves_no_marker_fragment() {
    let mut chat = open_hub();
    for fragment in ["\x1b[200~", "draft", "\x1b[201~"] {
        chat.handle_extension_hub_key(fragment, None);
    }
    let query = query_of(&chat);
    assert!(
        !query.contains('[') && !query.contains("200~"),
        "no marker fragment reaches the filter: {query:?}"
    );
    assert!(
        query == "draft" || query.is_empty() || query == "\x1b[200~draft\x1b[201~",
        "the payload lands once, or not at all - never doubled or partially parsed: {query:?}"
    );
}

// Verifies: gh #329 - **the arm beside the fallthrough still works.** Backspace
// deletes and Enter opens the selected row; a fix that swallowed every unparsed key
// must not cost the keys the hub actually owns.
#[test]
fn backspace_and_enter_still_work_after_the_fix() {
    let mut chat = open_hub();
    for c in "acme".chars() {
        chat.handle_extension_hub_key(&c.to_string(), None);
    }
    assert_eq!(query_of(&chat), "acme");
    chat.handle_extension_hub_key("\x7f", Some("backspace"));
    assert_eq!(query_of(&chat), "acm", "backspace deletes one character");

    // Enter still opens consent for the selected row.
    for c in "e".chars() {
        chat.handle_extension_hub_key(&c.to_string(), None);
    }
    chat.handle_extension_hub_key("", Some("enter"));
    assert!(
        chat.install_consent.is_some(),
        "Enter opens consent for the filtered row"
    );
}

// Verifies: gh #329's sweep - **the hub is the only field with this shape.** Every
// other inline text field in `lca-ui` routes through `printable`/`paste_text`, which
// parse the key rather than scanning bytes. Asserted on the source, because the
// property is "no other file re-derives text from raw bytes" and a behavioural row
// can only see the fields it was written for.
#[test]
fn no_other_filter_field_scans_raw_bytes_for_text() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    let Ok(entries) = std::fs::read_dir(ui.join("src")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Test modules are cut, and comments are stripped: this row would otherwise
        // match its own doc comment and its own matcher. What is asserted is real
        // code, which is the only thing that can type into a field.
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("data.chars()") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    index + 1,
                    code.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a text field must take already-parsed text (`printable`/`paste_text`), \
         not scan raw input bytes (gh #329):\n{}",
        offenders.join("\n")
    );
}
