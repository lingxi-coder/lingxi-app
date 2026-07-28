use llm_client::client::DefaultLlmClient;
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, LlmError, LlmRequest, ModelProfile,
    PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, ProviderStreamTransport,
    ReasoningConfig, ResponseFormat,
};

#[tokio::test]
async fn client_builds_routes_from_config_and_lists_models() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
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
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    assert_eq!(client.available_models().len(), 1);
    assert!(client.prepare(&LlmRequest::new("fast")).await.is_ok());
}

#[tokio::test]
async fn prepare_returns_route_identity_and_encodes_resolved_request_model() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
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
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    let prepared = client.prepare(&LlmRequest::new("fast")).await.unwrap();

    assert_eq!(prepared.route.resolved_route.profile_name, "openai");
    assert_eq!(
        prepared.provider_request.url,
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(prepared.provider_request.body_json["model"], "gpt-4o");
}

fn anthropic_fast_profile(profile_name: &str, base_url: &str) -> ProviderProfile {
    ProviderProfile {
        provider_id: ProviderId::AnthropicFirstParty,
        profile_name: profile_name.to_string(),
        base_url: base_url.to_string(),
        protocol: ProtocolFamily::AnthropicMessages,
        auth: AuthStrategy::ApiKey,
        credential: CredentialConfig::None,
        models: vec![ModelProfile {
            display_model: "claude-opus-5".to_string(),
            request_model: "claude-opus-5".to_string(),
            billing_model: "claude-opus-5".to_string(),
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
    }
}

#[tokio::test]
async fn fast_speed_survives_only_on_the_builtin_anthropic_route() {
    let direct = DefaultLlmClient::from_config(ClientConfig {
        providers: vec![anthropic_fast_profile(
            "anthropic",
            "https://api.anthropic.com",
        )],
    })
    .unwrap();
    let mut request = LlmRequest::new("claude-opus-5");
    request.speed = Some("fast".to_string());
    let prepared = direct.prepare(&request).await.unwrap();
    assert_eq!(prepared.provider_request.body_json["speed"], "fast");

    let custom = DefaultLlmClient::from_config(ClientConfig {
        providers: vec![anthropic_fast_profile(
            "anthropic-compatible",
            "https://gateway.example",
        )],
    })
    .unwrap();
    let prepared = custom.prepare(&request).await.unwrap();
    assert!(
        prepared.provider_request.body_json.get("speed").is_none(),
        "custom Anthropic-compatible routes must not inherit first-party fast mode"
    );
}

#[tokio::test]
async fn fast_speed_is_removed_for_models_without_the_registry_capability() {
    let mut profile = anthropic_fast_profile("anthropic", "https://api.anthropic.com");
    profile.models[0].display_model = "claude-sonnet-5".to_string();
    profile.models[0].request_model = "claude-sonnet-5".to_string();
    profile.models[0].billing_model = "claude-sonnet-5".to_string();
    let client = DefaultLlmClient::from_config(ClientConfig {
        providers: vec![profile],
    })
    .unwrap();
    let mut request = LlmRequest::new("claude-sonnet-5");
    request.speed = Some("fast".to_string());
    let prepared = client.prepare(&request).await.unwrap();
    assert!(prepared.provider_request.body_json.get("speed").is_none());
}

#[tokio::test]
async fn github_copilot_gpt5_and_codex_route_to_responses_endpoint() {
    // GitHub Copilot serves GPT-5.x / codex models ONLY via `/responses`, but
    // the provider profile declares one OpenAiChat protocol. The per-model
    // override must send those models to `…/responses` (same host) while older
    // models keep `/chat/completions`. Regression for the live-QA
    // "model gpt-5.5 is not accessible via the /chat/completions endpoint".
    let model = |display: &str, id: &str| ModelProfile {
        display_model: display.to_string(),
        request_model: id.to_string(),
        billing_model: id.to_string(),
        aliases: vec![],
        description: None,
        capabilities: Capabilities {
            streaming: true,
            tools: true,
            ..Default::default()
        },
    };
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "github-copilot".to_string(),
            },
            profile_name: "github-copilot".to_string(),
            base_url: "https://api.githubcopilot.com".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![
                model("GPT-5.5", "gpt-5.5"),
                model("GPT-5 Codex", "gpt-5-codex"),
                model("GPT-4o", "gpt-4o"),
            ],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
        }],
    };
    let client = DefaultLlmClient::from_config(config).unwrap();

    for responses_model in ["gpt-5.5", "gpt-5-codex"] {
        let prepared = client
            .prepare(&LlmRequest::new(responses_model))
            .await
            .unwrap();
        assert_eq!(
            prepared.route.protocol,
            ProtocolFamily::OpenAiResponses,
            "{responses_model} must route via Responses"
        );
        assert_eq!(
            prepared.provider_request.url, "https://api.githubcopilot.com/responses",
            "{responses_model} must hit /responses"
        );
    }

    // Older models keep the chat/completions endpoint.
    let four = client.prepare(&LlmRequest::new("gpt-4o")).await.unwrap();
    assert_eq!(four.route.protocol, ProtocolFamily::OpenAiChat);
    assert_eq!(
        four.provider_request.url,
        "https://api.githubcopilot.com/chat/completions"
    );
}

