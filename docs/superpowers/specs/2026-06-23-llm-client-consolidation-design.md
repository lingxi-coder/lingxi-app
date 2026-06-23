# Consolidating LLM communication into `llm-client`

**Status:** Design — approved decisions, awaiting spec review
**Date:** 2026-06-23
**Author:** luolingfeng (with Claude)

## Implementation status (updated 2026-06-23)

- **Plan A — `http-client` unification:** ✅ landed on `main`.
- **Plan B — policy + convert (Tasks 1-2):** ✅ landed on `main`. NOTE: **all 10** `model/`
  modules moved into `llm_client::model` — the §2/§7.2 "overflow/prompt_too_long/fallback
  *stay*" split was artificial (they're pure protocol-policy that the movers depend on;
  `prompt_too_long` was promoted out of `compaction` to break a cycle). Treat the "what stays"
  text below as superseded for the model modules.
- **Plan B — `ApiService` facade (Tasks 3-4):** ⏸ **DEFERRED** to a dedicated design pass — the
  drive loop is entangled with orchestrator-domain `crate::prompt` (system-prompt splitting /
  cache-breakpoint structure) + `crate::cost_wiring`, so the extraction is a *parameterizing
  refactor*, not a byte-identical relocation, and needs its own spec.
- **Plan C — OAuth merge:** 📋 planned, not started.

## 1. Summary

Make `llm-client` the single, self-contained subsystem for talking to LLMs:
request build, codecs, the drive loop (retry / rate-limit / reconnect), the
provider-protocol policy (betas, thinking, effort, user-agent, token counting,
api telemetry), and the full auth stack (api-key/bearer/oauth). The
orchestrator and agent keep only thin *ports* (their existing trait
definitions) that delegate into llm-client, plus the conversation-domain logic
that isn't really "talking to a provider."

In parallel, collapse the HTTP socket layer onto **one** pure-Rust
`reqwest + rustls` implementation in a new shared `http-client` crate (the
codebase already uses reqwest+rustls on every real target; today it's just
copied three times plus one stub).

Behaviorally this is **byte-identical to today** — the same code, relocated and
re-wrapped. Risk is mechanical move-bugs only, contained by a strangler rollout
that stays green at every phase.

## 2. Goals / Non-goals

**Goals**
- `llm-client` owns the end-to-end LLM request lifecycle behind one facade.
- One pure-Rust HTTP transport (`reqwest + rustls`) for all platforms; delete
  the 2 duplicate copies and the stub.
- Auth (OAuth) folded into `llm-client`.
- Bring LingXi's module layout closer to claude-code's centralized
  `services/api` (a parity-alignment upside, not just cleanup).

**Non-goals**
- No change to provider wire bytes / behavior. Parity is preserved.
- Do **not** move the consumer trait *definitions* (`OrchestratorApiClient`,
  `StreamingApiClient`, `SubagentApiClient`) — they stay as ports. (The
  "own the API too" option was explicitly declined.)
- Do **not** move conversation-domain policy (`model::overflow`,
  `model::prompt_too_long`, `model::fallback` *policy*) into llm-client.
- No new provider support; this is a relocation, not a feature.

## 3. Current architecture (the seam today)

| Layer | Where | Lines | Notes |
|---|---|---|---|
| Provider codecs / transport trait / registry / route / catalog / cost / sigv4 / eventstream | `llm-client` | — | already owned |
| **The bridge** `ProviderApiAdapter` | `orchestrator/src/provider_adapter.rs` | **6,295** | impls all 3 consumer traits; builds requests (beta/thinking/effort/temp/cache); runs retry/rate-limit driver; decodes |
| **Protocol policy** | `orchestrator/src/model/*` | **6,432** | betas, thinking, retry, rate_limit, user_agent, count_tokens, telemetry **(move)** + overflow, prompt_too_long, fallback **(stay)** |
| Message/tool conversion | `agent/src/convert.rs` | — | sole caller is the adapter |
| Consumer ports | `OrchestratorApiClient`+`StreamingApiClient` (conversation.rs), `SubagentApiClient` (agent/api.rs) | — | stay; become thin delegations |

