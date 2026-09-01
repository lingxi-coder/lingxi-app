# LLM Providers v2 — P7: Router (aliases/fallback/retry) + `/model` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** A core router over `ProviderRegistry` — model aliases, fallback chains, retry/backoff — plus accurate `/model` listing data.

**Architecture:** Aliases resolve as a string substitution at the top of `resolve()`. Fallback + retry are `LlmProvider` decorators (`FallbackProvider`, `RetryingProvider`) the registry composes per-resolve; `resolve()` returns the (possibly decorated) provider, so `ProviderApiAdapter` is unchanged. A `RoutingConfig` (aliases, fallback map, retry policy) is parsed from a settings `routing` object and held by the registry. `ModelRouter` gains a default-impl `available_models()` (only `ProviderRegistry` overrides it → no test-stub blast radius).

**Tech Stack:** Rust 1.82.0. Run cargo from `lingxi-code/`.

**Spec:** `2026-06-01-llm-providers-v2-design.md` §2.2, §3.6, §3.7. Branch `llm-providers-v2` (P6 done, tag `llm-v2-p6`).

**Bounded:** `is_retryable` = `Server{429|5xx}` ∪ `RateLimited` ∪ `UnexpectedStreamEnd` (conservative — never retries `InvalidRequest`/`Unauthorized`/`MalformedStream`). Fallover decides on the INITIAL `stream()`/`complete()` error only (no mid-stream switch). The `/model` handle integration is trace-first (the data layer ships regardless).

**Parity gate:** with no `routing` configured, `resolve()` behaves exactly as today (no alias, no decorators). Existing tests + `test-harness` parity stay green. Do NOT modify `traits/`.

---

## Task A: Router decorators + `RoutingConfig` + registry integration (providers crate)

**Files:** `providers/src/routing.rs` (new), `providers/Cargo.toml` (tokio `time`), `providers/src/registry.rs`, `providers/src/lib.rs`.

- [ ] **Step 1 — tokio `time` feature.** In `providers/Cargo.toml` `[dependencies]`, add `"time"` to the tokio features (currently `["sync"]` → `["sync", "time"]`).

- [ ] **Step 2 — `routing.rs`: decorators + config + `is_retryable`.**
```rust
//! Core router: model aliases, fallback chains, and retry/backoff, modeled as
//! `LlmProvider` decorators the registry composes. `resolve()` returns the
//! decorated provider, so the orchestrator adapter is unchanged.

use crate::capabilities::Capabilities;
use crate::provider::LlmProvider;
use crate::request::CanonicalRequest;
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use cost::ProviderId;
use futures::stream::BoxStream;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// Retry policy for transient failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Max total attempts (>= 1).
    pub max_attempts: u32,
    /// Base backoff between attempts (multiplied by the attempt number).
    pub backoff_ms: u64,
}

/// Router configuration parsed from the settings `routing` object.
#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// `alias -> "provider/model"`.
    pub aliases: BTreeMap<String, String>,
    /// `alias-or-"provider/model" -> ["provider/model", …]` fallback chain.
    pub fallback: BTreeMap<String, Vec<String>>,
    /// Optional retry policy applied to every resolved provider.
    pub retry: Option<RetryPolicy>,
}

/// Whether an error is worth retrying / failing over (transient).
#[must_use]
pub fn is_retryable(e: &ApiError) -> bool {
    matches!(e, ApiError::Server { status, .. } if *status == 429 || *status >= 500)
        || matches!(e, ApiError::RateLimited { .. } | ApiError::UnexpectedStreamEnd)
}

/// Retries `inner` on transient errors with linear backoff.
pub struct RetryingProvider {
    inner: Arc<dyn LlmProvider>,
    max_attempts: u32,
    backoff_ms: u64,
}

impl RetryingProvider {
    /// Wrap `inner` with a retry policy.
    #[must_use]
    pub fn new(inner: Arc<dyn LlmProvider>, max_attempts: u32, backoff_ms: u64) -> Self {
        Self { inner, max_attempts: max_attempts.max(1), backoff_ms }
    }
}

#[async_trait]
impl LlmProvider for RetryingProvider {
    fn id(&self) -> ProviderId { self.inner.id() }
    fn capabilities(&self) -> &Capabilities { self.inner.capabilities() }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.inner.complete(req.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) if attempt < self.max_attempts && is_retryable(&e) => {
                    tokio::time::sleep(Duration::from_millis(self.backoff_ms * u64::from(attempt))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.inner.stream(req.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) if attempt < self.max_attempts && is_retryable(&e) => {
                    tokio::time::sleep(Duration::from_millis(self.backoff_ms * u64::from(attempt))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Tries each `(provider, model)` in order, advancing on a transient error.
pub struct FallbackProvider {
    members: Vec<(Arc<dyn LlmProvider>, String)>,
}

impl FallbackProvider {
    /// Build from a non-empty member list (primary first). Each member carries
    /// its own provider-local model id (rebound onto the request before the call).
    #[must_use]
    pub fn new(members: Vec<(Arc<dyn LlmProvider>, String)>) -> Self {
        Self { members }
    }
}

#[async_trait]
impl LlmProvider for FallbackProvider {
    fn id(&self) -> ProviderId { self.members[0].0.id() }
    fn capabilities(&self) -> &Capabilities { self.members[0].0.capabilities() }

    async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
        let mut last = None;
        for (provider, model) in &self.members {
            let mut r = req.clone();
            r.model = model.clone();
            match provider.complete(r).await {
                Ok(resp) => return Ok(resp),
                Err(e) if is_retryable(&e) => { last = Some(e); }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| ApiError::MalformedStream("empty fallback chain".to_string())))
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let mut last = None;
        for (provider, model) in &self.members {
            let mut r = req.clone();
            r.model = model.clone();
            match provider.stream(r).await {
                Ok(s) => return Ok(s),
                Err(e) if is_retryable(&e) => { last = Some(e); }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| ApiError::MalformedStream("empty fallback chain".to_string())))
    }
}
```
Add a `#[cfg(test)] mod tests` with a mock `LlmProvider` (configurable to return a sequence of Ok/Err) and tests:
- `is_retryable` true for `Server{429}`/`Server{503}`/`RateLimited`/`UnexpectedStreamEnd`, false for `Unauthorized`/`MalformedStream`/`Server{400}`.
- `RetryingProvider` retries a `Server{503}`-then-`Ok` mock and succeeds; gives up after `max_attempts` on persistent `Server{503}`; does NOT retry `Unauthorized`.
- `FallbackProvider` advances to the 2nd member when the 1st returns `Server{503}`, returns its `Ok`; surfaces a terminal `Unauthorized` from the 1st without trying the 2nd.

