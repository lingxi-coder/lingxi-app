# LLM Providers — P1 (Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `providers` crate's core abstraction (`LlmProvider` trait + pure `WireCodec`/`SseDecoder` + `GenericClient`), wrap Anthropic as the first `LlmProvider`, and add a `ProviderApiAdapter` bridge so the orchestrator can route any provider through its existing `OrchestratorApiClient`/`StreamingApiClient` traits — with zero change to existing behavior.

**Architecture:** A new engine-tier crate `providers` defines the provider abstraction. The translation core is a pure `WireCodec` (encode request / decode response) + `SseDecoder` (stateful streaming). `GenericClient<C: WireCodec>` drives a codec over `traits::HttpTransport` (the harness OpenAI/Gemini use in P3/P4). `AnthropicLlmProvider<T>` delegates verbatim to the existing `api_client::AnthropicProvider` so byte-parity is automatic. A `ProviderApiAdapter` in the `orchestrator` crate adapts any `Arc<dyn LlmProvider>` to the two orchestrator traits. **This plan does NOT touch `apps/cli/src/init.rs`** — production still wires `AnthropicProviderAdapter`, so all existing tests stay green by construction. Composition-root switching is P2.

**Tech Stack:** Rust 1.82.0 (pinned via `rust-toolchain.toml`). `async-trait`, `futures` (stream combinators), `serde_json`, `thiserror`. Reuses `protocol` (canonical types), `traits::HttpTransport`, `api-client` (`AnthropicProvider`, `ApiError`, wire types), `cost::ProviderId`.

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§3 architecture, §12 P1).

**Conventions (read before starting):**
- Run **all** `cargo` / `scripts` commands from `lingxi-code/` (the workspace root). Example: `cd lingxi-code && cargo test -p providers`.
- The workspace enables `missing_docs = "warn"` and `clippy::pedantic = "warn"`; the gate runs `cargo clippy -- -D warnings`. **Every `pub` item needs a `///` doc comment**, inherent constructors need `#[must_use]`, and code must be pedantic-clean. The code blocks below already satisfy this — paste them verbatim.
- Do **NOT** modify the `traits/` crate (frozen).
- Crate package name is `providers`; in Rust code the dependency `api-client` is imported as `api_client`.

---

## File Structure

**New crate `lingxi-code/providers/`:**
- `Cargo.toml` — manifest (engine-tier deps).
- `src/lib.rs` — module declarations + public re-exports.
- `src/request.rs` — `CanonicalRequest` + `DEFAULT_MAX_TOKENS` (the provider-neutral request the bridge builds).
- `src/capabilities.rs` — `Capabilities`, `ReasoningSupport`, `SystemStyle`.
- `src/auth.rs` — `Auth` (the per-provider auth-header seam).
- `src/error.rs` — `CodecError` (pure-codec failures).
- `src/codec.rs` — `WireCodec` + `SseDecoder` traits (pure translation contract).
- `src/provider.rs` — `LlmProvider` trait (the runtime provider contract).
- `src/client.rs` — `GenericClient<C>` + the streaming pump (codec → `HttpTransport`).
- `src/anthropic.rs` — `AnthropicLlmProvider<T>` (delegates to `api_client::AnthropicProvider`).
- `src/testutil.rs` — `#[cfg(test)]` `MockTransport` shared by unit tests.

**Modified crate `lingxi-code/orchestrator/`:**
- `Cargo.toml` — add `providers` dependency.
- `src/provider_adapter.rs` — NEW: `ProviderApiAdapter` (bridge to `OrchestratorApiClient` + `StreamingApiClient`).
- `src/lib.rs` — declare + re-export `provider_adapter`.

**Modified workspace:**
- `lingxi-code/Cargo.toml` — add `"providers"` to `members`.

---

## Task 1: Scaffold the `providers` crate

**Files:**
- Create: `lingxi-code/providers/Cargo.toml`
- Create: `lingxi-code/providers/src/lib.rs`
- Modify: `lingxi-code/Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the crate manifest**

Create `lingxi-code/providers/Cargo.toml`:

```toml
[package]
name = "providers"
version = "0.10.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
protocol = { path = "../protocol" }
traits = { path = "../traits" }
api-client = { path = "../api-client" }
cost = { path = "../cost" }
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
futures = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time", "sync"] }

