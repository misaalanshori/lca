//! The scripted provider/login/ui answers both delivery modes share
//! (the Phase 3 half of NFR-25). Split from `lib.rs` for the workspace
//! file ceiling (gate 11). Behaviour unchanged.
//!
//! Everything here is plain host-agnostic script: the WASM guest and the
//! native twin call the same functions through `crate::`, so their
//! results agree by construction.

use super::{AUTHORIZE_URL, CALLBACK_PATH, FIXTURE_CODE, IdentityCap};
use lca_protocol::CapabilityError;
// ---------------------------------------------------------------------------
// Provider world: one script, both modes (the Phase 3 half of NFR-25)
// ---------------------------------------------------------------------------

/// The model list both modes return (FR-PROV-2).
/// The models the probe reports. A `models` setting (what `login-submit`
/// hands the host to persist, ADR-0033) is the discovered list and wins -
/// the same rule in both delivery modes (ADR-0035).
pub fn provider_models(settings: &[(String, String)]) -> Vec<lca_protocol::ModelInfo> {
    if let Some((_, list)) = settings.iter().find(|(key, _)| key == "models") {
        return list
            .split(',')
            .filter(|id| !id.is_empty())
            .map(|id| lca_protocol::ModelInfo {
                id: id.to_string(),
                name: id.to_string(),
                context_window: 4096,
                max_tokens: 0,
                extras: Default::default(),
            })
            .collect();
    }
    provider_models_default()
}

/// The probe's fixed list, when no `models` setting was passed.
pub fn provider_models_default() -> Vec<lca_protocol::ModelInfo> {
    vec![
        lca_protocol::ModelInfo {
            id: "conformance-a".to_string(),
            name: "Conformance A".to_string(),
            context_window: 4096,
            max_tokens: 512,
            extras: Default::default(),
        },
        lca_protocol::ModelInfo {
            id: "conformance-b".to_string(),
            name: "Conformance B".to_string(),
            context_window: 8192,
            max_tokens: 1024,
            extras: Default::default(),
        },
    ]
}

/// The per-completion usage record every successful script ends with
/// (testing plan: usage is mandatory on every turn).
pub fn scripted_usage() -> lca_protocol::Usage {
    lca_protocol::Usage {
        input: 100,
        output: 50,
        cache_read: 1000,
        cache_write: 20,
        cache_write_1h: 0,
        cost: 0.002,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    }
}

/// The event script, chosen by the request's model string. Both modes
/// walk this same list, so their event streams are identical by
/// construction (NFR-25).
pub fn scripted_events(model: &str) -> Vec<lca_protocol::StreamEvent> {
    use lca_protocol::StreamEvent as E;
    let usage = || E::Usage {
        usage: scripted_usage(),
    };
    match model {
        // FR-PROV-7's shape: start strictly before every delta.
        "conformance-tool" => vec![
            E::ToolCallStart {
                call_id: "c1".to_string(),
                name: "read".to_string(),
            },
            E::ToolCallArgDelta {
                call_id: "c1".to_string(),
                delta: "{\"path\":\"".to_string(),
            },
            E::ToolCallArgDelta {
                call_id: "c1".to_string(),
                delta: "notes.txt\"}".to_string(),
            },
            E::ToolCallEnd {
                call_id: "c1".to_string(),
            },
            usage(),
        ],
        // FR-PROV-8's shape: a delta with no open start, which the host's
        // accumulator must discard and record as a protocol error.
        "conformance-orphan-delta" => vec![
            E::ToolCallArgDelta {
                call_id: "ghost".to_string(),
                delta: "{}".to_string(),
            },
            usage(),
        ],
        // The reserved escape hatch (ABI `vendor-event`).
        "conformance-vendor" => vec![
            E::VendorEvent {
                kind: "image.generate".to_string(),
                payload: serde_json::json!({ "size": "1024x1024" }),
            },
            usage(),
        ],
        // Ends with a typed error, so no usage follows.
        "conformance-error" => vec![E::Error {
            message: "scripted failure".to_string(),
            retryable: false,
        }],
        // The default text/reasoning script.
        _ => vec![
            E::TextDelta {
                delta: "Hel".to_string(),
            },
            E::TextDelta {
                delta: "lo".to_string(),
            },
            E::ReasoningDelta {
                delta: "reason".to_string(),
            },
            usage(),
        ],
    }
}

/// The compaction script: a body carrying `call-completion` makes the
/// strategy ask the host through the `completion` capability (whose
/// denial must surface as the refusal - FR-PERM-3's completion case);
/// anything else gets the mechanical summary both modes must agree on.
pub fn compact_script(
    excerpts: &[(String, String)],
    ask: Option<&dyn Fn() -> Result<String, String>>,
) -> Result<String, String> {
    let wants_model = excerpts
        .iter()
        .any(|(_kind, body)| body.contains("call-completion"));
    if wants_model {
        match ask {
            Some(ask) => ask(),
            None => Err("completion is not available".to_string()),
        }
    } else {
        Ok(format!("conformance compacted {} records", excerpts.len()))
    }
}

