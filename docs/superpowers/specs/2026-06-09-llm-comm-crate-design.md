# LLM Communication Crate - Provider Auth, Transport, Streaming, and Cost (Design)

- **Date:** 2026-06-09
- **Status:** Draft spec; approved concept; pending written-spec review
- **Scope choice:** External reusable Rust crate, not a LingXi-only wrapper
- **Working workspace crate name:** `llm-comm`
- **Primary references:** `opencode/packages/llm`, `codex/codex-rs/codex-api`,
  `codex/codex-rs/model-provider`, `liter-llm/crates/liter-llm`,
  `claude-code/src/services/api`

## 1. Context and motivation

LingXi already has a `providers` crate, but the real LLM communication boundary is
still split:

- `providers` owns provider traits, codecs, routing, cloud auth, and generic clients.
- `api-client` still owns the production Anthropic client, OAuth hook, retry behavior,
  SSE parsing, telemetry, and cost recording.
- `cost` owns pricing and session accumulation, but some provider/model inference
  remains outside the provider route that actually made the call.
- `sidequery`, tools, subagents, and main conversation still have separate wiring
  paths, including Anthropic-specific paths.

The goal is to extract provider communication into one reusable crate that can be used
by LingXi and by external Rust consumers. The crate should own provider auth,
request/response translation, HTTP/SSE communication, retry/error classification,
usage normalization, and model/token-based cost estimation.

The crate must replace the current split implementation without regressing Claude Code
parity. Claude Code remains the behavior reference for Anthropic-specific edge cases;
Codex, opencode, and liter-llm are architecture references.

## 2. Reference takeaways

| Reference | What to reuse conceptually | What not to copy |
|---|---|---|
| Claude Code | Anthropic parity rules: OAuth refresh, 401 refresh, 429/529 behavior, prompt-cache usage, max-token overflow retry, per-model cost recording | Its implementation is tightly coupled to Anthropic and deployment flags; it is not a reusable multi-provider architecture |
| Codex | Request-level auth that can inspect the final request/body; provider/account separation; transport retry separate from endpoint config | OpenAI/Codex endpoint assumptions and product-specific auth state |
| opencode | Route composition: protocol + endpoint + auth + transport; canonical events; secret redaction and error classification | TypeScript runtime shape and app-specific protocol packaging |
| liter-llm | Data-driven provider/pricing catalogs; optional Tower middleware; broad provider registry | A primarily OpenAI-compatible center of gravity that would hide Anthropic/Gemini/Bedrock protocol differences |

## 3. Goals

- Provide a reusable Rust crate for LLM provider communication.
- Support Anthropic, OpenAI Responses, OpenAI Chat Completions, OpenAI-compatible
  endpoints, Gemini, Vertex, Bedrock Claude, and Azure OpenAI as first-class route
  families.
- Make provider auth request-aware: API key, bearer/OAuth, AWS SigV4, GCP token, and
  Azure token all use the same `Authenticator` seam.
- Normalize provider responses into canonical responses, streaming events, and token
  usage.
- Compute cost from resolved `provider + model + token usage breakdown`, not from a
  hardcoded provider path or model-string prefix guess.
- Keep raw provider metadata available for diagnostics while redacting secrets by
  default.
- Feature-gate heavyweight cloud auth, Tower adapters, and optional transports so the
  crate can remain usable in mobile/minimal builds.
- Provide LingXi adapters so `orchestrator`, `sidequery`, tools, and subagents can
  migrate without absorbing provider wire details.

## 4. Non-goals

- No agent orchestration, tool execution, permission policy, secret storage UI, session
  management, or model-selection UX inside the crate.
- No standalone proxy server in the first design.
- No prompted tool-call shim for models without native tool support.
- No direct ownership of LingXi's session-level cost summaries. The crate estimates
  per-call cost and emits normalized usage; LingXi's existing cost/session layer can
  aggregate it.
- No dependency from the reusable core crate on LingXi runtime crates such as
  `api-client`, `orchestrator`, `telemetry`, existing `providers`, existing `cost`, or
  existing `protocol`.
- Not every designed route family ships with runtime migration in the first
  implementation wave. The first wave is Anthropic first-party, OpenAI Chat
  Completions, and Gemini. OpenAI Responses, Vertex, Bedrock, and Azure are designed
  here and implemented in follow-up waves.
