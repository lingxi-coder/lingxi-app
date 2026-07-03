//! Tests for `service.rs`, extracted from inline `#[cfg(test)]` blocks. Included via `#[path] mod service_test;`.

pub use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::retry::DEFAULT_MAX_RETRIES;
    use crate::{
        AuthStrategy, BoxFuture, Capabilities, ClientConfig, CredentialConfig, LlmError,
        ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, ProviderRequest,
        ProviderResponse, StreamingResponse,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // Local opaque stand-ins for the orchestrator's locked-template constants
    // (the build_request tests assert the system prefix self-referentially and
    // use SECTION_SEP only as the "\n\n" separator, so any non-empty HEADER works).
    const HEADER: &str = "You are LingXi, an agentic command-line coding assistant.";
    const SECTION_SEP: &str = "\n\n";

    // ── FakeTransport ─────────────────────────────────────────────────────────

    /// A fake Transport that returns a scripted sequence of responses (or errors).
    struct FakeTransport {
        /// Pre-recorded responses returned in order; cycles to last entry.
        responses: Mutex<Vec<FakeResponse>>,
        /// All requests received, in order.
        seen: Mutex<Vec<ProviderRequest>>,
    }

    #[derive(Clone)]
    #[allow(dead_code)]
    enum FakeResponse {
        Ok(ProviderResponse),
        Err(LlmError),
    }

    impl FakeTransport {
        /// Return the same response on every call.
        fn always(resp: ProviderResponse) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(vec![FakeResponse::Ok(resp)]),
                seen: Mutex::new(vec![]),
            })
        }

        /// Return each response in sequence; the last is repeated forever.
        fn sequence(resps: Vec<FakeResponse>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(resps),
                seen: Mutex::new(vec![]),
            })
        }

        fn seen_count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }

        fn seen_headers(&self, idx: usize) -> BTreeMap<String, String> {
            self.seen.lock().unwrap()[idx].headers.clone()
        }

        /// Return the `"model"` field from the JSON body of the `idx`-th request.
        /// Useful for asserting chain-walk model sequences.
        fn seen_body_model(&self, idx: usize) -> Option<String> {
            self.seen.lock().unwrap()[idx]
                .body_json
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
    }

    impl Transport for FakeTransport {
        fn execute<'a>(
            &'a self,
            request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            let mut seen = self.seen.lock().unwrap();
            seen.push(request.clone());
            let idx = (seen.len() - 1).min({
                let resps = self.responses.lock().unwrap();
                resps.len().saturating_sub(1)
            });
            let resp = {
                let resps = self.responses.lock().unwrap();
                resps[idx].clone()
            };
            Box::pin(async move {
                match resp {
                    FakeResponse::Ok(r) => Ok(r),
                    FakeResponse::Err(e) => Err(e),
                }
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "open_stream not scripted".to_string(),
                })
            })
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn ok_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": "msg_test",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
    }

    fn make_adapter_with_subscriber(
        transport: Arc<dyn Transport>,
        subscriber: SubscriberState,
    ) -> ApiService {
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ApiService::new(
            client,
            transport,
            subscriber,
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
    }

    fn make_adapter(transport: Arc<dyn Transport>) -> ApiService {
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ApiService::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
    }

    fn make_adapter_for_protocol(
        protocol: ProtocolFamily,
        provider_id: ProviderId,
        base_url: &str,
    ) -> ApiService {
        let azure = if matches!(protocol, ProtocolFamily::AzureOpenAi) {
            Some(crate::AzureConfig {
                api_version: "2024-02-01".to_string(),
            })
        } else {
            None
        };
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id,
                    profile_name: "p".to_string(),
                    base_url: base_url.to_string(),
                    protocol,
                    auth: AuthStrategy::None,
                    credential: CredentialConfig::None,
                    models: vec![ModelProfile {
                        display_model: "model".to_string(),
                        request_model: "model".to_string(),
                        billing_model: "model".to_string(),
                        aliases: Vec::new(),
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            structured_output: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ApiService::new(
            client,
            FakeTransport::always(ProviderResponse::json(200, ok_response_json())),
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
    }

    async fn headers_after_inject_for_protocol(
        protocol: ProtocolFamily,
        provider_id: ProviderId,
        base_url: &str,
    ) -> BTreeMap<String, String> {
        let adapter = make_adapter_for_protocol(protocol, provider_id, base_url);
        let request = LlmRequest::new("model").with_user_text("hi");
        let mut prepared = adapter.client.prepare(&request).await.expect("prepare");
        adapter.inject_headers(&mut prepared, "req_test");
        prepared.provider_request.headers
    }

    // ── Prompt-cache breakpoints (CACHE.1) ──────────────────────────────────

    // Serializes the two prompt-cache tests: one mutates DISABLE_PROMPT_CACHING
    // (process-global), so the default-on assertion in the other must not run
    // concurrently. Lock poison is benign here — recover the guard.
    static CACHE_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn text_user_msg(s: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: s.to_string(),
            }],
            is_meta: false,
        }
    }

    /// `last_request_id()` captures the provider's server-side request id on
    /// every recorded headers pass (origin = Server), falls back to the
    /// client-generated id when no server header is present (origin = Client),
    /// and clears only when both are absent. This is the slot the persisted
    /// assistant line's top-level `requestId` reads from.
    #[test]
    fn last_request_id_captures_request_id_header() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        assert_eq!(adapter.last_request_id(), None);
        assert_eq!(adapter.last_request_id_origin(), None);

        let mut h = std::collections::BTreeMap::new();
        h.insert("request-id".to_string(), "req_011abc".to_string());
        adapter.record_rate_limit_from_headers(&h, "client_xyz");
        assert_eq!(adapter.last_request_id(), Some("req_011abc".to_string()));
        // Server header present → server origin (client id ignored).
        assert_eq!(
            adapter.last_request_id_origin(),
            Some(RequestIdOrigin::Server)
        );

        // `x-request-id` (OpenAI/generic) is also a server header → server origin.
        let mut h2 = std::collections::BTreeMap::new();
        h2.insert("x-request-id".to_string(), "req_xfallback".to_string());
        adapter.record_rate_limit_from_headers(&h2, "client_xyz");
        assert_eq!(adapter.last_request_id(), Some("req_xfallback".to_string()));
        assert_eq!(
            adapter.last_request_id_origin(),
            Some(RequestIdOrigin::Server)
        );

        // No server id header → fall back to the client-generated id, marked
        // client-origin (correlation-only, not provider-lookupable).
        adapter.record_rate_limit_from_headers(&std::collections::BTreeMap::new(), "client_xyz");
        assert_eq!(adapter.last_request_id(), Some("client_xyz".to_string()));
        assert_eq!(
            adapter.last_request_id_origin(),
            Some(RequestIdOrigin::Client)
        );

        // Neither a server header nor a client id → cleared (no stale leak).
        adapter.record_rate_limit_from_headers(&std::collections::BTreeMap::new(), "");
        assert_eq!(adapter.last_request_id(), None);
        assert_eq!(adapter.last_request_id_origin(), None);
    }

    #[test]
    fn build_request_splits_system_into_org_blocks_by_default() {
        use crate::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        // A HEADER-prefixed assembled-shape system prompt splits into prefix +
        // rest (splitSysPromptPrefix default mode), both org-scoped.
        let system = format!("{HEADER}{SECTION_SEP}rest body here");
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hello")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // (a) two system blocks: prefix (HEADER) + rest, each org-scoped → each
        // carries an ephemeral breakpoint (the attribution block is never
        // emitted, so 2 not 3).
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].text, HEADER);
        assert_eq!(req.system[0].cache_control, Some(CacheControl::Ephemeral));
        assert_eq!(req.system[1].text, "rest body here");
        assert_eq!(req.system[1].cache_control, Some(CacheControl::Ephemeral));
        // (c) the last message's last block carries the one message breakpoint.
        let last = req.messages.last().expect("a message");
        match last.content.last().expect("a content block") {
            LlmContentBlock::Text { cache_control, .. } => {
                assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
            }
            other => panic!("expected trailing text block, got {other:?}"),
        }
    }

    #[test]
    fn thinking_is_provider_aware() {
        use crate::model::thinking::ThinkingConfig;
        use crate::ReasoningConfig;
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let build = |model: &str| {
            adapter
                .build_request(
                    model,
                    None,
                    None,
                    vec![text_user_msg("hi")],
                    vec![],
                    false,
                    None,
                )
                .expect("build_request")
                .reasoning
        };

        // Claude with the default (Adaptive) thinking → Adaptive, byte-faithful.
        assert_eq!(
            build("claude-opus-4-8-20260115"),
            Some(ReasoningConfig::Adaptive)
        );
        // Non-Claude with the default (Adaptive) → NO reasoning field (provider
        // applies its own default instead of a forced high-effort / budget-0).
        assert_eq!(build("gpt-5"), None);
        assert_eq!(build("gemini-2.5-pro"), None);
        assert_eq!(build("deepseek-reasoner"), None);

        // An EXPLICIT fixed budget on a non-Claude model is still honored.
        let adapter_fixed = make_adapter(FakeTransport::always(ProviderResponse::json(
            200,
            ok_response_json(),
        )))
        .with_thinking(ThinkingConfig::Enabled {
            budget_tokens: 4096,
        });
        let r = adapter_fixed
            .build_request(
                "gpt-5",
                None,
                None,
                vec![text_user_msg("hi")],
                vec![],
                false,
                None,
            )
            .expect("build_request")
            .reasoning;
        assert_eq!(
            r,
            Some(ReasoningConfig::Enabled {
                budget_tokens: 4096
            })
        );
    }

    #[test]
    fn build_request_omits_cache_breakpoints_when_disabled() {
        use crate::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DISABLE_PROMPT_CACHING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let system = format!("{HEADER}{SECTION_SEP}rest body here");
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hello")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        // Still split into 2 blocks, but none carry a breakpoint.
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].cache_control, None);
        assert_eq!(req.system[1].cache_control, None);
        let last = req.messages.last().expect("a message");
        match last.content.last().expect("a content block") {
            LlmContentBlock::Text { cache_control, .. } => assert_eq!(*cache_control, None),
            other => panic!("expected trailing text block, got {other:?}"),
        }
    }

    #[test]
    fn build_request_global_cache_gate_dormant_by_default() {
        // Without the opt-in env, the global gate is off even for a subscriber:
        // a boundary-bearing prompt still splits org-default (2 blocks).
        use crate::prompt_format::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("LINGXI_GLOBAL_CACHE_SCOPE");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let system = format!(
            "{HEADER}{SECTION_SEP}static{SECTION_SEP}{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}{SECTION_SEP}dynamic"
        );
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hi")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // Gate off → org default (no scope:global block, marker left inline).
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].cache_control, Some(CacheControl::Ephemeral));
    }

    #[test]
    fn build_request_global_cache_gate_armed_marks_global_static() {
        // With the opt-in env + subscriber, the 1P global path activates and the
        // static block carries scope:global while prefix/dynamic are uncached.
        use crate::prompt_format::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
        use crate::CacheScope;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS");
        std::env::set_var("LINGXI_GLOBAL_CACHE_SCOPE", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let system = format!(
            "{HEADER}{SECTION_SEP}static{SECTION_SEP}{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}{SECTION_SEP}dynamic"
        );
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hi")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("LINGXI_GLOBAL_CACHE_SCOPE");
        assert_eq!(req.system.len(), 3);
        assert_eq!(req.system[0].text, HEADER);
        assert_eq!(req.system[0].cache_control, None); // prefix uncached
        assert_eq!(req.system[1].text, "static");
        assert_eq!(
            req.system[1].cache_control,
            Some(CacheControl::EphemeralScoped {
                scope: Some(CacheScope::Global),
                ttl_1h: false
            })
        );
        assert_eq!(req.system[2].text, "dynamic");
        assert_eq!(req.system[2].cache_control, None); // dynamic uncached
    }

    // ── 1P cache-EDITING (cache_edits / cache_reference, RESIDUAL 4) ───────────

    /// A multi-message conversation: an assistant tool_use, a user tool_result,
    /// an assistant text, then a trailing user text. Only the trailing message
    /// carries the cache_control marker, so the tool_result (in an earlier user
    /// message) is strictly within the cached prefix.
    fn tool_result_conversation() -> Vec<ConversationMessage> {
        use protocol::{ContentBlock as PB, ConversationMessage as CM, MessageId, ToolUseId};
        let tool_id = ToolUseId::new();
        vec![
            CM::Assistant {
                id: MessageId::new(),
                content: vec![PB::ToolUse {
                    id: tool_id.clone(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "/x"}),
                    provider_id: Some("toolu_abc".to_string()),
                }],
                stop_reason: None,
            },
            CM::User {
                id: MessageId::new(),
                content: vec![PB::ToolResult {
                    tool_use_id: tool_id,
                    content: "file body".to_string(),
                    is_error: false,
                    provider_tool_use_id: Some("toolu_abc".to_string()),
                    content_blocks: None,
                }],
                is_meta: false,
            },
            CM::Assistant {
                id: MessageId::new(),
                content: vec![PB::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: None,
            },
            text_user_msg("continue"),
        ]
    }

    #[test]
    fn build_request_cache_editing_dormant_by_default() {
        // Gate OFF (default): no cache_reference on tool_results, no cache_edits
        // block — byte-identical to the pre-feature request.
        use crate::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("LINGXI_CACHE_EDITING");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        // Even a subscriber + injected edits must stay inert without the env.
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        )
        .with_cache_editing_inputs(CacheEditingInputs {
            new_edits: vec![crate::CacheEdit::Delete {
                cache_reference: "toolu_zzz".to_string(),
            }],
            pinned: vec![],
        });
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                tool_result_conversation(),
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // No cache_edits block anywhere.
        for m in &req.messages {
            for b in &m.content {
                assert!(
                    !matches!(b, LlmContentBlock::CacheEdits { .. }),
                    "no cache_edits block on the default path"
                );
                if let LlmContentBlock::ToolResult {
                    cache_reference, ..
                } = b
                {
                    assert_eq!(*cache_reference, None, "no cache_reference by default");
                }
            }
        }
    }

    #[test]
    fn build_request_cache_editing_armed_stamps_refs_and_inserts_block() {
        // Gate ARMED (subscriber + opt-in env): tool_results before the marker
        // get cache_reference=tool_use_id, and injected new+pinned cache_edits
        // are inserted with cross-block delete-ref dedup.
        use crate::{CacheEdit, ContentBlock as LlmContentBlock};
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS");
        std::env::set_var("LINGXI_CACHE_EDITING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        // pinned (pos 1, the tool_result user msg) deletes ref "dup" + "p1";
        // new (last user msg) deletes "dup" (collapsed by dedup) + "n1".
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        )
        .with_cache_editing_inputs(CacheEditingInputs {
            new_edits: vec![
                CacheEdit::Delete {
                    cache_reference: "dup".to_string(),
                },
                CacheEdit::Delete {
                    cache_reference: "n1".to_string(),
                },
            ],
            pinned: vec![PinnedCacheEdits {
                user_message_index: 1,
                edits: vec![
                    CacheEdit::Delete {
                        cache_reference: "dup".to_string(),
                    },
                    CacheEdit::Delete {
                        cache_reference: "p1".to_string(),
                    },
                ],
            }],
        });
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                tool_result_conversation(),
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("LINGXI_CACHE_EDITING");

        // (a) cache_reference stamped on the tool_result (it precedes the marker).
        let mut stamped = 0;
        for m in &req.messages {
            for b in &m.content {
                if let LlmContentBlock::ToolResult {
                    tool_call_id,
                    cache_reference,
                    ..
                } = b
                {
                    assert_eq!(cache_reference.as_deref(), Some(tool_call_id.as_str()));
                    stamped += 1;
                }
            }
        }
        assert_eq!(stamped, 1, "exactly one tool_result stamped");

        // (b) collect every cache_edits delete ref across the whole request.
        let mut refs: Vec<String> = vec![];
        for m in &req.messages {
            for b in &m.content {
                if let LlmContentBlock::CacheEdits { edits } = b {
                    for e in edits {
                        let CacheEdit::Delete { cache_reference } = e;
                        refs.push(cache_reference.clone());
                    }
                }
            }
        }
        refs.sort();
        // dedup: "dup" appears once (pinned wins, new collapses), plus p1 + n1.
        assert_eq!(
            refs,
            vec!["dup".to_string(), "n1".to_string(), "p1".to_string()]
        );

        // (c) the pinned block landed in the tool_result user message, spliced
        // immediately AFTER the tool_result block.
        let pinned_msg = &req.messages[1];
        let tr_pos = pinned_msg
            .content
            .iter()
            .position(|b| matches!(b, LlmContentBlock::ToolResult { .. }))
            .expect("tool_result present");
        assert!(matches!(
            pinned_msg.content[tr_pos + 1],
            LlmContentBlock::CacheEdits { .. }
        ));
    }

    // ── build_request profile threading (Unit B Task 5) ──────────────────────

    #[test]
    fn build_request_sets_profile_when_provided() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let req = adapter
            .build_request(
                "gpt-5.2",
                Some("github-copilot"),
                None,
                vec![],
                vec![],
                false,
                None,
            )
            .expect("build_request with profile");
        assert_eq!(req.model, "gpt-5.2", "model must be preserved verbatim");
        assert_eq!(
            req.profile.as_deref(),
            Some("github-copilot"),
            "profile must be threaded into LlmRequest"
        );
    }

    #[test]
    fn build_request_leaves_profile_none_when_not_provided() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let req = adapter
            .build_request("claude-opus-4-7", None, None, vec![], vec![], false, None)
            .expect("build_request without profile");
        assert_eq!(
            req.model, "claude-opus-4-7",
            "model must be preserved verbatim"
        );
        assert!(
            req.profile.is_none(),
            "profile must be None when not passed"
        );
    }

    // ── build_request thinking / temperature / max_tokens (DIV-1/3/4) ────────

    // Serialize the env-touching thinking tests: they mutate process-global
    // LINGXI_DISABLE_THINKING. A module-level mutex keeps them from racing
    // each other (and is poison-tolerant).
    static THINKING_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_thinking_env() {
        std::env::remove_var("LINGXI_DISABLE_THINKING");
        std::env::remove_var("LINGXI_DISABLE_ADAPTIVE_THINKING");
    }

    #[test]
    fn build_request_adaptive_models_get_adaptive_no_temperature_model_max_tokens() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport); // default ThinkingConfig::Adaptive

        // (model, expected model-max-output-tokens) — binary YCe (v2.1.183):
        // opus-4-8 / fable-5 → 64k default; sonnet-4-6 → 32k default.
        // 2.1.198 pIe: sonnet-5 → 64k default (adaptive via registry capability).
        for (model, expected_max) in [
            ("claude-opus-4-8", 64_000u32),
            ("claude-sonnet-4-6", 32_000),
            ("claude-sonnet-5", 64_000),
            ("claude-fable-5", 64_000),
        ] {
            let req = adapter
                .build_request(model, None, None, vec![], vec![], false, None)
                .expect("build_request");
            assert_eq!(
                req.reasoning,
                Some(crate::ReasoningConfig::Adaptive),
                "{model} → adaptive"
            );
            assert!(
                req.temperature.is_none(),
                "{model} → no temperature when thinking on"
            );
            assert_eq!(
                req.max_tokens,
                Some(expected_max),
                "{model} → model max_tokens"
            );
        }
        clear_thinking_env();
    }

    #[test]
    fn build_request_non_adaptive_model_gets_fixed_budget() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport); // default Adaptive intent, but model disallows adaptive

        // haiku-4-5 supports thinking but NOT adaptive → Enabled{upperLimit-1}.
        // compaction max output for haiku-4-5 = (32_000, 64_000) → budget 63_999,
        // clamped to max_tokens(32_000)-1 = 31_999.
        let req = adapter
            .build_request("claude-haiku-4-5", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert_eq!(req.max_tokens, Some(32_000));
        assert_eq!(
            req.reasoning,
            Some(crate::ReasoningConfig::Enabled {
                budget_tokens: 31_999
            }),
            "haiku-4-5 → fixed budget clamped to max_tokens-1"
        );
        assert!(req.temperature.is_none(), "thinking on → no temperature");
        clear_thinking_env();
    }

    /// cc 2.1.198 "Subagents + compaction inherit extended thinking config" —
    /// the SUBAGENT seam half. `ProviderApiAdapter`'s `agent::SubagentApiClient`
    /// impl delegates 1:1 to `ApiService::messages_create` / `stream`, which
    /// route every request through the SAME `build_request` and thus the SAME
    /// session `self.thinking` (binary: the child session's options carry
    /// `thinkingConfig: sDi(n.options.thinkingConfig, …)` @215628753). Lock
    /// that the subagent entry point issues a wire body whose `thinking`
    /// field matches the session config exactly like the main loop's.
    #[tokio::test]
    async fn subagent_entry_point_inherits_session_thinking_config() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let model = "claude-sonnet-4-20250514";

        // Default session config (Adaptive intent): the issued body carries the
        // SAME thinking the shared main-loop builder computes for this model.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let expected = match adapter
            .build_request(model, None, None, vec![], vec![], false, None)
            .expect("build_request")
            .reasoning
        {
            Some(crate::ReasoningConfig::Adaptive) => serde_json::json!({"type": "adaptive"}),
            Some(crate::ReasoningConfig::Enabled { budget_tokens }) => {
                serde_json::json!({"type": "enabled", "budget_tokens": budget_tokens})
            }
            None => serde_json::Value::Null,
        };
        assert_ne!(expected, serde_json::Value::Null, "session thinking is ON by default");
        adapter
            .messages_create(model, None, None, vec![], vec![])
            .await
            .expect("messages_create");
        let body = transport.seen.lock().unwrap()[0].body_json.clone();
        assert_eq!(
            body.get("thinking").cloned().unwrap_or(serde_json::Value::Null),
            expected,
            "subagent seam body inherits the session thinking config"
        );

        // Explicit session config (fixed budget): the subagent seam carries it too.
        let transport2 = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter2 = make_adapter(transport2.clone())
            .with_thinking(crate::model::thinking::ThinkingConfig::Enabled { budget_tokens: 2_048 });
        adapter2
            .messages_create(model, None, None, vec![], vec![])
            .await
            .expect("messages_create");
        let body2 = transport2.seen.lock().unwrap()[0].body_json.clone();
        assert_eq!(
            body2["thinking"],
            serde_json::json!({"type": "enabled", "budget_tokens": 2_048}),
            "an explicit session budget rides on the subagent seam"
        );
        clear_thinking_env();
    }

    #[test]
    fn build_request_disable_thinking_env_drops_reasoning_sets_temperature() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        std::env::set_var("LINGXI_DISABLE_THINKING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);

        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert!(req.reasoning.is_none(), "thinking disabled → no reasoning");
        // opus-4-8 is not in the `rhn` temperature-gate set → no temperature even
        // when thinking is env-disabled.
        assert!(
            req.temperature.is_none(),
            "opus-4-8 thinking-disabled → no temperature (not in rhn set)"
        );
        // max_tokens still the model value (binary YCe: opus-4-8 → 64k).
        assert_eq!(req.max_tokens, Some(64_000));
        clear_thinking_env();
    }

    #[test]
    fn build_request_explicit_max_tokens_override_wins() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        // Escalation override (Some) honored verbatim.
        let req = adapter
            .build_request(
                "claude-opus-4-8",
                None,
                None,
                vec![],
                vec![],
                false,
                Some(7_777),
            )
            .expect("build_request");
        assert_eq!(req.max_tokens, Some(7_777));
        clear_thinking_env();
    }

    #[test]
    fn build_request_thinking_disabled_config_drops_reasoning() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter =
            make_adapter(transport).with_thinking(crate::model::thinking::ThinkingConfig::Disabled);
        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert!(
            req.reasoning.is_none(),
            "ThinkingConfig::Disabled → no reasoning"
        );
        // opus-4-8 is NOT in the `rhn` temperature-gate set → field omitted even
        // with thinking disabled (binary @205866168: `!xs && rhn(u) ? … : void 0`).
        assert!(
            req.temperature.is_none(),
            "opus-4-8 thinking-disabled → no temperature (not in rhn set)"
        );
        clear_thinking_env();
    }

    #[test]
    fn build_request_thinking_disabled_temperature_only_for_rhn_models() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter =
            make_adapter(transport).with_thinking(crate::model::thinking::ThinkingConfig::Disabled);
        // claude-sonnet-4-5 IS in the `rhn` set → temperature:1 is sent.
        let req = adapter
            .build_request("claude-sonnet-4-5", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert!(req.reasoning.is_none(), "thinking disabled → no reasoning");
        assert_eq!(
            req.temperature,
            Some(1.0),
            "sonnet-4-5 thinking-disabled → temperature:1 (in rhn set)"
        );
        clear_thinking_env();
    }

    #[test]
    fn build_request_metadata_threaded_when_set() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport).with_request_metadata(crate::RequestMetadata {
            user_id: "{\"session_id\":\"s1\"}".to_string(),
        });
        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert_eq!(
            req.metadata,
            Some(crate::RequestMetadata {
                user_id: "{\"session_id\":\"s1\"}".to_string()
            })
        );

        // Default adapter → no metadata.
        let bare = make_adapter(FakeTransport::always(ProviderResponse::json(
            200,
            ok_response_json(),
        )))
        .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
        .expect("build_request");
        assert!(bare.metadata.is_none());
        clear_thinking_env();
    }

    #[test]
    fn build_api_metadata_user_id_shapes_and_orders() {
        // Serialize env access (CLAUDE_CODE_EXTRA_METADATA) with the other
        // env-mutating tests in this module.
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_EXTRA_METADATA");
        // No extra → exactly the three canonical keys, in order, compact JSON.
        assert_eq!(
            ApiService::build_api_metadata_user_id("dev123", "acct-9", "sess-1"),
            r#"{"device_id":"dev123","account_uuid":"acct-9","session_id":"sess-1"}"#
        );
        // Empty account_uuid (the `?? ''` branch) still emits the key.
        assert_eq!(
            ApiService::build_api_metadata_user_id("d", "", "s"),
            r#"{"device_id":"d","account_uuid":"","session_id":"s"}"#
        );
        // Valid extra object is spread FIRST; a colliding key keeps its first
        // position but takes the canonical value (JS `{...extra, device_id,…}`).
        std::env::set_var(
            "CLAUDE_CODE_EXTRA_METADATA",
            r#"{"team":"core","device_id":"override"}"#,
        );
        assert_eq!(
            ApiService::build_api_metadata_user_id("dev", "acct", "sess"),
            r#"{"team":"core","device_id":"dev","account_uuid":"acct","session_id":"sess"}"#
        );
        // Invalid extra (not a JSON object) is ignored.
        std::env::set_var("CLAUDE_CODE_EXTRA_METADATA", "not json");
        assert_eq!(
            ApiService::build_api_metadata_user_id("d", "a", "s"),
            r#"{"device_id":"d","account_uuid":"a","session_id":"s"}"#
        );
        std::env::remove_var("CLAUDE_CODE_EXTRA_METADATA");
    }

    // ── effective_subscriber (batch-5 Task 3: live SharedSubscription) ───────

    fn shared_slot(
        snap: Option<traits::subscription::SubscriptionSnapshot>,
    ) -> traits::subscription::SharedSubscription {
        Arc::new(std::sync::RwLock::new(snap))
    }

    #[test]
    fn effective_subscriber_prefers_live_snapshot() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, SubscriberState::default())
            .with_subscription(shared_slot(Some(
                traits::subscription::SubscriptionSnapshot {
                    is_subscriber: true,
                    subscription_type: Some("enterprise".to_string()),
                    ..Default::default()
                },
            )));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(sub.is_enterprise);
    }

    #[test]
    fn effective_subscriber_falls_back_when_slot_empty_or_absent() {
        let static_state = SubscriberState {
            is_subscriber: true,
            is_enterprise: false,
        };

        // No slot attached → static build-time state.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, static_state);
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);

        // Slot attached but unresolved (None) → static build-time state.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, static_state)
            .with_subscription(shared_slot(None));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);
    }

    #[test]
    fn effective_subscriber_non_enterprise_tier_is_not_enterprise() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, SubscriberState::default())
            .with_subscription(shared_slot(Some(
                traits::subscription::SubscriptionSnapshot {
                    is_subscriber: true,
                    subscription_type: Some("team".to_string()),
                    ..Default::default()
                },
            )));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);
    }

    // ── Previously-ignored tests (un-ignored, ported to FakeTransport) ────────

    #[tokio::test]
    async fn bridge_resolves_and_forwards_local_model() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let resp = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");
        assert_eq!(resp.model, "claude-sonnet-4-20250514");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn available_models_non_empty() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let models = adapter.available_models();
        assert!(
            !models.is_empty(),
            "available_models must return at least one entry"
        );
    }

    #[tokio::test]
    async fn bridge_forwards_batched_tools() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "read a file",
            "input_schema": {"type": "object"}
        })];
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                Vec::new(),
                tools,
            )
            .await
            .expect("ok");
        assert_eq!(transport.seen_count(), 1);
    }

    #[tokio::test]
    async fn messages_create_with_tools_on_tool_capable_model_succeeds() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "read",
            "input_schema": {"type": "object"}
        })];
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), tools)
            .await;
        assert!(result.is_ok(), "tool-capable model should accept tools");
    }

    #[tokio::test]
    async fn image_to_vision_model_is_allowed() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url {
                    url: "https://x/y.png".to_string(),
                },
            }],
            is_meta: false,
        }];
        // The capability check is in DefaultLlmClient.validate_capabilities; since
        // FakeTransport doesn't inspect the body, this exercises the whole path.
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, msgs, Vec::new())
            .await;
    }

    // ── New plan-named tests ──────────────────────────────────────────────────

    /// Plan test: 429 with `retry-after` triggers one sleep then succeeds.
    #[tokio::test]
    async fn live_path_surfaces_rate_limit_headers() {
        let mut rate_limit_headers = BTreeMap::new();
        rate_limit_headers.insert("retry-after".to_string(), "1".to_string());

        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: rate_limit_headers,
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let adapter = make_adapter(transport.clone());
        let resp = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok after retry");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        // Two executions: the 429 then the 200.
        assert_eq!(transport.seen_count(), 2);
    }

    /// Plan test: betas and User-Agent are injected post-prepare.
    #[tokio::test]
    async fn betas_and_user_agent_applied_post_prepare() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");
        let headers = transport.seen_headers(0);
        assert!(
            headers.contains_key("anthropic-beta"),
            "anthropic-beta must be present; got: {headers:?}"
        );
        let ua = headers.get("user-agent").cloned().unwrap_or_default();
        assert!(
            ua.starts_with("claude-cli/"),
            "user-agent must start with claude-cli/; got: {ua}"
        );
    }

    #[tokio::test]
    async fn anthropic_beta_header_is_protocol_scoped() {
        let anthropic_headers = headers_after_inject_for_protocol(
            ProtocolFamily::AnthropicMessages,
            ProviderId::AnthropicFirstParty,
            "https://api.anthropic.com",
        )
        .await;
        assert!(anthropic_headers.contains_key("anthropic-beta"));

        let routes = [
            (
                ProtocolFamily::OpenAiChat,
                "https://api.openai.com/v1",
                "openai",
            ),
            (
                ProtocolFamily::OpenAiResponses,
                "https://api.openai.com/v1",
                "openai-responses",
            ),
            (
                ProtocolFamily::GeminiGenerateContent,
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini",
            ),
            (
                ProtocolFamily::AzureOpenAi,
                "https://example.openai.azure.com/openai/deployments/model",
                "azure-openai",
            ),
            (
                ProtocolFamily::BedrockClaude,
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                "bedrock-claude",
            ),
            (
                ProtocolFamily::VertexClaude,
                "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/anthropic/models/model:rawPredict",
                "vertex-claude",
            ),
            (
                ProtocolFamily::VertexGemini,
                "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models/model:generateContent",
                "vertex-gemini",
            ),
        ];

        for (protocol, base_url, name) in routes {
            let headers = headers_after_inject_for_protocol(
                protocol,
                ProviderId::OpenAICompatible {
                    name: name.to_string(),
                },
                base_url,
            )
            .await;
            assert!(
                !headers.contains_key("anthropic-beta"),
                "{name} must not receive anthropic-beta: {headers:?}"
            );
            assert!(headers.contains_key("user-agent"));
            assert_eq!(
                headers.get("x-request-id").map(String::as_str),
                Some("req_test")
            );
        }
    }

    #[tokio::test]
    async fn user_agent_is_provider_aware() {
        // Anthropic-family routes keep the byte-faithful claude-cli UA.
        for (proto, pid, url) in [
            (
                ProtocolFamily::AnthropicMessages,
                ProviderId::AnthropicFirstParty,
                "https://api.anthropic.com",
            ),
            (
                ProtocolFamily::BedrockClaude,
                ProviderId::BedrockClaude,
                "https://bedrock-runtime.us-east-1.amazonaws.com",
            ),
        ] {
            let h = headers_after_inject_for_protocol(proto, pid, url).await;
            let ua = h.get("user-agent").expect("ua");
            assert!(ua.starts_with("claude-cli/"), "anthropic-family UA: {ua}");
        }

        // Non-Anthropic routes get a neutral UA, never claude-cli.
        let h = headers_after_inject_for_protocol(
            ProtocolFamily::OpenAiChat,
            ProviderId::OpenAICompatible {
                name: "openai".to_string(),
            },
            "https://api.openai.com/v1",
        )
        .await;
        let ua = h.get("user-agent").expect("ua");
        assert!(ua.starts_with("LingXi-Code/"), "neutral UA expected: {ua}");
        assert!(!ua.contains("claude-cli"), "must not leak claude-cli: {ua}");
    }

    #[tokio::test]
    async fn authenticator_user_agent_is_not_duplicated() {
        // Simulate the Copilot authenticator having set `User-Agent` during
        // prepare(): inject_headers must NOT add a second lowercase `user-agent`.
        let adapter = make_adapter_for_protocol(
            ProtocolFamily::OpenAiChat,
            ProviderId::OpenAICompatible {
                name: "github-copilot".to_string(),
            },
            "https://api.githubcopilot.com",
        );
        let request = LlmRequest::new("model").with_user_text("hi");
        let mut prepared = adapter.client.prepare(&request).await.expect("prepare");
        prepared
            .provider_request
            .headers
            .insert("User-Agent".to_string(), "LingXi-Code".to_string());
        adapter.inject_headers(&mut prepared, "req_test");
        let h = &prepared.provider_request.headers;
        assert_eq!(h.get("User-Agent").map(String::as_str), Some("LingXi-Code"));
        assert!(
            !h.contains_key("user-agent"),
            "no duplicate lowercase user-agent: {h:?}"
        );
    }

    /// Plan test: budget terminates after DEFAULT_MAX_RETRIES + 1 executions.
    #[tokio::test]
    async fn retry_terminal_after_budget() {
        // All responses are 500 — should retry DEFAULT_MAX_RETRIES times then fail.
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(ProviderResponse::json(
            500,
            serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "internal"}}),
        ))]);
        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            Some(0),
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_err(), "must fail after exhausting budget");
        // Should have tried DEFAULT_MAX_RETRIES + 1 = 11 times.
        assert_eq!(
            u32::try_from(transport.seen_count()).unwrap(),
            DEFAULT_MAX_RETRIES + 1,
            "expected {} executions, got {}",
            DEFAULT_MAX_RETRIES + 1,
            transport.seen_count()
        );
    }

    /// Plan test (Step 1b): x-should-retry: false on a 503 is terminal (no retry).
    #[tokio::test]
    async fn x_should_retry_false_is_terminal_for_5xx() {
        let mut headers = BTreeMap::new();
        headers.insert("x-should-retry".to_string(), "false".to_string());

        let transport = FakeTransport::always(ProviderResponse {
            status: 503,
            headers,
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "api_error", "message": "service unavailable"}
            }),
            request_id: None,
        });
        let adapter = make_adapter(transport.clone());
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_err(), "x-should-retry:false must be terminal");
        // Only ONE execution — no retries.
        assert_eq!(
            transport.seen_count(),
            1,
            "x-should-retry:false must not retry"
        );
    }

    /// Plan test: tool_use id round-trips through the adapter without mangling.
    #[tokio::test]
    async fn tool_use_id_round_trip_within_turn() {
        use crate::ContentBlock as LlmBlock;

        let response_json = serde_json::json!({
            "id": "msg_tool",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "tool_use", "id": "toolu_abc", "name": "Read", "input": {"path": "/x"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = make_adapter(transport);
        let resp = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");
        match resp.content.as_slice() {
            [LlmBlock::ToolCall { id, name, .. }] => {
                assert_eq!(id, "toolu_abc", "tool_use id must round-trip verbatim");
                assert_eq!(name, "Read");
            }
            other => panic!("expected single ToolCall, got: {other:?}"),
        }
    }

    // ---- MULTIMODAL.6: per-request media cap (stripExcessMediaItems) ----

    fn img(n: usize) -> protocol::ContentBlock {
        protocol::ContentBlock::Image {
            source: protocol::ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: format!("img{n}"),
            },
        }
    }

    fn user_with_imgs(range: std::ops::Range<usize>) -> ConversationMessage {
        let mut content = vec![ContentBlock::Text {
            text: "hi".to_string(),
        }];
        content.extend(range.map(img));
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content,
            is_meta: false,
        }
    }

    fn image_data_in_order(msgs: &[ConversationMessage]) -> Vec<String> {
        let mut out = Vec::new();
        for m in msgs {
            if let ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } = m
            {
                for b in content {
                    if let ContentBlock::Image {
                        source: protocol::ImageSource::Base64 { data, .. },
                    } = b
                    {
                        out.push(data.clone());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn count_media_counts_top_level_images_across_messages() {
        let msgs = vec![user_with_imgs(0..3), user_with_imgs(3..5)];
        assert_eq!(count_media(&msgs), 5);
    }

    #[test]
    fn tool_result_string_content_contributes_no_media() {
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: protocol::ToolUseId::new(),
                    content: "lots of text, no media".to_string(),
                    is_error: false,
                    provider_tool_use_id: None,
                    content_blocks: None,
                },
                img(0),
            ],
            is_meta: false,
        }];
        assert_eq!(count_media(&msgs), 1);
    }

    #[test]
    fn count_media_includes_nested_tool_result_media() {
        // An MCP image result populates `content_blocks` with `{"type":"image"}`
        // values; these MUST count toward the media cap (claude.ts:965-969), or an
        // image-heavy MCP transcript silently exceeds the API limit and 400s.
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: protocol::ToolUseId::new(),
                content: "see images".to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: Some(vec![
                    serde_json::json!({"type": "text", "text": "x"}),
                    serde_json::json!({"type": "image", "source": {"data": "AAA"}}),
                    serde_json::json!({"type": "image", "source": {"data": "BBB"}}),
                ]),
            }],
            is_meta: false,
        }];
        assert_eq!(count_media(&msgs), 2, "two nested image blocks must count");
    }

    #[test]
    fn strip_excess_media_strips_nested_tool_result_media() {
        // Over the cap, nested tool_result media is stripped oldest-first
        // (claude.ts:982-999), leaving the text + the most-recent nested image.
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: protocol::ToolUseId::new(),
                content: "imgs".to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: Some(vec![
                    serde_json::json!({"type": "image", "source": {"data": "a"}}),
                    serde_json::json!({"type": "image", "source": {"data": "b"}}),
                    serde_json::json!({"type": "image", "source": {"data": "c"}}),
                    serde_json::json!({"type": "text", "text": "keep"}),
                ]),
            }],
            is_meta: false,
        }];
        let stripped = strip_excess_media(msgs, 1);
        assert_eq!(
            count_media(&stripped),
            1,
            "nested media trimmed to the limit"
        );
        // The text block and the newest image survive.
        let ConversationMessage::User { content, .. } = &stripped[0] else {
            panic!("user message");
        };
        let ContentBlock::ToolResult {
            content_blocks: Some(blocks),
            ..
        } = &content[0]
        else {
            panic!("tool_result with content_blocks");
        };
        assert_eq!(
            blocks.len(),
            2,
            "one image + the text remain; got {blocks:?}"
        );
        assert!(blocks
            .iter()
            .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("text")));
    }

    #[test]
    fn strip_excess_media_trims_oldest_to_limit_without_touching_history() {
        let stored = vec![user_with_imgs(0..60), user_with_imgs(60..102)];
        assert_eq!(count_media(&stored), 102);

        let to_send = stored.clone();
        let trimmed = strip_excess_media(to_send, MAX_MEDIA_PER_REQUEST);

        assert_eq!(count_media(&trimmed), 100);
        let remaining = image_data_in_order(&trimmed);
        assert_eq!(remaining.len(), 100);
        assert_eq!(remaining.first().unwrap(), "img2");
        assert_eq!(remaining.last().unwrap(), "img101");
        assert!(!remaining.contains(&"img0".to_string()));
        assert!(!remaining.contains(&"img1".to_string()));

        assert_eq!(count_media(&stored), 102);
        assert_eq!(image_data_in_order(&stored).first().unwrap(), "img0");
    }

    #[test]
    fn strip_excess_media_leaves_within_limit_messages_unchanged() {
        let msgs = vec![user_with_imgs(0..50), user_with_imgs(50..100)];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before, "exactly 100 media → no stripping");
        assert_eq!(count_media(&out), 100);
    }

    #[test]
    fn strip_excess_media_no_images_is_noop() {
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: "no media here".to_string(),
            }],
            is_meta: false,
        }];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before);
    }

    // ── Fix 2: error_kind label-parity test ──────────────────────────────────

    /// Labels must be locked to api-client originals (Fix 2).
    ///
    /// Ensures that re-naming a label here triggers a test failure so the
    /// telemetry schema change is explicit.
    #[test]
    fn error_kind_labels_match_api_client_originals() {
        // api-client: Unauthorized → "unauthorized"
        assert_eq!(
            ApiService::error_kind(&LlmError::Authentication),
            "unauthorized"
        );
        assert_eq!(
            ApiService::error_kind(&LlmError::PermissionDenied),
            "unauthorized"
        );
        // api-client: Server → "server"
        assert_eq!(
            ApiService::error_kind(&LlmError::ProviderInternal),
            "server"
        );
        // api-client: Http → "http"
        assert_eq!(
            ApiService::error_kind(&LlmError::Transport {
                message: "t".into()
            }),
            "http"
        );
        // api-client: MalformedStream → "malformed_stream"
        assert_eq!(
            ApiService::error_kind(&LlmError::StreamInterrupted {
                message: "s".into()
            }),
            "malformed_stream"
        );
        // api-client: Overloaded → "overloaded"
        assert_eq!(
            ApiService::error_kind(&LlmError::Overloaded { repeated: false }),
            "overloaded"
        );
        // api-client: RateLimited → "rate_limited"
        assert_eq!(
            ApiService::error_kind(&LlmError::RateLimited {
                retry_after: None,
                scope: None
            }),
            "rate_limited"
        );
        // api-client: PromptTooLong → "prompt_too_long"
        assert_eq!(
            ApiService::error_kind(&LlmError::ContextOverflow { token_gap: 0 }),
            "prompt_too_long"
        );
    }

    // ── Task 8: subscriber 429 gate end-to-end through the adapter ───────────

    /// Task 8 gate: subscriber non-enterprise 429 → terminal immediately (no retry).
    ///
    /// Wire: `SubscriberState { is_subscriber: true, is_enterprise: false }` +
    /// a transport that always returns 429 → the adapter returns an error without
    /// making a second request.
    #[tokio::test]
    async fn subscriber_429_is_terminal_in_adapter() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "You have reached your usage limit"}
            }),
            request_id: None,
        });
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_err(), "subscriber 429 must be terminal");
        // Only ONE execution — no retries.
        assert_eq!(
            transport.seen_count(),
            1,
            "subscriber 429 must not retry (seen_count should be 1)"
        );
    }

    /// Directly guards [`ApiService::clear_pending_429`]: a staged 429
    /// snapshot must be discarded so a subsequent promote writes NOTHING. This
    /// goes RED iff `clear_pending_429`'s body is emptied (a no-op clear leaves
    /// the slot `Some`, so promote would copy A's `rejected` snapshot into
    /// `last_rate_limit`). The active cross-drive isolation is the per-attempt
    /// record stage-or-clear; this test is the credibility guard for the
    /// defensive drive-entry / success backstop.
    #[test]
    fn clear_pending_429_discards_staged_snapshot() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);

        // Stage: a 429 carrying a representative-claim → pending = Some(info).
        let headers = {
            let mut h = std::collections::BTreeMap::new();
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-status".to_string(),
                "rejected".to_string(),
            );
            h
        };
        adapter.record_rate_limit_from_429(&headers);
        // Sanity: the slot is genuinely staged before we clear it.
        assert!(
            adapter.pending_429.lock().unwrap().is_some(),
            "precondition: record_rate_limit_from_429 must stage a snapshot"
        );

        // Clear, then a terminal promote: with the slot emptied, promote is a
        // no-op and `last_rate_limit` stays None.
        adapter.clear_pending_429();
        adapter.promote_pending_429();

        assert_eq!(
            adapter.last_rate_limit_info(),
            None,
            "clear_pending_429 must discard the staged snapshot so promote writes nothing"
        );
    }

    /// Task 6 (batch 5): a terminal 429 whose response carries the unified
    /// headers records the forced-`rejected` snapshot AND the composed
    /// limits copy (byte-pinned: no reset header → no ` · resets …` clause,
    /// `rateLimitMessages.ts:149` + `:333-344`).
    #[tokio::test]
    async fn terminal_429_with_unified_headers_records_limits_copy() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    "seven_day".to_string(),
                );
                h.insert(
                    "anthropic-ratelimit-unified-status".to_string(),
                    "rejected".to_string(),
                );
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "You have reached your usage limit"}
            }),
            request_id: None,
        });
        // Subscriber (non-enterprise) → the 429 is terminal on the first try.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(
            matches!(result, Err(LlmError::RateLimited { .. })),
            "got {result:?}"
        );

        // The composed copy is cached for the orchestrator's terminal re-map.
        assert_eq!(
            adapter.last_rate_limit_error_message().as_deref(),
            Some("You've hit your weekly limit"),
            "seven_day → formatLimitReachedText('weekly limit', '') verbatim"
        );
        // errors.ts:482-516 — the limits snapshot is updated from the error's
        // headers with status FORCED 'rejected'.
        let info = adapter.last_rate_limit_info().expect("snapshot");
        assert_eq!(info.status.as_deref(), Some("rejected"));
        assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));
    }

    // ── M12: rate-limit warning flicker (2.1.196 monotonic guard) ────────────

    fn unified_headers(status: &str) -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert(
            "anthropic-ratelimit-unified-representative-claim".to_string(),
            "seven_day".to_string(),
        );
        h.insert(
            "anthropic-ratelimit-unified-status".to_string(),
            status.to_string(),
        );
        h
    }

    /// The `Bha`/`Nha` monotonic guard: an out-of-order (older-timestamp)
    /// response must NOT overwrite a newer at-limit snapshot, so the warning
    /// cannot flicker off while still at the limit. A genuinely NEWER response
    /// still updates.
    #[test]
    fn stale_parallel_response_does_not_flip_rate_limit_warning_off() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);

        // t=1000: the account is at its weekly limit (rejected).
        adapter.record_rate_limit_from_headers_at(&unified_headers("rejected"), "", 1000);
        assert_eq!(
            adapter.last_rate_limit_info().and_then(|i| i.status),
            Some("rejected".to_string()),
        );

        // t=500: a STALE parallel response reporting 'allowed' arrives late —
        // it must be DROPPED (no flicker): the warning stays 'rejected'.
        adapter.record_rate_limit_from_headers_at(&unified_headers("allowed"), "", 500);
        assert_eq!(
            adapter.last_rate_limit_info().and_then(|i| i.status),
            Some("rejected".to_string()),
            "a stale (older-timestamp) response must not flip the warning off"
        );

        // t=2000: a genuinely NEWER 'allowed' response clears the limit.
        adapter.record_rate_limit_from_headers_at(&unified_headers("allowed"), "", 2000);
        assert_eq!(
            adapter.last_rate_limit_info().and_then(|i| i.status),
            Some("allowed".to_string()),
            "a fresher response must update the snapshot"
        );
    }

    /// The guard is inert under normal monotonic operation: equal-or-increasing
    /// timestamps always record (the production path uses wall-clock `now_ms`).
    #[test]
    fn equal_or_increasing_timestamps_always_record() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        adapter.record_rate_limit_from_headers_at(&unified_headers("allowed"), "", 5000);
        adapter.record_rate_limit_from_headers_at(&unified_headers("rejected"), "", 5000);
        assert_eq!(
            adapter.last_rate_limit_info().and_then(|i| i.status),
            Some("rejected".to_string()),
            "an equal-timestamp record is not stale and updates"
        );
    }

    // ── B6-T1: terminal-only 429 state promotion (pending slot) ──────────────
    //
    // claude-code updates the limits/raw module state ONLY in the terminal
    // catch handler `extractQuotaStatusFromError` (claudeAiLimits.ts:487),
    // never on a retried attempt that later recovers. The Rust seam stages
    // each 429 attempt's snapshot in a `pending_429` slot and promotes it into
    // the live caches only when the retry loop declares the error TERMINAL —
    // and discards it on drive-entry and on any subsequent success.

    /// A 429 carrying BOTH the unified limits headers (gate passes) AND the
    /// per-window quartet (raw non-empty) that exhausts the retry budget
    /// promotes BOTH: the forced-`rejected` limits snapshot
    /// (`last_rate_limit_full`) and the raw per-window utilization
    /// (`last_raw_utilization`) — `extractRawUtilization` runs on the SAME
    /// error headers (claudeAiLimits.ts:500).
    #[tokio::test]
    async fn terminal_429_promotes_snapshot_and_raw() {
        let headers = {
            let mut h = BTreeMap::new();
            // Limits gate (from_429_error_headers → Some).
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            // Per-window quartet → raw non-empty.
            h.insert(
                "anthropic-ratelimit-unified-5h-utilization".to_string(),
                "0.42".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-5h-reset".to_string(),
                "1750000005".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-7d-utilization".to_string(),
                "0.77".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-7d-reset".to_string(),
                "1750000007".to_string(),
            );
            h
        };
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers,
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        });
        // Subscriber (non-enterprise) → the 429 is terminal on the first try.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(
            matches!(result, Err(LlmError::RateLimited { .. })),
            "got {result:?}"
        );

        // Promoted at the terminal: rejected limits snapshot.
        let info = adapter.last_rate_limit_info().expect("snapshot");
        assert_eq!(info.status.as_deref(), Some("rejected"));
        assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));

        // Promoted at the terminal: raw per-window utilization from the SAME
        // error headers (extractRawUtilization, ts:500).
        let raw = adapter.last_raw_utilization().expect("raw");
        assert_eq!(raw.five_hour.map(|w| w.utilization), Some(0.42));
        assert_eq!(raw.five_hour.map(|w| w.resets_at), Some(1_750_000_005));
        assert_eq!(raw.seven_day.map(|w| w.utilization), Some(0.77));
        assert_eq!(raw.seven_day.map(|w| w.resets_at), Some(1_750_000_007));
    }

    /// A 429-with-headers that is RETRIED and then RECOVERS on a 200 must NOT
    /// leave the rejected snapshot behind. This verifies the SUCCESS PATH:
    /// after recovery, `last_rate_limit` reflects the 200 (here headerless →
    /// `None`), NOT the retried 429 — because the limits cache is only written
    /// at the terminal promote, which never fires on a recovered turn. (The
    /// success-clear of the pending SLOT itself is guarded directly by
    /// `clear_pending_429_discards_staged_snapshot`, not here.)
    #[tokio::test]
    async fn retried_429_does_not_update_limits_snapshot() {
        let headers_429 = {
            let mut h = BTreeMap::new();
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-status".to_string(),
                "rejected".to_string(),
            );
            // retry-after 0 → no real sleep.
            h.insert("retry-after".to_string(), "0".to_string());
            h
        };
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: headers_429,
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            // Recovery: a 200 with NO unified headers.
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        // Enterprise subscriber → the 429 is retried, then the 200 recovers.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: true,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_ok(), "429 then 200 must recover: {result:?}");

        // The success seam cleared the limits cache (the 200 carried no unified
        // headers); the retried 429's rejected snapshot was NEVER promoted.
        assert_eq!(
            adapter.last_rate_limit_info(),
            None,
            "retried-then-recovered 429 must not plant a rejected snapshot"
        );
        // The 429 message copy was likewise cleared on success.
        assert_eq!(adapter.last_rate_limit_error_message(), None,);
    }

    /// Cross-drive isolation, asserted on the OBSERVABLE promoted snapshot: a
    /// pending 429 staged by an EARLIER drive must never survive into a LATER
    /// drive's terminal promote. Drive A = `[429-with-headers(seven_day),
    /// 400-invalid-request terminal]` — A stages a `seven_day` snapshot but the
    /// 400 is non-RateLimited, so it never promotes and the slot is orphaned.
    /// Drive B ends on a TERMINAL 429 carrying its OWN fresh headers
    /// (`five_hour`, DIFFERENT from A's) → B promotes B's snapshot. The promoted
    /// `last_rate_limit_full()` must read `five_hour` (DRIVE B), proving A's
    /// orphaned `seven_day` slot did NOT leak in.
    ///
    /// The ACTIVE isolation mechanism this asserts is the per-attempt record
    /// stage-or-clear in `record_rate_limit_from_429` (drive B's first attempt
    /// overwrites the slot with B's snapshot before the terminal promote runs).
    /// The drive-entry reset is a defensive backstop, not what this test
    /// exercises — `clear_pending_429_discards_staged_snapshot` guards that
    /// directly. This test is non-vacuous: it fails if promotion ever reads a
    /// stale slot (B's snapshot would be wrong, or `seven_day` would surface).
    #[tokio::test]
    async fn stale_pending_429_not_promoted_across_drives() {
        // Drive A's 429: representative-claim = seven_day. Staged then orphaned.
        let resp_429_a = ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    "seven_day".to_string(),
                );
                h.insert(
                    "anthropic-ratelimit-unified-status".to_string(),
                    "rejected".to_string(),
                );
                h.insert("retry-after".to_string(), "0".to_string());
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };
        // A plain 400 invalid_request → terminal, NON-rate-limited (no promote).
        let resp_400 = ProviderResponse {
            status: 400,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": "bad request"}
            }),
            request_id: None,
        };
        // Drive B's 429: representative-claim = five_hour (DIFFERENT from A).
        // Terminal here → B promotes B's OWN fresh snapshot.
        let resp_429_b = ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    "five_hour".to_string(),
                );
                h.insert(
                    "anthropic-ratelimit-unified-status".to_string(),
                    "rejected".to_string(),
                );
                h.insert("retry-after".to_string(), "0".to_string());
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };
        // Drive A consumes idx 0,1; Drive B consumes idx 2,3 (global cursor).
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(resp_429_a),
            FakeResponse::Ok(resp_400),
            FakeResponse::Ok(resp_429_b.clone()),
            FakeResponse::Ok(resp_429_b),
        ]);
        // Non-subscriber (429 is retryable), settings_max_retries=1 so each
        // drive retries exactly once then terminates; backoff 0 → no sleeps.
        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            Some(1),
            Some(0),
        );

        // Drive A: 429(seven_day) (retried, pending set) → 400 (terminal,
        // non-RateLimited → no promotion). Pending lingers with A's snapshot.
        let a = adapter
            .messages_create(
                "claude-haiku-4-20250307",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(
            matches!(a, Err(LlmError::InvalidRequest { .. })),
            "drive A must die on the 400, got {a:?}"
        );

        // Drive B: 429(five_hour) (retried, pending OVERWRITTEN with B's
        // snapshot) → 429(five_hour) terminal → promotes B's snapshot.
        let b = adapter
            .messages_create(
                "claude-haiku-4-20250307",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(
            matches!(b, Err(LlmError::RateLimited { .. })),
            "drive B must die on its terminal 429, got {b:?}"
        );

        // The promoted snapshot must be DRIVE B's (five_hour), proving drive A's
        // orphaned seven_day slot did not survive into B's promote.
        let info = adapter
            .last_rate_limit_info()
            .expect("drive B promotes its own snapshot");
        assert_eq!(
            info.rate_limit_type.as_deref(),
            Some("five_hour"),
            "promoted snapshot must reflect DRIVE B (five_hour), not A's stale seven_day"
        );
        assert_eq!(info.status.as_deref(), Some("rejected"));
    }

    /// Task 6 (batch 5): a 429 WITHOUT unified headers fails the
    /// `if (rateLimitType || overageStatus)` gate (errors.ts:480) — no copy
    /// is composed, and a copy from an earlier 429 is superseded (the slot
    /// reflects the most recent 429), so the generic surface applies.
    #[tokio::test]
    async fn terminal_429_without_unified_headers_records_no_copy() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        });
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(
            adapter.last_rate_limit_error_message(),
            None,
            "no unified headers on the 429 → no limits copy"
        );
    }

    /// Task 6 (batch 5): the live subscription slot's `pro` plan flips the
    /// `seven_day_sonnet` wording to "weekly limit"
    /// (`rateLimitMessages.ts:176-181`).
    #[tokio::test]
    async fn terminal_429_sonnet_copy_uses_pro_subscription_wording() {
        let resp_429 = |claim: &str| ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    claim.to_string(),
                );
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };

        // Without a pro/enterprise snapshot → "Sonnet limit".
        let adapter = make_adapter_with_subscriber(
            FakeTransport::always(resp_429("seven_day_sonnet")),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        );
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert_eq!(
            adapter.last_rate_limit_error_message().as_deref(),
            Some("You've hit your Sonnet limit")
        );

        // With a live `pro` snapshot → "weekly limit".
        let pro = make_adapter_with_subscriber(
            FakeTransport::always(resp_429("seven_day_sonnet")),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: false,
            },
        )
        .with_subscription(shared_slot(Some(
            traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("pro".to_string()),
                ..Default::default()
            },
        )));
        let _ = pro
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert_eq!(
            pro.last_rate_limit_error_message().as_deref(),
            Some("You've hit your weekly limit")
        );
    }

    /// Task 8 gate: enterprise subscriber 429 → retries through the full budget.
    ///
    /// Wire: `SubscriberState { is_subscriber: true, is_enterprise: true }` +
    /// a transport that returns 429 then 200 → the adapter retries and succeeds.
    #[tokio::test]
    async fn enterprise_subscriber_429_retries_in_adapter() {
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: {
                    let mut h = BTreeMap::new();
                    h.insert("retry-after".to_string(), "0".to_string());
                    h
                },
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: true,
            },
        );
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(
            result.is_ok(),
            "enterprise subscriber 429 must retry and succeed"
        );
        assert_eq!(
            transport.seen_count(),
            2,
            "should have made 2 requests (429 then 200)"
        );
    }

    /// #5 (main-loop parity): the adapter surfaces the most recent drive's
    /// budget-consuming retry count via `last_retry_count()`, so the cost path
    /// can record the REAL retry count (not the previous hardcoded `0`). A 429
    /// followed by a 200 is exactly one retry.
    #[tokio::test]
    async fn last_retry_count_reflects_429_retry() {
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: {
                    let mut h = BTreeMap::new();
                    h.insert("retry-after".to_string(), "0".to_string());
                    h
                },
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState {
                is_subscriber: true,
                is_enterprise: true,
            },
        );
        // No drive yet ⇒ zero.
        assert_eq!(adapter.last_retry_count(), 0);
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(result.is_ok(), "429→200 must succeed");
        // Exactly one budget-consuming retry was performed.
        assert_eq!(
            adapter.last_retry_count(),
            1,
            "one 429 retry ⇒ last_retry_count() == 1"
        );
    }

    /// Fix 1 end-to-end: a fake transport returns a 400 PTL envelope with counts;
    /// the resulting `LlmError::ContextOverflow` carries the parsed `token_gap`.
    ///
    /// This pins the adapter→codec→LlmError path: the adapter decodes the 400
    /// PTL response through the AnthropicMessagesCodec and surfaces the variant
    /// with the correct non-zero gap, so the turn-loop's `token_gap` binding is
    /// non-zero instead of the old `0` sentinel.
    #[tokio::test]
    async fn ptl_response_surfaces_context_overflow_with_gap() {
        // The 400 PTL envelope that Anthropic returns.
        let ptl_body = serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "prompt is too long: 210000 tokens > 200000 maximum"
            }
        });
        let transport = FakeTransport::always(ProviderResponse::json(400, ptl_body));
        let adapter = make_adapter(transport);

        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;

        match result {
            Err(LlmError::ContextOverflow { token_gap }) => {
                assert_eq!(
                    token_gap, 10_000,
                    "token_gap must be 210000 - 200000 = 10000; got {token_gap}"
                );
            }
            other => panic!("expected ContextOverflow {{ token_gap: 10000 }}, got {other:?}"),
        }
    }

    // ── 3c-T1: streaming 429 + retry-after header drives correct delay ────────

    /// Empty frame-stream for scripted streaming errors.
    struct EmptyFrames;
    impl crate::FrameStream for EmptyFrames {
        fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<crate::RawStreamFrame>, LlmError>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// A Transport that sequences execute responses AND can return scripted
    /// streaming (`open_stream`) responses.
    struct FakeStreamTransport {
        /// Sequence of `open_stream` results.
        stream_resps: Mutex<Vec<FakeStreamResp>>,
        stream_call_count: Mutex<usize>,
    }

    #[allow(dead_code)]
    enum FakeStreamResp {
        /// Streaming response with given status + headers + no frames.
        Status {
            status: u16,
            headers: BTreeMap<String, String>,
        },
        /// Terminal transport error (e.g. connection failure).
        Err(LlmError),
    }

    impl FakeStreamTransport {
        fn sequence(stream_resps: Vec<FakeStreamResp>) -> Arc<Self> {
            Arc::new(Self {
                stream_resps: Mutex::new(stream_resps),
                stream_call_count: Mutex::new(0),
            })
        }

        fn stream_call_count(&self) -> usize {
            *self.stream_call_count.lock().unwrap()
        }
    }

    impl Transport for FakeStreamTransport {
        fn execute<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "execute not scripted in FakeStreamTransport".to_string(),
                })
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            let mut count = self.stream_call_count.lock().unwrap();
            let idx = (*count).min(self.stream_resps.lock().unwrap().len().saturating_sub(1));
            *count += 1;
            drop(count);
            let resp = {
                let resps = self.stream_resps.lock().unwrap();
                match &resps[idx] {
                    FakeStreamResp::Status { status, headers } => Ok(StreamingResponse {
                        status: *status,
                        headers: headers.clone(),
                        frames: Box::new(EmptyFrames),
                    }),
                    FakeStreamResp::Err(e) => Err(e.clone()),
                }
            };
            Box::pin(async move { resp })
        }
    }

    /// 3c-T1 pin: a connect-phase 429 with `retry-after: 7` on the streaming
    /// path must drive a 7 s `RetryAfter` delay (not the 1 s fallback).
    ///
    /// We use `tokio::time::pause()` so the test completes instantly; the
    /// `drive_stream` loop sleeps via `tokio::time::sleep` which respects the
    /// paused clock.  After the first 429 the test advances time past 7 s and
    /// the second (200) attempt is served, confirming the delay was honoured.
    #[tokio::test(start_paused = true)]
    async fn streaming_429_with_retry_after_header_drives_7s_not_1s() {
        // Attempt 1: 429 with retry-after: 7.
        let mut headers_429 = BTreeMap::new();
        headers_429.insert("retry-after".to_string(), "7".to_string());

        // Attempt 2: 200 with an empty body (the codec will produce
        // StreamInterrupted on an empty frame-stream, but that is terminal and
        // proves two calls were made — what we care about).
        let stream_transport = FakeStreamTransport::sequence(vec![
            FakeStreamResp::Status {
                status: 429,
                headers: headers_429,
            },
            FakeStreamResp::Status {
                status: 200,
                headers: BTreeMap::new(),
            },
        ]);

        let adapter = make_adapter(Arc::clone(&stream_transport) as Arc<dyn Transport>);

        // Record the instant before calling drive_stream.
        let before = tokio::time::Instant::now();

        // drive_stream is private; call it through the StreamingApiClient trait.
        // The result will be an error (empty frame-stream on attempt 2) or Ok
        // depending on the codec — we only care that two open_stream calls were
        // made and that the elapsed time is ≥ 7 s (the retry-after delay).
        let _result = adapter
            .stream(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .await;

        let elapsed = before.elapsed();
        // The sleep was for exactly 7 s (retry-after value).  With time paused
        // the sleep advances the mock clock, so elapsed reports ≥ 7 s.
        assert!(
            elapsed >= std::time::Duration::from_secs(7),
            "retry-after:7 must drive a ≥7 s delay; elapsed={elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "delay must be close to 7 s (not jittered / not 1 s fallback); elapsed={elapsed:?}"
        );
        // Two open_stream calls: 429 then 200.
        assert_eq!(
            stream_transport.stream_call_count(),
            2,
            "must retry exactly once (429 → 200)"
        );
    }

    // ── M12: streaming idle watchdog (cc 2.1.196 default-on) ─────────────────

    /// A frame stream that NEVER produces a frame nor completes — models a
    /// hung connection so the idle watchdog must fire.
    struct HangingFrames;
    impl crate::FrameStream for HangingFrames {
        fn next_frame(
            &mut self,
        ) -> BoxFuture<'_, Result<Option<crate::RawStreamFrame>, LlmError>> {
            Box::pin(std::future::pending())
        }
    }

    /// Transport that opens a 200 stream whose frames hang forever.
    struct HangingStreamTransport;
    impl Transport for HangingStreamTransport {
        fn execute<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "execute not scripted".to_string(),
                })
            })
        }
        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                Ok(StreamingResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    frames: Box::new(HangingFrames),
                })
            })
        }
    }

    /// The watchdog is ON by default and aborts a stream that produces no
    /// event within the idle timeout, surfacing a detectable idle-timeout
    /// error. `start_paused` auto-advances the mock clock to the deadline.
    #[tokio::test(start_paused = true)]
    async fn streaming_idle_watchdog_aborts_hung_stream() {
        use futures::StreamExt;
        let adapter = make_adapter(Arc::new(HangingStreamTransport) as Arc<dyn Transport>)
            .with_stream_idle_timeout_override(Some(std::time::Duration::from_millis(50)));
        let mut stream = adapter
            .stream(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .await
            .expect("connect-phase 200 opens the stream");
        let first = stream.next().await.expect("the watchdog yields an error item");
        let err = first.expect_err("a hung stream must abort with an idle-timeout error");
        assert!(
            crate::model::stream_watchdog::is_stream_idle_timeout(&err),
            "expected a watchdog idle-timeout abort, got {err:?}"
        );
    }

    // ── Task 1: routing.fallback/retry adapter tests ──────────────────────────

    /// Build an adapter with routing overrides for per-model fallback and retry.
    fn make_adapter_with_routing(
        transport: Arc<dyn Transport>,
        fallback_overrides: std::collections::BTreeMap<String, Vec<String>>,
        settings_max_retries: Option<u32>,
        settings_backoff_ms: Option<u64>,
    ) -> ApiService {
        #[allow(deprecated)]
        std::env::set_var("ROUTING_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ROUTING_TEST_KEY".to_string(),
                    },
                    models: vec![
                        ModelProfile {
                            display_model: "claude-opus-4-6".to_string(),
                            request_model: "claude-opus-4-6".to_string(),
                            billing_model: "claude-opus-4-6".to_string(),
                            aliases: vec![],
                            description: None,
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                reasoning: true,
                                ..Default::default()
                            },
                        },
                        ModelProfile {
                            display_model: "claude-haiku-4-20250307".to_string(),
                            request_model: "claude-haiku-4-20250307".to_string(),
                            billing_model: "claude-haiku-4".to_string(),
                            aliases: vec![],
                            description: None,
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                reasoning: true,
                                ..Default::default()
                            },
                        },
                        ModelProfile {
                            display_model: "claude-sonnet-4-20250514".to_string(),
                            request_model: "claude-sonnet-4-20250514".to_string(),
                            billing_model: "claude-sonnet-4".to_string(),
                            aliases: vec!["claude".to_string()],
                            description: None,
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                reasoning: true,
                                ..Default::default()
                            },
                        },
                    ],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ApiService::new_with_routing(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
            None,
            fallback_overrides,
            settings_max_retries,
            settings_backoff_ms,
        )
    }

    fn routing_ok_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": "msg_routing",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
    }

    /// Per-model fallback entry wins over global fallback_model.
    ///
    /// Uses `claude-opus-4-6` as primary (is_non_custom_opus = true → allow_fallback
    /// activates naturally for a non-subscriber, no process env mutation needed).
    /// After 3 consecutive 529s the per-model fallback to haiku fires.
    #[tokio::test]
    async fn per_model_fallback_wins_over_global() {
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        // Per-model: opus → haiku (single-entry chain).
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        // No env var needed: claude-opus-4-6 is_non_custom_opus=true → allow_fallback=true
        // for non-subscriber (default SubscriberState).
        let mut adapter =
            make_adapter_with_routing(transport.clone(), fallback_overrides, None, Some(0));
        // Global fallback also points somewhere — per-model must win.
        adapter.fallback_model = Some("claude-sonnet-4-20250514".to_string());

        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None, // no explicit call-site fallback
                false,
                false,
            )
            .await;

        // Should succeed — 3 × 529 then haiku 200.
        assert!(
            result.is_ok(),
            "per-model fallback should route to haiku and succeed: {result:?}"
        );
        assert_eq!(
            transport.seen_count(),
            4,
            "expected 4 requests: 3 × 529 + 1 × 200"
        );
    }

    /// Global fallback is used when no per-model entry is present.
    #[tokio::test]
    async fn global_fallback_used_when_no_per_model_entry() {
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku2",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        // No per-model overrides.
        let mut adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            Some(0),
        );
        // Global fallback: opus → haiku.
        adapter.fallback_model = Some("claude-haiku-4-20250307".to_string());

        // claude-opus-4-6 is_non_custom_opus=true → allow_fallback=true for non-subscriber.
        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                false,
                false,
            )
            .await;

        assert!(result.is_ok(), "global fallback should work: {result:?}");
        assert_eq!(transport.seen_count(), 4);
    }

    /// Per-model fallback fires even when the request uses an ALIAS of the
    /// primary model.
    ///
    /// `fallback_overrides` keys are keyed by the display model; if the request
    /// arrives as an alias (e.g. `"claude"` instead of `"claude-sonnet-4-20250514"`)
    /// the lookup must normalize via `alias_to_display` before probing the map.
    ///
    /// RED on the old code (raw model probe skips the per-model entry when an
    /// alias is used); GREEN after the alias normalization fix.
    #[tokio::test]
    async fn per_model_fallback_fires_via_alias() {
        // Three 529s then a success on the fallback (haiku).
        let overloaded_json = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, routing_ok_response_json())),
        ]);

        // Per-model fallback: display "claude-sonnet-4-20250514" → "claude-haiku-4-20250307"
        // (single-entry chain).
        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-sonnet-4-20250514".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        let adapter =
            make_adapter_with_routing(transport.clone(), fallback_overrides, None, Some(0));

        // Request via ALIAS — the alias_to_display map must normalize this to
        // "claude-sonnet-4-20250514" before the fallback_overrides lookup.
        let result = adapter
            .messages_create_with_fallback(
                "claude", // alias of "claude-sonnet-4-20250514"
                None,
                Some("sys"),
                Vec::new(),
                Vec::new(),
                None, // no explicit call-site fallback (per-model must activate)
                false,
                false,
            )
            .await;

        assert!(
            result.is_ok(),
            "alias-keyed per-model fallback should fire and succeed; got: {result:?}"
        );
        // 3 primary 529s + 1 fallback success = 4 transport calls.
        assert_eq!(
            transport.seen_count(),
            4,
            "expected 3 failing primary calls + 1 successful fallback call"
        );
    }

    // ── Task 5: fallback chain walk tests ────────────────────────────────────

    /// 2-entry chain: primary → chain[0] → chain[1] when all 529s.
    ///
    /// Scripted: 3×529 on primary, 3×529 on chain[0], then 200 on chain[1].
    /// Asserts the model sequence primary→c0→c1 via captured request bodies.
    #[tokio::test]
    async fn two_entry_chain_walks_both_entries() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let c1_ok = serde_json::json!({
            "id": "msg_c1",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "c1 ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        // 3 × 529 on primary (claude-opus-4-6)
        // 3 × 529 on chain[0] (claude-sonnet-4-20250514)
        // 1 × 200 on chain[1] (claude-haiku-4-20250307)
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, c1_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        // 2-entry chain: opus-4-6 → sonnet-4-20250514 → haiku-4-20250307
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-haiku-4-20250307".to_string(),
            ],
        );

        let adapter =
            make_adapter_with_routing(transport.clone(), fallback_overrides, None, Some(0));

        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                false,
                false,
            )
            .await;

        assert!(
            result.is_ok(),
            "chain walk must succeed on chain[1]: {result:?}"
        );
        assert_eq!(
            transport.seen_count(),
            7,
            "3 primary + 3 chain[0] + 1 chain[1]"
        );

        // Assert the model sequence: first 3 requests use primary, next 3 use chain[0],
        // last 1 uses chain[1].
        let primary = "claude-opus-4-6";
        let c0 = "claude-sonnet-4-20250514";
        let c1 = "claude-haiku-4-20250307";
        for i in 0..3 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some(primary),
                "request {i} must use primary model"
            );
        }
        for i in 3..6 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some(c0),
                "request {i} must use chain[0]"
            );
        }
        assert_eq!(
            transport.seen_body_model(6).as_deref(),
            Some(c1),
            "request 6 must use chain[1]"
        );
    }

    /// Chain exhaustion: when all entries are overloaded, the call is terminal.
    ///
    /// Single-entry chain: primary 3×529 → chain[0] persistent 529 → terminal error.
    #[tokio::test]
    async fn chain_exhausted_gives_terminal_error() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        // primary: 3 × 529 → Fallback
        // chain[0]: persistent 529 → RepeatedOverloaded (is_external=true in make_adapter_with_routing)
        let transport = FakeTransport::always(ProviderResponse::json(529, overloaded));

        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        let adapter =
            make_adapter_with_routing(transport.clone(), fallback_overrides, None, Some(0));

        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                false,
                false,
            )
            .await;

        assert!(
            result.is_err(),
            "exhausted chain must produce terminal error"
        );
        // The error must be Overloaded (either repeated=true from external path or
        // plain Overloaded — either variant indicates the chain was walked and terminated).
        assert!(
            matches!(result.unwrap_err(), LlmError::Overloaded { .. }),
            "terminal error must be LlmError::Overloaded"
        );
    }

    /// Single-entry chain behaves like batch-1 (exactly one fallback hop).
    ///
    /// Uses the existing `per_model_fallback_wins_over_global` scenario but
    /// verifies via `seen_body_model` that the model sequence is correct.
    #[tokio::test]
    async fn single_entry_chain_behaves_like_batch1() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );
        let adapter =
            make_adapter_with_routing(transport.clone(), fallback_overrides, None, Some(0));

        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                false,
                false,
            )
            .await;

        assert!(
            result.is_ok(),
            "single-entry chain must succeed: {result:?}"
        );
        assert_eq!(transport.seen_count(), 4, "3 primary 529s + 1 fallback 200");
        // First 3 requests: primary model.
        for i in 0..3 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some("claude-opus-4-6"),
                "request {i} must use primary"
            );
        }
        // 4th request: fallback model.
        assert_eq!(
            transport.seen_body_model(3).as_deref(),
            Some("claude-haiku-4-20250307"),
            "request 3 must use chain[0]"
        );
    }

    /// Global fallback_model still works when no per-model chain is configured.
    ///
    /// The global `fallback_model` is wrapped into a single-entry chain and walks
    /// the same code path; this test guards that wiring.
    #[tokio::test]
    async fn global_fallback_model_works_without_chain_entry() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let sonnet_ok = serde_json::json!({
            "id": "msg_sonnet",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, sonnet_ok)),
        ]);

        let mut adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            Some(0),
        );
        // Set global fallback only (no per-model chain).
        adapter.fallback_model = Some("claude-sonnet-4-20250514".to_string());

        let result = adapter
            .messages_create_with_fallback(
                "claude-opus-4-6",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                false,
                false,
            )
            .await;

        assert!(result.is_ok(), "global fallback must work: {result:?}");
        assert_eq!(transport.seen_count(), 4);
        // Request 3 must use the global fallback model.
        assert_eq!(
            transport.seen_body_model(3).as_deref(),
            Some("claude-sonnet-4-20250514"),
            "request 3 must use global fallback model"
        );
    }

    /// `settings_max_retries=2` causes terminal after 3 executions (not 11).
    ///
    /// The adapter reads `LINGXI_MAX_RETRIES` from the process env in
    /// `messages_create`.  To avoid interference with parallel tests we verify
    /// via the retry.rs layer (which is injected, not process-env) rather than
    /// through the adapter's env path.  The adapter's `settings_max_retries`
    /// field is directly observable via the resolve_retry_control_with_settings
    /// call: when env var is absent it uses `settings_max_retries` as the
    /// effective limit.  We temporarily clear the env var then restore it.
    #[tokio::test]
    async fn settings_max_retries_beats_default() {
        // We can test the settings path directly: when the env var is absent
        // the settings_max_retries field applies.  We control LINGXI_MAX_RETRIES
        // for the duration of this test — accept minor isolation risk since the
        // pre-existing test suite also does this.
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(ProviderResponse::json(
            500,
            serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "internal"}}),
        ))]);

        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            Some(2), // settings says max 2 retries
            None,
        );

        // Temporarily unset LINGXI_MAX_RETRIES so settings value wins.
        let saved = std::env::var("LINGXI_MAX_RETRIES").ok();
        #[allow(deprecated)]
        std::env::remove_var("LINGXI_MAX_RETRIES");

        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;

        // Restore.
        if let Some(v) = saved {
            #[allow(deprecated)]
            std::env::set_var("LINGXI_MAX_RETRIES", v);
        }

        assert!(result.is_err(), "must fail after exhausting budget");
        // max_retries=2 → 3 executions (2 sleeps + 1 final).
        assert_eq!(
            transport.seen_count(),
            3,
            "settings_max_retries=2 should give 3 executions (2 retries + 1 initial)"
        );
    }

    /// Env `LINGXI_MAX_RETRIES` beats `settings_max_retries`.
    ///
    /// Proven via `resolve_retry_control_with_settings` unit tests in retry.rs;
    /// this adapter-level test verifies the wiring by injecting through
    /// `ResolveRetryEnv` directly rather than mutating the process env.
    ///
    /// We use `crate::model::retry::resolve_retry_control_with_settings` to
    /// build the expected `RetryControl` and compare `max_retries`.
    #[test]
    fn env_max_retries_beats_settings_via_resolve() {
        use crate::model::retry::{
            resolve_retry_control_with_settings, ResolveRetryEnv, DEFAULT_MAX_RETRIES,
        };

        // env=Some("1") + settings=Some(8) → max_retries=1 (env wins).
        let env_with_1 = ResolveRetryEnv {
            max_retries: Some("1".to_string()),
            ..ResolveRetryEnv::default()
        };
        let ctl = resolve_retry_control_with_settings(
            "claude-sonnet-4-20250514",
            None,
            false,
            &env_with_1,
            Some(8),
        );
        assert_eq!(ctl.max_retries, 1, "env(1) must beat settings(8)");

        // env=None + settings=Some(7) → max_retries=7 (settings wins).
        let env_absent = ResolveRetryEnv::default();
        let ctl2 = resolve_retry_control_with_settings(
            "claude-sonnet-4-20250514",
            None,
            false,
            &env_absent,
            Some(7),
        );
        assert_eq!(ctl2.max_retries, 7, "settings(7) must beat default(10)");

        // env=None + settings=None → DEFAULT.
        let ctl3 = resolve_retry_control_with_settings(
            "claude-sonnet-4-20250514",
            None,
            false,
            &env_absent,
            None,
        );
        assert_eq!(ctl3.max_retries, DEFAULT_MAX_RETRIES);
    }

    /// `settings_backoff_ms=1000` doubles the jitter ladder base.
    ///
    /// With `backoff_ms=1000` and time paused, we verify the first retry delay
    /// is ≥ 800 ms (= 1000 × 0.8 lower-jitter-bound).  Without the setting the
    /// base would be 500 ms (lower bound 400 ms) — so 800 ms is above the
    /// un-scaled upper bound (600 ms) which proves scaling is active.
    #[tokio::test(start_paused = true)]
    async fn backoff_ms_scales_jitter_base() {
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(
                500,
                serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "err"}}),
            )),
            FakeResponse::Ok(ProviderResponse::json(200, routing_ok_response_json())),
        ]);

        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            Some(1000), // backoff_ms = 1000 → first rung 1000, additive jitter [1000, 1250)
        );

        let before = tokio::time::Instant::now();
        let result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        let elapsed = before.elapsed();

        assert!(result.is_ok(), "should succeed after retry: {result:?}");
        // Additive jitter (binary `sle`: base + rand(0,0.25)·base) → base 1000
        // gives [1000, 1250). Default base 500 → [500, 625), upper 625 < 800.
        // So ≥ 800 ms proves the 1000 ms base (not the default 500) is in effect.
        assert!(
            elapsed >= std::time::Duration::from_millis(800),
            "backoff_ms=1000 should produce ≥ 800 ms delay; elapsed={elapsed:?}"
        );
        // Must be < 1250 ms (upper additive-jitter bound: 1000 + 0.25·1000).
        assert!(
            elapsed < std::time::Duration::from_millis(1250),
            "backoff_ms=1000 delay should be < 1250 ms; elapsed={elapsed:?}"
        );
    }

    // ── T3 Step 2: 2xx rate-limit header feed ────────────────────────────────

    /// A 2xx response with `anthropic-ratelimit-unified-overage-status: rejected`
    /// must be stored in `last_rate_limit_info` and trigger a `tracing::warn!`.
    ///
    /// We only assert the data is stored; the warn fires on a live-log subscriber
    /// which we do not attach in tests — the absence of a panic is the assertion.
    #[tokio::test]
    async fn rate_limit_info_stored_from_2xx_response() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim".to_string(),
            "five_hour".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-status".to_string(),
            "rejected".to_string(),
        );

        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers,
            body_json: ok_response_json(),
            request_id: None,
        });
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");

        let info = adapter
            .last_rate_limit_info()
            .expect("last_rate_limit_info must be Some after a 2xx with unified headers");
        assert_eq!(
            info.rate_limit_type.as_deref(),
            Some("five_hour"),
            "rate_limit_type must be five_hour"
        );
        assert_eq!(
            info.overage_status.as_deref(),
            Some("rejected"),
            "overage_status must be rejected"
        );
    }

    /// Without unified rate-limit headers the cached info stays `None`.
    #[tokio::test]
    async fn rate_limit_info_none_when_no_unified_headers() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");

        // No unified headers → None still.
        assert!(
            adapter.last_rate_limit_info().is_none(),
            "last_rate_limit_info must be None when no unified headers are present"
        );
    }

    // ── count_tokens (inherent ApiService) ──────────────────────────────────────

    /// The adapter override drives the real `/v1/messages/count_tokens` endpoint
    /// on an Anthropic route: it sends one request to the count_tokens URL
    /// carrying the `count_tokens` beta header and returns the decoded
    /// `input_tokens` from the response.
    #[tokio::test]
    async fn count_tokens_through_adapter_hits_anthropic_endpoint_with_beta() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({ "input_tokens": 2095 }),
            request_id: None,
        });
        let adapter = make_adapter(transport.clone());

        let count = adapter
            .count_tokens(
                "claude-sonnet-4-20250514",
                None,
                Some("you are helpful"),
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("count_tokens ok");

        assert_eq!(
            count, 2095,
            "decoded input_tokens from the count_tokens response"
        );
        assert_eq!(
            transport.seen_count(),
            1,
            "exactly one count_tokens request sent"
        );

        let url = transport.seen.lock().unwrap()[0].url.clone();
        assert!(
            url.ends_with("/v1/messages/count_tokens"),
            "must route to the count_tokens endpoint; url={url}"
        );
        let expected_beta = crate::model::betas::assemble_beta_header(
            crate::model::betas::Provider::Anthropic,
            crate::model::betas::Endpoint::CountTokens,
            &crate::model::betas::BetaContext::for_model("claude-sonnet-4-20250514"),
        );
        assert_eq!(
            transport
                .seen_headers(0)
                .get("anthropic-beta")
                .map(String::as_str),
            Some(expected_beta.as_str()),
            "anthropic-beta header must equal assemble_beta_header(Anthropic, CountTokens)"
        );
    }

    // ── T3 Step 3: stream telemetry twins ────────────────────────────────────

    /// A `FrameStream` that yields scripted raw SSE frames (encoded as
    /// JSON byte sequences) then terminates. Used to drive the stream decoder
    /// with a controlled event sequence so the `drive_stream` unfold
    /// emits telemetry at the right points.
    struct ScriptedFrames {
        frames: Vec<Vec<u8>>,
        idx: usize,
    }

    impl ScriptedFrames {
        fn new(frames: Vec<Vec<u8>>) -> Self {
            Self { frames, idx: 0 }
        }
    }

    impl crate::FrameStream for ScriptedFrames {
        fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<crate::RawStreamFrame>, LlmError>> {
            let result = if self.idx < self.frames.len() {
                let bytes = self.frames[self.idx].clone();
                self.idx += 1;
                Ok(Some(crate::RawStreamFrame::new(bytes)))
            } else {
                Ok(None)
            };
            Box::pin(async move { result })
        }
    }

    /// A transport that delivers a fixed successful stream ending in `message_stop`.
    ///
    /// Builds valid Anthropic SSE frames (as raw JSON lines) so the `AnthropicMessages`
    /// codec can decode them. The stream ends with `message_stop` which is the
    /// terminal event → should trigger `emit_succeeded` once.
    struct ScriptedStreamTransport {
        frames: Vec<Vec<u8>>,
        headers: BTreeMap<String, String>,
        status: u16,
    }

    impl ScriptedStreamTransport {
        /// Build a transport that delivers a minimal valid anthropic stream:
        /// `message_start` → `message_delta(stop_reason=end_turn)` → `message_stop`.
        fn anthropic_success() -> Arc<Self> {
            let frames = vec![
                br#"{"type":"message_start","message":{"id":"msg_t","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#.to_vec(),
                br#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#.to_vec(),
                br#"{"type":"message_stop"}"#.to_vec(),
            ];
            Arc::new(Self {
                frames,
                headers: BTreeMap::new(),
                status: 200,
            })
        }

        /// Build a transport that delivers a single malformed frame → decoder error.
        fn malformed_frame() -> Arc<Self> {
            let frames = vec![b"not-valid-json".to_vec()];
            Arc::new(Self {
                frames,
                headers: BTreeMap::new(),
                status: 200,
            })
        }
    }

    impl Transport for ScriptedStreamTransport {
        fn execute<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "execute not used in ScriptedStreamTransport".into(),
                })
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            let frames: Vec<Vec<u8>> = self.frames.clone();
            let headers = self.headers.clone();
            let status = self.status;
            Box::pin(async move {
                Ok(StreamingResponse {
                    status,
                    headers,
                    frames: Box::new(ScriptedFrames::new(frames)),
                })
            })
        }
    }

    /// Build a streaming adapter with an attached analytics bus.
    async fn make_stream_adapter_with_bus(
        transport: Arc<dyn Transport>,
    ) -> (ApiService, Arc<::telemetry::InMemorySink>) {
        use ::telemetry::{AnalyticsBus, InMemorySink};

        #[allow(deprecated)]
        std::env::set_var("STREAM_TELEM_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "STREAM_TELEM_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec![],
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );

        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::new());
        bus.attach_sink(sink.clone()).await;

        let adapter = ApiService::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            Some(bus),
            None,
        );
        (adapter, sink)
    }

    /// Stream telemetry twin — succeed path:
    /// a clean stream (message_stop at the end) must emit:
    /// 1. `tengu_api_request_started` (stream=true)
    /// 2. `tengu_api_request_succeeded` (from the unfold's terminal-event arm)
    #[tokio::test]
    async fn stream_emit_succeeded_fires_on_message_stop() {
        use ::telemetry::AnalyticsValue;
        use futures::StreamExt as _;

        let transport = ScriptedStreamTransport::anthropic_success();
        let (adapter, sink) = make_stream_adapter_with_bus(transport).await;

        let mut stream = adapter
            .stream(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .await
            .expect("stream open ok");

        // Drain all events.
        while stream.next().await.is_some() {}

        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();

        assert!(
            names.contains(&"tengu_api_request_started"),
            "started must fire; got {names:?}"
        );
        assert!(
            names.contains(&"tengu_api_request_succeeded"),
            "succeeded must fire on message_stop; got {names:?}"
        );

        // Must NOT fire multiple times.
        let succeeded_count = names
            .iter()
            .filter(|&&n| n == "tengu_api_request_succeeded")
            .count();
        assert_eq!(
            succeeded_count, 1,
            "succeeded must fire exactly once; got {succeeded_count}"
        );

        // Verify stream=true on the started event.
        let started = events
            .iter()
            .find(|e| e.name == "tengu_api_request_started")
            .unwrap();
        assert!(
            matches!(&started.metadata["stream"], AnalyticsValue::Bool(true)),
            "stream must be true on the started event"
        );
    }

    /// Stream telemetry twin — fail path:
    /// a stream that produces a decode error must emit `tengu_api_request_failed`
    /// and NOT emit `tengu_api_request_succeeded`.
    #[tokio::test]
    async fn stream_emit_failed_fires_on_decode_error() {
        use futures::StreamExt as _;

        let transport = ScriptedStreamTransport::malformed_frame();
        let (adapter, sink) = make_stream_adapter_with_bus(transport).await;

        let stream_result = adapter
            .stream(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .await;

        // May error at open or during drain.
        if let Ok(mut stream) = stream_result {
            while let Some(item) = stream.next().await {
                // consume until error or end
                let _ = item;
            }
        }

        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();

        // failed must fire (decode error or stream-interrupted)
        // succeeded must NOT fire
        assert!(
            names.contains(&"tengu_api_request_failed"),
            "failed must fire on decode error; got {names:?}"
        );
        assert!(
            !names.contains(&"tengu_api_request_succeeded"),
            "succeeded must NOT fire on error; got {names:?}"
        );
    }

    // ── AWS auth refresh trigger (2.1.198 V_c/G_c/s_f + Ygf) ─────────────────

    /// Counting stand-in for the `ZBd` driver — the drive loop only needs the
    /// object-safe `AwsAuthRefresh` seam.
    #[derive(Debug, Default)]
    struct CountingAwsRefresh {
        calls: std::sync::atomic::AtomicU32,
    }

    impl crate::aws_auth::AwsAuthRefresh for CountingAwsRefresh {
        fn refresh(&self) -> BoxFuture<'_, bool> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { true })
        }
    }

    /// Bedrock-provider adapter with the AWS auth-refresh seam attached.
    fn make_bedrock_adapter_with_aws(
        transport: Arc<dyn Transport>,
        aws: Arc<CountingAwsRefresh>,
    ) -> ApiService {
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::BedrockClaude,
                    profile_name: "bedrock".to_string(),
                    base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
                    protocol: ProtocolFamily::BedrockClaude,
                    // Auth None so prepare() succeeds without SigV4 material —
                    // the refresh trigger keys off the RESPONSE error + the
                    // route's provider_id, not the auth strategy.
                    auth: AuthStrategy::None,
                    credential: CredentialConfig::None,
                    models: vec![ModelProfile {
                        display_model: "model".to_string(),
                        request_model: "model".to_string(),
                        billing_model: "model".to_string(),
                        aliases: Vec::new(),
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ApiService::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
        .with_aws_auth(aws)
    }

    /// 401 `authentication_error` body (decodes to `LlmError::Authentication`
    /// through the Bedrock codec's Anthropic-shape error decode) — the
    /// expired-STS terminal the 2.1.198 trigger classifies via `V_c`.
    fn expired_sts_response() -> ProviderResponse {
        ProviderResponse::json(
            401,
            serde_json::json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "The security token included in the request is expired"
                }
            }),
        )
    }

    #[tokio::test]
    async fn aws_auth_error_refreshes_and_retries_once() {
        // 401 (expired STS) then 200: the hook must run ZBd once and the retry
        // must succeed — the 2.1.198 behavior replacing the "/login" dead end.
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(expired_sts_response()),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let aws = Arc::new(CountingAwsRefresh::default());
        let adapter = make_bedrock_adapter_with_aws(transport.clone(), aws.clone());

        let out = adapter
            .messages_create("model", None, None, Vec::new(), Vec::new())
            .await;
        assert!(out.is_ok(), "retry after refresh must succeed: {out:?}");
        assert_eq!(
            aws.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one refresh"
        );
        assert_eq!(transport.seen_count(), 2, "original + one retry");
    }

    #[tokio::test]
    async fn aws_auth_retries_bounded_at_ygf_two() {
        // Every attempt 401s: the hook may fire at most AWS_AUTH_MAX_ATTEMPTS
        // (Ygf=2) times, then the error goes terminal (the binary's
        // `api_request_aws_auth_exhausted` throw).
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(expired_sts_response())]);
        let aws = Arc::new(CountingAwsRefresh::default());
        let adapter = make_bedrock_adapter_with_aws(transport.clone(), aws.clone());

        let out = adapter
            .messages_create("model", None, None, Vec::new(), Vec::new())
            .await;
        assert!(
            matches!(out, Err(LlmError::Authentication)),
            "exhausted refresh budget surfaces the auth error: {out:?}"
        );
        assert_eq!(
            aws.calls.load(std::sync::atomic::Ordering::SeqCst),
            crate::aws_auth::AWS_AUTH_MAX_ATTEMPTS,
            "refresh bounded at Ygf=2"
        );
        assert_eq!(transport.seen_count(), 3, "initial attempt + 2 refresh retries");
    }

    #[tokio::test]
    async fn non_aws_provider_never_triggers_refresh() {
        // Provider gate (multi-provider structure is sacrosanct): the SAME 401
        // on the Anthropic first-party provider must NOT touch the refresher.
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(expired_sts_response())]);
        let aws = Arc::new(CountingAwsRefresh::default());
        let adapter = make_adapter(transport.clone());
        let adapter = adapter.with_aws_auth(aws.clone());

        let out = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;
        assert!(matches!(out, Err(LlmError::Authentication)));
        assert_eq!(
            aws.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "refresh must never run for a non-AWS provider"
        );
        assert_eq!(transport.seen_count(), 1, "401 stays terminal, no retry");
    }
}
