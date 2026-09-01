# Plan 3c — modelProviders settings + multi-provider live routing — Design

**Status:** Design (brainstormed 2026-06-15). Drives the Plan 3c implementation plan.

**Goal:** Make the LingXi engine *actually route* to every configured provider — the built-in
catalog (OpenRouter / DeepSeek / GLM-coding / GitHub Copilot), user-defined custom providers from
`settings.providers`, and `settings.routing` aliases + cross-provider fallback chains — with
keychain-backed credentials and an interactive `/connect` flow. Today these providers are *listed*
(the `/model` picker shows them) but the engine builds a single-Anthropic `ClientConfig`, so
selecting one does not resolve.

**Branch:** `llm-client-3a-resume` (off `parity-llm-client-3a`). Built on the completed Plan 3a
(engine model seam on `llm_client::DefaultLlmClient`) + Plan 3b (api-client / providers crates
deleted).

---

## 1. Scope

Full Plan 3c in one spec — all four sub-systems (decided during brainstorming):

1. **Built-in catalog routing** — merge `llm_client::builtin_presets()` into the engine
   `ClientConfig` + wire the merged pricing catalog.
2. **User-defined providers** — parse `settings.providers` into `llm_client::ProviderProfile`s
   (re-implements the deleted `providers::parse_profiles` onto llm-client).
3. **Routing config** — `settings.routing` aliases + **cross-provider fallback chains** + retry
   (re-implements the deleted `providers::parse_routing` + the chain-walking `ModelRouter`).
4. **Credential management UX** — keychain-stored keys via an interactive `/connect` (incl. Copilot
   OAuth device-flow), env-var fallback, and a "show-all + Connect badge" `/model` picker.

### Key decisions (brainstorming outcomes)

| Decision | Choice |
|---|---|
| Scope | Full Plan 3c (all four) in one spec |
| Credentials | Keychain (via `CredentialManager`) + env fallback, set through `/connect` |
| Fallback | Full cross-provider chains (parity with the deleted `ModelRouter`) |
| Picker gating | Show all providers; badge unconfigured ones; selecting one launches `/connect` |
| `/connect` Copilot | Implement both API-key providers **and** Copilot OAuth device-flow |
| Architecture | Approach B — a dedicated `provider-config` crate for assembly + composite credentials; chain-walking executes in the orchestrator |

---

## 2. Current state (starting point)

- `llm_client::builtin_presets() -> BuiltinCatalog { providers: Vec<ProviderProfile>, pricing:
  PricingCatalog }` already returns profiles **ready to merge** into a `ClientConfig`
  (openrouter / deepseek / glm-coding / github-copilot). Presets ship `CredentialConfig::Env{var}`
  defaults.
- `DefaultLlmClient::authenticate` already resolves `CredentialConfig::Env{var}` via
  `EnvCredentialProvider` (no wiring), and `CredentialConfig::Static{id}`/`HostManaged{id}` via the
  single `self.credentials` slot, keyed by `CredentialScope{provider_id, credential_id}`. Secrets
  are extracted from `Credential::ApiKey|BearerToken` uniformly; the `AuthStrategy`
  (`ApiKey`/`Bearer`/`OAuthBearer`/`CopilotBearer`) selects the header.
- `lingxi_core::settings` already defines `providers: Option<BTreeMap<String, Value>>` (shape:
  `{"groq":{"type":"openai","baseUrl":"…","apiKeyEnv":"GROQ_API_KEY"}}`) and a `routing` block; these
  ride into `DesktopConfig.provider_profiles` / `DesktopConfig.routing` but are currently **unread**
  (they were parsed by the now-deleted `providers::parse_profiles` / `parse_routing`).
- The deleted `providers::routing` defined `RoutingConfig { aliases: {alias→"provider/model"},
  fallback: {key→["provider/model",…]}, retry: {maxAttempts, backoffMs} }` and a chain-walking
  `FallbackChain` that advanced on transient errors.
- The engine `build()` constructs `ClientConfig { providers: vec![anthropic_profile(...)] }` only;
  the composite credential slot currently holds the api-key `StaticCredentialProvider` or the
  Anthropic `OAuthCredentialProvider` (from the Plan-3a OAuth nit).
