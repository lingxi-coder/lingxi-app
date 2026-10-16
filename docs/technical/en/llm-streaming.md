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
pub async fn collect_structured_json(self, format: &OutputFormat) -> Result<StructuredStreamResult<serde_json::Value>, StructuredStreamError>
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

- [llm-client/src/client/stream.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/stream.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/client/session.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/session.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/llm.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/stream_assembly.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/stream_assembly.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/framing/sse.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/framing/sse.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/framing/eventstream.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/framing/eventstream.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/model_stream.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/model_stream.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/structured_output.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/structured_output.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
