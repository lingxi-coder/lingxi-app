# llm-client Engine Adoption — Plan 1 of 3 (P1 wire additions + P2 transport bridge)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the wire features the engine needs to llm-client (reasoning budget, prompt-cache control + system blocks, count_tokens) and bridge `platform_api::HttpTransport` to `llm_client::Transport` in platforms/common.

**Architecture:** Spec `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md` (rev2). This plan covers spec phases P1 and P2's transport bridge. The anthropic-oauth CredentialProvider impl (P2b) moves to Plan 2 alongside the orchestrator policy ports (P3) because both need those crates' internals read first; Plan 3 covers P4+P5. Deviation from spec noted in Task 6: `stream_sse` is a required trait method, so the "fall back to stream_raw_bytes + SseFrameSplitter" path is dead code and is not built (YAGNI).

**Tech Stack:** Rust 2021, cargo workspace at `lingxi-code/`, TDD with `cargo test -p <crate>`, clippy must stay clean (`cargo clippy -p <crate> --all-targets`).

**Working directory:** all paths below are relative to `lingxi-code/`. Run commands from there.

---

### Task 1: `ReasoningConfig` on `LlmRequest` + per-codec encoding

**Files:**
- Modify: `llm-client/src/protocol.rs` (LlmRequest fields, new struct, validate_capabilities)
- Modify: `llm-client/src/lib.rs` (export)
- Modify: `llm-client/src/providers/anthropic.rs` (encode)
- Modify: `llm-client/src/providers/gemini.rs` (encode)
- Modify: `llm-client/src/providers/openai.rs` (reject)
- Test: `llm-client/tests/anthropic_codec_test.rs`, `llm-client/tests/gemini_codec_test.rs`, `llm-client/tests/openai_codec_test.rs`, `llm-client/tests/protocol_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `llm-client/tests/anthropic_codec_test.rs`:

```rust
#[test]
fn encode_reasoning_budget_as_thinking() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["thinking"]["type"], "enabled");
    assert_eq!(provider_request.body_json["thinking"]["budget_tokens"], 2048);

    let bare = codec.encode_request(&LlmRequest::new("claude-sonnet-4-20250514")).unwrap();
    assert!(bare.body_json.get("thinking").is_none());
}
```

Append to `llm-client/tests/gemini_codec_test.rs`:

```rust
#[test]
fn encode_reasoning_budget_as_thinking_config() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(
        provider_request.body_json["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        2048
    );
}
```

Append to `llm-client/tests/openai_codec_test.rs`:

```rust
#[test]
fn reasoning_config_is_rejected_until_responses_api_exists() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 2048 });

    let err = codec.encode_request(&request).unwrap_err();

    assert!(matches!(err, llm_client::LlmError::InvalidRequest { message } if message.contains("reasoning")));
}
```

Append to `llm-client/tests/protocol_test.rs`:

```rust
#[test]
fn reasoning_config_requires_reasoning_capability() {
    let mut request = LlmRequest::new("m").with_user_text("hi");
    request.reasoning = Some(llm_client::ReasoningConfig { budget_tokens: 1024 });
    let capabilities = Capabilities {
        streaming: true,
        tools: true,
        reasoning: false,
        ..Default::default()
    };

    let error = validate_capabilities(&request, capabilities).expect_err("reasoning should fail");

    assert!(matches!(
        error,
        llm_client::LlmError::UnsupportedCapability { capability } if capability == "reasoning"
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p llm-client --no-fail-fast --test anthropic_codec_test --test gemini_codec_test --test openai_codec_test --test protocol_test 2>&1 | grep -E "^error" | sort -u`
Expected: `error[E0609]: no field `reasoning` on type `LlmRequest`` and `cannot find ... ReasoningConfig` compile errors.

- [ ] **Step 3: Implement**

In `llm-client/src/protocol.rs`, after the `stop_sequences` field of `LlmRequest` add:

```rust
    /// Optional reasoning/thinking budget request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
```

After the `LlmRequest` impl block add:

```rust
/// Provider-neutral reasoning/thinking budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// Maximum tokens the model may spend on reasoning.
    pub budget_tokens: u32,
}
```

In `validate_capabilities`, after the tools check add:

```rust
    if request.reasoning.is_some() && !capabilities.reasoning {
        return Err(LlmError::UnsupportedCapability {
            capability: "reasoning".to_string(),
        });
    }
```

In `llm-client/src/lib.rs` add `ReasoningConfig` to the `pub use protocol::{...}` list (alphabetical position after `RawStreamFrame`).

In `llm-client/src/providers/anthropic.rs::encode_request`, after the `stop_sequences` insertion add:

```rust
        if let Some(reasoning) = &request.reasoning {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled", "budget_tokens": reasoning.budget_tokens}),
            );
        }
```

In `llm-client/src/providers/gemini.rs::encode_request`, inside the existing `generation_config` block (before the `if !generation_config.is_empty()` check) add:

```rust
        if let Some(reasoning) = &request.reasoning {
            generation_config.insert(
                "thinkingConfig".to_string(),
                serde_json::json!({"thinkingBudget": reasoning.budget_tokens}),
            );
        }
```

In `llm-client/src/providers/openai.rs::encode_request`, immediately after the `reject_unsupported_content_blocks(request)?;` line add:

```rust
        if request.reasoning.is_some() {
            return Err(LlmError::InvalidRequest {
                message: "OpenAiChatCodec does not encode reasoning budgets yet".to_string(),
            });
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p llm-client 2>&1 | grep -cE "FAILED|error\["`
Expected: `0` (all suites green).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/llm-client && git commit -m "feat(llm-client): provider-neutral reasoning budget

Anthropic encodes thinking{enabled,budget_tokens}; Gemini encodes
generationConfig.thinkingConfig.thinkingBudget; OpenAI Chat rejects until
a Responses codec exists. Preflight requires the reasoning capability.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `CacheControl` + system blocks (breaking schema change)

**Files:**
- Modify: `llm-client/src/protocol.rs` (CacheControl, SystemBlock, ContentBlock fields, LlmRequest.system)
- Modify: `llm-client/src/lib.rs` (exports)
- Modify: `llm-client/src/providers/anthropic.rs`, `llm-client/src/providers/openai.rs`, `llm-client/src/providers/gemini.rs`
- Modify (mechanical re-type of `system`): `llm-client/tests/anthropic_codec_test.rs`, `llm-client/tests/openai_codec_test.rs`, `llm-client/tests/gemini_codec_test.rs`
- Test (new): same three codec test files

- [ ] **Step 1: Write the failing tests**

Append to `llm-client/tests/anthropic_codec_test.rs`:

```rust
#[test]
fn encode_system_blocks_and_cache_control() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.system = vec![
        llm_client::SystemBlock { text: "stable prefix".to_string(), cache_control: Some(llm_client::CacheControl::Ephemeral) },
        llm_client::SystemBlock { text: "tail".to_string(), cache_control: None },
    ];
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: Some(llm_client::CacheControl::Ephemeral),
        }],
    });

    let provider_request = codec.encode_request(&request).unwrap();
    let body = &provider_request.body_json;

    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "stable prefix");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(body["system"][1].get("cache_control").is_none());
    assert_eq!(body["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");

    let bare = codec
        .encode_request(&LlmRequest::new("claude-sonnet-4-20250514").with_user_text("hi"))
        .unwrap();
    assert!(bare.body_json.get("system").is_none());
    assert!(bare.body_json["messages"][0]["content"][0].get("cache_control").is_none());
}
```

Append to `llm-client/tests/openai_codec_test.rs`:

```rust
#[test]
fn system_blocks_join_into_one_system_message() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.system = vec![
        llm_client::SystemBlock { text: "a".to_string(), cache_control: Some(llm_client::CacheControl::Ephemeral) },
        llm_client::SystemBlock { text: "b".to_string(), cache_control: None },
    ];

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["messages"][0]["role"], "system");
    assert_eq!(provider_request.body_json["messages"][0]["content"], "a\n\nb");
}
```

Append to `llm-client/tests/gemini_codec_test.rs`:

```rust
#[test]
fn system_blocks_become_system_instruction_parts() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.system = vec![
        llm_client::SystemBlock { text: "a".to_string(), cache_control: None },
        llm_client::SystemBlock { text: "b".to_string(), cache_control: Some(llm_client::CacheControl::Ephemeral) },
    ];

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.body_json["systemInstruction"]["parts"][0]["text"], "a");
    assert_eq!(provider_request.body_json["systemInstruction"]["parts"][1]["text"], "b");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p llm-client --no-fail-fast 2>&1 | grep -E "^error" | sort -u | head`
Expected: compile errors naming `SystemBlock`, `CacheControl`, and a missing `cache_control` field on `ContentBlock::Text`.

- [ ] **Step 3: Implement the schema**

In `llm-client/src/protocol.rs`:

Change the `system` field on `LlmRequest`:

```rust
    /// System prompt blocks, in order (empty = no system prompt).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
