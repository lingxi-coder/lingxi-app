# LLM Providers — P4 (Gemini Codec) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the native Google Gemini `WireCodec` (`generateContent` + `streamGenerateContent` with native function-calling) so a `gemini`-kind profile drives LingXi's agentic loop — completing the v1 provider set (Anthropic + OpenAI-compatible + Gemini).

**Architecture:** A pure `GeminiCodec` (in the `providers` crate) implements P1's `WireCodec`, exactly mirroring P3's OpenAI codec seam: `encode_request` (canonical → Gemini `contents`/`parts` + `systemInstruction` + `tools.functionDeclarations`), `decode_response` (Gemini `candidates[].content.parts` → canonical `MessageResponse`), and `new_stream_decoder` (a `GeminiSseDecoder` for `:streamGenerateContent?alt=sse`). P2's `ProviderRegistry::build` Gemini arm changes from the "codec unavailable until P4" error to constructing `GenericClient<GeminiCodec>`. After this, all three v1 codecs resolve.

**Tech Stack:** Rust 1.82.0. `serde_json`, `protocol::{ToolUseId, ConversationMessage, ContentBlock}`, `api_client::types::*`, `api_client::ApiError`. Builds on P1 (`WireCodec`/`SseDecoder`/`GenericClient`/`Auth`/`Capabilities`/`CanonicalRequest`), P2 (`ProviderRegistry`/`ProviderKind`), P3 (the proven codec pattern + `OpenAiSseDecoder`'s `done`-flag terminal logic).

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§4 degradation, §5.2 Gemini codec, §12 P4).

**Conventions (read first):**
- Run all commands from `lingxi-code/`. Rust 1.82.0.
- Gate: `cargo clippy -p providers --all-targets -- -D warnings` (providers has no tool-api in its graph → lints cleanly). Every `pub` item needs a `///` doc; inherent constructors `#[must_use]`; backtick identifier-like words (`` `Gemini` ``, `` `OpenAI` ``) for `doc_markdown`.
- Do NOT modify `traits/` (frozen) or add an `api_client::ApiError` variant.

**Gemini wire facts (locked — differ from OpenAI):**
- **URL**: model is in the PATH, and stream vs non-stream use different endpoints. Base default `https://generativelanguage.googleapis.com/v1beta`. Non-stream: `{base}/models/{model}:generateContent`. Stream: `{base}/models/{model}:streamGenerateContent?alt=sse`. The codec picks based on `req.stream`.
- **Auth**: `x-goog-api-key: <key>` header (`Auth::Header { name, value }`, NOT bearer).
- **Roles**: `contents:[{role:"user"|"model", parts:[…]}]` — Assistant→`"model"`, User→`"user"`. No system/assistant/tool roles. `system` (and `ConversationMessage::System`) → top-level `systemInstruction:{parts:[{text}]}`.
- **Tool pairing by NAME, not id:** Gemini has no tool-call ids. `functionCall:{name,args}` (a `model` part) is answered by `functionResponse:{name,response}` (a `user` part) matched on `name`. Canonical `ToolResult` carries only `tool_use_id` (UUID), so the encoder MUST build an `id→name` map from all `ToolUse` blocks in history and look up the name when emitting a `functionResponse`.
- **`functionResponse.response` is an object**, not a string: wrap the canonical `ToolResult.content` string as `{"result": <content>}` (or `{"error": <content>}` when `is_error`).
- **Tool-use stop reason:** Gemini's `finishReason` is `STOP` even when returning a `functionCall`. So canonical `stop_reason` = `"tool_use"` when ANY `functionCall` is present; otherwise map `finishReason` (`STOP`→`end_turn`, `MAX_TOKENS`→`max_tokens`, else passthrough).
- **No `[DONE]` sentinel:** the stream just ends. The terminal `MessageDelta`+`MessageStop` is emitted by `finish()` (reuse P3's `done`-flag idea so it's emitted exactly once). `functionCall` args arrive WHOLE in one chunk (no index-keyed fragment reassembly) — so this decoder is simpler than OpenAI's.
- **Tool-call ids:** as in P3, mint `ToolUseId::new()` on decode (Gemini sends no id); on encode, `functionCall` carries no id at all.
- **Usage:** `usageMetadata.{promptTokenCount→input_tokens, candidatesTokenCount→output_tokens, cachedContentTokenCount→cache_read_input_tokens}`.

---

## File Structure

