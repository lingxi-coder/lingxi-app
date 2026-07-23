//! `count_tokens` facade: real endpoint on Anthropic routes, documented
//! character-based approximation elsewhere.

use crate::model::betas::{apply_beta_header, BetaContext, Endpoint, Provider};
use crate::{client::DefaultLlmClient, AnthropicMessagesCodec, LlmError, LlmRequest, Transport};

/// Approximation divisor for non-Anthropic routes (byte-length/4 ≈ tokens).
pub const APPROX_CHARS_PER_TOKEN: u64 = 4;

/// Count input tokens for `request`'s resolved route.
pub async fn count_tokens(
    client: &DefaultLlmClient,
    transport: &dyn Transport,
    request: &LlmRequest,
) -> Result<u64, LlmError> {
    match try_count_tokens_exact(client, transport, request).await? {
        Some(tokens) => Ok(tokens),
        None => Ok(approximate_tokens(request)),
    }
}

/// Count tokens only when the resolved route exposes Anthropic's exact
/// `count_tokens` endpoint. `None` means callers must use their documented
/// fallback rather than mistaking the generic text approximation for an exact
/// tool-schema count.
pub async fn try_count_tokens_exact(
    client: &DefaultLlmClient,
    transport: &dyn Transport,
    request: &LlmRequest,
) -> Result<Option<u64>, LlmError> {
    match client.prepare_count_tokens(request).await {
        Ok(mut provider_request) => {
            // Gate on the resolved request model in the prepared body (the
            // count_tokens ERr whitelist depends only on the model).
            let model = provider_request
                .body_json
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(request.model.as_str())
                .to_string();
            apply_beta_header(
                &mut provider_request,
                Provider::Anthropic,
                Endpoint::CountTokens,
                &BetaContext::for_model(model),
            );
            let response = transport.execute(&provider_request).await?;
            // decode via a throwaway codec: decode is stateless and
            // base_url-independent.
            AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01")
                .decode_count_tokens_response(&response)
                .map(Some)
        }
        // Coupled to prepare_count_tokens' error message ("count_tokens is only available on AnthropicMessages routes").
        Err(LlmError::InvalidRequest { message }) if message.contains("count_tokens") => Ok(None),
        Err(other) => Err(other),
    }
}

