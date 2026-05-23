//! Integration test: 401 → refresh → retry, and 5xx × 3 → `RetryExhausted`.

use lingxi_api_client::anthropic::{AnthropicProvider, DEFAULT_BASE_URL};
use lingxi_api_client::{
    ApiError, BearerToken, NoOpOAuthHook, OAuthHookError, OAuthRefreshHook, TokenHash,
};
use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId, Secret};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

mod mock_server;
use mock_server::{MockResp, spawn_mock};

fn make_msgs() -> Vec<ConversationMessage> {
    vec![ConversationMessage::User {
        id: MessageId::new(),
        content: vec![ContentBlock::Text { text: "hi".into() }],
    }]
}

#[tokio::test]
async fn happy_path_200_succeeds_in_one_attempt() {
    let server = spawn_mock(vec![MockResp {
        status: 200,
        body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#.into(),
        headers: vec![],
    }])
    .await;
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", make_msgs(), transport.as_ref())
        .await;
    assert!(r.is_ok(), "happy path must succeed: {r:?}");
    assert_eq!(server.attempt_count(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn three_5xx_yields_retry_exhausted() {
    let server = spawn_mock(vec![
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
        MockResp {
            status: 503,
            body: "boom".into(),
            headers: vec![],
        },
    ])
    .await;
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()));
    let transport = server.transport();
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", make_msgs(), transport.as_ref())
        .await;
    match r {
        Err(ApiError::RetryExhausted { last_status }) => {
            assert_eq!(last_status, Some(503));
        }
        other => panic!("expected RetryExhausted, got {other:?}"),
    }
    assert_eq!(server.attempt_count(), 3);
    server.shutdown().await;
}

// ---- 401 → refresh → retry --------------------------------------------------

struct ScriptedHook {
    refresh_count: AtomicU8,
}

#[async_trait::async_trait]
impl OAuthRefreshHook for ScriptedHook {
    async fn refresh(&self, _prev: TokenHash) -> Result<BearerToken, OAuthHookError> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        Ok(BearerToken(Secret::new("refreshed-token".to_string())))
    }
}

// Rust identifiers cannot start with a digit, so the function name uses
// `four01_` instead of the plan's literal `401_`. The semantic 401-handling
// behaviour under test is unchanged.
#[tokio::test]
async fn four01_then_200_triggers_one_refresh() {
    let server = spawn_mock(vec![
        MockResp { status: 401, body: "expired".into(), headers: vec![] },
        MockResp { status: 200, body: r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#.into(), headers: vec![] },
    ])
    .await;

    let hook = Arc::new(ScriptedHook {
        refresh_count: AtomicU8::new(0),
    });
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()))
        .with_oauth_hook(hook.clone());

    let transport = server.transport();
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", make_msgs(), transport.as_ref())
        .await;
    assert!(r.is_ok(), "401→refresh→200 must succeed: {r:?}");
    assert_eq!(hook.refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.attempt_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn four01_twice_propagates_unauthorized() {
    // Spec rule: retry ONCE after a refresh; second 401 surfaces immediately.
    let server = spawn_mock(vec![
        MockResp {
            status: 401,
            body: "expired".into(),
            headers: vec![],
        },
        MockResp {
            status: 401,
            body: "still expired".into(),
            headers: vec![],
        },
    ])
    .await;
    let hook = Arc::new(ScriptedHook {
        refresh_count: AtomicU8::new(0),
    });
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()))
        .with_oauth_hook(hook.clone());

    let transport = server.transport();
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", make_msgs(), transport.as_ref())
        .await;
    match r {
        Err(ApiError::Unauthorized(_)) => {}
        other => panic!("expected Unauthorized after second 401, got {other:?}"),
    }
    assert_eq!(hook.refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.attempt_count(), 2);
    server.shutdown().await;
}

#[tokio::test]
async fn no_op_hook_does_not_retry_on_401() {
    let server = spawn_mock(vec![MockResp {
        status: 401,
        body: "unauthorized".into(),
        headers: vec![],
    }])
    .await;
    let provider = AnthropicProvider::new("sk-test", Some(server.base_url.clone()))
        .with_oauth_hook(Arc::new(NoOpOAuthHook));
    let transport = server.transport();
    let r = provider
        .messages_create_non_stream("claude-opus-4-6", make_msgs(), transport.as_ref())
        .await;
    // NoOp hook returns TokenStale → caller surfaces as Unauthorized.
    match r {
        Err(
            ApiError::OAuthHook(OAuthHookError::TokenStale) | ApiError::Unauthorized(_),
        ) => {}
        other => panic!("expected Unauthorized or OAuthHook(TokenStale), got {other:?}"),
    }
    server.shutdown().await;
}

#[test]
fn default_base_url_matches_spec_byte_for_byte() {
    assert_eq!(DEFAULT_BASE_URL, "https://api.anthropic.com");
}