**`providers` crate (new):**
- `src/gemini/mod.rs` — `GeminiCodec` (`WireCodec`) + module wiring.
- `src/gemini/encode.rs` — `encode_generate_body(req) -> Value` + `GEMINI_DEFAULT_BASE` (pure).
- `src/gemini/decode.rs` — `decode_generate_response(status, body) -> Result<MessageResponse, ApiError>` + `map_finish_reason` + `usage_from_value` (pure).
- `src/gemini/stream.rs` — `GeminiSseDecoder` (`SseDecoder`).
- `src/lib.rs` — `pub mod gemini;` + re-export `GeminiCodec`.

**`providers` crate (modify):**
- `src/registry.rs` — Gemini arm of `build()` constructs `GenericClient<GeminiCodec>`; update tests.
- `src/capabilities.rs` — add `Capabilities::gemini()`.

**`test-harness` crate (new):**
- `tests/parity_gemini_codec.rs` — request/response/SSE round-trip fixtures.

---

## Task 1: Gemini request encoding

**Files:**
- Create: `lingxi-code/providers/src/gemini/encode.rs`
- (module wired in Task 4)

- [ ] **Step 1: Write `encode.rs` with tests**

Create `lingxi-code/providers/src/gemini/encode.rs`:

```rust
//! Canonical request → Google `Gemini` `generateContent` request body (pure).

use crate::request::CanonicalRequest;
use protocol::{ContentBlock, ConversationMessage};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Default `Gemini` API base URL (no trailing slash).
pub const GEMINI_DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Build the `Gemini` `generateContent` request body for `req`.
///
/// Maps the canonical conversation to `Gemini`'s `contents` array (`user` /
/// `model` roles + `functionCall` / `functionResponse` parts), hoists the
/// system prompt to `systemInstruction`, and translates canonical tool schemas
/// to `functionDeclarations`. `Thinking` blocks are dropped.
#[must_use]
pub fn encode_generate_body(req: &CanonicalRequest) -> Value {
    // Gemini pairs functionResponse → functionCall by NAME, but a canonical
    // ToolResult carries only the tool_use_id. Build id → name from every
    // ToolUse block first, so a ToolResult can recover the function name.
    let id_to_name = build_id_name_map(&req.messages);

    let mut contents: Vec<Value> = Vec::new();
    let mut system_text = req.system.clone().unwrap_or_default();

    for msg in &req.messages {
        match msg {
            ConversationMessage::System { content, .. } => {
                if !system_text.is_empty() {
                    system_text.push('\n');
                }
                system_text.push_str(content);
            }
            ConversationMessage::User { content, .. } => {
                if let Some(c) = encode_user(content, &id_to_name) {
                    contents.push(c);
                }
            }
            ConversationMessage::Assistant { content, .. } => {
                if let Some(c) = encode_assistant(content) {
                    contents.push(c);
                }
            }
        }
    }

    let mut body = Map::new();
    body.insert("contents".to_string(), Value::Array(contents));
    if !system_text.is_empty() {
        body.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system_text}]}),
        );
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req.tools.iter().map(encode_tool).collect();
        body.insert("tools".to_string(), json!([{"functionDeclarations": decls}]));
    }
    let mut gen_config = Map::new();
    gen_config.insert("maxOutputTokens".to_string(), json!(req.max_tokens));
    if let Some(t) = req.temperature {
        gen_config.insert("temperature".to_string(), json!(t));
    }
    body.insert("generationConfig".to_string(), Value::Object(gen_config));
    Value::Object(body)
}

/// Map every `ToolUse` block's `id.as_uuid()` string → its function name.
fn build_id_name_map(messages: &[ConversationMessage]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for msg in messages {
        let blocks = match msg {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content,
            ConversationMessage::System { .. } => continue,
        };
        for b in blocks {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                m.insert(id.as_uuid().to_string(), name.clone());
            }
        }
    }
    m
}

/// Canonical tool (`{name, description, input_schema}`) → `Gemini`
/// `functionDeclaration` (`{name, description, parameters}`).
fn encode_tool(tool: &Value) -> Value {
    let name = tool.get("name").cloned().unwrap_or(Value::Null);
    let description = tool.get("description").cloned().unwrap_or(Value::Null);
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    json!({"name": name, "description": description, "parameters": parameters})
}

/// User content → one `{role:"user", parts:[…]}` (text + `functionResponse`).
fn encode_user(content: &[ContentBlock], id_to_name: &BTreeMap<String, String>) -> Option<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"text": text})),
            ContentBlock::ToolResult {
                tool_use_id,
                content: result,
                is_error,
            } => {
                let name = id_to_name
                    .get(&tool_use_id.as_uuid().to_string())
                    .cloned()
                    .unwrap_or_default();
                let response = if *is_error {
                    json!({"error": result})
                } else {
                    json!({"result": result})
                };
                parts.push(json!({"functionResponse": {"name": name, "response": response}}));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(json!({"role": "user", "parts": parts}))
    }
}

/// Assistant content → one `{role:"model", parts:[…]}` (text + `functionCall`).
fn encode_assistant(content: &[ContentBlock]) -> Option<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"text": text})),
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(json!({"functionCall": {"name": name, "args": input}}));
            }
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(json!({"role": "model", "parts": parts}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};

    #[test]
    fn system_prompt_hoisted_to_system_instruction() {
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.system = Some("be helpful".to_string());
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: "hi".to_string() }],
        }];
        let body = encode_generate_body(&req);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be helpful");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn tools_become_function_declarations() {
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.tools = vec![json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
        let body = encode_generate_body(&req);
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "Read");
        assert_eq!(decl["parameters"]["type"], "object");
    }

    #[test]
    fn assistant_tool_use_then_tool_result_pairs_by_name() {
        let id = ToolUseId::new();
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id,
                name: "Bash".to_string(),
                input: json!({"command": "ls"}),
            }],
            stop_reason: Some("tool_use".to_string()),
        };
        let user = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "file1 file2".to_string(),
                is_error: false,
            }],
        };
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.messages = vec![assistant, user];
        let body = encode_generate_body(&req);
        // model turn carries the functionCall
        assert_eq!(body["contents"][0]["role"], "model");
        assert_eq!(body["contents"][0]["parts"][0]["functionCall"]["name"], "Bash");
        assert_eq!(body["contents"][0]["parts"][0]["functionCall"]["args"]["command"], "ls");
        // user turn answers with functionResponse matched on the SAME name
        assert_eq!(body["contents"][1]["role"], "user");
        assert_eq!(body["contents"][1]["parts"][0]["functionResponse"]["name"], "Bash");
        assert_eq!(body["contents"][1]["parts"][0]["functionResponse"]["response"]["result"], "file1 file2");
    }

    #[test]
    fn error_tool_result_uses_error_key() {
        let id = ToolUseId::new();
        let req = CanonicalRequest {
            messages: vec![
                ConversationMessage::Assistant {
                    id: MessageId::new(),
                    content: vec![ContentBlock::ToolUse { id, name: "X".to_string(), input: json!({}) }],
                    stop_reason: None,
                },
                ConversationMessage::User {
                    id: MessageId::new(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: "boom".to_string(),
                        is_error: true,
                    }],
                },
            ],
            ..CanonicalRequest::new("gemini-2.0-flash")
        };
        let body = encode_generate_body(&req);
        assert_eq!(body["contents"][1]["parts"][0]["functionResponse"]["response"]["error"], "boom");
    }
}
```

