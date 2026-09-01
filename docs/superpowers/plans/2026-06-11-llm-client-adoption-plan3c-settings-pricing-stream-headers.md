# Plan 3c — modelProviders settings wiring + PricingCatalog + streaming headers

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Fresh implementer per task, two-stage review, SEQUENTIAL edit-agents. Strict TDD.

**Goal:** Finish the llm-client adoption: (1) the engine's `providers`/`routing` settings actually configure `llm_client::ClientConfig` (today: parsed but consumed by nothing), (2) llm-client's `CostEstimator` runs off a catalog populated from the cost crate so `LlmResponse.cost` is real, (3) streaming responses carry true status/headers through an ADDITIVE `platform_api::HttpTransport` extension so connect-phase 429s honor server reset headers.

**Branch:** `worktree-llm-client-plan3c` (main @ c86af605). Cargo from `lingxi-code/`. NEVER `git add -A`. Clippy `-D warnings` (pedantic, --all-targets --no-deps). `traits/`+`protocol/` frozen — ADDITIVE ONLY (T1 adds; the diff vs main must show only additions). Commit trailer:
```
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

**Survey facts (verified):** engine schema `providers: Option<BTreeMap<String, Value>>` + `routing: Option<Value>` (schema.rs:155-168, deep-merge, doc says "Passed as raw JSON to llm_client::ClientConfig via the host build()"); MobileConfig has `provider_profiles`/`routing` fields (always None); DesktopConfig has NEITHER (only fallback_model, which works); hosts build `platform_common::llm_config::builtin_anthropic_config(api_base, oauth_path)` only. llm-client `ClientConfig{providers: Vec<ProviderProfile>}`, `ProviderProfile{provider_id, profile_name, base_url, protocol: ProtocolFamily, auth: AuthStrategy, credential: CredentialConfig, models: Vec<ModelProfile>, pricing: PricingConfig}`. cost crate: nano-USD/token (`MoneyPerToken.nano_usd_per_token`), `builtin_reference()` 35+ entries, `resolve()` 3-step; llm-client `TokenPricing` is per-million USD → conversion `usd_per_million = nano_usd_per_token as f64 / 1000.0`. `LlmTransportBridge::open_stream` hardcodes `status: 200, headers: BTreeMap::new()` on Ok (llm_transport.rs:124-127); error path has status but empty headers. Orchestrator `drive_stream` already reads `streaming.status/headers` on the connect-phase ≥400 path (provider_adapter.rs:516-549) and `resolve_retry_after` falls back to 1s when headers are empty — real headers immediately improve 429 delays. claude-code has NO providers/routing settings (LingXi extension — no TS parity constraint; `modelOverrides` is the closest analogue).

---

### Task 1: streaming metadata — additive traits extension + bridge + impls

**Files:** `platform-api/src/http.rs` (ADD ONLY), `platforms/common/src/http.rs` (ReqwestHttp override), `platforms/common/src/llm_transport.rs` (bridge uses it), `apps/engine-mobile/src/host.rs` (DynHttp forward), `platforms/posix-minimal` (default impl suffices — verify compile), tests.

1. **traits (additive):** add
```rust
/// SSE stream plus the response metadata that preceded it.
pub struct SseStreamWithMeta {
    /// HTTP status of the streaming response.
    pub status: u16,
    /// Response headers (lowercased names).
    pub headers: Vec<(String, String)>,
    /// The event stream.
    pub stream: SseStream,
}
```
and a defaulted trait method `async fn stream_sse_with_meta(&self, req: HttpRequest) -> Result<SseStreamWithMeta, HttpError> { Ok(SseStreamWithMeta { status: 200, headers: Vec::new(), stream: self.stream_sse(req).await? }) }` — existing impls keep compiling unchanged. Doc: default loses metadata; real transports should override.
2. **ReqwestHttp:** override `stream_sse_with_meta` capturing `resp.status()` + headers (lowercased) BEFORE draining frames; share the SSE-channel machinery with the existing `stream_sse` (refactor the body into a helper; `stream_sse` delegates or stays — zero behavior change to it). TDD via the crate's existing axum test harness: a test asserting status+headers (incl. a `retry-after`-style header) surface while events still arrive.
3. **DynHttp (engine-mobile):** forward `stream_sse_with_meta` to inner (one-line; without it the default would silently drop metadata for mobile).
4. **Bridge:** `LlmTransportBridge::open_stream` calls `stream_sse_with_meta`; populates `StreamingResponse{status, headers}` for real (headers Vec→BTreeMap lowercase). Error path `HttpError::Status` unchanged (headers stay empty there — reqwest's error arm has no headers; document). Update the stale "SseStream surfaces no response metadata" comments here + the provider_adapter.rs:469-475 gap note (now closed) + memory of llm_config doc if it mentions it.
5. **Orchestrator effect test:** provider_adapter test: fake `Transport` whose open_stream returns 429 + `retry-after: 7` headers → assert the connect-phase retry sleeps per header ladder (next_step RetryAfter with 7s, not the 1s fallback). (The adapter code path already exists — the test pins that real headers now drive it; use the existing fake-transport patterns.)
6. Gates: `cargo test -p platform-api -p platform-common -p engine-mobile -p orchestrator` 0 failed; clippy; `git diff main -- lingxi-code/platform-api` shows ONLY additions; workspace check.
7. Commit: `feat(traits,platform-common): streaming responses carry real status/headers (additive stream_sse_with_meta)`.

### Task 2: settings providers/routing → ClientConfig

**Files:** `platforms/common/src/llm_config.rs` (new parse/apply fns + tests), `apps/engine-desktop/src/lib.rs` (DesktopConfig fields + flow), `apps/engine-mobile/src/host.rs` (consume the existing fields), engine settings flow check (how DesktopConfig is populated from `engine` settings — READ the existing settings→DesktopConfig path and follow its pattern).

1. **platform-common (TDD first):**
```rust
pub fn apply_settings_providers(
    cfg: &mut ClientConfig,
    providers: &BTreeMap<String, serde_json::Value>,
    routing: Option<&serde_json::Value>,
) -> Result<(), llm_client::LlmError>
```
Per settings entry `{name: {"type": "openai"|"anthropic"|"gemini", "baseUrl": str, "apiKeyEnv": str, "models": [{"id": str, "aliases": [str]?, "capabilities": {...}?}]?}}` (schema doc's example shape) → `ProviderProfile{profile_name: name, provider_id: <map type→ProviderId variant — READ the enum; openai-compat custom name>, protocol: type→ProtocolFamily (openai→OpenAiChat, gemini→GeminiGenerateContent, anthropic→AnthropicMessages), auth: ApiKey, credential: Env{var: apiKeyEnv}, models: parsed or a sensible default-capability model from "models" REQUIRED (error if absent/empty — no guessing), pricing: PricingConfig::default()}` appended to cfg.providers. Unknown "type" → InvalidRequest naming it. routing handling: `routing.aliases: {alias: "profile/model"}` → push alias onto the TARGET ModelProfile (resolve `profile/model` across cfg.providers incl. builtin; unknown target → InvalidRequest); `routing.fallback`/`routing.retry` → NOT wired (return Ok but emit nothing; doc-comment: fallback_model comes from config/argv today, retry from CLAUDE_CODE_MAX_RETRIES — wiring them is future work, documented in the schema comment update). Tests: openai+gemini profile parse, env credential, alias injection (builtin + custom target), missing models error, unknown type error, duplicate profile name error.
2. **Desktop:** add `provider_profiles: Option<BTreeMap<String, Value>>` + `routing: Option<Value>` to DesktopConfig (serde defaults; mirror MobileConfig docs); FIND where DesktopConfig is built from engine settings (grep settings usage in apps/cli + engine-desktop; if DesktopConfig is built by the CLI host from `engine` settings schema, thread schema.providers/routing through — follow fallback_model's existing journey and mirror it). In the client construction block: after `builtin_anthropic_config`, call `apply_settings_providers` when fields are Some. Mobile: same call from its existing fields.
3. **Schema comment refresh** (core/src/settings/schema.rs:155-168): note aliases ARE wired; fallback/retry keys parsed-but-inert (documented).
4. Gates: tests on platform-common/engine-desktop/engine-mobile/cli + clippy + workspace check. An e2e-flavored test in engine-desktop: DesktopConfig with a groq-style openai profile → built client's `available_models()` includes the custom model and alias resolution works (construct via the same path build uses; no network).
5. Commit: `feat(apps,platform-common): wire providers/routing settings into ClientConfig (modelProviders)`.

### Task 3: PricingCatalog population + LlmResponse.cost

**Files:** `orchestrator/src/cost_wiring.rs` (bridge fn + adapter wiring) or platform-common (pick: orchestrator already deps cost + llm-client — put the bridge in cost_wiring), `orchestrator/src/provider_adapter.rs` (estimate at decode), `llm-client` only if a public seam is missing (check `CostEstimator`/`PricingCatalog` pub API: insert/with_price methods?).

1. READ llm-client `cost.rs` pub API (how to build a PricingCatalog: constructor/insert; PricingModelRef shape; CostEstimator::estimate signature + PricingPolicy) and cost crate `pricing.rs` (`builtin_reference()`, `ModelPricing.token_rates: HashMap<TokenClass, MoneyPerToken>`, iteration API over entries — if none pub, add a pub iterator/accessor to the COST crate (not frozen) with a test).
2. TDD bridge: `pub(crate) fn llm_catalog_from_cost(catalog: &cost::PricingCatalog) -> llm_client::PricingCatalog` mapping every entry: ModelRef→PricingModelRef (provider string + billing model name — READ both shapes; aliasing semantics documented), `usd_per_million = nano_usd_per_token as f64 / 1000.0` per bucket (Input/Output/CacheWrite/CacheRead/ReasoningOutput→reasoning_per_million; missing class → 0.0). Pin: opus tier converts to the exact expected per-million figure (compute from the cost crate's table — e.g. an entry with 15_000 nano/token input → 15.0 usd/M; use REAL table values).
3. Wire: adapter holds a `CostEstimator` (constructor param or built internally from `cost::PricingCatalog::builtin_reference()` via the bridge — prefer constructor param `Option<Arc<llm_client::CostEstimator>>`-style with hosts passing the builtin-derived one; check CostEstimator Send+Sync) and on successful decode populates `response.cost = Some(estimate)` mapped to llm-client's cost type (READ LlmResponse.cost type). Policy: estimator missing/unpriced model → cost stays None (never an error). CostTracker path UNTOUCHED (remains the budget authority). Tests: adapter returns response with cost populated for a priced model; None for unknown model; CostTracker recording unchanged (existing tests).
4. Hosts: pass the estimator (desktop+mobile) — small construction addition.
5. Gates: orchestrator/llm-client/cost/engine tests; clippy; workspace check.
6. Commit: `feat(orchestrator,cost): populate llm-client pricing from the cost catalog; LlmResponse.cost live`.

### Task 4: final gates + spec rev2.5 + review

1. `cargo build -p mock_stdio_mcp` then `cargo test --workspace --no-fail-fast` totals; 13-crate clippy battery + traits; `git diff main -- lingxi-code/traits lingxi-code/protocol` = additions in traits only, protocol empty; `cargo build -p cli -p engine-desktop -p engine-mobile`.
2. Spec rev2.5: 3c done; llm-client adoption COMPLETE; remaining future-work list (routing.fallback/retry keys inert; AwsSigV4/GcpToken signing; streaming error-path headers).
3. Final whole-plan review subagent; fix loop; finishing-a-development-branch.
