# Plan 3b — Delete `api-client` + `providers` crates

> Branch `llm-client-3a-resume`. Follows Plan 3a (done). Derived from a workspace-wide
> consumer analysis (Workflow `wpborbc2g`, 2026-06-14). Execute top-to-bottom; the
> workspace must stay green at every step. `api-client` only finally disappears after Step G.

## Reality check

14 crates declared `api-client`, 4 declared `providers`. Most is removable cheaply.
**Nothing needs net-new llm-client surface** — every migration is "copy locally" or
"delete dead/test" **except sidequery (Step G)**, a real rebuild and the only high-risk
item. `llm_client::providers` is a DIFFERENT module (llm-client's own) — never touch it.

## §2 — Safe dead-dep sweep ✅ DONE (commits 548e1088, bb35903f)

Removed `api-client` (zero code refs) from: `agent`, `anthropic-oauth`, `tasks`,
`commands/core` (dev), `memory`, `platforms/posix`. Dependent set 14 → 8.

## Step A — `compaction` (trivial, low risk)
- New `compaction/src/prompt_too_long.rs`: copy `PROMPT_TOO_LONG_ERROR_MESSAGE` +
  `prompt_too_long_token_gap` (+ `parse_prompt_too_long_token_counts` + regex)
  **byte-for-byte from `orchestrator/src/model/prompt_too_long.rs`** (canonical 1:1
  port; const must stay `"Prompt is too long"` — `autocompact` `starts_with` matches
  orchestrator's emitted sentinel). Add `regex = "1"` to `compaction/Cargo.toml`.
- Repoint `autocompact.rs:207,:221` → `crate::prompt_too_long::…`.
- Delete the DEAD `use api_client::ApiError;` (`:21`) + `CompactionError::Api(#[from] ApiError)`
  variant (`:48`) — never constructed/matched anywhere EXCEPT orchestrator's error test
  (fixed in Step B — do A+B's error-test fix together so the workspace stays green).
- Remove `api-client` from `compaction/Cargo.toml`.

## Step B — `orchestrator` off `api-client` + `providers` (medium)
- Inline `ModelSpec::parse` (~15 lines, `providers/src/model_spec.rs:13-43`) as a private
  helper in `orchestrator/src/cost_wiring.rs`; repoint `:9/:73/:84` (cost_wiring tests
  `:173-223` lock it). → drops `providers` from `orchestrator/Cargo.toml`. NO llm-client equiv.
- Fix `orchestrator/src/error.rs:125` test: Step A deleted `CompactionError::Api`, so build
  the `CompactionError` via a non-Api variant (test only asserts Display contains "compaction").
  → drops `api-client` from `orchestrator/Cargo.toml`.
- Reword dangling rustdoc intra-links: `config.rs:36/:42`, `error.rs:30`.

## Step C — `platforms/common` + `platforms/windows` (low; independent of A/B)
- Lift `parse_sse_chunks` (~32 LOC, `api-client/src/sse.rs:14-46`) into a private module in
  `platforms/common/src/http.rs`; point `platforms/windows/src/http.rs` at it. Returns
  `Vec<protocol::SseEvent>` preserving `event_type`/`id`. **Do NOT use
  `llm_client::SseFrameSplitter`** (drops `event:`/`id:` → breaks `stream_sse` + smoke tests).
- Remove `api-client` from both Cargo.tomls.

## Step D — the `AnthropicProvider` web-search unit (medium; tool-api → tools/web → engine apps)
This is the api-client `AnthropicProvider` blocker. **Do NOT adopt llm-client's codec** —
`AnthropicMessagesCodec` yields a `ProviderRequest` (no timeout/x-api-key, can't encode the
`web_search_20250305` tool block, and `decode_response` DROPS `server_tool_use`/
`web_search_tool_result` — the exact blocks WebSearch parses).
- **tool-api**: replace `BuiltinToolContext.provider: Arc<AnthropicProvider>`
  (`builtin_context.rs:80`, import `:12`) with a `{ api_key, base_url }` carrier (or drop +
  pass into WebSearch). Update fixtures `test_support.rs:538,:589`. → drops `api-client` dep.
- **tools/web** (`web_search.rs`): (1) copy `const WEB_SEARCH_BETA = "web-search-2025-03-05"`
  locally; (2) replace `UsageApi` with a 2-field local `#[derive(Default,Deserialize)]`
  (input_tokens/output_tokens only); (3) replace `ContentBlockApi` with a local deserialize
  (needs the dropped result blocks); (4) inline `fn anthropic_messages_request(api_key,base_url,&Value)
  -> protocol::HttpRequest` byte-for-byte from `api-client/src/anthropic.rs:148-161`
  (x-api-key, anthropic-version 2023-06-01, content-type, accept, 120s, `{base_url}/v1/messages`);
  (5) fix test ctx wiring `:821/:843`. The 13 wire-asserting tests catch drift. → drops `api-client`.
- **engine apps + cli-demo**: swap the 3 `AnthropicProvider::new` sites to the carrier
  (`engine-desktop/src/lib.rs:942-945`, `engine-mobile/src/host.rs:415-418/:475`); delete the
  dead `let _provider` + import in `examples/cli-demo/src/main.rs:17,:52` and drop its dep.

## Step E — engine-apps `providers` dead-code + `model_deprecation_warning` (low–medium; after D)
- Delete the DEAD `_registry` block in both engine apps (`engine-desktop/src/lib.rs:899-915`,
  `engine-mobile/src/host.rs:357-374`) + the `providers` import (multi-provider routing is 3c;
  no live behavior lost — `cfg.provider_profiles`/`cfg.routing` become unread).
- Copy `providers/src/deprecation.rs` (self-contained, ~10KB) into e.g.
  `apps/engine-desktop/src/model_deprecation.rs`; keep the re-export so
  `apps/cli/src/lib.rs:317,:354 → engine_desktop::model_deprecation_warning` stays identical.
- Drop `providers` from both engine Cargo.tomls.

## Step F — `test-harness` test migrations (after B/D)
- DELETE `parity_openai_codec.rs` + `parity_gemini_codec.rs` (test the providers codecs being
  deleted; exact subsets of `llm-client/tests/{openai,gemini}_codec_test.rs` — zero coverage lost).
  → drops the `providers` dev-dep.
- DELETE the `anthropic_provider_against_mock_http` test in `e2e_single_turn.rs:110-112` (keep
  the 1st test); llm-client `transport_{execute,stream}_test.rs` cover the transport path.
- `parity_messages_create.rs`, `parity_betas.rs`, `parity_web_tools.rs`: real claude-code parity
  locks — REWRITE each as a local test-harness lock using the Step-D helpers (WEB_SEARCH_BETA +
  local `anthropic_messages_request`), do NOT delete. → drops `api-client` dev-dep.

## Step G — `sidequery` (HIGH risk; its own session; **DESIGN DECISION below**)
Rebuild `ProviderSideQueryClient` on `DefaultLlmClient`:
- Build from a `ClientConfig` passed by the engine (constructor + call site
  `engine-desktop/src/lib.rs:1426-1431` change); map `SideQueryRequest`→`LlmRequest`; add a
  `Arc<dyn platform_api::HttpTransport>`→`llm_client::Transport` adapter (copy the 3a bridge);
  repoint `MessageResponse`→`LlmResponse`, `ContentBlockApi`→`ContentBlock` (`ToolUse`→`ToolCall`);
  `SideQueryError::Api(#[from] ApiError)`→`Api(#[from] LlmError)` (LlmError is Clone → satisfies
  `SideQueryError: Clone`; downstream `memory/selector`, `compaction/autocompact`,
  `tools/web/web_fetch` only match the outer variant).
- Rewrite the test module (`provider_side_query.rs:200-479`). → drops `api-client`.

**DESIGN DECISION (sidequery retry):** `DefaultLlmClient::execute` is single-shot; orchestrator's
retry driver is unreachable (cycle orchestrator→sidequery). Options:
- **(b) RECOMMENDED** — reconstruct a small local retry loop in sidequery over `DefaultLlmClient`
  + `llm_client::RetryPolicy::classify_error/classify_response` (stateless classifier; you supply
  the loop + sleep), preserving the current 3-attempt / 429 / 401-refresh behavior for the
  memory-selector / web-fetch / autocompact callers.
- (a) accept single-shot — a real behavior regression for those 3 callers; only with explicit approval.
- Cost/telemetry (`with_cost_tracker`/`with_bus`) has no `DefaultLlmClient` equivalent — reconstruct
  at the caller or consciously drop.

## Step H — delete the crates
- `grep -rl 'api-client = \|providers = ' --include=Cargo.toml .` must be empty.
- `git rm -r providers/ api-client/`; remove from root `Cargo.toml` members/default-members.
- Final `cargo build --workspace && cargo test --workspace` green.

## Effort
~8 increments, multi-session. Session 1 = §2 (done) + A + C. Session 2 = B + D + E + F.
Session 3 = G + H. Sidequery (G) is the gate on final deletion; everything else lands green
while it's in flight.
