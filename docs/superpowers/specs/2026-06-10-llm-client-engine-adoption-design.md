# Engine adoption of llm-client (full replacement of api-client) — design

Date: 2026-06-10
Status: approved (user), revision 2.5 — 3a + 3b + 3c DONE; llm-client adoption COMPLETE end-to-end.

**Revision history:**
- 2.2 — no intermediate policy crate; supersedes api-client entirely with no backwards compatibility.
- 2.3 — Plan 3a complete: live path swapped onto llm-client; 5 TS parity fixes merged to main.
- 2.4 — Plan 3b complete: api-client + providers crates deleted; ~16 dependents re-homed.
  3c remaining: modelProviders settings wiring — `DesktopConfig.provider_profiles` / `routing`
  and `SettingsJson.providers` / `routing` are loaded into `llm_client::ClientConfig` but the
  engine settings schema fields are not yet consumed (wired in as `None` / passthrough today);
  also pending: `PricingCatalog` population from the `cost` crate price table, and streaming
  rate-limit header metadata traits (`stream_sse` response-header surface).
- 2.5 — Plan 3c complete: providers/routing settings wired via
  `platform_common::apply_settings_providers` (`routing.aliases` live; `routing.fallback`/
  `routing.retry` keys parsed but inert, documented in the settings schema);
  `LlmResponse.cost` live via a cost-crate-derived `CostEstimator` in `ProviderApiAdapter`;
  streaming responses carry real status/headers via the additive
  `traits::HttpTransport::stream_sse_with_meta`. llm-client adoption COMPLETE end-to-end.
  Remaining future work: wire `routing.fallback`/`routing.retry` to the fallback/retry
  drivers; `AwsSigV4`/`GcpToken`/`AzureToken` auth signing; streaming error-path headers
  (reqwest error arm carries none); per-profile pricing overrides from settings.

## Goal

Replace the engine's existing model-API framework (`api-client`, Anthropic-only,
~6.7k lines) with direct use of the provider-neutral `llm-client` crate: wire
encoding/decoding, auth, error taxonomy, usage/cost, transport seam, and
multi-provider routing. `api-client` is deleted. Engine-facing types switch to
`llm_client::{LlmRequest, LlmResponse, LlmEvent, LlmError}` everywhere — no
compatibility shims and **no new intermediate crate**: each piece of the old
client lands at its natural owner.

User decisions captured:
1. `api-client` is **deleted**. Its claude-code-parity policy logic is
   **distributed to natural owners** (orchestrator, platforms/common,
   anthropic-oauth, agent) rather than collected in a new crate.
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
           (+ CredentialProvider from anthropic-oauth when OAuth configured)
    │
agent  ── SubagentApiClient trait re-typed to llm_client types;
          ConversationMessage → Message conversion lives here
    │
orchestrator wire adapter (production SubagentApiClient impl)
    retry driver (claude-code cadence) · beta injection · opus fallback ·
    rate-limit tracking · overflow copy · cost/telemetry emission ·
    count_tokens facade — each its own module under orchestrator
    │
llm-client  (codecs, auth, error taxonomy, RetryPolicy classification,
             CostEstimator, execute / execute_stream orchestration)
    │  llm_client::Transport
platforms/common  LlmTransportBridge<T: traits::HttpTransport>
    (one generic adapter shared by ReqwestHttp and native mobile impls)

tui rate-limit panel ── reads the rate-limit status type from the shared
    protocol crate; the tracker in orchestrator populates it
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
- No llm-client change needed for beta headers: callers inject
  `anthropic-beta` into `PreparedLlmCall.provider_request.headers` after
  `prepare()` (headers are public by design).
- `LlmError::Overloaded` distinguishes Anthropic 529/overloaded_error (retryable; drives MAX_529_RETRIES and opus fallback); `DefaultLlmClient::prepare_count_tokens` provides the authenticated count_tokens path.

### 2. Adapters at their endpoints

- **platforms/common**: `LlmTransportBridge<T: traits::HttpTransport>`
  implements `llm_client::Transport`. `execute` maps `ProviderRequest` →
  `protocol::HttpRequest`; `open_stream` uses `stream_sse`
  (`SseEvent.data` → `RawStreamFrame`);
  `stream_sse` is a required `HttpTransport` method, so no raw-bytes
  fallback is built (the `SseFrameSplitter` remains available for future
  byte-level hosts). One generic adapter serves ReqwestHttp
  and the native mobile transports. platforms/common gains an `llm-client`
  dependency (dependency-light by construction).
- **anthropic-oauth**: implements `llm_client::CredentialProvider` directly —
  token refresh inside `load()`, attached by hosts via
  `DefaultLlmClient::with_credential_provider`. The api-client
  `OAuthRefreshHook` indirection is deleted with the crate.
  `CredentialProvider::load` is async (BoxFuture) for exactly this reason.

### 3. Orchestrator wire adapter (policy home)

The production `SubagentApiClient` implementation already lives in the
orchestrator; the old api-client policy modules port to focused modules
beside it (`orchestrator/src/model/…`), re-typed to llm-client:

- `retry.rs`: the retry **driver** for all providers — classification from
  `llm_client::RetryPolicy`, claude-code cadence (3 attempts 500ms / 1s / 2s
  ± 20% jitter, `MAX_529_RETRIES`, server `retry-after`/`retry-after-ms`
  takes precedence). One implementation, no per-provider forks. Streaming:
  only connect-phase failures retry; `StreamInterrupted` (post-first-event)
  does not.
