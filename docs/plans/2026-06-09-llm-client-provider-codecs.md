# LLM Client Provider Codecs Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add fixture-backed Anthropic, OpenAI Chat/OpenAI-compatible, Gemini, and route/client facade foundations to the reusable `llm-client` crate without depending on old LingXi runtime crates.

**Architecture:** Extend `lingxi-code/llm-client` with pure provider codec modules that encode canonical `LlmRequest` values into provider wire JSON, decode provider responses into canonical `LlmResponse`, and decode stream frames into canonical `LlmEvent`. Keep network I/O behind existing transport/auth seams; route/client facade composes registry resolution, protocol encode/decode, auth, retry, usage, and cost without importing `api-client`, `providers`, `orchestrator`, `telemetry`, old `cost`, or old `protocol`.

**Tech Stack:** Rust 2021, workspace MSRV 1.82, `serde`, `serde_json`, `thiserror`, `futures`, existing `llm-client` modules, file-backed/inline fixtures copied from current parity references. Verification uses `cargo test -p llm-client`, `cargo clippy -p llm-client -- -D warnings`, `cargo test --workspace --no-run`, and `cargo tree -p llm-client`.

---

## Starting Context

- Worktree: `.worktrees/llm-client-codecs`
- Branch: `llm-client-codecs`
- Base: local `main` at `ac44b28a merge: integrate llm-client crate foundation`
- Baseline already checked: `cargo test -p llm-client --no-run` passes.
- Existing foundation crate: `lingxi-code/llm-client`
- Design spec: `docs/superpowers/specs/2026-06-09-llm-client-crate-design.md`
- Foundation plan: `docs/plans/2026-06-09-llm-client-crate.md`

## Hard Constraints

- `llm-client` must not depend on old LingXi runtime crates:
  - `api-client`
  - `providers`
  - `orchestrator`
  - `telemetry`
  - old `cost`
  - old `protocol`
- Provider codecs are pure: no network calls in codec tests.
- Use TDD for every task: write failing test, run it, then implement minimal code.
- Keep provider fixtures deterministic and provider-specific quirks explicit.
- Do not migrate LingXi runtime call sites in this plan.

## Reference Files to Mirror

- Anthropic request/stream reference: `lingxi-code/api-client/src/anthropic.rs`
- API DTO reference: `lingxi-code/api-client/src/types.rs`
- Anthropic wire mapper: `lingxi-code/providers/src/anthropic_wire.rs`
- Anthropic wrapper tests: `lingxi-code/providers/src/anthropic.rs`
- OpenAI codec references:
  - `lingxi-code/providers/src/openai/mod.rs`
  - `lingxi-code/providers/src/openai/encode.rs`
  - `lingxi-code/providers/src/openai/decode.rs`
  - `lingxi-code/providers/src/openai/stream.rs`
  - `lingxi-code/test-harness/tests/parity_openai_codec.rs`
- Gemini codec references:
  - `lingxi-code/providers/src/gemini/mod.rs`
  - `lingxi-code/providers/src/gemini/encode.rs`
  - `lingxi-code/providers/src/gemini/stream.rs`
  - `lingxi-code/test-harness/tests/parity_gemini_codec.rs`
- External API references:
  - Anthropic Messages: `https://platform.claude.com/docs/en/api/messages`
  - Anthropic streaming: `https://platform.claude.com/docs/en/build-with-claude/streaming`
  - OpenAI Chat Completions: `https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create/`
  - OpenAI structured outputs: `https://developers.openai.com/api/docs/guides/structured-outputs`
  - Gemini generateContent: `https://ai.google.dev/api/generate-content`
  - Gemini structured output: `https://ai.google.dev/gemini-api/docs/structured-output`
  - Gemini function calling: `https://ai.google.dev/gemini-api/docs/function-calling`

---

### Task 1: Canonical Stream Events and Response Stop Reasons

**Files:**
- Modify: `lingxi-code/llm-client/src/protocol.rs`
- Test: `lingxi-code/llm-client/tests/protocol_events_test.rs`

**Step 1: Write failing tests for richer stream events**

Create `lingxi-code/llm-client/tests/protocol_events_test.rs`:

```rust
use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDelta};

#[test]
fn stream_events_support_block_lifecycle_and_terminal_delta() {
    let start = LlmEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::Text { text: String::new() },
    };
    let delta = LlmEvent::ContentBlockDelta {
        index: 0,
        delta: ContentDelta::TextDelta { text: "hi".to_string() },
    };
    let stop = LlmEvent::ContentBlockStop { index: 0 };
    let terminal = LlmEvent::MessageDelta {
        delta: MessageDelta { stop_reason: Some("end_turn".to_string()) },
        usage: None,
    };

    assert!(matches!(start, LlmEvent::ContentBlockStart { .. }));
    assert!(matches!(delta, LlmEvent::ContentBlockDelta { .. }));
    assert!(matches!(stop, LlmEvent::ContentBlockStop { .. }));
    assert!(matches!(terminal, LlmEvent::MessageDelta { .. }));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test protocol_events_test
```

Expected: FAIL because `ContentDelta`, `MessageDelta`, and event variants do not exist.

**Step 3: Implement richer canonical stream types**

Modify `lingxi-code/llm-client/src/protocol.rs`:

- Add `ContentDelta` enum:
  - `TextDelta { text: String }`
  - `InputJsonDelta { partial_json: String }`
  - `ThinkingDelta { thinking: String }`
- Add `MessageDelta { stop_reason: Option<String> }`
- Extend `LlmEvent` with:
  - `MessageStart { response: Box<LlmResponse> }`
  - `ContentBlockStart { index: u32, content_block: ContentBlock }`
  - `ContentBlockDelta { index: u32, delta: ContentDelta }`
  - `ContentBlockStop { index: u32 }`
  - `MessageDelta { delta: MessageDelta, usage: Option<Usage> }`
  - keep `TextDelta` and `Completed` only if existing tests still need them; prefer mapping `TextDelta` through content block events in later tasks.

Export the new types from `lib.rs`.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p llm-client --test protocol_events_test
cargo test -p llm-client --test protocol_test
```

Expected: PASS.

---

### Task 2: Provider Codec Trait and Provider Request Envelope

**Files:**
- Modify: `lingxi-code/llm-client/src/protocol.rs`
- Create: `lingxi-code/llm-client/tests/codec_trait_test.rs`

**Step 1: Write failing codec trait tests**

Create `lingxi-code/llm-client/tests/codec_trait_test.rs`:

```rust
use llm_client::{LlmRequest, ProviderRequest, ProviderResponse, WireCodec};

#[derive(Debug)]
struct DummyCodec;

impl WireCodec for DummyCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, llm_client::LlmError> {
        Ok(ProviderRequest::post_json(
            "https://example.test/v1/messages",
            serde_json::json!({"model": request.model}),
        ))
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        assert_eq!(response.status, 200);
        Ok(llm_client::LlmResponse {
            id: "id".to_string(),
            model: "model".to_string(),
            content: vec![],
            usage: Default::default(),
            cost: None,
            provider_metadata: Default::default(),
        })
    }

    fn stream_decoder(&self) -> Box<dyn llm_client::StreamDecoder> {
        Box::new(llm_client::NoopStreamDecoder)
    }
}

#[test]
fn codec_returns_post_json_provider_request() {
    let request = DummyCodec.encode_request(&LlmRequest::new("model-a")).unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://example.test/v1/messages");
    assert_eq!(request.body_json["model"], "model-a");
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test codec_trait_test
```

Expected: FAIL because `WireCodec`, `ProviderRequest`, `ProviderResponse`, and `NoopStreamDecoder` do not exist.

**Step 3: Implement codec envelope types**

In `protocol.rs`, add:

- `ProviderRequest { method, url, headers, body_json }`
- `ProviderResponse { status, headers, body_json, request_id }`
- `WireCodec` trait with `encode_request`, `decode_response`, and `stream_decoder`
- `NoopStreamDecoder` returning no events
- `ProviderRequest::post_json(url, body_json)`

Use `BTreeMap<String, String>` for headers.

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p llm-client --test codec_trait_test
```

Expected: PASS.

---

### Task 3: Anthropic Messages Codec Request Encoding

**Files:**
- Create: `lingxi-code/llm-client/src/providers/mod.rs`
- Create: `lingxi-code/llm-client/src/providers/anthropic.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Test: `lingxi-code/llm-client/tests/anthropic_codec_test.rs`

**Step 1: Write failing Anthropic request-shape test**

Create `lingxi-code/llm-client/tests/anthropic_codec_test.rs`:

```rust
use llm_client::{AnthropicMessagesCodec, ContentBlock, LlmRequest, Message, ToolDeclaration, WireCodec};

#[test]
fn encode_request_shape_is_anthropic_messages() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut request = LlmRequest::new("claude-sonnet-4-20250514");
    request.system = Some("sys".to_string());
    request.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text { text: "hello".to_string() }],
    });
    request.tools.push(ToolDeclaration {
        name: "Read".to_string(),
        description: "Read a file".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    });

    let provider_request = codec.encode_request(&request).unwrap();

    assert_eq!(provider_request.method, "POST");
    assert!(provider_request.url.ends_with("/v1/messages"));
    assert_eq!(provider_request.headers["anthropic-version"], "2023-06-01");
    assert_eq!(provider_request.body_json["model"], "claude-sonnet-4-20250514");
    assert_eq!(provider_request.body_json["system"], "sys");
    assert_eq!(provider_request.body_json["messages"][0]["role"], "user");
    assert_eq!(provider_request.body_json["tools"][0]["input_schema"]["type"], "object");
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test anthropic_codec_test
```

Expected: FAIL because `AnthropicMessagesCodec` does not exist.

**Step 3: Implement minimal Anthropic encoder**

In `providers/anthropic.rs`:

- Create `AnthropicMessagesCodec { base_url, anthropic_version }`
- Implement `WireCodec`
- Encode:
  - URL: `{base_url}/v1/messages`
  - headers: `anthropic-version`, `content-type: application/json`
  - body: `model`, `max_tokens` default 4096 if no explicit field exists yet, `system`, `messages`, `tools`
- Map `ContentBlock::Text` to `{ type: "text", text }`
- Map `ToolDeclaration` to `{ name, description, input_schema }`

Export `AnthropicMessagesCodec` from `lib.rs`.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test anthropic_codec_test
```