- No mid-stream automatic provider failover unless a caller opts into a future
  resumable-stream policy.

## 5. Locked decisions

| Decision | Choice |
|---|---|
| Crate shape | External reusable crate, not a LingXi-only wrapper |
| Core architecture | Route composition: `Protocol + Endpoint + Authenticator + Transport + Usage/Cost` |
| Request signing | `Authenticator` runs after URL, headers, and body are prepared |
| Retry scope | Retry connection/setup failures and retryable responses; do not replay an already-yielding stream by default |
| Cost basis | `provider + concrete model + normalized token buckets` |
| Pricing catalog | Built-in default catalog plus external override provider |
| Tower | Optional adapter layer, not the core API |
| Cloud SDKs | Feature-gated: AWS, GCP, Azure are optional |
| LingXi migration | Use adapters first, then remove duplicated old communication paths after parity is locked |
| Core dependencies | Core `llm-comm` does not depend on LingXi runtime crates |

## 6. Architecture

The crate exposes a high-level client and a lower-level route API.

```text
LlmClient
  -> ModelRouter / ProviderRegistry
  -> Route
      -> Protocol
      -> Endpoint
      -> Authenticator
      -> Transport
      -> RetryPolicy
      -> UsageNormalizer
      -> CostEstimator
```

### 6.1 Public core modules

| Module | Responsibility |
|---|---|
| `types` | Canonical request, response, stream event, message, content, tools, usage, model, provider, cost result |
| `protocol` | Provider wire encode/decode and stream state machines |
| `route` | Binds one protocol, endpoint, auth, transport, retry policy, and pricing context |
| `auth` | API key, bearer, OAuth refresh, SigV4, GCP token, Azure token, composite auth |
| `transport` | HTTP and SSE I/O abstractions; default reqwest transport behind a feature |
| `retry` | Retry/backoff policy over canonical errors and response metadata |
| `registry` | Provider profiles, model aliases, route lookup, available model listing |
| `cost` | Pricing catalog, token-bucket pricing, per-call cost estimate |
| `redaction` | Secret redaction for logs, errors, and debug payloads |
| `tower` | Optional `tower::Service` adapter layer |

### 6.2 Core dependency boundary

The reusable core crate must not depend on LingXi runtime crates:

- no `api-client`
- no `orchestrator`
- no `telemetry`
- no existing `providers`
- no existing LingXi `cost`
- no existing LingXi `protocol`

The core crate owns its provider-neutral types, pricing structures, errors, and route
configuration. LingXi may convert between these types and existing workspace types at
adapter boundaries, but those conversions do not belong in the core crate.

### 6.3 LingXi integration adapters

LingXi adapters are not the core API. They live in a separate integration surface,
preferably a workspace crate such as `llm-comm-lingxi-adapter`:

- `OrchestratorApiClient` adapter
- `StreamingApiClient` adapter
- `CostTracker` sink adapter
- `TelemetryBus` event adapter
- `sidequery` runner adapter

This keeps the reusable crate free of LingXi-specific orchestration and telemetry
types while still allowing a low-risk migration.

## 7. Core API shape

The public API has two layers:

1. A convenience `LlmClient` for normal consumers.
2. Explicit `Route` construction for advanced users and LingXi migration code.

Illustrative shape:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest)
        -> Result<LlmStream, LlmError>;
}

#[async_trait]
pub trait Protocol: Send + Sync {
    fn encode(&self, request: &LlmRequest) -> Result<PreparedBody, LlmError>;
    fn decode_response(&self, response: RawResponse) -> Result<LlmResponse, LlmError>;
    fn stream_decoder(&self) -> Box<dyn StreamDecoder>;
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn apply(&self, request: PreparedRequest) -> Result<PreparedRequest, LlmError>;
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: PreparedRequest) -> Result<RawResponse, LlmError>;
    async fn open_sse(&self, request: PreparedRequest) -> Result<RawStream, LlmError>;
}
```

The exact Rust signatures can be refined during implementation planning, but the
direction is fixed:

- protocol is provider-wire-specific;
- auth receives the final request shape;
- transport does not know provider semantics;
- cost is computed from normalized usage after decode.

## 8. Canonical types

The canonical request/response format should be provider-neutral but agent-friendly.
It must carry enough structure for native tool calls, multimodal input, reasoning
models, and streaming deltas.

Core types:

- `LlmRequest`
- `LlmResponse`
- `LlmEvent`
- `Message`
- `ContentBlock`
- `ToolDeclaration`
- `ToolCall`
- `ToolResult`
- `ReasoningConfig`
- `ModelRef`
- `ProviderId`
- `Usage`
- `CostEstimate`
- `ProviderMetadata`

`ContentBlock` supports at least:

- text
- image
- document
- tool call
- tool result
- thinking/reasoning
- refusal/safety metadata when a provider emits it

Provider-specific fields stay in `ProviderMetadata`; callers should not need to parse
raw provider JSON for normal operation.

## 9. Usage and cost model

Cost is computed from resolved route identity and normalized token buckets.

```text
Cost input:
  provider_id
  concrete_model
  usage
  pricing_catalog