/// Byte-length approximation used on non-Anthropic routes.
///
/// Only Text block content and system text are counted; tool/reasoning/image payloads contribute nothing.
#[must_use]
pub fn approximate_tokens(request: &LlmRequest) -> u64 {
    let mut byte_len = 0u64;
    for block in &request.system {
        byte_len += block.text.len() as u64;
    }
    for message in &request.messages {
        for block in &message.content {
            if let crate::ContentBlock::Text { text, .. } = block {
                byte_len += text.len() as u64;
            }
        }
    }
    (byte_len / APPROX_CHARS_PER_TOKEN).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DefaultLlmClient;
    use crate::{
        AuthStrategy, BoxFuture, Capabilities, ClientConfig, ContentBlock, CredentialConfig,
        LlmRequest, Message, ModelProfile, PricingConfig, ProtocolFamily, ProviderId,
        ProviderProfile, ProviderRequest, ProviderResponse, StreamingResponse, SystemBlock,
        Transport,
    };
    use std::sync::Mutex;

    // ----------------------------------------------------------------
    // Byte-length approximation math tests
    // ----------------------------------------------------------------

    #[test]
    fn approximate_tokens_empty_request_returns_one() {
        let req = LlmRequest::new("model");
        assert_eq!(approximate_tokens(&req), 1);
    }

    #[test]
    fn approximate_tokens_known_char_count_divides_by_four() {
        // 40 bytes → 10 tokens
        let req =
            LlmRequest::new("model").with_user_text("1234567890123456789012345678901234567890");
        assert_eq!(req.messages[0].content.len(), 1);
        assert_eq!(approximate_tokens(&req), 10);
    }

    #[test]
    fn approximate_tokens_system_blocks_are_counted() {
        let mut req = LlmRequest::new("model");
        req.system.push(SystemBlock::text("12345678")); // 8 bytes
        assert_eq!(approximate_tokens(&req), 2); // 8/4 = 2
    }

    #[test]
    fn approximate_tokens_non_text_blocks_ignored() {
        // Image blocks should not count.
        let mut req = LlmRequest::new("model");
        req.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: vec![0u8; 100],
            }],
        });
        // No text → floor to 1.
        assert_eq!(approximate_tokens(&req), 1);
    }

    #[test]
    fn approximate_tokens_minimum_is_one_even_for_very_short_text() {
        // 3 bytes < 4 → floor to 1
        let req = LlmRequest::new("model").with_user_text("hi!");
        assert_eq!(approximate_tokens(&req), 1);
    }

    // ----------------------------------------------------------------
    // Fake Transport for integration-style tests
    // ----------------------------------------------------------------

    #[derive(Debug)]
    struct ScriptedTransport {
        response: ProviderResponse,
        seen: Mutex<Option<ProviderRequest>>,
    }

    impl ScriptedTransport {
        fn returning(response: ProviderResponse) -> Self {
            Self {
                response,
                seen: Mutex::new(None),
            }
        }
    }

    impl Transport for ScriptedTransport {
        fn execute<'a>(
            &'a self,
            request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            *self.seen.lock().expect("lock") = Some(request.clone());
            let response = self.response.clone();
            Box::pin(async move { Ok(response) })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "not used in count_tokens tests".to_string(),
                })
            })
        }
    }

    fn anthropic_client() -> DefaultLlmClient {
        std::env::set_var("LLM_COUNT_TOKENS_TEST_KEY", "ct-test-key");
        DefaultLlmClient::from_config(ClientConfig {
            providers: vec![ProviderProfile {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "anthropic".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                protocol: ProtocolFamily::AnthropicMessages,
                auth: AuthStrategy::ApiKey,
                credential: CredentialConfig::Env {
                    var: "LLM_COUNT_TOKENS_TEST_KEY".to_string(),
                },
                models: vec![ModelProfile {
                    display_model: "Claude".to_string(),
                    request_model: "claude-sonnet-4-20250514".to_string(),
                    billing_model: "claude-sonnet-4".to_string(),
                    aliases: vec!["claude".to_string()],
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
        })
        .expect("client")
    }

    fn openai_client() -> DefaultLlmClient {
        DefaultLlmClient::from_config(ClientConfig {
            providers: vec![ProviderProfile {
                provider_id: ProviderId::OpenAI,
                profile_name: "openai".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                protocol: ProtocolFamily::OpenAiChat,
                auth: AuthStrategy::None,
                credential: CredentialConfig::None,
                models: vec![ModelProfile {
                    display_model: "GPT".to_string(),
                    request_model: "gpt-4".to_string(),
                    billing_model: "gpt-4".to_string(),
                    aliases: vec!["gpt".to_string()],
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
        })
        .expect("client")
    }

    // ----------------------------------------------------------------
    // Happy path: Anthropic → real endpoint → decode input_tokens
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn anthropic_happy_path_returns_count_tokens_from_response() {
        let transport = ScriptedTransport::returning(ProviderResponse::json(
            200,
            serde_json::json!({ "input_tokens": 2095 }),
        ));
        let client = anthropic_client();
        let req = LlmRequest::new("claude").with_user_text("hello world");

        let count = count_tokens(&client, &transport, &req)
            .await
            .expect("count");

        assert_eq!(count, 2095);
        // Also verify the request was sent to the count_tokens endpoint.
        let seen = transport
            .seen
            .lock()
            .unwrap()
            .clone()
            .expect("request sent");
        assert!(
            seen.url.ends_with("/v1/messages/count_tokens"),
            "url={}",
            seen.url
        );
        assert_eq!(
            seen.headers.get("x-api-key").map(String::as_str),
            Some("ct-test-key")
        );
    }

    #[tokio::test]
    async fn anthropic_route_sends_count_tokens_beta_header() {
        let transport = ScriptedTransport::returning(ProviderResponse::json(
            200,
            serde_json::json!({ "input_tokens": 42 }),
        ));
        let client = anthropic_client();
        let req = LlmRequest::new("claude").with_user_text("hello");

        let _ = count_tokens(&client, &transport, &req)
            .await
            .expect("count");

        let seen = transport
            .seen
            .lock()
            .unwrap()
            .clone()
            .expect("request sent");
        let expected = crate::model::betas::assemble_beta_header(
            crate::model::betas::Provider::Anthropic,
            crate::model::betas::Endpoint::CountTokens,
            &crate::model::betas::BetaContext::for_model("claude-sonnet-4-20250514"),
        );
        assert_eq!(
            seen.headers.get("anthropic-beta").map(String::as_str),
            Some(expected.as_str()),
            "anthropic-beta header must equal assemble_beta_header(Anthropic, CountTokens); got: {:?}",
            seen.headers.get("anthropic-beta"),
        );
    }

    // ----------------------------------------------------------------
    // Non-Anthropic route → approximation fallback
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn non_anthropic_route_falls_back_to_approximation() {
        // ScriptedTransport should NOT be called — count_tokens returns
        // InvalidRequest before any network call.
        let transport = ScriptedTransport::returning(ProviderResponse::json(
            200,
            serde_json::json!({ "input_tokens": 9999 }),
        ));
        let client = openai_client();
        // 20 bytes of text → 5 tokens
        let req = LlmRequest::new("gpt").with_user_text("12345678901234567890");

        let count = count_tokens(&client, &transport, &req)
            .await
            .expect("count");

        assert_eq!(count, 5);
        // Transport must NOT have been called.
        assert!(
            transport.seen.lock().unwrap().is_none(),
            "transport should not be called"
        );
    }

    #[tokio::test]
    async fn exact_count_reports_unavailable_for_non_anthropic_route() {
        let transport = ScriptedTransport::returning(ProviderResponse::json(
            200,
            serde_json::json!({ "input_tokens": 9999 }),
        ));
        let client = openai_client();
        let req = LlmRequest::new("gpt").with_user_text("hello");

        assert_eq!(
            try_count_tokens_exact(&client, &transport, &req)
                .await
                .expect("route resolution"),
            None
        );
        assert!(transport.seen.lock().unwrap().is_none());
    }

    // ----------------------------------------------------------------
    // 401 envelope → Authentication error propagates
    // ----------------------------------------------------------------

    #[tokio::test]
    async fn authentication_error_propagates_from_401_response() {
        let transport = ScriptedTransport::returning(ProviderResponse::json(
            401,
            serde_json::json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "invalid api key" }
            }),
        ));
        let client = anthropic_client();
        let req = LlmRequest::new("claude").with_user_text("hello");

        let err = count_tokens(&client, &transport, &req)
            .await
            .expect_err("must fail");

        assert!(
            matches!(err, LlmError::Authentication),
            "expected Authentication, got {err:?}"
        );
    }
}