[lints]
workspace = true
```

- [ ] **Step 2: Create the crate root**

Create `lingxi-code/providers/src/lib.rs`:

```rust
//! Multi-provider LLM backend.
//!
//! Defines a provider abstraction (`LlmProvider`) whose translation core is a
//! pure `WireCodec` (encode request / decode response) plus a stateful
//! `SseDecoder`. Each provider normalizes to the canonical Anthropic-shaped
//! types in `api_client::types`, so the rest of the engine is unchanged.
//!
//! See `docs/superpowers/specs/2026-06-01-llm-providers-design.md`.
#![forbid(unsafe_code)]
```

- [ ] **Step 3: Register the crate in the workspace**

In `lingxi-code/Cargo.toml`, add `"providers",` to the `members = [ ... ]` array (e.g. on the line after `"orchestrator",`).

- [ ] **Step 4: Build to verify the empty crate compiles**

Run: `cd lingxi-code && cargo build -p providers`
Expected: compiles clean (`Finished` line, no errors).

- [ ] **Step 5: Verify the dependency gate still passes**

Run: `cd lingxi-code && bash scripts/check-deps.sh`
Expected: `check-deps: OK — <N> workspace crates, no §8.1 dependency violations` (where `<N>` is one more than before — `providers` is now counted).

- [ ] **Step 6: Commit**

```bash
cd lingxi-code
git add providers/Cargo.toml providers/src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(llm-p1): scaffold providers crate"
```

---

## Task 2: Core value types

**Files:**
- Create: `lingxi-code/providers/src/request.rs`
- Create: `lingxi-code/providers/src/capabilities.rs`
- Create: `lingxi-code/providers/src/auth.rs`
- Create: `lingxi-code/providers/src/error.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Write `request.rs` with its test**

Create `lingxi-code/providers/src/request.rs`:

```rust
//! The provider-neutral request the bridge builds and hands to a provider.

use protocol::ConversationMessage;

/// Default `max_tokens` ceiling. Matches `api_client::AnthropicProvider`'s
/// hardcoded value so the Anthropic path is unchanged.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// A provider-neutral completion request. Codecs translate this into each
/// provider's native wire shape; the Anthropic path consumes only `model`,
/// `system`, `messages`, and `tools`.
#[derive(Debug, Clone)]
pub struct CanonicalRequest {
    /// Provider-local model id (any `provider/` prefix already stripped — P2).
    pub model: String,
    /// Optional system prompt.
    pub system: Option<String>,
    /// Conversation history, oldest first.
    pub messages: Vec<ConversationMessage>,
    /// Canonical (Anthropic-shaped) tool schema declarations.
    pub tools: Vec<serde_json::Value>,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Whether this is a streaming request.
    pub stream: bool,
}

impl CanonicalRequest {
    /// Construct a request for `model` with empty history and defaults.
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            temperature: None,
            stream: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_defaults() {
        let r = CanonicalRequest::new("gpt-4o");
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(r.system.is_none());
        assert!(r.messages.is_empty());
        assert!(!r.stream);
    }
}
```

- [ ] **Step 2: Write `capabilities.rs` with its test**

Create `lingxi-code/providers/src/capabilities.rs`:

```rust
//! Per-provider capability descriptor used to gate features and degrade
//! gracefully ("omit, never invent").

/// How a provider exposes reasoning / thinking traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningSupport {
    /// No reasoning trace.
    None,
    /// Reasoning is decoded from responses into the canonical `Thinking`
    /// block, but never re-encoded into request history.
    DecodeOnly,
}

/// Where a provider expects the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStyle {
    /// A dedicated top-level field (Anthropic `system`, Gemini
    /// `systemInstruction`).
    TopLevel,
    /// A leading role message (OpenAI `system`/`developer`).
    RoleMessage,
}

/// What a provider can do. Codecs consult this to gate tools and features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Native function-calling support.
    pub native_tools: bool,
    /// Streaming (SSE) support.
    pub streaming: bool,
    /// Image input support.
    pub vision: bool,
    /// Prompt-caching support.
    pub prompt_cache: bool,
    /// Reasoning-trace support.
    pub reasoning: ReasoningSupport,
    /// Whether multiple tool calls can be returned in one turn.
    pub parallel_tool_calls: bool,
    /// Maximum output tokens the provider accepts, if known.
    pub max_output_tokens: Option<u32>,
    /// Where the system prompt goes.
    pub system_style: SystemStyle,
}

impl Capabilities {
    /// Capabilities for the Anthropic provider (everything native).
    #[must_use]
    pub fn anthropic() -> Self {
        Self {
            native_tools: true,
            streaming: true,
            vision: true,
            prompt_cache: true,
            reasoning: ReasoningSupport::DecodeOnly,
            parallel_tool_calls: true,
            max_output_tokens: None,
            system_style: SystemStyle::TopLevel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_caps_are_fully_native() {
        let c = Capabilities::anthropic();
        assert!(c.native_tools);
        assert!(c.streaming);
        assert_eq!(c.system_style, SystemStyle::TopLevel);
    }
}
```