- [ ] **Step 3 — `lib.rs`**: `pub mod routing;` + `pub use routing::{RoutingConfig, RetryPolicy, FallbackProvider, RetryingProvider};`.

- [ ] **Step 4 — `registry.rs`: integrate routing.**
Add a `routing: RoutingConfig` field to `ProviderRegistry`; change `new` to `pub fn new(profiles, env, transport, routing: RoutingConfig) -> Self`. Extract the current cached per-profile build into a helper `fn provider_for_profile(&self, profile_name: &str) -> Result<Arc<dyn LlmProvider>, ApiError>` (the existing cache-lookup-or-build logic, returning the bare provider). Rewrite `resolve`:
```rust
    fn resolve(&self, model: &str) -> Result<Resolved, ApiError> {
        // 1. alias substitution
        let target = self.routing.aliases.get(model).map_or(model, String::as_str);
        let spec = ModelSpec::parse(target);
        let primary = self.provider_for_profile(&spec.profile)?;

        // 2. fallback chain (keyed by the original selector or the alias target)
        let chain = self.routing.fallback.get(model).or_else(|| self.routing.fallback.get(target));
        let provider = if let Some(targets) = chain {
            let mut members: Vec<(Arc<dyn LlmProvider>, String)> = vec![(primary, spec.model.clone())];
            for t in targets {
                let s = ModelSpec::parse(t);
                members.push((self.provider_for_profile(&s.profile)?, s.model));
            }
            Arc::new(crate::routing::FallbackProvider::new(members)) as Arc<dyn LlmProvider>
        } else {
            primary
        };

        // 3. retry wrap
        let provider = if let Some(rp) = &self.routing.retry {
            Arc::new(crate::routing::RetryingProvider::new(provider, rp.max_attempts, rp.backoff_ms)) as Arc<dyn LlmProvider>
        } else {
            provider
        };

        Ok(Resolved { provider, model: spec.model })
    }
```
Add `available_models()` as a DEFAULT method on the `ModelRouter` trait returning `Vec::new()`, and OVERRIDE it on `ProviderRegistry` to return `profile/model` for each profile that declares a `models` list, plus alias names — for now (no per-profile `models` field exists yet) return `self.profiles.keys()` rendered as `"{profile}/"` placeholders PLUS the alias keys; KISS:
```rust
    fn available_models(&self) -> Vec<String> {
        let mut out: Vec<String> = self.routing.aliases.keys().cloned().collect();
        out.extend(self.profiles.keys().cloned());
        out.sort();
        out.dedup();
        out
    }
```
Update the `registry()` test helper to pass `RoutingConfig::default()`. Add tests: `alias_resolves_to_target` (alias `"fast" -> "openai/gpt-4o"`, `resolve("fast")` → model `"gpt-4o"`, id `OpenAI`); `no_routing_resolves_unchanged` (empty `RoutingConfig` → `resolve("claude-opus-4-7")` unchanged). (Fallback/retry behavior is covered by the routing.rs decorator tests; a registry-level fallback test is optional.)

