# Plan 3a — Route the engine model seam onto llm-client (+ true-parity fixes)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Dispatch a fresh implementer subagent per task with two-stage review (spec then quality). Per the project's concurrent-agent worktree hazard memory: run edit-agents SEQUENTIALLY in this one checkout, never in parallel.

**Goal:** Make the engine's live model path flow `orchestrator → ProviderApiAdapter → llm_client::DefaultLlmClient (prepare/execute/execute_stream) → Transport`, retyping the agent + orchestrator seams to `llm_client::{LlmRequest, LlmResponse, LlmEvent, LlmError}`, wiring the (currently dead) Plan-2 `orchestrator/src/model/*` policy modules onto that path, and correcting five claude-code parity divergences. **api-client stays in the tree** (its deletion + the `providers`-crate removal + `modelProviders` settings are Plan 3b/3c). End state: the engine compiles and is green on llm-client for the live Anthropic route; api-client is no longer on the live model path.

**Architecture:** The production `ProviderApiAdapter` (orchestrator) today wraps a `providers::ModelRouter` whose Anthropic backing is `api_client::AnthropicProvider` — which runs its own retry loop and swallows response headers. Plan 3a replaces that adapter body with a direct `DefaultLlmClient` drive: `prepare()` → `transport.execute()` / `open_stream()` → `codec.decode_response()`, so `HttpResponse.headers` survive to feed `model/rate_limit.rs` + `model/retry.rs`. The orchestrator's three seam traits (`OrchestratorApiClient`, `StreamingApiClient`, `NoStreamingApiClient`) and the `agent::SubagentApiClient` trait switch return types from api-client to llm-client. The retry **driver** becomes `model/retry.rs`'s `next_step` loop wrapped around the manual prepare/execute, with claude-code cadence and the corrected budget.

**Decisions locked (user-approved, 2026-06-11):**
1. **Remove the `providers` crate from the live path** — drive llm-client directly (design intent / prereqs item 12). (The `providers` crate is *not deleted* in 3a — only bypassed in `ProviderApiAdapter`; deletion is 3b.)
2. **Fix to true claude-code parity** — five corrections, each pinned by a TS-anchored test (Tasks 1–3, 7, 8).
3. **3a/3b/3c split** — 3a is this plan; 3b deletes api-client + the providers crate + clears ~11 dependents; 3c adds `modelProviders` settings + multi-provider routing.

