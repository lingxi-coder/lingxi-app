# Engine adoption of llm-client (full replacement of api-client) — design

Date: 2026-06-10
Status: approved (user); supersedes api-client entirely — no backwards compatibility.

## Goal

Replace the engine's existing model-API framework (`api-client`, Anthropic-only,
~6.7k lines) with the provider-neutral `llm-client` crate, end to end: wire
encoding/decoding, auth, error taxonomy, usage/cost, transport seam, and
multi-provider routing. `api-client` is deleted. Engine-facing types switch to
`llm_client::{LlmRequest, LlmResponse, LlmEvent, LlmError}` everywhere — no
compatibility shims.

User decisions captured:
1. `api-client` is **deleted**; its claude-code-parity policy logic moves to a
   **new crate `model-policy`**.
2. **Multi-provider routing is wired up in this change** (Anthropic, OpenAI
   Chat, Gemini via llm-client codecs), configured through settings.
3. **Uniform generic runtime policy** across providers: one retry driver, one
   error-copy template set. Anthropic-specific surfaces (rate-limit panel
   data, opus fallback, beta headers, count_tokens endpoint) activate only on
   Anthropic routes. Anthropic behavior stays byte-for-byte claude-code parity.

## Architecture

```
apps/engine-desktop, apps/engine-mobile (hosts)
    build: ClientConfig (settings + env) → DefaultLlmClient
           + HttpTransportAdapter(ReqwestHttp / native)
    │
agent / orchestrator ──→ model-policy (NEW; replaces api-client)
    retry driving · rate-limit tracking · beta injection · OAuth credential
    bridge · opus fallback · overflow copy · cost/telemetry emission ·
    ConversationMessage→Message conversion · count_tokens facade
    │
llm-client  (codecs, auth, error taxonomy, RetryPolicy classification,
             CostEstimator, execute / execute_stream orchestration)
    │  llm_client::Transport
HttpTransportAdapter (lives in model-policy) over traits::HttpTransport
    (platforms/common ReqwestHttp; native impls on mobile)

tui rate-limit panel ──reads── model-policy rate-limit status
```

Dependency rule preserved (D17): nothing above `platforms/` imports a concrete
HTTP client; `llm-client` stays free of repo-internal dependencies.

## Component changes

### 1. llm-client wire additions (prerequisites)

- `LlmRequest.reasoning: Option<ReasoningConfig>` with
  `ReasoningConfig { budget_tokens: u32 }`. Anthropic encodes
  `thinking: {type: "enabled", budget_tokens}`; Gemini encodes
  `generationConfig.thinkingConfig.thinkingBudget`; OpenAI Chat rejects
  explicitly (staged until the Responses API codec exists). Capability
  preflight: requires `capabilities.reasoning`.
- Prompt caching: `ContentBlock` gains optional
  `cache_control: Option<CacheControl>` (`CacheControl::Ephemeral`,
  serde-skipped when absent) and `LlmRequest.system` becomes
  `Vec<SystemBlock>` (`{ text, cache_control }`) — breaking change, no
  compatibility kept. Anthropic encodes `cache_control` and the system block
  array; OpenAI/Gemini ignore cache_control (their caching is implicit) and
  join system text.
- `count_tokens`: inherent `encode_count_tokens_request` /
  `decode_count_tokens_response` on `AnthropicMessagesCodec`
  (`POST {base}/v1/messages/count_tokens`); the `WireCodec` trait is
  unchanged.
- No llm-client change needed for beta headers: `model-policy` injects
  `anthropic-beta` into `PreparedLlmCall.provider_request.headers` after
  `prepare()`.

### 2. model-policy (new crate)

Ports `api-client` modules onto llm-client types; claude-code parity
assertions move with their tests.

- `retry.rs`: the retry **driver** (loop) for all providers — classification
  from `llm_client::RetryPolicy`, claude-code cadence (3 attempts 500ms / 1s /
  2s ± 20% jitter, `MAX_529_RETRIES`, server `retry-after`/`retry-after-ms`
  takes precedence). One implementation, no per-provider forks.
- `rate_limit.rs`: tracker fed by response headers and
  `LlmError::RateLimited`; reset-time formatting (chrono/iana-time-zone)
  unchanged. Source of truth for the TUI panel; populated only when
  Anthropic-style headers are present.
- `betas.rs`: unchanged logic; applied post-`prepare()` on Anthropic routes.
- `oauth.rs`: `OAuthRefreshHook` becomes an implementation of
  `llm_client::CredentialProvider` (token refresh inside `load()`), attached
  via `DefaultLlmClient::with_credential_provider`. The frozen hook trait
  surface from api-client is preserved for the existing oauth implementation
  (`anthropic-oauth` crate) to plug into.
- `fallback.rs` (from `opus.rs`): opus→sonnet fallback on Anthropic models
  only, driven by llm-client error classes.
- `overflow.rs` / `prompt_too_long.rs`: detection largely replaced by
  `LlmError::ContextOverflow`; the token-gap parsing and user-facing copy
  port as-is.
- `convert.rs`: `protocol::ConversationMessage` → `llm_client::Message`
  (and tool wire JSON → `ToolDeclaration`).
