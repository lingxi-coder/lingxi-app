# LLM Providers — Multi-Provider Model Backend (Design)

- **Date:** 2026-06-01
- **Status:** Approved (brainstorm) — pending implementation plan
- **Branch:** `llm-providers` (off `61ab521`, the M9 / v0.10.0 tip)
- **References:** [rig-core](https://github.com/0xPlaygrounds/rig/tree/main/crates/rig-core) (in-process provider trait abstraction), [litellm](https://github.com/BerriAI/litellm) (translation breadth + routing/proxy layer)

## §0 · Context & motivation

LingXi today talks to exactly one backend: Anthropic. `api-client::AnthropicProvider`
is a concrete struct that builds Anthropic Messages-API requests (`/v1/messages`,
`x-api-key`, `anthropic-version`, `anthropic-beta`), parses Anthropic SSE, and runs
the retry / 429 / OAuth-refresh middleware. All network I/O routes through the frozen
`platform_api::HttpTransport` port.

Three facts make multi-provider support a natural, low-risk extension rather than a
rewrite:

1. **The abstraction seam already exists.** The orchestrator does not depend on
   `AnthropicProvider` directly — it codes against two traits defined in the
   `orchestrator` crate (not the frozen `traits/` crate):
   - `OrchestratorApiClient::messages_create(model, system, msgs) -> MessageResponse`
   - `StreamingApiClient::stream(model, system, msgs, tools) -> BoxStream<StreamEvent>`

   `AnthropicProviderAdapter` / `AnthropicProviderStreamingAdapter` are the only
   production impls today; `MockApiClient` is the test impl. The orchestrator holds
   `Arc<dyn OrchestratorApiClient>`.

2. **One canonical format throughout.** The whole engine, TUI, session JSONL, and
   cost layer speak Anthropic-shaped types — `api_client::types::{MessageResponse,
   StreamEvent, ContentBlockApi, ContentDelta, UsageApi}` and
   `protocol::{ConversationMessage, ContentBlock}`. `protocol/src/messages.rs` already
   documents the intent: *"Mirrors Anthropic's content-block model but is API-neutral
   — provider adapters map their native shapes to these."* `api-client/src/types.rs`
   similarly declares the DTOs *"provider-neutral where possible; Anthropic-specific
   fields flagged."*

3. **The cost layer is pre-wired.** `cost::ProviderId` already enumerates
   `Anthropic / OpenAI / GoogleGemini / OpenAICompatible{name} / Custom{name}` (only
   Anthropic has price tables so far).

**Net:** a new provider is a *translation adapter* behind the existing orchestrator
traits — native wire ⇄ the canonical Anthropic-shaped format. The rest of the engine
stays untouched, so **claude-code behavioral parity is preserved by construction**,
and the frozen `traits/` crate is not modified.

## §1 · Goals & non-goals

### Goals (v1)

- An **integrated, in-engine** provider abstraction so LingXi can use any supported
  model as its main-loop backend.
- **Anthropic stays the default**; existing model strings and behavior are unchanged.
- **OpenAI-compatible** provider (covers OpenAI, OpenRouter, Together, Groq, DeepSeek,
  Ollama, vLLM, LM Studio, … via `base_url`; **Azure OpenAI** via a small
  URL-template / `api-version` profile variant — see §5.1) and **native Google
  Gemini**.
- **Native function-calling parity:** translate the canonical `tool_use`/`tool_result`
  round-trip into each provider's native format. The agentic coding loop works on
  every supported provider that has native tools.
- **`provider/model` selection** + named provider **profiles** in settings for custom
  endpoints.
- **Streaming and non-streaming** paths for every provider.

### Non-goals (v1) — explicit

- Router features: provider fallback chains, multi-key load-balancing, per-provider
  budget caps, model-alias routing.
- A standalone proxy server.
- Prompted/JSON tool-use shim for models lacking native function-calling — such models
  are simply excluded from the agentic loop.
- Signed/federated auth: Azure AD tokens, GCP service-account (Vertex), AWS SigV4
  (Bedrock). API-key auth only in v1.
- Synthesizing Anthropic-only features on other providers (prompt caching,
  thinking-in-history, server tools, citations, connector text).
- Embeddings / image generation / audio endpoints.

### Assumption flagged for review

- **Vision / image input:** v1 ships **text + tools only**. Image content-block
  translation (OpenAI `image_url` parts, Gemini `inlineData`) is a **stretch** inside
  P3/P4. Flip this at review if image input must be in the v1 baseline.

## §2 · Locked decisions (brainstorm outcomes)

| # | Decision | Choice |
|---|---|---|
| Q1 | Scope shape | Integrated layer (no router/proxy); Anthropic default; parity untouched; normalize-to-canonical; v1 = OpenAI-compatible + Gemini |
| Q2 | Tool-calling fidelity | Native function-calling, full parity; non-native models excluded from the loop |
| Q3 | Selection UX | `provider/model` prefix + named profiles in settings |
| Arch | Implementation architecture | **B** — `LlmProvider` trait + pure `WireCodec`/`SseDecoder` + one generic bridge to the existing orchestrator traits |

## §3 · Architecture

### §3.1 · Crate & placement

New engine-tier crate **`providers`** (flat in `lingxi-code/`, no prefix, per
convention). Dependencies:

- `protocol` — canonical message types.
- `traits` — `HttpTransport` (read-only; **not modified**).
- `api-client` — reuse `with_retry`, SSE plumbing, `betas`, `ApiError`, telemetry
  helpers, and `AnthropicProvider` itself.
- `cost` — `ProviderId` / pricing.
- `telemetry`, `serde`, `serde_json`, `async-trait`, `futures`, `tokio`.

The orchestrator's `OrchestratorApiClient` / `StreamingApiClient` traits stay where
they are. A generic bridge `ProviderApiAdapter` (sibling to `AnthropicProviderAdapter`
in the `orchestrator` crate) wraps `Arc<dyn providers::LlmProvider>` and implements
both. `orchestrator` gains a dependency on `providers`.

**check-deps §8.1:** all engine-tier; `orchestrator → providers` and
`composition-root → providers` are allowed engine↔engine edges.

### §3.2 · Anthropic becomes one `LlmProvider`

To keep routing uniform with **zero parity risk**, Anthropic is itself an
`LlmProvider` impl that **delegates to the existing `AnthropicProvider` verbatim** (no
re-implemented request building). Byte-parity is automatic and remains covered by the
existing `parity_messages_create` / `parity_betas` byte-lock fixtures.

> **Conservative fallback (R1):** if the parity fixtures prove fussy under the new
> path, leave `AnthropicProviderAdapter` on its dedicated path and route only
> non-Anthropic providers through `ProviderApiAdapter`. Two code paths, but Anthropic
> stays exactly as today. The implementation plan starts with the uniform path and
> falls back only if a byte-lock test fails.

### §3.3 · The `LlmProvider` trait + pure `WireCodec` + bridge

```text
CanonicalRequest {
    model: String,                       // provider-local model id (prefix already stripped)
    system: Option<String>,
    messages: Vec<protocol::ConversationMessage>,
    tools: Vec<serde_json::Value>,       // canonical (Anthropic) tool schema
    max_tokens: u32,
    temperature: Option<f32>,
    stream: bool,
}

#[async_trait]
trait LlmProvider: Send + Sync {
    fn id(&self) -> cost::ProviderId;
    fn capabilities(&self) -> &Capabilities;
    async fn complete(&self, req: CanonicalRequest)
        -> Result<api_client::types::MessageResponse, api_client::ApiError>;
    async fn stream(&self, req: CanonicalRequest)
        -> Result<BoxStream<'static, Result<api_client::types::StreamEvent, ApiError>>, ApiError>;
}
```

The translation core is a **pure** `WireCodec` (no I/O — unit/snapshot testable):

```text
trait WireCodec: Send + Sync {
    fn encode_request(&self, req: &CanonicalRequest, auth: &Auth) -> Result<HttpRequest, CodecError>;
    fn decode_response(&self, status: u16, body: &str) -> Result<MessageResponse, ApiError>;
    fn new_stream_decoder(&self) -> Box<dyn SseDecoder>;
}

trait SseDecoder: Send {                 // tiny per-stream state machine
    fn push(&mut self, raw_data: &str) -> Vec<StreamEvent>;   // one SSE `data:` payload in, 0..n canonical events out
    fn finish(&mut self) -> Vec<StreamEvent>;                 // flush trailing state (e.g. final tool-call block)
}
```

A `GenericClient<C: WireCodec>` owns `base_url` + `Auth` + `Arc<dyn HttpTransport>` and
implements `LlmProvider`: `encode_request → transport.request/stream_sse →
decode_response / decoder.push`, reusing `api-client`'s retry + 429 + telemetry. The
Anthropic impl bypasses `GenericClient` and delegates to `AnthropicProvider` directly
(see §3.2).

The bridge `ProviderApiAdapter` in the `orchestrator` crate builds a
`CanonicalRequest` from the `(model, system, msgs, tools)` arguments and calls
`complete` / `stream`.

**Per-provider work = one `WireCodec` + one `SseDecoder`, both pure and
fixture-testable.**

## §4 · Capabilities & degradation ("omit, never invent")

This reuses the M9 §2.4 rule: where a canonical field has no provider equivalent, the
codec omits it — it never fabricates engine state.

```text
Capabilities {
    native_tools: bool,
    streaming: bool,
    vision: bool,                 // false in v1 unless image stretch lands
    prompt_cache: bool,
    reasoning: ReasoningSupport,  // None | DecodeOnly | …
    parallel_tool_calls: bool,
    max_output_tokens: Option<u32>,
    system_style: SystemStyle,    // TopLevel (Anthropic/Gemini) | RoleMessage (OpenAI)
}
```

- **Encode-drop** Anthropic-only history fields with no equivalent: `Thinking` blocks,
  cache markers, signatures — silently omitted.
- **Decode-map** provider reasoning *into* the canonical `Thinking` block where the
  provider emits it (OpenAI o-series reasoning, Gemini thinking, DeepSeek-R1
  `reasoning_content`) — otherwise nothing.
- **Usage** maps to `UsageApi`; OpenAI `prompt_tokens_details.cached_tokens` →
  `cache_read`; absent cache counters → `0`.
- A model with `native_tools = false` selected while tools are present → **fail fast at
  selection** with a clear error (`ApiError::UnsupportedModel`-style). No prompted
  fallback in v1.
- Non-Anthropic codecs **never synthesize** server-tool / connector / citation /
  advisor blocks.

## §5 · Translation specifics

### §5.1 · OpenAI codec — `POST {base_url}/v1/chat/completions`

Covers all OpenAI-compatible endpoints via `base_url`. Auth: `Authorization: Bearer`.

- **Messages / roles:** canonical `System` → a `system` (or `developer`) role message;
  `User`/`Assistant` text → `content`. An Assistant message carrying `ToolUse` blocks
  → an assistant message with
  `tool_calls: [{ id, type:"function", function:{ name, arguments: <stringified input> } }]`.
  Canonical `ToolResult` → a `tool` role message `{ tool_call_id, content }`. The codec
  groups the assistant-`tool_calls` message immediately before its `tool` replies, as
  OpenAI requires.
- **Tool schema:** Anthropic `{ name, description, input_schema }` →
  `{ type:"function", function:{ name, description, parameters } }` (`input_schema` →
  `parameters`).
- **`finish_reason` map:** `tool_calls → tool_use`, `stop → end_turn`,
  `length → max_tokens`, others → passthrough string.
- **Streaming:** OpenAI streams `choices[].delta`; partial
  `delta.tool_calls[].function.arguments` fragments are keyed by `index`. The
  `SseDecoder` accumulates per-index tool-call arguments and synthesizes canonical
  `ContentBlockStart(ToolUse)` + `InputJsonDelta` + `ContentBlockStop`; text deltas →
  `TextDelta`; the terminal `[DONE]` sentinel → `MessageStop`. Usage (when
  `stream_options.include_usage`) → `MessageDelta.usage`.
- **Azure OpenAI variant:** identical request/response body, but a different URL
  template (`{base}/openai/deployments/{deployment}/chat/completions?api-version=…`)
  and an `api-key` header instead of `Authorization: Bearer`. Handled by optional
  `urlTemplate` + `apiVersion` fields on the profile and the `auth_headers()` seam —
  *not* pure `base_url`. Sequenced as a small add-on within P3.

### §5.2 · Gemini codec — `…/v1beta/models/{model}:generateContent` (+ `:streamGenerateContent?alt=sse`)

Auth: `x-goog-api-key`. `base_url` overridable (Vertex base lands with signed auth
later).

- **Roles:** `contents: [{ role:"user"|"model", parts:[…] }]` (Gemini has no
  "assistant" — map Assistant → `"model"`). `system` → top-level `systemInstruction`.
- **Tools:** canonical tools → `tools:[{ functionDeclarations:[{ name, description,
  parameters }] }]`. `ToolUse` → a `model` part `{ functionCall:{ name, args } }`;
  `ToolResult` → a `user` part `{ functionResponse:{ name, response } }`.
- **Response:** `candidates[0].content.parts` → text + `functionCall` → `ToolUse`;
  `finishReason` mapped; `usageMetadata` → `UsageApi`.
- **Streaming:** SSE chunks each carry partial `candidates[].content.parts` → emit
  `TextDelta` / tool-use blocks; the final chunk's `finishReason` → `MessageDelta` +
  `MessageStop`.
- **Schema caveat (R3):** Gemini accepts a restricted JSON-schema subset. The tool
  schema normalizer strips/loweres unsupported keywords; unsupported features are
  documented.

## §6 · Model selection — `ModelSpec` + profiles + registry

Settings gains one **object-merge** (deep-merge) field, `providers`. It is a LingXi
extension — claude-code has no such key. The field is `Option`, so claude-code-shaped
settings files are unaffected. (`SettingsJson` uses `serde(rename_all="camelCase",
deny_unknown_fields)`; the new field is added to the struct so it is recognized. It is
typed `Option<BTreeMap<String, Value>>` (object / deep-merge, mirroring the existing
`sandbox` / `hooks` fields); the profile internals are parsed leniently by the
`providers` registry, so per-profile keys stay flexible and `deny_unknown_fields` on
`SettingsJson` is satisfied.)

```jsonc
{
  "model": "openai/gpt-4o",            // existing scalar field; now accepts provider/model
  "providers": {                        // NEW deep-merge field
    "openai": { "type": "openai", "apiKeyEnv": "OPENAI_API_KEY" },
    "groq":   { "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY" },
    "ollama": { "type": "openai", "baseUrl": "http://localhost:11434/v1", "apiKeyEnv": null },
    "gemini": { "type": "gemini", "apiKeyEnv": "GEMINI_API_KEY" }
  }
}
```

- `ModelSpec::parse("groq/llama-3.3-70b")` → `{ profile: "groq", model: "llama-3.3-70b" }`.
- **Back-compat:** a string with no `/` (or a `claude-*` string) resolves to the
  Anthropic default profile, so every existing model string is unchanged.
- **Built-in profiles** for `openai` / `gemini` / `anthropic` exist without any settings
  block (only the API-key env var is required), so `openai/gpt-4o` works out of the
  box. Settings profiles add custom / OpenAI-compatible endpoints, and can override the
  built-ins by name.
- `ProviderRegistry::resolve(profile)` builds and caches an `Arc<dyn LlmProvider>` from
  the profile (type → codec; `base_url`; auth key from the named env var;
  `HttpTransport`).

## §7 · Composition wiring

- The bridge **`ProviderApiAdapter` resolves the provider per call from the `model`
  argument** (which already flows into `messages_create` / `stream`):
  `ModelSpec::parse(model) → registry.resolve(profile) → provider.complete(model_id, …)`.
  Provider switching is therefore automatic from the model string:
  `switch_model` stays trivial (sets the session string; the next turn routes
  correctly) and gains optional validation (reject an unconfigured profile up front).
  `list_available_models` enumerates the configured profiles.
- `apps/cli/src/init.rs`: build
  `ProviderRegistry::from_settings(effective.providers, env, http)`; set
  `api_client = Arc::new(ProviderApiAdapter::new(registry))`.
- **`tool_provider` (WebSearch / sidequery) stays Anthropic in v1** — server-side web
  search is an Anthropic feature and is not meaningful to route elsewhere.

## §8 · Cost / pricing

`cost::ProviderId` already has the variants. This adds:

- OpenAI + Gemini price tables (per-model input / output / cache rates).
- `cost_wiring::model_ref_from_string` learns to read the provider prefix → the correct
  `ProviderId` (`openai/…` → `OpenAI`, `gemini/…` → `GoogleGemini`, a custom profile →
  `OpenAICompatible{name}` / `Custom{name}`, bare/`claude-*` → `Anthropic`).
- An unknown model is recorded with cost `0` under the correct provider — never an
  invented rate.
- Per-provider cache-token mapping (OpenAI `cached_tokens` → `cache_read`).

## §9 · Auth (v1 = keys; signed auth deferred)

Per-profile API key resolved from the env var named by `apiKeyEnv`. OpenAI uses
`Authorization: Bearer`, Gemini uses `x-goog-api-key`. The client's auth is a single
`auth_headers()` seam so that **Azure AD / GCP service-account (Vertex) / AWS SigV4
(Bedrock)** can drop in later without touching codecs. Those are explicit v1 non-goals.

A missing/empty key follows the existing Anthropic precedent: construction succeeds
(so slash-command dispatch works offline); the first turn fails with a clear
auth error.

## §10 · Error mapping

Provider error bodies map onto the existing `ApiError` taxonomy
(`Server{status,body}` / `RateLimited` / `Unauthorized` / `MalformedStream` / …). The
`api-client` `with_retry` + 429 handling + the `tengu_api_*` telemetry vocabulary are
reused unchanged. Each codec's `decode_response` maps that provider's error JSON shape
into the taxonomy.

## §11 · Testing strategy (pure-function-first)

- **Pure codec unit tests:** canonical request → expected native JSON; native response
  JSON → canonical `MessageResponse`; raw SSE `data:` lines → canonical `StreamEvent`
  sequence (the hardest case — OpenAI tool-call reassembly across `index`-keyed
  fragments).
- **Parity fixtures** in `test-harness` (mirroring `parity_tui_multiagent.json`):
  recorded real OpenAI / Gemini request+response+SSE samples, round-tripped through the
  codecs.
- **Mock-transport integration** per provider: canned HTTP responses through
  `GenericClient` → assert `MessageResponse` / stream.
- **Tool-calling round-trip** test per provider: encode tools → decode `tool_calls`/
  `functionCall` → `ToolUse` → run → `ToolResult` → re-encode.
- **Back-compat gate:** every existing Anthropic byte-lock / parity test stays green —
  proof the Anthropic-through-`LlmProvider` path did not perturb the wire.
- Standard gates per sub-plan: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`,
  `check-deps`. Run from `lingxi-code/` (Rust 1.82.0).

## §12 · Phased plan (sub-plans, M9-style)

Each sub-plan is pure-function-first, fixture-tested, individually gated, and gets a
local annotated tag. No remote push.

1. **P1 — Core.** `providers` crate; `LlmProvider` / `WireCodec` / `SseDecoder` /
   `CanonicalRequest` / `Capabilities`; `GenericClient`; **Anthropic-as-`LlmProvider`**
   (parity-locked, §3.2); `ProviderApiAdapter` bridge in `orchestrator`; orchestrator
   routes through it. All existing tests green (the back-compat gate).
2. **P2 — Selection / wiring.** `ModelSpec` parser; `ProviderRegistry`; settings
   `providers` schema (+ env-parser + merger + JsonSchema); composition wiring in
   `init.rs`; `switch_model` / `list_available_models`; back-compat tests.
3. **P3 — OpenAI codec** (+ `SseDecoder`, tool-calling, streaming accumulation) +
   fixtures. Covers all OpenAI-compatible endpoints. (Image-input stretch decided
   here.)
4. **P4 — Gemini codec** (+ `SseDecoder`, schema normalizer) + fixtures.
5. **P5 — Cost / pricing** tables for OpenAI + Gemini; prefix-aware
   `model_ref_from_string`.
6. **P6 — Capabilities / degradation enforcement, error-mapping polish, `/model` UX
   (provider-aware list), docs (README / CHANGELOG / PLATFORMS), release bump.**

## §13 · Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| R1 | Anthropic-through-`LlmProvider` perturbs byte parity | Delegate to existing `AnthropicProvider` verbatim; existing parity fixtures are the gate; conservative dual-path fallback (§3.2) |
| R2 | OpenAI streaming tool-call reassembly (index-keyed fragments) is subtly wrong | Dedicated `SseDecoder` state machine + raw-SSE fixture tests covering multi-tool, interleaved, and partial-arg cases |
| R3 | Tool-schema dialect drift (Anthropic `input_schema` vs OpenAI/Gemini `parameters`; Gemini's restricted subset) | Schema normalizer + per-provider fixtures; documented unsupported keywords |
| R4 | Silent capability mismatch (a selected model can't actually do tools) | `Capabilities` + fail-fast at selection with a clear message |
| R5 | `deny_unknown_fields` + claude-code settings parity | `providers` is optional and documented as a LingXi extension; absent from claude-code-shaped files |

## §14 · Open questions / future work

- **Image / vision input** (v1 assumption: deferred — see §1).
- **Router phase** (fallbacks, load-balancing, budgets, model aliases) — slots on top
  of `ProviderRegistry` as a later milestone.
- **Signed auth** for Vertex / Bedrock / Azure AD — via the `auth_headers()` seam.
- **Provider-aware tools** (web search / sidequery on non-Anthropic providers) — out of
  v1.
- **`count_tokens`** for non-Anthropic providers (most expose a tokenizer or an
  estimate endpoint) — not required for the loop; deferred.