- [ ] **Step 3: Write `auth.rs` with its test**

Create `lingxi-code/providers/src/auth.rs`:

```rust
//! Per-provider auth-header seam. v1 supports API-key styles; signed auth
//! (Vertex / Bedrock / Azure AD) is added later behind the same seam.

/// How a provider authenticates a request.
#[derive(Debug, Clone)]
pub enum Auth {
    /// No auth header (e.g. a local Ollama endpoint).
    None,
    /// `Authorization: Bearer <key>` (OpenAI and OpenAI-compatible).
    Bearer(String),
    /// A literal header name/value pair (e.g. `x-goog-api-key`, `x-api-key`).
    Header {
        /// Header name.
        name: String,
        /// Header value.
        value: String,
    },
}

impl Auth {
    /// Append this auth's header(s) to `headers`. No-op for [`Auth::None`].
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        match self {
            Self::None => {}
            Self::Bearer(key) => {
                headers.push(("authorization".to_string(), format!("Bearer {key}")));
            }
            Self::Header { name, value } => headers.push((name.clone(), value.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_appends_authorization_header() {
        let mut h = Vec::new();
        Auth::Bearer("sk-123".to_string()).apply(&mut h);
        assert_eq!(h, vec![("authorization".to_string(), "Bearer sk-123".to_string())]);
    }

    #[test]
    fn none_appends_nothing() {
        let mut h = Vec::new();
        Auth::None.apply(&mut h);
        assert!(h.is_empty());
    }
}
```

- [ ] **Step 4: Write `error.rs`**

Create `lingxi-code/providers/src/error.rs`:

```rust
//! Pure-codec failure type. Distinct from `api_client::ApiError`: a
//! `CodecError` means the request could not even be encoded.

use thiserror::Error;

/// A failure while encoding a canonical request into a provider's wire shape.
#[derive(Debug, Clone, Error)]
pub enum CodecError {
    /// The request used a feature this provider/model cannot represent.
    #[error("unsupported request: {0}")]
    Unsupported(String),
    /// The request body could not be assembled.
    #[error("encode failed: {0}")]
    Encode(String),
}
```

- [ ] **Step 5: Wire the modules into `lib.rs`**

Replace `lingxi-code/providers/src/lib.rs` with:

```rust
//! Multi-provider LLM backend.
//!
//! Defines a provider abstraction (`LlmProvider`) whose translation core is a
//! pure `WireCodec` (encode request / decode response) plus a stateful
//! `SseDecoder`. Each provider normalizes to the canonical Anthropic-shaped
//! types in `api_client::types`, so the rest of the engine is unchanged.
//!
//! See `docs/superpowers/specs/2026-06-01-llm-providers-design.md`.
#![forbid(unsafe_code)]

pub mod auth;
pub mod capabilities;
pub mod error;
pub mod request;

pub use auth::Auth;
pub use capabilities::{Capabilities, ReasoningSupport, SystemStyle};
pub use error::CodecError;
pub use request::{CanonicalRequest, DEFAULT_MAX_TOKENS};
```

- [ ] **Step 6: Run the tests**

Run: `cd lingxi-code && cargo test -p providers`
Expected: PASS (`new_sets_defaults`, `anthropic_caps_are_fully_native`, `bearer_appends_authorization_header`, `none_appends_nothing`).

- [ ] **Step 7: Lint clean**

Run: `cd lingxi-code && cargo clippy -p providers --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 8: Commit**

```bash
cd lingxi-code
git add providers/src
git commit -m "feat(llm-p1): CanonicalRequest, Capabilities, Auth, CodecError"
```

---

## Task 3: Traits + `GenericClient` + streaming pump

**Files:**
- Create: `lingxi-code/providers/src/codec.rs`
- Create: `lingxi-code/providers/src/provider.rs`
- Create: `lingxi-code/providers/src/client.rs`
- Create: `lingxi-code/providers/src/testutil.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Write the codec traits**

Create `lingxi-code/providers/src/codec.rs`:

