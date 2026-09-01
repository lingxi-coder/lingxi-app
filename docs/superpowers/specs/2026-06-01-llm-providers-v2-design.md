# LLM Providers v2 — Multimodal, Reasoning, Cloud Auth & Routing (Design)

> Builds on the completed v1 `providers` crate (on `main` @ v0.11.0). v1 shipped
> the `LlmProvider` abstraction, pure `WireCodec`/`SseDecoder`, `GenericClient`,
> the Anthropic/OpenAI/Gemini codecs, `ModelSpec`/profiles/`ProviderRegistry`,
> and `ProviderApiAdapter`. This document designs the six v2 capabilities as a
> single spec, executed in phased sub-plans P1–P8.

---

## §0 · Context & motivation

v1 made LingXi multi-provider for **text + tools** over **API-key auth**, with
Anthropic as the byte-identical default. Six capabilities were explicitly
deferred (v1 spec §1 non-goals, §14 future work). v2 closes them:

1. **Image / vision input** — the agent can send images to vision-capable models.
2. **Reasoning-model params** — request-side reasoning controls (effort / thinking
   budget) and decoding reasoning traces into the canonical `Thinking` block.
3. **Azure OpenAI** — the deployment-URL + `api-key` variant of the OpenAI wire.
4. **Vertex + Bedrock signed auth** — GCP service-account tokens (Vertex) and AWS
   SigV4 (Bedrock), using official cloud crates for signing/token-minting only.
5. **Profile-accurate `/model` listing** — enumerate configured profiles + their
   declared models + aliases instead of hardcoded examples.
6. **Core router** — model aliases, fallback chains, and retry/backoff.

### Locked scope decisions (brainstorm outcomes)

| # | Decision | Choice |
|---|---|---|
| Slicing | One spec, phased execution | **All six in a single spec**, P1–P8 sub-plans |
| Auth impl | Signing / token-minting strategy | **Official cloud crates** (`aws-sigv4`, `aws-config`, `gcp_auth`, `aws-smithy-eventstream`) — for signing/token/frame-decode only; the model request still flows through `platform_api::HttpTransport` |
| Bedrock breadth | Which Bedrock models | **Claude-on-Bedrock only** (reuse the Anthropic Messages body) |
| Router scope | Which router features | **Core only**: aliases + fallback chains + retry/backoff. No budgets, no load-balancing, no cooldowns |

---

## §1 · Goals & non-goals

### Goals (v2)
- Image input across Anthropic / OpenAI / Gemini (and the Azure/Vertex/Bedrock
  variants that wrap them).
- Request-side reasoning params + decode reasoning into canonical `Thinking`.
- Azure OpenAI, Vertex (Gemini), and Bedrock (Claude) as first-class profiles.
- An async, request-aware auth seam that supports SigV4 + cloud token minting.
- A core router (aliases, fallback, retry) layered on `ProviderRegistry`.
- Accurate `/model` listing from configured profiles + aliases.

### Non-goals (v2) — explicit
- **Multi-family Bedrock** (Titan / Llama / Mistral / etc.) — Claude-on-Bedrock only.
- **Router budgets / multi-key load-balancing / cooldowns / health-checks** — core
  routing only.
- **Image *generation*, audio, embeddings** endpoints.
- **Request-side Anthropic extended-thinking** via the providers crate — the
  Anthropic path stays delegated to `api_client::AnthropicProvider`, unchanged.
- **Prompt-caching** for non-Anthropic providers.

### Invariant (relaxed precisely from v1)
v1's rule was "Anthropic byte-identical, full stop." v2 adds a canonical image
content block, so the rule becomes:

> **Anthropic (and OpenAI/Gemini) request/response bytes are byte-identical for
> every image-free, reasoning-param-free conversation** — i.e. the entire existing
> parity suite still passes unchanged. Image encoding and reasoning params are
> net-new behavior, active only when an image block or reasoning param is present.

---

## §2 · Architecture — the two new seams

Everything in v2 hangs off two additions to the v1 crate.

### §2.1 · Seam A — async `Authenticator`

