# LLM Client Crate - Provider Auth, Transport, Streaming, and Cost (Design)

- **Date:** 2026-06-09
- **Status:** Draft spec; approved concept; pending written-spec review
- **Scope choice:** External reusable Rust crate, not a LingXi-only wrapper
- **Working workspace crate name:** `llm-client`
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
- Keep credential lookup separate from auth application so environment variables,
  static keys, OAuth token sources, cloud token sources, keyrings, and host-managed
  secret stores can be swapped without changing protocol or transport code.
- Normalize provider responses into canonical responses, streaming events, and token
  usage.
- Compute cost from resolved `provider + model + token usage breakdown`, not from a
  hardcoded provider path or model-string prefix guess.
- Expose route/model listing from configured profiles so hosts can implement `/model`
  or picker UIs without hardcoded examples.
- Keep raw provider metadata available for diagnostics while redacting secrets by
  default.
- Feature-gate heavyweight cloud auth, Tower adapters, and optional transports so the
  crate can remain usable in mobile/minimal builds.
- Let LingXi runtime code call `llm-client` directly, so `orchestrator`,
  `sidequery`, tools, and subagents share one provider communication path instead
  of carrying compatibility adapters for the old client traits.

## 4. Non-goals

- No agent orchestration, tool execution, permission policy, secret storage UI, session
  management, or model-selection UX inside the crate.
- No standalone proxy server in the first design.
- No prompted tool-call shim for models without native tool support.
- No provider-side `count_tokens` endpoint in the first implementation wave. The crate
  preserves usage returned by provider responses and may add explicit token-counting
  APIs later.
- No direct ownership of LingXi's session-level cost summaries. The crate estimates
  per-call cost and emits normalized usage; LingXi's existing cost/session layer can
  aggregate it.
- No dependency from the reusable core crate on LingXi runtime crates such as
  `api-client`, `orchestrator`, `telemetry`, existing `providers`, existing `cost`, or
  existing `protocol`.
- No compatibility guarantee for LingXi's old `api-client` / `providers` /
  `OrchestratorApiClient` communication seams. Those seams may be replaced during
  migration instead of wrapped.
- Not every designed route family ships with runtime migration in the first
  implementation wave. The first wave is Anthropic first-party, OpenAI Chat
  Completions, OpenAI-compatible Chat Completions profiles, and Gemini. OpenAI
  Responses, Vertex, Bedrock, and Azure are designed here and implemented in follow-up
  waves.
- No mid-stream automatic provider failover unless a caller opts into a future
  resumable-stream policy.

## 5. Locked decisions

| Decision | Choice |
|---|---|
| Crate shape | External reusable crate, not a LingXi-only wrapper |
| Core architecture | Route composition: `Protocol + Endpoint + Authenticator + Transport + Usage/Cost` |
| Request signing | `Authenticator` runs after URL, headers, and body are prepared |
| Credential lookup | `CredentialProvider` resolves secrets/tokens before `Authenticator` applies them |
| Retry scope | Retry connection/setup failures and retryable responses; do not replay an already-yielding stream by default |
| Cost basis | `provider + concrete model + normalized token buckets` |
| Pricing catalog | Built-in default catalog plus external override provider |
| Unknown pricing | Core default is `estimated=false`; hosts may opt into fallback pricing or cost-required errors |
| Tower | Optional adapter layer, not the core API |
| Cloud SDKs | Feature-gated: AWS, GCP, Azure are optional |
| LingXi migration | Replace old LingXi communication seams directly; no compatibility adapter crate |
| Core dependencies | Core `llm-client` does not depend on LingXi runtime crates |

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
| `config` | Raw and resolved client configuration, provider profiles, credential references, pricing policy, merge/validation |
| `protocol` | Provider wire encode/decode and stream state machines |
| `route` | Binds one protocol, endpoint, auth, transport, retry policy, and pricing context |
| `credentials` | Secret/token lookup traits and lightweight env/static providers |
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

The core crate owns its provider-neutral types, configuration, pricing structures,
errors, and route construction. LingXi can convert host settings and session state
into these types, but those conversions live at composition roots and must not become
part of the reusable crate.

### 6.3 Configuration boundary