- [ ] **Step 2: Temporarily wire the module**

Add `pub mod gemini;` to `lingxi-code/providers/src/lib.rs` (after `pub mod error;` or near `pub mod openai;` — keep alphabetical: `gemini` before `model_spec`). Create `lingxi-code/providers/src/gemini/mod.rs` with `//! Google `Gemini` codec.` + `pub mod encode;`.

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers gemini::encode` → 4 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/gemini providers/src/lib.rs
git commit -m "feat(llm-p4): Gemini request encoding (contents/parts, functionCall/Response, systemInstruction)"
```

---

## Task 2: Gemini non-streaming response decoding

**Files:**
- Create: `lingxi-code/providers/src/gemini/decode.rs`
- Modify: `lingxi-code/providers/src/gemini/mod.rs`

- [ ] **Step 1: Write `decode.rs` with tests**

Create `lingxi-code/providers/src/gemini/decode.rs`:

```rust
//! Google `Gemini` `generateContent` response → canonical `MessageResponse`.

use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
use api_client::ApiError;
use protocol::ToolUseId;
use serde_json::Value;

/// Map a `Gemini` `finishReason` to the canonical stop-reason vocabulary.
/// (Tool-use is detected separately by the presence of a `functionCall`.)
#[must_use]
pub fn map_finish_reason(gemini: &str) -> String {
    match gemini {
        "STOP" => "end_turn".to_string(),
        "MAX_TOKENS" => "max_tokens".to_string(),
        other => other.to_string(),
    }
}

/// Decode a non-streaming `Gemini` response body.
///
/// # Errors
/// * Non-2xx → [`ApiError::Server`]. Unparseable / shape-invalid 2xx →
///   [`ApiError::MalformedStream`].
pub fn decode_generate_response(status: u16, body: &str) -> Result<MessageResponse, ApiError> {
    if !(200..300).contains(&status) {
        return Err(ApiError::Server { status, body: body.to_string() });
    }
    let root: Value = serde_json::from_str(body)
        .map_err(|e| ApiError::MalformedStream(format!("gemini response decode: {e}")))?;

    let id = root.get("responseId").and_then(Value::as_str).unwrap_or_default().to_string();
    let model = root.get("modelVersion").and_then(Value::as_str).unwrap_or_default().to_string();

    let candidate = root
        .get("candidates")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ApiError::MalformedStream("gemini response: no candidates".to_string()))?;

    let mut content: Vec<ContentBlockApi> = Vec::new();
    let mut saw_tool = false;
    if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array) {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push(ContentBlockApi::Text { text: text.to_string() });
                }
            } else if let Some(fc) = part.get("functionCall") {
                saw_tool = true;
                let name = fc.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let input = fc.get("args").cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                content.push(ContentBlockApi::ToolUse { id: ToolUseId::new(), name, input });
            }
        }
    }

    // Gemini's finishReason stays STOP even with a functionCall, so detect
    // tool-use by the presence of a functionCall part.
    let stop_reason = if saw_tool {
        Some("tool_use".to_string())
    } else {
        candidate.get("finishReason").and_then(Value::as_str).map(map_finish_reason)
    };

    let usage = usage_from_value(root.get("usageMetadata"));

    Ok(MessageResponse { id, model, content, stop_reason, usage })
}

/// Map `Gemini` `usageMetadata` to canonical `UsageApi`.
#[must_use]
pub fn usage_from_value(usage: Option<&Value>) -> UsageApi {
    let Some(u) = usage else {
        return UsageApi::default();
    };
    UsageApi {
        input_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: u.get("cachedContentTokenCount").and_then(Value::as_u64).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":2}}"#;
    const TOOL: &str = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":5,"cachedContentTokenCount":4}}"#;

    #[test]
    fn non_2xx_is_server_error() {
        assert!(matches!(decode_generate_response(403, "denied").unwrap_err(), ApiError::Server { status: 403, .. }));
    }

    #[test]
    fn text_response_decodes() {
        let r = decode_generate_response(200, TEXT).unwrap();
        assert_eq!(r.model, "gemini-2.0-flash");
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        match &r.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 8);
    }

    #[test]
    fn function_call_decodes_as_tool_use_with_tool_use_stop_reason() {
        let r = decode_generate_response(200, TOOL).unwrap();
        // finishReason is STOP but a functionCall is present → tool_use.
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        match &r.content[0] {
            ContentBlockApi::ToolUse { name, input, .. } => {
                assert_eq!(name, "Bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
        assert_eq!(r.usage.cache_read_input_tokens, 4);
    }

    #[test]
    fn finish_reason_map() {
        assert_eq!(map_finish_reason("STOP"), "end_turn");
        assert_eq!(map_finish_reason("MAX_TOKENS"), "max_tokens");
        assert_eq!(map_finish_reason("SAFETY"), "SAFETY");
    }
}
```

