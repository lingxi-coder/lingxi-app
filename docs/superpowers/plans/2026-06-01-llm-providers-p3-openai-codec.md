# LLM Providers — P3 (OpenAI Codec) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the OpenAI / OpenAI-compatible `WireCodec` — non-streaming + streaming chat-completions translation with native function-calling — so any `openai`-kind profile (built-in `openai` plus settings-declared Groq/Together/Ollama/vLLM/DeepSeek/OpenRouter endpoints) drives LingXi's agentic loop.

**Architecture:** A pure `OpenAiCodec` (in the `providers` crate) implements P1's `WireCodec`: `encode_request` (canonical → OpenAI `/chat/completions` JSON), `decode_response` (OpenAI response → canonical `MessageResponse`), and `new_stream_decoder` (an `OpenAiSseDecoder` that reassembles OpenAI's index-keyed streaming `tool_calls` fragments into canonical `StreamEvent`s). P2's `ProviderRegistry::build` OpenAi arm changes from the "codec unavailable" error to constructing `GenericClient<OpenAiCodec>`. Parity fixtures in `test-harness` lock the wire shapes.

**Tech Stack:** Rust 1.82.0. `serde_json` (wire JSON), `protocol::{ToolUseId, ConversationMessage, ContentBlock}`, `api_client::types::{MessageResponse, StreamEvent, ContentBlockApi, ContentDelta, UsageApi, MessageDeltaPayload}`, `api_client::ApiError`, `traits::HttpError`. Builds on P1 (`WireCodec`/`SseDecoder`/`GenericClient`/`Auth`/`Capabilities`/`CanonicalRequest`) and P2 (`ProviderRegistry`/`ProviderKind`).

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§4 degradation, §5.1 OpenAI codec, §12 P3).

**Conventions (read first):**
- Run all commands from `lingxi-code/`. Rust 1.82.0.
- Workspace lints: `missing_docs = "warn"` + `clippy::pedantic = "warn"`; gate `cargo clippy -p providers --all-targets -- -D warnings` (providers has no tool-api in its graph, so it lints cleanly). Every `pub` item needs a `///` doc; inherent constructors `#[must_use]`; backtick identifier-like words in docs (e.g. `` `OpenAI` ``) for `doc_markdown`.
- Do NOT modify `traits/` (frozen) or add an `api_client::ApiError` variant.