**Problem.** v1's `WireCodec::encode_request(req, &Auth) -> HttpRequest` is
synchronous and the codec attaches auth headers itself. AWS SigV4 must sign over
the *fully-built* request (HTTP method + canonical URI + canonical query +
canonical headers + `SHA256(body)` + ISO-8601 timestamp + credential scope), and
GCP/Azure token minting is *async* (and cached/refreshed). Neither fits a pure,
synchronous codec.

**Design.**
- `WireCodec::encode_request` drops the `auth` parameter and becomes
  **auth-agnostic** (still pure, still synchronous):
  ```rust
  fn encode_request(&self, req: &CanonicalRequest) -> Result<HttpRequest, CodecError>;
  ```
- New trait, applied by `GenericClient` **after** encode, **before** transport:
  ```rust
  #[async_trait]
  pub trait Authenticator: Send + Sync {
      /// Attach/replace auth on the built request (headers, and for SigV4 a
      /// signed `Authorization` over method+uri+query+headers+SHA256(body)+date).
      async fn authorize(&self, req: &mut HttpRequest) -> Result<(), ApiError>;
  }
  ```
- `GenericClient` holds `authenticator: Arc<dyn Authenticator>` (replacing the
  `auth: Auth` field) and calls `self.authenticator.authorize(&mut http).await?`
  in both `complete` and `stream` after `encode_request`.

**Impls.**
| Authenticator | Used by | Behavior |
|---|---|---|
| `StaticAuth(Auth)` | OpenAI, Gemini, Azure(api-key), custom | Wraps the v1 `Auth` enum (`None`/`Bearer`/`Header`); synchronous header push inside an async fn |
| `SigV4Authenticator` | Bedrock | `aws-config` resolves credentials (env / profile / SSO / IMDS); `aws-sigv4` signs the built request for service `bedrock`, region from profile |
| `GcpTokenAuthenticator` | Vertex | `gcp_auth` provides a cached, auto-refreshed OAuth2 access token (SA key file via `GOOGLE_APPLICATION_CREDENTIALS`, or ADC/metadata); attached as `Authorization: Bearer` |
| `AzureAdAuthenticator` | Azure (optional) | Azure AD token → `Authorization: Bearer` (when `apiKeyEnv` is absent and AD is configured) |

**Blast radius (parity-safe).** Only the OpenAI and Gemini codecs change (auth
moves out; their emitted request *bytes are unchanged* — same headers, attached
one stage later). The **Anthropic path is `AnthropicLlmProvider`, which delegates
to `api_client::AnthropicProvider` and does not use `WireCodec`/`GenericClient`
at all → completely untouched.** The `Auth` enum is retained (wrapped by
`StaticAuth`), so no concept is lost.

### §2.2 · Seam B — router as `LlmProvider` decorators

**Problem.** `ModelRouter::resolve(model) -> Resolved { provider, model }` returns
a *single* provider, so fallback ("try A, on error try B") cannot live inside
`resolve()`.

**Design.**
- **Aliases** resolve *before* profile lookup: a `BTreeMap<String, String>`
  (`alias → "provider/model"`) consulted at the top of `resolve()`; an alias maps
  to a concrete `provider/model` string which then parses via `ModelSpec` as today.
- **Fallback / retry are `LlmProvider` decorators** the registry composes:
  ```rust
  pub struct RetryingProvider { inner: Arc<dyn LlmProvider>, max_attempts: u32, backoff_ms: u64 }
  pub struct FallbackProvider { chain: Vec<Arc<dyn LlmProvider>> } // try in order on retryable error
  ```
  Both implement `LlmProvider` (delegating `id`/`capabilities` to the primary).
  `resolve()` returns the (possibly decorated) `Arc<dyn LlmProvider>`; the
  orchestrator's `ProviderApiAdapter` is **unchanged**.
- **Retryable-error classification:** transient HTTP (429, 5xx, transport I/O) is
  retryable/failover-eligible; 4xx (bad request, auth, capability guardrail) is
  terminal and short-circuits. A small `is_retryable(&ApiError) -> bool` helper.