**Branch:** `parity-llm-client-3a`. **Conventions:** Cargo root `lingxi-code/`; run git from the repo root with explicit `lingxi-code/...` paths; **NEVER `git add -A`** (untracked `codex/`, `liter-llm/`, `opencode/`, `.omo/`, `.codegraph/` dirs live at repo root — stage only named paths). `-D missing-docs` + clippy pedantic `-D warnings`. `traits/` + `protocol/` are FROZEN — additive only; verify `git diff main -- lingxi-code/traits lingxi-code/protocol` is empty at the end. **engine-mobile must keep building.** Commit footer EXACTLY:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```
Commit with `git commit -F <tempfile>` from the repo root.

**Reference of truth:** `claude-code/src` (un-minified). Key files: `services/api/withRetry.ts` (retry loop), `utils/betas.ts` (beta assembly), `constants/oauth.ts` (`OAUTH_BETA_HEADER`), `utils/http.ts` (User-Agent), `services/api/claude.ts` (streaming→non-streaming fallback), `services/rateLimitMessages.ts` + `services/api/errors.ts` (copy strings).

---

## Type-mapping table (api-client → llm-client) — applies to every retype task

| api-client | llm-client | Notes |
|---|---|---|
| `api_client::MessageResponse` (`types.rs:83`) | `llm_client::LlmResponse` (`protocol.rs:196`) | fields `id/model/content/stop_reason/usage`; LlmResponse adds `cost`/`provider_metadata` |
| `api_client::ApiError` (`error.rs`) | `llm_client::LlmError` (`error.rs:6`) | variant remap below |
| `api_client::types::StreamEvent` | `llm_client::LlmEvent` (`protocol.rs:219`) | LlmEvent **drops `Ping`/`Error`**, **adds `Completed{response}`** |
| `types::ContentBlockApi::Text{text}` | `ContentBlock::Text{text, cache_control}` | |
| `types::ContentBlockApi::ToolUse{id, name, input}` | `ContentBlock::ToolCall{id: String, name, input}` | `id` was `ToolUseId` → now `String` |
| `types::ContentBlockApi::Thinking{thinking, signature}` | `ContentBlock::Reasoning{text, signature}` | field `thinking`→`text` |
| `types::ContentDelta` | `llm_client::ContentDelta` (`protocol.rs:263`) | `TextDelta/InputJsonDelta/ThinkingDelta/SignatureDelta` |
| `types::MessageDeltaPayload{stop_reason}` | `llm_client::MessageDeltaPayload{stop_reason}` (`protocol.rs:288`) | |
| `types::UsageApi` | `llm_client::Usage` | feeds `cost::Usage` in `cost_wiring.rs` |
| `api_client::PROMPT_TOO_LONG_ERROR_MESSAGE` | `orchestrator::model::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE` | already ported in model/ |

**LlmError variant remap** (for `error.rs`, `turn_loop.rs`, capability gates): `ApiError::Http(InvalidRequest)` → `LlmError::InvalidRequest{message}`; `ApiError::Overloaded{repeated}` → `LlmError::Overloaded` (repeated bit reconstructed at render time, Task 1); `ApiError::Server{429,..}` rate-limit → `LlmError::RateLimited{retry_after, scope}`; context overflow → `LlmError::ContextOverflow`.

---

### Task 0: branch + baseline

**Files:** none (setup).

- [ ] From repo root: `git checkout main && git pull` (if remote) then `git checkout -b parity-llm-client-3a`.
- [ ] Baseline: `cd lingxi-code && cargo build -p orchestrator -p agent -p anthropic-oauth -p engine-desktop -p engine-mobile` and `cargo test -p orchestrator -p agent` — record pass/fail counts. (The pre-existing unrelated `sidequery` test failure on main is expected; note it and ignore.)
- [ ] No commit.

---

### Task 1: parity — retry budget 10 + loop semantics (model/retry.rs)

**Files:** Modify `orchestrator/src/model/retry.rs` (+ tests). Reference: `claude-code/src/services/api/withRetry.ts:52,179,189,789-796`.

**Parity facts (first-hand verified):** `DEFAULT_MAX_RETRIES = 10`; loop `for (attempt = 1; attempt <= maxRetries + 1; attempt++)` → **11 executions / up to 10 sleeps**; `MAX_529_RETRIES = 3` (independent consecutive-overload sub-budget); env override `CLAUDE_CODE_MAX_RETRIES` (`getMaxRetries → options.maxRetries ?? CLAUDE_CODE_MAX_RETRIES ?? 10`).

- [ ] **Step 1 (failing test):** add `default_retry_budget_matches_claude_code` asserting the default max-retries constant is `10` (not the current `3`), and `claude_code_max_retries_env_overrides` asserting `CLAUDE_CODE_MAX_RETRIES=5` yields budget `5`. Add `next_step_allows_eleven_executions_at_default`: drive `next_step` returning `RetryAfter` each time and assert it returns `Terminal` only after the 11th execution attempt (10 sleeps) at the default budget.
- [ ] **Step 2:** run, confirm FAIL (current `DEFAULT_RETRY_BUDGET = 3`, `orchestrator/src/model/retry.rs:28`).
- [ ] **Step 3 (impl):** rename/replace the `3` budget with `DEFAULT_MAX_RETRIES: u32 = 10`; add `fn max_retries_from_env() -> u32` reading `CLAUDE_CODE_MAX_RETRIES` (parse, fall back to 10); make the `next_step` terminal condition fire at `attempt > max_retries` (so initial + `max_retries` retries = `max_retries + 1` executions). Keep `MAX_529_RETRIES = 3` untouched. Update the module doc to cite `withRetry.ts:52,189`.
- [ ] **Step 4:** run, confirm PASS; run the whole `cargo test -p orchestrator --lib model::retry` green.
- [ ] **Step 5 (commit):** stage `orchestrator/src/model/retry.rs`; commit `fix(orchestrator): retry budget → claude-code DEFAULT_MAX_RETRIES=10 (+CLAUDE_CODE_MAX_RETRIES)`.

---

### Task 2: parity — OAuth beta appended for subscribers, not clobbering (model/betas.rs + llm-client client.rs)

**Files:** Modify `orchestrator/src/model/betas.rs` (+ tests); Modify `llm-client/src/client.rs` (`authenticate`, lines 219-226). Reference: `claude-code/src/utils/betas.ts:234,251-252`, `constants/oauth.ts:36` (`OAUTH_BETA_HEADER = 'oauth-2025-04-20'`).

**Parity fact:** under OAuth subscriber auth, claude-code appends `oauth-2025-04-20` as **one element of the comma-joined `anthropic-beta` list**, alongside the model betas (`claude-code-20250219`, interleaved-thinking, etc.). It never sends it as a standalone clobbering header. The current `api_client::betas.rs:184-186 (OAUTH, false)` hard-codes it OFF (wrong); `llm-client/src/client.rs:225` `.insert("anthropic-beta", "oauth-2025-04-20")` clobbers model betas (wrong).

- [ ] **Step 1 (failing test, betas.rs):** `oauth_beta_appended_for_subscriber`: build the merged beta list for a subscriber OAuth route with model betas present; assert the result contains BOTH `claude-code-20250219` (or whatever the model beta is) AND `oauth-2025-04-20`, comma-joined, with `oauth-2025-04-20` present exactly once. `oauth_beta_absent_for_non_subscriber`: assert it is NOT added when `is_subscriber == false`. `oauth_beta_absent_for_api_key_auth`: not added under ApiKey auth.
- [ ] **Step 2:** run, confirm FAIL.
- [ ] **Step 3 (impl, betas.rs):** add `const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";` thread an `auth: AuthStrategy` (or `is_oauth_subscriber: bool`) input into the beta-merge entry and append `OAUTH_BETA_HEADER` when `is_oauth_subscriber`, deduped, into the comma-joined header value written onto `ProviderRequest.headers`.
- [ ] **Step 4 (impl, client.rs):** in `authenticate` (llm-client/src/client.rs:219-226), change the clobbering `.insert(...)` so it **does not overwrite** an existing `anthropic-beta` header: if absent, set `oauth-2025-04-20`; if present, append `, oauth-2025-04-20` only when not already contained. Add an llm-client unit test `oauth_beta_does_not_clobber_existing_betas` (prepare a request that already carries an `anthropic-beta` header, OAuthBearer route, assert both values survive comma-joined).
- [ ] **Step 5:** run `cargo test -p orchestrator --lib model::betas` and `cargo test -p llm-client` → green.
- [ ] **Step 6 (commit):** stage `orchestrator/src/model/betas.rs lingxi-code/llm-client/src/client.rs`; commit `fix(llm-client,orchestrator): append oauth-2025-04-20 beta for subscribers (no clobber) — claude-code betas.ts parity`.

