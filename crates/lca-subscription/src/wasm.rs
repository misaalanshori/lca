//! The shared WASM dispatch for subscription-gateway providers (gh
//! #63 review): the whole component guest — capability view, pull
//! stream, identity, login, models — parameterized by the spec table,
//! so every future dispatch fix lands once. Each extension's
//! `wasm_mode.rs` is a thin constructor invoking the
//! `subscription_wasm_dispatcher` macro.
//!
//! The macro expands at the call site, where the crate's own
//! `wit_bindgen` output (`exports::...`, `lca::...`) and its
//! `stored`/`endpoint` helpers live. It needs those plus
//! `lca-protocol`, `lca-subscription`, and `serde_json` in scope as
//! crates (all three are already direct dependencies of every
//! subscription extension).

/// Expand the subscription provider's WASM guest.
///
/// Parameters (all paths resolve in the invoking crate):
/// - `struct $wasm`: the guest struct to declare (e.g. `CodexWasm`).
/// - `spec: $spec`: the [`crate::OAuthSpec`] table (e.g. `crate::SPEC`).
/// - `responses_path: $path`: the inference path (e.g. `crate::RESPONSES_PATH`).
/// - `headers: $headers`: `fn(&str, &str, &str) -> Vec<(String, String)>`
///   building the request headers from `(account_id, model, token)`.
/// - `stored: $stored`, `endpoint: $endpoint`: the credential-namespace
///   read and the endpoint-override helpers.
/// - `login:`, `logout:`, `usage:`, `list_models:`: the extension's
///   identity and catalog functions.
/// - `account_error: $message`: shown when no account is stored.
#[macro_export]
macro_rules! subscription_wasm_dispatcher {
    (
        struct $wasm:ident,
        spec: $spec:expr,
        responses_path: $responses_path:expr,
        headers: $headers:expr,
        stored: $stored:path,
        endpoint: $endpoint:path,
        login: $login:path,
        logout: $logout:path,
        usage: $usage:path,
        list_models: $list_models:path,
        account_error: $account_error:expr
    ) => {
        wit_bindgen::generate!({
            path: "../../wit",
            world: "provider",
            export_macro_name: "export_provider",
            with: {
                "lca:host/log@0.6.0": generate,
                "lca:host/net@0.6.0": generate,
                "lca:host/oauth@0.6.0": generate,
                "lca:host/credentials@0.6.0": generate,
                "lca:host/resources@0.6.0": generate,
                "lca:host/state@0.6.0": generate,
            },
        });

        use core::cell::RefCell;

        use exports::lca::ext::provider_completion::{
            CompletionStream, Guest as CompletionGuest, GuestCompletionStream,
            StreamEvent as WasmEvent,
        };
        use exports::lca::ext::provider_identity::{
            Guest as IdentityGuest, IdentityOutcome as WasmOutcome, TokenUsage,
        };
        use exports::lca::ext::provider_login::{
            Guest as LoginGuest, LoginAnswer as WasmLoginAnswer,
            LoginOption as WasmLoginOption, LoginResult as WasmLoginResult,
        };
        use exports::lca::ext::provider_models::{Guest as ModelsGuest, ModelInfo as WasmModel};
        use lca::ext::types::{ExtraPair, Usage as WasmUsage};
        use lca::host::{credentials, net, oauth, resources};
        use lca_protocol::{OauthCap, ProviderCap};

        fn map_net(err: net::Error) -> lca_protocol::CapabilityError {
            use lca_protocol::CapabilityError as E;
            match err {
                net::Error::Permission(d) => E::Permission(d),
                net::Error::NotGranted(d) => E::NotGranted(d),
                net::Error::Dns(d) => E::Io(d),
                net::Error::Tls(d) => E::Io(d),
                net::Error::Io(d) => E::Io(d),
                net::Error::Invalid(d) => E::Invalid(d),
            }
        }

        /// The resource import's error, mapped the same way its
        /// siblings map theirs (gh #314): `permission` and `not-found`
        /// keep their meaning because callers act differently on them -
        /// a refused path is a bug, a missing one may be an optional
        /// resource.
        fn map_resources(err: resources::Error) -> lca_protocol::CapabilityError {
            use lca_protocol::CapabilityError as E;
            match err {
                resources::Error::Permission(d) => E::Permission(d),
                resources::Error::NotFound(d) => E::NotFound(d),
                resources::Error::Invalid(d) => E::Invalid(d),
            }
        }

        fn map_oauth(err: oauth::Error) -> lca_protocol::CapabilityError {
            use lca_protocol::CapabilityError as E;
            match err {
                oauth::Error::Permission(d) => E::Permission(d),
                oauth::Error::NotGranted(d) => E::NotGranted(d),
                oauth::Error::Timeout(d) => E::Timeout(d),
                oauth::Error::Io(d) => E::Io(d),
                oauth::Error::Invalid(d) => E::Invalid(d),
            }
        }

        /// The guest's capability view: host imports only.
        struct GuestCap;

        /// A `'static` view so the streaming driver can borrow the
        /// capability view for as long as its resource lives (a unit
        /// struct: free).
        static GUEST_CAP: GuestCap = GuestCap;

        impl ProviderCap for GuestCap {
            fn net_request(
                &self,
                method: &str,
                url: &str,
                headers: &[(&str, &str)],
                body: Option<&[u8]>,
            ) -> Result<u32, lca_protocol::CapabilityError> {
                net::request(
                    method,
                    url,
                    &headers
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect::<Vec<_>>(),
                    body,
                )
                .map_err(map_net)
            }

            fn net_response_status(
                &self,
                handle: u32,
            ) -> Result<u16, lca_protocol::CapabilityError> {
                net::response_status(handle).map_err(map_net)
            }

            fn net_read_body(
                &self,
                handle: u32,
                max: usize,
            ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
                net::read_body(handle, max as u64).map_err(map_net)
            }

            // Gh #314: the trait's default returns an empty list, which
            // compiled fine and silently cost the WASM path every header
            // the native path sees - `retry-after` above all, so a rate
            // limit was invisible and the kit could not back off on the
            // server's own clock the way native does.
            fn net_response_headers(
                &self,
                handle: u32,
            ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
                net::response_headers(handle).map_err(map_net)
            }

            fn net_close_response(
                &self,
                handle: u32,
            ) -> Result<(), lca_protocol::CapabilityError> {
                net::close_response(handle).map_err(map_net)
            }

            fn credentials_get(&self, key: &str) -> Option<String> {
                credentials::get(key)
            }

            fn credentials_set(
                &self,
                key: &str,
                value: &str,
            ) -> Result<(), lca_protocol::CapabilityError> {
                credentials::set(key, value).map_err(|err| match err {
                    credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                    credentials::Error::NotGranted(d) => {
                        lca_protocol::CapabilityError::NotGranted(d)
                    }
                    credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                    credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
                })
            }

            fn credentials_delete(
                &self,
                key: &str,
            ) -> Result<(), lca_protocol::CapabilityError> {
                credentials::delete(key).map_err(|err| match err {
                    credentials::Error::Permission(d) => lca_protocol::CapabilityError::Permission(d),
                    credentials::Error::NotGranted(d) => {
                        lca_protocol::CapabilityError::NotGranted(d)
                    }
                    credentials::Error::Io(d) => lca_protocol::CapabilityError::Io(d),
                    credentials::Error::Invalid(d) => lca_protocol::CapabilityError::Invalid(d),
                })
            }

            // Gh #314: the trait's default reports `NotFound` for every
            // path, so a WASM provider could not read its own `resources/`
            // bag - presets, curated model lists, any data-only package
            // content. The import was already bound in this world; only
            // the delegation was missing.
            fn resource_read(
                &self,
                path: &str,
            ) -> Result<Vec<u8>, lca_protocol::CapabilityError> {
                resources::read(path).map_err(map_resources)
            }
        }

        impl OauthCap for GuestCap {
            fn oauth_begin(
                &self,
                redirect_path: &str,
            ) -> Result<(String, u32), lca_protocol::CapabilityError> {
                oauth::begin(redirect_path).map_err(map_oauth)
            }

            fn oauth_open(&self, url: &str) -> Result<(), lca_protocol::CapabilityError> {
                oauth::open(url).map_err(map_oauth)
            }

            fn oauth_await(
                &self,
                handle: u32,
            ) -> Result<Vec<(String, String)>, lca_protocol::CapabilityError> {
                oauth::await_callback(handle).map_err(map_oauth)
            }

            fn oauth_end(&self, handle: u32) -> Result<(), lca_protocol::CapabilityError> {
                oauth::end_flow(handle).map_err(map_oauth)
            }
        }

        fn to_wit_usage(usage: &lca_protocol::Usage) -> WasmUsage {
            WasmUsage {
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                cache_write_hour: usage.cache_write_1h,
                cost: usage.cost,
                extras: usage
                    .extras
                    .iter()
                    .map(|(key, value)| ExtraPair {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            }
        }

        fn to_wit_event(event: lca_protocol::StreamEvent) -> WasmEvent {
            use lca_protocol::StreamEvent as P;
            match event {
                P::TextDelta { delta } => WasmEvent::TextDelta(delta),
                P::ReasoningDelta { delta } => WasmEvent::ReasoningDelta(delta),
                P::ToolCallStart { call_id, name } => WasmEvent::ToolCallStart((call_id, name)),
                P::ToolCallArgDelta { call_id, delta } => {
                    WasmEvent::ToolCallArgDelta((call_id, delta))
                }
                P::ToolCallEnd { call_id } => WasmEvent::ToolCallEnd(call_id),
                P::Usage { usage } => WasmEvent::Usage(to_wit_usage(&usage)),
                P::Error { message, retryable } => WasmEvent::Error((message, retryable)),
                // Gh #390 gap 1: the shared subscription guest is the one path most
                // providers take, so the truncation must survive it. The WIT stream
                // has no case for it, so it rides the ABI's reserved escape hatch
                // and `host_imports/wit.rs` maps it back on the other side.
                P::Truncated => WasmEvent::VendorEvent((
                    lca_protocol::TRUNCATED_KIND.to_string(),
                    String::new(),
                )),
                P::VendorEvent { kind, payload } => {
                    WasmEvent::VendorEvent((kind, payload.to_string()))
                }
            }
        }

        fn to_wit_outcome(outcome: lca_protocol::IdentityOutcome) -> WasmOutcome {
            match outcome {
                lca_protocol::IdentityOutcome::Ok => WasmOutcome::Ok,
                lca_protocol::IdentityOutcome::NotSupported => WasmOutcome::NotSupported,
                lca_protocol::IdentityOutcome::Failed(reason) => WasmOutcome::Failed(reason),
            }
        }

        pub struct $wasm;

        /// The `completion-stream` resource: a pull stream over the kit
        /// driver, so events leave as the host yields body chunks.
        pub struct QueuedStream {
            driver: RefCell<Option<::lca_wire_openai::ResponseStreamDriver<'static>>>,
            /// The failure an open returned, delivered as the stream's first
            /// (and only) event (gh #566). `retryable` survives this way; the
            /// class does not - see `open_stream`'s note.
            seed: RefCell<Option<(String, bool)>>,
        }

        impl GuestCompletionStream for QueuedStream {
            fn next(&self) -> Option<WasmEvent> {
                // Gh #566: a failed open is delivered as the stream's own typed
                // error, not as the `result<_, string>` the WIT signature
                // allows - so `retryable` is the wire kit's exact answer and the
                // turn loop's retry rule acts on it, instead of the host
                // re-guessing from the message text.
                if let Some((message, retryable)) = self.seed.borrow_mut().take() {
                    return Some(WasmEvent::Error((message, retryable)));
                }
                let mut slot = self.driver.borrow_mut();
                let driver = slot.as_mut()?;
                match driver.next_event() {
                    None => {
                        *slot = None;
                        None
                    }
                    Some(Ok(event)) => Some(to_wit_event(event)),
                    Some(Err(failure)) => {
                        *slot = None;
                        Some(WasmEvent::Error((failure.message, failure.retryable)))
                    }
                }
            }
        }

        /// Open one stream over the guest capability view: authenticate,
        /// build the Responses body, send it.
        ///
        /// Typed as `StreamFailure`, not `String` (gh #566): flattening here
        /// threw away `class` and `retryable`, and the host fell back to
        /// guessing the class from the message text (gh #356).
        ///
        /// What crosses the WIT boundary is `(message, retryable)` - the
        /// stream-event case's shape - so `class` has no wire slot. The turn
        /// loop labels every event-delivered failure `transport`, which is the
        /// same exit code as `auth`/`invalid` (all three are the provider
        /// failure code) but a different word in the headless envelope. Pinned
        /// in the report rather than papered over: giving a class to a stream
        /// event is either a turn-loop change or a WIT change, and this cycle
        /// owns neither.
        fn open_stream(
            request: &lca_protocol::CompletionRequest,
        ) -> Result<::lca_wire_openai::ResponseStreamDriver<'static>, ::lca_wire_openai::StreamFailure>
        {
            // `IdentityFailure` -> `StreamFailure` is the kit's existing
            // bridge: class `invalid`, never retried (a missing login is a
            // configuration problem).
            let token = ::lca_subscription::access_token(&GUEST_CAP, &$spec)?;
            let account_id = $stored(&GUEST_CAP, "account_id");
            if account_id.is_empty() {
                return Err(::lca_wire_openai::StreamFailure {
                    message: $account_error.to_string(),
                    class: "invalid",
                    retryable: false,
                });
            }
            let system: String = request
                .messages
                .iter()
                .filter(|message| message.role == lca_protocol::MessageRole::System)
                .flat_map(|message| {
                    message.content.iter().filter_map(|block| match block {
                        lca_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            let effort = request.extras.get("reasoning-effort").map(String::as_str);
            let body = ::lca_wire_openai::build_responses_body(
                request,
                &system,
                &request.model,
                effort,
            );
            let body_bytes = serde_json::to_vec(&body).map_err(|err| {
                ::lca_wire_openai::StreamFailure {
                    message: format!("cannot build request: {err}"),
                    class: "invalid",
                    retryable: false,
                }
            })?;
            let url = format!(
                "{}{}",
                $endpoint(&GUEST_CAP, "api_base", $spec.api_base).trim_end_matches('/'),
                $responses_path
            );
            let headers = $headers(&account_id, &request.model, &token);
            let refs: Vec<(&str, &str)> = headers
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            ::lca_wire_openai::ResponseStreamDriver::open(
                &GUEST_CAP,
                &url,
                &refs,
                &body_bytes,
                &|| ::lca_subscription::purge_tokens(&GUEST_CAP),
            )
        }

        impl ModelsGuest for $wasm {
            fn list_models(_settings: Vec<ExtraPair>) -> Vec<WasmModel> {
                $list_models(&GuestCap)
                    .into_iter()
                    .map(|model| WasmModel {
                        id: model.id,
                        name: model.name,
                        context_window: model.context_window,
                        max_tokens: model.max_tokens,
                        // Gh #316: the ABI record has carried `extras`
                        // since its first draft (`wit/world-provider.wit`)
                        // and the host copies it through untouched, so
                        // hardcoding an empty list here made the WASM
                        // path advertise every model as having no
                        // metadata - no vision, no thinking levels, no
                        // prompt-cache lifetimes - while native kept it.
                        // An omission, not an ABI-shape problem: no WIT
                        // change, no new field.
                        extras: model
                            .extras
                            .into_iter()
                            .map(|(key, value)| ExtraPair { key, value })
                            .collect(),
                    })
                    .collect()
            }
        }

        impl CompletionGuest for $wasm {
            type CompletionStream = QueuedStream;

            fn stream_completion(
                request: exports::lca::ext::provider_completion::CompletionRequest,
            ) -> Result<CompletionStream, String> {
                let messages = request
                    .messages
                    .iter()
                    .map(|message| lca_protocol::ChatMessage {
                        role: match message.role.as_str() {
                            "system" => lca_protocol::MessageRole::System,
                            "user" => lca_protocol::MessageRole::User,
                            "assistant" => lca_protocol::MessageRole::Assistant,
                            _ => lca_protocol::MessageRole::Tool,
                        },
                        content: message
                            .content
                            .iter()
                            .map(|block| match block {
                                lca::ext::types::ContentBlock::Text(text) => {
                                    lca_protocol::ContentBlock::Text { text: text.clone() }
                                }
                                lca::ext::types::ContentBlock::Image((media_type, bytes)) => {
                                    lca_protocol::ContentBlock::Image {
                                        media_type: media_type.clone(),
                                        bytes: bytes.clone(),
                                    }
                                }
                            })
                            .collect(),
                        tool_calls: message
                            .tool_calls
                            .iter()
                            .map(|call| lca_protocol::ToolCall {
                                call_id: call.call_id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                                parent_call_id: None,
                            })
                            .collect(),
                        tool_call_id: message.tool_call_id.clone(),
                        usage: None,
                        // Gh #566: the WIT `message` record carries
                        // `extras: list<extra-pair>` and the host copies it
                        // through verbatim (the reserved-map contract). Writing
                        // an empty map here dropped every per-message extra a
                        // WASM-delivered subscription provider would have seen -
                        // `extras["queue"]` (ADR-0038's steering marker) above
                        // all, so a WASM codex/grok turn could not tell a steered
                        // message from a typed one. Same omission gh #316 fixed
                        // for `ModelInfo.extras` one screen above, and the same
                        // fix: copy the pairs, no ABI change.
                        extras: message
                            .extras
                            .iter()
                            .map(|pair| (pair.key.clone(), pair.value.clone()))
                            .collect(),
                    })
                    .collect();
                let tools = request
                    .tools
                    .iter()
                    .map(|tool| lca_protocol::ToolSpec {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: serde_json::from_str(&tool.parameters)
                            .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
                        exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
            extras: tool
                            .extras
                            .iter()
                            .map(|pair| (pair.key.clone(), pair.value.clone()))
                            .collect(),
                    })
                    .collect();
                let protocol_request = lca_protocol::CompletionRequest {
                    messages,
                    tools,
                    model: request.model,
                    stable_prefix: request.stable_prefix as usize,
                    extras: request
                        .extras
                        .iter()
                        .map(|pair| (pair.key.clone(), pair.value.clone()))
                        .collect(),
                };
                match open_stream(&protocol_request) {
                    Ok(driver) => Ok(CompletionStream::new(QueuedStream {
                        driver: RefCell::new(Some(driver)),
                        seed: RefCell::new(None),
                    })),
                    // Gh #566: the stream is created and carries the failure as
                    // its own typed error, so the exact `retryable` reaches the
                    // turn loop's retry rule. Returned as `Err(String)` instead,
                    // the host would re-derive it from the message text.
                    Err(failure) => Ok(CompletionStream::new(QueuedStream {
                        driver: RefCell::new(None),
                        seed: RefCell::new(Some(::lca_subscription::open_failure_event(&failure))),
                    })),
                }
            }
        }

        impl IdentityGuest for $wasm {
            fn login() -> WasmOutcome {
                match $login(&GuestCap, &GuestCap) {
                    Ok(outcome) => to_wit_outcome(outcome),
                    Err(err) => WasmOutcome::Failed(err.0),
                }
            }

            fn logout() -> WasmOutcome {
                to_wit_outcome($logout(&GuestCap))
            }

            fn usage() -> Result<TokenUsage, WasmOutcome> {
                match $usage(&GuestCap) {
                    Ok(usage) => Ok(TokenUsage {
                        input: usage.input,
                        output: usage.output,
                        cache_read: usage.cache_read,
                        cache_write: usage.cache_write,
                        cache_write_hour: usage.cache_write_1h,
                        cost: usage.cost,
                        extras: usage
                            .extras
                            .iter()
                            .map(|(key, value)| ExtraPair {
                                key: key.clone(),
                                value: value.clone(),
                            })
                            .collect(),
                    }),
                    Err(err) => Err(WasmOutcome::Failed(err.0)),
                }
            }
        }

        impl LoginGuest for $wasm {
            fn login_options() -> Vec<WasmLoginOption> {
                // Signs in through its identity flow directly, so it
                // offers no picker presets (ADR-0033).
                Vec::new()
            }

            fn login_submit(_answer: WasmLoginAnswer) -> WasmLoginResult {
                WasmLoginResult::Ok
            }
        }

        export_provider!($wasm);
    };
}