#[tokio::test]
async fn non_copilot_openai_chat_provider_is_never_overridden() {
    // The override is scoped to the `github-copilot` profile: a GPT-5 id served
    // by some other OpenAiChat gateway (e.g. openrouter) keeps /chat/completions.
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            profile_name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-5.5".to_string(),
                request_model: "gpt-5.5".to_string(),
                billing_model: "gpt-5.5".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
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
    };
    let client = DefaultLlmClient::from_config(config).unwrap();
    let prepared = client.prepare(&LlmRequest::new("gpt-5.5")).await.unwrap();
    assert_eq!(prepared.route.protocol, ProtocolFamily::OpenAiChat);
    assert_eq!(
        prepared.provider_request.url,
        "https://openrouter.ai/api/v1/chat/completions"
    );
}

#[tokio::test]
async fn reasoning_is_dropped_for_a_non_reasoning_model_not_hard_failed() {
    // Live-QA regression: with the session thinking config on, switching to a
    // model whose catalog capabilities advertise NO reasoning (e.g. an OpenRouter
    // free coder model) carried `request.reasoning`, and prepare hard-failed with
    // "unsupported capability: reasoning". Reasoning is a best-effort enhancement:
    // it must silently DROP for such a model, not break the turn.
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            profile_name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "Qwen3 Coder (free)".to_string(),
                request_model: "qwen/qwen3-coder:free".to_string(),
                billing_model: "qwen/qwen3-coder:free".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    reasoning: false,
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
    };
    let client = DefaultLlmClient::from_config(config).unwrap();
    let mut req = LlmRequest::new("qwen/qwen3-coder:free");
    req.reasoning = Some(ReasoningConfig::Enabled {
        budget_tokens: 2048,
    });

    // Must PREPARE OK (previously errored with UnsupportedCapability { reasoning }).
    let prepared = client
        .prepare(&req)
        .await
        .expect("reasoning must be dropped, not hard-fail the request");
    // And the reasoning must not leak onto the wire body.
    let body = prepared.provider_request.body_json.to_string();
    assert!(
        !body.contains("reasoning") && !body.contains("thinking"),
        "reasoning must be stripped from the wire body: {body}"
    );

    // The MID-CONVERSATION case: history carries Reasoning / RedactedThinking
    // blocks from an earlier thinking model. Those blocks must be stripped too —
    // `validate_capabilities` rejects them independently of the top-level field,
    // so a switch after any thinking must not hard-fail.
    let mut resumed = LlmRequest::new("qwen/qwen3-coder:free");
    resumed.messages = vec![
        llm_client::Message {
            role: "assistant".to_string(),
            content: vec![
                llm_client::ContentBlock::Reasoning {
                    text: "let me think".to_string(),
                    signature: None,
                },
                llm_client::ContentBlock::Text {
                    text: "the answer is 42".to_string(),
                    cache_control: None,
                },
            ],
        },
        llm_client::Message {
            role: "user".to_string(),
            content: vec![llm_client::ContentBlock::Text {
                text: "thanks".to_string(),
                cache_control: None,
            }],
        },
    ];
    // No top-level reasoning field this turn — only history blocks.
    let prepared = client
        .prepare(&resumed)
        .await
        .expect("history reasoning blocks must be stripped, not hard-fail");
    let body = prepared.provider_request.body_json.to_string();
    assert!(
        !body.contains("let me think"),
        "history reasoning block must be stripped from the wire body: {body}"
    );
    assert!(
        body.contains("the answer is 42"),
        "non-reasoning content must survive: {body}"
    );
}