Cost output:
  total_cost_usd
  input_cost_usd
  output_cost_usd
  cache_read_cost_usd
  cache_write_cost_usd
  reasoning_cost_usd
  estimated
  pricing_source
```

Canonical usage separates billable buckets from provider/context totals. Billable
buckets are the only inputs to cost calculation; totals are diagnostics and
context-window inputs.

```text
Usage {
  billable_tokens: TokenUsage
  context_tokens: Option<u64>
  provider_reported_total_tokens: Option<u64>
  server_tool_use: Option<ServerToolUsage>
  provider_metadata
}

TokenUsage {
  input
  output
  cache_write
  cache_read
  reasoning_output
}
```

Invariants:

- Token buckets are independent billable classes. The crate does not assume that a
  provider's reported `input_total` equals `input + cache_read + cache_write`.
- `context_tokens` is separate from billing. When a provider reports a context-window
  total, the codec preserves it. When it does not, the crate may derive a conservative
  context total from known buckets and mark the derivation in metadata.
- `reasoning_output` is populated only when the provider reports reasoning tokens as a
  separately billable class. If reasoning tokens are already folded into output tokens,
  they remain in `output` and metadata records that relationship.
- If a provider only returns totals, the codec sets the known total fields and leaves
  unknown billable buckets at zero with metadata explaining the limitation.
- Unknown pricing never discards the LLM response. It returns `CostEstimate {
  estimated: false }` plus `Usage`.

Pricing catalog rules:

- Match exact `provider_id + model` first.
- Then match explicit model aliases from registry.
- Then match configured prefix/pattern entries.
- External override catalog wins over the built-in catalog.
- The crate never guesses a provider from the model string after routing has resolved
  the provider.

This satisfies the requirement that cost is based on model and token usage, and keeps
future ChatGPT/OpenAI, Gemini, Bedrock, Vertex, and custom models on the same path.

## 10. Data flow

### 10.1 Non-streaming request

```text
caller
  -> LlmClient::complete(request)
  -> registry resolves ModelRef to Route
  -> protocol.encode(request)
  -> endpoint builds URL, query, and base headers
  -> auth.apply(prepared request)
  -> retry executes transport.send()
  -> protocol.decode_response(raw response)
  -> protocol normalizes usage
  -> cost estimates provider/model/token cost
  -> response returned with usage, cost, and redacted metadata
```

### 10.2 Streaming request

```text
caller
  -> LlmClient::stream(request)
  -> same route/auth/transport setup
  -> transport.open_sse()
  -> protocol stream decoder maps raw frames to LlmEvent
  -> final usage event normalizes token usage
  -> final cost estimate is emitted on the terminal event
```

Streaming rules:

- Failures before a stream starts may be retried.
- Once the stream has yielded semantic events, interruption becomes
  `StreamInterrupted`; it is not silently replayed.
- A future resumable policy may be added as an explicit opt-in.

## 11. Error handling

The public error type is stable and provider-neutral:

```text
LlmError
  Authentication
  PermissionDenied
  InvalidRequest
  RateLimited { retry_after, scope }
  QuotaExceeded
  ContextOverflow
  ModelUnavailable
  ProviderInternal
  Transport
  StreamInterrupted
  CostUnavailable
  UnsupportedCapability
