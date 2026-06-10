# llm-client Engine Adoption — Plan 3a (live-path swap; api-client kept-but-unused)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the live engine path (agent seam → orchestrator adapter → providers registry → apps hosts) onto llm-client + the Plan-2 policy modules, with claude-code-TS-arbitrated retry/header semantics. api-client remains in-tree but off the live path; deletion is Plan 3b.

**Prerequisite reading for every task:** `docs/superpowers/plans/2026-06-11-llm-client-adoption-plan3-prereqs.md` (same directory) and spec rev2.2.

**Arbitrations (resolved against `/Users/luolingfeng/Projects/LingXi-Next/claude-code` TS source — final):**
1. Retry budget: TS `withRetry.ts:189` loops `attempt <= maxRetries+1` → budget N = initial + N retries. Plan-2 `next_step` (4 executions at budget 3) is CORRECT; api-client's 3-total loop was the divergence. No change to `next_step`.
2. x-should-retry (`withRetry.ts:732-751`): `false` → terminal unless `USER_TYPE=ant` && 5xx; `true` → retry when `!is_subscriber || is_enterprise`. Implemented in the wire adapter (header access pre-decode), Task 5.
3. OAuth beta on messages.create (`utils/http.ts:78-82`): TS stamps `anthropic-beta: oauth-2025-04-20` with the Bearer header → llm-client `authenticate()` behavior is correct; `apply_beta_header` merge preserves it. Resolved, no change.
4. Mid-stream errors: TS does not auto-retry streams; errors surface to the turn loop. llm-client semantics already match. Resolved.

**Working directory:** cargo from `lingxi-code/`; git from the worktree root. House rules: TDD (observe RED), clippy clean on touched crates, conventional commits with trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. After EVERY task: full `cargo check --workspace` green.

---

### Task 1: llm-client schema parity for the accumulator (anthropic extended blocks)

agent's accumulator/runner consume block kinds llm-client lacks. Add, TDD, in `llm-client`:

- `ContentBlock` variants: `ServerToolUse { id: String, name: String, input: Value }`, `ConnectorText { text: String, connector_id: String }` and `AdvisorToolResult { content: Value }` — mirror the EXACT field names/shapes of `api-client/src/types.rs::ContentBlockApi` (READ IT FIRST; adjust variant fields to match it, including serde tags used on the anthropic wire: `server_tool_use`, plus the two feature-gated block tags exactly as api-client decodes them).
- `ContentDelta` variants: `CitationsDelta { citation: Value }` and `ConnectorTextDelta { text: String }` (match api-client's `ContentDelta` shapes/tags).
- `Usage.speed: Option<String>` populated from anthropic usage JSON key `speed` in `normalize_anthropic_usage` (serde skip-if-none; check api-client `UsageApi.speed` semantics: values like "fast").
- Anthropic codec: decode these block/delta types in `decode_content_block`/`decode_content_delta` (they are currently skipped as unknown) and ENCODE `ServerToolUse` back (tool-use round-trip); `ConnectorText`/`AdvisorToolResult` encode per api-client's encoding if it exists, else reject-with-message like Document.
- OpenAI/Gemini codecs: extend their reject/skip arms for the new variants mirroring existing Reasoning handling.
- Tests in `llm-client/tests/anthropic_codec_test.rs`: decode each new block from response JSON; stream delta decode for both new deltas; usage speed decode; round-trip encode for ServerToolUse. Update `validate_capabilities` only if api-client gated these (check; default: no gating).

Verify `cargo test -p llm-client` green+clippy; commit `feat(llm-client): anthropic extended blocks (server_tool_use, connector_text, advisor_tool_result), citation/connector deltas, usage speed`.

### Task 2: agent convert module + seam trait re-type (compile boundary starts)

- New `agent/src/convert.rs`: `pub fn to_llm_messages(messages: Vec<protocol::ConversationMessage>) -> Vec<llm_client::Message>` and `pub fn to_tool_declarations(tools: Vec<serde_json::Value>) -> Result<Vec<llm_client::ToolDeclaration>, llm_client::LlmError>` — mapping table (protocol → llm_client): `User/Assistant` roles; `ContentBlock::Text→Text`, `ToolUse→ToolCall`, `ToolResult{tool_use_id,content,is_error}→ToolResult{tool_call_id, output: Value::String(content), is_error}`, `Thinking{thinking,signature}→Reasoning{text,signature}`, `Image{source}→Image{media_type,bytes}` (decode base64 from protocol's ImageSource — READ protocol/src/messages.rs for its exact shape), `Document` likewise; `System` conversation messages are NOT expected in the vec (assert via error). Tools JSON `{name, description, input_schema}` → ToolDeclaration (error on missing fields).
- `agent/src/api.rs`: re-type `SubagentApiClient` — `messages_create -> Result<llm_client::LlmResponse, llm_client::LlmError>`; `messages_create_stream -> Result<BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>, llm_client::LlmError>`; default impl uses re-typed `response_to_stream_events`. `system: Option<&str>` parameter stays (adapter builds `Vec<SystemBlock>`).
- agent Cargo.toml gains llm-client.
- TDD: convert tests (each block kind, tool decl error case); compile errors downstream (accumulator/runner) are EXPECTED — fix in Tasks 3-4; this task may leave the workspace RED at its end ONLY if Tasks 2-4 are committed together... they must NOT be. Therefore: Task 2 keeps old trait alongside? NO — instead Task 2 = convert.rs ONLY (additive, green), trait re-type happens in Task 4 with accumulator+runner in ONE commit (the type swap is atomic within agent). Commit Task 2: `feat(agent): protocol→llm-client conversion module`.

### Task 3: agent accumulator re-type (prep in isolation)

Rewrite `agent/src/accumulator.rs` against `llm_client::{LlmEvent, LlmResponse, ContentBlock, ContentDelta, Usage}` as a NEW parallel module `accumulator_llm.rs` (additive, old one untouched): `accumulate_stream(BoxStream<Result<LlmEvent, LlmError>>) -> Result<LlmResponse, LlmError>` and `response_to_stream_events(LlmResponse) -> Vec<LlmEvent>`. Port the merge/usage/stop_reason logic 1:1 (READ the old file; same block-index bookkeeping; `merge_usage` over llm_client Usage incl. speed; `StreamEvent::Error` equivalent = stream item Err). Port ALL accumulator tests re-typed. Commit `feat(agent): llm-client event accumulator (parallel module)`.

### Task 4: agent seam atomic swap

In ONE commit: api.rs trait re-type (per Task 2 description); delete old accumulator.rs, rename accumulator_llm.rs → accumulator.rs (keep module name/api); runner.rs re-type (ContentBlockApi match sites → llm_client::ContentBlock: `Text{text,..}`, `ToolCall{id,name,input}`, `Reasoning{text,signature}`, drop ServerToolUse/ConnectorText/AdvisorToolResult arms same as before; MessageResponse field reads → LlmResponse: `content`, `stop_reason` (now Option<String> on the response), usage); runner test fixtures re-typed (MessageResponse literals → LlmResponse, StreamEvent → LlmEvent). agent must be fully green standalone; orchestrator now RED — acceptable ONLY within this task if the orchestrator re-type (Task 5) lands in the SAME commit? NO — keep workspace green: orchestrator still implements the OLD trait via its adapter... it implements `agent::SubagentApiClient` whose signature changed → orchestrator breaks. THEREFORE Tasks 4+5+6 form one **compile unit**: implementer executes Task 4 and Task 5 and Task 6 as a single batch with one commit at the end of Task 6 IF intermediate commits can't compile. Preferred: try to land Task 4 + minimal orchestrator adapter re-type (Task 5 core) in one commit, then Task 6 separately if separable. The PLAN accepts a single larger commit here; reviews still happen per-task scope.

### Task 5: orchestrator wire adapter v2 + loops re-type (the core swap)

`orchestrator/src/provider_adapter.rs` rebuilt on llm-client (delete ModelRouter usage):

- Adapter state: `DefaultLlmClient` + `Arc<LlmTransportBridge<ReqwestHttp-ish>>` (held as `Arc<dyn llm_client::Transport>`), `Option<Arc<AnalyticsBus>>`, `Arc<CostTracker>`, subscriber flags, fallback config. Constructor takes these from the host (apps wire in Task 7).
- **Non-stream path** (`messages_create`): build `LlmRequest` (model, system → `vec![SystemBlock::text(s)]` when Some, convert::to_llm_messages, to_tool_declarations, `max_tokens`/thinking per existing turn defaults — READ what api-client's `messages_create_non_stream_with_thinking` sent and mirror); loop: `client.prepare(&request).await` → `model::betas::apply_beta_header(&mut prepared.provider_request, Provider::Anthropic, Endpoint::MessagesCreate)` (Anthropic routes only — gate on resolved provider) → insert `user-agent` (ported `user_agent()`) and request-id header (`new_request_id()` port; api-client header name: READ anthropic.rs build_request for the exact header) → `transport.execute(&prepared.provider_request).await` → **pre-decode header pass**: capture headers for rate_limit trackers + `x-should-retry` override → `prepared.route.codec.decode_response(response)`; on Err: telemetry emit_failed (ported error_kind/status_of re-typed to LlmError — port these label fns into `orchestrator/src/model/labels.rs` with TS-locked strings), feed `model::retry::next_step` (+x-should-retry/429-gate overrides BEFORE next_step per arbitration #2: should_retry=false → Terminal unless USER_TYPE=ant && status>=500; 429 when `is_subscriber && !is_enterprise` → terminal with the rate-limit copy; 429 retry path sleeps the reset ladder retry-after→unified→requests-reset→1s and emits emit_rate_limited); DriveStep::RetryAfter → sleep+retry; AdjustMaxTokens → set request.max_tokens, retry without consuming; Fallback → swap model, telemetry, retry; Terminal → map to the turn-facing error. 401 reactive: on `LlmError::Authentication` once per call, force OAuth refresh (Task 6 adds `OAuthCredentialProvider::force_refresh()` calling the driver refresh) then retry once.
- **Stream path**: `client.execute_stream` equivalent — but headers must survive: open via `prepare` + beta/UA headers + `transport.open_stream` manually, drain ≥400 via the same error path (llm-client `decode_stream_error` is private — REPLICATE its small logic locally or make it pub(crate)→pub in a tiny llm-client patch task: choose making `LlmEventStream::new` + a `pub async fn open_llm_event_stream(codec, frames)`-style helper public in llm-client IF needed; prefer minimal llm-client addition: `pub fn event_stream(decoder, frames) -> LlmEventStream` constructor). Wrap into `BoxStream<Result<LlmEvent, LlmError>>` via `futures::stream::unfold` on `next_event`.
- `streaming_loop.rs`/`turn_loop.rs`/`conversation.rs`/`error.rs`/`sse/*`: re-type StreamEvent→LlmEvent, ApiError→LlmError (PTL: `LlmError::ContextOverflow` + ported `prompt_too_long` token-gap copy; FallbackTriggered interception → DriveStep::Fallback is internal to the adapter now: turn_loop's fallback handling moves/simplifies — READ turn_loop 606-728 and preserve observable behavior incl. PTL retry looping); `cost_wiring.rs`: `usage_api_to_cost_usage` → `llm_usage_to_cost_usage(&llm_client::Usage)` (buckets map 1:1; speed string passthrough; server_tool_use web_search_requests u64→u32 clamp); record_api_response_v2 call sites re-typed; "Repeated 529 Overloaded errors" byte-locked copy rendered when terminal error is Overloaded && state.consecutive_overloaded >= MAX_529_RETRIES.
- Port `resolve_retry_control` (envs FALLBACK_FOR_ALL_PRIMARY_MODELS/USER_TYPE/IS_SANDBOX + is_non_custom_opus + is_subscriber) into `orchestrator/src/model/retry.rs` (it already hosts RetryControl) with its TS-locked tests.
- All orchestrator tests re-typed; test_support fakes re-typed. This task is the largest: the implementer may sub-commit compiling milestones if each keeps `cargo check --workspace` green; otherwise one commit.

### Task 6: anthropic-oauth decoupling + reactive refresh entry

- Add inherent `pub async fn force_refresh(&self) -> Result<(), LlmError>`-equivalent on `OAuthCredentialProvider` (drives the single-flight refresh regardless of expiry; maps errors to Authentication) + test.
- Replace `credential_provider.rs`'s internal use of `api_client::oauth_hook::OAuthRefreshHook` with an inherent method on `RefreshDriver` (add `pub(crate) async fn refresh_now(&self, prev: TokenHash-equiv) -> ...` if needed) so anthropic-oauth no longer NEEDS api-client for this module (the legacy hook impl may stay until 3b). Tests stay green.
- Commit separately.

### Task 7: providers crate re-core + apps hosts swap

- `providers` crate: `ProviderRegistry` internals build per-profile `llm_client::ProviderProfile`s → ONE `DefaultLlmClient` (ClientConfig from parse_profiles: kind→ProtocolFamily/ProviderId, base_url defaults per family, api_key_env→CredentialConfig::Env, models: profile model lists — READ providers/src/profile.rs + builtin_profiles for the existing model tables and map them incl. aliases/capabilities; reasoning_effort threading → ReasoningConfig where applicable). Public surface becomes: `build_llm_client(profiles, env, routing) -> (DefaultLlmClient, RoutingConfig)`-style constructor consumed by the orchestrator adapter; `ModelRouter`/`Resolved`/`LlmProvider`/GenericClient paths deleted or feature-stubbed (prefer delete; this crate's api-client dep drops).
- PricingCatalog population: `orchestrator/src/cost_wiring.rs` (or providers) gains `fn llm_pricing_catalog(catalog: &cost::PricingCatalog) -> llm_client::PricingCatalog` mapping ModelRef/nano-USD → TokenPricing per-million (nano_usd_per_token × 1e6 ÷ 1e9 = usd_per_million: `(nano as f64) * 1e6 / 1e9`); wire CostEstimator where the adapter records cost (or keep CostTracker as today and defer estimator — KEEP CostTracker path, catalog fn still added + tested for Plan-3b/future use).
- apps/engine-desktop + engine-mobile: replace AnthropicProvider/ModelRouter construction with: ReqwestHttp → `LlmTransportBridge::new` → providers' new constructor → adapter::new(client, transport, bus, cost_tracker, subscriber flags, fallback from routing, oauth credential provider when OAuth configured via `with_credential_provider(Arc<OAuthCredentialProvider>)`).
- Workspace check + targeted app smoke tests green; commit.

### Task 8: full verification + docs

- `cargo test --workspace` (entire tree) — record totals; clippy on agent/orchestrator/providers/apps/llm-client/anthropic-oauth; `cargo test -p api-client` still green (unused but intact).
- Spec rev2.3: record arbitrations 1-4 outcomes, Plan-3a完成 scope, Plan-3b remaining (deletion + blast-radius scrub of client-adapter/commands/compaction/examples/sidequery + workspace members + oauth legacy hook removal).
- Commit docs.

## After this plan (Plan 3b)

Delete api-client; scrub remaining consumers (client-adapter, commands/core, compaction, examples/cli-demo, hook_prompt_runner, sidequery tests); remove legacy oauth hook impl + api-client dep from anthropic-oauth; workspace members cleanup; final E2E + memory updates.
