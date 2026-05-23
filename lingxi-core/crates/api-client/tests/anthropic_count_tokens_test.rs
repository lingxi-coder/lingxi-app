//! Integration test for `count_tokens(model, msgs)` including the Vertex
//! restricted-model whitelist.

use lingxi_api_client::ApiError;
use lingxi_api_client::anthropic::{AnthropicProvider, CountTokensProvider};
use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId};

mod mock_server;
use mock_server::{MockResp, spawn_mock};

fn msgs() -> Vec<ConversationMessage> {
    vec![ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::Text {
            text: "count me".into(),
        }],
    }]
}

#[tokio::test]
async fn anthropic_provider_accepts_any_model() {
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"input_tokens":42}"#.into(),
        headers: vec![],
    }])
    .await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .count_tokens(
            "any-model-ever",
            msgs(),
            CountTokensProvider::Anthropic,
            transport.as_ref(),
        )
        .await;
    assert!(r.is_ok(), "Anthropic must accept any model: {r:?}");
    let resp = r.unwrap();
    assert_eq!(resp.input_tokens, 42);
    server.shutdown().await;
}

#[tokio::test]
async fn vertex_provider_rejects_non_claude_model() {
    // No need for a mock — rejection must happen client-side before HTTP.
    let server = spawn_mock(vec![]).await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .count_tokens(
            "gpt-4-turbo",
            msgs(),
            CountTokensProvider::Vertex,
            transport.as_ref(),
        )
        .await;
    match r {
        Err(ApiError::UnsupportedModel { model, provider }) => {
            assert_eq!(model, "gpt-4-turbo");
            assert_eq!(provider, "vertex");
        }
        other => panic!("expected UnsupportedModel, got {other:?}"),
    }
    assert_eq!(
        server.attempt_count(),
        0,
        "rejection must happen before any HTTP request"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn vertex_provider_accepts_claude_3_model() {
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"input_tokens":7}"#.into(),
        headers: vec![],
    }])
    .await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .count_tokens(
            "claude-3-5-sonnet-20241022",
            msgs(),
            CountTokensProvider::Vertex,
            transport.as_ref(),
        )
        .await;
    assert!(r.is_ok(), "Vertex must accept claude-3 family: {r:?}");
    server.shutdown().await;
}

#[tokio::test]
async fn vertex_provider_accepts_claude_opus_model() {
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"input_tokens":3}"#.into(),
        headers: vec![],
    }])
    .await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .count_tokens(
            "claude-opus-4-1-20250805",
            msgs(),
            CountTokensProvider::Vertex,
            transport.as_ref(),
        )
        .await;
    assert!(r.is_ok());
    server.shutdown().await;
}

#[tokio::test]
async fn bedrock_provider_uses_same_whitelist_as_vertex() {
    let server = spawn_mock(vec![]).await;
    let p = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = p
        .count_tokens(
            "llama-3-70b",
            msgs(),
            CountTokensProvider::Bedrock,
            transport.as_ref(),
        )
        .await;
    match r {
        Err(ApiError::UnsupportedModel { model, provider }) => {
            assert_eq!(model, "llama-3-70b");
            assert_eq!(provider, "bedrock");
        }
        other => panic!("expected UnsupportedModel, got {other:?}"),
    }
    server.shutdown().await;
}