- **Streaming + fallback:** failover decides on the *initial* `stream()` result
  (connection / first error). Once a stream is yielding events, a mid-stream error
  is surfaced as-is (no mid-stream provider switch in v2) — documented limitation.

---

## §3 · Per-capability design

### §3.1 · Vision (image input)

**Canonical type (additive, `protocol` crate).**
```rust
pub enum ContentBlock {
    Text { text: String },
    ToolUse { /* … */ },
    ToolResult { /* … */ },
    Thinking { /* … */ },
    Image { source: ImageSource },          // NEW
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },  // media_type e.g. "image/png"
    Url { url: String },
}
```
Additive serde variant — existing messages and session JSONL deserialize
unchanged; only messages that actually contain an image carry the new block.

**Per-codec encoding.**
| Provider | Image encoding |
|---|---|
| Anthropic (`api-client`) | `{ "type":"image", "source": { "type":"base64", "media_type":…, "data":… } }` or `{ "type":"image", "source": { "type":"url", "url":… } }` |
| OpenAI | content-parts array: `{ "type":"image_url", "image_url": { "url": "data:<media_type>;base64,<data>" } }` (or the raw URL for `Url`) |
| Gemini | a `parts` entry: `{ "inlineData": { "mimeType":…, "data":… } }` (Base64) or `{ "fileData": { "mimeType":…, "fileUri":… } }` (Url) |

**Capability gate.** `Capabilities.vision` set true for vision-capable profiles
(gpt-4o/4.1, gemini-1.5/2.x, claude). If a request contains an `Image` block and
`!capabilities.vision`, the adapter **fails fast** with `InvalidRequest`
("model `X` does not support image input") — mirrors v1's tool guardrail; we
never silently drop content.

**Ingestion wiring (traced in P2).** M7-10 added image paste to the TUI prompt.
Today that path is presentation-side; P2 must route a pasted image into a
`ContentBlock::Image` on the outgoing user message (orchestrator/session), and
confirm session JSONL round-trips it. This trace is the first step of P2.

### §3.2 · Reasoning params

**Canonical request fields (additive, `CanonicalRequest`).**
```rust
pub reasoning_effort: Option<ReasoningEffort>, // Low | Medium | High
pub thinking_budget:  Option<u32>,             // max thinking tokens
```
Both default `None` → existing requests serialize unchanged.

**Per-provider mapping.**
| Provider | Request mapping | Decode |
|---|---|---|
| OpenAI o-series (o1/o3/o4-mini, reasoning GPT-5) | set `reasoning_effort`; **`max_tokens` → `max_completion_tokens`**; **omit `temperature`** (rejected by these models) | reasoning summary (when present) → `Thinking` |
| Gemini 2.5 (flash/pro thinking) | `generationConfig.thinkingConfig.thinkingBudget` (+ `includeThoughts: true`) | `thought`-flagged parts → `Thinking` |
| DeepSeek-R1 (OpenAI-compatible) | no request param | `reasoning_content` delta/field → `Thinking` |
| Anthropic | unchanged (delegated path) | unchanged |

**Capability descriptor.** Extend:
```rust
pub enum ReasoningSupport { None, DecodeOnly, Effort, Budget }
```
`Effort`/`Budget` imply decode. A profile **declares reasoning style** (see §4)
so the `max_completion_tokens`/no-`temperature` switch is explicit, not a
model-name heuristic. Decode is DecodeOnly (reasoning is shown but never
re-encoded into request history — v1 rule preserved).

### §3.3 · Azure OpenAI

- New `ProviderKind::AzureOpenAi`. Reuses the **OpenAI chat-completions body**.
- `OpenAiCodec` gains a `UrlStyle`:
  ```rust
  enum UrlStyle {
      OpenAi { base_url: Option<String> },
      Azure  { base_url: String, deployment: String, api_version: String },
  }
  ```
  Azure URL: `{base_url}/openai/deployments/{deployment}/chat/completions?api-version={api_version}`.
