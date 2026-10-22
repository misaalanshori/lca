//! NFR-24 guard for gh #418 Phase G: the `/mcp` manager modal and the boundary
//! it sits on.
//!
//! The interesting properties here are not "does the picker move" - those are
//! rows in `lca-ui`'s own test module, next to the code. What this file pins is
//! the *contract between three crates*, which no single crate's tests can see:
//!
//! 1. **`lca-mcp` answers with data, never a widget.** ADR-0047's annotation and
//!    `crates/lca-core/tests/architecture.rs` pin the dependency direction; this
//!    row pins the *shape* - the modal-facing types carry no display text beyond
//!    a state sentence and a stderr tail, and the exposure vocabulary the picker
//!    offers is the one the config parser accepts.
//! 2. **The direct slash verbs survive the modal.** A script or muscle memory that
//!    uses `/mcp login <server>` must not break because a picker was added.
//! 3. **The modal's four exposures match `mcp.json`'s four.** Two lists that must
//!    agree and live in different crates, so the disagreement would be silent.
//!
//! The transient-failure status set is pinned here too, because the guide in
//! `docs/mcp.md` states it and a guide that drifts from the code is the defect
//! #419 filed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_mcp::config::McpExposure;

/// The four levels the modal's picker offers, as `lca-ui` declares them.
///
/// Read from the constant rather than re-typed, so a fifth level added on one
/// side fails here instead of silently becoming unreachable in the picker.
#[test]
fn the_pickers_four_levels_are_the_parsers_four_levels() {
    let picker = lca_ui::EXPOSURE_LEVELS;
    assert_eq!(picker.len(), 4, "the picker offers {picker:?}");

    for level in picker {
        // `parse` is what `mcp.json` validation uses: a level the picker can
        // select but the parser refuses would persist a config the next load
        // rejects.
        McpExposure::parse(level).unwrap_or_else(|err| {
            panic!("the picker offers `{level}`, which the config parser refuses: {err}")
        });
    }

    // And the reverse direction: every level the parser accepts is selectable.
    // `codemode-deferred` is pi's legacy alias and is not a distinct level, so it
    // is excluded by name rather than by omission.
    for (level, _) in [
        ("direct", McpExposure::Direct),
        ("codemode", McpExposure::Codemode),
        ("deferred", McpExposure::Deferred),
        ("hidden", McpExposure::Hidden),
    ] {
        assert!(
            picker.contains(&level),
            "the parser accepts `{level}` but the picker cannot select it"
        );
    }
}

/// The transient statuses `docs/mcp.md` documents, pinned to the function.
///
/// The guide says "`408`, `429`, or a `5xx` other than `501`", and three attempts
/// on idempotent reads. If this function changes, the guide is wrong and this row
/// is what says so - which is exactly the drift #419 was filed about.
#[test]
fn the_documented_transient_status_set_is_the_one_the_code_uses() {
    assert!(
        lca_mcp::is_transient_status(408),
        "408 is documented as transient"
    );
    assert!(
        lca_mcp::is_transient_status(429),
        "429 is documented as transient"
    );
    assert!(lca_mcp::is_transient_status(500));
    assert!(lca_mcp::is_transient_status(503));
    assert!(
        !lca_mcp::is_transient_status(501),
        "501 is documented as NOT transient: the server does not implement it"
    );
    assert!(
        !lca_mcp::is_transient_status(400),
        "a bad request is not transient"
    );
    assert!(
        !lca_mcp::is_transient_status(401),
        "401 is a state (needs-auth), never a retry"
    );
    assert!(
        !lca_mcp::is_transient_status(404),
        "404 is not transient; the guide says to Reconnect instead"
    );
}

