# ApiService Consolidation Implementation Plan (Plan B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the provider-protocol policy + the LLM drive loop into `llm-client`, behind a new `llm_client::service::ApiService` facade; reduce the orchestrator/agent consumer-trait impls to thin delegations.

**Architecture:** The self-contained `model/` policy modules and `agent::convert` move down into `llm-client`. `ProviderApiAdapter`'s ~6.3k lines of drive logic (build → execute → decode → retry/rate-limit) become `llm_client::service::ApiService`, speaking `protocol::ConversationMessage` + llm-client types. The three consumer traits (`OrchestratorApiClient`, `StreamingApiClient`, `SubagentApiClient`) **stay** in orchestrator/agent; `ProviderApiAdapter` shrinks to a holder of `Arc<ApiService>` whose trait impls delegate 1:1. Byte-identical relocation. Builds on Plan A (llm-client already has `traits`/`protocol`/`async-trait`/`futures-util` and owns the transport bridge).

**Tech Stack:** Rust (MSRV 1.82), the existing `llm-client` + `orchestrator` + `agent` crates.

> **STATUS (2026-06-23):** Tasks 1-2 LANDED on `main` (`eeb1eaec`, `b40a4069`) — all 10 `model/`
> policy modules + `agent::convert` moved into `llm-client` (`prompt_too_long` promoted out of
> `compaction` to break a cycle; the "3 stayers" split was artificial — all 10 are pure). Full
> workspace green (8565 tests). **Tasks 3-4 (the `ApiService` facade) are DEFERRED** to a dedicated
> design pass — the drive loop is entangled with orchestrator-domain `crate::prompt` (system-prompt
> splitting / cache-breakpoint structure) + `crate::cost_wiring`, so the extraction is a
> *parameterizing refactor*, not the byte-identical relocation Task 3 below assumes, and needs its own spec.

## Global Constraints

- **MSRV 1.82** — no crate may raise the floor.
- **Byte-identical behavior.** Pure relocation; no logic changes. Parity vs claude-code v2.1.185 preserved. The request bytes (thinking/effort/temperature/betas/cache) and stream decoding must be unchanged.
- **No dependency cycle:** `llm-client` may depend on `traits`/`protocol`/`telemetry` (all verified cycle-free). The consumer traits' DEFINITIONS stay in orchestrator/agent (do NOT move them — that was explicitly declined). `ApiService` speaks `protocol::ConversationMessage` + `llm_client` types, never orchestrator-internal types.
- **What MOVES into llm-client:** `model/{betas,count_tokens,rate_limit,retry,telemetry,thinking,user_agent}`, `agent::convert`, and `ProviderApiAdapter`'s drive substance.
- **What STAYS in orchestrator:** `model/{overflow,prompt_too_long,fallback}` (conversation-domain), the 3 consumer-trait definitions, and a thin `ProviderApiAdapter` holding `Arc<ApiService>`.
- **Build env (guarded-ff):** prefix every cargo command with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
- **Green at every task:** `cargo build --workspace --tests` passes before each commit; run owning-crate tests too.
- **Commit per task.** Relocation tasks have no new failing-test-first cycle; the safety net is "the moved tests still pass + the workspace compiles."

---

## File Structure

- **Modify** `lingxi-code/llm-client/Cargo.toml` — add `telemetry` dep.
- **Create** `lingxi-code/llm-client/src/model/mod.rs` + move 7 modules in (`betas.rs`, `count_tokens.rs`, `rate_limit.rs`, `retry.rs`, `telemetry.rs`, `thinking.rs`, `user_agent.rs`).
- **Create** `lingxi-code/llm-client/src/convert.rs` (moved from `agent/src/convert.rs`).
- **Create** `lingxi-code/llm-client/src/service.rs` (+ optional submodules) — `ApiService`.
- **Modify** `lingxi-code/llm-client/src/lib.rs` — declare `model`, `convert`, `service`; re-export.
- **Modify** `lingxi-code/orchestrator/src/model/mod.rs` — keep only `overflow`, `prompt_too_long`, `fallback`; re-export the moved ones from `llm_client::model` for in-orchestrator callers (back-compat).
- **Modify** `lingxi-code/orchestrator/src/provider_adapter.rs` — shrink `ProviderApiAdapter` to an `Arc<ApiService>` holder + thin trait impls.
- **Modify** `lingxi-code/orchestrator/src/conversation.rs`, `test_support.rs`, `error.rs` — re-point `model::{rate_limit,count_tokens}` refs to `llm_client::model::…`.
- **Delete** `lingxi-code/agent/src/convert.rs` (moved); **modify** `agent/src/lib.rs`.
- **Modify** `lingxi-code/apps/engine-desktop/src/lib.rs`, `apps/engine-mobile/src/host.rs` — construct `ApiService` + wrap in the thin `ProviderApiAdapter`.