---

### Task 3: parity — byte-locked `user_agent()` (model/user_agent.rs)

**Files:** Create `orchestrator/src/model/user_agent.rs`; Modify `orchestrator/src/model/mod.rs` (add `pub mod user_agent;`). Reference: `claude-code/src/utils/http.ts:18-35`.

**Parity template (byte-exact):**
```
claude-cli/<VERSION> (<USER_TYPE>, <ENTRYPOINT>[, agent-sdk/<V>][, client-app/<APP>][, workload/<W>])
```
- `<VERSION>` = the build version macro; `<USER_TYPE>` = `USER_TYPE` env inlined RAW (literal `undefined` if unset, matching JS); `<ENTRYPOINT>` = `CLAUDE_CODE_ENTRYPOINT` env, default `cli`; the three optional `, key/val` suffixes appear in order only when `CLAUDE_AGENT_SDK_VERSION` / `CLAUDE_AGENT_SDK_CLIENT_APP` / `getWorkload()` are set. Separators are exactly `", "`; no space after `(`. The `claude-cli/` prefix is load-bearing — do not alter.

- [ ] **Step 1 (failing test):** `user_agent_minimal` (only USER_TYPE + entrypoint set) → exact `claude-cli/<v> (external, cli)`; `user_agent_with_all_suffixes` → exact string with all three suffixes in order; `user_agent_unset_user_type_is_undefined` → contains `(undefined, cli)`; `user_agent_default_entrypoint_cli`. Use an injectable env map (a `UserAgentEnv` struct of `Option<String>` fields + a version param) so the test is deterministic (no real env mutation).
- [ ] **Step 2:** run, confirm FAIL (module absent).
- [ ] **Step 3 (impl):** `pub fn user_agent(env: &UserAgentEnv, version: &str) -> String` building the template exactly. Provide `UserAgentEnv::from_process_env()` reading the real envs for production callers. `#![allow]` nothing; keep `-D missing-docs` satisfied.
- [ ] **Step 4:** run green.
- [ ] **Step 5 (commit):** stage `orchestrator/src/model/user_agent.rs orchestrator/src/model/mod.rs`; commit `feat(orchestrator): byte-locked claude-cli User-Agent (http.ts parity)`. (Wired onto requests in Task 6.)

---

### Task 4: retype the `agent` seam to llm-client

**Files:** Modify `agent/src/api.rs`, `agent/src/accumulator.rs`, `agent/src/runner.rs`; Modify `agent/Cargo.toml` (+`llm-client`, drop `api-client` once no longer referenced — but keep it if any test still needs it; verify). Anchors from survey: trait `api.rs:29-67`; accumulator `accumulate_stream` `accumulator.rs:313`, `into_content_block` `:99`, `response_to_stream_events` `:399`; runner `translate_response_blocks` `runner.rs:123`, stop-reason branch `runner.rs:434`, mocks `runner.rs:645-893`.