- [ ] **Step 5 — gates + commit.**
```bash
cargo test -p providers
cargo clippy -p providers --no-deps --all-targets -- -D warnings
git add lingxi-code/providers/
git commit -m "feat(llm-v2 P7): router decorators (aliases/fallback/retry) + RoutingConfig

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B: Cross-crate wiring (settings `routing` + composition + `/model`)

**Files:** `core/src/settings/{schema.rs,merger.rs,tracer.rs}`, `apps/cli/src/init.rs`, `orchestrator/src/handle_impl.rs` (trace-first).

- [ ] **Step 1 — engine settings `routing` field (mirror `providers`).**
In `core/src/settings/schema.rs`: add `pub routing: Option<serde_json::Value>` to the settings struct (camelCase `routing`, `#[serde(...)]` matching the `providers` field's attributes), and add `("routing", MergeStrategy::DeepMerge)` to the merge-strategies list (next to `("providers", …)`). In `merger.rs` + `tracer.rs`, mirror whatever the `providers` field required (likely nothing beyond the strategy entry + a tracer provenance line — match the `providers` pattern exactly). Add a `routing_field_roundtrips` test mirroring `providers_field_roundtrips`.

- [ ] **Step 2 — `providers`: parse `RoutingConfig` from a settings Value.**
Add `pub fn parse_routing(raw: Option<&serde_json::Value>) -> RoutingConfig` to `routing.rs` (and re-export): parse `{ "aliases": {a: "p/m"}, "fallback": {k: ["p/m"]}, "retry": {"maxAttempts": n, "backoffMs": n} }` into `RoutingConfig`, tolerating missing keys (default empty). Add a `parse_routing_reads_aliases_fallback_retry` test.

- [ ] **Step 3 — `apps/cli/src/init.rs`: load routing + pass to registry.**
Add a `load_routing()` helper mirroring `load_provider_profiles()` (read the `routing` settings Value via the same loader path). Build `let routing = providers::parse_routing(routing_value.as_ref());` and pass it to `ProviderRegistry::new(profiles, env_snapshot, http.clone(), routing)`.

- [ ] **Step 4 — `/model` accuracy (TRACE-FIRST).**
Read `orchestrator/src/handle_impl.rs::list_available_models` and determine whether the handle can reach the resolved `ModelRouter`/`ProviderApiAdapter` to call `available_models()`.
  - **IF reachable in a bounded way:** have `list_available_models` return the router's `available_models()` (falling back to the current example list if empty).
  - **ELSE:** leave `list_available_models` as-is and add a one-line note (in the code or `docs/LLM_PROVIDERS.md`) that `/model` shows examples; accurate enumeration via `ModelRouter::available_models()` is wired where the registry is reachable. Do NOT change the frozen `traits` trait signature.
Record the trace finding in the commit message.

- [ ] **Step 5 — gates + commit.**
```bash
cargo test -p providers -p core -p orchestrator
cargo build --workspace
git add lingxi-code/core/ lingxi-code/apps/cli/ lingxi-code/orchestrator/ lingxi-code/providers/
git commit -m "feat(llm-v2 P7): wire routing config from settings + /model enumeration

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task C: Phase gates + tag

- [ ] **Step 1:** `cargo test -p providers -p core -p orchestrator -p test-harness` — all pass.
- [ ] **Step 2:** `cargo build --workspace` — Finished.
- [ ] **Step 3:** `cargo clippy -p providers -p core -p orchestrator --no-deps --all-targets -- -D warnings` — clean.
- [ ] **Step 4:** from `lingxi-code/`: `bash scripts/check-deps.sh` — OK 73 (no new deps).
- [ ] **Step 5:** `git tag -a llm-v2-p7 -m "LLM Providers v2 P7: router + /model"`.

---

## Self-Review

**Spec coverage (§2.2/§3.6/§3.7):** aliases (A4) ✓; `FallbackProvider`/`RetryingProvider` + `is_retryable` (A2) ✓; `RoutingConfig` + `parse_routing` (A2/B2) ✓; settings `routing` + composition wiring (B1/B3) ✓; `available_models()` data layer (A4) ✓; `/model` integration (B4, trace-first) ✓.

**Placeholder scan:** decorators + registry `resolve` refactor + config are fully paste-ready. The settings-field step says "mirror the `providers` field exactly" (a concrete, known pattern in the same files). The `/model` step is trace-first with explicit branches (not vague).

**Type consistency:** `RoutingConfig`/`RetryPolicy` consistent across routing.rs, registry (`new` param + `resolve`), parse_routing, and init.rs. `ProviderRegistry::new(profiles, env, transport, routing)` — the added 4th param ripples to init.rs (Step B3) + the registry test helper (Step A4); both are updated. `available_models()` default-impl on `ModelRouter` → no test-stub changes needed. `FallbackProvider::new(Vec<(Arc<dyn LlmProvider>, String)>)` consistent between routing.rs and the registry `resolve`.

**Blast-radius:** `ProviderRegistry::new` signature changes → `init.rs` + registry `registry()` test helper (both called out). `ModelRouter::available_models` is a DEFAULT method → `StubRouter`/`FixedRouter` test stubs need no change. No `ProviderProfile` field additions this phase.