#[tokio::test]
async fn vision_image_blocks_dropped_for_non_vision_model_not_hard_failed() {
    // Regression: an image pasted into a conversation persists in session
    // history, so switching to a model that doesn't advertise vision must
    // silently drop Image/ImageUrl blocks from history — not break every
    // subsequent turn with "unsupported capability: vision".
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            profile_name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "Qwen3 Coder (free)".to_string(),
                request_model: "qwen/qwen3-coder:free".to_string(),
                billing_model: "qwen/qwen3-coder:free".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    vision: false,
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
    };
    let client = DefaultLlmClient::from_config(config).unwrap();

    // The common "paste image into input view" scenario.
    let mut image_request = LlmRequest::new("qwen/qwen3-coder:free");
    image_request.messages = vec![llm_client::Message {
        role: "user".to_string(),
        content: vec![
            llm_client::ContentBlock::Text {
                text: "describe this image".to_string(),
                cache_control: None,
            },
            llm_client::ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: vec![1, 2, 3],
            },
        ],
    }];
    let prepared = client
        .prepare(&image_request)
        .await
        .expect("image block must be silently dropped, not hard-fail the request");
    let body = prepared.provider_request.body_json.to_string();
    assert!(
        !body.contains("\"type\":\"image\""),
        "image block must be stripped from the wire body: {body}"
    );

    // The MID-CONVERSATION case: history carries Image blocks from an earlier
    // vision-capable model. All subsequent turns (even text-only) must not
    // hard-fail.
    let mut history_request = LlmRequest::new("qwen/qwen3-coder:free");
    history_request.messages = vec![
        llm_client::Message {
            role: "user".to_string(),
            content: vec![
                llm_client::ContentBlock::Text {
                    text: "look at this".to_string(),
                    cache_control: None,
                },
                llm_client::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    bytes: vec![1, 2, 3],
                },
            ],
        },
        llm_client::Message {
            role: "assistant".to_string(),
            content: vec![llm_client::ContentBlock::Text {
                text: "i see a photo".to_string(),
                cache_control: None,
            }],
        },
        llm_client::Message {
            role: "user".to_string(),
            content: vec![llm_client::ContentBlock::Text {
                text: "ok what else".to_string(),
                cache_control: None,
            }],
        },
    ];
    // Text-only follow-up: no image in the current turn, but history
    // carries image blocks. Must silently strip and proceed.
    let prepared = client
        .prepare(&history_request)
        .await
        .expect("history image blocks must be stripped, not hard-fail");
    let body = prepared.provider_request.body_json.to_string();
    assert!(
        !body.contains("\"type\":\"image\""),
        "history image block must be stripped from the wire body: {body}"
    );
    assert!(
        body.contains("ok what else"),
        "text follow-up must survive: {body}"
    );
}