- `transport.rs`: `HttpTransportAdapter` implementing `llm_client::Transport`
  over `traits::HttpTransport` — `execute` maps `ProviderRequest` →
  `protocol::HttpRequest`; `open_stream` prefers `stream_sse`
  (`SseEvent.data` → `RawStreamFrame`) and falls back to `stream_raw_bytes` +
  `llm_client::SseFrameSplitter`.
- `client.rs`: `ModelClient` — the engine-facing handle composing
  `DefaultLlmClient` + transport + retry driver + rate-limit tracker + betas +
  fallback + cost/telemetry emission. API:
  `messages_create(...) -> Result<LlmResponse, LlmError>`,
  `messages_create_stream(...) -> Result<EventStream, LlmError>` (wrapping
  `LlmEventStream` with retry-on-connect and telemetry),
  `count_tokens(...)` (real endpoint on Anthropic; documented character-based
  approximation elsewhere).
- Telemetry/cost: emission points (`emit_started/succeeded/failed/...`) port
  unchanged; cost numbers come from `llm_client::CostEstimator` with a
  `PricingCatalog` populated from the existing `cost` crate price table.

### 3. Engine seam changes (breaking, by design)

- `agent::SubagentApiClient`: `messages_create` returns
  `llm_client::LlmResponse`; `messages_create_stream` returns
  `BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>`.
  The default stream-from-non-stream synthesis stays, emitting `LlmEvent`.
- `agent::accumulator`: accumulates `LlmEvent` into `LlmResponse`
  (anthropic-shaped event grammar is unchanged, so this is mechanical);
  `response_to_stream_events` emits `LlmEvent`.
- `agent::runner`: stop-reason / tool-use branching reads
  `LlmResponse.stop_reason` and `ContentBlock::ToolCall/ToolResult`.
- orchestrator Wire adapter: rebuilt on `model_policy::ModelClient`.
- `tui`: rate-limit components read `model_policy::rate_limit` types.
- apps hosts: construct `ClientConfig` (see §4), `DefaultLlmClient`,
  `HttpTransportAdapter`, `ModelClient`; delete `AnthropicProvider` wiring.

### 4. Configuration (multi-provider)

- New optional settings section `modelProviders` (lingxi extension; absent in
  claude-code): an array deserializing into `llm_client::ProviderProfile`
  (provider_id, profile_name, base_url, protocol, auth, credential, models
  with aliases/capabilities, pricing).
- **Default (section absent): exactly one built-in Anthropic profile** from
  `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` (or OAuth when configured) with
  the built-in claude model table — zero behavioral difference from
  claude-code.
- Declaring profiles activates them: model selection strings resolve through
  the llm-client registry (display names, aliases, ambiguity rejection).
- Credentials: `env` and OAuth (`host_managed` via the CredentialProvider
  bridge); pricing per profile from the cost-crate catalog, falling back to
  `MarkUnestimated`.

### 5. Deletion

`api-client/` removed from the tree and from both workspace member lists;
all `api_client::` imports gone. `agent`, `orchestrator`, `tui`, `apps/*`
compile against `model-policy` + `llm-client` only.

## Error handling

Single taxonomy end to end: `LlmError` (already status-aware per provider).
The retry driver consumes `RetryPolicy::classify_error`; user-facing copy
(rate-limit reset times, prompt-too-long token gap, 529 overloaded) lives in
`model-policy` and keeps claude-code wording on Anthropic routes; generic
wording elsewhere via the same templates.

## Testing

- `model-policy` unit tests: ported api-client tests with identical
  assertions where behavior is parity-bound (retry cadence/jitter bounds,
  rate-limit header parsing + reset formatting, beta join rules, fallback
  triggers, prompt-too-long copy).
- llm-client additions: red-green codec tests (reasoning, cache_control,
  system blocks, count_tokens) in the existing per-provider test files.
- Adapter tests: fake `traits::HttpTransport` driving
  `HttpTransportAdapter` (request mapping, SSE event mapping, raw-bytes +
  splitter fallback, error mapping).
- Engine: existing agent/runner/orchestrator mocks re-typed to `LlmEvent`/
  `LlmResponse`; orchestrator smoke tests stay green.
- Full-workspace `cargo test` + clippy at every phase boundary.

## Implementation phases

1. **P1 — llm-client wire additions**: reasoning config, cache_control +
   system blocks, count_tokens codec support.
2. **P2 — model-policy skeleton**: crate, transport adapter, OAuth
   credential bridge, ModelClient::execute path (no policies yet).
3. **P3 — policy ports**: retry driver, rate-limit, betas, fallback,
   overflow copy, cost/telemetry emission (+ ported parity tests).
4. **P4 — engine seam swap**: agent types, accumulator, runner,
   orchestrator wire, TUI, apps hosts construction.
5. **P5 — deletion + multi-provider config**: remove api-client, add
   `modelProviders` settings parsing, end-to-end verification.

Each phase lands with the full workspace compiling and green.

## Risks

- **Active main churn**: engine crates are being modified by parallel parity
  work; implement on a fresh-main worktree branch and rebase before merge.
- **Parity drift**: retry/rate-limit copy is asserted byte-for-byte in ported
  tests; any intentional divergence must be called out in review.
- **Streaming semantics**: `LlmEventStream` upgrades post-first-event
  transport failures to `StreamInterrupted` (non-retryable) — matches the
  old client's no-replay-after-events behavior; the retry driver only
  retries connect-phase failures.