- [ ] **Step 1 (trait, TDD):** change `SubagentApiClient::messages_create` return to `Result<llm_client::LlmResponse, llm_client::LlmError>` and `messages_create_stream` to `Result<BoxStream<'static, Result<llm_client::LlmEvent, llm_client::LlmError>>, llm_client::LlmError>`. Update the default `messages_create_stream` body to call `accumulator::response_to_stream_events` (now emitting `LlmEvent`). Update imports (`use llm_client::{LlmEvent, LlmError, LlmResponse}`).
- [ ] **Step 2 (accumulator):** retype `accumulate_stream` to drain `BoxStream<LlmEvent>` into `LlmResponse`; map the event grammar (`MessageStart/ContentBlockStart/ContentBlockDelta/ContentBlockStop/MessageDelta/MessageStop`, plus the new `Completed{response}` short-circuit). The old `Ping`/`Error` arms (`accumulator.rs:376-382`) are removed — `LlmEvent` has neither (transport/error are surfaced as `Err(LlmError)` from the stream). Retype `into_content_block` to build `ContentBlock::{Text, ToolCall, Reasoning}` per the mapping table. Retype `response_to_stream_events` to emit `LlmEvent`. Update the accumulator tests to construct `LlmEvent`/`LlmResponse` literals.
- [ ] **Step 3 (runner):** retype `translate_response_blocks(content: &[llm_client::ContentBlock])` mapping `ContentBlock::{Text, ToolCall{id,name,input}, Reasoning{text,signature}}` → `protocol::ContentBlock` (drop the variants the old code dropped). The stop-reason branch (`runner.rs:434` `stop_reason.as_deref() == Some("tool_use")`) is unchanged — `LlmResponse.stop_reason: Option<String>` keeps Anthropic vocabulary. Tool-use extraction reads `ContentBlock::ToolCall`. Retype the in-file mocks `MockSubagentApiClient`/`StreamingMockApiClient` to produce `LlmResponse`/`LlmEvent`.
- [ ] **Step 4 (gate):** `cargo test -p agent` green; clippy `-p agent -D warnings`.
- [ ] **Step 5 (commit):** stage the agent paths + `agent/Cargo.toml`; commit `refactor(agent): retype SubagentApiClient seam to llm_client types`.

---

### Task 5: retype the orchestrator seam traits + the ~8 production consumers

**Files:** Modify `orchestrator/src/conversation.rs` (trait defs for `OrchestratorApiClient` `:116`, `StreamingApiClient` `:203`, `NoStreamingApiClient`, and the unused `AnthropicProviderAdapter`/`AnthropicProviderStreamingAdapter` `:2481-2624` — retype or mark for 3b deletion), `orchestrator/src/turn_loop.rs` (`:6-7,:182,:1848`), `orchestrator/src/error.rs` (`:7`), `orchestrator/src/cost_wiring.rs` (`:6`), `orchestrator/src/hook_prompt_runner.rs` (`:24-25`), `orchestrator/src/streaming_loop.rs` (`:16-17`), `orchestrator/src/sse/event_router.rs` (`:16`), `orchestrator/src/test_support.rs` (`:8,:103`), `orchestrator/src/test_support_stream.rs` (`:18-21`), `orchestrator/src/config.rs` + `orchestrator/src/sse/accumulator.rs` (doc-comment-only stale refs — fix text). Survey section C is the full inventory.

> This is a mechanical re-type guided by the table. Implementer reads each file's actual `api_client::` symbols (survey lists them per file) and swaps to the llm-client equivalent + remaps `ApiError` arms to `LlmError`. The orchestrator's own SSE accumulator/event_router consume `LlmEvent` (lose `Ping`/`Error`, gain `Completed`).

