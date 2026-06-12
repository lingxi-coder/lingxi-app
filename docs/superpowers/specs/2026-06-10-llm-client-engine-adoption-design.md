# Engine adoption of llm-client (full replacement of api-client) — design

Date: 2026-06-10
Status: approved (user), revision 2.7 — future-work batch 2 COMPLETE.

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
- 2.6 — Future-work batch 1 COMPLETE (see §Future work — batch 1 detail below). Also fixed
  in batch 1: streaming-path CostTracker recording (was a real gap — now wired; see
  §Streaming-cost evidence below). Remaining (batch 2 candidates): Vertex/Bedrock codecs +
  AWS event-stream framing, OpenAI document parts, Gemini File API ImageUrl, fallback chains
  beyond chain[0], rate-limit TUI surface.
- 2.7 — Future-work batch 2 COMPLETE: ALL protocol families except OpenAiResponses now have
  codecs.
  - **BedrockClaude**: AWS event-stream binary framing via a hand-rolled CRC32-validated
    splitter (prelude-CRC-validated-before-buffering hostile-length defense + 8 MiB frame
    bound); each frame's `{"bytes": b64}` payload is unwrapped and fed to the inner anthropic
    decoder; SigV4 signs streaming bodies; new `"bedrock-claude"` settings type maps
    `region` → `SigningConfig` and derives the `base_url`.
  - **VertexClaude**: `rawPredict`/`streamRawPredict` SSE endpoints;
    `anthropic_version: vertex-2023-10-16`; `base_url` carries the full
    project/location prefix by convention.
  - **VertexGemini**: URL-only wrapper over `GeminiCodec` (`alt=sse` streaming).
  - Settings types `vertex-claude`/`vertex-gemini`: `apiKeyEnv` holds the Bearer-token env
    var; `GcpToken` accepts Env-loaded credentials.
  - **Media completions**: OpenAI `Document` → file parts with data-URI + `"document"`
    default filename (was REJECT before); Gemini `ImageUrl` → `file_data.file_uri` with
    `mimeType` omitted (service-inferred).
  - **Fallback chains walk ALL entries**: per-model `Vec` of fallbacks; the retry `ctl` is
    rebuilt per Fallback step with a consecutive-529 reset; `retry.rs` untouched.
  - **Rate-limit surface** stopped at `OrchestratorHandle::last_rate_limit_info`
    (frozen-protocol scope guard — a full TUI feed needs a protocol event, documented).
  - Remaining: OpenAiResponses codec (no demand), Gemini File API upload flow, TUI
    rate-limit rendering via a future protocol event.

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

## Future work — batch 1 detail (rev 2.6, merged to main)

### fw-T1 — routing.fallback/retry wiring + settings validation

`routing.fallback` is a per-model fallback chain; only chain[0] (the first
fallback target) is consumed by the driver in batch 1. `routing.retry` has
fields `maxAttempts` and `backoffMs`; env vars take precedence over settings
take precedence over compiled-in defaults (standard three-tier precedence).
`backoffMs = 0` is rejected at settings-parse time. `baseUrl` and `apiKeyEnv`
are now required fields in a `modelProviders` profile entry (previously
optional with silent defaults).

### fw-T2 — per-profile pricing overrides

A USD-per-million-tokens shape (`input`, `output`, `cacheRead`, `cacheWrite`)
may be specified per `modelProviders` profile in settings. This overrides the
built-in catalog for that profile's models. **Limitation**: the `"anthropic"`
profile name is reserved for the built-in Anthropic provider; overriding
prices for built-in Anthropic model names requires declaring a second
`anthropic`-typed profile with a distinct name and listing those models there.
Profiles that declare `pricing` but name an unknown provider are rejected at
parse time.

### fw-T3 — header observability + cleanups

- Streaming error-arm now surfaces real HTTP response headers (previously
  the reqwest error arm carried none); the adapter surfaces them via the
  `last_rate_limit_info` getter on the adapter.
- 2xx responses with a `Retry-After` or `x-ratelimit-*` header are tracked
  as soft rate-limit signals even when the status is success (2xx rate-limit
  tracking).
- Stream telemetry twins: `emit_started`/`emit_succeeded`/`emit_failed`
  events fire on the streaming path as well as the batched path (previously
  only the batched path fired them).
- Dead `ServerError` variant removed from the error taxonomy (was unused
  after the LlmError unification in 3c).

### fw-T4 — codec backlog

- **HTTP-date `Retry-After`**: llm-client now parses both the `delay-seconds`
  form and the `HTTP-date` form of `Retry-After` response headers.
- **OpenAI content-array decode**: `messages_create` responses whose `content`
  field is a JSON array (multi-part) are decoded correctly; previously only
  the string form was handled.
- **Image encode (data-URI)**: `ContentBlock::Image` with a `data:` URI source
  is encoded for OpenAI (base64-stripped, MIME type passed through). Document
  blocks are still rejected for OpenAI (no document support in Chat API).
- **Gemini inline_data image + document encode**: both image and document
  inline_data parts are encoded for Gemini; `tool_choice` is wired for Gemini
  (previously silently dropped).
