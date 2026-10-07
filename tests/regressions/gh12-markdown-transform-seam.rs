//! GitHub issue #12 (open): "pi's markdown pipeline exposes a transform
//! hook (markdown-transform extension point). Deliberately divergent
//! today (no consumer). Add the hook when an extension needs it."
//!
//! The minimal honest seam (composer-polish brief, pi's
//! `registerMarkdownTransformer`): the markdown pipeline takes an
//! ordered list of pre-parse `fn(&str, &ctx) -> String` transforms,
//! applied in order before parsing; a transform that panics behaves as
//! identity (pi's try/catch). No WIT growth: the dispatch trait exposes
//! it as an optional Rust-side method (default: absent), and the WASM
//! export ships with the first third-party-shaped consumer.
//!
//! One wired consumer proves the shape end to end: a native extension's
//! transform is collected by the host into the transcript's pipeline.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::text::strip_terminal_sequences;
use lca_tui::widgets::markdown::{MarkdownTransformer, render_markdown};
use lca_ui::{Chat, UiOptions};

fn theme() -> lca_tui::widgets::markdown::MarkdownTheme {
    lca_tui::widgets::markdown::MarkdownTheme::default()
}

// A registered transform changes the parsed output: it runs pre-parse,
// so its output is what the parser sees.
#[test]
fn a_registered_transform_changes_the_parsed_output() {
    let upper: MarkdownTransformer = Arc::new(|text: &str, _ctx| text.to_uppercase());
    let options = lca_tui::widgets::markdown::MarkdownOptions {
        transformers: vec![upper],
        ..Default::default()
    };
    let lines = render_markdown("hello *world*", 40, &theme(), &options);
    let text: String = lines
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("HELLO"),
        "the transform ran before parsing:\n{text}"
    );
    assert!(!text.contains("hello"), "no untransformed text:\n{text}");
}

// Transforms chain in registration order.
#[test]
fn transforms_chain_in_order() {
    let append = |suffix: &'static str| -> MarkdownTransformer {
        Arc::new(move |text: &str, _ctx| format!("{text}{suffix}"))
    };
    let options = lca_tui::widgets::markdown::MarkdownOptions {
        transformers: vec![append("A"), append("B")],
        ..Default::default()
    };
    let lines = render_markdown("x", 40, &theme(), &options);
    let text: String = lines
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("xAB"),
        "registration order is application order:\n{text}"
    );
}

// A transform that panics behaves as identity: the render survives and
// matches the untransformed output.
#[test]
fn a_panicking_transform_behaves_as_identity() {
    let boom: MarkdownTransformer = Arc::new(|_text: &str, _ctx| panic!("a hostile transform"));
    let plain = render_markdown(
        "hello",
        40,
        &theme(),
        &lca_tui::widgets::markdown::MarkdownOptions::default(),
    );
    let guarded = render_markdown(
        "hello",
        40,
        &theme(),
        &lca_tui::widgets::markdown::MarkdownOptions {
            transformers: vec![boom],
            ..Default::default()
        },
    );
    assert_eq!(guarded, plain, "a throw keeps the current markdown");
}

// The transcript's pipeline carries a registered transform into the
// rendered answer.
#[test]
fn the_transcript_pipeline_carries_a_registered_transform() {
    let mut chat = chat_with_models();
    chat.transcript
        .register_markdown_transformer(Arc::new(|text: &str, _ctx| {
            text.replace(" classified", " CLASSIFIED")
        }));
    chat.transcript.push_user("q");
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta(
        "this is classified info".into(),
    ));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    let text: String = chat
        .render(80)
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("CLASSIFIED"),
        "the registered transform reached the transcript:\n{text}"
    );
}

fn chat_with_models() -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("openai-compatible/m".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: "auto".to_string(),
            theme_dir: std::path::PathBuf::new(),
            themes: lca_ui::theme::THEMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            initial_lines: Vec::new(),
            initial_records: Vec::new(),
            initial_tail_lines: Vec::new(),
            initial_messages: Vec::new(),
            yolo: false,
            thinking_visibility: Default::default(),
            codeblock_border: Default::default(),
            plain: true,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
            keybinding_overrides: Default::default(),
            keybinding_error: None,
            render_regions: None,
            ui_events: None,
            update_notice: None,
            login: None,
            complete_login: None,
            pick_login: None,
            confirm_login_grant: None,
            confirm_switch: None,
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

// The dispatch layer exposes the seam to native extensions: a native
// handle's transform is collected by the host, and an absent one (the
// default) contributes nothing.
#[test]
fn a_native_extension_transform_reaches_the_host_collection() {
    struct Demo;
    impl lca_ext_abi::ExtensionDispatch for Demo {
        fn name(&self) -> &str {
            "demo"
        }
        fn delivery(&self) -> lca_ext_abi::DeliveryMode {
            lca_ext_abi::DeliveryMode::Native
        }
        fn worlds(&self) -> Vec<lca_ext_abi::World> {
            Vec::new()
        }
        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }
        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Err(
                lca_protocol::DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "tool",
                },
            )))
        }
        fn command_specs(
            &self,
        ) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }
        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }
        fn on_pre_turn(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }
        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn markdown_transformer(&self) -> Option<lca_ext_abi::MarkdownTransformFn> {
            Some(std::sync::Arc::new(|text: &str, _context| {
                text.replace("classified", "CLASSIFIED")
            }))
        }
    }
    struct Silent;
    impl lca_ext_abi::ExtensionDispatch for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        fn delivery(&self) -> lca_ext_abi::DeliveryMode {
            lca_ext_abi::DeliveryMode::Native
        }
        fn worlds(&self) -> Vec<lca_ext_abi::World> {
            Vec::new()
        }
        fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }
        fn execute_tool<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Err(
                lca_protocol::DispatchError::MissingWorld {
                    extension: self.name().to_string(),
                    world: "tool",
                },
            )))
        }
        fn command_specs(
            &self,
        ) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
            Ok(Vec::new())
        }
        fn invoke_command(
            &self,
            _name: &str,
            _argument: &str,
        ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
            Ok(lca_protocol::CommandEffect::None)
        }
        fn on_pre_turn(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_pre_tool_use<'a>(
            &'a self,
            _call: &'a lca_protocol::ToolCall,
        ) -> lca_ext_abi::DispatchFuture<
            'a,
            Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
        > {
            Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
        }
        fn on_post_tool_use<'a>(
            &'a self,
            _observation: &'a lca_protocol::PostToolObservation,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_post_turn_end<'a>(
            &'a self,
            _status: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_attention_required<'a>(
            &'a self,
            _reason: &'a str,
        ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn on_session_close(
            &self,
        ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }
    let handles: Vec<lca_ext_native::NativeHandle> = vec![Arc::new(Demo), Arc::new(Silent)];
    let collected = lca_cli::tui::collect_markdown_transformers(&handles);
    assert_eq!(
        collected.len(),
        1,
        "the absent transform contributes nothing"
    );
    let out = render_markdown(
        "it is classified",
        40,
        &theme(),
        &lca_tui::widgets::markdown::MarkdownOptions {
            transformers: collected,
            ..Default::default()
        },
    );
    let text: String = out
        .iter()
        .map(|l| strip_terminal_sequences(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("CLASSIFIED"),
        "the native transform reached the pipeline:\n{text}"
    );
}