```

Add after `ReasoningConfig`:

```rust
/// One system-prompt block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemBlock {
    /// System text.
    pub text: String,
    /// Optional prompt-cache breakpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    /// Create a plain system block without a cache breakpoint.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache_control: None,
        }
    }
}

/// Prompt-cache control marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheControl {
    /// Anthropic ephemeral cache breakpoint.
    Ephemeral,
}
```

On `ContentBlock`, add to the `Text` and `ToolResult` variants (after their last field):

```rust
        /// Optional prompt-cache breakpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
```

In `llm-client/src/lib.rs` add `CacheControl` and `SystemBlock` to the protocol re-export list.

- [ ] **Step 4: Mechanically fix every constructor and pattern**

Compiler-driven; the complete list of sites:

- `llm-client/src/protocol.rs::with_user_text` — `ContentBlock::Text { text: text.into(), cache_control: None }`.
- `llm-client/src/providers/anthropic.rs`:
  - `encode_content_block` `Text` arm becomes:
    ```rust
    ContentBlock::Text { text, cache_control } => Ok(with_cache_control(
        serde_json::json!({"type": "text", "text": text}),
        cache_control,
    )),
    ```
  - `ToolResult` arm: change the pattern to `{ tool_call_id, output, is_error, cache_control }` and wrap the built block with the same helper before returning:
    ```rust
    Ok(with_cache_control(block, cache_control))
    ```
  - Add the helper next to `encode_content_block`:
    ```rust
    fn with_cache_control(mut block: Value, cache_control: &Option<crate::CacheControl>) -> Value {
        if cache_control.is_some() {
            block["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        block
    }
    ```
  - `encode_request` system handling becomes:
    ```rust
    if !request.system.is_empty() {
        let system: Vec<Value> = request
            .system
            .iter()
            .map(|block| {
                with_cache_control(
                    serde_json::json!({"type": "text", "text": block.text}),
                    &block.cache_control,
                )
            })
            .collect();
        body.insert("system".to_string(), Value::Array(system));
    }
    ```
  - `decode_content_block` `text` arm constructs `cache_control: None`; the `count_tokens` builder in Task 3 reuses these helpers.
- `llm-client/src/providers/openai.rs`:
  - system handling becomes:
    ```rust
    if !request.system.is_empty() {
        let text = request
            .system
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        messages.push(serde_json::json!({"role": "system", "content": text}));
    }
    ```
  - `encode_message` `Text` pattern → `ContentBlock::Text { text: block_text, .. }`; decode site constructs `cache_control: None`; stream `handle_text` block start constructs `cache_control: None`.
- `llm-client/src/providers/gemini.rs`:
  - system handling becomes:
    ```rust
    if !request.system.is_empty() {
        let parts: Vec<Value> = request
            .system
            .iter()
            .map(|block| serde_json::json!({"text": block.text}))
            .collect();
        body.insert("systemInstruction".to_string(), serde_json::json!({"parts": parts}));
    }
    ```
  - `Text` patterns gain `, ..`; decode/stream constructors gain `cache_control: None`.
- Tests: every `request.system = Some("sys".to_string())` becomes
  `request.system = vec![llm_client::SystemBlock::text("sys")]`; every
  `ContentBlock::Text { text: ... }` literal gains `cache_control: None`;
  every `ContentBlock::ToolResult { ... }` literal gains `cache_control: None`.
  Affected test files: `anthropic_codec_test.rs`, `openai_codec_test.rs`,
  `gemini_codec_test.rs`, `request_intent_test.rs`, `protocol_test.rs`,
  `protocol_events_test.rs` (the pinned `content_block_start` JSON stays valid
  because `cache_control` is skip-serialized when `None`), `transport_stream_test.rs`.

- [ ] **Step 5: Run the full suite**

Run: `cargo test -p llm-client 2>&1 | awk '/test result: ok/ {p+=$4} /FAILED/ {f+=1} END {print p" passed, "f+0" failed"}'`
Expected: all passed, 0 failed (count grows by the 3 new tests).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p llm-client --all-targets 2>&1 | grep -E "^(warning|error)" || true` — expected: no output.

```bash
git add lingxi-code/llm-client && git commit -m "feat(llm-client)!: system blocks and prompt-cache control

LlmRequest.system becomes Vec<SystemBlock> and Text/ToolResult blocks
carry an optional CacheControl::Ephemeral breakpoint. Anthropic encodes
the system array and cache_control markers; OpenAI joins system text;
Gemini emits one systemInstruction part per block. Breaking by design -
the crate has a single consumer migration in flight.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Anthropic `count_tokens`

**Files:**
- Modify: `llm-client/src/providers/anthropic.rs`
- Test: `llm-client/tests/anthropic_codec_test.rs`

- [ ] **Step 1: Write the failing tests**

Append to `llm-client/tests/anthropic_codec_test.rs`:

```rust
#[test]
fn count_tokens_request_and_response_round_trip() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514").with_user_text("hello");
    request.system = vec![llm_client::SystemBlock::text("sys")];

    let provider_request = codec.encode_count_tokens_request(&request).unwrap();

    assert!(provider_request.url.ends_with("/v1/messages/count_tokens"));
    assert_eq!(provider_request.body_json["model"], "claude-sonnet-4-20250514");
    assert_eq!(provider_request.body_json["messages"][0]["role"], "user");
    assert_eq!(provider_request.body_json["system"][0]["text"], "sys");
    assert!(provider_request.body_json.get("max_tokens").is_none());
    assert!(provider_request.body_json.get("stream").is_none());

    let count = codec
        .decode_count_tokens_response(ProviderResponse::json(200, serde_json::json!({"input_tokens": 2095})))
        .unwrap();
    assert_eq!(count, 2095);
}

#[test]
fn count_tokens_error_status_maps_through_taxonomy() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");

    let err = codec
        .decode_count_tokens_response(ProviderResponse::json(401, serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "bad key"}
        })))
        .unwrap_err();

    assert!(matches!(err, llm_client::LlmError::Authentication));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p llm-client --test anthropic_codec_test 2>&1 | grep -E "^error" | sort -u | head -3`
Expected: `E0599` no method `encode_count_tokens_request` / `decode_count_tokens_response`.

- [ ] **Step 3: Implement**

In `llm-client/src/providers/anthropic.rs`, refactor `encode_request`'s body
construction so both endpoints share it, then add the inherent methods inside
`impl AnthropicMessagesCodec`:

```rust
    fn count_tokens_url(&self) -> String {
        format!("{}/v1/messages/count_tokens", self.base_url.trim_end_matches('/'))
    }

    /// Encode a `count_tokens` request (same prompt shape, no generation
    /// controls).
    pub fn encode_count_tokens_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        let body = base_body(request)?;
        let mut provider_request = ProviderRequest::post_json(self.count_tokens_url(), Value::Object(body));
        provider_request
            .headers
            .insert("anthropic-version".to_string(), self.anthropic_version.clone());
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        Ok(provider_request)
    }

    /// Decode a `count_tokens` response into the input-token count.
    pub fn decode_count_tokens_response(&self, response: ProviderResponse) -> Result<u64, LlmError> {
        if response.status >= 400 {
            return Err(decode_error_response(&response));
        }
        response
            .body_json
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| LlmError::InvalidRequest {
                message: "Anthropic count_tokens response missing input_tokens".to_string(),
            })
    }
