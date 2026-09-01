# llm-client transport layer — design

Date: 2026-06-10
Status: approved (user), implementation in `llm-client`

## Goal

Give `llm-client` an execution path from `LlmRequest` to `LlmResponse` /
`LlmEvent` stream over real HTTP, without coupling the crate to any HTTP
library. Production HTTP stays on the existing platform stack
(`platform_api::HttpTransport` / `platforms/common::ReqwestHttp`, design rule D17:
engine code never imports a concrete HTTP client); `llm-client` ships the
contract and the orchestration, the host ships a thin adapter.

## Non-goals

- A concrete HTTP implementation inside `llm-client` (no reqwest/tokio
  runtime dependencies; tokio returns as dev-dependency only).
- The `platform_api::HttpTransport -> llm_client::Transport` adapter (written when
  the engine adopts `llm-client`).
- Retry driving (RetryPolicy classification exists; the host owns the loop).
- Bedrock binary event-stream framing.

## Components

### 1. Transport contract (`src/transport.rs`)

Object-safe async via a hand-rolled alias — no `async_trait`, no `futures`:

```rust
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Transport: Send + Sync {
    fn execute<'a>(&'a self, request: &'a ProviderRequest)
        -> BoxFuture<'a, Result<ProviderResponse, LlmError>>;
    fn open_stream<'a>(&'a self, request: &'a ProviderRequest)
        -> BoxFuture<'a, Result<StreamingResponse, LlmError>>;
}

pub struct StreamingResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,   // lowercase names expected
    pub frames: Box<dyn FrameStream>,
}

pub trait FrameStream: Send {
    /// Next frame; Ok(None) is normal end of stream.
    fn next_frame(&mut self) -> BoxFuture<'_, Result<Option<RawStreamFrame>, LlmError>>;
}
```

Responsibility boundary: implementations own HTTP semantics only.
Connect/TLS/timeout failures map to `LlmError::Transport`. Implementations
never interpret provider payloads — non-2xx responses come back as data
(`ProviderResponse`, or `StreamingResponse` whose frames carry raw body
bytes), and the orchestration layer routes them through the existing
status-aware `WireCodec::decode_response` error taxonomy.

Frame contract: one frame = one SSE `data:` payload without the field prefix
(exactly what the three stream decoders already consume, `[DONE]` included).

### 2. SSE frame splitter (`src/sse.rs`)

`SseFrameSplitter` — incremental, dependency-free, for hosts that only have a
byte stream (the repo's `stream_sse` already yields parsed events and can
bypass this):

- `push(&mut self, bytes: &[u8]) -> Vec<RawStreamFrame>` buffers across chunk
  boundaries; events end at a blank line (`\n\n` or `\r\n\r\n`).
- Multiple `data:` lines in one event join with `\n` (SSE spec). One leading
  space after `data:` is stripped.
- `:` comment lines and `event:`/`id:`/`retry:` fields are ignored.
- Events with no data lines produce no frame.
- `finish(&mut self) -> Option<RawStreamFrame>` flushes a trailing
  unterminated event.

### 3. Execution orchestration (`DefaultLlmClient` methods + `LlmEventStream`)

```rust
impl DefaultLlmClient {
    pub async fn execute(&self, request: &LlmRequest, transport: &dyn Transport)
        -> Result<LlmResponse, LlmError>;
    pub async fn execute_stream(&self, request: &LlmRequest, transport: &dyn Transport)
        -> Result<LlmEventStream, LlmError>;
}
```

- `execute`: `prepare()` (resolve + validate + encode + authenticate) →
  `transport.execute` → `codec.decode_response` (status-aware) → `LlmResponse`.
- `execute_stream`: requires `request.stream == true` (else
  `InvalidRequest`); `prepare()` → `transport.open_stream`; on status >= 400
  the frames are drained, concatenated, parsed as JSON (invalid JSON becomes
  `Value::Null`), and routed through `decode_response` so stream errors get
  the same taxonomy as non-stream errors; otherwise returns `LlmEventStream`.
- `LlmEventStream::next_event() -> Result<Option<LlmEvent>, LlmError>`:
  pull-based. Drains queued events first; pulls frames through
  `StreamDecoder::decode_frame`; on `Ok(None)` from the frame stream calls
  `StreamDecoder::finish()` once and yields its events; then `Ok(None)`.
- Error semantics: a transport failure after at least one event has been
  yielded is upgraded to `LlmError::StreamInterrupted` (non-retryable, per
  retry policy); before any event it stays `LlmError::Transport` (retryable).
  Decoder errors pass through unchanged. Any error is terminal: subsequent
  `next_event` calls return `Ok(None)`.

`async fn` on inherent impls keeps the crate free of `async_trait`.

## Testing (TDD, red-green per component)

- `sse.rs`: pure unit tests — split across chunk boundaries, CRLF, multi-line
  data joining, comment/field skipping, trailing flush, `[DONE]` passthrough.
- Orchestration: an in-crate fake `Transport` (canned `ProviderResponse` /
  frame scripts, including mid-stream errors) drives:
  - `execute` success decode and 429 → `RateLimited` taxonomy mapping;
  - `execute_stream` over the three providers' fixture frames → full
    `LlmEvent` sequences (single `MessageStop`, terminal usage);
  - non-2xx stream open → taxonomy error;
  - mid-stream failure after first event → `StreamInterrupted`, then `None`;
  - `stream: false` rejection.
- `tokio` returns as dev-dependency (workspace features) for `#[tokio::test]`.

## Integration path (follow-up, not this change)

A ~50-line adapter in the engine/platform layer implements
`llm_client::Transport` over `platform_api::HttpTransport`: `execute` maps
`ProviderRequest` → `protocol::HttpRequest`; `open_stream` uses `stream_sse`
(mapping each `SseEvent.data` to one `RawStreamFrame`) or `stream_raw_bytes` +
`SseFrameSplitter`.