- [ ] **Step 2: Wire the module**

In `lingxi-code/providers/src/gemini/mod.rs`: add `pub mod decode;` (after `pub mod encode;`).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers gemini::decode` → 4 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/gemini
git commit -m "feat(llm-p4): Gemini non-streaming response decoding (functionCall -> tool_use)"
```

---

## Task 3: Gemini SSE decoder

**Files:**
- Create: `lingxi-code/providers/src/gemini/stream.rs`
- Modify: `lingxi-code/providers/src/gemini/mod.rs`

Gemini's `:streamGenerateContent?alt=sse` emits `data:` chunks each shaped like a `generateContent` response (`candidates[0].content.parts` + optional `finishReason` + `usageMetadata`). There is NO `[DONE]` sentinel and `functionCall` parts arrive WHOLE — so this is simpler than OpenAI's decoder (no fragment reassembly). The terminal sequence is emitted by `finish()`.

- [ ] **Step 1: Write the test first (TDD)**

Create `lingxi-code/providers/src/gemini/stream.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent};

    fn run(frames: &[&str]) -> Vec<StreamEvent> {
        let mut d = GeminiSseDecoder::new();
        let mut out = Vec::new();
        for f in frames {
            out.extend(d.push(f));
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn text_stream_terminates_via_finish() {
        let events = run(&[
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hel"}]}}]}"#,
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]}}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":1}}"#,
            r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
        ]);
        assert!(matches!(events.first(), Some(StreamEvent::MessageStart { .. })));
        assert!(events.iter().any(|e| matches!(
            e, StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text }, .. } if text == "hel"
        )));
        // exactly one terminal MessageStop + one MessageDelta (from finish, no [DONE])
        assert_eq!(events.iter().filter(|e| matches!(e, StreamEvent::MessageStop)).count(), 1);
        assert_eq!(events.iter().filter(|e| matches!(e, StreamEvent::MessageDelta { .. })).count(), 1);
        assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
    }

    #[test]
    fn function_call_stream_emits_tool_use_block_and_tool_use_stop() {
        let events = run(&[
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]}"#,
        ]);
        assert!(events.iter().any(|e| matches!(
            e, StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
        )));
        // the whole args arrive as one InputJsonDelta
        let args: String = events.iter().filter_map(|e| match e {
            StreamEvent::ContentBlockDelta { delta: ContentDelta::InputJsonDelta { partial_json }, .. } => Some(partial_json.clone()),
            _ => None,
        }).collect();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&args).unwrap()["command"], "ls");
        // functionCall present → terminal stop_reason is tool_use
        let sr = events.iter().find_map(|e| match e {
            StreamEvent::MessageDelta { delta, .. } => Some(delta.stop_reason.clone()),
            _ => None,
        }).flatten();
        assert_eq!(sr.as_deref(), Some("tool_use"));
    }

    #[test]
    fn unparseable_lines_ignored() {
        let events = run(&["", "not json"]);
        // never started → no events
        assert!(events.is_empty());
    }
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p providers gemini::stream 2>&1 | head -20`
Expected: FAIL — `cannot find ... GeminiSseDecoder`.