- Auth: `api-key: <key>` header via `StaticAuth(Auth::Header{…})`, or
  `AzureAdAuthenticator` when no `apiKeyEnv` is set.

### §3.4 · Vertex (Gemini on Vertex AI)

- New `ProviderKind::Vertex { project, region }`. Reuses the **Gemini body**.
- `GeminiCodec` gains a `UrlStyle`:
  ```rust
  enum UrlStyle {
      GeminiApi { base_url: Option<String> },      // x-goog-api-key (v1)
      Vertex    { project: String, region: String }, // OAuth bearer
  }
  ```
  Vertex URL: `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:generateContent` (+ `:streamGenerateContent?alt=sse`).
- Auth: `GcpTokenAuthenticator`.

### §3.5 · Bedrock (Claude-on-Bedrock only)

- New `ProviderKind::Bedrock { region }`. Model string e.g.
  `bedrock/anthropic.claude-3-5-sonnet-20241022-v2:0`.
- New `BedrockCodec`:
  - **Body** = the Anthropic Messages body, but **without** a top-level `model`
    (model is in the URL) and **with** `anthropic_version: "bedrock-2023-05-31"`.
    To avoid drift and to keep the live Anthropic provider untouched, P6 extracts
    a small **pure Anthropic body+SSE mapper** (`anthropic_wire`) inside the
    `providers` crate, shared by `BedrockCodec` (and a future first-party
    Anthropic codec). The existing `api_client::AnthropicProvider` is unchanged.
  - **URL:** `https://bedrock-runtime.{region}.amazonaws.com/model/{modelId}/invoke`
    (non-streaming) / `/invoke-with-response-stream` (streaming).
  - **Auth:** `SigV4Authenticator` (service `bedrock`, region from profile).
  - **Streaming:** the response body is an **AWS event-stream** (binary frames,
    `application/vnd.amazon.eventstream`); each frame wraps a JSON chunk that is an
    Anthropic stream event. The Bedrock `SseDecoder` decodes frames with
    `aws-smithy-eventstream`, then maps the inner Anthropic event JSON to canonical
    `StreamEvent`s via the shared `anthropic_wire` mapper.

### §3.6 · `/model` listing

- `ProviderProfile` gains optional `models: Vec<String>` (provider-local ids).
- `ModelRouter` gains:
  ```rust
  fn available_models(&self) -> Vec<String>; // ["anthropic/claude-opus-4-7", "groq/llama-3.3-70b", "@fast", …]
  ```
  enumerating each profile's `{profile}/{model}` plus declared aliases (rendered
  with an `@` sentinel or listed separately — finalized in P7).
- `handle_impl::list_available_models` consumes `available_models()`, replacing the
  hardcoded `openai/gpt-4o`-style example list.

### §3.7 · Router config

Composed by the registry from the new `routing` settings block (§4): alias map,
fallback chains (keyed by alias or by `provider/model`), and a retry policy
(`max_attempts`, `backoff_ms`).

---

## §4 · Configuration surface

Extends the v1 `providers` object and adds a sibling `routing` object. All
camelCase, all optional, `deny_unknown_fields`-tolerant (merged via the existing
`DeepMerge` strategy already wired for `providers`).

```jsonc
{
  "providers": {
    "azure": {
      "kind": "azureOpenAi",
      "baseUrl": "https://my-resource.openai.azure.com",
      "azureDeployment": "gpt-4o",
      "azureApiVersion": "2024-10-21",
      "apiKeyEnv": "AZURE_OPENAI_KEY"
    },
    "vertex":  { "kind": "vertex",  "project": "my-proj", "region": "us-central1" },
    "bedrock": { "kind": "bedrock", "region": "us-east-1" },
    "groq": {
      "kind": "openAi",
      "baseUrl": "https://api.groq.com/openai/v1",
      "apiKeyEnv": "GROQ_API_KEY",
      "models": ["llama-3.3-70b"],
      "reasoning": "effort"
    }
  },
  "routing": {
    "aliases":  { "fast": "groq/llama-3.3-70b", "smart": "anthropic/claude-opus-4-7" },
    "fallback": { "smart": ["openai/gpt-4o", "gemini/gemini-2.0-flash"] },
    "retry":    { "maxAttempts": 3, "backoffMs": 250 }
  }
}
```