- `ProviderId` variants include `OpenAICompatible{name}` (used by catalog presets) and `Custom{name}`
  (for user providers).

---

## 3. Architecture (Approach B)

```
settings.json{providers, routing} + builtin_presets() + Anthropic cfg
        │
        ▼
  provider-config::assemble(..)                         ← NEW crate (pure, unit-tested)
        │   → Assembled { client_config, pricing, chains, credential_sources, warnings }
        ▼
  engine build() (desktop + mobile)
        │   • DefaultLlmClient::from_config(client_config)
        │   • .with_credential_provider(MultiCredentialProvider)   ← composite, single slot
        │   • CostEstimator::from(pricing)
        │   • ProviderApiAdapter::new(.., chains)
        │   • availability map → /model picker
        ▼
  runtime: /model pick (provider_id, request_model) → switch_model → run_turn
        ▼
  ProviderApiAdapter: alias→chain → [outer chain loop [inner retry-driver loop]]
        ▼
  DefaultLlmClient.prepare(resolve provider/model) → authenticate(composite|env) → execute
```

**Layering rationale.** `llm-client` stays a provider-neutral mechanism (catalog data + the
`DefaultLlmClient` execute/auth path). The settings→config assembly + the composite credential
resolver are isolated in a new, unit-testable `provider-config` crate. The **chain-walking
execution** lives in the orchestrator's `ProviderApiAdapter` because it must interleave with the
retry driver (which is in the orchestrator, not llm-client). Config is data (`provider-config`);
execution needs the retry driver (orchestrator) — the split is inherent, not incidental.

---

## 4. Components

### 4.1 `provider-config` crate (NEW)

Pure, no I/O, unit-testable. Workspace member; deps `llm-client`, `secret`, `serde_json`. It takes
the raw `serde_json::Value`s the engine already holds (`DesktopConfig.provider_profiles` /
`routing`), so it needs no `engine` dependency; the Anthropic-OAuth credential case is passed in as a
pre-built `Arc<dyn llm_client::CredentialProvider>` delegate, so it needs no `anthropic-oauth`
dependency either.

- `parse_user_providers(providers: &BTreeMap<String, Value>) -> (Vec<ProviderProfile>, Vec<Warning>)`
- `parse_routing(routing: Option<&Value>) -> (ChainConfig, Vec<Warning>)`
- `assemble(AssembleInputs) -> Assembled`
- `MultiCredentialProvider` (impl `llm_client::CredentialProvider`)
- `ChainConfig`, `ChainEntry`, `RetryOverride`, `CredentialSource` types.

### 4.2 `secret` crate extension

`CredentialManager` (today: OAuth tokens) gains generic provider-key storage:
`set_provider_key(id: &str, secret: &str)` / `get_provider_key(id: &str) -> Option<Secret>`,
backed by the same OS keychain, keyed by credential id.

### 4.3 orchestrator `ProviderApiAdapter`

Gains a `chains: ChainConfig` field and the chain-walking outer loop around the existing
retry-driver loop, on both the non-streaming and streaming-connect paths (§7).

### 4.4 engine `build()` (engine-desktop + engine-mobile)

Calls `assemble`, builds the client, attaches the composite credential provider, wires the
`CostEstimator`, passes `chains` to the adapter, computes the availability map. Desktop also drives
`/connect`. (engine-mobile shares the routing core; its interactive `/connect` UI is a follow-up —
mobile uses env/settings keys.)

### 4.5 `/connect` command + picker

`commands-core` registers `/connect`; the engine owns the keychain writes + Copilot device-flow
driving; the tui owns the interactive UI (secure key input, device-code display, poll spinner) and
the picker Connect badges + select-launches-`/connect`.

---

## 5. Parsing contracts

### 5.1 `settings.providers` → `Vec<ProviderProfile>` (`parse_user_providers`)

Input: `{ "<name>": { "type": "openai"|"anthropic"|"gemini", "baseUrl": "...", "apiKeyEnv":
"ENV_VAR"?, "models": ["id", …]? } }`