Expected: PASS.

---

### Task 4: Anthropic Response Decode and Stream Event Mapping

**Files:**
- Modify: `lingxi-code/llm-client/src/providers/anthropic.rs`
- Test: `lingxi-code/llm-client/tests/anthropic_codec_test.rs`

**Step 1: Add failing response and stream tests**

Append tests:

```rust
use llm_client::{ContentDelta, LlmEvent, ProviderResponse, StreamDecoder};

#[test]
fn decode_text_response_maps_usage_and_stop_reason() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let response = ProviderResponse::json(200, serde_json::json!({
        "id": "msg_1",
        "model": "claude-sonnet-4-20250514",
        "content": [{"type":"text","text":"hi"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 9, "output_tokens": 3}
    }));

    let decoded = codec.decode_response(response).unwrap();

    assert_eq!(decoded.id, "msg_1");
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
}

#[test]
fn stream_decoder_maps_text_delta_and_rejects_garbage() {
    let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
    let mut decoder = codec.stream_decoder();

    let events = decoder.decode_frame(llm_client::RawStreamFrame::new(
        br#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#.to_vec(),
    )).unwrap();

    assert!(matches!(
        &events[0],
        LlmEvent::ContentBlockDelta { index: 0, delta: ContentDelta::TextDelta { text } } if text == "hi"
    ));
    assert!(decoder.decode_frame(llm_client::RawStreamFrame::new(b"not json".to_vec())).is_err());
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test anthropic_codec_test
```

Expected: FAIL because decode/stream mapping is incomplete.

**Step 3: Implement Anthropic decode and stream mapper**

Implement in `providers/anthropic.rs`:

- Decode `content[].type == "text"` into `ContentBlock::Text`
- Decode `content[].type == "tool_use"` into `ContentBlock::ToolCall`
- Normalize `usage` via existing `normalize_anthropic_usage`
- Add `AnthropicStreamDecoder`
- Map stream JSON by `type`:
  - `message_start` -> `LlmEvent::MessageStart`
  - `content_block_start` -> `ContentBlockStart`
  - `content_block_delta` text/input_json/thinking -> `ContentBlockDelta`
  - `content_block_stop` -> `ContentBlockStop`
  - `message_delta` -> `MessageDelta`
  - `message_stop` -> `MessageStop`
  - `ping` -> no events
  - `error` -> `LlmError::ProviderInternal`

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test anthropic_codec_test
```

Expected: PASS.

---

### Task 5: OpenAI Chat Codec Encode/Decode

**Files:**
- Create: `lingxi-code/llm-client/src/providers/openai.rs`
- Modify: `lingxi-code/llm-client/src/providers/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Test: `lingxi-code/llm-client/tests/openai_codec_test.rs`

**Step 1: Write failing OpenAI encode/decode tests**

Create `lingxi-code/llm-client/tests/openai_codec_test.rs` using the parity fixture shape from `test-harness/tests/parity_openai_codec.rs`:

```rust
use llm_client::{ContentBlock, LlmRequest, OpenAiChatCodec, ToolDeclaration, WireCodec, ProviderResponse};

#[test]
fn encode_request_shape_is_openai_chat_completions() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let mut request = LlmRequest::new("gpt-4o");
    request.system = Some("sys".to_string());
    request.tools = vec![ToolDeclaration {
        name: "Read".to_string(),
        description: "d".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    }];

    let provider_request = codec.encode_request(&request).unwrap();

    assert!(provider_request.url.ends_with("/chat/completions"));
    assert_eq!(provider_request.body_json["messages"][0]["role"], "system");
    assert_eq!(provider_request.body_json["tools"][0]["type"], "function");
}

#[test]
fn decode_text_and_tool_responses() {
    let codec = OpenAiChatCodec::new("https://api.openai.com/v1");
    let text = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-x",
        "model":"gpt-4o-2024-08-06",
        "choices":[{"index":0,"message":{"role":"assistant","content":"hi there"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":9,"completion_tokens":3}
    }));
    let decoded = codec.decode_response(text).unwrap();
    assert_eq!(decoded.model, "gpt-4o-2024-08-06");
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
    assert!(matches!(decoded.content[0], ContentBlock::Text { .. }));

    let tool = ProviderResponse::json(200, serde_json::json!({
        "id":"chatcmpl-y",
        "model":"gpt-4o",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]
    }));
    let decoded = codec.decode_response(tool).unwrap();
    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref name, .. } if name == "Bash"));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test openai_codec_test
```

Expected: FAIL because `OpenAiChatCodec` does not exist.

**Step 3: Implement OpenAI encode/decode**

Implement:

- URL: `{base_url}/chat/completions`
- `messages`: system first, then canonical messages
- `tools`: `[{ type: "function", function: { name, description, parameters } }]`
- `usage.prompt_tokens` -> `billable_tokens.input`
- `usage.completion_tokens` -> `billable_tokens.output`
- `choices[0].message.content` -> `ContentBlock::Text`
- `choices[0].message.tool_calls[]` -> `ContentBlock::ToolCall`
- Parse function arguments into JSON; if invalid, preserve as string in metadata or return `InvalidRequest` only when strict mode is later added. For this task parse valid JSON.

Export `OpenAiChatCodec`.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test openai_codec_test
```

Expected: PASS.

---

### Task 6: OpenAI Chat Stream Decoder

**Files:**
- Modify: `lingxi-code/llm-client/src/providers/openai.rs`
- Test: `lingxi-code/llm-client/tests/openai_codec_test.rs`

**Step 1: Add failing OpenAI SSE reassembly test**

Append the parity frames from `test-harness/tests/parity_openai_codec.rs`:

```rust
use llm_client::{ContentDelta, LlmEvent, RawStreamFrame};

#[test]
fn sse_stream_reassembles_text_then_tool_call() {
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"On it. "}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_z","type":"function","function":{"name":"Bash","arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
    let mut decoder = OpenAiChatCodec::new("https://api.openai.com/v1").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    assert!(matches!(events.first(), Some(LlmEvent::MessageStart { .. })));
    let args: String = events.iter().filter_map(|event| match event {
        LlmEvent::ContentBlockDelta { delta: ContentDelta::InputJsonDelta { partial_json }, .. } => Some(partial_json.clone()),
        _ => None,
    }).collect();
    assert_eq!(args, "{\"command\":\"ls\"}");
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test openai_codec_test
```

Expected: FAIL because stream decoder does not reassemble deltas.

**Step 3: Implement OpenAI stream decoder**

Mirror `providers/src/openai/stream.rs` behavior using `llm-client` event types:

- `[DONE]` terminates exactly once
- first valid chunk emits `MessageStart`
- `delta.content` opens text block and emits `TextDelta`
- `delta.reasoning_content` opens reasoning block and emits `ThinkingDelta`
- `delta.tool_calls[index]` opens tool block and emits `InputJsonDelta` fragments
- `finish_reason` maps:
  - `stop` -> `end_turn`
  - `length` -> `max_tokens`
  - `tool_calls` -> `tool_use`
- `usage` chunk maps to `Usage`
- `finish()` emits terminal `MessageDelta` + `MessageStop` if `[DONE]` was missing, but never double-emits.

Add `StreamDecoder::finish(&mut self) -> Result<Vec<LlmEvent>, LlmError>` if the trait does not have it yet, and update old tests.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test openai_codec_test
cargo test -p llm-client --test protocol_test
```