- [ ] **Step 3: Write the decoder above the test module**

Prepend to `lingxi-code/providers/src/gemini/stream.rs`:

```rust
//! Google `Gemini` `:streamGenerateContent?alt=sse` → canonical `StreamEvent`s.
//!
//! Each SSE chunk is a `generateContent`-shaped JSON (`candidates[0].content.
//! parts` + optional `finishReason` + `usageMetadata`). Text arrives as
//! incremental `text` parts; `functionCall` parts arrive WHOLE (no fragment
//! reassembly). There is no `[DONE]` sentinel — `finish()` emits the terminal
//! `MessageDelta` + `MessageStop` exactly once.

use crate::codec::SseDecoder;
use api_client::types::{
    ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi,
};
use protocol::ToolUseId;
use serde_json::Value;

use super::decode::{map_finish_reason, usage_from_value};

/// Reassembles a `Gemini` streaming response into canonical events.
pub struct GeminiSseDecoder {
    started: bool,
    text_open: bool,
    text_index: u32,
    next_index: u32,
    saw_tool: bool,
    stop_reason: Option<String>,
    usage: Option<UsageApi>,
    done: bool,
}

impl GeminiSseDecoder {
    /// Construct a fresh decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: false,
            text_open: false,
            text_index: 0,
            next_index: 0,
            saw_tool: false,
            stop_reason: None,
            usage: None,
            done: false,
        }
    }

    fn ensure_started(&mut self, root: &Value, out: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let model = root.get("modelVersion").and_then(Value::as_str).unwrap_or_default().to_string();
        out.push(StreamEvent::MessageStart {
            message: MessageResponse {
                id: String::new(),
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

    fn handle_function_call(&mut self, fc: &Value, out: &mut Vec<StreamEvent>) {
        self.saw_tool = true;
        let index = self.next_index;
        self.next_index += 1;
        let name = fc.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = fc.get("args").cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        out.push(StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name,
                input: Value::Object(serde_json::Map::new()),
            },
        });
        // Gemini sends the whole args at once → one InputJsonDelta, then close.
        out.push(StreamEvent::ContentBlockDelta {
            index,
            delta: ContentDelta::InputJsonDelta { partial_json: args.to_string() },
        });
        out.push(StreamEvent::ContentBlockStop { index });
    }

    fn terminal_stop_reason(&self) -> Option<String> {
        if self.saw_tool {
            Some("tool_use".to_string())
        } else {
            self.stop_reason.clone()
        }
    }
}

impl Default for GeminiSseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder for GeminiSseDecoder {
    fn push(&mut self, data: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let data = data.trim();
        let Ok(root) = serde_json::from_str::<Value>(data) else {
            return out; // ignore blanks / keepalives
        };
        self.ensure_started(&root, &mut out);

        if let Some(u) = root.get("usageMetadata").filter(|v| !v.is_null()) {
            self.usage = Some(usage_from_value(Some(u)));
        }
        let Some(candidate) = root.get("candidates").and_then(|c| c.get(0)) else {
            return out;
        };
        if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.handle_text(text, &mut out);
                } else if let Some(fc) = part.get("functionCall") {
                    self.handle_function_call(fc, &mut out);
                }
            }
        }
        if let Some(fr) = candidate.get("finishReason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(fr));
        }
        out
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        // Gemini has no `[DONE]` — finish() emits the single terminal sequence.
        let mut out = Vec::new();
        if self.started && !self.done {
            self.done = true;
            if self.text_open {
                out.push(StreamEvent::ContentBlockStop { index: self.text_index });
                self.text_open = false;
            }
            out.push(StreamEvent::MessageDelta {
                delta: MessageDeltaPayload { stop_reason: self.terminal_stop_reason() },
                usage: self.usage,
            });
            out.push(StreamEvent::MessageStop);
        }
        out
    }
}
```

- [ ] **Step 4: Wire the module**

In `lingxi-code/providers/src/gemini/mod.rs`: add `pub mod stream;` (after `pub mod decode;`).

- [ ] **Step 5: Test + lint + commit**

Run: `cargo test -p providers gemini` → all pass (encode 4 + decode 4 + stream 3).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/gemini
git commit -m "feat(llm-p4): Gemini SSE decoder (whole functionCall; finish()-only terminal)"
```

