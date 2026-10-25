# Requests & execution

Compose typed model input and control each physical attempt with draft, prepare, dispatch, and collect.

<a id="chat-request"></a>

## ChatRequest

ChatRequest::new(model) initializes empty conversation input and default optional controls. Populate messages with ConversationMessage and ContentBlock. Use system blocks, tools and tool_choice for function tools, hosted_tools for provider-executed tools, and output_format for output contracts.

controls carries explicit wire controls; thinking and service_tier are validated against the chosen model and protocol. continuation is scoped to the original account, endpoint, profile and model. Native content and signatures must be preserved when replaying assistant turns.

### Typed input · excerpt / 类型化输入片段

```rust
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage};

let mut request = ChatRequest::new("gpt-4.1-mini");
request.messages.push(ConversationMessage::user_text("Explain ownership in Rust."));
request.max_tokens = Some(1024);
```

Verification method: API excerpt

### ChatRequest::new

```text
pub fn new(model: impl Into<String>) -> Self
```

Construct model input with no conversation or optional overrides.

### ConversationMessage::user_text

```text
pub fn user_text(text: impl Into<String>) -> Self
```

Create a user message containing one plain text block.

### ChatService::complete_in

```text
pub async fn complete_in(self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<ChatResponse, LlmError>
```

Select the starting profile explicitly. complete() instead resolves a route by model.

<a id="input-field-guide"></a>

## Common input fields and defaults

| Field | Meaning / default | When to set it |
| --- | --- | --- |
| model | Configured model selector; ChatRequest::new requires it | Choose from the selected connection rather than passing a display label by guess |
| messages | Empty ordered conversation input | First user turn plus full assistant/tool turns for stateless replay |
| max_tokens | Optional output ceiling, not an input context limit | Bound response size; allow enough room for reasoning where supported |
| tools / tool_choice | No functions; ToolChoice::Auto | Advertise host functions and choose Auto, Any, None or one named Tool |
| output_format | OutputFormat::Text | Ask for a JSON object or schema-constrained final answer |
| continuation | No provider state reference | Use a completed, same-account ContinuationRef and send only new input |

Optional controls are validated by the selected model and protocol. An explicitly unsupported option is an error; the client does not silently translate every provider feature into another wire format.

<a id="request-option-guide"></a>

## Options that should be request-scoped

Changing a model request must not mutate a global key, timeout or reasoning setting shared by other sessions. Pass a fresh RequestOptions for each application request and reuse the long-lived client handle.

| Option | Application contract |
| --- | --- |
| credential | A valid secret for the primary connection; the host refreshes and stores it |
| fallback_credentials | Keys indexed by exact fallback profile; never an implicit copy of the primary key |
| total_timeout | Whole-operation budget including reads. Streams need an explicit value for a total deadline |
| account_scope | Stable non-secret identity for provider continuation; use the same account on the next turn |
| file_account_scope | Independent account identity for eligible provider upload reuse |
| mcp_authorizations | Fresh remote-server credentials indexed by declared label/name; keep them out of history |

<a id="function-tool-roundtrip"></a>

## Complete a function-tool round trip

A function definition describes what the model may request; it does not execute code. The host validates a completed ToolUse result, runs its registered implementation, adds a ToolResult with the original call ID, and makes the next request. Keep a round limit, cancellation path and permission decision in the host.

This source-checked example exposes one harmless in-memory article lookup. The request must already contain a user message, and the selected route must support strict tool input. It rejects unregistered/namespaced calls and malformed input rather than executing model-supplied names. It does not implement a general Agent runtime.

### Bounded tool loop · host-owned implementation

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{
    ChatRequest, ChatResponse, ContentBlock, ConversationMessage,
    MessageRole, StopReason, ToolChoice, ToolSpec,
};

pub async fn answer_with_lookup(
    client: &LlmClient,
    profile: &str,
    mut request: ChatRequest,
    options: &RequestOptions,
) -> Result<ChatResponse, Box<dyn std::error::Error>> {
    request.tools = vec![serde_json::from_str::<ToolSpec>(r#"{
        "name":"lookup_article", "description":"Read one local help article by ID.",
        "input_schema":{"type":"object","properties":{"article":{"type":"string"}},
            "required":["article"],"additionalProperties":false}, "strict":true
    }"#)?];
    request.tool_choice = ToolChoice::Auto;
    for _ in 0..3 {
        let response = client.chat().complete_in(profile, &request, options).await?;
        if !response.tool_input_issues.is_empty() {
            return Err("tool input was not valid JSON".into());
        }
        if response.stop_reason != StopReason::ToolUse { return Ok(response); }
        let mut results = Vec::new();
        for (id, namespace, name, input) in response.message.tool_uses() {
            if namespace.is_some() || name != "lookup_article" {
                return Err("tool is not registered by this host".into());
            }
            let article = input.string_at("/article").ok_or("article must be a string")?;
            let found = article == "quickstart";
            results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: if found { "Use the LLM Client quickstart." } else { "Article not found." }.into(),
                is_error: Some(!found), blocks: None, cache_reference: None, toolset_name: None,
            });
        }
        if results.is_empty() { return Err("tool-use response has no callable tool".into()); }
        request.messages.push(response.message);
        request.messages.push(ConversationMessage {
            role: MessageRole::User, content: results, native_options: Vec::new(),
        });
    }
    Err("tool round limit reached".into())
}
```

Verification method: Compile check (llm-function-tool-roundtrip)

- [Harness tools and permissions](harness-tools.md)
- [Structured tool and output contracts](llm-controls.md)

Preserve the complete assistant message before appending tool results: tool IDs, native provider content and reasoning signatures may be required by the next request. For permissions, tool scheduling and persistent sessions, integrate Harness instead.

<a id="request-options"></a>

## RequestOptions

- credential authenticates the primary profile. fallback_credentials supplies separate keys for named fallback profiles; the primary key is never implicitly forwarded.
- account_scope is a stable nonsecret identity used for continuation. file_account_scope independently scopes reusable provider uploads.
- total_timeout bounds preparation and response reads. Ordinary completion defaults to 120 seconds, or two hours with video; streams have no total deadline by default.
- finalizer applies synchronous host policy after encoding and before authentication. It must not dispatch requests or fetch credentials.

<a id="prepared-call"></a>

## One attempt, one dispatch

prepare_on requires an exact connection name, freezes its model row and authenticates the request. Preparation may upload attachments, but it sends no generation request. PreparedCall is immutable and cannot be cloned; dispatch consumes it. Retrying or switching connections requires fresh preparation.

### Host execution · excerpt with existing client/request/options / 宿主执行片段

```rust
use lingxi_llm_client::codecs::RequestMode;