/// The transform script: a rejection marker rejects, an injection
/// marker appends one message, everything else passes through - both
/// modes run this exact decision (NFR-25).
pub fn transform_script(
    mut messages: Vec<lca_protocol::ChatMessage>,
) -> Result<Vec<lca_protocol::ChatMessage>, String> {
    let text: String = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.contains("conformance-reject") {
        return Err("conformance transform rejection".to_string());
    }
    if text.contains("conformance-inject") {
        messages.push(lca_protocol::ChatMessage::text(
            lca_protocol::MessageRole::System,
            "[conformance injected] instructions",
        ));
    }
    Ok(messages)
}

/// Render the candidate records for the compaction script (the same
/// `(kind, body)` pair in both modes).
pub fn compact_excerpts(records: &[lca_protocol::Record]) -> Vec<(String, String)> {
    records
        .iter()
        .map(|record| {
            (
                record.type_tag().to_string(),
                serde_json::to_string(record).unwrap_or_default(),
            )
        })
        .collect()
}

/// The scripted tree for each region - identical in both modes
/// (NFR-25). The footer carries an escape-sequence span on purpose:
/// it is the hostile-extension fixture, and the host must render those
/// bytes literally (FR-UI-2, ADR-0003).
pub fn ui_script(region: &str) -> Option<Vec<lca_protocol::Widget>> {
    use lca_protocol::Widget;
    let text = |content: &str, role: &str| Widget::Text {
        content: content.to_string(),
        role: role.to_string(),
    };
    Some(match region {
        "status-line" => vec![text("conformance", "accent")],
        // The footer is the vocabulary page: every widget kind the ABI
        // carries, arena-style from one root, with the hostile bytes as
        // a real escape character - the freeze gate says conformance
        // covers every surface, so the widget variant's cases all cross
        // here in both modes.
        "footer" => vec![
            Widget::Column(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]),
            text("hostile: \u{1b}[31mNOT A PROMPT\u{1b}[0m", "warning"),
            // The 0.6 vocabulary page (gh #172): dual-channel hex
            // plus a hostile-role twin the host must degrade, not
            // paint.
            Widget::StyledText {
                content: "styled \u{1b}[32mhex".to_string(),
                style: lca_protocol::TextStyle {
                    fg: Some("#50fa7b".to_string()),
                    bg: Some("#282a36".to_string()),
                    bold: true,
                    dim: false,
                    italic: false,
                    underline: true,
                },
            },
            Widget::Row(vec![4, 5]),
            text("row-left", "muted"),
            text("row-right", "muted"),
            Widget::Spinner {
                frames: "\u{280b}\u{2819}".to_string(),
            },
            Widget::Progress {
                label: "conformance".to_string(),
                fill: 0.5,
            },
            Widget::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3, 4],
            },
            Widget::Vendor("conformance.demo".to_string()),
            Widget::Markdown {
                source: "# conformance\n\n- one\n- two".to_string(),
            },
            Widget::Button {
                id: "ok".to_string(),
                label: "OK".to_string(),
            },
            Widget::Table {
                headers: vec!["name".to_string(), "value".to_string()],
                rows: vec![vec!["a".to_string(), "1".to_string()]],
            },
            Widget::ScrollContainer {
                max_height: 2,
                children: vec![11, 13],
            },
        ],
        "panel" => vec![
            Widget::Column(vec![1, 2]),
            Widget::KeyValue(vec![
                ("mode".to_string(), "stateless".to_string()),
                ("arena".to_string(), "node0 is the root".to_string()),
            ]),
            // Gh #172: the clickable panel button the mouse receipt drives.
            Widget::Button {
                id: "ok".to_string(),
                label: "OK".to_string(),
            },
        ],
        // The modal pairs the two remaining cases: a boxed child and a
        // column beneath it.
        "modal" => vec![
            Widget::Boxed {
                title: Some("conformance modal".to_string()),
                border: Some("accent".to_string()),
                background: None,
                child: 1,
            },
            Widget::Column(vec![2, 3]),
            text("modal body", "default"),
            Widget::KeyValue(vec![("dismiss".to_string(), "esc".to_string())]),
        ],
        _ => return None,
    })
}