---

## Task 1: Move the 7 protocol-policy modules → `llm_client::model`

**Files:**
- Modify: `lingxi-code/llm-client/Cargo.toml` (add `telemetry`), `lingxi-code/llm-client/src/lib.rs`
- Move: `orchestrator/src/model/{betas,count_tokens,rate_limit,retry,telemetry,thinking,user_agent}.rs` → `llm-client/src/model/`
- Create: `lingxi-code/llm-client/src/model/mod.rs`
- Modify: `orchestrator/src/model/mod.rs`, `orchestrator/src/provider_adapter.rs`, `orchestrator/src/conversation.rs`, `orchestrator/src/test_support.rs`, `orchestrator/src/error.rs`

**Interfaces:**
- Produces: `llm_client::model::{betas, count_tokens, rate_limit, retry, telemetry, thinking, user_agent}` with all their current public items (e.g. `rate_limit::{RateLimitInfo, RawUtilization, RawWindow, rate_limit_error_message}`, `count_tokens::APPROX_CHARS_PER_TOKEN`, `thinking::ThinkingConfig`, `betas::{BetaContext, Endpoint, Provider, apply_beta_header_with_auth}`, `user_agent::{user_agent, UserAgentEnv}`).

- [ ] **Step 1: Add the `telemetry` dep**

In `lingxi-code/llm-client/Cargo.toml` `[dependencies]`, add:
```toml
telemetry = { path = "../telemetry" }
```

- [ ] **Step 2: Move the 7 module files**

```bash
cd lingxi-code
mkdir -p llm-client/src/model
for m in betas count_tokens rate_limit retry telemetry thinking user_agent; do
  git mv orchestrator/src/model/$m.rs llm-client/src/model/$m.rs
done
```

- [ ] **Step 3: Create the model module root**

Create `lingxi-code/llm-client/src/model/mod.rs`:
```rust
//! Provider-protocol policy moved from `orchestrator::model` — betas, thinking,
//! retry/rate-limit, token counting, user-agent, and api telemetry. These are
//! the "how to talk to the provider" concerns; conversation-domain modules
//! (overflow, prompt_too_long, fallback) stay in `orchestrator::model`.

pub mod betas;
pub mod count_tokens;
pub mod rate_limit;
pub mod retry;
pub mod telemetry;
pub mod thinking;
pub mod user_agent;
```

In `lingxi-code/llm-client/src/lib.rs`, add `pub mod model;` near the other module declarations.

- [ ] **Step 4: Fix intra-module references in the moved files**

The moved modules referenced each other via `crate::model::…` when they lived in orchestrator — that path is identical inside llm-client (`crate::model::…`), so most need no change. Verify and fix any that referenced orchestrator-only paths:
```bash
grep -rnE 'crate::(conversation|settings|session|provider_adapter|state)' lingxi-code/llm-client/src/model/ || echo "clean — no orchestrator-internal refs (expected)"
```
If any hit appears, STOP and report — these modules were verified self-contained, so a hit means the audit missed something.

- [ ] **Step 5: Re-point orchestrator's in-crate consumers**