| field | maps to |
|---|---|
| key `<name>` | `profile_name`; `provider_id = OpenAICompatible{name}` (openai) or `Custom{name}` (anthropic/gemini) |
| `type` | `openai`→`ProtocolFamily::OpenAiChat`+`AuthStrategy::Bearer`; `anthropic`→`AnthropicMessages`+`ApiKey`; `gemini`→`GeminiGenerateContent`+`ApiKey` |
| `baseUrl` | `base_url` |
| `apiKeyEnv` | recorded as the env fallback in the emitted `CredentialSource` |
| `models[]` | each id → a `ModelProfile` (display=request=billing=id, permissive `Capabilities`) |

- **User-provider models (decision):** the registry resolves by registered model, so a routable
  user provider must declare `models`. Omitting `models` → the provider is listing-only (not
  routable) until models are declared or a routing alias targets a declared model. The legacy
  free-form `provider/model` typing is reconstructed via `routing.aliases` + declared models; a true
  wildcard passthrough is an explicit **non-goal**.
- Unknown `type` or missing `baseUrl` → the entry is skipped with a collected warning (non-fatal).

### 5.2 `settings.routing` → `ChainConfig` (`parse_routing`)

Input: `{ "aliases": {alias: "provider/model"}, "fallback": {key: ["provider/model", …]}, "retry":
{"maxAttempts": n, "backoffMs": n} }`

```rust
struct ChainConfig {
    aliases: BTreeMap<String, String>,                 // alias → "provider/model"
    chains:  BTreeMap<String, Vec<ChainEntry>>,        // key → ordered failover entries
    retry:   Option<RetryOverride>,                    // { max_attempts, backoff_ms }
}
struct ChainEntry { provider_id: ProviderId, model: String }
```

- `aliases` — `assemble` **folds** each `alias → "provider/model"` into the target
  `ModelProfile.aliases` (located by provider + model), so `registry.resolve("fast")` works
  natively. Unknown target → warn + skip.
- `fallback` — parsed into `chains`; key is an alias or `"provider/model"`; each `"provider/model"`
  entry is validated against the registered profiles (unknown → warn + skip the entry).
- `retry` — `RetryOverride { max_attempts, backoff_ms }`, fed to the per-entry retry-driver budget.

### 5.3 `assemble(AssembleInputs) -> Assembled`

Inputs: Anthropic cfg (`api_base`, default/fallback model, `has_api_key`, `has_oauth`, `models`) +
`settings.providers` JSON + `settings.routing` JSON.

```rust
struct Assembled {
    client_config:      ClientConfig,            // anthropic + builtin_presets + user providers
    pricing:            PricingCatalog,           // merged
    chains:             ChainConfig,
    credential_sources: Vec<CredentialSource>,    // per profile
    warnings:           Vec<String>,
}
struct CredentialSource {
    provider_id:   ProviderId,
    credential_id: String,                        // keychain key id (== profile credential id)
    env_var:       Option<String>,                // env fallback (from apiKeyEnv / preset default)
    kind:          CredentialKind,                // ApiKey | OAuth | Keychain
}
```

Steps: build the Anthropic profile (the 3-way ApiKey/OAuthBearer/None from the Plan-3a OAuth nit) +
`builtin_presets().providers` (rewrite each preset's `CredentialConfig::Env{var}` → `Static{id}` and
record `env_var = var`) + `parse_user_providers`; merge into one `ClientConfig.providers`; merge the
pricing catalogs; fold `routing.aliases`; build validated `chains`; emit `credential_sources`.

### 5.4 Uniform credential model

Every routable profile's `CredentialConfig` becomes `Static{id}` routed to the composite provider.
This is what lets `/connect`-stored keychain keys take precedence over env. The composite resolves
**keychain[id] → env[var] → None**.

---

## 6. Credentials — composite provider, gating, `/connect`

### 6.1 `MultiCredentialProvider` (the single slot)

Holds `Arc<CredentialManager>` + the `credential_sources` map + the Anthropic api-key + an optional
**OAuth delegate** (an `Arc<dyn llm_client::CredentialProvider>` — the existing
`OAuthCredentialProvider`, built by the engine which already deps `anthropic-oauth`). Async
`load(scope: CredentialScope)` dispatches on `(provider_id, credential_id)`:

- `anthropic-oauth` → delegate to the OAuth provider.
- `anthropic-api-key` → the configured Anthropic key.
- any other provider (`credential_id` = the provider's key id) → **keychain[id] → env[recorded var]
  → None**.

The secret is returned as `Credential::ApiKey`/`BearerToken` and `load_secret` extracts it uniformly
(the `AuthStrategy` chooses the header), so Copilot (`CopilotBearer`, key from keychain or
`GITHUB_TOKEN`) rides the same path. This **replaces** the separate `StaticCredentialProvider` /
`OAuthCredentialProvider` attachment from the Plan-3a OAuth nit — the composite is now the one slot
for all providers (delegating the OAuth case).

### 6.2 Availability / gating

At `build()` time, per profile: `available = keychain_has(id) || env_set(var) || anthropic
key/oauth present`. **All** providers still enter the `ClientConfig` (the "show all" choice); the
availability map drives the picker only. Selecting an unavailable provider launches `/connect`
instead of routing.

### 6.3 `/connect` flow (engine drives, tui renders)

- `/connect <provider>` (API-key providers: openrouter / deepseek / glm / user) → secure key prompt
  → `CredentialManager.set_provider_key(id, key)`.
- `/connect github-copilot` → device-flow: `CopilotLogin::begin()` → display `user_code` +
  `verification_uri` → poll `poll_once` (wired to a host `CopilotHttp` over `PosixHttp`, honoring
  `SlowDown`/`Pending`) until `Success` → `set_provider_key("github-copilot", token)`.
- No restart: the composite reads the keychain live on the next request; the picker availability
  refreshes.
- Keys are stored without a validation probe (a bad key 401s on the next turn).

### 6.4 Picker UX (extends Phase 3-B)

Each provider group carries an availability flag → render a **Connect** badge on unconfigured
providers; selecting an unconfigured provider's model launches `/connect <provider>` (then switches
on success); configured providers `switch_model` as today.

---

## 7. Fallback-chain execution (orchestrator)

Chain-walking lives in `ProviderApiAdapter` as an **outer loop wrapping the existing retry-driver
loop**, on both the non-streaming (`drive_non_stream`) and streaming (`stream()` connect) paths:

```
resolve alias → model;  chain = chains[model]  (or [model] if none)
for entry in chain:                         // OUTER: cross-provider failover
    req.model = entry.model                 // registry resolves entry's provider/model
    result = retry_driver_loop(req, budget) // INNER: per-attempt retry (today's loop)
    match result {
        Ok        => record served model; return,
        Err(e) if failover_worthy(e) && more entries => continue,  // advance chain
        Err(e)    => return Err(e),          // terminal-class, or last entry
    }
```

- **`failover_worthy`** (mirrors the legacy `is_transient`): advance on an exhausted transient-class
  error (`Overloaded` / `RateLimited` / `ProviderInternal` / `Transport`) or an unavailable
  provider; **stop immediately** on terminal-class (`Authentication` / `InvalidRequest` /
  `ContextOverflow` / `UnsupportedCapability`).
- **Chain supersedes built-in fallback (reconciliation rule):** a model that *has* a `routing`
  chain runs each entry with `allow_fallback = false` — the explicit chain *is* the fallback, so the
  retry driver's built-in Opus→Sonnet one-shot does not compound. A model with *no* chain keeps
  today's behavior exactly (single entry + the built-in `fallback_model`). `routing.fallback` for a
  model overrides the built-in Anthropic fallback for that model only.
- **Per-entry retry budget** comes from `routing.retry` when set, else the existing default —
  applied fresh to each entry's inner loop.
- **Provider/model resolution:** `entry.model` is set on the request and the registry resolves it;
  `assemble` validated `(provider_id, model)` at parse time. Model-id collisions across providers
  resolve deterministically to the first-registered (documented; catalog ids like `openai/gpt-4o`
  are already vendor-qualified).
- **Served-model surfacing (also resolves Plan-3a deferred nit (e)):** when the chain advances, the
  adapter records the served model — non-streaming injects the `{fallback_from, fallback_to}` marker
  into `response.provider_metadata` (existing `record_model_fallback` → `handle_model_fallback`
  persist + warn + `tengu_model_fallback_triggered`); streaming (chain-walk at connect, before the
  stream opens) emits the connect-phase fallback warning + stamps the assembled response's model.
  This gives the streaming connect-phase fallback the surfacing channel nit (e) was missing.

---

## 8. Engine wiring (`build()`)