/// The scripted response to one interaction, both modes.
pub fn ui_event_script(region: &str, input: &lca_protocol::UiInput) -> lca_protocol::UiEffect {
    use lca_protocol::{UiEffect, UiInput};
    if region == "modal" {
        return match input {
            UiInput::Key { key } if key == "q" => UiEffect::CloseModal,
            UiInput::Key { key } if key == "m" => UiEffect::OpenModal,
            UiInput::Submit { text } => UiEffect::ShowNotice(format!("heard: {text}")),
            UiInput::Cancel => UiEffect::CloseModal,
            UiInput::ClickWidget { id } => UiEffect::ShowNotice(format!("clicked: {id}")),
            UiInput::Click { .. } | UiInput::Scroll { .. } | UiInput::Key { .. } => UiEffect::None,
        };
    }
    // Gh #172: the panel button answers like the modal one.
    if region == "panel"
        && let UiInput::ClickWidget { id } = input
    {
        return UiEffect::ShowNotice(format!("clicked: {id}"));
    }
    UiEffect::None
}

/// `login`: exercise the credentials round trip and the full oauth
/// begin/await/end flow through whichever mode's [`IdentityCap`] is supplied,
/// then report a deterministic outcome (NFR-25: both modes produce the same
/// `IdentityOutcome` for the same injected callback).
pub fn scripted_login(cap: &dyn IdentityCap) -> lca_protocol::IdentityOutcome {
    use lca_protocol::IdentityOutcome;
    let credentials = (|| -> Result<(), CapabilityError> {
        cap.credentials_set("probe", "1")?;
        match cap.credentials_get("probe")? {
            Some(value) if value == "1" => {}
            other => {
                return Err(CapabilityError::Io(format!(
                    "credential read back {other:?}, expected \"1\""
                )));
            }
        }
        cap.credentials_delete("probe")?;
        if cap.credentials_get("probe")?.is_some() {
            return Err(CapabilityError::Io(
                "credential survived delete".to_string(),
            ));
        }
        Ok(())
    })();
    if let Err(err) = credentials {
        return IdentityOutcome::Failed(err.to_string());
    }
    let (url, handle) = match cap.oauth_begin(CALLBACK_PATH) {
        Ok(pair) => pair,
        Err(err) => return IdentityOutcome::Failed(err.to_string()),
    };
    if let Err(err) = cap.oauth_open(AUTHORIZE_URL) {
        let _ = cap.oauth_end(handle);
        return IdentityOutcome::Failed(err.to_string());
    }
    let params = match cap.oauth_await(handle) {
        Ok(params) => params,
        Err(err) => {
            let _ = cap.oauth_end(handle);
            return IdentityOutcome::Failed(err.to_string());
        }
    };
    if let Err(err) = cap.oauth_end(handle) {
        return IdentityOutcome::Failed(err.to_string());
    }
    let code = params
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.as_str());
    if url.starts_with("http://127.0.0.1:") && code == Some(FIXTURE_CODE) {
        IdentityOutcome::Ok
    } else {
        IdentityOutcome::Failed(format!("unexpected oauth callback at {url}: {params:?}"))
    }
}

/// `logout` is this provider's not-supported case.
pub fn scripted_logout() -> lca_protocol::IdentityOutcome {
    lca_protocol::IdentityOutcome::NotSupported
}

/// The standard usage shape the generic `/usage` prints.
pub fn scripted_usage_report() -> lca_protocol::Usage {
    lca_protocol::Usage {
        input: 700,
        output: 70,
        cache_read: 7000,
        cache_write: 0,
        cache_write_1h: 0,
        cost: 0.007,
        cost_input: 0.0,
        cost_cache_read: 0.0,
        cost_cache_write: 0.0,
        extras: Default::default(),
    }
}

/// The login options the conformance probe reports (ADR-0033): a fixed
/// pair so both delivery modes are compared on identical data.
pub fn scripted_login_options() -> Vec<lca_protocol::LoginOption> {
    vec![
        lca_protocol::LoginOption {
            id: "conformance".to_string(),
            name: "Conformance".to_string(),
            kind: "api-key".to_string(),
            host: "conformance.example.com".to_string(),
            fields: vec!["api-key".to_string()],
            extras: Default::default(),
        },
        lca_protocol::LoginOption {
            id: "local".to_string(),
            name: "Local".to_string(),
            kind: "api-key".to_string(),
            host: "localhost".to_string(),
            fields: vec!["api-key".to_string()],
            extras: Default::default(),
        },
    ]
}

/// Consume one answer the way a real provider would: return the opaque
/// settings the host persists (ADR-0033).
pub fn scripted_login_submit(
    answer: &lca_protocol::LoginAnswer,
) -> Result<Vec<(String, String)>, String> {
    if answer.choice.is_empty() {
        return Err("no choice given".to_string());
    }
    let key = answer.value("api-key").unwrap_or_default();
    Ok(vec![
        (
            "base_url".to_string(),
            format!("https://{}/v1", answer.choice),
        ),
        ("key_len".to_string(), key.len().to_string()),
    ])
}