**Critical wire facts (locked — do not deviate):**
- **Tool-call ids:** `protocol::ToolUseId` is a UUID newtype with NO string constructor. On **decode**, mint `ToolUseId::new()` per OpenAI tool call (discard OpenAI's `call_…` id). On **encode**, render the id as `id.as_uuid().to_string()` for both the assistant `tool_calls[].id` and the matching `tool` message `tool_call_id`. OpenAI only requires intra-request consistency between them — which the orchestrator guarantees by pairing the `ToolResult.tool_use_id` to the `ToolUse.id`.
- **`finish_reason` map:** `"tool_calls"`→`"tool_use"`, `"stop"`→`"end_turn"`, `"length"`→`"max_tokens"`, anything else → passthrough verbatim.
- **System prompt:** `CanonicalRequest.system` (if `Some`) becomes the FIRST message `{role:"system", content}`. `ConversationMessage::System` blocks in history also become `{role:"system", content}`.
- **Default base URL:** `https://api.openai.com/v1`; endpoint = `{base}/chat/completions`. Profile `base_url` overrides the base.
- **Degradation (§4):** canonical `Thinking` blocks are dropped on encode (OpenAI has no thinking-in-history). `is_error` on a `ToolResult` is folded into the `tool` message content (OpenAI has no error flag).

---

## File Structure

**`providers` crate (new):**
- `src/openai/mod.rs` — `OpenAiCodec` (`WireCodec` impl) + `openai_capabilities()` + module wiring.
- `src/openai/encode.rs` — `encode_chat_body(req) -> serde_json::Value` (pure) + `OPENAI_DEFAULT_BASE`.
- `src/openai/decode.rs` — `decode_chat_response(status, body) -> Result<MessageResponse, ApiError>` + `map_finish_reason` (pure).
- `src/openai/stream.rs` — `OpenAiSseDecoder` (`SseDecoder` impl).
- `src/lib.rs` — `pub mod openai;` + re-export `OpenAiCodec`.

**`providers` crate (modify):**
- `src/registry.rs` — OpenAi arm of `build()` constructs `GenericClient<OpenAiCodec>`; update tests.
- `src/capabilities.rs` — add `Capabilities::openai()`.

**`test-harness` crate (new):**
- `tests/parity_openai_codec.rs` — request/response/SSE round-trip fixtures.

---

## Task 1: OpenAI request encoding

**Files:**
- Create: `lingxi-code/providers/src/openai/encode.rs`
- (module wired in Task 4)

- [ ] **Step 1: Write `encode.rs` with tests**

Create `lingxi-code/providers/src/openai/encode.rs`:

```rust
//! Canonical request → `OpenAI` `/chat/completions` request body (pure).

use crate::request::CanonicalRequest;
use protocol::{ContentBlock, ConversationMessage};
use serde_json::{json, Map, Value};

/// Default `OpenAI` API base URL (no trailing slash).
pub const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com/v1";

/// Build the `OpenAI` chat-completions request body for `req`.
///
/// Maps the canonical conversation to `OpenAI`'s `messages` array (system /
/// user / assistant / tool roles), translates canonical tool schemas to
/// `OpenAI` `function` tools, and sets `stream`. `Thinking` blocks are dropped
/// (no `OpenAI` equivalent).
#[must_use]
pub fn encode_chat_body(req: &CanonicalRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = &req.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    for msg in &req.messages {
        encode_message(msg, &mut messages);
    }

    let mut body = Map::new();
    body.insert("model".to_string(), json!(req.model));
    body.insert("max_tokens".to_string(), json!(req.max_tokens));
    body.insert("messages".to_string(), Value::Array(messages));
    if let Some(t) = req.temperature {
        body.insert("temperature".to_string(), json!(t));
    }
    if !req.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(req.tools.iter().map(encode_tool).collect()),
        );
    }
    if req.stream {
        body.insert("stream".to_string(), json!(true));
        // Ask for a final usage chunk on the stream.
        body.insert("stream_options".to_string(), json!({"include_usage": true}));
    }
    Value::Object(body)
}

/// Translate one canonical tool schema (`{name, description, input_schema}`)
/// into an `OpenAI` function tool (`{type:"function", function:{...}}`).
fn encode_tool(tool: &Value) -> Value {
    let name = tool.get("name").cloned().unwrap_or(Value::Null);
    let description = tool.get("description").cloned().unwrap_or(Value::Null);
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters}
    })
}

/// Flatten one canonical message into 0..n `OpenAI` messages.
fn encode_message(msg: &ConversationMessage, out: &mut Vec<Value>) {
    match msg {
        ConversationMessage::System { content, .. } => {
            out.push(json!({"role": "system", "content": content}));
        }
        ConversationMessage::User { content, .. } => encode_user(content, out),
        ConversationMessage::Assistant { content, .. } => encode_assistant(content, out),
    }
}

/// User content: text blocks → one `user` message; tool results → `tool`
/// messages (each `OpenAI` `tool` message answers one `tool_call_id`).
fn encode_user(content: &[ContentBlock], out: &mut Vec<Value>) {
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content: result,
                is_error,
            } => {
                // OpenAI has no is_error flag; prefix on error so the model sees it.
                let body = if *is_error {
                    format!("[error] {result}")
                } else {
                    result.clone()
                };
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id.as_uuid().to_string(),
                    "content": body,
                }));
            }
            // Thinking has no OpenAI history equivalent — drop it.
            ContentBlock::Thinking { .. } => {}
        }
    }
    if !text.is_empty() {
        out.push(json!({"role": "user", "content": text}));
    }
}

/// Assistant content: text + tool-use blocks → one `assistant` message with
/// `content` and/or `tool_calls`.
fn encode_assistant(content: &[ContentBlock], out: &mut Vec<Value>) {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "id": id.as_uuid().to_string(),
                    "type": "function",
                    "function": {"name": name, "arguments": input.to_string()},
                }));
            }
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
        }
    }
    let mut m = Map::new();
    m.insert("role".to_string(), json!("assistant"));
    // OpenAI requires `content` to be present (may be null) on an assistant msg.
    m.insert(
        "content".to_string(),
        if text.is_empty() { Value::Null } else { json!(text) },
    );
    if !tool_calls.is_empty() {
        m.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    out.push(Value::Object(m));
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};

    fn user_text(s: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: s.to_string() }],
        }
    }

    #[test]
    fn system_prompt_becomes_first_message() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.system = Some("be helpful".to_string());
        req.messages = vec![user_text("hi")];
        let body = encode_chat_body(&req);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be helpful");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hi");
        assert_eq!(body["model"], "gpt-4o");
    }

    #[test]
    fn tools_translate_to_function_shape() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.tools = vec![serde_json::json!({
            "name": "Read", "description": "read a file",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        })];
        let body = encode_chat_body(&req);
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "Read");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn assistant_tool_use_and_tool_result_share_id() {
        let id = ToolUseId::new();
        let id_str = id.as_uuid().to_string();
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id,
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/x"}),
            }],
            stop_reason: Some("tool_use".to_string()),
        };
        let user = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "file body".to_string(),
                is_error: false,
            }],
        };
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![assistant, user];
        let body = encode_chat_body(&req);
        let msgs = body["messages"].as_array().unwrap();
        // assistant message with tool_calls
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["tool_calls"][0]["id"], id_str);
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "Read");
        // tool message answering the same id
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], id_str);
        assert_eq!(msgs[1]["content"], "file body");
    }

    #[test]
    fn stream_sets_stream_and_usage_options() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.stream = true;
        let body = encode_chat_body(&req);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn thinking_block_is_dropped() {
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Thinking { thinking: "hmm".to_string(), signature: None },
                ContentBlock::Text { text: "answer".to_string() },
            ],
            stop_reason: None,
        };
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![assistant];
        let body = encode_chat_body(&req);
        assert_eq!(body["messages"][0]["content"], "answer");
        assert!(body["messages"][0].get("thinking").is_none());
    }
}
```

- [ ] **Step 2: Temporarily wire the module to test it**

Add to `lingxi-code/providers/src/lib.rs`: `pub mod openai;` (after `pub mod model_spec;`). Create a minimal `lingxi-code/providers/src/openai/mod.rs` with just `pub mod encode;` for now (Task 4 fills the rest).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers openai::encode` → 5 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/openai providers/src/lib.rs
git commit -m "feat(llm-p3): OpenAI request encoding (messages/roles/tools/tool-calls)"
```

---

## Task 2: OpenAI non-streaming response decoding

**Files:**
- Create: `lingxi-code/providers/src/openai/decode.rs`
- Modify: `lingxi-code/providers/src/openai/mod.rs`

- [ ] **Step 1: Write `decode.rs` with tests**

Create `lingxi-code/providers/src/openai/decode.rs`:

```rust
//! `OpenAI` chat-completions response → canonical `MessageResponse` (pure).

use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
use api_client::ApiError;
use protocol::ToolUseId;
use serde_json::Value;
use traits::HttpError;

/// Map an `OpenAI` `finish_reason` to the canonical stop-reason vocabulary.
#[must_use]
pub fn map_finish_reason(openai: &str) -> String {
    match openai {
        "tool_calls" => "tool_use".to_string(),
        "stop" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        other => other.to_string(),
    }
}

/// Decode a non-streaming `OpenAI` response body.
///
/// # Errors
/// * Non-2xx status → [`ApiError::Server`] (carrying the body for diagnostics).
/// * Unparseable / shape-invalid 2xx → [`ApiError::MalformedStream`].
pub fn decode_chat_response(status: u16, body: &str) -> Result<MessageResponse, ApiError> {
    if !(200..300).contains(&status) {
        return Err(ApiError::Server {
            status,
            body: body.to_string(),
        });
    }
    let root: Value = serde_json::from_str(body)
        .map_err(|e| ApiError::MalformedStream(format!("openai response decode: {e}")))?;

    let id = root
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let choice = root
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ApiError::MalformedStream("openai response: no choices".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| ApiError::MalformedStream("openai response: no message".to_string()))?;

    let mut content: Vec<ContentBlockApi> = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            content.push(ContentBlockApi::Text { text: text.to_string() });
        }
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            // OpenAI `arguments` is a JSON *string*; parse to a Value (fall back
            // to an empty object on malformed partials).
            let input = serde_json::from_str::<Value>(args_str)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
            content.push(ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name,
                input,
            });
        }
    }

    let stop_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(map_finish_reason);

    let usage = decode_usage(root.get("usage"));

    Ok(MessageResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

/// Map `OpenAI` `usage` to canonical `UsageApi`. Cached input tokens (when
/// present under `prompt_tokens_details.cached_tokens`) map to `cache_read`.
fn decode_usage(usage: Option<&Value>) -> UsageApi {
    let Some(u) = usage else {
        return UsageApi::default();
    };
    let input = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    UsageApi {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_RESP: &str = r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#;
    const TOOL_RESP: &str = r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_x","type":"function","function":{"name":"Read","arguments":"{\"path\":\"/x\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":20,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":8}}}"#;

    #[test]
    fn non_2xx_is_server_error() {
        let err = decode_chat_response(429, "rate limited").unwrap_err();
        assert!(matches!(err, ApiError::Server { status: 429, .. }));
    }

    #[test]
    fn text_response_decodes() {
        let r = decode_chat_response(200, TEXT_RESP).unwrap();
        assert_eq!(r.id, "chatcmpl-1");
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        match &r.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 2);
    }

    #[test]
    fn tool_call_response_decodes_with_parsed_input_and_cache() {
        let r = decode_chat_response(200, TOOL_RESP).unwrap();
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        match &r.content[0] {
            ContentBlockApi::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "/x");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
        assert_eq!(r.usage.cache_read_input_tokens, 8);
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason("tool_calls"), "tool_use");
        assert_eq!(map_finish_reason("stop"), "end_turn");
        assert_eq!(map_finish_reason("length"), "max_tokens");
        assert_eq!(map_finish_reason("content_filter"), "content_filter");
    }
}
```

- [ ] **Step 2: Wire the module**

In `lingxi-code/providers/src/openai/mod.rs`: add `pub mod decode;` (after `pub mod encode;`).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers openai::decode` → 4 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/openai
git commit -m "feat(llm-p3): OpenAI non-streaming response decoding + finish_reason map"
```

---

## Task 3: OpenAI SSE decoder (streaming tool-call reassembly)

**Files:**
- Create: `lingxi-code/providers/src/openai/stream.rs`
- Modify: `lingxi-code/providers/src/openai/mod.rs`

This is the hard part: OpenAI streams `choices[].delta` where `tool_calls` arrive as index-keyed fragments (the first fragment for an index carries `id`+`function.name`; later fragments carry `function.arguments` chunks). We reassemble into canonical `StreamEvent`s.

- [ ] **Step 1: Write the test first (TDD)**

Create `lingxi-code/providers/src/openai/stream.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};

    fn run(frames: &[&str]) -> Vec<StreamEvent> {
        let mut d = OpenAiSseDecoder::new();
        let mut out = Vec::new();
        for f in frames {
            out.extend(d.push(f));
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn text_stream_emits_start_delta_stop_and_message_stop() {
        let events = run(&[
            r#"{"id":"c1","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"hel"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        assert!(matches!(events.first(), Some(StreamEvent::MessageStart { .. })));
        // text block opened, two text deltas, then closed
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hel"
        )));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ContentBlockStop { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn tool_call_stream_reassembles_index_keyed_fragments() {
        let events = run(&[
            r#"{"id":"c2","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Read","arguments":""}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\""}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"/x\"}"}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        // A ToolUse block was started with name "Read"
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Read"
        )));
        // Its arguments arrived as InputJsonDelta fragments
        let json_frag: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ContentBlockDelta { delta: ContentDelta::InputJsonDelta { partial_json }, .. } => {
                    Some(partial_json.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(json_frag, "{\"path\":\"/x\"}");
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn done_without_finish_still_terminates() {
        let events = run(&[r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#, "[DONE]"]);
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn ignores_blank_and_role_only_deltas() {
        let events = run(&[
            r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
            "[DONE]",
        ]);
        // Only MessageStart + MessageStop; no spurious content blocks.
        assert!(matches!(events.first(), Some(StreamEvent::MessageStart { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::ContentBlockStart { .. })));
    }
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p providers openai::stream 2>&1 | head -20`
Expected: FAIL — `cannot find ... OpenAiSseDecoder`.

- [ ] **Step 3: Write the decoder above the test module**

Prepend to `lingxi-code/providers/src/openai/stream.rs`:

```rust
//! `OpenAI` streaming SSE → canonical `StreamEvent`s.
//!
//! `OpenAI` streams `choices[0].delta`. Text arrives as `delta.content`
//! fragments; tool calls arrive as `delta.tool_calls[]` fragments keyed by an
//! `index`, where the FIRST fragment for an index carries `id`+`function.name`
//! and later fragments carry `function.arguments` string chunks. This decoder
//! reassembles those into Anthropic-shaped content blocks: a text block at
//! canonical index 0 (opened lazily) and one `tool_use` block per `OpenAI`
//! tool-call index, assigned canonical indices after the text block.

use crate::codec::SseDecoder;
use api_client::types::{
    ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
};
use protocol::ToolUseId;
use serde_json::Value;
use std::collections::BTreeMap;

use super::decode::map_finish_reason;

/// Reassembles an `OpenAI` chat-completions stream into canonical events.
pub struct OpenAiSseDecoder {
    started: bool,
    text_open: bool,
    text_index: u32,
    /// next canonical block index to assign.
    next_index: u32,
    /// `OpenAI` tool-call index → canonical block index (for opened tool blocks).
    tool_index: BTreeMap<u64, u32>,
    stop_reason: Option<String>,
    usage: Option<UsageApi>,
}

impl OpenAiSseDecoder {
    /// Construct a fresh decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: false,
            text_open: false,
            text_index: 0,
            next_index: 0,
            tool_index: BTreeMap::new(),
            stop_reason: None,
            usage: None,
        }
    }

    fn ensure_started(&mut self, root: &Value, out: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let id = root.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let model = root.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        out.push(StreamEvent::MessageStart {
            message: MessageResponse {
                id,
                model,
                content: Vec::new(),
                stop_reason: None,
                usage: UsageApi::default(),
            },
        });
    }

    fn handle_text(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
        if text.is_empty() {
            return;
        }
        if !self.text_open {
            self.text_open = true;
            self.text_index = self.next_index;
            self.next_index += 1;
            out.push(StreamEvent::ContentBlockStart {
                index: self.text_index,
                content_block: ContentBlockApi::Text { text: String::new() },
            });
        }
        out.push(StreamEvent::ContentBlockDelta {
            index: self.text_index,
            delta: ContentDelta::TextDelta { text: text.to_string() },
        });
    }

    fn handle_tool_fragment(&mut self, tc: &Value, out: &mut Vec<StreamEvent>) {
        let Some(oai_idx) = tc.get("index").and_then(Value::as_u64) else {
            return;
        };
        if !self.tool_index.contains_key(&oai_idx) {
            // First fragment for this tool-call index: open a ToolUse block.
            let idx = self.next_index;
            self.next_index += 1;
            self.tool_index.insert(oai_idx, idx);
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            out.push(StreamEvent::ContentBlockStart {
                index: idx,
                content_block: ContentBlockApi::ToolUse {
                    id: ToolUseId::new(),
                    name,
                    input: Value::Object(serde_json::Map::new()),
                },
            });
        }
        let idx = self.tool_index[&oai_idx];
        if let Some(args) = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
        {
            if !args.is_empty() {
                out.push(StreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: ContentDelta::InputJsonDelta { partial_json: args.to_string() },
                });
            }
        }
    }

    fn close_open_blocks(&mut self, out: &mut Vec<StreamEvent>) {
        if self.text_open {
            out.push(StreamEvent::ContentBlockStop { index: self.text_index });
            self.text_open = false;
        }
        let mut tool_indices: Vec<u32> = self.tool_index.values().copied().collect();
        tool_indices.sort_unstable();
        for idx in tool_indices {
            out.push(StreamEvent::ContentBlockStop { index: idx });
        }
        self.tool_index.clear();
    }
}

impl Default for OpenAiSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder for OpenAiSseDecoder {
    fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let data = data.trim();
        if data == "[DONE]" {
            self.close_open_blocks(&mut out);
            out.push(StreamEvent::MessageDelta {
                delta: MessageDeltaPayload { stop_reason: self.stop_reason.clone() },
                usage: self.usage,
            });
            out.push(StreamEvent::MessageStop);
            return out;
        }
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return out; // ignore unparseable keepalive lines
        };
        self.ensure_started(&root, &mut out);

        // Usage-only chunk (empty choices + usage) when include_usage is set.
        if let Some(u) = root.get("usage").filter(|v| !v.is_null()) {
            self.usage = Some(super::decode::usage_from_value(u));
        }

        let Some(choice) = root.get("choices").and_then(|c| c.get(0)) else {
            return out;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.handle_text(text, &mut out);
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in tcs {
                    self.handle_tool_fragment(tc, &mut out);
                }
            }
        }
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(fr));
        }
        out
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        // If the server closed the stream without a `[DONE]` sentinel, still
        // terminate cleanly.
        let mut out = Vec::new();
        if self.started {
            self.close_open_blocks(&mut out);
            out.push(StreamEvent::MessageStop);
        }
        out
    }
}
```

- [ ] **Step 4: Expose `usage_from_value` in `decode.rs`**

The decoder reuses the usage mapping. In `lingxi-code/providers/src/openai/decode.rs`, rename the private `decode_usage(usage: Option<&Value>)` to keep its `Option` API AND add a `pub(crate)` helper taking a non-null `&Value`:

```rust
/// Map a non-null `OpenAI` `usage` object to canonical `UsageApi`.
#[must_use]
pub(crate) fn usage_from_value(u: &Value) -> UsageApi {
    let input = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    UsageApi {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    }
}
```

And change `decode_usage` to delegate: replace its body with `match usage { Some(u) => usage_from_value(u), None => UsageApi::default() }`.

- [ ] **Step 5: Wire the module**

In `lingxi-code/providers/src/openai/mod.rs`: add `pub mod stream;` (after `pub mod decode;`).

- [ ] **Step 6: Test + lint + commit**

Run: `cargo test -p providers openai` → all pass (encode 5 + decode 4 + stream 4).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/openai
git commit -m "feat(llm-p3): OpenAI SSE decoder (index-keyed tool-call reassembly)"
```

---

## Task 4: `OpenAiCodec` (assemble the `WireCodec`)

**Files:**
- Modify: `lingxi-code/providers/src/openai/mod.rs`
- Modify: `lingxi-code/providers/src/capabilities.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Add `Capabilities::openai()`**

In `lingxi-code/providers/src/capabilities.rs`, add (next to `anthropic()`):

```rust
    /// Capabilities for an `OpenAI` / OpenAI-compatible provider (v1: text +
    /// native tools, no vision/cache/thinking-in-history).
    #[must_use]
    pub fn openai() -> Self {
        Self {
            native_tools: true,
            streaming: true,
            vision: false,
            prompt_cache: false,
            reasoning: ReasoningSupport::None,
            parallel_tool_calls: true,
            max_output_tokens: None,
            system_style: SystemStyle::RoleMessage,
        }
    }
```

- [ ] **Step 2: Write `OpenAiCodec` in `mod.rs`**

Replace `lingxi-code/providers/src/openai/mod.rs` with:

```rust
//! `OpenAI` / OpenAI-compatible chat-completions codec.

pub mod decode;
pub mod encode;
pub mod stream;

use crate::auth::Auth;
use crate::codec::{SseDecoder, WireCodec};
use crate::error::CodecError;
use crate::request::CanonicalRequest;
use api_client::types::MessageResponse;
use api_client::ApiError;
use protocol::{HttpMethod, HttpRequest};

use encode::OPENAI_DEFAULT_BASE;

/// `WireCodec` for `OpenAI` chat-completions. `base_url` is the API base (no
/// trailing slash); `None` uses [`OPENAI_DEFAULT_BASE`].
pub struct OpenAiCodec {
    base_url: String,
}

impl OpenAiCodec {
    /// Construct a codec for the given base URL (or the `OpenAI` default).
    #[must_use]
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| OPENAI_DEFAULT_BASE.to_string()),
        }
    }
}

impl WireCodec for OpenAiCodec {
    fn encode_request(&self, req: &CanonicalRequest, auth: &Auth) -> Result<HttpRequest, CodecError> {
        let body = encode::encode_chat_body(req);
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        auth.apply(&mut headers);
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/chat/completions", self.base_url),
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }

    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError> {
        decode::decode_chat_response(status, body)
    }

    fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
        Box::new(stream::OpenAiSseDecoder::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_request_targets_chat_completions_with_bearer() {
        let codec = OpenAiCodec::new(None);
        let auth = Auth::Bearer("sk-test".to_string());
        let req = CanonicalRequest::new("gpt-4o");
        let http = codec.encode_request(&req, &auth).unwrap();
        assert_eq!(http.url, "https://api.openai.com/v1/chat/completions");
        assert!(http
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-test"));
    }

    #[test]
    fn custom_base_url_is_used() {
        let codec = OpenAiCodec::new(Some("https://api.groq.com/openai/v1".to_string()));
        let http = codec
            .encode_request(&CanonicalRequest::new("llama"), &Auth::None)
            .unwrap();
        assert_eq!(http.url, "https://api.groq.com/openai/v1/chat/completions");
    }
}
```

- [ ] **Step 3: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`, add `pub use openai::OpenAiCodec;` (after the `client` re-export).

- [ ] **Step 4: Test + lint + commit**

Run: `cargo test -p providers openai` → all pass (incl. the 2 codec tests).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src
git commit -m "feat(llm-p3): OpenAiCodec (WireCodec) + Capabilities::openai()"
```

---

## Task 5: Wire OpenAI into the registry

**Files:**
- Modify: `lingxi-code/providers/src/registry.rs`

- [ ] **Step 1: Replace the OpenAi arm of `build()`**

In `lingxi-code/providers/src/registry.rs`, the `build` method's `ProviderKind::OpenAi` arm currently returns the "not available until P3" error. Replace that arm with:

```rust
            ProviderKind::OpenAi => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Bearer(key)
                };
                let codec = crate::openai::OpenAiCodec::new(profile.base_url.clone());
                let id = if name == "openai" {
                    cost::ProviderId::OpenAI
                } else {
                    cost::ProviderId::OpenAICompatible { name: name.to_string() }
                };
                let client = crate::client::GenericClient::new(
                    codec,
                    auth,
                    self.transport.clone(),
                    id,
                    crate::capabilities::Capabilities::openai(),
                );
                Ok(std::sync::Arc::new(client) as std::sync::Arc<dyn crate::provider::LlmProvider>)
            }
```

(Leave the `ProviderKind::Gemini` arm unchanged — it still returns the "not available until P4" error.)

- [ ] **Step 2: Update the registry tests**

In `registry.rs`'s test module, the test `openai_profile_errors_codec_unavailable_in_p2` is now WRONG (openai resolves). Replace it with a test that openai now resolves:

```rust
    #[test]
    fn openai_profile_resolves_now() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("openai/gpt-4o").expect("openai resolves in P3");
        assert_eq!(resolved.model, "gpt-4o");
        assert_eq!(resolved.provider.id(), cost::ProviderId::OpenAI);
    }

    #[test]
    fn custom_openai_compatible_profile_resolves() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "groq".to_string(),
            ProviderProfile {
                kind: ProviderKind::OpenAi,
                base_url: Some("https://api.groq.com/openai/v1".to_string()),
                api_key_env: Some("GROQ_API_KEY".to_string()),
            },
        );
        let r = registry(extra);
        let resolved = r.resolve("groq/llama-3.3-70b").expect("groq resolves");
        assert_eq!(resolved.model, "llama-3.3-70b");
        assert_eq!(
            resolved.provider.id(),
            cost::ProviderId::OpenAICompatible { name: "groq".to_string() }
        );
    }
```

Also UPDATE `codec_unavailable_errors_are_repeatable` to use `gemini` instead of `openai` (gemini is still unavailable in P3):

```rust
    #[test]
    fn codec_unavailable_errors_are_repeatable() {
        // gemini failures are not cached, so they keep erroring (until P4).
        let r = registry(BTreeMap::new());
        assert!(r.resolve("gemini/gemini-2.0-flash").is_err());
        assert!(r.resolve("gemini/gemini-2.0-flash").is_err());
    }
```

(Keep `gemini_profile_errors_codec_unavailable_in_p2` as-is — gemini still errors in P3. Its name mentions p2 but the behavior is still correct; leave it.)

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers` → all pass (registry tests now reflect openai-live).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/registry.rs
git commit -m "feat(llm-p3): registry builds GenericClient<OpenAiCodec> for openai + custom profiles"
```

---

## Task 6: Parity fixtures (round-trip the OpenAI wire)

**Files:**
- Create: `lingxi-code/test-harness/tests/parity_openai_codec.rs`

- [ ] **Step 1: Write the fixture-driven test**

Create `lingxi-code/test-harness/tests/parity_openai_codec.rs`:

```rust
//! Parity fixtures for the OpenAI codec — exercise the pure encode/decode/SSE
//! functions against representative wire samples so the shapes stay locked.

use providers::openai::decode::decode_chat_response;
use providers::openai::encode::encode_chat_body;
use providers::openai::stream::OpenAiSseDecoder;
use providers::{Auth, CanonicalRequest, OpenAiCodec, WireCodec};

#[test]
fn encode_request_shape_is_openai_chat_completions() {
    let codec = OpenAiCodec::new(None);
    let mut req = CanonicalRequest::new("gpt-4o");
    req.system = Some("sys".to_string());
    req.tools = vec![serde_json::json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
    let http = codec.encode_request(&req, &Auth::Bearer("k".to_string())).unwrap();
    assert!(http.url.ends_with("/chat/completions"));
    let body: serde_json::Value = serde_json::from_str(http.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["tools"][0]["type"], "function");
}

#[test]
fn decode_real_world_text_and_tool_responses() {
    let text = r#"{"id":"chatcmpl-x","model":"gpt-4o-2024-08-06","choices":[{"index":0,"message":{"role":"assistant","content":"hi there"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#;
    let r = decode_chat_response(200, text).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));

    let tool = r#"{"id":"chatcmpl-y","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    let r = decode_chat_response(200, tool).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn sse_stream_reassembles_text_then_tool_call() {
    use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};
    let frames = [
        r#"{"id":"c","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"On it. "}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_z","type":"function","function":{"name":"Bash","arguments":"{\"command\":"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
    let mut d = OpenAiSseDecoder::new();
    let mut events = Vec::new();
    for f in frames {
        events.extend(d.push(f));
    }
    events.extend(d.finish());

    assert!(matches!(events.first(), Some(StreamEvent::MessageStart { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
    )));
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ContentBlockDelta { delta: ContentDelta::InputJsonDelta { partial_json }, .. } => Some(partial_json.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(args, "{\"command\":\"ls\"}");
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
}
```

- [ ] **Step 2: Ensure the modules are reachable**

The test imports `providers::openai::{decode, encode, stream}` (the `openai` module + submodules are `pub`) and `providers::{OpenAiCodec, WireCodec, Auth, CanonicalRequest}` (re-exported). `WireCodec` must be re-exported from `lib.rs` (it is, from P1). If `test-harness/Cargo.toml` lacks `providers` as a dev-dependency, add `providers = { path = "../providers" }` under `[dev-dependencies]`, and `api-client = { path = "../api-client" }` if not already present.

- [ ] **Step 3: Test + commit**

Run: `cargo test -p test-harness parity_openai_codec` → 3 tests pass.
```bash
git add test-harness/tests/parity_openai_codec.rs test-harness/Cargo.toml
git commit -m "test(llm-p3): OpenAI codec parity fixtures (encode/decode/SSE round-trip)"
```

---

## Task 7: Back-compat gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt -p providers -p test-harness`
Run: `git status --porcelain` — if changed, `git add -A && git commit -m "style(llm-p3): cargo fmt"`.

- [ ] **Step 2: Lint + tests**

Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
Run: `cargo test -p providers` → all pass (P1/P2 + the new openai::encode/decode/stream + codec + registry tests).
Run: `cargo test -p test-harness 2>&1 | grep -E "FAILED|test result:" | tail -25` → 0 failed (incl. `parity_openai_codec` AND the Anthropic back-compat suites still green).

- [ ] **Step 3: Workspace build + deps**

Run: `cargo build --workspace` → Finished.
Run: `bash scripts/check-deps.sh` → `check-deps: OK — 73 workspace crates, no §8.1 dependency violations` (no new crate; `test-harness → providers` dev-dep is allowed).

- [ ] **Step 4: Tag (local only, no push)**

```bash
git tag -a llm-p3 -m "LLM Providers P3 (OpenAI codec): chat-completions encode/decode + SSE tool-call reassembly; openai + custom OpenAI-compatible profiles live. Anthropic unchanged; gemini -> P4."
git --no-pager tag -l "llm-p3"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§4 / §5.1 / §12 P3):**
- OpenAI request encoding (roles, tool_calls, tool results, tools schema, system) → Task 1.
- Non-streaming decode + `finish_reason` map + usage/cache mapping → Task 2.
- Streaming SSE tool-call reassembly (index-keyed fragments → canonical blocks) → Task 3.
- `OpenAiCodec` `WireCodec` impl + `Capabilities::openai()` → Task 4.
- Registry builds `GenericClient<OpenAiCodec>` for `openai` + custom OpenAI-compatible profiles → Task 5.
- Parity fixtures → Task 6.
- Back-compat (Anthropic unchanged; gemini still deferred) → Task 7.
- §4 degradation honored: `Thinking` dropped on encode; `is_error` folded into tool content; cache tokens mapped on decode; vision/prompt_cache `false` in `Capabilities::openai()`.
- *Deferred (correctly):* Azure URL-template/`api-version` variant (§5.1 "small add-on") — a follow-on once the standard codec is proven; image/vision input (locked assumption); Gemini (P4); per-provider cost tables (P5).

**2. Placeholder scan:** none — every code step is complete; every run step has an exact command + expected result. The tool-call id strategy (mint `ToolUseId::new()` on decode, render `id.as_uuid()` on encode) is fully specified because `ToolUseId` has no string constructor.

**3. Type consistency:** `encode_chat_body(&CanonicalRequest)->Value`, `decode_chat_response(u16,&str)->Result<MessageResponse,ApiError>`, `map_finish_reason(&str)->String`, `usage_from_value(&Value)->UsageApi`, `OpenAiSseDecoder::{new,push,finish}`, `OpenAiCodec::new(Option<String>)` are named identically across tasks and match P1's `WireCodec`/`SseDecoder` trait signatures and `GenericClient::new(codec, auth, transport, id, caps)`. The registry arm constructs `GenericClient<OpenAiCodec>` and erases to `Arc<dyn LlmProvider>`, consistent with the Anthropic arm. `StreamEvent`/`ContentBlockApi`/`ContentDelta`/`UsageApi`/`MessageDeltaPayload` are the `api_client::types` the rest of the engine already consumes.
