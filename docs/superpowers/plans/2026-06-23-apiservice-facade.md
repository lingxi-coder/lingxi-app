# ApiService Facade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract `ProviderApiAdapter`'s drive loop into `llm_client::service::ApiService`, reducing the orchestrator/agent consumer-trait impls to thin 1:1 delegations.

**Architecture:** Per spec `docs/superpowers/specs/2026-06-23-apiservice-facade-design.md`. Move the cache formatter (`split_system_blocks_with`) into llm-client, then move the adapter's inherent drive logic into a new `ApiService` (provider-neutral, speaking `protocol::ConversationMessage` + llm-client types). `ProviderApiAdapter` shrinks to a holder of `Arc<ApiService>` whose 3 trait impls delegate. Byte-identical relocation. Builds on Plans A/B (llm-client already owns `model`, `convert`, transport, the deps).

**Tech Stack:** Rust (MSRV 1.82), `llm-client` + `orchestrator` + `agent`.

## Global Constraints

- **Byte-identical behavior** — pure relocation; the request bytes (thinking/effort/temp/betas/cache/system-block splitting) and stream decoding must be unchanged. Parity vs claude-code preserved.
- **No dependency cycle:** `llm-client` must NOT depend on `orchestrator`/`agent`/`compaction`. The 3 consumer-trait DEFINITIONS stay in orchestrator/agent. `ApiService` references only `llm_client::*` + `protocol` + `traits` + `telemetry` — no orchestrator-internal paths in the drive path.
- **Build env (guarded-ff):** prefix every cargo command with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
- **Green at every task:** `cargo build --workspace --tests` passes before each commit.
- **Commit per task.** Relocation — no new failing-test-first cycle; the safety net is "moved tests still pass + workspace compiles."
- **macOS:** use `perl -pi -e`, not BSD `sed`, for any `\b` path rewrite.

---

## File Structure
- **Create** `lingxi-code/llm-client/src/prompt_format.rs` — moved `split_system_blocks_with` + `SplitOptions` + `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` (+ the `SECTION_SEP` separator it needs).
- **Create** `lingxi-code/llm-client/src/service.rs` — `ApiService` (the drive loop).
- **Modify** `lingxi-code/llm-client/src/lib.rs` — `pub mod prompt_format; pub mod service;` (alphabetical) + re-exports.
- **Modify** `lingxi-code/orchestrator/src/prompt/mod.rs` — re-export the moved formatter from `llm_client::prompt_format`; keep `locked_templates` (content).
- **Modify** `lingxi-code/orchestrator/src/provider_adapter.rs` — shrink `ProviderApiAdapter` to an `Arc<ApiService>` holder + thin trait impls; move the `#[cfg(test)] mod tests` per Task 4.
- **Modify** `lingxi-code/apps/engine-desktop/src/lib.rs`, `apps/engine-mobile/src/host.rs` — construct `ApiService`, wrap in the thin `ProviderApiAdapter`.

---

## Task 1: Move the cache formatter → `llm_client::prompt_format`

**Files:**
- Create: `lingxi-code/llm-client/src/prompt_format.rs`; Modify: `llm-client/src/lib.rs`, `orchestrator/src/prompt/mod.rs`

**Interfaces:**
- Produces: `llm_client::prompt_format::{split_system_blocks_with, SplitOptions, SYSTEM_PROMPT_DYNAMIC_BOUNDARY}` — `split_system_blocks_with(s: &str, enable_caching: bool, opts: SplitOptions) -> Vec<llm_client::SystemBlock>`.

- [ ] **Step 1: Identify what the formatter needs**

Read `orchestrator/src/prompt/mod.rs` lines 214-345 (`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` const, `SplitOptions`, `split_system_blocks_with`). It uses `SECTION_SEP` (a `"\n\n"` separator) from `locked_templates` to build the boundary marker. Note every const/type it references.

- [ ] **Step 2: Create the module**