```

`CostUnavailable` is reserved for explicit cost-required APIs and invalid pricing
catalog failures. Normal LLM calls with unknown model pricing return the response plus
`CostEstimate { estimated: false }`.

Mapping examples:

- Anthropic auth error or HTTP 401 -> `Authentication`
- Anthropic/OpenAI/Gemini HTTP 429 -> `RateLimited`
- OpenAI billing/quota errors -> `QuotaExceeded`
- provider context-window errors -> `ContextOverflow`
- unsupported image/tool/reasoning request -> `UnsupportedCapability`
- transient I/O, 500/502/503/504, and 529 -> retryable provider/transport errors

Retry policy consumes canonical errors, not provider JSON directly:

- Retry by default: transient transport errors, 429, 500, 502, 503, 504, 529.
- Respect `retry-after` when present.
- Do not retry by default: auth, permission, invalid request, context overflow,
  unsupported capability, and cost unavailability.
- Provider-specific auth refresh, such as Claude OAuth refresh, GCP token refresh, or
  Bedrock credential refresh, is implemented as a policy hook around auth/retry.

## 12. Secret redaction and debug payloads

The crate must redact secrets before logging, telemetry, debug dumps, and error
metadata.

Always redact:

- `Authorization`
- `x-api-key`
- provider-specific API key headers
- API key query parameters
- OAuth access/refresh tokens
- AWS temporary credentials and signatures
- known secret JSON fields

Allowed diagnostic metadata:

- request id
- provider id
- concrete model
- route/profile name
- retry count
- rate-limit headers after secret filtering
- status code
- classified error kind

Raw payload capture is default-off. When enabled, payloads still pass through the
redactor before being written or emitted.

## 13. Provider route families

Designed route families:

| Family | Protocol | Auth | Notes |
|---|---|---|---|
| Anthropic first-party | Messages API | API key, OAuth bearer | Must preserve Claude Code parity edge cases |
| OpenAI Responses | Responses API | Bearer/API key | Primary future OpenAI path; follow-up wave after Chat/Gemini migration parity |
| OpenAI Chat | Chat Completions | Bearer/API key | Needed for OpenAI-compatible endpoints |
| OpenAI-compatible | Chat Completions-compatible | Bearer/header/custom | Base URL/profile driven |
| Gemini | Gemini generateContent | API key or GCP token | Supports native Gemini usage metadata |
| Vertex Gemini | Gemini via Vertex endpoint | GCP token | Separate provider id, endpoint, rate-limit, and pricing from first-party Gemini |
| Vertex Claude | Anthropic Messages-compatible body via Vertex endpoint | GCP token | Separate route family for Claude-on-Vertex deployment parity |
| Bedrock Claude | Anthropic body wrapped for Bedrock | AWS SigV4 or bearer | Separate provider id, endpoint, rate-limit, and pricing from first-party Anthropic |
| Azure OpenAI | OpenAI body with Azure URL style | API key or Azure token | Deployment and API version in profile |

Each family can have multiple route variants when the wire protocol differs. For
example OpenAI Responses and OpenAI Chat are separate protocols, not one codec with
hidden mode flags. Cloud-hosted Claude routes are also separate from first-party
Anthropic because auth, endpoint shape, model ids, pricing, request ids, and
rate-limit metadata differ.

## 14. Registry and profiles

The registry resolves a requested model into a concrete route:

```text
requested ModelRef
  -> alias lookup
  -> provider/profile lookup
  -> model capability validation
  -> route construction
