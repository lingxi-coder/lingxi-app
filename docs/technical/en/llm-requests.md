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

- [llm-client/src/client/options.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/options.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/prepared.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/prepared.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/executor.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/client/requests.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/requests.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/protocol/llm.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/codecs/mod.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/codecs/mod.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/transport.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/transport.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/execution_contracts.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/execution_contracts.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)

- llm-client: HEAD 179d336607b2367cf0709a78c760c9333ec25bb3; Referenced input fingerprint sha256:64423ae6bcb4985bab0883b693378509bb7fe060e59bf4facafedcb4d2f32293; Product pin 450d52b12b6d88a377ab3332c3b8a6a1820e2b7c

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