---

## Task 4: `GeminiCodec` (assemble the `WireCodec`)

**Files:**
- Modify: `lingxi-code/providers/src/gemini/mod.rs`
- Modify: `lingxi-code/providers/src/capabilities.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Add `Capabilities::gemini()`**

In `lingxi-code/providers/src/capabilities.rs`, add (next to `openai()`):

```rust
    /// Capabilities for a native `Gemini` provider (v1: text + native tools,
    /// no vision/cache/thinking-in-history).
    #[must_use]
    pub fn gemini() -> Self {
        Self {
            native_tools: true,
            streaming: true,
            vision: false,
            prompt_cache: false,
            reasoning: ReasoningSupport::None,
            parallel_tool_calls: true,
            max_output_tokens: None,
            system_style: SystemStyle::TopLevel,
        }
    }
```

- [ ] **Step 2: Write `GeminiCodec` in `mod.rs`**

Replace `lingxi-code/providers/src/gemini/mod.rs` with:

```rust
//! Google `Gemini` `generateContent` codec.

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

use encode::GEMINI_DEFAULT_BASE;

/// `WireCodec` for the native `Gemini` API. `base_url` is the API base (no
/// trailing slash); `None` uses [`GEMINI_DEFAULT_BASE`]. The model goes in the
/// URL path, and streaming uses a different endpoint than non-streaming.
pub struct GeminiCodec {
    base_url: String,
}

impl GeminiCodec {
    /// Construct a codec for the given base URL (or the `Gemini` default).
    #[must_use]
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            base_url: base_url.unwrap_or_else(|| GEMINI_DEFAULT_BASE.to_string()),
        }
    }
}

impl WireCodec for GeminiCodec {
    fn encode_request(&self, req: &CanonicalRequest, auth: &Auth) -> Result<HttpRequest, CodecError> {
        let body = encode::encode_generate_body(req);
        let url = if req.stream {
            format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url, req.model)
        } else {
            format!("{}/models/{}:generateContent", self.base_url, req.model)
        };
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        auth.apply(&mut headers);
        if req.stream {
            headers.push(("accept".to_string(), "text/event-stream".to_string()));
        }
        Ok(HttpRequest {
            method: HttpMethod::Post,
            url,
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(600)),
        })
    }

    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError> {
        decode::decode_generate_response(status, body)
    }

    fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
        Box::new(stream::GeminiSseDecoder::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_stream_url_has_model_and_generate_content() {
        let codec = GeminiCodec::new(None);
        let auth = Auth::Header { name: "x-goog-api-key".to_string(), value: "k".to_string() };
        let req = CanonicalRequest::new("gemini-2.0-flash");
        let http = codec.encode_request(&req, &auth).unwrap();
        assert_eq!(
            http.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert!(http.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "k"));
    }

    #[test]
    fn stream_url_uses_stream_endpoint() {
        let codec = GeminiCodec::new(None);
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.stream = true;
        let http = codec.encode_request(&req, &Auth::None).unwrap();
        assert!(http.url.ends_with("/models/gemini-2.0-flash:streamGenerateContent?alt=sse"));
    }
}
```

- [ ] **Step 3: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`, add `pub use gemini::GeminiCodec;` (next to the `OpenAiCodec` re-export).

