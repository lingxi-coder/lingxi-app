# Streaming & cancellation

Read normalized events or per-chunk observations while preserving tools, native content, usage, and terminal errors.

<a id="events"></a>

## Consume events

ModelStream exposes its own asynchronous next(); no StreamExt import is required. Errors can occur when opening the stream or reading it. Read through None, including after End, before treating reports as final.

### Complete streaming function

```rust
use lingxi_llm_client::protocol::{ChatRequest, StreamEvent};
use lingxi_llm_client::{LlmClient, RequestOptions};

async fn stream_text(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = client.chat().stream_in("openai", request, options).await?;
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text, .. } => print!("{text}"),
            StreamEvent::End { stop_reason, .. } => eprintln!("{stop_reason:?}"),
            _ => {}
        }
    }
    eprintln!("Profile: {}", stream.executed_profile());
    eprintln!("Usage: {:?}", stream.usage_report());
    eprintln!("Complete: {}", stream.usage_is_complete());
    Ok(())
}
```

Verification method: Compile check (llm-stream-text)

This display-only example ignores nontext events. Tool-enabled applications must retain tool calls, reasoning signatures, native provider content, and block boundaries when constructing the next assistant turn.

<a id="event-handling-map"></a>

## What to do with each event family

| Event | Use | Do not infer |
| --- | --- | --- |
| Start | Retain model and optional response identity | A final answer or a reusable continuation |
| TextDelta / TextDeltaJsUtf16 | Update the displayed block; preserve exact UTF-16 sidecars when supplied | A complete response or parseable JSON document |
| ToolCallDelta / BlockEnd | Accumulate the original block and arguments; inspect terminal assembly before execution | Permission to execute partial arguments |
| ProviderContent / ThoughtSignature | Retain replay data and signatures in the assistant transcript | That display-only reasoning text can replace native replay content |
| ProviderEvent / NativeControl | Keep provider observation/control data separately | A local function call to dispatch |
| End | Inspect stop_reason, usage and inference; then drain through None | That every terminal response is a successful, untruncated final answer |

<a id="stream-result-checklist"></a>

## Build a final message safely

1. Use StreamAccumulator to retain events while the UI shows text. Key content by the original block index, because indices may be sparse and different output kinds may interleave.
2. When reading ends, inspect StreamAssembly.terminal, unfinished_blocks and incomplete_tools. A MaxTokens stop is terminal but may still truncate the answer or a tool argument.
3. Persist usage_report and inference_report before deciding whether to render an error, execute tools or schedule another attempt. Reading failed after output began is not the same as no request having been sent.
4. Keep the full reconstructed assistant message for the next request. If all you retained was the concatenated visible text, do not fabricate missing signatures, tool calls or provider-native blocks.

- [Lossless assembly and incomplete blocks](llm-streaming.md#lossless-assembly)
- [Per-attempt accounting on failure](llm-execution-accounting.md)

<a id="stream-ui-behavior"></a>

## Keep the UI responsive without changing protocol semantics

Batch display updates in the UI instead of repainting for every token, but keep event ordering and accounting observations unchanged. If the UI falls behind, bound its queue rather than building an unbounded string/event buffer.

A user stop drops the stream and records the partial attempt. A later retry is a fresh request with a new application attempt identity; it is not a promise to resume exactly at the last displayed character. Provider-scoped continuation is available only after a completed response.

<a id="batches"></a>

## Observe each transport chunk

next_batch reads at most one transport chunk and returns its events, current usage, inference facts, and finished flag. A batch can have no events while usage changes; a terminal error remains alongside its latest usage. Record observations before validating content or yielding control.

### Chunk observation · excerpt with an existing stream / 传输块观测片段

```rust
while let Some(batch) = stream.next_batch().await {
    // Persist these facts in the host before interpreting events.
    let usage = batch.usage;
    let inference = batch.inference;
    eprintln!("{usage:?} {inference:?}");
    for event in batch.events {
        let event = event?;
        // Dispatch or retain event in your application.
        eprintln!("{event:?}");
    }
}
```

Verification method: API excerpt

### ModelStream::next

```text
pub async fn next(&mut self) -> Option<Result<StreamEvent, LlmError>>
```

Read one provider-neutral event.

### ModelStream::next_batch

```text
pub async fn next_batch(&mut self) -> Option<StreamBatch>
```

Read one accounting observation without awaiting another chunk after decoding.

### StreamBatch

```text
pub struct StreamBatch { pub events: Vec<Result<StreamEvent, LlmError>>, pub usage: UsageReport, pub inference: InferenceReport, pub finished: bool }
```

Retain events and accounting facts together, including usage-only chunks and terminal errors.

### ModelStream::usage_is_complete

```text
pub fn usage_is_complete(&self) -> bool
```

Check for complete, internally consistent usage before estimating actual cost.

<a id="cancel"></a>

## Cancellation & deadlines

- Dropping ModelStream releases the underlying byte stream and disconnects the read. The host keeps the usage already observed and owns settlement for an interrupted attempt.
- Once a stream has been returned, read failures do not trigger automatic failover. The host may prepare a fresh attempt according to its retry policy.
- Set RequestOptions.total_timeout to bound the whole stream. An omitted value means no default total stream deadline.
- A continuation reference is exposed only for a completed response. An interrupted stream cannot supply a valid continuation.

<a id="structured-websocket"></a>

## Structured output & WebSocket

collect_structured_json and collect_structured<T> reconstruct a terminal response before validating JSON/schema or deserializing. Individual deltas are not standalone JSON documents. ResponsesSession provides connection reuse and continuation; preparation establishes the connection before host admission and dispatch sends one response.create.

### ModelStream::collect_structured_json

```text
pub async fn collect_structured_json(self, format: &OutputFormat) -> Result<StructuredStreamResult<JsonPayload>, StructuredStreamError>
```

Consume and validate final JSON while retaining the reconstructed response and events.

### ModelStream::continuation

```text
pub fn continuation(&self) -> Option<&ContinuationRef>
```

Read account-bound state only after a complete stream.

PreparedCall WebSocket execution requires a Transport implementing connect_websocket. A failed send never resends the current call. The host owns reconnection, cancellation, and whether to prepare another attempt.

<a id="lossless-assembly"></a>

## Preserve canonical and native stream data

Use StreamAccumulator or ModelStream::collect_response when a host needs a complete assistant message. Assembly preserves sparse block indices, completed block order, thought signatures, native replay content and exact tool input JSON. The partial assembly on failure exposes unfinished blocks and incomplete tools; it must not be treated as a successful terminal tool request.

| Event family | Consumer handling |
| --- | --- |
| TextDelta / TextDeltaJsUtf16 | Display and assemble by block; retain exact UTF-16 units where present |
| ReasoningDelta / ThoughtSignature / RedactedThinking | Reasoning display and replay signatures/opaque data have separate roles |
| ToolCallDelta / BlockEnd | Accumulate exact arguments and provider IDs; execute only a validated completed call |
| ProviderContent / Native / NativeDelta | Preserve typed/native payload and protocol ownership |
| ProviderEvent / NativeControl | Observation/control only; not replayable message content or local tool calls |
| Inference / WebSearch / FileSearch / End | Retain facts, attribution and stop reason; continue until EOF |

A final End event and complete UsageReport are different facts. Provider streams can omit usage or fail after content; use usage_is_complete() and final inference when estimating cost. model_stream and structured_output tests validate local projection, not live streaming latency.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/stream.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/session.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/llm.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/stream_assembly.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/framing/sse.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/framing/eventstream.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/model_stream.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/structured_output.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/message.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/usage_report.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