```

Extract `base_body` (free function) holding what `encode_request` builds for
`model`, `messages`, `system`, `tools`, and `thinking` — and have
`encode_request` call it before adding `max_tokens`, sampling controls,
`stream`, and `tool_choice`:

```rust
/// Prompt-shaped body fields shared by messages and count_tokens.
fn base_body(request: &LlmRequest) -> Result<serde_json::Map<String, Value>, LlmError> {
    if request.response_format.is_some() {
        return Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec does not encode response_format yet".to_string(),
        });
    }

    let messages = request
        .messages
        .iter()
        .map(encode_message)
        .collect::<Result<Vec<_>, _>>()?;

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert("messages".to_string(), Value::Array(messages));
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(encode_tool).collect()),
        );
    }
    if !request.system.is_empty() {
        let system: Vec<Value> = request
            .system
            .iter()
            .map(|block| {
                with_cache_control(
                    serde_json::json!({"type": "text", "text": block.text}),
                    &block.cache_control,
                )
            })
            .collect();
        body.insert("system".to_string(), Value::Array(system));
    }
    if let Some(reasoning) = &request.reasoning {
        body.insert(
            "thinking".to_string(),
            serde_json::json!({"type": "enabled", "budget_tokens": reasoning.budget_tokens}),
        );
    }
    Ok(body)
}
```

`encode_request` becomes `let mut body = base_body(request)?;` followed by its
existing `max_tokens` / sampling / `stream` / `tool_choice` / header logic.

- [ ] **Step 4: Run the full suite + clippy**

Run: `cargo test -p llm-client 2>&1 | grep -c FAILED` — expected `0`.
Run: `cargo clippy -p llm-client --all-targets 2>&1 | grep -E "^(warning|error)" || true` — expected no output.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/llm-client && git commit -m "feat(llm-client): Anthropic count_tokens endpoint support

Inherent encode/decode on AnthropicMessagesCodec sharing the prompt body
builder with encode_request; error statuses route through the existing
taxonomy. WireCodec trait unchanged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: platforms/common dependency + request/response mapping

**Files:**
- Modify: `platforms/common/Cargo.toml` (add `llm-client = { path = "../../llm-client" }`; ensure `[dev-dependencies] tokio = { workspace = true }` exists)
- Create: `platforms/common/src/llm_transport.rs`
- Modify: `platforms/common/src/lib.rs` (add `pub mod llm_transport;` and `pub use llm_transport::LlmTransportBridge;`)
- Test: `platforms/common/tests/llm_transport_test.rs`

- [ ] **Step 1: Write the failing test (non-streaming execute)**

Create `platforms/common/tests/llm_transport_test.rs`:

```rust
use std::sync::Mutex;