New `ProviderProfile` fields (all `Option`): `azure_deployment`,
`azure_api_version`, `project`, `region`, `models`, `reasoning`
(`"none"|"decodeOnly"|"effort"|"budget"`). New `ProviderKind` variants:
`AzureOpenAi`, `Vertex`, `Bedrock`. Credentials for signed providers come from
the cloud SDK's own discovery (env / SA-key file / IMDS), **not** `apiKeyEnv`.

---

## §5 · Cost / pricing

- Add `cost::ProviderId::{AmazonBedrock, GoogleVertex, AzureOpenAi}` with their own
  reference price tables (Bedrock/Vertex/Azure publish distinct list prices from
  the first-party APIs). Tables seeded for the Claude-on-Bedrock and
  Gemini-on-Vertex models in scope; Azure mirrors OpenAI list prices unless
  overridden.
- The registry already assigns the correct `ProviderId` per profile; `ModelRef`
  is prefix-aware (v1 P5). `cost_wiring::provider_id_for_profile` extends with the
  new kinds.
- Reasoning output tokens map to `TokenUsage::reasoning_output` where the provider
  reports them separately (OpenAI `completion_tokens_details.reasoning_tokens`,
  Gemini `thoughtsTokenCount`); otherwise they fold into `output`.

---

## §6 · Testing strategy (pure-function-first)

- **Unit (pure):** encode/decode fixtures per new feature — image-block encode for
  all three codecs; reasoning request shaping (o-series `max_completion_tokens` +
  no-temp; Gemini `thinkingConfig`); reasoning decode (`reasoning_content`,
  Gemini thoughts) → `Thinking`; Azure/Vertex URL builders; Bedrock body shape.
- **SigV4:** sign a fixed request and assert the canonical request + signature
  against **AWS's published SigV4 test vectors** (deterministic, offline).
- **Router:** `MockTransport` that errors on the first provider → assert
  `FallbackProvider` advances; `RetryingProvider` retries N times then surfaces;
  4xx short-circuits (no retry/failover).
- **Streaming:** Bedrock event-stream frame fixtures → inner Anthropic events →
  canonical `StreamEvent`s.
- **Parity (back-compat gate):** the entire v1 `test-harness` suite stays green —
  Anthropic/OpenAI/Gemini request/response bytes unchanged for image-free,
  reasoning-free conversations. Add v2 fixtures alongside (do not modify v1 locks).
- **No live network:** transport + cloud auth are mocked/stubbed throughout.

---

## §7 · Phased sub-plans (P1–P8)

Executed via subagent-driven development (fresh implementer per task; spec-review
then code-quality review; opus for the riskiest — SigV4 + event-stream). Each
phase tagged locally (`llm-v2-p1` … `llm-v2-p8`). Gates per phase: `cargo test`
on touched crates, `cargo clippy --all-targets -- -D warnings`, and
`bash scripts/check-deps.sh` where deps/graph change.