#[test]
fn duplicate_profile_names_are_rejected_during_client_construction() {
    let config = ClientConfig {
        providers: vec![
            ProviderProfile {
                provider_id: ProviderId::OpenAI,
                profile_name: "shared".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                protocol: ProtocolFamily::OpenAiChat,
                auth: AuthStrategy::Bearer,
                credential: CredentialConfig::None,
                models: vec![ModelProfile {
                    display_model: "GPT-4o".to_string(),
                    request_model: "gpt-4o".to_string(),
                    billing_model: "gpt-4o".to_string(),
                    aliases: vec!["fast".to_string()],
                    description: None,
                    capabilities: Capabilities {
                        streaming: true,
                        tools: true,
                        ..Default::default()
                    },
                }],
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
                supports_websockets: false,
                supports_websocket_compression: false,
                websocket_connect_timeout_ms: None,
            },
            ProviderProfile {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "shared".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                protocol: ProtocolFamily::AnthropicMessages,
                auth: AuthStrategy::Bearer,
                credential: CredentialConfig::None,
                models: vec![ModelProfile {
                    display_model: "Claude".to_string(),
                    request_model: "claude-sonnet-4-20250514".to_string(),
                    billing_model: "claude-sonnet-4-20250514".to_string(),
                    aliases: vec![],
                    description: None,
                    capabilities: Capabilities {
                        streaming: true,
                        tools: true,
                        ..Default::default()
                    },
                }],
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
                supports_websockets: false,
                supports_websocket_compression: false,
                websocket_connect_timeout_ms: None,
            },
        ],
    };

    let err = DefaultLlmClient::from_config(config).unwrap_err();
    assert!(matches!(err, LlmError::InvalidRequest { .. }));
}

/// `OpenAiResponses` profiles construct successfully (the old "no codec yet"
/// error is gone) and `prepare` targets `{base_url}/responses` with POST.
#[tokio::test]
async fn openai_responses_profile_prepares_post_to_responses_endpoint() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai-responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "gpt-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
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
    };

    // The old build_codec arm returned LlmError::InvalidRequest ("no codec
    // yet"); construction must now succeed.
    let client = DefaultLlmClient::from_config(config)
        .expect("OpenAiResponses must have a codec; the 'no codec yet' error is gone");

    let prepared = client.prepare(&LlmRequest::new("gpt-4o")).await.unwrap();
    assert_eq!(
        prepared.route.resolved_route.profile_name,
        "openai-responses"
    );
    assert_eq!(prepared.provider_request.method, "POST");
    assert!(
        prepared.provider_request.url.ends_with("/responses"),
        "URL must end with /responses; got: {}",
        prepared.provider_request.url
    );
    assert_eq!(
        prepared.provider_request.url,
        "https://api.openai.com/v1/responses"
    );
    assert_eq!(prepared.provider_request.body_json["model"], "gpt-4o");
}

#[tokio::test]
async fn openai_responses_websocket_capability_selects_stream_transport_only_for_streaming() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai-responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiResponses,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "gpt-5".to_string(),
                request_model: "gpt-5".to_string(),
                billing_model: "gpt-5".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: true,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: Some(1234),
        }],
    };
    let client = DefaultLlmClient::from_config(config).unwrap();

    let unary = client.prepare(&LlmRequest::new("gpt-5")).await.unwrap();
    assert_eq!(
        unary.provider_request.stream_transport,
        ProviderStreamTransport::Http
    );
    assert_eq!(unary.provider_request.websocket_connect_timeout_ms, None);

    let mut streaming_request = LlmRequest::new("gpt-5").with_user_text("hi");
    streaming_request.stream = true;
    let streaming = client.prepare(&streaming_request).await.unwrap();
    assert_eq!(
        streaming.provider_request.stream_transport,
        ProviderStreamTransport::ResponsesWebSocket
    );
    assert_eq!(
        streaming.provider_request.websocket_connect_timeout_ms,
        Some(1234)
    );
}

#[test]
fn websocket_capability_is_rejected_for_non_responses_protocols() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai-chat".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "gpt-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec![],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: true,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
        }],
    };

    let err = DefaultLlmClient::from_config(config).unwrap_err();
    assert!(
        matches!(err, LlmError::InvalidRequest { ref message } if message.contains("OpenAiResponses")),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn response_format_is_rejected_when_selected_model_lacks_structured_output() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                description: None,
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    structured_output: false,
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
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    let mut request = LlmRequest::new("fast");
    request.response_format = Some(ResponseFormat::JsonObject);

    let err = client.prepare(&request).await.unwrap_err();
    assert!(
        matches!(err, LlmError::UnsupportedCapability { capability } if capability == "structured_output")
    );
}