```rust
//! The pure translation contract. No I/O lives here — `encode_request`
//! builds a request, `decode_response` parses a body, and `SseDecoder`
//! turns provider SSE frames into canonical stream events.

use crate::auth::Auth;
use crate::error::CodecError;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use protocol::HttpRequest;

/// Pure encode/decode for one provider's wire format.
pub trait WireCodec: Send + Sync {
    /// Build the native HTTP request for `req`, attaching `auth` headers.
    ///
    /// # Errors
    /// Returns [`CodecError`] if the request cannot be represented.
    fn encode_request(&self, req: &CanonicalRequest, auth: &Auth) -> Result<HttpRequest, CodecError>;

    /// Decode a non-streaming response body into the canonical shape.
    ///
    /// # Errors
    /// Returns [`ApiError`] for non-success responses or malformed bodies.
    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError>;

    /// Create a fresh per-stream decoder for this provider's SSE format.
    fn new_stream_decoder(&self) -> Box<dyn SseDecoder>;
}

/// A stateful per-stream decoder. `push` is called once per SSE `data:`
/// payload; `finish` flushes any trailing state at end-of-stream.
pub trait SseDecoder: Send {
    /// Decode one SSE `data:` payload into zero or more canonical events.
    fn push(&mut self, data: &str) -> Vec<StreamEvent>;

    /// Flush any buffered trailing events when the stream ends.
    fn finish(&mut self) -> Vec<StreamEvent>;
}
```

- [ ] **Step 2: Write the provider trait**

Create `lingxi-code/providers/src/provider.rs`:

```rust
//! The runtime provider contract. Implemented by `AnthropicLlmProvider` and
//! (via a codec) `GenericClient`.

use crate::capabilities::Capabilities;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;

/// A model backend that can complete and stream the canonical request shape.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// The provider's identity (for cost attribution / telemetry).
    fn id(&self) -> ProviderId;

    /// What this provider can do.
    fn capabilities(&self) -> &Capabilities;

    /// Non-streaming completion.
    ///
    /// # Errors
    /// Returns [`ApiError`] on transport, auth, or decode failure.
    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError>;

    /// Streaming completion: yields canonical stream events until end-of-stream.
    ///
    /// # Errors
    /// Returns [`ApiError`] if the connection cannot be opened; per-event
    /// failures surface as `Err` items inside the stream.
    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError>;
}
```

- [ ] **Step 3: Write the `#[cfg(test)]` mock transport**

Create `lingxi-code/providers/src/testutil.rs`:

```rust
//! Test-only `HttpTransport` mock. Returns canned responses / SSE frames.

use async_trait::async_trait;
use futures::stream;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use traits::{HttpError, HttpTransport, SseStream};

/// A canned transport for unit tests.
pub(crate) struct MockTransport {
    status: u16,
    body: String,
    sse_frames: Vec<String>,
}

impl MockTransport {
    /// Return one non-streaming response with `status` and `body`.
    pub(crate) fn responding(status: u16, body: impl Into<String>) -> Self {
        Self { status, body: body.into(), sse_frames: Vec::new() }
    }

    /// Return `frames` as successive SSE `data:` payloads (status 200).
    pub(crate) fn streaming(frames: Vec<&str>) -> Self {
        Self {
            status: 200,
            body: String::new(),
            sse_frames: frames.into_iter().map(String::from).collect(),
        }
    }
}

#[async_trait]
impl HttpTransport for MockTransport {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse { status: self.status, headers: Vec::new(), body: self.body.clone() })
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
        let frames: Vec<Result<SseEvent, HttpError>> = self
            .sse_frames
            .iter()
            .map(|d| Ok(SseEvent { event_type: None, data: d.clone(), id: None }))
            .collect();
        Ok(Box::pin(stream::iter(frames)))
    }
}
```

- [ ] **Step 4: Write the `GenericClient` test (TDD — should fail to compile first)**