| Phase | Scope | Depends on | Risk |
|---|---|---|---|
| **P1** | Seam A: drop `auth` from `WireCodec::encode_request`; add `Authenticator` trait + `StaticAuth`; rewire `GenericClient`, registry, OpenAI/Gemini codecs. Pure refactor; parity-safe. | — | Low (broad but mechanical) |
| **P2** | Vision: `ContentBlock::Image` + `ImageSource` in `protocol`; encode in Anthropic/OpenAI/Gemini; `Capabilities.vision`; fail-fast guardrail; session JSONL round-trip; **trace + wire** M7-10 paste → history. | — | Medium (protocol + ingestion trace) |
| **P3** | Reasoning: `CanonicalRequest` fields; `ReasoningSupport` extension; OpenAI o-series + Gemini budget + DeepSeek decode; profile `reasoning` style; cost reasoning tokens. | — | Medium |
| **P4** | Azure OpenAI: `ProviderKind::AzureOpenAi`; `OpenAiCodec` `UrlStyle`; api-key/AzureAD; cost. | P1 | Low |
| **P5** | Vertex: `GcpTokenAuthenticator` (`gcp_auth`); `GeminiCodec` `UrlStyle::Vertex`; `ProviderKind::Vertex`; cost. | P1 | Medium (async token) |
| **P6** | Bedrock: `SigV4Authenticator` (`aws-sigv4`/`aws-config`); `anthropic_wire` pure mapper; `BedrockCodec`; `aws-smithy-eventstream` streaming; `ProviderKind::Bedrock`; cost. | P1 | **High (heaviest)** |
| **P7** | Router + `/model`: aliases, `FallbackProvider`, `RetryingProvider`, `is_retryable`; `routing` settings; `available_models()`; `handle_impl` listing. | P1 | Medium |
| **P8** | Polish/release: `docs/LLM_PROVIDERS.md` updates, `CHANGELOG`, README subsystem row, **version bump v0.12.0**, `deny.toml` + `check-deps` allowances, final holistic review. | P1–P7 | Low |

P1 is the foundation for the signed-auth phases; P2/P3/P7 are independent of auth
and may proceed in parallel with P4–P6 if desired (sequential by default).

---

## §8 · Dependencies & dep-gate plan

New crates, added **only to `providers/Cargo.toml`** (a leaf-ish crate that apps
compose; nothing in `traits`/`engine`/`orchestrator` gains these):

| Crate | Purpose | License |
|---|---|---|
| `aws-config` | AWS credential resolution (env/profile/SSO/IMDS) | Apache-2.0 |
| `aws-credential-types` | credential types for signing | Apache-2.0 |
| `aws-sigv4` | SigV4 request signing (no networking) | Apache-2.0 |
| `aws-smithy-eventstream` | Bedrock binary stream frame decode | Apache-2.0 |
| `gcp_auth` | GCP OAuth2 access-token source (cached/refreshed) | MIT/Apache-2.0 |

Gate actions in P8 (and incrementally as each is introduced): update `deny.toml`
(licenses + advisory allowlist) and `scripts/check-deps.sh` graph rules so
`providers` may depend on these while still forbidding leakage upward. **Usage is
confined to signing / token-minting / frame-decode** — the actual model inference
request is always issued through `platform_api::HttpTransport`, preserving the v1
testability seam. `aws-config`/`gcp_auth` may perform their *own* side-channel I/O
for credential discovery; that is acceptable (auth resolution, not inference).

---

## §9 · Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| R1 | AWS/GCP dep trees are large | Confine to `providers`; signing/token-only (no service SDKs / no `aws-sdk-bedrockruntime`); explicit `deny.toml` review |
| R2 | SigV4 correctness | Test against AWS published canonical-request/signature vectors; use `aws-sigv4` rather than hand-rolling |
| R3 | TUI paste → history wiring deeper than expected | Trace it as the first task of P2 before committing the codec work; fall back to documenting "images via API only" if the TUI path is out of reach this phase |
| R4 | Anthropic-body/stream reuse for Bedrock drifts from the live path | Extract a single pure `anthropic_wire` mapper; assert it against the same fixtures the live Anthropic path uses |
| R5 | `gcp_auth` credential discovery fails in headless/cron | Document explicit `GOOGLE_APPLICATION_CREDENTIALS` SA-key-file; surface a clear auth error |
| R6 | Image block perturbs parity | Additive serde variant; image-free conversations are byte-identical; parity suite is the gate |
| R7 | Mid-stream failover ambiguity | v2 fails over only on the initial `stream()` error; mid-stream errors surface as-is (documented) |

---

## §10 · Open questions / future work (post-v2)

- Multi-family Bedrock (Titan/Llama/Mistral bodies).
- Router budgets, multi-key load-balancing, cooldowns, health-checks.
- Request-side Anthropic extended-thinking through a first-party Anthropic codec
  (would let the `anthropic_wire` mapper replace the delegated path).
- Image generation / audio / embeddings endpoints.
- Prompt-caching for non-Anthropic providers.