Create `lingxi-code/llm-client/src/prompt_format.rs` with `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`, `SplitOptions`, `split_system_blocks_with`, AND a private `const SECTION_SEP: &str = "\n\n";` (move the separator value — it's pure formatting, NOT prompt content; do NOT move `HEADER` or the rest of `locked_templates`, which are content). Copy the fn/struct/const bodies verbatim; fix paths so it references only `crate::{SystemBlock, CacheControl, CacheScope}` (llm-client types) + its own consts. Move the formatter's `#[cfg(test)]` unit tests with it (rewrite any `locked_templates::SECTION_SEP` in those tests to the local const). Add `pub mod prompt_format;` to `llm-client/src/lib.rs` (alphabetical position).

- [ ] **Step 3: Re-export from orchestrator + fix the call site**

In `orchestrator/src/prompt/mod.rs`, replace the moved definitions with `pub use llm_client::prompt_format::{split_system_blocks_with, SplitOptions, SYSTEM_PROMPT_DYNAMIC_BOUNDARY};` so in-orchestrator callers keep resolving `crate::prompt::…`. (`provider_adapter.rs:~722` calls `crate::prompt::split_system_blocks_with` — it now resolves via the re-export, no edit needed.)

- [ ] **Step 4: Build + test**
```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p orchestrator
```
Expected: PASS (the formatter's moved tests run under llm-client; orchestrator resolves the re-export; the `split_system_blocks_with` byte output is unchanged).

- [ ] **Step 5: Commit**
```bash
git add lingxi-code/llm-client lingxi-code/orchestrator/src/prompt/mod.rs
git commit -m "refactor(llm-client): own the system-block cache formatter (split_system_blocks_with)"
```

---

## Task 2: Extract `ApiService` (struct + builders + ALL inherent drive logic)

The big task. The inherent `impl ProviderApiAdapter` block (`provider_adapter.rs:341-1824`) IS the drive loop. Move it into `llm_client::service::ApiService` as inherent methods. Do it in the four sub-steps below, building green between each. The 3 trait impls stay in orchestrator (Task 3).

**Files:**
- Create: `lingxi-code/llm-client/src/service.rs`; Modify: `llm-client/src/lib.rs`, `orchestrator/src/provider_adapter.rs`

**Interfaces:**
- Produces: `llm_client::service::ApiService` with: the struct (fields from `provider_adapter.rs:128-175` — `client`, `transport`, `subscriber`, `subscription`, `forced_tool_choice`, `thinking`, `request_metadata`, `cache_editing_inputs`, `ua`, `version`, `analytics`, fallback config); constructors `new`/`new_with_estimator`/`new_with_routing` (:341/368/414); builders `with_subscription`/`with_forced_tool_choice`/`with_thinking`/`with_request_metadata`/`with_cache_editing_inputs` (:474/518/526/534/591); and inherent drive methods:
  - `messages_create(model, system: Option<&str>, messages: Vec<protocol::ConversationMessage>, tools) -> Result<LlmResponse, LlmError>`
  - `messages_create_with_opts(…, opts) -> Result<LlmResponse, LlmError>`
  - `stream(…) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>` + `stream_forced(…)`
  - (these wrap the existing `build_request` + `drive_non_stream`/`drive_non_stream_seeded_with_chain`/`drive_stream` + all the private helpers at :556-1824)

- [ ] **Step 2a: Skeleton — struct + state + constructors + builders**

Create `lingxi-code/llm-client/src/service.rs`. Move the `ProviderApiAdapter` struct def (`:128-175`) → `pub struct ApiService` (rename; fields verbatim). Move `new`/`new_with_estimator`/`new_with_routing` + the 5 `with_*` builders + `build_api_metadata_user_id`. Fix paths: `crate::model::…` → `crate::model::…` (already correct inside llm-client), `crate::prompt::split_system_blocks_with` → `crate::prompt_format::split_system_blocks_with`, `llm_client::X` → `crate::X`; references to `crate::conversation`/orchestrator types must NOT appear (the drive path has none — verify). Add `pub mod service; pub use service::ApiService;` to `lib.rs`. Leave `ProviderApiAdapter` intact in orchestrator for now. Build `-p llm-client`. Commit (`feat(llm-client): ApiService struct + constructors`).

- [ ] **Step 2b: Move the request-build + cache/header/rate-limit helpers**

Move into `ApiService` (verbatim, fixing paths as in 2a): `effective_subscriber`, `should_use_global_cache_scope`, `should_1h_cache_ttl`, `should_use_cache_editing`, `build_request` (:684), `beta_context`, `inject_headers`, `inject_stream_headers`, `resolve_retry_after`, `error_kind`, `last_rate_limit_info`, `record_rate_limit_from_headers`, `is_pro_or_enterprise`, `record_rate_limit_from_429`, `clear_pending_429`, `promote_pending_429`, `status_of`. Build `-p llm-client`. Commit.

- [ ] **Step 2c: Move the non-streaming drive + expose `messages_create`**

Move `drive_non_stream` (:1251) + `drive_non_stream_seeded_with_chain` (:1274). Add inherent `ApiService::messages_create` + `messages_create_with_opts` whose bodies are the drive logic currently in `OrchestratorApiClient::messages_create`/`_with_opts` (`:1825+`) — provider-neutral (take `Vec<protocol::ConversationMessage>`, return `LlmResponse`). Build `-p llm-client`. Commit.

- [ ] **Step 2d: Move the streaming drive + WS preconnect + expose `stream`**

Move `drive_stream` (:1550), `preconnect_responses_websocket` (:484), `spawn_responses_websocket_preconnect` (:501). Add inherent `ApiService::stream` + `stream_forced` whose bodies are the drive logic from `StreamingApiClient::stream` (`:2153+`) and the forced variant (the `SubagentApiClient` stream-forced path at `:2100+`). Build `-p llm-client -p orchestrator` (orchestrator's `ProviderApiAdapter` still compiles unchanged — it still has its own copy until Task 3). Commit.

---

## Task 3: Thin `ProviderApiAdapter` to delegations + re-point composition roots

**Files:**
- Modify: `orchestrator/src/provider_adapter.rs`, `apps/engine-desktop/src/lib.rs`, `apps/engine-mobile/src/host.rs`

**Interfaces:**
- Consumes: `llm_client::ApiService` (Task 2).
- Produces: `ProviderApiAdapter { service: Arc<llm_client::ApiService> }` with `new(Arc<ApiService>) -> Self`; the 3 trait impls delegate 1:1.

- [ ] **Step 1: Replace the inherent block + trait impls with thin delegations**

In `provider_adapter.rs`, delete the now-moved inherent `impl ProviderApiAdapter` block (`:341-1824`) and replace the struct with `pub struct ProviderApiAdapter { service: std::sync::Arc<llm_client::ApiService> }` + `pub fn new(service: Arc<llm_client::ApiService>) -> Self`. Rewrite the 3 trait impls to delegate:
```rust
#[async_trait::async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(&self, model: &str, system: Option<&str>, messages: Vec<protocol::ConversationMessage>, tools: Vec<serde_json::Value>) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        self.service.messages_create(model, system, messages, tools).await
    }
    async fn messages_create_with_opts(&self, /* … */) -> /* … */ { self.service.messages_create_with_opts(/* … */).await }
    async fn messages_create_with_fallback(&self, /* … */) -> /* … */ { /* compose: self.service.messages_create(model_a) → on fallback, crate::model::fallback → self.service.messages_create(model_b) */ }
}
#[async_trait::async_trait]
impl StreamingApiClient for ProviderApiAdapter { /* delegate stream */ }
#[async_trait::async_trait]
impl agent::SubagentApiClient for ProviderApiAdapter { /* delegate messages_create / stream / stream_forced */ }
```
Keep `last_rate_limit_info` reachable (delegate to `self.service.last_rate_limit_info()`). Move the `#[cfg(test)] mod tests` handling to Task 4.

- [ ] **Step 2: Re-point composition roots**

In `apps/engine-desktop/src/lib.rs` (the `ProviderApiAdapter::new_with_routing(...).with_*(...)` chain, ~line 2091 region) build the service then wrap: `let service = Arc::new(llm_client::ApiService::new_with_routing(...).with_request_metadata(...)…); let adapter = ProviderApiAdapter::new(service);`. Same in `apps/engine-mobile/src/host.rs`. Keep the exact same `with_*` arguments.

- [ ] **Step 3: Build + test**
```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p orchestrator -p agent -p engine-desktop
```
(The provider_adapter `#[cfg(test)] mod tests` may not compile yet if it referenced moved inherent methods — Task 4 resolves test placement. If a test references a now-private/moved method, temporarily `#[cfg(test)]`-gate or move it in Task 4; do not delete coverage.)

- [ ] **Step 4: Commit**
```bash
git add lingxi-code/orchestrator/src/provider_adapter.rs lingxi-code/apps/engine-desktop lingxi-code/apps/engine-mobile
git commit -m "refactor(orchestrator): ProviderApiAdapter delegates to llm_client::ApiService"
```

---

## Task 4: Test split + full workspace green

**Files:** Modify: `orchestrator/src/provider_adapter.rs` (its `mod tests`), Create: `lingxi-code/llm-client/tests/api_service.rs` (or `service.rs` unit tests).

- [ ] **Step 1: Classify the provider_adapter tests**

For each test in `provider_adapter.rs`'s `mod tests` (`:2440+`): if it exercises drive behavior with **plain fixtures** (FakeTransport + plain strings/messages) → move it to an `ApiService` unit test (in `llm-client`, using `llm_client::ApiService` directly). If it depends on **orchestrator-domain helpers** (`locked_templates::HEADER`, `cost_wiring`, `crate::error`) → keep it in orchestrator as a thin-adapter integration test (construct `ProviderApiAdapter::new(Arc::new(ApiService::…))`). Preserve every assertion — no coverage lost.

- [ ] **Step 2: Move the FakeTransport + helpers as needed**

The test `FakeTransport`/`FakeStreamTransport`/`ScriptedStreamTransport` impls (`impl Transport for …`) move to wherever the tests using them land (llm-client test module and/or orchestrator). Duplicate only if both sides need them.

- [ ] **Step 3: Full workspace gate**
```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --no-fail-fast
```
Expected: PASS (known-flaky `powershell` passes in isolation — re-run `-p tool-shell` to confirm if it trips).

- [ ] **Step 4: Byte-parity spot check**

`wc -l lingxi-code/orchestrator/src/provider_adapter.rs` — should drop from ~6.3k to a few hundred (delegations + the integration tests). Confirm a request/stream byte-capture test (moved to ApiService) passes.

- [ ] **Step 5: Commit**
```bash
git add lingxi-code/orchestrator lingxi-code/llm-client
git commit -m "test(llm-client): ApiService unit tests; orchestrator keeps thin-adapter integration tests; facade complete"
```

---

## Self-Review

**Spec coverage:** §4.1 formatter move → Task 1. §4.2 ApiService → Task 2. §4.3 thin impls + composition → Task 3. §4.4 test placement → Task 4. §6 phasing → Tasks 1-4 (Phases 2-4 = Task 2 sub-steps a-d). Cycle analysis (§5) enforced by the no-orchestrator-path constraint.

**Placeholder scan:** The trait-impl bodies in Task 3 are shown as the delegation pattern with `/* … */` for the exact arg lists — the implementer fills them from the existing trait signatures (the methods + signatures are named explicitly). The ApiService method bodies in Task 2 are "the existing drive logic from provider_adapter.rs:NNNN" (exact source lines named) — relocation, not invention.

**Type consistency:** `ApiService` method names (`messages_create`/`_with_opts`/`stream`/`stream_forced`) match the delegations in Task 3 and the traits they back. `new_with_routing` + the 5 `with_*` names match `provider_adapter.rs`.

**Risk note:** Task 2 is large (≈1480-line inherent block); the 4 sub-steps each build green. Task 3 is the integration. If the inherent-vs-trait separation snags mid-extraction, report BLOCKED rather than forcing it.
