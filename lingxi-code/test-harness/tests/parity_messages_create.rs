//! Parity driver: assert that `messages_create_non_stream` builds a wire
//! request whose URL + locked headers byte-match claude-code @ 6a25909.

use api_client::anthropic::{user_agent, AnthropicProvider, ANTHROPIC_VERSION, DEFAULT_BASE_URL};
use api_client::betas::{assemble_beta_header, Endpoint, Provider};
use serde::Deserialize;
use serde_json::Value;
use test_harness::parity::load_fixture;

#[derive(Deserialize)]
struct Fixture {
    expected_request: ExpectedRequest,
}

#[derive(Deserialize)]
struct ExpectedRequest {
    method: String,
    url_suffix: String,
    headers: Headers,
    body_keys: Vec<String>,
}

#[derive(Deserialize)]
struct Headers {
    #[serde(rename = "anthropic-version")]
    anthropic_version: String,
    #[serde(rename = "content-type")]
    content_type: String,
    accept: String,
    #[serde(rename = "user-agent_prefix")]
    user_agent_prefix: String,
    #[serde(rename = "user-agent_suffix")]
    user_agent_suffix: String,
    #[serde(rename = "anthropic-beta")]
    anthropic_beta: String,
}

#[test]
fn messages_create_request_shape_matches_claude_code() {
    let fx: Fixture = load_fixture("messages_create");
    let provider = AnthropicProvider::new("sk-test", None);
    let body = serde_json::json!({
        "model": "claude-opus-4-6",
        "max_tokens": 4096u32,
        "messages": [ { "role": "user", "content": [ { "type": "text", "text": "hello" } ] } ],
    });
    let req = provider.build_request(&body);

    // URL — default base + suffix from fixture.
    assert_eq!(
        req.url,
        format!("{DEFAULT_BASE_URL}{}", fx.expected_request.url_suffix)
    );
    // Method.
    assert_eq!(
        format!("{:?}", req.method).to_uppercase(),
        fx.expected_request.method.to_uppercase()
    );
    // Locked header values.
    let get = |k: &str| -> Option<String> {
        req.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get("anthropic-version").as_deref(), Some(ANTHROPIC_VERSION));
    assert_eq!(
        get("anthropic-version").as_deref(),
        Some(fx.expected_request.headers.anthropic_version.as_str())
    );
    assert_eq!(
        get("content-type").as_deref(),
        Some(fx.expected_request.headers.content_type.as_str())
    );
    assert_eq!(
        get("accept").as_deref(),
        Some(fx.expected_request.headers.accept.as_str())
    );

    // User-agent — assert prefix + suffix bracket the version string.
    // (build_request() emits the base request shape; the live request path
    // attaches user-agent via build_request_with_betas. Here we exercise the
    // user_agent() function directly to lock its format.)
    let ua = user_agent();
    assert!(
        ua.starts_with(&fx.expected_request.headers.user_agent_prefix),
        "user-agent must start with {:?}; got {ua:?}",
        fx.expected_request.headers.user_agent_prefix
    );
    assert!(
        ua.ends_with(&fx.expected_request.headers.user_agent_suffix),
        "user-agent must end with {:?}; got {ua:?}",
        fx.expected_request.headers.user_agent_suffix
    );

    // Beta header — the assembler must produce the exact comma-joined value.
    let assembled = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
    assert_eq!(
        assembled, fx.expected_request.headers.anthropic_beta,
        "anthropic-beta drift detected. If betas.ts changed upstream, update messages_create.json AND betas.rs constants in the same commit.",
    );

    // Body keys.
    let body_val: Value = serde_json::from_str(req.body.as_ref().unwrap()).unwrap();
    for key in &fx.expected_request.body_keys {
        assert!(
            body_val.get(key).is_some(),
            "request body missing key {key:?}; got {body_val:?}",
        );
    }
}
