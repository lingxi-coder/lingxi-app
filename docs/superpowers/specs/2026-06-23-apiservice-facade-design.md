# ApiService Facade — Re-design (supersedes deferred Plan B Tasks 3-4)

**Status:** Design — awaiting approval
**Date:** 2026-06-23
**Author:** luolingfeng (with Claude)

## 1. Summary

Extract `ProviderApiAdapter`'s ~6.3k-line drive loop into `llm_client::service::ApiService`,
reducing the orchestrator/agent consumer-trait impls to thin delegations. The original Plan B
deferred this fearing a "parameterizing refactor" entangled with orchestrator-domain
`crate::prompt` + `crate::cost_wiring`. A deeper investigation **reverses that**: the drive
logic's *only* orchestrator-domain dependency is one self-contained pure formatter
(`split_system_blocks_with`); everything else flagged was **test-only**. So this is a **clean
relocation** (like Plans A/B/C) plus one small formatter move — not a refactor.

## 2. The corrected finding (why this is now tractable)

Initial ref-counting flagged `crate::prompt` (8×), `crate::cost_wiring` (2×), `crate::error`,
`crate::conversation` in `provider_adapter.rs`. Resolved per usage:

| Apparent dep | Reality |
|---|---|
| `crate::prompt::split_system_blocks_with` + `SplitOptions` + `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` | **The one real drive dep.** Pure formatter `&str → Vec<llm_client::SystemBlock>` (Anthropic cache-block placement, mirrors `utils/api.ts`). Body uses only llm-client types + its own boundary const — self-contained. |
| `crate::prompt::locked_templates` (HEADER/SECTION_SEP) | **Test-only** (tests build realistic prompts). These are byte-locked *prompt content* → stay in orchestrator. |
| `crate::cost_wiring::llm_catalog_from_cost` | **Test-only.** The drive uses the *injected* `cost_estimator` (already parameterized via `new_with_routing`). |
| `crate::error::{OrchestratorError, REPEATED_529_ERROR_MESSAGE}` | **Test-only** (in `#[test]` fns). |
| `crate::conversation::{OrchestratorApiClient, StreamingApiClient}` | Just the **trait imports** — stay with the thin adapter. |

Verified: `split_system_blocks_with`'s body is self-contained; `cost` crate does **not** depend
on llm-client; both `cost_wiring` refs and the `crate::error` ref are inside `#[test]` functions.

## 3. Goals / Non-goals

**Goals**
- `llm_client::service::ApiService` owns the full drive loop (build → execute → decode →
  retry/rate-limit), speaking `protocol::ConversationMessage` + llm-client types.
- The 3 consumer traits stay in orchestrator/agent; their impls become 1:1 delegations.
- Move `split_system_blocks_with` + `SplitOptions` + `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` into
  llm-client (provider-protocol cache formatting).
- Byte-identical behavior.

**Non-goals**
- No parameterizing refactor (the deferral's feared path — not needed).
- Do **not** move the consumer-trait *definitions* (stay as ports — explicitly declined earlier).
- Do **not** move `locked_templates` (byte-locked prompt *content* → orchestrator-domain).
- No change to the cost path (already injected).

## 4. The design

### 4.1 Move the cache formatter → llm-client
`split_system_blocks_with` + `SplitOptions` + `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`
(`orchestrator/src/prompt/mod.rs`) → `llm_client::prompt_format` (new module). They produce
`llm_client::SystemBlock` already; only their `crate::prompt::…` self-refs change. Orchestrator
re-exports them from `llm_client::prompt_format` for any in-orchestrator callers.

### 4.2 `ApiService` facade (the drive loop)
Move `ProviderApiAdapter`'s struct + inherent drive methods (build_request, the
thinking/effort/temp/betas/cache request build, `transport.execute`/`open_stream`, decode, the
retry/rate-limit driver, cost estimation, the `should_use_global_cache_scope`/`should_1h_cache_ttl`
cache-scope predicates) into `llm_client::service::ApiService`. It calls
`crate::prompt_format::split_system_blocks_with` internally. Builders (`new_with_routing`,
`with_thinking`/`with_forced_tool_choice`/`with_request_metadata`/`with_subscription`) move with it.