- **Anthropic response_format**: kept as a rejected field (beta-only
  `output_config` available in TS `sidequery.ts:190`; not yet surfaced in the
  Rust codec — documented divergence, not a bug).

### fw-T5 — cloud auth (SigV4 + GCP + Azure) + AzureOpenAi codec

- **AwsSigV4**: implemented from scratch with AWS official test vectors.
  Hardening: query-string values are decode-then-re-encode normalized to
  RFC3986 percent-encoding; path segments are double-percent-encoded per SigV4
  spec; header values are trimall-normalized; a `Value::Null` body signs the
  4-byte literal `"null"` — matching exactly what `LlmTransportBridge` puts on
  the wire (`body_json.to_string()` unconditionally; pinned by
  `sigv4_null_body_signs_as_null_string`); clock is injectable for
  deterministic tests; trailing slashes in the URI path are preserved
  (botocore behavior).
- **GcpToken**: Bearer-token auth via a host-loaded token injected as a
  `Bearer` `Authorization` header, reusing `BearerAuthenticator` — no
  service-account or metadata-server flow is implemented; the authenticator
  reads a pre-fetched token string surfaced as `CredentialProvider`.
- **AzureToken**: Azure Entra token injected as `api-key` header (Azure
  OpenAI's expected header name for token auth).
- **AzureOpenAi codec**: deployment-URL construction (base URL + deployment
  name suffix), model field stripped from the wire request (Azure derives model
  from the deployment URL). The settings type `"azure-openai"` maps to this
  codec.

### Streaming-cost evidence + fix (batch 1, rev 2.6)

**Original verdict (pre-fix): real gap — streaming turns were NEVER billed to `CostTracker`.**

**Fixed in batch 1** (`fix(orchestrator): record streaming-turn usage in CostTracker`).

Evidence (pre-fix):
- `orchestrator/src/turn_loop.rs:381` — sole call site of
  `CostTracker::record_api_response_v2` + `api_calls_recorded.fetch_add`; this
  was the **non-streaming** (batched) path only, inside `execute_one_turn`.
- `orchestrator/src/conversation.rs:1765` — `try_run_turn_streaming` loop
  called `pump_stream` then processed `PumpedTurn`, but contained **no**
  `record_api_response_v2` call and never incremented `api_calls_recorded`.
- `orchestrator/src/streaming_loop.rs:54` — `PumpedTurn.output_tokens` carried
  only the final `MessageDelta` output-token count. The full `LlmUsage`
  (input tokens + cache read + cache write) was consumed by
  `emit_usage_if_present` at `event_router.rs:170-171` and discarded.

Fix implemented (TDD):
- `PumpedTurn` gained `usage: Option<LlmUsage>` — populated from the final
  `MessageDelta` usage (authoritative) or falls back to `MessageStart` usage
  (input tokens) when the delta carries no snapshot.
- `RouterAction::RecordStopReason` and `RecordUsage` gained `usage: Option<Usage>`
  (cloned before `emit_usage_if_present` consumes it).
- `pump_stream` captures `MessageStart.usage` as seed; sets `turn.usage`
  on each `RecordStopReason`/`RecordUsage` action via **per-field merge**
  (see below).
- `try_run_turn_streaming` in `conversation.rs` calls `record_api_response_v2`
  + `api_calls_recorded.fetch_add(1)` after every successful `pump_stream`,
  mirroring `turn_loop.rs:375-393` with `Duration::ZERO` / `retries=0` / no bus.
- The midstream-fallback path (`llm_response_to_pumped_turn`) sets
  `usage: Some(resp.usage.clone())` so the same billing block records it.
- Two new tests in `orchestrator/tests/streaming_cost_recording_test.rs`:
  `streaming_turn_records_cost_in_tracker` (17_500_000 nano-USD for 1000 input +
  500 output on claude-opus-4-6) and `streaming_turn_increments_api_calls_recorded`.
  Both tests use the real Anthropic wire shape: `message_start` carries input
  tokens; `message_delta` carries output tokens only.
- Remaining secondary gap (batch 2): `LlmResponse.cost` on the streaming path
  is still `None` (streaming response never passes through `decode_response`);
  this is cosmetic only now that `CostTracker` records the real usage.

**Per-field usage merge (batch 1, rev 2.6 fix)**: a naïve
`delta.or_else(seed)` in `pump_stream` would zero input + cache tokens whenever
`message_delta.usage` is present (because the delta carries `output` only; its
`input`/`cache_*` fields are `0`). The fix implements per-field merge identical
to `agent::accumulator::merge_usage`: `output` always takes the delta value;
`input`/`cache_write`/`cache_read`/`reasoning_output` take the delta value only
when non-zero, otherwise keep the `MessageStart` seed. This is the real
Anthropic wire contract. TDD: wire-shaped RED test
(`per_field_usage_merge_preserves_input_and_cache_from_message_start` in
`streaming_loop.rs`) observed failing on old code; GREEN after fix. The existing
`streaming_cost_recording_test.rs` tests were reshaped to use the real wire form
(start=input-only, delta=output-only) to prevent them from masking the bug.