- `rate_limit.rs`: tracker fed by response headers and
  `LlmError::RateLimited`; reset-time formatting (chrono/iana-time-zone)
  ports unchanged. The TUI consumes a pre-formatted message string from the orchestrator (verified: no api_client::rate_limit imports in tui), so all rate-limit types stay orchestrator-local.
- `betas.rs`: unchanged join/dedupe logic; applied post-`prepare()` on
  Anthropic routes.
- `fallback.rs` (from `opus.rs`): opus→sonnet fallback on Anthropic models
  only, driven by llm-client error classes.
- `overflow.rs` / `prompt_too_long.rs`: detection largely replaced by
  `LlmError::ContextOverflow`; token-gap parsing and user-facing copy port
  as-is to where ApiError copy is rendered today.
- `count_tokens`: facade using `AnthropicMessagesCodec`'s inherent methods +
  the transport on Anthropic routes; documented character-based approximation
  elsewhere (used by compaction). (facade in `orchestrator::model::count_tokens`; approximation = chars/4, min 1).
- cost/telemetry: emission points (`emit_started/succeeded/failed/...`) port
  unchanged; cost numbers come from `llm_client::CostEstimator` with a
  `PricingCatalog` populated from the existing `cost` crate price table.

### 4. Engine seam changes (breaking, by design)

- `agent::SubagentApiClient`: `messages_create` returns
  `llm_client::LlmResponse`; `messages_create_stream` returns
  `BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>`.
  The default stream-from-non-stream synthesis stays, emitting `LlmEvent`.
- `agent::convert`: `protocol::ConversationMessage` → `llm_client::Message`
  (and tool wire JSON → `ToolDeclaration`); agent gains the llm-client
  dependency its seam types already imply.
- `agent::accumulator`: accumulates `LlmEvent` into `LlmResponse`
  (anthropic-shaped event grammar unchanged — mechanical re-type);
  `response_to_stream_events` emits `LlmEvent`.
- `agent::runner`: stop-reason / tool-use branching reads
  `LlmResponse.stop_reason` and `ContentBlock::ToolCall/ToolResult`.
- apps hosts: construct `ClientConfig` (see §5), `DefaultLlmClient`,
  `LlmTransportBridge`, and hand them to the orchestrator wire adapter;
  delete `AnthropicProvider` wiring.
- `tui`: rate-limit components import the status type from the shared
  protocol crate.

### 5. Configuration (multi-provider)

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
- Credentials: `env` and OAuth (`host_managed` via anthropic-oauth's
  CredentialProvider); pricing per profile from the cost-crate catalog,
  falling back to `MarkUnestimated`.

### 6. Deletion

`api-client/` removed from the tree and from both workspace member lists;
all `api_client::` imports gone. `agent`, `orchestrator`, `tui`, `apps/*`
compile against `llm-client` (+ orchestrator policy modules) only.

## Error handling

Single taxonomy end to end: `LlmError` (already status-aware per provider).
The retry driver consumes `RetryPolicy::classify_error`; user-facing copy
(rate-limit reset times, prompt-too-long token gap, 529 overloaded) keeps
claude-code wording on Anthropic routes; generic wording elsewhere via the
same templates.

## Testing

- llm-client additions: red-green codec tests (reasoning, cache_control,
  system blocks, count_tokens) in the existing per-provider test files.
- platforms/common: adapter tests with a fake `traits::HttpTransport`
  (request mapping, SSE event mapping, raw-bytes + splitter fallback, error
  mapping).
- orchestrator: ported api-client tests with identical assertions where
  behavior is parity-bound (retry cadence/jitter bounds, rate-limit header
  parsing + reset formatting, beta join rules, fallback triggers,
  prompt-too-long copy).
- Engine: existing agent/runner/orchestrator mocks re-typed to `LlmEvent`/
  `LlmResponse`; orchestrator smoke tests stay green.
- Full-workspace `cargo test` + clippy at every phase boundary.

## Implementation phases

1. **P1 — llm-client wire additions**: reasoning config, cache_control +
   system blocks, count_tokens codec support.
2. **P2 — endpoint adapters**: `LlmTransportBridge` in platforms/common;
   `CredentialProvider` impl in anthropic-oauth.
3. **P3 — orchestrator policy ports**: retry driver, rate-limit (+ status
   type to the shared protocol crate), betas, fallback, overflow copy,
   cost/telemetry emission, count_tokens facade (+ ported parity tests).
4. **P4 — engine seam swap**: agent types + convert + accumulator + runner,
   orchestrator wire adapter on llm-client, TUI imports, apps hosts
   construction.
5. **P5 — deletion + multi-provider config**: remove api-client, add
   `modelProviders` settings parsing, end-to-end verification.

Each phase lands with the full workspace compiling and green.

## Risks

- **Active main churn**: engine crates are being modified by parallel parity
  work; implement on a fresh-main worktree branch and rebase before merge.
- **Parity drift**: retry/rate-limit copy is asserted byte-for-byte in ported
  tests; any intentional divergence must be called out in review.
- **Orchestrator growth**: the policy modules concentrate there by design;
  keep them as focused single-purpose modules. If a second retry-loop
  consumer appears later, extract then.
- **Streaming semantics**: `LlmEventStream` upgrades post-first-event
  transport failures to `StreamInterrupted` (non-retryable) — matches the
  old client's no-replay-after-events behavior; the retry driver only
  retries connect-phase failures.