**Connection wiring (verified):**
```
reqwest (ReqwestHttp : traits::http::HttpTransport)   [platforms/common, windows, posix — 3 copies]
  → LlmTransportBridge<T: HttpTransport> : llm_client::Transport   [platforms/common/llm_transport.rs]
  → Arc<dyn llm_client::Transport>  → ProviderApiAdapter::new_with_routing(...)   [engine-desktop/lib.rs:2091]
```
- All three "platform" HTTP impls are the same `reqwest + rustls-tls` code
  (Windows is **not** WinHTTP). `platforms/posix-minimal` is a **stub** that
  errors on every call (`"Plan 17 wires the real client"`).
- `traits::http::HttpTransport` is a **shared** abstraction also used by
  `web_fetch`/`web_search`, MCP, cron, sidequery, and the oauth refreshers —
  only composition roots reference the concrete `ReqwestHttp`.

## 4. Decisions (locked)

1. **Scope = "+ protocol policy."** Move the bridge substance + the
   provider-protocol half of `model/` down. Consumer traits stay up.
2. **Structure = `ApiService` facade.** `llm_client::service::ApiService` owns
   the whole drive loop; ports become 1:1 delegations.
3. **Auth = merge.** Fold `anthropic-oauth` + `openai-oauth` into
   `llm_client::oauth::{anthropic,openai}`; delete the crates.
4. **Connection = unify on pure-Rust reqwest.** One `reqwest + rustls`
   transport in a new shared `http-client` crate; move the
   `HttpTransport → llm_client::Transport` bridge **into** llm-client; the
   concrete socket is injected (shared infra). `llm-client` re-exports a
   convenience constructor.

## 5. Target architecture

```
┌─ http-client crate (NEW) ─────────────────────────────────────────┐
│  ReqwestHttp : traits::http::HttpTransport   (the ONE Rust socket) │
│  reqwest + rustls-tls + tokio-tungstenite (WS)                     │
└───────────────────────────────────────────────────────────────────┘
        ▲ injected as Arc<dyn traits::http::HttpTransport>
        │  (only composition roots know about http-client)
┌─ llm-client crate ────────────────────────────────────────────────┐
│  transport::LlmTransportBridge   (HttpTransport → Transport, moved)│
│  service::ApiService             (drive loop: build→send→decode→   │
│                                   retry/rate-limit; the facade)    │
│  model::{betas,thinking,retry,rate_limit,user_agent,count_tokens,  │
│          telemetry}              (moved policy)                    │
│  convert::*                      (moved from agent)                │
│  oauth::{anthropic,openai}       (moved from the two crates)       │
│  + existing: providers/*, codecs, registry, route, auth, cost…    │
└───────────────────────────────────────────────────────────────────┘
        ▲ thin 1:1 delegations
┌─ orchestrator / agent ────────────────────────────────────────────┐
│  OrchestratorApiClient / StreamingApiClient / SubagentApiClient    │
│      (trait DEFS stay; impls delegate to ApiService)               │
│  model::{overflow, prompt_too_long, fallback-policy}  (stay)       │
└───────────────────────────────────────────────────────────────────┘
```

## 6. Dependency analysis (cycle-free — verified)

New edges and why each is safe:
- `llm-client → protocol` — protocol does **not** depend on llm-client. ✅
- `llm-client → traits` — traits does **not** depend on llm-client. ✅
- `llm-client → telemetry` — telemetry does **not** depend on llm-client. ✅
- `http-client → {traits, protocol, reqwest, tokio-tungstenite}` — leaf; nobody
  depends back on it except composition roots. ✅
- `agent::convert` leaves `agent` (agent depends on llm-client, so the reverse
  is forbidden — moving convert *down* resolves it). ✅
- `anthropic-oauth`/`openai-oauth` already depend on llm-client; folding their
  code *in* removes the crates, no new cycle. ✅

## 7. Detailed moves

### 7.1 `http-client` crate (connection)
- Move `ReqwestHttp` (+ no-redirect client, SSE boundary scanner, WS via
  tokio-tungstenite) from `platforms/common/src/http.rs` → `http-client`.