Create `lingxi-code/providers/src/client.rs` with ONLY the test module for now (the implementation goes in Step 6):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockTransport;
    use api_client::types::{ContentBlockApi, ContentDelta, UsageApi};
    use futures::StreamExt;
    use protocol::{HttpMethod, HttpRequest};
    use std::sync::Arc;

    struct MockCodec;

    impl WireCodec for MockCodec {
        fn encode_request(
            &self,
            _req: &CanonicalRequest,
            _auth: &Auth,
        ) -> Result<HttpRequest, CodecError> {
            Ok(HttpRequest {
                method: HttpMethod::Post,
                url: "https://mock.local/v1/chat".to_string(),
                headers: Vec::new(),
                body: Some("{}".to_string()),
                timeout: None,
            })
        }

        fn decode_response(&self, _status: u16, body: &str) -> Result<MessageResponse, ApiError> {
            Ok(MessageResponse {
                id: "m".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlockApi::Text { text: body.to_string() }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }

        fn new_stream_decoder(&self) -> Box<dyn SseDecoder> {
            Box::new(MockDecoder)
        }
    }

    struct MockDecoder;

    impl SseDecoder for MockDecoder {
        fn push(&mut self, data: &str) -> Vec<StreamEvent> {
            vec![StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: data.to_string() },
            }]
        }

        fn finish(&mut self) -> Vec<StreamEvent> {
            vec![StreamEvent::MessageStop]
        }
    }

    fn client(transport: MockTransport) -> GenericClient<MockCodec> {
        GenericClient::new(
            MockCodec,
            Auth::None,
            Arc::new(transport),
            cost::ProviderId::OpenAI,
            Capabilities::anthropic(),
        )
    }

    #[tokio::test]
    async fn complete_runs_encode_request_decode() {
        let c = client(MockTransport::responding(200, "PONG"));
        let resp = c.complete(CanonicalRequest::new("gpt-4o")).await.expect("ok");
        match &resp.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "PONG"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_flattens_decoder_events_then_finish() {
        let c = client(MockTransport::streaming(vec!["a", "b"]));
        let s = c.stream(CanonicalRequest::new("gpt-4o")).await.expect("stream");
        let events: Vec<_> = s.collect().await;
        assert_eq!(events.len(), 3, "2 deltas + MessageStop");
        assert!(matches!(events[0], Ok(StreamEvent::ContentBlockDelta { .. })));
        assert!(matches!(events[2], Ok(StreamEvent::MessageStop)));
    }
}
```

- [ ] **Step 5: Run the test to confirm it fails (no implementation yet)**

Run: `cd lingxi-code && cargo test -p providers --lib client 2>&1 | head -20`
Expected: FAIL — compile error `cannot find ... GenericClient` (and `WireCodec`/`SseDecoder` not in scope). This confirms the test drives the implementation.

- [ ] **Step 6: Write the `GenericClient` + streaming pump above the test module**

Prepend the following to `lingxi-code/providers/src/client.rs` (above the `#[cfg(test)] mod tests` block):

```rust
//! `GenericClient` drives a `WireCodec` over a `traits::HttpTransport`. This
//! is the harness the OpenAI/Gemini codecs plug into (P3/P4).

use crate::auth::Auth;
use crate::capabilities::Capabilities;
use crate::codec::{SseDecoder, WireCodec};
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::{self, BoxStream, StreamExt};
use std::collections::VecDeque;
use std::sync::Arc;
use traits::{HttpError, HttpTransport, SseStream};

/// A codec-driven provider: encode → transport → decode.
pub struct GenericClient<C: WireCodec> {
    codec: C,
    auth: Auth,
    transport: Arc<dyn HttpTransport>,
    id: ProviderId,
    capabilities: Capabilities,
}

impl<C: WireCodec> GenericClient<C> {
    /// Construct a client from a codec, auth, transport, identity, and caps.
    #[must_use]
    pub fn new(
        codec: C,
        auth: Auth,
        transport: Arc<dyn HttpTransport>,
        id: ProviderId,
        capabilities: Capabilities,
    ) -> Self {
        Self { codec, auth, transport, id, capabilities }
    }
}

/// State threaded through the streaming pump.
struct StreamPump {
    wire: SseStream,
    decoder: Box<dyn SseDecoder>,
    queue: VecDeque<Result<StreamEvent, ApiError>>,
    done: bool,
}

/// Flatten a transport SSE stream through a stateful decoder into canonical
/// events. Owns `wire` + `decoder`, so the result is `'static`.
fn pump_stream(
    wire: SseStream,
    decoder: Box<dyn SseDecoder>,
) -> BoxStream<'static, Result<StreamEvent, ApiError>> {
    let init = StreamPump { wire, decoder, queue: VecDeque::new(), done: false };
    stream::unfold(init, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                return Some((item, st));
            }
            if st.done {
                return None;
            }
            match st.wire.next().await {
                Some(Ok(sse)) => {
                    for ev in st.decoder.push(&sse.data) {
                        st.queue.push_back(Ok(ev));
                    }
                }
                Some(Err(e)) => return Some((Err(ApiError::Http(e)), st)),
                None => {
                    for ev in st.decoder.finish() {
                        st.queue.push_back(Ok(ev));
                    }
                    st.done = true;
                }
            }
        }
    })
    .boxed()
}

#[async_trait]
impl<C: WireCodec + 'static> LlmProvider for GenericClient<C> {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let http = self
            .codec
            .encode_request(&req, &self.auth)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        let resp = self.transport.request(http).await.map_err(ApiError::Http)?;
        self.codec.decode_response(resp.status, &resp.body)
    }

    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut req = req;
        req.stream = true;
        let http = self
            .codec
            .encode_request(&req, &self.auth)
            .map_err(|e| ApiError::Http(HttpError::InvalidRequest(e.to_string())))?;
        let wire = self.transport.stream_sse(http).await.map_err(ApiError::Http)?;
        let decoder = self.codec.new_stream_decoder();
        Ok(pump_stream(wire, decoder))
    }
}
```

- [ ] **Step 7: Wire the new modules into `lib.rs`**

Replace `lingxi-code/providers/src/lib.rs` with:

```rust
//! Multi-provider LLM backend.
//!
//! Defines a provider abstraction (`LlmProvider`) whose translation core is a
//! pure `WireCodec` (encode request / decode response) plus a stateful
//! `SseDecoder`. Each provider normalizes to the canonical Anthropic-shaped
//! types in `api_client::types`, so the rest of the engine is unchanged.
//!
//! See `docs/superpowers/specs/2026-06-01-llm-providers-design.md`.
#![forbid(unsafe_code)]