let snapshot = client.snapshot();
let call = snapshot.prepare_on("openai", &request, &options, RequestMode::Complete).await?;
let prices = call.pricing_snapshot();
let received = call.dispatch_once().await?;
let collected = received.collect().await?;
let usage = collected.usage_report().clone();
let inference = collected.inference_report().clone();
let answer = collected.decode();
collected.finish().await;
let response = answer?;
```

Verification method: API excerpt

### ClientSnapshot::prepare_on

```text
pub async fn prepare_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<PreparedCall, LlmError>
```

Prepare exactly one connection using a retained configuration snapshot.

### PreparedCall::pricing_snapshot

```text
pub fn pricing_snapshot(&self) -> FrozenPricing
```

Retain the selected prices before consuming the call, including on cancellation.

### PreparedCall::dispatch_once

```text
pub async fn dispatch_once(self) -> Result<ReceivedCall, LlmError>
```

Send at most one generation request with no retry or connection fallback.

### ReceivedCall::collect

```text
pub async fn collect(self) -> Result<CollectedResponse, LlmError>
```

Collect raw response and accounting facts before decoding application output.

Retain usage and inference before decode(): unsuccessful status or malformed tool JSON can still carry accounting observations. The host decides how to persist those facts and uses finish() for attachment cleanup.

<a id="draft-admission"></a>

## Drafts & host admission

### ClientSnapshot::prepare_draft_on

```text
pub async fn prepare_draft_on(&self, profile: &str, request: &ChatRequest, options: &RequestOptions, mode: RequestMode) -> Result<RequestDraft, LlmError>
```

Resolve and encode without signing the generation request. Drafts use only the explicitly supplied total timeout.

### RequestDraft::request_mut

```text
pub fn request_mut(&mut self) -> &mut HttpRequest
```

Apply the final host transformation before authentication.

### RequestDraft::seal

```text
pub async fn seal(self) -> Result<PreparedCall, LlmError>
```

Authenticate final bytes and consume the draft. A draft cannot dispatch directly.

### PreparedCall::dispatch_once_with

```text
pub async fn dispatch_once_with(self, on_dispatch: impl FnOnce() -> Result<(), LlmError> + Send) -> Result<ReceivedCall, LlmError>
```

Run a synchronous admission marker immediately before sending; rejection causes no send.

### LlmClient::count_tokens_exact_in

```text
pub async fn count_tokens_exact_in(&self, profile: &str, request: &ChatRequest, options: &RequestOptions) -> Result<Option<u64>, LlmError>
```

Use Anthropic exact input counting. Unsupported protocols return None; counting errors stay errors and no answer is generated.

<a id="wire-pipeline"></a>

## From typed input to transport

1. Capture configuration, resolve model/profile/region/protocol and check explicit controls, continuation, hosted-tool and file constraints.
2. Resolve host attachments, choose inline/provider-file binding and capture request-local file provenance. A generation preflight can perform attachment I/O and uploads.
3. WireCodec encodes the selected protocol using CodecContext/EncodeRequest. Apply the host finalizer and authenticate the final bytes once.
4. Transport returns status/headers and a raw body stream. For completion collect then decode; for streaming use the codec decoder and accounting observations per chunk.
5. Return canonical content, usage, inference facts and scoped continuation. The host executes local tools, owns transcript persistence, chooses another round and performs cleanup.

![Model request from snapshot and route admission through attachments, codec, authentication and transport to decoded events and host tools](../assets/llm-request-flow.svg)

The SDK owns provider communication. Harness owns tool execution, permissions, history and attempt accounting.

A request admission failure can occur before transport; a response parsing failure can occur after the provider has accepted and charged a call. Do not use successful JSON decoding as the boundary for recording a physical attempt.

<a id="input-replay"></a>

## Tools, native content and continuation

ConversationMessage carries canonical ContentBlock values and protocol-tagged native_options. Text, thinking, signatures, tool IDs and exact tool argument JSON have distinct replay roles. ProviderContent belongs to its original protocol and is rejected on incompatible replay; raw ProviderEvent observations are not transcript content.

A completed response produces a reusable ContinuationRef only with stable account_scope. The next request carries that reference and only new input. It must use the original executed profile, endpoint, model and account; raw response_id alone is not a portable continuation.

- [Attempt accounting and retry policy](llm-execution-accounting.md)
- [Attachments and provider files](llm-files.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/options.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/prepared.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/executor.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/requests.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/llm.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/codecs/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/transport.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/execution_contracts.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/chat.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/message.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/json_payload.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/continuation.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