After the OAuth-state block: `provider_config::assemble(Anthropic cfg + cfg.provider_profiles +
cfg.routing)` → build `DefaultLlmClient` from `assembled.client_config` → attach the composite
`MultiCredentialProvider` (replaces the api-key/OAuth attachment) → wire a `CostEstimator` from
`assembled.pricing` (extends `cost_wiring` so non-Anthropic responses are priced) → pass
`assembled.chains` to `ProviderApiAdapter::new` → compute the availability map for the picker.
`cfg.provider_profiles` / `cfg.routing` become **read**. `assemble().warnings` go to `tracing`.

engine-mobile shares the `assemble`/client/composite/chains core (api-key + env for Anthropic; no
OAuth); its interactive `/connect` UI is a follow-up.

**Availability surfacing decision:** to keep `traits` / `protocol` / `llm-client` frozen-clean,
availability rides a **sibling map** from the handle (not a new field on the frozen `ModelListing`
DTO); the tui picker joins it by `provider_id`. (Additive optional field is the fallback, matching
Phase 3-A's Option A.)

---

## 9. Error handling

- Malformed `providers` / `routing` entry → collected warning, entry skipped, build continues
  (non-fatal); warnings surfaced via `tracing`.
- Unconfigured provider routed directly (bypassing the picker) → `Authentication` / 401, surfaced as
  today; the picker's Connect badge prevents the normal path.
- Chain exhaustion → the last entry's error propagates.
- `/connect`: API-key stored without a probe; Copilot device-flow `Failed`/timeout → surfaced,
  nothing stored; keychain-unavailable (headless) → clear error, env keys still work.

---

## 10. Testing

- **`provider-config` unit:** `parse_user_providers` (each type, missing/unknown fields);
  `parse_routing` (aliases/fallback/retry, malformed); `assemble` (merge, alias-folding,
  `credential_sources`, warnings); `MultiCredentialProvider.load` dispatch (keychain→env→none
  precedence; anthropic-oauth/api-key; copilot).
- **orchestrator unit:** chain-walking via the existing mock transport — advance on
  transient-exhausted, stop on terminal, chain-supersedes-built-in-fallback, per-entry retry budget,
  served-model marker; non-stream **and** stream-connect.
- **`secret` unit:** `set/get_provider_key` roundtrip.
- **engine-desktop integration:** `build()` with a `providers`+`routing` fixture → merged
  `ClientConfig`, chains on the adapter, availability correct; a mock-transport routed turn → right
  provider/model + cost.
- **`/connect` + picker:** API-key roundtrip; Copilot device-flow (mock `CopilotHttp` → token
  stored); Connect-badge render + select-launches-`/connect`.
- **Frozen-guard:** `traits` / `protocol` / `llm-client` untouched (the catalog is read-only; the
  composite is a consumer of `llm_client::CredentialProvider`).

---

## 11. Non-goals / deferred

- Wildcard / passthrough models for user providers (must declare `models`).
- engine-mobile interactive `/connect` UI (mobile uses env/settings keys; routing core is shared).
- OpenRouter ranking headers (`HTTP-Referer` / `X-Title`) — needs a provider header seam that does
  not exist yet.
- Runtime model discovery (the catalog stays a vendored models.dev snapshot).
- Per-provider key validation probes on `/connect`.
- The staged-but-unwired `orchestrator::model::telemetry.rs` / `count_tokens.rs` parity ports
  (separate wiring task).

---

## 12. Suggested implementation order (for the plan)

1. `secret`: `set/get_provider_key` (keychain) + tests.
2. `provider-config` crate: types + `parse_user_providers` + `parse_routing` + `assemble` + tests.
3. `provider-config`: `MultiCredentialProvider` + availability + tests.
4. orchestrator: `ProviderApiAdapter` chain-walking (non-stream then stream-connect) + served-model
   marker + tests.
5. engine `build()` (desktop, then mobile core): wire assemble → client + composite + cost + chains
   + availability + tests.
6. `/connect` command (commands-core + engine: API-key + Copilot device-flow host) + tests.
7. tui: `/connect` interactive UI + picker Connect badges + select-launches-`/connect` + tests.

Each step is a working, testable increment; routing is functional after step 5 (env keys), with the
full credential UX after step 7.