pub mod auth;
pub mod capabilities;
pub mod client;
pub mod codec;
pub mod error;
pub mod provider;
pub mod request;

#[cfg(test)]
mod testutil;

pub use auth::Auth;
pub use capabilities::{Capabilities, ReasoningSupport, SystemStyle};
pub use client::GenericClient;
pub use codec::{SseDecoder, WireCodec};
pub use error::CodecError;
pub use provider::LlmProvider;
pub use request::{CanonicalRequest, DEFAULT_MAX_TOKENS};
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cd lingxi-code && cargo test -p providers`
Expected: PASS (`complete_runs_encode_request_decode`, `stream_flattens_decoder_events_then_finish`, plus Task 2 tests).

- [ ] **Step 9: Lint clean**

Run: `cd lingxi-code && cargo clippy -p providers --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 10: Commit**

```bash
cd lingxi-code
git add providers/src
git commit -m "feat(llm-p1): WireCodec/SseDecoder/LlmProvider traits + GenericClient"
```

---

## Task 4: `AnthropicLlmProvider` (delegates to `api_client::AnthropicProvider`)

**Files:**
- Create: `lingxi-code/providers/src/anthropic.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

**Why generic over `T`:** `AnthropicProvider::messages_create_non_stream<T: HttpTransport>(..., &T)` and `messages_create_stream<T: …>(..., Arc<T>)` require a *sized* transport type, so this provider is generic over the concrete `T` and is type-erased to `Arc<dyn LlmProvider>` at construction (P2 passes the real `PosixHttp`).

- [ ] **Step 1: Write the test first (TDD)**

Create `lingxi-code/providers/src/anthropic.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockTransport;
    use api_client::types::ContentBlockApi;
    use futures::StreamExt;
    use std::sync::Arc;

    const CANNED_200: &str = r#"{"id":"msg_x","model":"claude-opus-4-7","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#;

    fn provider(t: MockTransport) -> AnthropicLlmProvider<MockTransport> {
        AnthropicLlmProvider::new("sk-test", Some("https://mock.local".to_string()), Arc::new(t))
    }

    #[test]
    fn id_is_anthropic() {
        let p = provider(MockTransport::responding(200, ""));
        assert_eq!(p.id(), cost::ProviderId::Anthropic);
        assert!(p.capabilities().native_tools);
    }

    #[tokio::test]
    async fn complete_decodes_canned_anthropic_response() {
        let p = provider(MockTransport::responding(200, CANNED_200));
        let resp = p.complete(CanonicalRequest::new("claude-opus-4-7")).await.expect("ok");
        assert_eq!(resp.id, "msg_x");
        match &resp.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hi"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_yields_decoded_events() {
        let frames = vec![
            r#"{"type":"message_start","message":{"id":"msg_x","model":"claude-opus-4-7","content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let p = provider(MockTransport::streaming(frames));
        let s = p.stream(CanonicalRequest::new("claude-opus-4-7")).await.expect("stream");
        let events: Vec<_> = s.collect().await;
        assert!(matches!(events.first(), Some(Ok(StreamEvent::MessageStart { .. }))));
        assert!(matches!(events.last(), Some(Ok(StreamEvent::MessageStop))));
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cd lingxi-code && cargo test -p providers --lib anthropic 2>&1 | head -20`
Expected: FAIL — compile error `cannot find ... AnthropicLlmProvider`.

- [ ] **Step 3: Write the implementation above the test module**

Prepend to `lingxi-code/providers/src/anthropic.rs`:

```rust
//! Anthropic as an `LlmProvider`. Delegates verbatim to
//! `api_client::AnthropicProvider`, so the wire is byte-identical to today's
//! `AnthropicProviderAdapter` and stays covered by the existing parity
//! fixtures. Generic over the concrete transport `T`; erased to
//! `Arc<dyn LlmProvider>` by the caller.

use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::{AnthropicProvider, ApiError};
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;
use std::sync::Arc;
use traits::HttpTransport;

/// Anthropic provider backed by `api_client::AnthropicProvider` + a transport.
pub struct AnthropicLlmProvider<T: HttpTransport + Send + Sync + 'static> {
    inner: AnthropicProvider,
    transport: Arc<T>,
    capabilities: Capabilities,
}

impl<T: HttpTransport + Send + Sync + 'static> AnthropicLlmProvider<T> {
    /// Construct from an API key, optional base URL, and a transport.
    #[must_use]
    pub fn new(api_key: impl Into<String>, base_url: Option<String>, transport: Arc<T>) -> Self {
        Self {
            inner: AnthropicProvider::new(api_key, base_url),
            transport,
            capabilities: Capabilities::anthropic(),
        }
    }
}