`orchestrator/src/model/mod.rs` — replace the 7 moved `pub mod` lines with re-exports so existing in-orchestrator `crate::model::…` callers keep working; keep the 3 stayers:
```rust
// moved to llm-client; re-export for in-orchestrator callers
pub use llm_client::model::{betas, count_tokens, rate_limit, retry, telemetry, thinking, user_agent};

pub mod fallback;
pub mod overflow;
pub mod prompt_too_long;
```
This keeps `provider_adapter.rs`'s `crate::model::betas` / `crate::model::thinking` / etc. resolving (now via re-export) — no churn there yet. Verify `conversation.rs`, `test_support.rs`, `error.rs` still resolve their `crate::model::rate_limit::…` / `crate::model::count_tokens::…` references through the re-export.

- [ ] **Step 6: Build + test**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p orchestrator
```
Expected: PASS (the moved unit tests run under llm-client now; orchestrator resolves the re-exports).

- [ ] **Step 7: Commit**

```bash
git add lingxi-code/llm-client lingxi-code/orchestrator/src/model/mod.rs
git commit -m "refactor(llm-client): own model policy (betas/thinking/retry/rate_limit/user_agent/count_tokens/telemetry)"
```

---

## Task 2: Move `agent::convert` → `llm_client::convert`

**Files:**
- Move: `agent/src/convert.rs` → `llm-client/src/convert.rs`
- Modify: `llm-client/src/lib.rs`, `agent/src/lib.rs`, `orchestrator/src/provider_adapter.rs`

**Interfaces:**
- Produces: `llm_client::convert::{to_llm_messages, normalize_messages_for_api, ensure_tool_result_pairing, to_tool_declarations}`.

- [ ] **Step 1: Confirm the sole consumer**

```bash
grep -rnE 'convert::(to_llm_messages|normalize_messages_for_api|ensure_tool_result_pairing|to_tool_declarations)|use (crate|agent)::convert' lingxi-code --include='*.rs' | grep -v '/target/' | grep -vE '/convert\.rs'
```
Expected: only `orchestrator/src/provider_adapter.rs` (the import `use agent::convert::{…}`). If other consumers appear, list them — each re-points to `llm_client::convert` in this task.

- [ ] **Step 2: Move the file**

```bash
cd lingxi-code
git mv agent/src/convert.rs llm-client/src/convert.rs
```

- [ ] **Step 3: Re-home + fix imports**

In `lingxi-code/llm-client/src/lib.rs` add `pub mod convert;`. Remove `pub mod convert;` (and any `pub use convert::…`) from `agent/src/lib.rs`. In `llm-client/src/convert.rs`, fix imports: anything it referenced as `crate::…` (agent-internal) becomes the real source (`protocol::…`, `llm_client` types become `crate::…`); it must not reference `agent::…`. If `convert.rs` genuinely needs an agent-internal type, STOP and report (it should only touch `protocol` + llm-client message types).

- [ ] **Step 4: Re-point provider_adapter**

In `orchestrator/src/provider_adapter.rs`, change `use agent::convert::{ensure_tool_result_pairing, normalize_messages_for_api, to_llm_messages, to_tool_declarations};` → `use llm_client::convert::{…};` (same names).

- [ ] **Step 5: Build + test**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p agent -p orchestrator
```
Expected: PASS (convert's moved tests run under llm-client).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/llm-client lingxi-code/agent lingxi-code/orchestrator/src/provider_adapter.rs
git commit -m "refactor(llm-client): own message/tool conversion (moved from agent::convert)"
```

---

## Task 3: Extract `ApiService` (the drive-loop facade)

This is the largest task. Do it in the four sub-steps below, building green between each. The principle: the **inherent** drive logic of `ProviderApiAdapter` (everything that builds a request, calls the transport, decodes, retries) moves into `llm_client::service::ApiService`, speaking `protocol::ConversationMessage` + llm-client types. The **three trait impls stay in orchestrator** (their traits live there + in agent — moving the impls would cycle).

**Files:**
- Create: `lingxi-code/llm-client/src/service.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`, `orchestrator/src/provider_adapter.rs`

**Interfaces:**
- Produces: `llm_client::service::ApiService` with:
  - `ApiService::new_with_routing(client, transport, subscriber, ua, version, analytics, fallback_model, cost_estimator, fallback_overrides, max_retries, backoff_ms) -> Self` (same params as today's `ProviderApiAdapter::new_with_routing` at provider_adapter.rs:404)
  - builders `with_subscription`, `with_forced_tool_choice`, `with_thinking`, `with_request_metadata` (move verbatim from provider_adapter.rs:463/507/515/523)
  - `async fn messages_create(&self, model, system, messages: Vec<protocol::ConversationMessage>, tools) -> Result<LlmResponse, LlmError>`
  - `async fn messages_create_with_opts(&self, …, opts) -> Result<LlmResponse, LlmError>`
  - `async fn stream(&self, …) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>`
  - `async fn stream_forced(&self, …) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>`
  - (exact signatures: copy the bodies of the 3 trait impls' methods at provider_adapter.rs:1808/2079/2132 as inherent `ApiService` methods, replacing `self.<field>` access with the moved struct's fields.)
- Consumes: `crate::model::*`, `crate::convert::*`, `crate::Transport`, `protocol::*`, `traits::subscription::SharedSubscription`, `telemetry::AnalyticsBus`.

- [ ] **Step 3a: Create `ApiService` with the struct state + constructors (no drive methods yet)**

Create `lingxi-code/llm-client/src/service.rs`. Move the `ProviderApiAdapter` **struct definition** (provider_adapter.rs:128, all fields) into it, renamed `ApiService`. Move `new_with_routing` (:404) and the four `with_*` builders (:463/507/515/523) verbatim, fixing paths (`crate::model::…` stays `crate::model::…`; `llm_client::X` becomes `crate::X`). Add `pub mod service;` + `pub use service::ApiService;` to `llm-client/src/lib.rs`.
In `orchestrator/src/provider_adapter.rs`, temporarily keep `ProviderApiAdapter` as-is (still compiles). Build `-p llm-client`. Commit (`feat(llm-client): ApiService struct + constructors`).

- [ ] **Step 3b: Move the non-streaming drive methods**

Copy the bodies of `OrchestratorApiClient::messages_create` / `messages_create_with_opts` and `SubagentApiClient::messages_create` (provider_adapter.rs:1808/2079) into `ApiService` as inherent `async fn`s (provider-neutral; take `Vec<protocol::ConversationMessage>` + tools, return `LlmResponse`). Move every private helper they call (request building — thinking/effort/temperature/betas/cache/user-agent/metadata; cost; the retry driver loop; decode) into `service.rs`. Build `-p llm-client`. Commit.

- [ ] **Step 3c: Move the streaming drive methods**

Copy `StreamingApiClient::stream` + the forced variant (provider_adapter.rs:2132) into `ApiService` as inherent `async fn stream`/`stream_forced` returning `BoxStream<'static, Result<LlmEvent, LlmError>>`. Move their private helpers (stream open, SSE/WS handling, rate-limit header capture, reconnect). Build `-p llm-client`. Commit.

- [ ] **Step 3d: Thin `ProviderApiAdapter` to a delegating holder + re-point composition roots**

Replace `orchestrator/src/provider_adapter.rs`'s `ProviderApiAdapter` with:
```rust
pub struct ProviderApiAdapter { service: std::sync::Arc<llm_client::ApiService> }
impl ProviderApiAdapter {
    pub fn new(service: std::sync::Arc<llm_client::ApiService>) -> Self { Self { service } }
}
#[async_trait::async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(&self, model: &str, system: Option<&str>, messages: Vec<protocol::ConversationMessage>, tools: Vec<serde_json::Value>) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
        self.service.messages_create(model, system, messages, tools).await
    }
    // messages_create_with_opts → self.service.messages_create_with_opts(...).await
    // messages_create_with_fallback → see Task 4 (composed in orchestrator)
}
#[async_trait::async_trait]
impl StreamingApiClient for ProviderApiAdapter { /* delegate stream(...) */ }
#[async_trait::async_trait]
impl agent::SubagentApiClient for ProviderApiAdapter { /* delegate messages_create / stream / stream_forced */ }
```
Update the composition roots (`apps/engine-desktop/src/lib.rs` ~the `ProviderApiAdapter::new_with_routing(...)` call near line 2091; `apps/engine-mobile/src/host.rs`): build the `ApiService` via `llm_client::ApiService::new_with_routing(...)` (+ the same `.with_*` chain), wrap with `ProviderApiAdapter::new(Arc::new(service))`. Keep `with_forced_tool_choice`/`with_request_metadata`/`with_subscription` on `ApiService`.
Build `--workspace --tests`; test `-p llm-client -p orchestrator -p agent`. Commit.

---

## Task 4: Fallback composition (stays in orchestrator) + cleanup

**Files:**
- Modify: `orchestrator/src/provider_adapter.rs` (the thin adapter), `orchestrator/src/model/mod.rs`

**Interfaces:**
- `OrchestratorApiClient::messages_create_with_fallback` is implemented in the thin `ProviderApiAdapter` by composing `ApiService::messages_create` calls with orchestrator's `model::fallback` policy.

- [ ] **Step 1: Compose fallback in the thin adapter**

Implement `messages_create_with_fallback` on the thin `ProviderApiAdapter`: call `self.service.messages_create(model_a, …)`; on the fallback condition, consult `crate::model::fallback` (which stays in orchestrator) for `model_b`; call `self.service.messages_create(model_b, …)`. `ApiService` stays provider-neutral (no fallback policy inside it). If the current `messages_create_with_fallback` logic is fused with the retry driver such that splitting is artificial, STOP and report — the controller decides whether to move the whole method down instead.

- [ ] **Step 2: Confirm the stayers are intact**

Verify `orchestrator/src/model/{overflow,prompt_too_long,fallback}.rs` still exist and compile; confirm any reference they make to the moved modules now resolves via `llm_client::model::…` (grep `crate::model::{betas,thinking,retry,rate_limit,user_agent,count_tokens,telemetry}` in those 3 files; the re-export from Task 1 Step 5 covers them, but make it explicit if cleaner).

- [ ] **Step 3: Full workspace gate**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --no-fail-fast
```
Expected: PASS (known-flaky powershell/web_fetch may need isolated re-run).

- [ ] **Step 4: Byte-parity spot check**

Confirm `provider_adapter.rs` shrank from ~6.3k lines to a few hundred (delegations only): `wc -l lingxi-code/orchestrator/src/provider_adapter.rs`. Confirm `ApiService` builds the same request: if a request-capture test exists in the moved suite, it passes; otherwise note for the final review.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/orchestrator
git commit -m "refactor(orchestrator): fallback composed over ApiService; thin adapter; Plan B complete"
```

---

## Self-Review

**Spec coverage (Plan B = spec Phases 0-deps, 2, 3, 4, 5):**
- ✅ telemetry dep + 7 policy modules → llm_client::model — Task 1.
- ✅ agent::convert → llm_client::convert — Task 2.
- ✅ ApiService facade + thin trait impls — Task 3 (a–d).
- ✅ fallback/overflow/prompt_too_long stay in orchestrator; fallback composed — Task 4.
- ✅ byte-identical, green per task — gates throughout.

**Placeholder scan:** The ApiService method bodies are "copy from provider_adapter.rs:NNNN" (the exact source lines), not invented — this is a relocation; pasting 6.3k lines here would be the implementation. Each sub-step names the exact source line ranges to move.

**Type consistency:** `ApiService` method names (`messages_create`, `messages_create_with_opts`, `stream`, `stream_forced`) match the delegations in Task 3d and the trait methods they back. `new_with_routing` params match provider_adapter.rs:404.

**Risk note:** Task 3 is the large one — the four sub-steps each end green; if the inherent-vs-trait separation proves entangled mid-extraction, report BLOCKED rather than forcing it. Task 1's re-export (model/mod.rs) keeps `provider_adapter` compiling without per-reference churn until Task 3 moves it wholesale.