Expected: PASS.

---

### Task 7: Gemini GenerateContent Encode/Decode

**Files:**
- Create: `lingxi-code/llm-client/src/providers/gemini.rs`
- Modify: `lingxi-code/llm-client/src/providers/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Test: `lingxi-code/llm-client/tests/gemini_codec_test.rs`

**Step 1: Write failing Gemini encode/decode tests**

Create `lingxi-code/llm-client/tests/gemini_codec_test.rs` based on `test-harness/tests/parity_gemini_codec.rs`:

```rust
use llm_client::{ContentBlock, GeminiCodec, LlmRequest, ProviderResponse, ToolDeclaration, WireCodec};

#[test]
fn encode_request_shape_is_gemini_generate_content() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let mut request = LlmRequest::new("gemini-2.0-flash");
    request.system = Some("sys".to_string());
    request.tools = vec![ToolDeclaration {
        name: "Read".to_string(),
        description: "d".to_string(),
        input_schema: serde_json::json!({"type":"object"}),
    }];

    let provider_request = codec.encode_request(&request).unwrap();

    assert!(provider_request.url.contains(":generateContent"));
    assert_eq!(provider_request.body_json["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(provider_request.body_json["tools"][0]["functionDeclarations"][0]["name"], "Read");
}

#[test]
fn decode_text_and_function_call() {
    let codec = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta");
    let text = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"text":"hi there"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3}
    }));
    let decoded = codec.decode_response(text).unwrap();
    assert_eq!(decoded.usage.billable_tokens.input, 9);
    assert_eq!(decoded.usage.billable_tokens.output, 3);
    assert!(matches!(decoded.content[0], ContentBlock::Text { .. }));

    let tool = ProviderResponse::json(200, serde_json::json!({
        "modelVersion":"gemini-2.0-flash",
        "candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]
    }));
    let decoded = codec.decode_response(tool).unwrap();
    assert!(matches!(decoded.content[0], ContentBlock::ToolCall { ref name, .. } if name == "Bash"));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test gemini_codec_test
```

Expected: FAIL because `GeminiCodec` does not exist.

**Step 3: Implement Gemini encode/decode**

Implement:

- URL: `{base_url}/models/{model}:generateContent`
- `systemInstruction.parts[0].text`
- `contents[]` from canonical messages
- `tools[].functionDeclarations[]` from `ToolDeclaration`
- `usageMetadata.promptTokenCount` -> input
- `usageMetadata.candidatesTokenCount` -> output
- `parts[].text` -> text block
- `parts[].functionCall` -> tool call block
- finish reason `STOP` -> `end_turn`; functionCall present -> `tool_use`

Export `GeminiCodec`.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test gemini_codec_test
```

Expected: PASS.

---

### Task 8: Gemini Stream Decoder

**Files:**
- Modify: `lingxi-code/llm-client/src/providers/gemini.rs`
- Test: `lingxi-code/llm-client/tests/gemini_codec_test.rs`

**Step 1: Add failing Gemini stream test**

Append parity stream test:

```rust
use llm_client::{LlmEvent, RawStreamFrame};

#[test]
fn sse_stream_reassembles_text_then_function_call() {
    let frames = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"On it."}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
    ];
    let mut decoder = GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta").stream_decoder();
    let mut events = Vec::new();
    for frame in frames {
        events.extend(decoder.decode_frame(RawStreamFrame::new(frame.as_bytes().to_vec())).unwrap());
    }
    events.extend(decoder.finish().unwrap());

    assert!(matches!(events.first(), Some(LlmEvent::MessageStart { .. })));
    assert!(events.iter().any(|event| matches!(event, LlmEvent::ContentBlockStart { content_block: ContentBlock::ToolCall { name, .. }, .. } if name == "Bash")));
    assert!(matches!(events.last(), Some(LlmEvent::MessageStop)));
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test gemini_codec_test
```

Expected: FAIL because stream decoder does not emit tool/text events.

**Step 3: Implement Gemini stream decoder**

Mirror `providers/src/gemini/stream.rs`:

- no `[DONE]` sentinel; terminal events emitted from `finish()`
- first valid chunk emits `MessageStart`
- text parts open text block and emit `TextDelta`
- `thought: true` text parts open reasoning block and emit `ThinkingDelta`
- `functionCall` emits one tool block, one `InputJsonDelta` containing full args JSON, then closes the block
- usage from `usageMetadata`
- terminal stop reason is `tool_use` if a function call was seen

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test gemini_codec_test
```

Expected: PASS.

---

### Task 9: Route Construction and Default Client Facade

**Files:**
- Create: `lingxi-code/llm-client/src/route.rs`
- Create: `lingxi-code/llm-client/src/client.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Test: `lingxi-code/llm-client/tests/client_route_test.rs`

**Step 1: Write failing client/route tests**

Create `lingxi-code/llm-client/tests/client_route_test.rs`:

```rust
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, DefaultLlmClient, LlmRequest,
    ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
};

#[test]
fn client_builds_routes_from_config_and_lists_models() {
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
                capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
        }],
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    assert_eq!(client.available_models().len(), 1);
    assert!(client.prepare(&LlmRequest::new("fast")).is_ok());
}
```

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test client_route_test
```

Expected: FAIL because `DefaultLlmClient` and route preparation do not exist.

**Step 3: Implement route/client facade**

Implement:

- `Route { resolved_route, codec: Box<dyn WireCodec> }`
- `PreparedLlmCall { route, provider_request }`
- `DefaultLlmClient { registry, routes }`
- `DefaultLlmClient::from_config(config)` builds codecs by `ProtocolFamily`:
  - `AnthropicMessages` -> `AnthropicMessagesCodec`
  - `OpenAiChat` -> `OpenAiChatCodec`
  - `GeminiGenerateContent` -> `GeminiCodec`
  - unsupported follow-up families return `UnsupportedCapability`
- `available_models()` delegates to registry
- `prepare(request)` resolves model, validates capabilities, encodes provider request

Do not implement real transport execution here.

**Step 4: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test client_route_test
```

Expected: PASS.

---

### Task 10: Fixture Files and Parity Smoke Matrix

**Files:**
- Create: `lingxi-code/llm-client/tests/fixtures/openai_stream_text_tool.jsonl`
- Create: `lingxi-code/llm-client/tests/fixtures/gemini_stream_text_tool.jsonl`
- Create: `lingxi-code/llm-client/tests/fixtures/anthropic_text_response.json`
- Test: `lingxi-code/llm-client/tests/provider_fixture_smoke_test.rs`

**Step 1: Write failing fixture smoke test**

Create test that loads each fixture with `include_str!`, parses JSON/JSONL, and runs the corresponding codec/decoder.

**Step 2: Run test to verify RED**

Run:

```bash
cargo test -p llm-client --test provider_fixture_smoke_test
```

Expected: FAIL because fixture files do not exist.

**Step 3: Add fixtures**

Use exact frame strings from parity tests:

- OpenAI JSONL frames from `parity_openai_codec.rs`
- Gemini JSONL frames from `parity_gemini_codec.rs`
- Anthropic response JSON from the Anthropic decode test in Task 4

**Step 4: Implement fixture smoke test**

Assert:

- OpenAI stream ends with one `MessageStop`
- Gemini stream ends with one `MessageStop`
- Anthropic response decodes usage correctly

**Step 5: Run test to verify GREEN**

Run:

```bash
cargo test -p llm-client --test provider_fixture_smoke_test
```

Expected: PASS.

---

### Task 11: Final Verification and Dependency Boundary

**Files:**
- All changed files.

**Step 1: Run crate tests**

Run:

```bash
cargo test -p llm-client
```

Expected: PASS.

**Step 2: Run clippy**

Run:

```bash
cargo clippy -p llm-client -- -D warnings
```

Expected: PASS.

**Step 3: Run workspace compile**

Run:

```bash
cargo test --workspace --no-run
```

Expected: PASS.

**Step 4: Confirm dependency boundary**

Run:

```bash
cargo tree -p llm-client
```

Expected: no dependency on `api-client`, `providers`, `orchestrator`, `telemetry`, old `cost`, or old `protocol`.

---

## Out of Scope for This Plan

- No real HTTP transport execution.
- No OAuth refresh loop.
- No AWS/GCP/Azure cloud auth implementation.
- No LingXi orchestrator/sidequery/tool runtime migration.
- No deletion of old provider/api-client code.
- No OpenAI Responses, Bedrock, Vertex, or Azure runtime enablement.

## Follow-Up Plan After This

Write a separate migration plan to wire LingXi composition roots and orchestrator calls through `llm_client::DefaultLlmClient`, then delete old seams only after existing Anthropic parity gates pass.
