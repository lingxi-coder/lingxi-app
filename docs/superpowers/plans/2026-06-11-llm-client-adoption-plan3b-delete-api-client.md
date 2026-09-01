# Plan 3b — Delete api-client + providers (repoint the last consumers)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Fresh implementer per task, two-stage review, SEQUENTIAL edit-agents (worktree hazard memory). TDD where behavior changes; mechanical removals are locked by existing suites.

**Goal:** Remove the `api-client` and `providers` crates (~13.5k LoC) from the workspace. The live model path already runs on llm-client (Plan 3a); this plan repoints the three remaining substantive consumers (sidequery, WebSearch tool provider, compaction's PTL utilities), removes dead/unused references everywhere else, and deletes the crates.

**Branch:** `worktree-llm-client-plan3b` (based on main ff0bc1a6). Cargo from `lingxi-code/`. NEVER `git add -A`. Clippy `-D warnings` (pedantic) on touched crates. `traits/` + `protocol/` frozen (additive only; expect zero diff). Commit trailer:
```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

**Survey ground truth (verified 2026-06-11):**
- Substantive consumers: sidequery (`AnthropicProvider::messages_create_non_stream_with_opts` + `MessageResponse`/`ContentBlockApi` decode + `SideQueryError::Api(ApiError)`), tools/web+tool-api (`BuiltinToolContext.provider: Arc<AnthropicProvider>`, only method used is `build_request(&body) -> HttpRequest`; plus `betas::WEB_SEARCH` + `UsageApi`), compaction (`CompactionError::Api(#[from] ApiError)` + `PROMPT_TOO_LONG_ERROR_MESSAGE` + `prompt_too_long_token_gap`), orchestrator cost_wiring (`providers::ModelSpec::parse`), examples/cli-demo.
- Unused deps (Cargo.toml only, zero src usage): memory, platforms/posix, platforms/windows. Doc/comment-only: anthropic-oauth, client-adapter, apps/cli, platforms/common.
- Dead code: both engines' `let _registry = ProviderRegistry::new(...)`; engine-desktop's re-export of `providers::deprecation::model_deprecation_warning`.
- Deleted with the crates: test-harness `parity_openai_codec.rs` + `parity_gemini_codec.rs` (pin the codecs being deleted; llm-client has its own codec suites).
- Known pre-broken on main: sidequery test `text_response_decodes_text_usage_and_stop_reason` (api-client's method never forwarded `system`; llm-client DOES forward system — Task 1 fixes this for real).
- Dependency direction: orchestrator → compaction (so compaction may NOT dep orchestrator; PTL helpers move INTO compaction, orchestrator re-exports FROM it).

---

### Task 1: sidequery onto llm-client (+ compaction error/PTL re-home)

**Files:** `sidequery/src/provider_side_query.rs` (rebuild), `sidequery/src/side_query.rs` (`SideQueryError::Api` retype), `sidequery/Cargo.toml` (+llm-client, −api-client), `compaction/src/autocompact.rs` + new `compaction/src/prompt_too_long.rs` (move const+fns from `orchestrator/src/model/prompt_too_long.rs`), `compaction/Cargo.toml` (+llm-client, −api-client), `orchestrator/src/model/prompt_too_long.rs` (re-export from compaction; keep the orchestrator-only items), callers of CompactionError::Api.

1. **compaction first (TDD):** create `compaction/src/prompt_too_long.rs` holding `PROMPT_TOO_LONG_ERROR_MESSAGE`, `parse_prompt_too_long_token_counts`, `prompt_too_long_token_gap`, `is_prompt_too_long_body` MOVED from orchestrator (with their tests). `orchestrator/src/model/prompt_too_long.rs` becomes `pub use compaction::prompt_too_long::*;` plus any orchestrator-only leftovers. `CompactionError::Api(#[from] api_client::ApiError)` → `Api(#[from] llm_client::LlmError)`; fix `autocompact.rs:207,221` to use the local module. Update every constructor/match of `CompactionError::Api` across the workspace (grep).
2. **sidequery rebuild (TDD):** `ProviderSideQueryClient` drops `AnthropicProvider`; new internals: `llm_client::DefaultLlmClient` (single anthropic profile from api_key/base_url — reuse `platform_common::llm_config::builtin_anthropic_config` then override base_url/credential, OR a minimal local ClientConfig; pick the smaller) + `Arc<dyn llm_client::Transport>` via `LlmTransportBridge` over the existing `Arc<dyn HttpTransport>`. Request path: build `LlmRequest` (model, system → SystemBlock — NOW actually forwarded, messages, max_tokens, temperature, tools via `agent::convert::to_tool_declarations`-equivalent — check sidequery's tools type; if it already builds raw JSON tool decls, convert minimally) → `client.prepare` → `transport.execute` → `route.codec.decode_response`. Decode path: `LlmResponse.content` `ContentBlock::{Text, ToolCall}` arms (drop arms for the others, same as before); usage map `llm_client::Usage.billable_tokens` → `cost::Usage`. `SideQueryError::Api(#[from] LlmError)`. Constructors `new(api_key, base_url, transport)` / `from_provider` → adjust (from_provider callers: grep).
3. **Fix the pre-broken test:** `text_response_decodes_text_usage_and_stop_reason` now expects `body["system"]` to BE forwarded (llm-client encodes system blocks) — re-pin the assertion to the llm-client anthropic wire shape (`"system":[{"type":"text","text":"system"}]` — verify against llm-client codec) and un-break it. RED→GREEN evidence required.
4. Gates: `cargo test -p sidequery -p compaction -p orchestrator -p memory 2>&1` 0 failed (sidequery fully green incl. the previously broken test); clippy clean; `cargo check --workspace` Finished.
5. Commit: `refactor(sidequery,compaction): rebuild side-query on llm-client; re-home PTL utilities into compaction`.

### Task 2: WebSearch request-builder repoint

**Files:** `tool-api/src/builtin_context.rs` (provider field retype), `tools/web/src/web_search.rs`, new home for the request builder (prefer `platform-common/src/anthropic_request.rs` — platform-common already deps llm-client and is shared by both engines; tool-api deps platform-common? CHECK — if not, put the builder in tool-api itself with zero new deps), `apps/engine-desktop/src/lib.rs:852`, `apps/engine-mobile/src/host.rs:344`, Cargo.tomls (−api-client on tool-api/tools-web; + whatever the builder home needs).

1. READ `api_client::AnthropicProvider::build_request` first — replicate its EXACT output for the WebSearch call (method POST, url join `/v1/messages`, headers: x-api-key, anthropic-version 2023-06-01, content-type, user-agent?, beta handling — quote what it sets) into a small `AnthropicRequestBuilder { api_key: String, base_url: String }` with `pub fn build_request(&self, body: &serde_json::Value) -> platform_api::HttpRequest`. TDD: header/url/body assertions ported from api-client's behavior (+ the WEB_SEARCH beta is applied by the TOOL via its body/headers — check how web_search.rs adds `betas::WEB_SEARCH` and keep byte-identical; the constant moves to the builder module or stays local to tools/web).
2. `BuiltinToolContext.provider: Arc<AnthropicRequestBuilder>`; `UsageApi` in `WebSearchMessageResponse` → a local minimal `#[derive(Deserialize)] struct WebSearchUsage { input_tokens: u64, output_tokens: u64 }` (it only reads those two fields).
3. Hosts construct the builder instead of AnthropicProvider. All web tests re-pointed (MockHttpTransport fixtures unchanged — wire shape must stay byte-identical; that IS the test).
4. Gates: `cargo test -p tool-api -p tool-web -p engine-desktop -p engine-mobile` 0 failed; clippy; workspace check.
5. Commit: `refactor(tool-api,web): local Anthropic request builder for WebSearch; drop api-client from the tool path`.

### Task 3: orchestrator ModelSpec re-home + dead-path removal + deprecation move

**Files:** `orchestrator/src/cost_wiring.rs` (local parse), `apps/engine-desktop/src/lib.rs` (drop `_registry` + move/own `model_deprecation_warning`), `apps/engine-mobile/src/host.rs` (drop `_registry`), `apps/cli` (follow the deprecation re-export), Cargo.tomls (−providers on orchestrator/engines).

1. cost_wiring: replace `providers::ModelSpec::parse` with a local `fn split_profile(model: &str) -> (profile, bare_model)` replicating ModelSpec::parse's semantics for the cost path (READ providers' impl: prefix `profile/model` split with default profile when no slash — port + its relevant tests).
2. READ `providers/src/deprecation.rs::model_deprecation_warning` — move it (+tests) into engine-desktop (or platform-common if mobile needs it; check engine-mobile/cli usage) and repoint the re-export.
3. Delete both `_registry` blocks + their imports (`builtin_profiles, parse_profiles, parse_routing, ProviderRegistry`); the settings parsing they consumed: if `parse_profiles/parse_routing` outputs feed ONLY the dead registry, drop the calls too; if routing feeds `fallback_model` for the adapter, keep that part (READ the host code — Task 10 wired fallback_model from routing; preserve it: move the tiny routing-parse the adapter needs into the host or platform-common).
4. Gates: tests on orchestrator+engines, clippy, workspace check.
5. Commit: `refactor(orchestrator,apps): local model-spec parse + deprecation warning; drop dead ProviderRegistry path`.

### Task 4: delete the crates

**Files:** delete `api-client/` + `providers/` directories; workspace `Cargo.toml` members + default-members (4 lines); remove api-client/providers from ALL remaining Cargo.tomls (memory, platforms/posix, platforms/windows, anthropic-oauth, client-adapter dev-dep, commands/core dev-dep, examples/cli-demo, tasks, test-harness, tool-api...— grep after Tasks 1-3); delete `test-harness/tests/parity_openai_codec.rs` + `parity_gemini_codec.rs`; examples/cli-demo: repoint to llm-client minimally or delete the example (READ it first — if it's an api-client demo with no other value, delete + remove from members; if it demos the engine, repoint); scrub doc-comment mentions (platforms/common http.rs:1,11, anthropic-oauth refresh.rs comments, client-adapter turn.rs:250, apps/cli comments) to reference the llm-client equivalents.

1. Order: Cargo.toml scrub → `git rm -r api-client providers` → members lines → cli-demo decision → parity suite deletion → comment scrub → `cargo check --workspace`.
2. Gates: `cargo test --workspace --no-fail-fast` (expect the api-client env flake GONE with the crate; sidequery previously-broken test now green from Task 1 — so 0 unexpected failures; tool-shell load flake may appear, rerun isolated), full clippy battery `-p` over every touched crate + engines, frozen-crates diff empty, `grep -rn "api_client\|api-client" --include="*.rs" --include="*.toml" . | grep -v target` → zero hits (and same for `providers::`/`"providers"` modulo unrelated words).
3. Commit: `feat!: delete api-client + providers crates — llm-client is the only model client`.

### Task 5: final gates + docs + review

1. Full `cargo test --workspace --no-fail-fast` totals recorded; clippy battery; `cargo build -p engine-desktop -p engine-mobile -p cli`.
2. Update `docs/superpowers/specs/2026-06-10-llm-client-engine-adoption-design.md` (rev2.4: 3b done, 3c remaining: modelProviders settings + PricingCatalog + streaming rate-limit headers).
3. Final whole-plan review subagent; fix loop; then finishing-a-development-branch.