- [ ] **Step 4: Test + lint + commit**

Run: `cargo test -p providers gemini` → all pass (incl. the 2 codec tests).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src
git commit -m "feat(llm-p4): GeminiCodec (WireCodec; model-in-path URL, x-goog-api-key) + Capabilities::gemini()"
```

---

## Task 5: Wire Gemini into the registry

**Files:**
- Modify: `lingxi-code/providers/src/registry.rs`

- [ ] **Step 1: Replace the Gemini arm of `build()`**

In `lingxi-code/providers/src/registry.rs`, the `build` method's `ProviderKind::Gemini` arm currently returns the "not available until P4" error. Replace it with:

```rust
            ProviderKind::Gemini => {
                let key = self.api_key_for(profile);
                let auth = if key.is_empty() {
                    crate::auth::Auth::None
                } else {
                    crate::auth::Auth::Header {
                        name: "x-goog-api-key".to_string(),
                        value: key,
                    }
                };
                let codec = crate::gemini::GeminiCodec::new(profile.base_url.clone());
                let client = crate::client::GenericClient::new(
                    codec,
                    auth,
                    self.transport.clone(),
                    cost::ProviderId::GoogleGemini,
                    crate::capabilities::Capabilities::gemini(),
                );
                Ok(std::sync::Arc::new(client) as std::sync::Arc<dyn crate::provider::LlmProvider>)
            }
```

(The `OpenAi` and `Anthropic` arms are unchanged. After this, no `ProviderKind` arm returns an "unavailable" error.)

- [ ] **Step 2: Update the registry tests**

In `registry.rs`'s test module:
- Replace `gemini_profile_errors_codec_unavailable_in_p2` with:
```rust
    #[test]
    fn gemini_profile_resolves_now() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("gemini/gemini-2.0-flash").expect("gemini resolves in P4");
        assert_eq!(resolved.model, "gemini-2.0-flash");
        assert_eq!(resolved.provider.id(), cost::ProviderId::GoogleGemini);
    }
```
- DELETE `codec_unavailable_errors_are_repeatable` (no codec is unavailable after P4). The `unknown_profile_errors` test still covers the only remaining error path (an unconfigured profile name).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers` → all pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/registry.rs
git commit -m "feat(llm-p4): registry builds GenericClient<GeminiCodec> for gemini profiles"
```

---

## Task 6: Parity fixtures

**Files:**
- Create: `lingxi-code/test-harness/tests/parity_gemini_codec.rs`

- [ ] **Step 1: Write the fixture-driven test**

Create `lingxi-code/test-harness/tests/parity_gemini_codec.rs`:

```rust
//! Parity fixtures for the Gemini codec — exercise encode/decode/SSE against
//! representative wire samples so the shapes stay locked.

use providers::gemini::decode::decode_generate_response;
use providers::gemini::stream::GeminiSseDecoder;
use providers::{Auth, CanonicalRequest, GeminiCodec, SseDecoder, WireCodec};