- Delete the duplicate copies in `platforms/windows/src/http.rs` and
  `platforms/posix/src/http.rs`; both depend on `http-client` instead.
- Replace `platforms/posix-minimal` stub (and the ios/android wiring) with the
  real `http-client` transport (finishes "Plan 17"). If a genuinely no-network
  build is ever needed, that becomes an explicit opt-out, not the default.
- Move `LlmTransportBridge` (`platforms/common/src/llm_transport.rs`) **into**
  `llm-client` as `llm_client::transport::from_http(Arc<dyn HttpTransport>) ->
  Arc<dyn Transport>` (+ keep the generic bridge type).
- Composition roots (`engine-desktop`, `engine-mobile`, `cli`) construct
  `http_client::ReqwestHttp`, wrap via `llm_client::transport::from_http`, and
  pass to `ApiService`.

### 7.2 Protocol-policy modules → `llm_client::model::*`
- Move `betas, thinking, retry, rate_limit, user_agent, count_tokens,
  telemetry` (no orchestrator `crate::` imports — confirmed self-contained).
- Re-point the only external refs: `conversation.rs`/`test_support.rs` use
  `rate_limit::{RateLimitInfo,RawUtilization,RawWindow}`; `conversation.rs`
  uses `count_tokens::APPROX_CHARS_PER_TOKEN`; `error.rs` doc-refs
  `rate_limit_error_message`. They switch to `use llm_client::…`.
- `model::telemetry` carries the `tengu_api_*` events; depends on
  `telemetry::AnalyticsBus` (now an llm-client dep).

### 7.3 `agent::convert` → `llm_client::convert`
- Move `to_llm_messages`, `normalize_messages_for_api`,
  `ensure_tool_result_pairing`, `to_tool_declarations`. Sole caller is the
  adapter → trivial re-point.

### 7.4 `ApiService` facade
- Move `ProviderApiAdapter`'s substance (request build, header injection,
  `transport.execute`/`open_stream`, decode, retry/rate-limit driver loop,
  prompt-cache gates, forced-tool-choice, request-metadata, thinking config,
  cost estimation, analytics) into `llm_client::service::ApiService`.
- Struct state moves verbatim: `client`, `transport`, subscriber/subscription
  (`traits::subscription::SharedSubscription`), `forced_tool_choice`,
  `thinking`, `request_metadata`, `cache_editing_inputs`, `ua`, `version`,
  `analytics`, fallback config.
- Public surface mirrors the union of the three traits' methods, in protocol /
  llm-client types:
  - `messages_create`, `messages_create_with_opts`
  - `stream`, `stream_forced`  (`BoxStream<Result<LlmEvent, LlmError>>`)
  - builder config: `with_thinking`, `with_forced_tool_choice`,
    `with_request_metadata`, `with_subscription`, fallback config.
- The three trait impls stay in orchestrator/agent and become thin:
  `impl OrchestratorApiClient for ProviderApiAdapter { async fn
  messages_create(..) { self.service.messages_create(..).await } }`, etc.
  `ProviderApiAdapter` shrinks to a holder of `Arc<ApiService>`.

### 7.5 Fallback stays in orchestrator
- `messages_create_with_fallback` is an `OrchestratorApiClient` method. Keep
  `model::fallback` *policy* (which model to fall back to) in orchestrator; the
  thin port implements it by calling `service.messages_create(model A)`,
  consulting orchestrator fallback config, then `service.messages_create(model
  B)`. `ApiService` stays provider-neutral. **(Decided: port-composed.)**

### 7.6 Auth → `llm_client::oauth::*`
- Move `anthropic-oauth/src/*` → `llm_client::oauth::anthropic`,
  `openai-oauth/src/*` → `llm_client::oauth::openai`. Delete both crates.
- Deps are only `llm-client` + `tokio` (no reqwest/browser/keyring) → no new
  heavy deps. Includes `profile`/`subscription` (feeds the 429 subscriber gate)
  and the `OAuthCredentialProvider : llm_client::CredentialProvider` impls.