Configuration is a first-class part of the crate, not a LingXi-only concern and not a
transport afterthought. This matches the successful shape in Codex, opencode, and
liter-llm: model/provider profiles are resolved before the request reaches protocol,
auth, transport, or pricing.

Config types:

- `ClientConfig`: raw, serde-friendly client configuration supplied by a host,
  config file, environment overlay, or tests.
- `ResolvedClientConfig`: validated runtime configuration after defaults, feature
  gates, provider profiles, credential references, pricing policy, and route families
  are resolved.
- `ProviderProfile`: provider kind, profile name, base URL, endpoint shape, protocol
  family, auth strategy, auth resolution policy, credential reference, default
  headers/query params, supported models, aliases, capabilities, retry overrides,
  stream timeouts, and pricing overrides.
- `ModelProfile`: requested model id, provider-local request model, display name,
  aliases, capabilities, context window, default generation limits, pricing provider id,
  and billing/pricing model id.
- `CredentialConfig`: serializable references to secret sources such as environment
  variable names, static host-supplied secret ids, OAuth token callbacks, cloud
  credential chains, or keyring references. Secret values themselves stay behind
  `CredentialProvider`.
- `PricingConfig`: built-in catalog selection, external overrides, and unknown-pricing
  policy.
- `AuthResolution`: provider-specific credential precedence and refresh behavior after
  config/profile/host settings have been merged.
- `RedactionConfig`: opt-in raw payload capture plus mandatory secret filters.

Config resolution rules:

- Merge order is explicit: built-in defaults, config file/provider catalog,
  host-provided overrides, then environment/runtime overrides.
- LingXi composition roots first apply LingXi's existing settings semantics, including
  scalar override for `model` and object deep-merge for provider/routing maps, then map
  that merged result into `ClientConfig`. The reusable crate must not silently reinterpret
  LingXi settings merge rules.
- Validation fails before network I/O when auth, protocol, feature flags,
  capabilities, or pricing-required policy are inconsistent.
- `Debug`/diagnostic output for config always redacts credential material and
  secret-bearing headers.
- Route construction consumes `ResolvedClientConfig`, not ad hoc base URL or model
  prefix parsing.

Auth resolution rules:

- Credential precedence is explicit per provider profile. The default Anthropic
  first-party order is: host-provided request credential, explicit API key, explicit
  bearer token, then stored Claude.ai OAuth. If an API key or bearer token is effective,
  Claude.ai OAuth is not considered active.
- Claude.ai subscriber / enterprise flags are resolved from the effective auth source and
  passed into Anthropic retry/fallback policy. They are not recomputed from raw
  environment variables after route resolution.
- OAuth and cloud-token refresh must be single-flight per credential scope. Concurrent
  reactive and proactive refresh attempts for the same token collapse into one refresh.
- Cloud auth profiles declare their own chain: Vertex/GCP token, Bedrock AWS SigV4 or
  bearer, and Azure API key or Azure AD token. Missing cloud SDK features fail at config
  validation, before request execution.

### 6.4 LingXi direct integration

LingXi is a consumer of `llm-client`, not a peer API that the crate must preserve.
Because backward compatibility with the old LingXi communication seams is not a
requirement, migration should replace those seams directly:

- `orchestrator` should depend on `llm_client::LlmClient` / `LlmRequest` /
  `LlmResponse` instead of keeping `OrchestratorApiClient` as the long-term LLM
  boundary.
- streaming turn code should consume `llm_client::LlmEvent` directly instead of
  translating through a separate `StreamingApiClient` compatibility trait.
- `sidequery`, subagents, hook prompt runners, and tool-side LLM calls should share the
  same configured `LlmClient` instance or registry.
- LingXi cost/session aggregation consumes per-call `Usage` and `CostEstimate` from
  `llm-client`; only session summaries remain in LingXi.
- LingXi telemetry consumes redacted metadata from the client response/events; the core
  crate does not depend on the telemetry crate.

Temporary local glue is acceptable inside the same implementation wave while a caller is
being rewritten, but it must be deleted with that rewrite. It must not become a
separate `llm-client-lingxi-adapter` crate or a durable old-seam layer.

LingXi request assembly invariants that must survive direct migration:

- system prompt assembly, output style injection, memory/context blocks, and permission
  instructions remain LingXi orchestration concerns; `llm-client` receives the assembled
  canonical request.