- [ ] **Step 1:** retype the three seam traits' return types to `LlmResponse`/`LlmEvent`/`LlmError`. Keep inputs (`model/system/messages/tools`) unchanged.
- [ ] **Step 2:** retype each production consumer file (turn_loop, error, cost_wiring, hook_prompt_runner, streaming_loop, sse/event_router). For `turn_loop.rs:182`, drop the `pub(crate) use api_client::PROMPT_TOO_LONG_ERROR_MESSAGE` re-export in favor of `model::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE`. For `error.rs`, the orchestrator error converts from `LlmError` instead of `ApiError`. For `cost_wiring.rs`, `UsageApi`→`llm_client::Usage`→`cost::Usage`.
- [ ] **Step 3:** retype `test_support.rs` + `test_support_stream.rs` mocks (`MockApiClient` returns `Result<LlmResponse, LlmError>`; stream mocks emit `LlmEvent`). These are `feature = "test-support"` and reused by cli/tui, so they must compile.
- [ ] **Step 4 (gate):** `cargo build -p orchestrator` and `cargo test -p orchestrator` compile (the adapter body in Task 6 makes them pass; if traits compile but `ProviderApiAdapter` doesn't yet, land Task 5+6 as a pair before gating tests). Fix stale doc refs in `config.rs`/`sse/accumulator.rs`.
- [ ] **Step 5 (commit):** stage the listed orchestrator paths; commit `refactor(orchestrator): retype model seam traits + consumers to llm_client types`.

---

### Task 6: rewire `ProviderApiAdapter` to drive `DefaultLlmClient` directly (the heart)

**Files:** Modify `orchestrator/src/provider_adapter.rs` (replace the `providers::ModelRouter` body); Modify `orchestrator/Cargo.toml` (+`llm-client`; keep `providers`/`api-client` deps for now — they go in 3b). Drives Task-1 retry, Task-2 betas, Task-3 user_agent, and the existing `model/{rate_limit,fallback,overflow,count_tokens,telemetry}`.

**New adapter shape:**
```rust
pub struct ProviderApiAdapter {
    client: Arc<llm_client::DefaultLlmClient>,
    transport: Arc<dyn llm_client::Transport>,
    retry: model::retry::RetryConfig,       // budget from Task 1
    subscriber: SubscriberState,            // Task 8: is_subscriber/is_enterprise
    ua: model::user_agent::UserAgentEnv,    // Task 3
    analytics: Option<Arc<dyn telemetry::AnalyticsBus>>, // Task: telemetry emission
}
```
- [ ] **Step 1 (non-stream drive):** rewrite `OrchestratorApiClient::messages_create` to: build an `LlmRequest` from `(model, system, messages, tools)` (port the `ConversationMessage → llm_client::Message` + tool-JSON → `ToolDeclaration` conversion — this is the conversion the design says lives at the seam; reuse/adapt the `providers::CanonicalRequest` builder logic but emit `LlmRequest`); keep `strip_excess_media` (port to operate on the messages before conversion). Then run the **retry driver loop** around: `let prepared = client.prepare(&req).await?` → apply Task-3 user_agent + Task-2 betas onto `prepared.provider_request.headers` (post-prepare, headers are public) → `let resp = transport.execute(&prepared.provider_request).await` → on success `prepared.route.codec.decode_response(resp)`; on the returned `ProviderResponse`/error, feed headers to `model::rate_limit` (RateLimitInfo::from_headers) + classify via `model::retry::next_step` (RetryAfter→sleep using the reset ladder retry-after→unified-reset→requests-reset→1s; AdjustMaxTokens→re-encode via `model::overflow::adjusted_max_tokens`; Fallback→`model::fallback` opus→sonnet on Anthropic; Terminal→map to `LlmError`). Emit `model::telemetry` events at the started/succeeded/failed points. The capability gate (vision/native_tools) now comes free from `validate_capabilities` inside `prepare()`; drop the manual `providers` capability checks (or keep a thin pre-check returning `LlmError::InvalidRequest`).
- [ ] **Step 1b (x-should-retry parity — prereqs item 2):** in the non-stream driver, BEFORE status-classifying a retryable 5xx, honor `x-should-retry: false` as terminal (claude-code `withRetry.ts:746-750`; the ant-only 5xx carve-out is intentionally omitted for this external build, matching `api-client/src/retry.rs:196`). Because Task 6 surfaces `ProviderResponse.headers` to the driver, add a header pre-check `if header("x-should-retry") == "false" { Terminal }`. Test `x_should_retry_false_is_terminal_for_5xx`: a 503 carrying `x-should-retry: false` is NOT retried.
- [ ] **Step 2 (stream drive):** rewrite `StreamingApiClient::stream` to set `req.stream = true`, drive `client.prepare` + header injection + `transport.open_stream`, and return the `LlmEventStream` adapted to `BoxStream<Result<LlmEvent, LlmError>>`. Connect-phase failures retry through the driver; post-first-event `StreamInterrupted` does NOT retry (already enforced by `LlmEventStream`, client.rs:362-372). The `SubagentApiClient` impl keeps forwarding to these. **Streaming header note (prereqs item 11):** `execute_stream` exposes `streaming.status` + `streaming.headers` at the llm-client Transport layer, but the Plan-1 `LlmTransportBridge` may not yet populate them from `platform_api::HttpTransport::stream_sse` (which surfaces no metadata). Connect-phase status (≥400 → `decode_stream_error`) must work; if streaming-route **rate-limit header tracking** needs the metadata and the bridge can't supply it, scope that as best-effort and surface the gap in review (an *additive* `stream_sse` metadata return is allowed on the frozen `traits` — prefer it over dropping the feature, but only if it stays additive).
- [ ] **Step 3 (rate-limit → TUI):** confirm the orchestrator populates the protocol rate-limit status type from `model::rate_limit` on the live path (the tracker now actually receives headers). No tui change needed (tui reads the protocol type; verified no `api_client` in tui).
- [ ] **Step 4 (tests):** port the adapter's existing stub tests to a fake `llm_client::Transport` (record the `ProviderRequest`, return canned `ProviderResponse` with headers). Add: `live_path_surfaces_rate_limit_headers` (a 429 with `retry-after` drives one sleep then succeeds); `betas_and_user_agent_applied_post_prepare` (assert the outgoing `ProviderRequest.headers` carry both); `retry_terminal_after_budget` (11 executions at default). Keep media-cap tests (port `strip_excess_media` tests).
- [ ] **Step 5 (gate):** `cargo test -p orchestrator` green; clippy `-D warnings`.
- [ ] **Step 6 (commit):** stage `orchestrator/src/provider_adapter.rs orchestrator/Cargo.toml`; commit `feat(orchestrator): drive llm_client::DefaultLlmClient directly (retry+rate-limit+betas+UA live)`.

---

### Task 7: parity — mid-stream 529 → non-streaming fallback

**Files:** Modify `orchestrator/src/streaming_loop.rs` (or the adapter's stream drive); add tests. Reference: `claude-code/src/services/api/claude.ts:2404,2469-2502,2551-2594`, `withRetry.ts:141,186` (`initialConsecutive529Errors`).

**Parity fact:** after the first SSE event, an in-band error (overloaded_error / 529) is caught and — unless `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` — claude-code retries via a **fresh non-streaming request** (`executeNonStreamingRequest`) seeded with `initialConsecutive529Errors: is529Error ? 1 : 0`, rebuilding the assistant message from scratch (accumulation reset). The streaming path itself is NOT replayed.

- [ ] **Step 1 (failing test):** simulate a stream that yields one `ContentBlockStart` then a `LlmError::Overloaded` (post-first-event). Assert the loop (a) does NOT replay the stream, (b) issues a fresh **non-streaming** `messages_create`, (c) seeds the consecutive-529 counter to 1 so the non-streaming retry budget accounts for it, (d) resets accumulation (the final response is built only from the non-streaming reply). Add a twin with `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK=1` asserting the error propagates instead.
- [ ] **Step 2:** confirm FAIL (no fallback today — survey Q4).
- [ ] **Step 3 (impl):** on a post-first-event `Overloaded`/in-band error in the stream consumer, branch to the non-streaming adapter path seeding `model::retry` with `initial_consecutive_529 = if is_529 {1} else {0}`; gate on `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` (+ the existing flag if present). Reset the accumulator before adopting the non-streaming response.
- [ ] **Step 3b (Repeated-529 copy — prereqs item 10):** `LlmError::Overloaded` carries no `repeated` bit (unlike `api_client::ApiError::Overloaded{repeated}`). At terminal-529 rendering, reconstruct the byte-locked `"Repeated 529 Overloaded errors"` copy (claude-code `errors.ts:166`, thrown `withRetry.ts:359-362` for external non-sandbox) from `RetryState.consecutive_overloaded >= MAX_529_RETRIES && last_error == Overloaded`. Test `repeated_529_terminal_renders_byte_locked_copy` asserting the exact string. (The `You've hit your usage limit` 429 copy already lives in `model/rate_limit.rs`.)
- [ ] **Step 4:** run green; full `cargo test -p orchestrator`.
- [ ] **Step 5 (commit):** stage the touched paths; commit `feat(orchestrator): mid-stream 529 → non-streaming fallback with 529 carry-over (claude.ts parity)`.

---

### Task 8: parity — wire real `is_subscriber` / `is_enterprise` + port `resolve_retry_control`

**Files:** Modify `orchestrator/src/config.rs` (`:126,:163,:232-234` — currently hard-stub `false`), and the construction sites that build the config in the hosts. Reference: `withRetry.ts:331-369,737,767-769`; subscriber state from `utils/auth.js` (`isClaudeAISubscriber`/`isEnterpriseSubscriber`).

**Parity fact:** the 429 retry gate (`!is_subscriber || is_enterprise`), the `x-should-retry:true` gate, and the 529 fallback/terminal branch all read live subscriber state. Today both are stubbed `false`, so a subscriber is treated as non-subscriber (over-retries 429s). The source of truth is the auth/credential the host configured (OAuth subscriber vs API key).

- [ ] **Step 1 (failing test):** a config built from an OAuth-subscriber credential yields `is_subscriber == true`; an API-key config yields `false`; an enterprise-flagged subscriber yields `is_enterprise == true`. Assert the 429 gate in `model::retry` flips accordingly (subscriber-non-enterprise 429 → terminal, not retried).
- [ ] **Step 2:** confirm FAIL.
- [ ] **Step 3 (impl):** derive `SubscriberState{is_subscriber, is_enterprise}` from the configured credential/auth (OAuthBearer subscriber path → `is_subscriber = true`; read the enterprise flag from the OAuth profile/account info if available, else `false`), thread it into `config.rs` and the adapter (Task 6 `subscriber` field). Document the derivation + cite `withRetry.ts:767`.
- [ ] **Step 3b (port `resolve_retry_control` — prereqs item 6):** port `api_client::anthropic.rs::resolve_retry_control` (`:1187-1204`) into `model/retry.rs` (or `model/fallback.rs`), re-typed off api-client: `allow_fallback = FALLBACK_FOR_ALL_PRIMARY_MODELS (raw-truthy `||`) || (!is_subscriber && is_non_custom_opus(model))`; `is_external = USER_TYPE == "external"`; `is_sandbox = IS_SANDBOX is defined`; reuse `model::fallback::is_non_custom_opus_model`. Feed these into the driver's Fallback/Terminal decision (Task 6). Test the env matrix (`fallback_for_all` truthy; non-subscriber+opus; external+non-sandbox terminal) mirroring api-client's tests. `CLAUDE_CODE_UNATTENDED_RETRY`/persistent-mode is ant-only — not ported (document).
- [ ] **Step 4:** run green.
- [ ] **Step 5 (commit):** stage `orchestrator/src/config.rs` + host construction edits; commit `feat(orchestrator): wire real is_subscriber/is_enterprise into the 429 gate (withRetry.ts parity)`.

---

### Task 9: anthropic-oauth — re-home `RefreshDriver::refresh` to an inherent method

**Files:** Modify `anthropic-oauth/src/refresh.rs` (the `BearerToken`/`OAuthHookError`/`TokenHash` types `:16` + `impl OAuthRefreshHook for RefreshDriver` `:332`), `anthropic-oauth/src/credential_provider.rs` (`:10,:65-70`), `anthropic-oauth/src/client.rs` (`:10,:350`). Goal: nothing in anthropic-oauth routes through `api_client::oauth_hook` (so 3b can delete api-client). api-client STAYS present in 3a.

**Parity note (survey Q9 / prereqs item 9):** decide the reactive-401 refresh+retry-once path. In 3a, preserve today's behavior (refresh inside `CredentialProvider::load`); the reactive-401 path is documented and deferred to a follow-up unless trivially portable.

- [ ] **Step 1:** define the `BearerToken`/`OAuthHookError`/`TokenHash` types locally in anthropic-oauth (move from `api_client::oauth_hook`, or re-export-then-own) and give `RefreshDriver` an **inherent** `pub async fn refresh(&self, token_hash: TokenHash) -> Result<BearerToken, OAuthHookError>` with the existing body.
- [ ] **Step 2:** repoint `credential_provider.rs:65` to call `self.driver.refresh(token_hash)` (inherent) instead of `<RefreshDriver as api_client::oauth_hook::OAuthRefreshHook>::refresh`. Repoint `client.rs:350` registration off the api-client trait (if the hook-registration indirection is only for api-client's loop, drop it or keep a local trait).
- [ ] **Step 3 (tests):** existing anthropic-oauth tests green; add one asserting `CredentialProvider::load` returns a bearer via the inherent refresh with no api-client import (`grep -L api_client` on the touched files where feasible).
- [ ] **Step 4 (gate):** `cargo test -p anthropic-oauth` green; clippy `-D warnings`. Confirm the only remaining `api_client` references in anthropic-oauth are gone or doc-only (3b removes the dep).
- [ ] **Step 5 (commit):** stage anthropic-oauth paths; commit `refactor(anthropic-oauth): inherent RefreshDriver::refresh; drop api_client::oauth_hook coupling`.

---

### Task 10: host construction — build `DefaultLlmClient` + transport + credentials; feed the adapter

**Files:** Modify `apps/engine-desktop/src/lib.rs` (`:825-846` — replace the `ProviderRegistry`/`ModelRouter` build that backs the adapter), `apps/engine-mobile/src/host.rs` (`:312`). Keep the standalone `api_client::AnthropicProvider` WebSearch `tool_provider` (`engine-desktop:843`, `engine-mobile:318`) — that's repointed in 3b. The `providers::ProviderRegistry` may stay constructed if other code needs it, but the adapter no longer consumes it.

- [ ] **Step 1 (desktop):** construct `ClientConfig` from settings/env (one built-in Anthropic profile from `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL`, or OAuth `HostManaged` when configured) → `DefaultLlmClient::from_config(cfg)?` → `.with_credential_provider(Arc::new(anthropic_oauth::OAuthCredentialProvider::new(...)))` when OAuth. Build the `LlmTransportBridge<ReqwestHttp>` (from Plan 1, platforms/common) as the `Arc<dyn Transport>`. Build `ProviderApiAdapter::new(client, transport, retry_cfg, subscriber, ua, analytics)` and coerce the one `Arc` to both `Arc<dyn OrchestratorApiClient>` and `Arc<dyn agent::SubagentApiClient>` as today.
- [ ] **Step 2 (mobile):** same, with the native mobile transport behind `LlmTransportBridge`. **engine-mobile must build** — verify it does not pull anything desktop-only.
- [ ] **Step 3 (gate):** `cargo build -p engine-desktop -p engine-mobile`; a host smoke/integration test (if one exists) stays green. The default (no `modelProviders`, plain `ANTHROPIC_API_KEY`) path must behave like before for the Anthropic route except for the Task 1–3,7,8 parity corrections.
- [ ] **Step 4 (commit):** stage the two host paths + their `Cargo.toml` (+`llm-client`, +`platforms/common` bridge if not already); commit `feat(apps): construct DefaultLlmClient + transport + credentials; feed ProviderApiAdapter`.

---

### Task 11: final gates

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p agent -p orchestrator -p anthropic-oauth
cargo clippy -p agent -p orchestrator -p anthropic-oauth -p engine-desktop -p engine-mobile --all-targets --no-deps -- -D warnings
cargo build -p engine-desktop
cargo build -p engine-mobile
cargo test --workspace --no-run          # whole tree still compiles (api-client + providers still present)
```
- Frozen check: `git diff main -- lingxi-code/traits lingxi-code/protocol` empty.
- Confirm the **live model path no longer flows through api-client**: `grep -n "providers::\|api_client::AnthropicProvider" orchestrator/src/provider_adapter.rs` returns nothing (the WebSearch `AnthropicProvider` in hosts is fine; it's repointed in 3b).
- Note (expected): api-client + the `providers` crate are still compiled (other dependents remain) — that's 3b's job. The pre-existing unrelated `sidequery` test failure is untouched.

## Final verification
1. Live path = `orchestrator → ProviderApiAdapter → DefaultLlmClient.prepare/execute/execute_stream → Transport`; response headers reach `model/rate_limit.rs` + `model/retry.rs`.
2. Agent + orchestrator seams typed on `llm_client::{LlmRequest,LlmResponse,LlmEvent,LlmError}`; the Plan-2 `model/*` modules are LIVE (no longer dead).
3. Five parity corrections landed and TS-pinned: retry budget 10 (+env), OAuth beta appended-for-subscribers (no clobber), byte-locked User-Agent, mid-stream 529→non-streaming fallback, real subscriber state.
4. anthropic-oauth free of `api_client::oauth_hook`; engine-mobile builds; frozen surfaces untouched.
5. api-client + providers crate still present (deletion is 3b); default Anthropic-key path otherwise behaves as before.

## Risks
- **Streaming metadata (prereqs item 11):** `LlmTransportBridge` may not surface status/headers from `platform_api::HttpTransport::stream_sse`. Connect-phase status works; streaming-route rate-limit tracking is best-effort. An *additive* `stream_sse` metadata return is the preferred fix if needed (frozen `traits` allows additive). Call out in review if a non-additive change is the only option — then it escalates out of 3a.
- **Active main churn:** engine crates move under parallel parity work; rebase onto fresh main before merge; re-run the frozen diff.
- **Parity byte-locks:** retry cadence, the OAuth beta join, the User-Agent template, and the 529/usage-limit copy are asserted byte-for-byte. Any intentional divergence must be flagged in review, not silently adjusted.
- **Tasks 5 + 6 land together:** the seam traits (Task 5) don't pass tests until the adapter body (Task 6) exists. Implement Task 5, then Task 6, and gate tests at the end of 6 (compile-gate after 5).

## Hand-off to 3b / 3c (not in this plan)
- **3b:** delete `api-client/` + the `providers` crate; clear the ~11 remaining dependents (websearch `tool_provider` + `betas::WEB_SEARCH`, `platforms/{common,windows}` `sse::parse_sse_chunks`, `compaction` prompt-too-long, `sidequery`, `tool-api` fixtures, `examples/cli-demo`); drop the 4 dead deps (memory, test-harness, platforms/posix, apps/cli comment) and 3 test-only deps (tasks, commands/core, client-adapter); remove both from workspace members.
- **3c:** `modelProviders` settings section (`Vec<ProviderProfile>`), llm-client registry resolution for multi-provider (OpenAI/Gemini), pricing per profile; the absent-section default = one built-in Anthropic profile.