#[test]
fn encode_request_shape_is_gemini_generate_content() {
    let codec = GeminiCodec::new(None);
    let mut req = CanonicalRequest::new("gemini-2.0-flash");
    req.system = Some("sys".to_string());
    req.tools = vec![serde_json::json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
    let http = codec
        .encode_request(&req, &Auth::Header { name: "x-goog-api-key".to_string(), value: "k".to_string() })
        .unwrap();
    assert!(http.url.contains(":generateContent"));
    let body: serde_json::Value = serde_json::from_str(http.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
    assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "Read");
}

#[test]
fn decode_real_world_text_and_function_call() {
    let text = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"text":"hi there"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3}}"#;
    let r = decode_generate_response(200, text).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));

    let tool = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]}"#;
    let r = decode_generate_response(200, tool).unwrap();
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn sse_stream_reassembles_text_then_function_call() {
    use api_client::types::{ContentBlockApi, StreamEvent};
    let frames = [
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"On it."}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2}}"#,
    ];
    let mut d = GeminiSseDecoder::new();
    let mut events = Vec::new();
    for f in frames {
        events.extend(d.push(f));
    }
    events.extend(d.finish());

    assert!(matches!(events.first(), Some(StreamEvent::MessageStart { .. })));
    assert!(events.iter().any(|e| matches!(
        e, StreamEvent::ContentBlockStart { content_block: ContentBlockApi::ToolUse { name, .. }, .. } if name == "Bash"
    )));
    assert_eq!(events.iter().filter(|e| matches!(e, StreamEvent::MessageStop)).count(), 1);
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop)));
}
```

- [ ] **Step 2: Test + commit**

Run: `cargo test -p test-harness --test parity_gemini_codec` → 3 tests pass.
(`providers` is already a `test-harness` dev-dep from P3; `SseDecoder` is re-exported from P1.)
```bash
git add test-harness/tests/parity_gemini_codec.rs
git commit -m "test(llm-p4): Gemini codec parity fixtures (encode/decode/SSE round-trip)"
```

---

## Task 7: Back-compat gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt -p providers -p test-harness`
Run: `git status --porcelain` — if changed (ignore any stray untracked `test_entry`), `git add` the tracked fmt'd files and `git commit -m "style(llm-p4): cargo fmt"`.

- [ ] **Step 2: Lint + tests**

Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
Run: `cargo test -p providers` → all pass (P1/P2/P3 + new gemini encode/decode/stream + codec + registry tests).
Run: `cargo test -p test-harness --test parity_gemini_codec` → 3 pass.
Run: `cargo test -p test-harness 2>&1 | grep -E "FAILED|error\[|test result:" | grep -v "0 failed" | head` → empty (no failures; Anthropic + OpenAI parity suites still green).

- [ ] **Step 3: Workspace build + deps**

Run: `cargo build --workspace` → Finished.
Run: `bash scripts/check-deps.sh` → `check-deps: OK — 73 workspace crates, no §8.1 dependency violations`.

- [ ] **Step 4: Tag (local only, no push)**

```bash
git tag -a llm-p4 -m "LLM Providers P4 (Gemini codec): generateContent encode/decode + streamGenerateContent SSE; gemini profiles live. All three v1 codecs (anthropic/openai/gemini) resolve. Anthropic unchanged."
git --no-pager tag -l "llm-p4"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§4 / §5.2 / §12 P4):**
- Gemini request encoding (roles, functionCall/functionResponse, systemInstruction, functionDeclarations, id→name pairing) → Task 1.
- Non-streaming decode (functionCall→tool_use, finishReason map, usageMetadata) → Task 2.
- Streaming decoder (whole functionCall, no `[DONE]`, finish()-only terminal) → Task 3.
- `GeminiCodec` `WireCodec` (model-in-path URL, stream vs non-stream endpoint, `x-goog-api-key`) + `Capabilities::gemini()` → Task 4.
- Registry builds `GenericClient<GeminiCodec>` → Task 5.
- Parity fixtures → Task 6.
- Back-compat (Anthropic + OpenAI unchanged; all three codecs now resolve) → Task 7.
- §4 degradation: `Thinking` dropped on encode; `is_error` → `{"error": …}`; vision/prompt_cache `false` in `Capabilities::gemini()`; cache tokens mapped on decode.
- *Deferred (correctly):* Gemini thinking-trace → canonical `Thinking` mapping (reasoning v1 non-goal); image input (locked assumption); Vertex AI base + signed auth (§8 non-goal); restricted-schema normalization beyond pass-through (documented caveat); per-provider cost tables (P5).

**2. Placeholder scan:** none — every code step is complete; every run step has an exact command + expected result. The two Gemini-specific subtleties (functionResponse pairs by NAME via the `id→name` map; `stop_reason=tool_use` detected by functionCall presence, not finishReason) are fully implemented.

**3. Type consistency:** `encode_generate_body(&CanonicalRequest)->Value`, `decode_generate_response(u16,&str)->Result<MessageResponse,ApiError>`, `map_finish_reason(&str)->String`, `usage_from_value(Option<&Value>)->UsageApi`, `GeminiSseDecoder::{new,push,finish}`, `GeminiCodec::new(Option<String>)` mirror the P3 OpenAI naming and match P1's `WireCodec`/`SseDecoder` trait signatures + `GenericClient::new(codec, auth, transport, id, caps)`. The registry arm builds `GenericClient<GeminiCodec>` and erases to `Arc<dyn LlmProvider>`, exactly as the Anthropic + OpenAI arms do. `ProviderId::GoogleGemini` is the variant the cost layer already defines (P5 consumes it).