- tool schemas are converted once into `llm-client::ToolDeclaration` and then lowered by
  provider protocol codecs. LingXi must not keep provider-specific tool JSON in the
  orchestrator.
- media trimming keeps the existing oldest-first cap behavior before network I/O, with
  the cap represented as a request-preparation policy or validated model capability.
- vision, document, native-tool, reasoning, structured-output, and streaming capability
  checks fail before network I/O.
- subagents, hook prompt runners, sidequery, tool-side LLM calls, and main turns use the
  same configured client/registry so retry, auth, cost, redaction, and model listings do
  not diverge.
- `max_tokens` recovery, prompt-too-long handling, Opus fallback policy, prompt-cache
  safe parameters, and stream cancellation semantics must have explicit homes in either
  LingXi orchestration or `llm-client` retry/request policy before the old seam is
  deleted.

## 7. Core API shape

The public API has two layers:

1. A convenience `LlmClient` for normal consumers.
2. Explicit `Route` construction for advanced users and composition roots.

Illustrative shape:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest)
        -> Result<LlmStream, LlmError>;
}

pub struct ClientBuilder {
    config: ClientConfig,
}

impl ClientBuilder {
    pub fn from_config(config: ClientConfig) -> Self {
        Self { config }
    }

    pub async fn build(self) -> Result<DefaultLlmClient, LlmError> {
        let resolved = ResolvedClientConfig::resolve(self.config)?;
        DefaultLlmClient::from_resolved_config(resolved).await
    }
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
pub trait CredentialProvider: Send + Sync {
    async fn load(&self, scope: CredentialScope) -> Result<Credential, LlmError>;
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
- credential providers resolve secrets/tokens before request signing;
- auth receives the final request shape and applies headers or signatures;
- transport does not know provider semantics;
- cost is computed from normalized usage after decode.

## 7.1 Credential and secret-source boundary

`CredentialProvider` is separate from `Authenticator`.

- `CredentialProvider` locates or refreshes secret material: environment variables,
  static host-supplied keys, OAuth access-token callbacks, GCP/Azure token sources,
  AWS credential chains, or caller-provided secret stores.
- `Authenticator` transforms the prepared request using resolved credentials: header
  insertion, bearer application, OAuth refresh retry hooks, SigV4 signing, or
  provider-specific auth composition.
- The core crate may provide simple `StaticCredentialProvider` and
  `EnvCredentialProvider`. It does not own keychain/keyring storage, interactive login,
  persistent OAuth token storage, or LingXi's secret UI.
- All credentials expose a redacted debug representation by default.

## 8. Canonical types

The canonical request/response format should be provider-neutral but agent-friendly.
It must carry enough structure for native tool calls, multimodal input, reasoning
models, and streaming deltas.

Core types:

- `ClientConfig`
- `ResolvedClientConfig`
- `ProviderProfile`
- `ModelProfile`
- `CredentialConfig`
- `AuthResolution`
- `LlmRequest`
- `LlmResponse`
- `LlmEvent`
- `Message`
- `ContentBlock`
- `ToolDeclaration`
- `ToolChoice`
- `ToolCall`
- `ToolResult`
- `ResponseFormat`
- `ReasoningConfig`
- `ModelRef`
- `PricingModelRef`
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

### 8.1 Tool schema and structured-output dialects

The canonical tool and structured-output types are communication-layer concerns, not
orchestration policy:

- `ToolDeclaration` stores a JSON-schema input object plus name and description.
- `ToolChoice` represents auto/none/required/specific-tool where a provider supports
  it.
- `ResponseFormat` represents provider-native JSON mode / JSON schema output where a
  provider supports it.
- Protocol codecs own dialect mapping:
  - Anthropic: `input_schema`, `tool_choice`, native structured-output fields when
    available.
  - OpenAI Chat / OpenAI-compatible: `tools[].function.parameters`,
    `tool_choice`, and `response_format`.
  - OpenAI Responses: Responses-native tools and text/JSON format fields.
  - Gemini: `functionDeclarations[].parameters`, `functionCallingConfig`, and the
    Gemini-supported schema subset.
- Each protocol declares whether unsupported schema keywords are preserved, lowered, or
  rejected. The first-wave Gemini codec must normalize or reject unsupported schema
  shapes deterministically; it must not silently produce provider-invalid JSON.

## 9. Usage and cost model

Cost is computed from resolved pricing identity and normalized token buckets. The
pricing identity is not always the same as the model string sent to the provider.
Azure deployments, Bedrock model ARNs, Vertex publisher ids, and OpenAI-compatible
profiles can expose one model id to the user, send another provider-local id on the
wire, and bill against a third catalog key.

```text
Cost input:
  pricing_provider_id
  billing_model
  usage
  pricing_catalog

Cost output:
  pricing_model_ref
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
- Under the default `MarkUnestimated` policy, the cost estimator does not discard the
  LLM response when pricing is unknown. It returns `CostEstimate { estimated: false }`
  plus `Usage`.

Provider identity is explicit. First-class variants are:

- `AnthropicFirstParty`
- `OpenAI`
- `OpenAICompatible { name }`
- `Gemini`
- `VertexGemini`
- `VertexClaude`
- `BedrockClaude`
- `AzureOpenAI`
- `Custom { name }`

Route resolution assigns `ProviderId` before request execution. The resolved route also
emits a `PricingModelRef`:

```text
PricingModelRef {
  pricing_provider_id
  billing_model
  request_model
  display_model
}
```

`request_model` is the provider-local model/deployment value sent on the wire.
`display_model` is what users and `/model` listings show. `billing_model` is the model
key used for pricing. They may be identical for Anthropic/OpenAI first-party models, but
must be configurable independently for Azure deployments, Bedrock/Vertex deployments,
and custom OpenAI-compatible profiles. Pricing, retry metadata, telemetry consumers, and
model listing consume the resolved identity; they do not infer a provider from a raw
model string after routing.

Pricing catalog rules:

- Match exact `pricing_provider_id + billing_model` first.
- Then match explicit model aliases from registry.
- Then match configured prefix/pattern entries.
- External override catalog wins over the built-in catalog.
- The crate never guesses a provider from the model string after routing has resolved
  the provider.

Unknown-pricing policy is configurable:

- `MarkUnestimated` is the reusable core default: return usage plus
  `CostEstimate { estimated: false }`.
- `ApplyFallbackTier` lets a host provide a deliberate fallback tier when product policy
  allows unpriced models to continue running with an approximate estimate.
- `RequirePriced` returns `CostUnavailable` for APIs that explicitly require a priced
  model before making or accepting a response.

LingXi chooses this through `ClientConfig`. The core crate does not bake LingXi's
default unknown-pricing tier into its reusable API.

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
  -> cost estimates pricing_provider_id/billing_model/token cost
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

`CostUnavailable` is reserved for explicit cost-required APIs, invalid pricing catalog
failures, and the `RequirePriced` unknown-pricing policy. Under the reusable core
default `MarkUnestimated` policy, normal LLM calls with unknown model pricing return
the response plus `CostEstimate { estimated: false }`.

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
- display model
- request model
- billing model
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
| OpenAI-compatible | Chat Completions-compatible | Bearer/header/custom | Base URL/profile driven; first wave when the endpoint follows Chat Completions semantics |
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
- request model mapping
- display model mapping
- billing/pricing model mapping
- pricing overrides
- retry policy overrides
- stream idle timeout

Registry APIs:

```rust
pub trait ModelRegistry {
    fn resolve(&self, model: &ModelRef) -> Result<ResolvedRoute, LlmError>;
    fn available_models(&self) -> Vec<ModelListing>;
}

pub struct ModelListing {
    pub provider_id: ProviderId,
    pub profile_name: String,
    pub display_model: String,
    pub request_model: String,
    pub billing_model: String,
    pub aliases: Vec<String>,
    pub capabilities: Capabilities,
    pub pricing_known: bool,
}
```

`available_models()` lists configured profile/model pairs and aliases from the
registry; it does not hardcode examples. LingXi's `/model` implementation consumes
these listings directly.

Capability validation fails before network I/O when possible:

- tools on a model without native tools
- image/document input on a model without multimodal support
- reasoning params on a model without reasoning support
- streaming request on a non-streaming route

First-wave OpenAI-compatible scope:

- OpenAI first-party Chat Completions and OpenAI-compatible Chat Completions share the
  same first-wave protocol implementation.
- OpenAI-compatible first-wave support includes configurable base URL, auth header,
  default headers/query params, declared model list, aliases, and pricing overrides.
- Provider-specific quirks beyond Chat Completions compatibility remain per-profile
  follow-up work, not implicit first-wave behavior.

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

Migration is phased but this document is not the detailed implementation plan. Because
old LingXi communication seams do not need backward compatibility, each phase removes
the old seam it replaces instead of preserving a fallback path.

### Phase 1 - New crate, config, and fixtures

- Add `llm-client` crate.
- Add config/profile resolution, protocol/auth/transport/cost fixtures, and redaction
  tests.
- Build first-wave provider routes from `ClientConfig`.
- Prove the reusable crate can encode/decode key provider fixtures and compute usage/cost
  without depending on LingXi crates.

### Phase 2 - Replace LingXi LLM call boundaries

- Replace `OrchestratorApiClient` / `StreamingApiClient` as the runtime LLM boundary
  with `llm_client::LlmClient` and its request/response/event types.
- Route main conversation, subagents, prompt hooks, tool-side LLM calls, and sidequery
  through the same configured client/registry.
- Map LingXi settings into `ClientConfig` at desktop/mobile/CLI composition roots.
- Feed `Usage` and `CostEstimate` into LingXi session aggregation without preserving the
  old cost-wiring inference path.

### Phase 3 - Remove obsolete communication crates and duplicated paths

- Remove or reduce `api-client` Anthropic communication code only after the deletion
  gate below passes.
- Remove duplicated provider communication logic from the old `providers` crate instead
  of preserving it as a facade.
- Delete model-string provider inference in LingXi cost wiring after all calls use
  resolved route identity.
- Keep LingXi-specific orchestration and telemetry outside `llm-client`.

Deletion gate for old Anthropic communication code:

- 401 -> OAuth refresh -> retry once -> success is covered.
- second 401 after refresh surfaces authentication failure without another refresh loop.
- 400 `max_tokens` context-overflow reparses provider numbers, shrinks `max_tokens`, and
  retries without consuming the normal retry budget.
- three consecutive 529s on non-subscriber Opus with a configured fallback surfaces the
  configured fallback model; non-Opus and subscriber paths do not trigger fallback.
- 413 / prompt-too-long classification still feeds LingXi's proactive/reactive
  compaction behavior.
- SSE setup retry, mid-stream interruption, ping/no-op events, and stream cancellation
  match the existing streaming tests.
- prompt-cache usage, server-side tool usage, `speed` tier, and token buckets flow into
  `Usage` and `CostEstimate` without model-string provider inference.

## 17. Testing

### Protocol golden tests

- Canonical request -> provider body.
- Provider response -> `LlmResponse`.
- SSE frames -> ordered `LlmEvent`.
- Anthropic prompt-cache usage.
- OpenAI Chat and OpenAI-compatible Chat first-wave fixtures; OpenAI Responses fixtures
  before Responses runtime enablement.
- Gemini usage metadata.
- Tool schema dialect fixtures for Anthropic, OpenAI Chat/OpenAI-compatible, OpenAI
  Responses before enablement, and Gemini.
- Bedrock/Vertex wrapper paths.

### Auth tests

- `StaticCredentialProvider` and `EnvCredentialProvider` success/failure paths.
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

- Exact `pricing_provider_id + billing_model` price match.
- every first-class `ProviderId` variant routes to the intended pricing namespace.
- alias and pattern fallback.
- Azure deployment, Bedrock/Vertex deployment ids, and OpenAI-compatible profiles can use
  different display/request/billing model ids without mispricing.
- independent billable token buckets for input, output, cache write, cache read, and
  reasoning output.
- provider/context totals do not double-count billable buckets.
- `MarkUnestimated`, `ApplyFallbackTier`, and `RequirePriced` unknown-pricing policies.
- external pricing override wins over built-in catalog.

### Registry tests

- `available_models()` lists configured profile/model pairs and aliases.
- listings include provider id, profile name, display model, request model,
  billing/pricing model, capabilities, and pricing-known status.
- OpenAI-compatible profiles resolve through the Chat Completions protocol with their
  configured base URL and auth.

### Config tests

- `ClientConfig` parses provider profiles, model aliases, credential refs, pricing
  policy, and retry/redaction settings.
- merge precedence is deterministic: built-in defaults, config/catalog, host overrides,
  then environment/runtime overrides.
- LingXi settings mapping preserves scalar override for `model` and object deep-merge for
  provider/routing maps before producing `ClientConfig`.
- Anthropic auth precedence prefers effective API key/bearer credentials over stored
  Claude.ai OAuth, and subscriber/fallback policy consumes the resolved auth source.
- OAuth/cloud refresh is single-flight per credential scope.
- invalid auth/protocol/feature/capability combinations fail before network I/O.
- `Debug` and error output redact API keys, bearer tokens, credential refs with secret
  values, and secret-bearing headers.
- `ResolvedClientConfig` constructs routes without model-string provider guessing.

### LingXi migration tests

- Existing Anthropic byte/parity fixtures stay unchanged where they are meant to be
  byte-locked.
- Main orchestrator, subagent, hook prompt, and sidequery paths call the same configured
  `llm-client` instance or registry.
- media trimming, vision/tool/reasoning/stream capability gating, max-token recovery,
  prompt-too-long handling, Opus fallback policy, and stream cancellation are preserved
  after direct-client migration.
- LingXi session cost aggregation consumes `llm-client` usage/cost outputs and no longer
  infers provider id from raw model strings.
- `engine-mobile` feature checks prove cloud auth and Tower deps are absent by
  default.

## 18. Acceptance criteria

- A reusable `llm-client` crate exists with config/registry/route/protocol/auth/
  transport/cost modules.
- Anthropic first-party, OpenAI Chat Completions, OpenAI-compatible Chat Completions,
  and Gemini have fixture-backed complete and stream support in the first
  implementation wave.
- OpenAI Responses has route-family design now, with fixtures required before its
  runtime enablement wave.
- `available_models()` exposes configured profile/model listings, aliases,
  display/request/billing model ids, capabilities, and pricing-known status without
  hardcoded examples.
- Cost estimation uses resolved pricing provider, billing model, and token buckets.
- Unknown pricing follows the configured policy; the reusable core default does not
  block LLM responses.
- First-wave protocols have deterministic tool schema dialect fixtures.
- LingXi routes main conversation, subagent, hook prompt, tool-side LLM, and sidequery
  calls directly through `llm-client` types.
- Existing Claude Code parity tests for Anthropic pass, and old Anthropic
  communication code is not deleted until the Phase 3 deletion gate passes.
- Mobile/minimal builds can opt out of cloud auth and Tower dependencies.

## 19. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Anthropic parity regression | Start with golden fixtures, fixture-backed Claude Code edge cases, and the Phase 3 deletion gate before deleting old code |
| API over-generalization | Keep core API to config/registry/route/protocol/auth/transport/cost; make Tower and advanced catalogs optional |
| Cloud auth dependency weight | Feature-gate AWS/GCP/Azure and verify mobile dependency trees |
| Cost catalog drift | Allow external pricing overrides and mark estimates with pricing source |
| Provider error mismatch | Map all provider errors through canonical `LlmError` and keep raw metadata redacted |
| Streaming replay bugs | Do not replay streams after semantic events are emitted |
| Direct migration blast radius | Replace one LingXi LLM call family at a time and delete each old seam only after its direct-client tests pass |

## 20. Open implementation choices resolved for planning

These choices are fixed for the implementation plan:

- Workspace crate name: `llm-client`.
- Core architecture: route composition, not a provider-only trait wrapper.
- First-wave route families: Anthropic first-party, OpenAI Chat Completions,
  OpenAI-compatible Chat Completions, and Gemini.
- Cloud auth route families are designed now and implemented behind features.
- Tower support is optional and not required for first runtime migration.
- `count_tokens` is deferred outside the first implementation wave.
- Cost estimation lives in the new crate and uses resolved pricing provider plus
  billing model; session aggregation remains outside.
- Config is first-class: provider profiles, model aliases, credential refs, pricing
  policy, retry, and redaction are resolved before route construction.
- LingXi directly consumes `llm-client`; no `llm-client-lingxi-adapter` crate is planned.