#[async_trait]
impl<T: HttpTransport + Send + Sync + 'static> LlmProvider for AnthropicLlmProvider<T> {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        self.inner
            .messages_create_non_stream(
                &req.model,
                req.system.as_deref(),
                req.messages,
                self.transport.as_ref(),
            )
            .await
    }

    async fn stream(
        &self,
        req: CanonicalRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        self.inner
            .messages_create_stream(
                &req.model,
                req.system.as_deref(),
                req.messages,
                req.tools,
                self.transport.clone(),
            )
            .await
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`, add `pub mod anthropic;` (keep modules alphabetical: place it before `pub mod auth;`) and add the re-export `pub use anthropic::AnthropicLlmProvider;` (before `pub use auth::Auth;`).

- [ ] **Step 5: Run the tests**

Run: `cd lingxi-code && cargo test -p providers`
Expected: PASS (`id_is_anthropic`, `complete_decodes_canned_anthropic_response`, `stream_yields_decoded_events`, plus all earlier tests).

- [ ] **Step 6: Lint clean**

Run: `cd lingxi-code && cargo clippy -p providers --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 7: Commit**

```bash
cd lingxi-code
git add providers/src
git commit -m "feat(llm-p1): AnthropicLlmProvider delegating to api-client (parity-locked)"
```

---

## Task 5: `ProviderApiAdapter` bridge in the orchestrator

**Files:**
- Modify: `lingxi-code/orchestrator/Cargo.toml`
- Create: `lingxi-code/orchestrator/src/provider_adapter.rs`
- Modify: `lingxi-code/orchestrator/src/lib.rs`

- [ ] **Step 1: Add the `providers` dependency**

In `lingxi-code/orchestrator/Cargo.toml`, under `[dependencies]`, add (next to the other path deps, e.g. after the `api-client` line):

```toml
providers = { path = "../providers" }
```

- [ ] **Step 2: Write the bridge with its test (TDD)**

Create `lingxi-code/orchestrator/src/provider_adapter.rs`:

```rust
//! Bridge: adapt any `providers::LlmProvider` to the orchestrator's
//! `OrchestratorApiClient` / `StreamingApiClient` traits. The model string
//! flows through unchanged; provider routing by prefix is wired in P2.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::BoxStream;
use protocol::ConversationMessage;
use providers::{CanonicalRequest, LlmProvider};
use std::sync::Arc;

/// Adapts an `Arc<dyn LlmProvider>` to the orchestrator's API-client traits.
pub struct ProviderApiAdapter {
    provider: Arc<dyn LlmProvider>,
}

impl ProviderApiAdapter {
    /// Wrap a provider.
    #[must_use]
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        let mut req = CanonicalRequest::new(model);
        req.system = system.map(str::to_string);
        req.messages = msgs;
        self.provider.complete(req).await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut req = CanonicalRequest::new(model);
        req.system = system.map(str::to_string);
        req.messages = messages;
        req.tools = tools;
        req.stream = true;
        self.provider.stream(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, UsageApi};
    use futures::StreamExt;
    use providers::Capabilities;
    use std::sync::Mutex;

    /// Records the request it received and returns a canned response.
    struct StubProvider {
        seen_model: Mutex<Option<String>>,
        seen_system: Mutex<Option<String>>,
        caps: Capabilities,
    }

    impl StubProvider {
        fn new() -> Self {
            Self {
                seen_model: Mutex::new(None),
                seen_system: Mutex::new(None),
                caps: Capabilities::anthropic(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn id(&self) -> cost::ProviderId {
            cost::ProviderId::OpenAI
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            *self.seen_model.lock().unwrap() = Some(req.model.clone());
            *self.seen_system.lock().unwrap() = req.system.clone();
            Ok(MessageResponse {
                id: "stub".to_string(),
                model: req.model,
                content: vec![ContentBlockApi::Text { text: "ok".to_string() }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }
        async fn stream(
            &self,
            _req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    #[tokio::test]
    async fn bridge_forwards_model_and_system_to_provider() {
        let stub = Arc::new(StubProvider::new());
        let adapter = ProviderApiAdapter::new(stub.clone());
        let resp = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new())
            .await
            .expect("ok");
        assert_eq!(resp.model, "openai/gpt-4o");
        assert_eq!(stub.seen_model.lock().unwrap().as_deref(), Some("openai/gpt-4o"));
        assert_eq!(stub.seen_system.lock().unwrap().as_deref(), Some("sys"));
    }
}
```

- [ ] **Step 3: Declare + re-export the module**

In `lingxi-code/orchestrator/src/lib.rs`, add `pub mod provider_adapter;` (next to the other `pub mod` lines, e.g. after `pub mod prompt;`) and add `pub use provider_adapter::ProviderApiAdapter;` (after the `pub use prompt::{ ... };` block).

- [ ] **Step 4: Run the bridge test**

Run: `cd lingxi-code && cargo test -p orchestrator provider_adapter`
Expected: PASS (`bridge_forwards_model_and_system_to_provider`).

- [ ] **Step 5: Lint clean**

Run: `cd lingxi-code && cargo clippy -p orchestrator --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 6: Commit**

```bash
cd lingxi-code
git add orchestrator/Cargo.toml orchestrator/src/provider_adapter.rs orchestrator/src/lib.rs Cargo.lock
git commit -m "feat(llm-p1): ProviderApiAdapter bridge to orchestrator API-client traits"
```

---

## Task 6: Back-compat gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: Format the whole workspace**

Run: `cd lingxi-code && cargo fmt --all`
Then verify nothing else changed: `git status --short` should show only formatting (ideally nothing).

- [ ] **Step 2: Confirm the new crates are clippy-clean**

Run: `cd lingxi-code && cargo clippy -p providers -p orchestrator --all-targets -- -D warnings`
Expected: no warnings/errors.

- [ ] **Step 3: Run the new + touched crate tests**

Run: `cd lingxi-code && cargo test -p providers -p orchestrator`
Expected: all PASS, 0 failed.

- [ ] **Step 4: Back-compat gate — build the whole workspace**

Run: `cd lingxi-code && cargo build --workspace`
Expected: `Finished` with no errors (warnings from unrelated crates are acceptable; none should be new).

Optional deeper confidence (heavier): `cd lingxi-code && cargo test --workspace` — expected 0 failures (P1 changes nothing existing: `init.rs` is untouched, so production still wires `AnthropicProviderAdapter`).

- [ ] **Step 5: Dependency gate**

Run: `cd lingxi-code && bash scripts/check-deps.sh`
Expected: `check-deps: OK — <N> workspace crates, no §8.1 dependency violations`.

- [ ] **Step 6: Tag the sub-plan locally (no remote push)**

```bash
cd lingxi-code
git tag -a llm-p1 -m "LLM Providers P1 (Core): providers crate + LlmProvider/WireCodec + Anthropic provider + bridge"
git --no-pager tag -l "llm-p1"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§3 / §12 P1):**
- `providers` crate (§3.1) → Task 1.
- `CanonicalRequest` / `Capabilities` / `Auth` / `CodecError` (§2/§3.3) → Task 2.
- `LlmProvider` + `WireCodec` + `SseDecoder` (§3.3) → Task 3.
- `GenericClient` (§3.3) → Task 3.
- Anthropic-as-`LlmProvider`, parity-locked (§3.2) → Task 4.
- `ProviderApiAdapter` bridge, orchestrator routes through it (§3.1) → Task 5.
- Back-compat gate, all existing tests green (§11/§12) → Task 6.
- Frozen `traits/` untouched → no task modifies `traits/` (verified: only `protocol`/`api-client`/`cost`/`traits` are *read* as deps).
- *Out of P1 (correctly deferred to P2):* `ModelSpec`, `ProviderRegistry`, settings `providers` schema, `init.rs` wiring, `switch_model` routing. Codecs (OpenAI/Gemini) are P3/P4. `GenericClient` lands here with a mock codec so the harness is proven before real codecs.

**2. Placeholder scan:** none — every code step shows complete file/section content and every run step gives an exact command + expected result.

**3. Type consistency:** `LlmProvider::{complete, stream}`, `WireCodec::{encode_request, decode_response, new_stream_decoder}`, `SseDecoder::{push, finish}`, `CanonicalRequest::new`, `Capabilities::anthropic`, `GenericClient::new`, `AnthropicLlmProvider::new`, `ProviderApiAdapter::new` are named identically across all tasks. `StreamEvent` resolves to `api_client::types::StreamEvent` everywhere (matching the orchestrator's `StreamingApiClient` signature). `AnthropicLlmProvider<T>` is generic over the transport (required by `AnthropicProvider`'s sized-`T` methods); `GenericClient` holds `Arc<dyn HttpTransport>`; both erase to `Arc<dyn LlmProvider>`.