use async_trait::async_trait;
use platforms_common::LlmTransportBridge;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use platform_api::http::SseStream;
use platform_api::{HttpError, HttpTransport};

#[derive(Default)]
struct FakeHttp {
    response: Mutex<Option<Result<HttpResponse, HttpError>>>,
    sse: Mutex<Option<Result<Vec<Result<SseEvent, HttpError>>, HttpError>>>,
    seen: Mutex<Option<HttpRequest>>,
}

#[async_trait]
impl HttpTransport for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        *self.seen.lock().expect("seen") = Some(req);
        self.response.lock().expect("response").take().expect("scripted response")
    }

    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        *self.seen.lock().expect("seen") = Some(req);
        let events = self.sse.lock().expect("sse").take().expect("scripted sse")?;
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

fn provider_request() -> llm_client::ProviderRequest {
    let mut request = llm_client::ProviderRequest::post_json(
        "https://api.anthropic.com/v1/messages",
        serde_json::json!({"model": "m"}),
    );
    request.headers.insert("x-api-key".to_string(), "k".to_string());
    request
}

#[tokio::test]
async fn execute_maps_request_and_response() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Ok(HttpResponse {
        status: 200,
        headers: vec![
            ("Request-Id".to_string(), "req_1".to_string()),
            ("Retry-After".to_string(), "7".to_string()),
        ],
        body: r#"{"id":"msg_1"}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("response");

    assert_eq!(response.status, 200);
    assert_eq!(response.headers.get("retry-after").map(String::as_str), Some("7"));
    assert_eq!(response.request_id.as_deref(), Some("req_1"));
    assert_eq!(response.body_json["id"], "msg_1");

    let seen = bridge.inner().seen.lock().unwrap().take().expect("request sent");
    assert!(matches!(seen.method, protocol::HttpMethod::Post));
    assert_eq!(seen.url, "https://api.anthropic.com/v1/messages");
    assert!(seen.headers.iter().any(|(k, v)| k == "x-api-key" && v == "k"));
    assert_eq!(seen.body.as_deref(), Some(r#"{"model":"m"}"#));
}

#[tokio::test]
async fn execute_passes_error_statuses_through_as_data() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Ok(HttpResponse {
        status: 429,
        headers: vec![],
        body: r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("error status is data, not Err");

    assert_eq!(response.status, 429);
    assert_eq!(response.body_json["error"]["type"], "rate_limit_error");
}

#[tokio::test]
async fn http_status_error_variant_also_becomes_data() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Err(HttpError::Status {
        status: 500,
        body: r#"{"type":"error","error":{"type":"api_error","message":"boom"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let response = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect("status error is data");

    assert_eq!(response.status, 500);
    assert_eq!(response.body_json["error"]["type"], "api_error");
}

#[tokio::test]
async fn connection_errors_map_to_llm_transport_error() {
    let fake = FakeHttp::default();
    *fake.response.lock().unwrap() = Some(Err(HttpError::Connection("dns".to_string())));
    let bridge = LlmTransportBridge::new(fake);

    let error = llm_client::Transport::execute(&bridge, &provider_request())
        .await
        .expect_err("connection error");

    assert!(matches!(error, llm_client::LlmError::Transport { message } if message.contains("dns")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p platforms-common --test llm_transport_test 2>&1 | grep -E "^error" | sort -u | head -5`
Expected: unresolved `platforms_common::LlmTransportBridge` (plus the missing Cargo dep if Step 3 hasn't added it).

Note: confirm the crate's package name with `grep '^name' platforms/common/Cargo.toml` and adjust the `use` line if it differs from `platforms_common`.

- [ ] **Step 3: Implement the bridge (execute path)**

Create `platforms/common/src/llm_transport.rs`:

```rust
//! Bridge from `platform_api::HttpTransport` to `llm_client::Transport`.
//!
//! One generic adapter serves every platform HTTP implementation
//! (`ReqwestHttp` on desktop, native transports on mobile).

use std::collections::BTreeMap;

use llm_client::{
    BoxFuture, FrameStream, LlmError, ProviderRequest, ProviderResponse, RawStreamFrame,
    StreamingResponse,
};
use protocol::{HttpMethod, HttpRequest, HttpResponse};
use platform_api::http::SseStream;
use platform_api::{HttpError, HttpTransport};

/// Adapter exposing a [`platform_api::HttpTransport`] as an [`llm_client::Transport`].
pub struct LlmTransportBridge<T> {
    inner: T,
}

impl<T> LlmTransportBridge<T> {
    /// Wrap a platform HTTP transport.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Access the wrapped transport (used by hosts and tests).
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

fn to_http_request(request: &ProviderRequest) -> Result<HttpRequest, LlmError> {
    let method = match request.method.as_str() {
        "POST" => HttpMethod::Post,
        "GET" => HttpMethod::Get,
        other => {
            return Err(LlmError::InvalidRequest {
                message: format!("unsupported provider request method: {other}"),
            })
        }
    };
    Ok(HttpRequest {
        method,
        url: request.url.clone(),
        headers: request
            .headers
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        body: Some(request.body_json.to_string()),
        timeout: None,
    })
}

fn lowercase_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn request_id(headers: &BTreeMap<String, String>) -> Option<String> {
    headers
        .get("request-id")
        .or_else(|| headers.get("x-request-id"))
        .cloned()
}

fn to_provider_response(response: HttpResponse) -> ProviderResponse {
    let headers = lowercase_headers(&response.headers);
    let body_json =
        serde_json::from_str(&response.body).unwrap_or(serde_json::Value::Null);
    ProviderResponse {
        status: response.status,
        request_id: request_id(&headers),
        headers,
        body_json,
    }
}

fn status_error_response(status: u16, body: &str) -> ProviderResponse {
    ProviderResponse {
        status,
        headers: BTreeMap::new(),
        body_json: serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
        request_id: None,
    }
}

fn map_http_error(error: &HttpError) -> LlmError {
    LlmError::Transport {
        message: error.to_string(),
    }
}

impl<T: HttpTransport> llm_client::Transport for LlmTransportBridge<T> {
    fn execute<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
        Box::pin(async move {
            let http_request = to_http_request(request)?;
            match self.inner.request(http_request).await {
                Ok(response) => Ok(to_provider_response(response)),
                // Some transports surface non-2xx as an error variant; keep
                // it data so llm-client's taxonomy does the classification.
                Err(HttpError::Status { status, body }) => Ok(status_error_response(status, &body)),
                Err(error) => Err(map_http_error(&error)),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
        Box::pin(async move {
            let http_request = to_http_request(request)?;
            match self.inner.stream_sse(http_request).await {
                Ok(stream) => Ok(StreamingResponse {
                    status: 200,
                    headers: BTreeMap::new(),
                    frames: Box::new(SseFrames { stream }),
                }),
                Err(HttpError::Status { status, body }) => Ok(StreamingResponse {
                    status,
                    headers: BTreeMap::new(),
                    frames: Box::new(BodyFrame {
                        body: Some(body.into_bytes()),
                    }),
                }),
                Err(error) => Err(map_http_error(&error)),
            }
        })
    }
}

struct SseFrames {
    stream: SseStream,
}

impl FrameStream for SseFrames {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        Box::pin(async move {
            use futures_util::StreamExt;
            match self.stream.next().await {
                Some(Ok(event)) => Ok(Some(RawStreamFrame::new(event.data.into_bytes()))),
                Some(Err(error)) => Err(map_http_error(&error)),
                None => Ok(None),
            }
        })
    }
}

/// Error-status body delivered as a single frame for llm-client to drain.
struct BodyFrame {
    body: Option<Vec<u8>>,
}

impl FrameStream for BodyFrame {
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>> {
        let body = self.body.take();
        Box::pin(async move { Ok(body.map(RawStreamFrame::new)) })
    }
}
```

Add to `platforms/common/src/lib.rs`:

```rust
pub mod llm_transport;
pub use llm_transport::LlmTransportBridge;
```

Add to `platforms/common/Cargo.toml` `[dependencies]`:

```toml
llm-client = { path = "../../llm-client" }
```

and ensure `[dev-dependencies]` contains `tokio = { workspace = true }` and `futures-util = { workspace = true }`.

- [ ] **Step 4: Run the execute tests**

Run: `cargo test -p platforms-common --test llm_transport_test 2>&1 | grep "test result"`
Expected: `4 passed; 0 failed` (the streaming tests arrive in Task 5).

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/platforms/common lingxi-code/Cargo.lock && git commit -m "feat(platforms-common): LlmTransportBridge over platform_api::HttpTransport

Generic adapter exposing any platform HTTP transport as an
llm_client::Transport; non-2xx (including HttpError::Status) stays data
so llm-client's error taxonomy classifies it.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: bridge streaming path

**Files:**
- Modify: `platforms/common/tests/llm_transport_test.rs` (the implementation landed in Task 4; this task pins streaming behavior)

- [ ] **Step 1: Write the failing tests**

Append to `platforms/common/tests/llm_transport_test.rs`:

```rust
fn sse(data: &str) -> Result<SseEvent, HttpError> {
    Ok(SseEvent {
        event_type: Some("message".to_string()),
        data: data.to_string(),
        id: None,
    })
}

#[tokio::test]
async fn open_stream_maps_sse_events_to_frames() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_stop"}"#),
        sse("[DONE]"),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("stream");

    assert_eq!(streaming.status, 200);
    let first = streaming.frames.next_frame().await.unwrap().expect("frame");
    assert_eq!(first.bytes, br#"{"type":"message_stop"}"#);
    let second = streaming.frames.next_frame().await.unwrap().expect("frame");
    assert_eq!(second.bytes, b"[DONE]");
    assert!(streaming.frames.next_frame().await.unwrap().is_none());
}

#[tokio::test]
async fn open_stream_status_error_yields_status_and_body_frame() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Err(HttpError::Status {
        status: 429,
        body: r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#.to_string(),
    }));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("status error is data");

    assert_eq!(streaming.status, 429);
    let body = streaming.frames.next_frame().await.unwrap().expect("body frame");
    assert!(String::from_utf8(body.bytes).unwrap().contains("rate_limit_error"));
    assert!(streaming.frames.next_frame().await.unwrap().is_none());
}

#[tokio::test]
async fn mid_stream_http_error_maps_to_llm_transport_error() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_start","message":{"id":"m","model":"x","content":[],"usage":{}}}"#),
        Err(HttpError::Connection("reset".to_string())),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let mut streaming = llm_client::Transport::open_stream(&bridge, &provider_request())
        .await
        .expect("stream");

    assert!(streaming.frames.next_frame().await.unwrap().is_some());
    let error = streaming.frames.next_frame().await.expect_err("mid-stream error");
    assert!(matches!(error, llm_client::LlmError::Transport { message } if message.contains("reset")));
}
```

- [ ] **Step 2: Run tests to verify they pass against Task 4's implementation**

Run: `cargo test -p platforms-common --test llm_transport_test 2>&1 | grep "test result"`
Expected: `7 passed; 0 failed`. If any streaming test fails, fix the bridge (not the test) until green — the streaming code shipped in Task 4 and these tests are its specification.

- [ ] **Step 3: End-to-end sanity through llm-client orchestration**

Append one integration test proving the bridge composes with
`DefaultLlmClient::execute_stream`:

```rust
#[tokio::test]
async fn bridge_drives_llm_client_event_stream_end_to_end() {
    let fake = FakeHttp::default();
    *fake.sse.lock().unwrap() = Some(Ok(vec![
        sse(r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-20250514","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}"#),
        sse(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        sse(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#),
        sse(r#"{"type":"content_block_stop","index":0}"#),
        sse(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}"#),
        sse(r#"{"type":"message_stop"}"#),
    ]));
    let bridge = LlmTransportBridge::new(fake);

    let client = llm_client::client::DefaultLlmClient::from_config(llm_client::ClientConfig {
        providers: vec![llm_client::ProviderProfile {
            provider_id: llm_client::ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            protocol: llm_client::ProtocolFamily::AnthropicMessages,
            auth: llm_client::AuthStrategy::None,
            credential: llm_client::CredentialConfig::None,
            models: vec![llm_client::ModelProfile {
                display_model: "claude-sonnet-4-20250514".to_string(),
                request_model: "claude-sonnet-4-20250514".to_string(),
                billing_model: "claude-sonnet-4".to_string(),
                aliases: vec![],
                capabilities: llm_client::Capabilities {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
            }],
            pricing: llm_client::PricingConfig::default(),
        }],
    })
    .expect("client");

    let mut request = llm_client::LlmRequest::new("claude-sonnet-4-20250514").with_user_text("hello");
    request.stream = true;

    let mut events = client.execute_stream(&request, &bridge).await.expect("stream");
    let mut texts = String::new();
    let mut stop_reason = None;
    while let Some(event) = events.next_event().await.expect("event") {
        match event {
            llm_client::LlmEvent::ContentBlockDelta {
                delta: llm_client::ContentDelta::TextDelta { text },
                ..
            } => texts.push_str(&text),
            llm_client::LlmEvent::MessageDelta { delta, .. } => stop_reason = delta.stop_reason,
            _ => {}
        }
    }
    assert_eq!(texts, "hi");
    assert_eq!(stop_reason.as_deref(), Some("end_turn"));
}
```

Run: `cargo test -p platforms-common --test llm_transport_test 2>&1 | grep "test result"`
Expected: `8 passed; 0 failed`.

- [ ] **Step 4: Clippy + commit**

Run: `cargo clippy -p platforms-common --all-targets 2>&1 | grep -E "^(warning|error)" || true` — expected: no output.

```bash
git add lingxi-code/platforms/common && git commit -m "test(platforms-common): pin bridge streaming semantics end to end

SSE events map 1:1 to RawStreamFrames, status errors surface as
drainable body frames, mid-stream HTTP failures map to
LlmError::Transport, and the bridge drives DefaultLlmClient::
execute_stream through a full anthropic event sequence.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: workspace verification + spec deviation note

**Files:**
- Modify: `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md` (one-line amendment)

- [ ] **Step 1: Full workspace check**

Run: `cargo test -p llm-client -p platforms-common 2>&1 | awk '/test result: ok/ {p+=$4; s+=1} /FAILED/ {f+=1} END {print p" passed / "s" suites / "f+0" failed"}'`
Expected: all green. Then `cargo check --workspace 2>&1 | tail -2` — expected `Finished` with no errors (other crates untouched).

- [ ] **Step 2: Amend the spec's §2 bridge bullet**

Replace the sentence `open_stream` prefers `stream_sse` (...) and falls back to `stream_raw_bytes` + `llm_client::SseFrameSplitter`.` with:

```
  `open_stream` uses `stream_sse` (`SseEvent.data` → `RawStreamFrame`);
  `stream_sse` is a required `HttpTransport` method, so no raw-bytes
  fallback is built (the `SseFrameSplitter` remains available for future
  byte-level hosts).
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md && git commit -m "docs(spec): adoption rev2.1 - bridge is SSE-only (stream_sse is a required method)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## After this plan

Plan 2 (P2b+P3): anthropic-oauth `CredentialProvider` impl + orchestrator policy
ports (retry driver, rate-limit + shared status type, betas, fallback, overflow
copy, cost/telemetry, count_tokens facade). Plan 3 (P4+P5): engine seam swap +
api-client deletion + `modelProviders` settings. Write each against the
then-current tree before executing it.