```

Profile fields:

- provider kind
- profile name
- base URL
- auth configuration
- default headers
- query parameters
- supported models
- model aliases
- capabilities
- pricing overrides
- retry policy overrides
- stream idle timeout

Capability validation fails before network I/O when possible:

- tools on a model without native tools
- image/document input on a model without multimodal support
- reasoning params on a model without reasoning support
- streaming request on a non-streaming route

## 15. Dependency and feature policy

The crate should be usable in minimal and mobile builds.

Core dependencies should stay small:

- `serde`
- `serde_json`
- `thiserror`
- `async-trait` or equivalent stable async-trait strategy
- `futures`

Feature-gated dependencies:

- `transport-reqwest`
- `aws-auth`
- `gcp-auth`
- `azure-auth`
- `tower`
- `native-tls` / `rustls` choice where the transport supports it
- provider catalog loading formats beyond JSON

LingXi mobile builds must be able to exclude AWS/GCP/Azure/Tower dependencies unless a
mobile feature explicitly opts into them.

## 16. LingXi migration plan at design level

Migration is phased but this document is not the detailed implementation plan.

### Phase 1 - New crate and fixtures only

- Add `llm-comm` crate.
- Add protocol/auth/transport/cost fixtures.
- Keep existing LingXi runtime wiring unchanged.
- Prove the reusable crate can encode/decode key provider fixtures without runtime
  migration risk.

### Phase 2 - LingXi adapters

- Add adapters for `OrchestratorApiClient` and `StreamingApiClient`.
- Add sidequery runner backed by the same `llm-comm` client.
- Add cost and telemetry sink adapters.
- Keep the old Anthropic path behind a feature or fallback switch until parity is
  proven.

### Phase 3 - Runtime migration

- Route main conversation, subagents, tools, and sidequeries through the new client.
- Ensure resolved route identity is used for all usage/cost attribution.
- Keep old paths available only for comparison and emergency rollback.

### Phase 4 - Remove duplicated communication paths

- Remove or reduce `api-client` Anthropic communication code after parity locks.
- Remove duplicated provider communication logic from the old `providers` crate or
  turn it into a compatibility facade.
- Keep LingXi-specific orchestration and telemetry outside `llm-comm`.

## 17. Testing

### Protocol golden tests

- Canonical request -> provider body.
- Provider response -> `LlmResponse`.
- SSE frames -> ordered `LlmEvent`.
- Anthropic prompt-cache usage.
- OpenAI Chat first-wave fixtures; OpenAI Responses fixtures before Responses runtime
  enablement.
- Gemini usage metadata.
- Bedrock/Vertex wrapper paths.

### Auth tests

- API key and bearer headers.
- OAuth refresh after stale token.
- AWS SigV4 signs final URL, headers, and body.
- GCP/Azure token auth applies refreshed bearer token.
- Secret redaction removes all auth material.

### Retry and error tests

- 429, 500, 502, 503, 504, 529.
- `retry-after` handling.
- stream setup failure retries.
- mid-stream interruption does not auto-replay.
- auth refresh then retry.
- context overflow and unsupported capability do not retry.

### Cost tests

- Exact provider/model price match.
- alias and pattern fallback.
- independent billable token buckets for input, output, cache write, cache read, and
  reasoning output.
- provider/context totals do not double-count billable buckets.
- unknown model returns usage plus non-estimated cost.
- external pricing override wins over built-in catalog.

### LingXi migration tests

- Existing Anthropic byte/parity fixtures stay unchanged where they are meant to be
  byte-locked.
- Main orchestrator adapter emits the same canonical responses as the current path.
- `sidequery` uses the same route/cost path as the main client.
- `engine-mobile` feature checks prove cloud auth and Tower deps are absent by
  default.

## 18. Acceptance criteria

- A reusable `llm-comm` crate exists with route/protocol/auth/transport/cost modules.
- Anthropic first-party, OpenAI Chat Completions, and Gemini have fixture-backed
  complete and stream support in the first implementation wave.
- OpenAI Responses has route-family design now, with fixtures required before its
  runtime enablement wave.
- Cost estimation uses resolved provider/model plus token buckets.
- Unknown pricing does not block LLM responses.
- LingXi can route main conversation and sidequery calls through the new crate via
  adapters.
- Existing Claude Code parity tests for Anthropic still pass or any intentional change
  is explicitly reviewed.
- Mobile/minimal builds can opt out of cloud auth and Tower dependencies.

## 19. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Anthropic parity regression | Start with golden fixtures and keep old path as rollback until runtime parity is proven |
| API over-generalization | Keep core API to route/protocol/auth/transport/cost; make Tower and advanced catalogs optional |
| Cloud auth dependency weight | Feature-gate AWS/GCP/Azure and verify mobile dependency trees |
| Cost catalog drift | Allow external pricing overrides and mark estimates with pricing source |
| Provider error mismatch | Map all provider errors through canonical `LlmError` and keep raw metadata redacted |
| Streaming replay bugs | Do not replay streams after semantic events are emitted |
| LingXi adapter leakage | Keep adapters in a separate integration crate outside the core crate API |

## 20. Open implementation choices resolved for planning

These choices are fixed for the implementation plan:

- Workspace crate name: `llm-comm`.
- Core architecture: route composition, not a provider-only trait wrapper.
- First-wave route families: Anthropic first-party, OpenAI Chat Completions, and
  Gemini.
- Cloud auth route families are designed now and implemented behind features.
- Tower support is optional and not required for first runtime migration.
- Cost estimation lives in the new crate; session aggregation remains outside.
- LingXi-specific adapters are compatibility layers, not reusable crate core.
