//! The contract crate's public surface, pinned for third-party extension
//! authors: the version and package constants, the delivery labels the
//! extension list shows, and the `ExtensionDispatch` defaults every
//! optional world inherits. An implementor relies on those defaults, so
//! they are contract, not implementation detail.

use std::task::{Context, Poll};

use lca_ext_abi::{
    ABI_VERSION, DeliveryMode, DispatchFuture, ExtensionDispatch, PACKAGE, World, abi_in_window,
};
use lca_protocol::{
    ChatMessage, CommandEffect, CommandSpec, DispatchError, EventSink, HookAction,
    PostToolObservation, Record, StreamEvent, ToolCall, ToolResult, ToolSpec,
};

/// Drive the boxed defaults to completion. They are all
/// `std::future::ready`, so one poll always suffices; the loop is for
/// safety if a default ever awaits something.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

struct NullSink;

impl EventSink for NullSink {
    fn push(&self, _event: StreamEvent) -> bool {
        true
    }
}

/// The minimal implementor: only the required methods, so every assertion
/// below is about the *defaults*.
struct Minimal;

impl ExtensionDispatch for Minimal {
    fn name(&self) -> &str {
        "minimal"
    }

    fn delivery(&self) -> DeliveryMode {
        DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<World> {
        vec![World::Tool]
    }

    fn tool_specs(&self) -> Result<Vec<ToolSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<ToolResult, DispatchError>> {
        Box::pin(std::future::ready(Ok(ToolResult::ok(
            call.call_id.clone(),
            "ok",
        ))))
    }

    fn command_specs(&self) -> Result<Vec<CommandSpec>, DispatchError> {
        Ok(Vec::new())
    }

    fn invoke_command(&self, _name: &str, _argument: &str) -> Result<CommandEffect, DispatchError> {
        Ok(CommandEffect::None)
    }

    fn on_pre_turn(&self) -> DispatchFuture<'static, Result<(), DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> DispatchFuture<'a, Result<HookAction, DispatchError>> {
        Box::pin(std::future::ready(Ok(HookAction::Allow)))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a PostToolObservation,
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

// The constants cross every manifest and registry tag; a change here is a
// compatibility event, so they are pinned.
#[test]
fn the_package_name_and_abi_version_are_stable_shapes() {
    assert_eq!(PACKAGE, "lca:ext");
    let (major, minor) = ABI_VERSION
        .split_once('.')
        .unwrap_or_else(|| panic!("ABI_VERSION is `major.minor`, got {ABI_VERSION:?}"));
    assert!(major.parse::<u32>().is_ok(), "{ABI_VERSION}");
    assert!(
        minor.parse::<u32>().is_ok(),
        "minor is numeric: {ABI_VERSION}"
    );
}

// Verifies: NFR-19's two special cases, read against whatever the current line
// is. The window is relative (current minus one), so a line move needs no edit
// here - but the two *edges* are absolute facts about the 0.x development
// window and would not be visible in a purely relative reading, so they get
// their own row.
#[test]
fn the_window_edges_are_the_development_window_and_the_one_before_it() {
    // `1.0` is special: an extension installed against the 1.0-freeze host
    // declares `1.0`, and it keeps loading for the whole 0.x window (the same
    // amnesty the pre-freeze 0.1 line had on a 1.0 host). This is not "the line
    // after 1.x" - it is a freeze snapshot, which is why it loads *below* the
    // version it names.
    assert!(abi_in_window("1.0"), "the 1.0 freeze line is amnestied");

    // The previous line is accepted only while the current minor is above zero:
    // that is the development window, and it is what makes `current - 1`
    // meaningful at all.
    let (major, minor) = ABI_VERSION
        .split_once('.')
        .unwrap_or_else(|| panic!("ABI_VERSION is `major.minor`, got {ABI_VERSION:?}"));
    let major: u32 = major.parse().unwrap_or_else(|_| panic!("{ABI_VERSION}"));
    let minor: u32 = minor.parse().unwrap_or_else(|_| panic!("{ABI_VERSION}"));
    assert!(minor > 0, "0.x is the development window: {ABI_VERSION}");
    let previous = format!("{major}.{}", minor - 1);
    assert!(abi_in_window(&previous), "{previous} is the previous line");
    assert!(
        !abi_in_window(&format!("{major}.{}", minor + 1)),
        "a line from the future does not load"
    );

    // And the major rules: `1.0` is amnestied, `1.1` is not, and a component
    // declaring a major this line is not part of does not load. (`1.0` is
    // deliberately *not* derived as `major + 1`: for a 0.x host that is exactly
    // the amnestied snapshot, which is the point of the assertion above.)
    let _ = major;
    assert!(!abi_in_window("1.1"), "only the 1.0 snapshot is amnestied");
    assert!(
        !abi_in_window("2.0"),
        "a different major is a different line"
    );
    assert!(!abi_in_window("9.9"));
    assert!(!abi_in_window("garbage"), "garbage is not a line");
}

// Verifies: FR-EXT-7 (the list labels native and sandboxed deliveries).
#[test]
fn delivery_labels_match_the_extension_list() {
    assert_eq!(DeliveryMode::Native.label(), "in-process (unsandboxed)");
    assert_eq!(DeliveryMode::Wasm.label(), "sandboxed");
}

// Verifies: ADR-0012 (a provider without the optional exports is
// not-supported, not an error).
#[test]
fn the_identity_defaults_are_not_supported() {
    let handle = Minimal;
    assert!(matches!(
        block_on(handle.identity_login()).expect("host ok"),
        lca_protocol::IdentityOutcome::NotSupported
    ));
    assert!(matches!(
        block_on(handle.identity_logout()).expect("host ok"),
        lca_protocol::IdentityOutcome::NotSupported
    ));
    assert!(matches!(
        block_on(handle.identity_usage()).expect("host ok"),
        Err(lca_protocol::IdentityOutcome::NotSupported)
    ));
}

// A world an implementor did not declare answers `MissingWorld`, never a
// silent empty success (the SRDD's deny-by-default rule at the trait).
#[test]
fn undeclared_world_defaults_report_missing_world() {
    let handle = Minimal;
    let missing = |err: DispatchError, world: &str| matches!(err, DispatchError::MissingWorld { world: w, .. } if w == world);

    assert!(missing(
        handle.provider_models(&[]).unwrap_err(),
        "provider"
    ));

    let sink = NullSink;
    let request = lca_protocol::CompletionRequest {
        messages: Vec::new(),
        tools: Vec::new(),
        model: "m".into(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    assert!(missing(
        block_on(handle.stream_completion(request, &sink)).unwrap_err(),
        "provider"
    ));
    assert!(missing(
        block_on(handle.compact(&[])).unwrap_err(),
        "compaction"
    ));
}

// The observe-only and rendering defaults are the safe no-ops: no built-in
// slots, no regions, no drawing, and a transform that passes the list
// through unchanged.
#[test]
fn the_optional_seams_default_to_no_ops() {
    let handle = Minimal;
    assert!(handle.builtin_command_slots().is_empty());
    assert!(handle.ui_regions().is_empty());
    assert!(handle.render("status-line").expect("host ok").is_none());
    assert!(matches!(
        handle
            .on_ui_event("panel", &lca_protocol::UiInput::Cancel)
            .expect("host ok"),
        lca_protocol::UiEffect::None
    ));

    let messages = vec![ChatMessage::text(lca_protocol::MessageRole::User, "hello")];
    let transformed = block_on(handle.transform_messages(messages.clone()))
        .expect("host ok")
        .expect("no rejection");
    assert_eq!(transformed, messages);

    handle.interrupt(); // a no-op outside WASM
}

// The world list is the handle's own declaration; `World` is `Copy` so a
// registry can hold it, and the sixteen variants are distinct.
#[test]
fn every_world_is_distinct_and_copyable() {
    let worlds = [
        World::Tool,
        World::Command,
        World::Hooks,
        World::Provider,
        World::Compaction,
        World::ContextTransform,
        World::Ui,
        World::ToolCatalog,
        World::HooksMessage,
        World::HooksToolCall,
        World::HooksToolResult,
        World::HooksStream,
        World::HooksSettle,
        World::HooksCompaction,
        World::HooksCache,
        World::HooksTrust,
    ];
    for (index, left) in worlds.iter().enumerate() {
        for right in &worlds[index + 1..] {
            assert_ne!(left, right, "world variants are distinct");
        }
    }
    let copied = worlds[0]; // Copy
    assert_eq!(copied, World::Tool);
    let _: Vec<Record> = Vec::new();
}