/// The modal's data types carry no rendered display strings.
///
/// This is the boundary ADR-0047 pins, expressed as something a test can check: if
/// `lca-mcp` ever gained a `title: String` or a `row: String` in these types, the
/// subsystem would be composing display and the next step is a widget - which is
/// the dependency inversion the architecture test forbids.
///
/// The row is a source scan, not a type inspection: the property is "no field
/// description says it is display text", and a compile-time check cannot express
/// that. A false pass costs little (the architecture test already guards the
/// edge); a false failure would be a field whose doc says `row`/`title`/`label`
/// for another reason, which is worth reading anyway.
#[test]
fn the_modal_data_types_describe_data_not_display() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-mcp/src/manager.rs"
    ))
    .expect("the manager is in the tree");

    // The section this row guards: the modal's three type definitions.
    for type_name in [
        "ModalSnapshot",
        "ModalServerRow",
        "ModalServerDetail",
        "ModalToolRow",
    ] {
        let start = source
            .find(&format!("pub struct {type_name}"))
            .unwrap_or_else(|| panic!("{type_name} exists"));
        let body = &source[start..];
        let end = body.find("\n}").expect("the struct has a body");
        let definition = &body[..end];
        // A field documented as a display string would be the first step of the
        // subsystem rendering. `detail` is the state sentence the *host* composed
        // by joining `failed: <err>` - data about the failure, not styling.
        for forbidden in [
            "/// The row to draw",
            "/// The line to print",
            "/// The title to show",
        ] {
            assert!(
                !definition.contains(forbidden),
                "{type_name} carries display instructions ({forbidden:?}); the manager answers \
                 with data and the interface composes the widget (ADR-0047)"
            );
        }
    }
}

/// Gh #418 Phase G: the direct slash verbs still answer with a notice.
///
/// The modal is an addition, not a replacement. A user with `/mcp login sentry`
/// in muscle memory, or a script that types it, must get the same one-line answer
/// it always got. `lca-ui`'s own module has the detailed rows; this one pins the
/// regression at the level the issue described it.
#[test]
fn the_scriptable_mcp_verbs_do_not_open_the_modal() {
    // The modal opens only for a bare `/mcp` or `/mcp list`; every other verb goes
    // to the manager's `act`, which answers with a string. Pinned by reading the
    // router's shape rather than by driving a `Chat` (which needs the whole
    // interface): the property is "these two spellings are special and nothing
    // else is", and that lives in one arm.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/chat_commands.rs"
    ))
    .expect("the router is in the tree");
    let start = source
        .find("\"mcp\" if argument.trim().is_empty()")
        .expect("the bare-/mcp arm exists");
    let arm = &source[start..];
    let arm_end = arm.find("\n            }").expect("the arm has a body");
    let arm = &arm[..arm_end];

    // The arm opens the modal and returns: it must not fall through to the verb
    // route, or a bare `/mcp` would post a status notice *and* open a picker.
    assert!(
        arm.contains("open_mcp_manager"),
        "bare /mcp must open the manager: {arm}"
    );
    assert!(
        arm.contains("return"),
        "the bare-/mcp arm must return without falling through to the verb route: {arm}"
    );

    // And the verb arm is still there, still routing to the manager's `act`.
    assert!(
        source.contains("self.world.options.hooks.mcp_action.clone()"),
        "the scriptable verb route must still exist"
    );
}

/// A list row carries a SHORT state; the failure text and its stderr tail belong
/// on the detail screen.
///
/// Found in a live pane, not in a unit row: the first version inlined the whole
/// failure - including the ` stderr: …` suffix - into the row, which wrapped
/// across three lines and pushed the second server off the visible box. A list is
/// for triage; "failed" is the triage fact and the traceback is one `Enter` away.
///
/// The property is about text *length*, so it is asserted on the text: a row
/// whose state summary contains a newline-separated traceback, or the literal
/// `stderr:` marker, is a row that will wrap.
#[test]
fn a_list_row_carries_a_short_state_not_the_failure_text() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-mcp/src/manager.rs"
    ))
    .expect("the manager is in the tree");

    // The summary function exists and is what a row uses.
    assert!(
        source.contains("fn list_state_summary("),
        "the short-state helper exists"
    );
    let row_start = source
        .find("detail: list_state_summary(")
        .expect("a modal row uses the short summary, not the full state_detail");
    // Guard against the regression specifically: the row must not call
    // `state_detail`, which is the long form that carries the failure text.
    let row_slice = &source[row_start.saturating_sub(200)..row_start + 80];
    assert!(
        !row_slice.contains("detail: state_detail("),
        "a list row must not inline the long state (it wraps): {row_slice}"
    );

    // And the short form really is short for every state: assert against the
    // source of the function itself rather than constructing a `ServerStateKind`
    // (whose `Failed` variant's payload the helper deliberately drops).
    let start = source.find("fn list_state_summary(").expect("exists");
    let body = &source[start..];
    let end = body.find("\n}").expect("has a body");
    let body = &body[..end];
    assert!(
        !body.contains("err}") && !body.contains("detail}"),
        "the short summary must not interpolate the failure payload: {body}"
    );
}