- Re-point ~6 consumers: `apps/cli`, `apps/engine-desktop`,
  `apps/engine-mobile`, `migrations`, `platforms/android-minijail`,
  `test-harness` (`anthropic_oauth::` → `llm_client::oauth::anthropic::`).

## 8. Phasing (strangler — green + parity-verified at each step)

Independent phases marked `[indep]` can reorder.

| Phase | Work | Risk |
|---|---|---|
| **0** | Add `protocol`/`traits`/`telemetry` deps to llm-client; scaffold empty `http-client` crate | trivial |
| **1** `[indep]` | `http-client`: one reqwest+rustls transport; delete win/posix copies; finish posix-minimal/ios/android; move `LlmTransportBridge` into llm-client; re-point composition roots | low |
| **2** | Relocate the 7 policy modules → `llm_client::model::*`; re-point the few external refs | low |
| **3** | Relocate `agent::convert` → `llm_client::convert` | low |
| **4** | Extract adapter substance → `llm_client::service::ApiService`; thin the 3 trait impls (do in sub-steps: non-stream, stream, builders, tests) | **high — the big one** |
| **5** | Keep overflow/prompt_too_long/fallback-policy in orchestrator; wrapper composes fallback; cleanup re-exports & dead wiring | low |
| **6** `[indep]` | Auth: fold the two oauth crates → `llm_client::oauth::*`; delete crates; re-point ~6 consumers | low |

## 9. Ripple inventory (re-points)
- `conversation.rs`, `test_support.rs`, `error.rs` → `llm_client::model::rate_limit/count_tokens`.
- Composition roots `engine-desktop/lib.rs:2091`, `engine-mobile/host.rs:581` → construct `ApiService` + `http_client::ReqwestHttp`.
- `platforms/{windows,posix,posix-minimal,ios,android}` HTTP → depend on `http-client`.
- ~6 oauth consumers re-point imports.
- Tests living in `provider_adapter.rs` / `model/*` move with their code into llm-client.

## 10. Testing & parity strategy
- Every phase ends green: `cargo build --workspace --tests` + the owning
  crate's suite, no-fail-fast across the workspace after cross-cutting phases.
- Relocated unit tests move with their code (codec/beta/thinking/rate-limit
  tests → llm-client; convert tests → llm-client).
- No new behavioral tests are required — this is relocation. The parity claim
  is "same code, same bytes," checked by the existing suites + spot byte-checks
  of a request/stream against the v2.1.185 oracle after Phase 4.
- Mock clients (`MockApiClient`, `MockStreamingApiClient`, `ScriptedApiClient`)
  stay where the traits stay (orchestrator/agent), now delegating to a mock
  `ApiService` or unchanged (they impl the ports, not ApiService).

## 11. Risks & mitigations
- **Phase 4 is large (6.3k lines).** Mitigate: sub-step by method family;
  keep `ProviderApiAdapter` as the holder so call sites/composition roots barely
  change; green after each sub-step.
- **Mobile/iOS pure-Rust TLS tradeoff.** reqwest+rustls drops native system
  proxy / per-app VPN / background URLSession / ATS pinning. Accepted: the
  project already chose reqwest ("Plan 17"); irrelevant for a coding agent.
- **llm-client dep weight grows** (gains protocol/traits/telemetry; oauth code).
  Still no reqwest in llm-client itself (socket stays in `http-client`,
  injected) — so the core stays swappable and lighter than pulling reqwest in.
- **Hidden orchestrator coupling in the adapter** surfacing during Phase 4.
  Mitigate: the struct-field audit (§7.4) already enumerated state; anything
  orchestrator-only gets parameterized, not dragged down.

## 12. Resolved decisions (were open)
- **Crate name:** `http-client`.
- **`ApiService` signatures:** opts struct for the create/stream methods;
  positional for trivial ones (keeps port delegations 1:1).
- **`messages_create_with_fallback`:** port-composed — fallback *policy* stays
  in orchestrator; `ApiService` stays provider-neutral.
- **posix-minimal:** drop the stub; wire `http-client` everywhere (reqwest on
  all platforms). An explicit no-network opt-out is added only if a real target
  needs it.