Public surface (inherent, provider-neutral):
- `messages_create(model, system: Option<&str>, messages: Vec<protocol::ConversationMessage>, tools) -> Result<LlmResponse, LlmError>`
- `messages_create_with_opts(…, opts) -> Result<LlmResponse, LlmError>`
- `stream(…) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>` + `stream_forced(…)`

### 4.3 Thin the trait impls (stay in orchestrator/agent)
`ProviderApiAdapter` becomes `{ service: Arc<llm_client::ApiService> }`; the 3 trait impls
(`OrchestratorApiClient`, `StreamingApiClient`, `agent::SubagentApiClient`) delegate 1:1.
`messages_create_with_fallback` composes the fallback chain over `service.messages_create`.
**As implemented (2026-06-23):** the composition lives **in `ApiService`** (inherent), not the
thin adapter — the fallback chain-walk is inseparable from the private `drive_non_stream_seeded_with_chain`
driver, and it reads only `ApiService`-owned state (no orchestrator types). Keeping it in the
adapter would have forced re-exposing that moved state. Fallback-on-overflow is model-selection
(provider-protocol), so this does not breach provider-neutrality. (Allowed by the original Plan B
§7.5 "if splitting proves artificial, the method moves down whole".) The adapter delegates 1:1.

### 4.4 Test placement
The provider_adapter tests that use orchestrator-domain helpers (`locked_templates`, `cost_wiring`,
`error`) **stay in orchestrator** as thin-adapter integration tests (orchestrator can use both its
own modules and `llm_client`). `ApiService` gets **llm-client-level unit tests** for the drive
logic (system-block splitting, request build, retry) using plain-string fixtures + llm-client
types — no orchestrator helpers needed. The split-formatter's own unit tests move with it.

## 5. Dependency analysis (cycle-free)
- `split_system_blocks_with` is self-contained (llm-client types + its const) → moves cleanly.
- `ApiService`'s drive path uses `llm_client::{model, convert, prompt_format, Transport, …}` +
  `protocol` + `platform_api::subscription` + `telemetry` — all already llm-client deps (Plans A/B).
  No orchestrator-internal reference remains in the drive path.
- The thin adapter (orchestrator) holds `Arc<ApiService>` + the trait impls — orchestrator → llm-client (existing).

## 6. Phasing (strangler, green per step)
| Phase | Work | Risk |
|---|---|---|
| **1** | Move `split_system_blocks_with`/`SplitOptions`/boundary → `llm_client::prompt_format`; orchestrator re-exports; fix the one `build_request` call site | low |
| **2** | Create `ApiService` skeleton (struct + builders + `new_with_routing`) in `llm_client::service`; build green (unused) | low |
| **3** | Move the non-streaming drive (`messages_create`/`_with_opts` + their private helpers) into `ApiService` | med |
| **4** | Move the streaming drive (`stream`/`stream_forced` + helpers) into `ApiService` | med |
| **5** | Thin `ProviderApiAdapter` to delegations; re-point composition roots (engine-desktop/mobile); keep fallback composition in the adapter | **the integration step** |
| **6** | Test split: keep orchestrator-helper tests in orchestrator; add ApiService unit tests; full workspace green | low |

## 7. Risks
- **Phase 5 integration** is the real work — the 3 trait impls + composition roots. Do it after the
  drive logic already lives in `ApiService` (Phases 2-4), so it's pure delegation wiring.
- **Test split** (Phase 6): some provider_adapter tests assert drive behavior *through* orchestrator
  helpers — decide per-test whether it becomes an ApiService unit test (plain fixtures) or stays an
  orchestrator integration test. No behavioral change; just placement.
- The `should_use_global_cache_scope`/`should_1h_cache_ttl` predicates move with `ApiService`;
  confirm they read only adapter state (subscription/env), not orchestrator internals (expected).

## 8. Open decision (the one fork)
`split_system_blocks_with`: **move into llm-client** (§4.1 — recommended; it's pure provider-protocol
formatting producing `llm_client::SystemBlock`, exactly like the `model/` modules) **vs.
parameterize** (orchestrator splits and passes `Vec<SystemBlock>` into `ApiService`). This spec
assumes **move**; the parameterize variant only changes §4.1-4.2 (ApiService takes pre-split blocks;
the formatter stays in orchestrator) and is a smaller delta if preferred.
