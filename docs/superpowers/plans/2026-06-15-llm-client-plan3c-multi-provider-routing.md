# Plan 3c — Multi-Provider Live Routing + modelProviders Settings — Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the LingXi engine actually route to the built-in catalog (OpenRouter/DeepSeek/GLM/Copilot), user-defined `settings.providers`, and `settings.routing` aliases + cross-provider fallback chains, with keychain-backed credentials and an interactive `/connect`.

**Architecture:** A new pure `provider-config` crate parses settings + merges `llm_client::builtin_presets()` into one `ClientConfig` + a composite credential provider (keychain→env, single slot, delegating Anthropic OAuth); chain-walking executes in the orchestrator's `ProviderApiAdapter`.

**Tech Stack:** Rust, llm-client, the `secret` keychain, tokio, the existing orchestrator retry driver.

---

## Conventions (read once)

- All `cargo` commands run from the workspace dir `/Users/luolingfeng/Projects/LingXi-Next-wt/llm-client-3a-resume/lingxi-code`. The volume runs near-full, so **every** test/build command is prefixed `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
- **Frozen crates** (§10 of the spec): `traits`, `protocol`, `llm-client` get NO additive code — with exactly ONE documented exception: a `#[doc(hidden)]` token reader on `CopilotSecret` in `llm-client` (Task 7), raised as an explicit frozen-crate deviation because the token cannot be surfaced anywhere else. `secret`, `cost`, `provider-config`, `orchestrator`, `command-core`, `engine-desktop`, `engine-mobile`, `tui` are all free to add seams.
- TDD loop per task: write the failing test → run & expect-fail → implement → run & expect-pass → commit. Complete Rust is given in every code step; concretize nothing further.

---

## File map

**`lingxi-code/secret/`** (Task 1)
- Modify: `src/credential.rs` — `set_provider_key` / `get_provider_key` + `provider_key_account` helper + `provider_key_tests`.

**`lingxi-code/provider-config/`** (NEW crate, Tasks 2–9, 12–13)
- Create: `Cargo.toml`, `src/lib.rs`, `src/types.rs`, `src/parse_providers.rs`, `src/parse_routing.rs`, `src/assemble.rs`, `src/credentials.rs`, `src/availability.rs`, `src/cost_translate.rs`.
- Modify: `lingxi-code/Cargo.toml` — add `provider-config` to `members` + `default-members`.

**`lingxi-code/cost/`** (Task 12)
- Modify: `src/pricing.rs` — add `pub fn with_entry(self, ModelPricing) -> Self`.

**`lingxi-code/orchestrator/`** (Tasks 10–11)
- Modify: `Cargo.toml` — add `provider-config` dep.
- Modify: `src/provider_adapter.rs` — `chains` field + `new` param + `chain_for` accessor + `failover_worthy` + outer chain loops (non-stream + stream) + served-model stamp.

**`lingxi-code/llm-client/`** (Task 7 — the ONE frozen-crate exception)
- Modify: `src/copilot/auth.rs` — `#[doc(hidden)] pub fn token_for_storage(&self) -> &str` on `CopilotSecret`.

**`lingxi-code/commands/core/`** (Task 16)
- Create: `src/connect.rs`.
- Modify: `src/lib.rs` (module + re-exports), `src/register.rs` (`register_core_connect`).

**`lingxi-code/apps/engine-desktop/`** (Tasks 13–15, 17–18)
- Modify: `Cargo.toml` — add `provider-config` dep.
- Create: `src/connect.rs` — `EngineCredentialWriter`, `SecureKeyPrompt`, `PosixCopilotHttp`, `EngineCopilotConnect`, `PollSleeper`.
- Modify: `src/lib.rs` — `DesktopRuntime` fields, `build()` rewrite, `desktop_command_registry` signature, `DesktopConfig.connect_prompt`, `mod connect`.

**`lingxi-code/apps/engine-mobile/`** (Tasks 10, 13–14)
- Modify: `Cargo.toml` — `provider-config` optional dep under the `uniffi` feature.
- Modify: `src/host.rs` — `build_mobile_inner` rewrite.

**`lingxi-code/tui/`** (Tasks 19–25)
- Modify: `src/screens/model.rs` (`ModelRow.available`, `build_model_entries`, `[Connect]` badge, `ModelOutcome::Connect`), `src/screens/mod.rs` (`Screen::Connect`), `src/state.rs` (`pending_connect`, `pending_store_key`, `provider_availability` App field, `open_connect`), `src/root.rs` (`pump_open_model` map, `pump_open_connect`, `Screen::Connect` key arm), `src/app.rs` (availability App field threading).
- Create: `src/screens/connect.rs`.

---

## Cross-cutting reconciliations applied (decided here, not left to the executor)

1. **`AssembleInputs` canonical field names** (owned by Task 6): `anthropic_api_base`, `anthropic_models: Vec<llm_client::ModelProfile>`, `anthropic_has_api_key`, `anthropic_has_oauth`, `user_providers`, `routing`. There is **no** `default_model`/`fallback_model` field — the Anthropic models (including the host fallback) are pre-resolved by the caller into `anthropic_models`.
2. **`Assembled.pricing` is `cost::PricingCatalog`** (NOT `llm_client::PricingCatalog`). `llm_client::PricingCatalog` has private `prices`/`overrides` and no public iterator (verified `llm-client/src/cost.rs:47-50`), so it cannot be translated. Task 6 builds a `cost::PricingCatalog` from `cost::PricingCatalog::builtin_reference()` + `with_entry` rows for non-Anthropic models (Task 12 adds the mutator). There is **no `CostEstimator` wiring and no `merge_llm_pricing_into_cost`** — those types/functions do not exist.
3. **`ProviderApiAdapter::new` takes `chains: ChainConfig` as a 7th positional arg** (Task 10); a `pub fn chain_for(&self, key: &str) -> Vec<ChainEntry>` read accessor is added. There is **no `.with_chains(..)` builder.** All call sites pass `chains` positionally.
4. **`CredentialSource` carries `pub profile_name: String`** (Task 4); availability + the engine availability map key by `profile_name`.
5. **`parse_routing` returns a 3-tuple** `(ChainConfig, raw_fallback: BTreeMap<String, Vec<String>>, warnings: Vec<String>)`; `assemble` validates `raw_fallback` into `chains.chains`.
6. **`CredentialProvider::load` returns `Result<Credential, LlmError>`** (no `Option`); a missing credential is `Err(LlmError::Authentication)`.
7. **Model-less user providers are dropped + warned** (`ModelRegistry::from_config` rejects zero-model profiles, registry.rs:50). Spec §5.1 "listing-only (not routable)" is **deferred** — a listing-only path that bypasses the registry is out of scope.
8. **The engine availability map is threaded into the tui App** as a new non-frozen `App` field (Task 23); the picker reads it (Task 20) instead of an empty map.
9. **The desktop integration / registry tests reuse `orchestrator::test_support::RecordingPermissionSink`** (verified it exists and impls `client_adapter::PermissionRequestSink::emit_request`); no new `test_support` module is added to engine-desktop.

---

## Task list

### Task 1: `secret` — `CredentialManager::set_provider_key` / `get_provider_key` keychain roundtrip

Adds generic per-provider key storage to `CredentialManager`, keyed by credential id, backed by the same OS keychain (`SecureStorage` service `"lingxi"`) the Anthropic API key + OAuth tokens use. Mirrors `store_anthropic_api_key` / `get_anthropic_api_key` (`secret/src/credential.rs:114-149`) but derives the account from the id and labels it with the already-existing `SecretKind::GenericApiKey { provider }` (`kinds.rs:39-43`). Returns the generic `protocol::Secret<String>`. Provider keys deliberately bypass `api_key_cache` (that slot holds only the single Anthropic key) so `/connect` takes effect live with no restart (spec §6.3). The methods are `async` + `Result<_, CredentialError>` (the spec §4.2 `-> Option<Secret>` shorthand is the fallible+async real shape).

**Files:**
- Modify: `lingxi-code/secret/src/credential.rs`

- [ ] **Step 1: Write the failing test** — append a new module to the bottom of `lingxi-code/secret/src/credential.rs` (after the existing `oauth_tests` module). The in-memory harness is `mod`-private in `oauth_tests` and cannot be imported, so it is re-declared verbatim.

```rust
#[cfg(test)]
mod provider_key_tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use traits::SecureStorageBackend;

    #[derive(Default)]
    struct MemStorage {
        map: StdMutex<HashMap<(String, String), SecureStorageData>>,
    }

    #[async_trait]
    impl SecureStorage for MemStorage {
        async fn store(
            &self,
            service: &str,
            account: &str,
            data: SecureStorageData,
        ) -> Result<(), SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<SecureStorageData>, SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(&(service.into(), account.into()))
                .cloned())
        }
        async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .keys()
                .filter(|(s, _)| s == service)
                .map(|(_, a)| a.clone())
                .collect())
        }
        fn is_encrypted(&self) -> bool {
            false
        }
        fn backend(&self) -> SecureStorageBackend {
            SecureStorageBackend::PlainText
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
        }
    }

    struct NoHttp;
    #[async_trait]
    impl HttpTransport for NoHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
    }

    fn manager() -> (Arc<MemStorage>, CredentialManager) {
        let storage = Arc::new(MemStorage::default());
        let cm = CredentialManager::new(
            storage.clone() as Arc<dyn SecureStorage>,
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        );
        (storage, cm)
    }

    #[tokio::test]
    async fn provider_key_round_trips() {
        let (_storage, cm) = manager();
        cm.set_provider_key("openrouter", "sk-or-secret").await.expect("set");
        let got = cm.get_provider_key("openrouter").await.expect("get").expect("present");
        assert_eq!(got.expose_secret(), "sk-or-secret");
    }

    #[tokio::test]
    async fn get_provider_key_returns_none_when_absent() {
        let (_storage, cm) = manager();
        assert!(cm.get_provider_key("deepseek").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn set_provider_key_overwrites_previous() {
        let (_storage, cm) = manager();
        cm.set_provider_key("glm-coding", "old-key").await.expect("first set");
        cm.set_provider_key("glm-coding", "new-key").await.expect("second set");
        let got = cm.get_provider_key("glm-coding").await.expect("get").expect("present");
        assert_eq!(got.expose_secret(), "new-key");
    }

    #[tokio::test]
    async fn provider_keys_are_isolated_by_id() {
        let (_storage, cm) = manager();
        cm.set_provider_key("openrouter", "key-a").await.expect("set a");
        cm.set_provider_key("deepseek", "key-b").await.expect("set b");
        assert_eq!(
            cm.get_provider_key("openrouter").await.expect("get a").expect("present a").expose_secret(),
            "key-a"
        );
        assert_eq!(
            cm.get_provider_key("deepseek").await.expect("get b").expect("present b").expose_secret(),
            "key-b"
        );
    }

    #[tokio::test]
    async fn provider_key_persisted_under_lingxi_service_with_generic_kind() {
        let (storage, cm) = manager();
        cm.set_provider_key("github-copilot", "ghu_token").await.expect("set");
        let raw = storage
            .retrieve("lingxi", "provider-key-github-copilot")
            .await
            .expect("retrieve")
            .expect("present");
        assert_eq!(raw.expose_secret_bytes(), b"ghu_token");
        let expected_kind = SecretKind::GenericApiKey {
            provider: "github-copilot".to_string(),
        }
        .as_dto();
        assert_eq!(raw.metadata.kind, expected_kind);
    }
}
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p secret provider_key`  Expected: FAIL to compile — `no method named set_provider_key` / `get_provider_key`.

- [ ] **Step 3: Implement** — add the two methods to `impl CredentialManager` (after `store_anthropic_api_key`, ending at line 149), plus the free helper near the top-level `const` block (after `OAUTH_META_ACCOUNT`, line 26):

```rust
    /// Persist a per-provider API key (or bearer token) in [`SecureStorage`],
    /// keyed by credential `id`. Stored under the shared `service = "lingxi"`
    /// keychain at an account namespaced by `id` (`provider-key-<id>`), labelled
    /// with [`SecretKind::GenericApiKey`]. Overwrites any existing entry for `id`
    /// so re-running `/connect` rotates the key. Not cached: the composite reads
    /// the keychain live so a freshly connected key takes effect on the next
    /// request without a restart.
    pub async fn set_provider_key(&self, id: &str, secret: &str) -> Result<(), CredentialError> {
        let metadata = SecureStorageMetadata {
            created_at: self.clock.now(),
            last_accessed: None,
            kind: SecretKind::GenericApiKey {
                provider: id.to_string(),
            }
            .as_dto(),
        };
        let data = SecureStorageData::new(secret.as_bytes().to_vec(), metadata);
        self.storage
            .store("lingxi", &provider_key_account(id), data)
            .await?;
        Ok(())
    }

    /// Load the per-provider key stored under credential `id`. Returns `Ok(None)`
    /// when no key has been stored (the composite then falls through to env, then
    /// `Authentication`).
    pub async fn get_provider_key(
        &self,
        id: &str,
    ) -> Result<Option<Secret<String>>, CredentialError> {
        let Some(raw) = self
            .storage
            .retrieve("lingxi", &provider_key_account(id))
            .await?
        else {
            return Ok(None);
        };
        let s = String::from_utf8(raw.expose_secret_bytes().to_vec())
            .map_err(|_| CredentialError::Unavailable)?;
        Ok(Some(Secret::new(s)))
    }
```

```rust
/// Keychain account name for a per-provider key, namespaced by credential `id`.
fn provider_key_account(id: &str) -> String {
    format!("provider-key-{id}")
}
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p secret provider_key` (5 pass) then `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p secret` (existing `oauth_tests` still pass).

- [ ] **Step 5: Commit** — `git add lingxi-code/secret/src/credential.rs && git commit -m "feat(secret): per-provider keychain key storage (set/get_provider_key)"`

---

### Task 2: Scaffold the `provider-config` crate (full dep set)

Creates the new pure crate as a workspace member with the COMPLETE dependency set every later provider-config task needs, so no later task edits `Cargo.toml` again.

**Files:**
- Create: `lingxi-code/provider-config/Cargo.toml`, `lingxi-code/provider-config/src/lib.rs`
- Modify: `lingxi-code/Cargo.toml` (workspace `members` + `default-members`)

- [ ] **Step 1: Write the failing test** — write the smoke test as the new `lib.rs` body:

```rust
//! Plan 3c — settings→config assembly for multi-provider live routing.
//!
//! Pure, no-I/O parsing + assembly of `settings.providers` / `settings.routing`
//! into an `llm_client::ClientConfig` + cross-provider fallback `ChainConfig` +
//! a composite credential provider. See
//! `docs/superpowers/specs/2026-06-15-llm-client-plan3c-multi-provider-routing-design.md`.

#![forbid(unsafe_code)]

#[cfg(test)]
mod smoke_tests {
    #[test]
    fn links_llm_client() {
        let cat = llm_client::builtin_presets();
        assert_eq!(cat.providers.len(), 4);
    }
}
```

  Create `lingxi-code/provider-config/Cargo.toml` with the FULL dep set (later tasks add only modules):

```toml
[package]
name = "provider-config"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
llm-client = { path = "../llm-client" }
secret = { path = "../secret" }
protocol = { path = "../protocol" }
cost = { path = "../cost" }
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio = { workspace = true }
async-trait.workspace = true
traits = { path = "../traits" }

[lints]
workspace = true
```

  Add `"provider-config",` to BOTH the `members` and `default-members` lists in `lingxi-code/Cargo.toml`, right after the `"llm-client",` line in each:

```toml
    "llm-client",
    "provider-config",
    "orchestrator",
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config links_llm_client`  Expected (pre-create): `package ID specification 'provider-config' did not match any packages`.
- [ ] **Step 3: Implement** — the three files above ARE the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config links_llm_client`  Expected: PASS (1 test).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config lingxi-code/Cargo.toml lingxi-code/Cargo.lock && git commit -m "feat(provider-config): scaffold crate with full dep set (Plan 3c §4.1)"`

---

### Task 3: Shared types — `ChainConfig`, `ChainEntry`, `RetryOverride`, `CredentialKind`, `CredentialSource`, `Assembled`, `AssembleInputs`

These names are used VERBATIM by every downstream task. `CredentialSource` carries `pub profile_name` (reconciliation #4). `Assembled.pricing` is `cost::PricingCatalog` (reconciliation #2). `AssembleInputs` uses the canonical `anthropic_*` field names (reconciliation #1).

**Files:**
- Create: `lingxi-code/provider-config/src/types.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`pub mod types;` + re-exports)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/types.rs`:

```rust
//! Shared Plan 3c types: cross-provider fallback chains, retry overrides,
//! per-profile credential sources, and the `assemble` input/output bundles.
//! Spec §5.2 / §5.3.

use std::collections::BTreeMap;

use llm_client::{ClientConfig, ProviderId};

/// Per-model cross-provider failover entry (one hop in a `ChainConfig` chain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    /// Resolved provider identity for this hop.
    pub provider_id: ProviderId,
    /// Provider-local model id sent on the wire for this hop.
    pub model: String,
}

/// Retry budget override parsed from `settings.routing.retry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryOverride {
    /// Max total attempts per chain entry (clamped to >= 1 at parse time).
    pub max_attempts: u32,
    /// Base linear backoff in milliseconds (driver computes its own backoff;
    /// retained for parity / future custom-delay wiring).
    pub backoff_ms: u64,
}

/// Parsed routing config: aliases, fallback chains, and the retry override.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainConfig {
    /// `alias -> "provider/model"` (folded into model aliases by `assemble`).
    pub aliases: BTreeMap<String, String>,
    /// `key -> ordered failover entries` (`key` is an alias or `"provider/model"`).
    pub chains: BTreeMap<String, Vec<ChainEntry>>,
    /// Optional per-entry retry budget override.
    pub retry: Option<RetryOverride>,
}

/// Credential kind recorded per profile in `Assembled.credential_sources`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// Provider API key (keychain or env).
    ApiKey,
    /// Anthropic OAuth (delegated to the host OAuth provider).
    OAuth,
    /// Keychain-only (no env fallback).
    Keychain,
}

/// One profile's resolved credential source (keychain id + env fallback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSource {
    /// Provider identity this source serves.
    pub provider_id: ProviderId,
    /// Human profile name (== `ProviderProfile::profile_name`); the availability
    /// map keys by this (spec §6.2).
    pub profile_name: String,
    /// Keychain key id (== the profile's `CredentialConfig::Static{id}`).
    pub credential_id: String,
    /// Env fallback var (from `apiKeyEnv` / preset default), when any.
    pub env_var: Option<String>,
    /// Credential kind for this profile.
    pub kind: CredentialKind,
}

/// Inputs to `assemble` (spec §5.3). Carries the Anthropic 3-way auth state +
/// the raw settings JSON the engine already holds.
#[derive(Debug, Clone)]
pub struct AssembleInputs {
    /// Anthropic base URL (`cfg.api_base`).
    pub anthropic_api_base: String,
    /// Anthropic model ids to declare on the Anthropic profile (pre-resolved by
    /// the caller via `anthropic_models_for`, including the host fallback model).
    pub anthropic_models: Vec<llm_client::ModelProfile>,
    /// Whether an Anthropic API key is configured (api-key wins over OAuth).
    pub anthropic_has_api_key: bool,
    /// Whether an Anthropic OAuth session is available (only when no api key).
    pub anthropic_has_oauth: bool,
    /// Raw `settings.providers` block (`cfg.provider_profiles`).
    pub user_providers: BTreeMap<String, serde_json::Value>,
    /// Raw `settings.routing` block (`cfg.routing`).
    pub routing: Option<serde_json::Value>,
}

/// Result of `assemble` (spec §5.3).
#[derive(Debug)]
pub struct Assembled {
    /// Merged provider profiles (anthropic + presets + user) ready for the client.
    pub client_config: ClientConfig,
    /// Cost pricing catalog (Anthropic/OpenAI/Gemini reference tiers + non-Anthropic
    /// rows), ready to wrap in `Arc` and pass to `cost::CostTracker::new`.
    pub pricing: cost::PricingCatalog,
    /// Parsed + validated routing chains.
    pub chains: ChainConfig,
    /// Per-profile credential sources for the composite provider + availability.
    pub credential_sources: Vec<CredentialSource>,
    /// Non-fatal parse/merge warnings (surfaced via tracing by the engine).
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_config_default_is_empty() {
        let c = ChainConfig::default();
        assert!(c.aliases.is_empty());
        assert!(c.chains.is_empty());
        assert!(c.retry.is_none());
    }

    #[test]
    fn chain_entry_carries_provider_and_model() {
        let e = ChainEntry {
            provider_id: ProviderId::OpenAICompatible { name: "deepseek".to_string() },
            model: "deepseek-chat".to_string(),
        };
        assert_eq!(e.model, "deepseek-chat");
        assert_eq!(e.provider_id, ProviderId::OpenAICompatible { name: "deepseek".to_string() });
    }

    #[test]
    fn credential_source_records_profile_and_env_fallback() {
        let s = CredentialSource {
            provider_id: ProviderId::OpenAICompatible { name: "openrouter".to_string() },
            profile_name: "openrouter".to_string(),
            credential_id: "openrouter".to_string(),
            env_var: Some("OPENROUTER_API_KEY".to_string()),
            kind: CredentialKind::ApiKey,
        };
        assert_eq!(s.profile_name, "openrouter");
        assert_eq!(s.env_var.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(s.kind, CredentialKind::ApiKey);
    }
}
```

  Replace `lingxi-code/provider-config/src/lib.rs` body (keep the crate doc + `#![forbid(unsafe_code)]` + the smoke test):

```rust
//! Plan 3c — settings→config assembly for multi-provider live routing.
//!
//! Pure, no-I/O parsing + assembly of `settings.providers` / `settings.routing`
//! into an `llm_client::ClientConfig` + cross-provider fallback `ChainConfig` +
//! a composite credential provider. See
//! `docs/superpowers/specs/2026-06-15-llm-client-plan3c-multi-provider-routing-design.md`.

#![forbid(unsafe_code)]

pub mod types;

pub use types::{
    AssembleInputs, Assembled, ChainConfig, ChainEntry, CredentialKind, CredentialSource,
    RetryOverride,
};

#[cfg(test)]
mod smoke_tests {
    #[test]
    fn links_llm_client() {
        let cat = llm_client::builtin_presets();
        assert_eq!(cat.providers.len(), 4);
    }
}
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config types::`  Expected: `file not found for module 'types'` until `types.rs` exists (write both in this task).
- [ ] **Step 3: Implement** — the `types.rs` non-test content IS the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config types::`  Expected: PASS (3 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): shared types (ChainConfig/CredentialSource/Assembled/AssembleInputs) (Plan 3c §5.2/§5.3)"`

---

### Task 4: `parse_user_providers` — `settings.providers` → profiles + warnings (spec §5.1)

Maps each `{ "type", "baseUrl", "apiKeyEnv"?, "models"? }` entry to an `llm_client::ProviderProfile` (fields from `llm-client/src/config.rs:16-41`). `parse_user_providers` itself sets `credential: CredentialConfig::None` (a placeholder; `assemble` rewrites to `Static{id}` uniformly per §5.4). Type mapping (§5.1): `openai`→`OpenAiChat`+`Bearer`+`OpenAICompatible{name}`; `anthropic`→`AnthropicMessages`+`ApiKey`+`Custom{name}`; `gemini`→`GeminiGenerateContent`+`ApiKey`+`Custom{name}` (Gemini is `Custom`, NOT first-party `Gemini`, per the §5.1 table). Unknown `type` / missing `baseUrl` / non-object → skip + warn.

**Files:**
- Create: `lingxi-code/provider-config/src/parse_providers.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`pub mod parse_providers;` + re-export)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/parse_providers.rs`:

```rust
//! Parse `settings.providers` into `llm_client::ProviderProfile`s (spec §5.1).
//! Narrowed to openai|anthropic|gemini with skip+warn (non-fatal) on bad entries.

use std::collections::BTreeMap;

use llm_client::{
    AuthStrategy, Capabilities, CredentialConfig, ModelProfile, PricingConfig, ProtocolFamily,
    ProviderId, ProviderProfile,
};
use serde_json::Value;

/// A parsed user profile plus the `apiKeyEnv` recorded for its credential source.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUserProvider {
    /// The built provider profile (credential is a placeholder `None`; `assemble`
    /// rewrites it to `Static{id}` per spec §5.4).
    pub profile: ProviderProfile,
    /// The `apiKeyEnv` env fallback recorded by `assemble` into `CredentialSource`.
    pub env_var: Option<String>,
}

/// Parse `settings.providers` into profiles + warnings (spec §5.1). Unknown
/// `type` / missing `baseUrl` skips the entry with a warning. A model-less entry
/// parses with no models; `assemble` decides routability (it drops + warns).
#[must_use]
pub fn parse_user_providers(
    providers: &BTreeMap<String, Value>,
) -> (Vec<ParsedUserProvider>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for (name, value) in providers {
        let Some(obj) = value.as_object() else {
            warnings.push(format!("provider {name:?}: entry is not an object; skipped"));
            continue;
        };
        let Some(type_str) = obj.get("type").and_then(Value::as_str) else {
            warnings.push(format!("provider {name:?}: missing string \"type\"; skipped"));
            continue;
        };
        let Some(base_url) = obj.get("baseUrl").and_then(Value::as_str) else {
            warnings.push(format!("provider {name:?}: missing \"baseUrl\"; skipped"));
            continue;
        };

        let (protocol, auth, provider_id) = match type_str {
            "openai" => (
                ProtocolFamily::OpenAiChat,
                AuthStrategy::Bearer,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "anthropic" => (
                ProtocolFamily::AnthropicMessages,
                AuthStrategy::ApiKey,
                ProviderId::Custom { name: name.clone() },
            ),
            "gemini" => (
                ProtocolFamily::GeminiGenerateContent,
                AuthStrategy::ApiKey,
                ProviderId::Custom { name: name.clone() },
            ),
            other => {
                warnings.push(format!(
                    "provider {name:?}: unknown type {other:?} (expected openai|anthropic|gemini); skipped"
                ));
                continue;
            }
        };

        let env_var = obj.get("apiKeyEnv").and_then(Value::as_str).map(str::to_string);

        let models = obj
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str())
                    .map(|id| ModelProfile {
                        display_model: id.to_string(),
                        request_model: id.to_string(),
                        billing_model: id.to_string(),
                        aliases: Vec::new(),
                        capabilities: permissive_caps(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(ParsedUserProvider {
            profile: ProviderProfile {
                provider_id,
                profile_name: name.clone(),
                base_url: base_url.to_string(),
                protocol,
                auth,
                credential: CredentialConfig::None,
                models,
                pricing: PricingConfig::default(),
            },
            env_var,
        });
    }

    (out, warnings)
}

/// Permissive capabilities for a user-declared model (spec §5.1) so preflight
/// does not reject.
fn permissive_caps() -> Capabilities {
    Capabilities {
        streaming: true,
        tools: true,
        vision: true,
        documents: true,
        reasoning: true,
        structured_output: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(name: &str, v: Value) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        m.insert(name.to_string(), v);
        m
    }

    #[test]
    fn openai_maps_to_openai_chat_bearer_compatible() {
        let raw = one(
            "groq",
            json!({ "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY", "models": ["llama-3.3-70b"] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.profile.profile_name, "groq");
        assert_eq!(p.profile.provider_id, ProviderId::OpenAICompatible { name: "groq".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::OpenAiChat);
        assert_eq!(p.profile.auth, AuthStrategy::Bearer);
        assert_eq!(p.profile.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(p.env_var.as_deref(), Some("GROQ_API_KEY"));
        assert_eq!(p.profile.models.len(), 1);
        assert_eq!(p.profile.models[0].request_model, "llama-3.3-70b");
        assert_eq!(p.profile.models[0].billing_model, "llama-3.3-70b");
        assert_eq!(p.profile.models[0].display_model, "llama-3.3-70b");
        assert!(p.profile.models[0].capabilities.tools);
    }

    #[test]
    fn anthropic_maps_to_anthropic_messages_apikey_custom() {
        let raw = one(
            "myclaude",
            json!({ "type": "anthropic", "baseUrl": "https://proxy.example/anthropic", "models": ["claude-x"] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::Custom { name: "myclaude".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::AnthropicMessages);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
        assert_eq!(p.env_var, None);
    }

    #[test]
    fn gemini_maps_to_gemini_apikey_custom() {
        let raw = one(
            "g",
            json!({ "type": "gemini", "baseUrl": "https://generativelanguage.googleapis.com/v1beta", "models": ["gemini-2.5-pro"] }),
        );
        let (parsed, _warns) = parse_user_providers(&raw);
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::Custom { name: "g".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::GeminiGenerateContent);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
    }

    #[test]
    fn unknown_type_is_skipped_with_warning() {
        let raw = one("weird", json!({ "type": "cohere", "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("unknown type"));
        assert!(warns[0].contains("weird"));
    }

    #[test]
    fn missing_base_url_is_skipped_with_warning() {
        let raw = one("nb", json!({ "type": "openai", "models": ["m"] }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("baseUrl"));
    }

    #[test]
    fn non_object_entry_is_skipped_with_warning() {
        let raw = one("bad", json!("just-a-string"));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("not an object"));
    }

    #[test]
    fn missing_type_is_skipped_with_warning() {
        let raw = one("nt", json!({ "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("type"));
    }

    #[test]
    fn model_less_entry_parses_with_no_models() {
        let raw = one("listingonly", json!({ "type": "openai", "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].profile.models.is_empty());
    }
}
```

  Wire into `lingxi-code/provider-config/src/lib.rs` (after `pub mod types;`): `pub mod parse_providers;` and add to the `pub use` block: `pub use parse_providers::{parse_user_providers, ParsedUserProvider};`

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config parse_providers::`  Expected: `file not found for module 'parse_providers'` (pre-write).
- [ ] **Step 3: Implement** — the non-test portion above IS the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config parse_providers::`  Expected: PASS (8 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): parse_user_providers — openai/anthropic/gemini, skip+warn (Plan 3c §5.1)"`

---

### Task 5: `parse_routing` — `settings.routing` → `(ChainConfig, raw_fallback, warnings)` (spec §5.2)

Parses `aliases` + `fallback` + `retry`. `fallback` values stay raw `"provider/model"` strings (validated into `ChainEntry`s only in `assemble`, which has the registered profiles). Returns the 3-tuple `(ChainConfig{aliases, chains:empty, retry}, raw_fallback: BTreeMap<String, Vec<String>>, warnings)` (reconciliation #5). Retry defaults: `maxAttempts` 3, `backoffMs` 250, `max_attempts` clamped to >= 1.

**Files:**
- Create: `lingxi-code/provider-config/src/parse_routing.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`pub mod parse_routing;` + re-export)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/parse_routing.rs`:

```rust
//! Parse `settings.routing` into a partial `ChainConfig` + a raw fallback map
//! (spec §5.2). Fallback target strings stay raw here; `assemble` validates them
//! into `ChainEntry`s.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::types::{ChainConfig, RetryOverride};

/// Default total attempts when `routing.retry.maxAttempts` is absent.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// Default base backoff when `routing.retry.backoffMs` is absent.
const DEFAULT_BACKOFF_MS: u64 = 250;

/// Parse `settings.routing`. Returns the partial `ChainConfig` (aliases + retry;
/// `chains` left empty for `assemble`), the raw `key -> ["provider/model", …]`
/// fallback map, and collected warnings.
#[must_use]
pub fn parse_routing(
    routing: Option<&Value>,
) -> (ChainConfig, BTreeMap<String, Vec<String>>, Vec<String>) {
    let mut cfg = ChainConfig::default();
    let mut raw_fallback: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut warnings = Vec::new();

    let Some(obj) = routing.and_then(Value::as_object) else {
        return (cfg, raw_fallback, warnings);
    };

    if let Some(aliases) = obj.get("aliases").and_then(Value::as_object) {
        for (k, v) in aliases {
            if let Some(s) = v.as_str() {
                cfg.aliases.insert(k.clone(), s.to_string());
            } else {
                warnings.push(format!("routing.aliases[{k:?}]: value is not a string; skipped"));
            }
        }
    }

    if let Some(fb) = obj.get("fallback").and_then(Value::as_object) {
        for (k, v) in fb {
            let Some(arr) = v.as_array() else {
                warnings.push(format!("routing.fallback[{k:?}]: value is not an array; skipped"));
                continue;
            };
            let targets: Vec<String> =
                arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            if targets.is_empty() {
                warnings.push(format!("routing.fallback[{k:?}]: no string targets; skipped"));
                continue;
            }
            raw_fallback.insert(k.clone(), targets);
        }
    }

    if let Some(retry) = obj.get("retry").and_then(Value::as_object) {
        let max_attempts = retry
            .get("maxAttempts")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(DEFAULT_MAX_ATTEMPTS)
            .max(1);
        let backoff_ms = retry
            .get("backoffMs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_BACKOFF_MS);
        cfg.retry = Some(RetryOverride { max_attempts, backoff_ms });
    }

    (cfg, raw_fallback, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn none_routing_yields_empty_config() {
        let (cfg, fb, warns) = parse_routing(None);
        assert!(cfg.aliases.is_empty());
        assert!(cfg.chains.is_empty());
        assert!(cfg.retry.is_none());
        assert!(fb.is_empty());
        assert!(warns.is_empty());
    }

    #[test]
    fn parses_aliases() {
        let v = json!({ "aliases": { "fast": "deepseek/deepseek-chat", "smart": "openrouter/openai/gpt-4o" } });
        let (cfg, _fb, warns) = parse_routing(Some(&v));
        assert!(warns.is_empty());
        assert_eq!(cfg.aliases.get("fast").map(String::as_str), Some("deepseek/deepseek-chat"));
        assert_eq!(cfg.aliases.get("smart").map(String::as_str), Some("openrouter/openai/gpt-4o"));
    }

    #[test]
    fn parses_fallback_into_raw_map() {
        let v = json!({ "fallback": { "fast": ["deepseek/deepseek-chat", "openrouter/openai/gpt-4o-mini"] } });
        let (cfg, fb, warns) = parse_routing(Some(&v));
        assert!(warns.is_empty());
        assert!(cfg.chains.is_empty());
        assert_eq!(fb.get("fast").unwrap().len(), 2);
        assert_eq!(fb.get("fast").unwrap()[0], "deepseek/deepseek-chat");
    }

    #[test]
    fn retry_defaults_and_overrides() {
        let (cfg, _fb, _w) = parse_routing(Some(&json!({ "retry": {} })));
        let r = cfg.retry.unwrap();
        assert_eq!(r.max_attempts, 3);
        assert_eq!(r.backoff_ms, 250);

        let (cfg2, _fb2, _w2) = parse_routing(Some(&json!({ "retry": { "maxAttempts": 5, "backoffMs": 100 } })));
        let r2 = cfg2.retry.unwrap();
        assert_eq!(r2.max_attempts, 5);
        assert_eq!(r2.backoff_ms, 100);
    }

    #[test]
    fn retry_zero_attempts_clamped_to_one() {
        let (cfg, _fb, _w) = parse_routing(Some(&json!({ "retry": { "maxAttempts": 0 } })));
        assert_eq!(cfg.retry.unwrap().max_attempts, 1);
    }

    #[test]
    fn malformed_alias_value_warns_and_skips() {
        let (cfg, _fb, warns) = parse_routing(Some(&json!({ "aliases": { "fast": 42 } })));
        assert!(cfg.aliases.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("fast"));
    }

    #[test]
    fn malformed_fallback_value_warns_and_skips() {
        let (_cfg, fb, warns) = parse_routing(Some(&json!({ "fallback": { "fast": "not-an-array" } })));
        assert!(fb.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("not an array"));
    }

    #[test]
    fn empty_fallback_targets_warns_and_skips() {
        let (_cfg, fb, warns) = parse_routing(Some(&json!({ "fallback": { "fast": [1, 2, 3] } })));
        assert!(fb.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("no string targets"));
    }
}
```

  Wire into `lib.rs` (after `pub mod parse_providers;`): `pub mod parse_routing;` and add to the `pub use` block: `pub use parse_routing::parse_routing;`

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config parse_routing::`  Expected: module not found (pre-write).
- [ ] **Step 3: Implement** — the non-test portion IS the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config parse_routing::`  Expected: PASS (8 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): parse_routing — aliases/fallback/retry 3-tuple (Plan 3c §5.2)"`

---

### Task 6: `assemble` — merge anthropic + presets + user providers, fold aliases, validate chains, emit credential sources (spec §5.3)

Produces the full `Assembled`. Steps: (1) Anthropic profile from the 3-way auth state (mirrors `engine_desktop::anthropic_profile`, lib.rs:795). (2) `builtin_presets().providers`: rewrite each preset's `CredentialConfig::Env{var}` → `Static{id = profile_name}` + record `CredentialSource`. (3) `parse_user_providers`; routable (>=1 model) providers get `Static{id}` + a source; **model-less providers are dropped + warned** (`ModelRegistry::from_config` rejects zero-model profiles — reconciliation #7). (4) fold `routing.aliases` into target `ModelProfile.aliases`. (5) validate `raw_fallback` into `ChainEntry` lists. (6) `pricing`: set to `cost::PricingCatalog::builtin_reference()` here — **Task 12 (cost group) swaps this single line** for `crate::cost_translate::pricing_for(&providers)` once the `with_entry` mutator + translator land.

**Files:**
- Create: `lingxi-code/provider-config/src/assemble.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`pub mod assemble;` + re-export)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/assemble.rs`:

```rust
//! Assemble the merged `ClientConfig` + pricing + chains + credential sources
//! from Anthropic state + `settings.providers` + `settings.routing` (spec §5.3).

use llm_client::{
    AuthStrategy, ClientConfig, CredentialConfig, PricingConfig, ProtocolFamily, ProviderId,
    ProviderProfile,
};

use crate::parse_providers::parse_user_providers;
use crate::parse_routing::parse_routing;
use crate::types::{AssembleInputs, Assembled, ChainEntry, CredentialKind, CredentialSource};

/// Build the Anthropic provider profile from the 3-way auth state. Mirrors the
/// engine `anthropic_profile` (`apps/engine-desktop/src/lib.rs:795`).
fn anthropic_profile(inputs: &AssembleInputs) -> (ProviderProfile, Option<CredentialSource>) {
    let (auth, credential, cred_source) = if inputs.anthropic_has_api_key {
        (
            AuthStrategy::ApiKey,
            CredentialConfig::Static { id: "anthropic-api-key".to_string() },
            Some(CredentialSource {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "anthropic".to_string(),
                credential_id: "anthropic-api-key".to_string(),
                env_var: Some("ANTHROPIC_API_KEY".to_string()),
                kind: CredentialKind::ApiKey,
            }),
        )
    } else if inputs.anthropic_has_oauth {
        (
            AuthStrategy::OAuthBearer,
            CredentialConfig::Static { id: "anthropic-oauth".to_string() },
            Some(CredentialSource {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "anthropic".to_string(),
                credential_id: "anthropic-oauth".to_string(),
                env_var: None,
                kind: CredentialKind::OAuth,
            }),
        )
    } else {
        (AuthStrategy::None, CredentialConfig::None, None)
    };

    let profile = ProviderProfile {
        provider_id: ProviderId::AnthropicFirstParty,
        profile_name: "anthropic".to_string(),
        base_url: inputs.anthropic_api_base.clone(),
        protocol: ProtocolFamily::AnthropicMessages,
        auth,
        credential,
        models: inputs.anthropic_models.clone(),
        pricing: PricingConfig::default(),
    };
    (profile, cred_source)
}

/// Locate `(provider_idx, model_idx)` for a `"profile_name/model"` target.
fn locate(providers: &[ProviderProfile], target: &str) -> Option<(usize, usize)> {
    let (prof, model) = target.split_once('/')?;
    for (pi, p) in providers.iter().enumerate() {
        if p.profile_name != prof {
            continue;
        }
        for (mi, m) in p.models.iter().enumerate() {
            if m.request_model == model || m.display_model == model || m.billing_model == model {
                return Some((pi, mi));
            }
        }
    }
    None
}

/// Assemble the full multi-provider config (spec §5.3).
#[must_use]
pub fn assemble(inputs: AssembleInputs) -> Assembled {
    let mut warnings = Vec::new();
    let mut providers: Vec<ProviderProfile> = Vec::new();
    let mut credential_sources: Vec<CredentialSource> = Vec::new();

    // 1. Anthropic profile.
    let (anthropic, anthropic_cred) = anthropic_profile(&inputs);
    providers.push(anthropic);
    if let Some(cs) = anthropic_cred {
        credential_sources.push(cs);
    }

    // 2. Built-in presets: rewrite Env -> Static{id = profile_name}.
    let catalog = llm_client::builtin_presets();
    for mut preset in catalog.providers {
        let env_var = match &preset.credential {
            CredentialConfig::Env { var } => Some(var.clone()),
            _ => None,
        };
        let id = preset.profile_name.clone();
        preset.credential = CredentialConfig::Static { id: id.clone() };
        credential_sources.push(CredentialSource {
            provider_id: preset.provider_id.clone(),
            profile_name: preset.profile_name.clone(),
            credential_id: id,
            env_var,
            kind: CredentialKind::ApiKey,
        });
        providers.push(preset);
    }

    // 3. User providers (only routable ones — must declare >= 1 model).
    let (parsed_users, user_warns) = parse_user_providers(&inputs.user_providers);
    warnings.extend(user_warns);
    for pu in parsed_users {
        if pu.profile.models.is_empty() {
            warnings.push(format!(
                "provider {:?}: declares no models; dropped (not routable — declare \"models\" or a routing alias)",
                pu.profile.profile_name
            ));
            continue;
        }
        let id = pu.profile.profile_name.clone();
        let mut profile = pu.profile;
        profile.credential = CredentialConfig::Static { id: id.clone() };
        credential_sources.push(CredentialSource {
            provider_id: profile.provider_id.clone(),
            profile_name: profile.profile_name.clone(),
            credential_id: id,
            env_var: pu.env_var,
            kind: CredentialKind::ApiKey,
        });
        providers.push(profile);
    }

    // 4. Routing: aliases (fold) + fallback (validate).
    let (mut chains, raw_fallback, routing_warns) = parse_routing(inputs.routing.as_ref());
    warnings.extend(routing_warns);

    for (alias, target) in &chains.aliases {
        match locate(&providers, target) {
            Some((pi, mi)) => {
                let aliases = &mut providers[pi].models[mi].aliases;
                if !aliases.iter().any(|a| a == alias) {
                    aliases.push(alias.clone());
                }
            }
            None => warnings.push(format!(
                "routing.aliases[{alias:?}]: target {target:?} matches no registered provider/model; skipped"
            )),
        }
    }

    // 5. Validate fallback chains into ChainEntry lists.
    for (key, targets) in raw_fallback {
        let mut entries: Vec<ChainEntry> = Vec::new();
        for target in &targets {
            match locate(&providers, target) {
                Some((pi, mi)) => entries.push(ChainEntry {
                    provider_id: providers[pi].provider_id.clone(),
                    model: providers[pi].models[mi].request_model.clone(),
                }),
                None => warnings.push(format!(
                    "routing.fallback[{key:?}]: entry {target:?} matches no registered provider/model; skipped"
                )),
            }
        }
        if entries.is_empty() {
            warnings.push(format!("routing.fallback[{key:?}]: no valid entries; chain dropped"));
        } else {
            chains.chains.insert(key, entries);
        }
    }

    // 6. Pricing. Task 12 (cost group) swaps this line for
    //    `crate::cost_translate::pricing_for(&providers)` once with_entry lands.
    let pricing = cost::PricingCatalog::builtin_reference();

    Assembled {
        client_config: ClientConfig { providers },
        pricing,
        chains,
        credential_sources,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::ModelProfile;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn anthropic_only_inputs() -> AssembleInputs {
        AssembleInputs {
            anthropic_api_base: "https://api.anthropic.com".to_string(),
            anthropic_models: vec![ModelProfile {
                display_model: "claude-opus-4-6".to_string(),
                request_model: "claude-opus-4-6".to_string(),
                billing_model: "claude-opus-4-6".to_string(),
                aliases: Vec::new(),
                capabilities: llm_client::Capabilities::default(),
            }],
            anthropic_has_api_key: true,
            anthropic_has_oauth: false,
            user_providers: BTreeMap::new(),
            routing: None,
        }
    }

    #[test]
    fn merges_anthropic_and_presets() {
        let out = assemble(anthropic_only_inputs());
        assert_eq!(out.client_config.providers.len(), 5);
        let names: Vec<&str> = out
            .client_config
            .providers
            .iter()
            .map(|p| p.profile_name.as_str())
            .collect();
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"openrouter"));
        assert!(names.contains(&"deepseek"));
        assert!(names.contains(&"glm-coding"));
        assert!(names.contains(&"github-copilot"));
    }

    #[test]
    fn presets_env_rewritten_to_static_with_profile_name() {
        let out = assemble(anthropic_only_inputs());
        let openrouter = out
            .client_config
            .providers
            .iter()
            .find(|p| p.profile_name == "openrouter")
            .unwrap();
        assert_eq!(openrouter.credential, CredentialConfig::Static { id: "openrouter".to_string() });
        let cs = out.credential_sources.iter().find(|c| c.credential_id == "openrouter").unwrap();
        assert_eq!(cs.profile_name, "openrouter");
        assert_eq!(cs.env_var.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(cs.kind, CredentialKind::ApiKey);
    }

    #[test]
    fn anthropic_api_key_credential_source() {
        let out = assemble(anthropic_only_inputs());
        let cs = out.credential_sources.iter().find(|c| c.credential_id == "anthropic-api-key").unwrap();
        assert_eq!(cs.profile_name, "anthropic");
        assert_eq!(cs.env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(cs.kind, CredentialKind::ApiKey);
        let anthropic = &out.client_config.providers[0];
        assert_eq!(anthropic.auth, AuthStrategy::ApiKey);
        assert_eq!(anthropic.credential, CredentialConfig::Static { id: "anthropic-api-key".to_string() });
    }

    #[test]
    fn anthropic_oauth_path() {
        let mut inp = anthropic_only_inputs();
        inp.anthropic_has_api_key = false;
        inp.anthropic_has_oauth = true;
        let out = assemble(inp);
        let anthropic = &out.client_config.providers[0];
        assert_eq!(anthropic.auth, AuthStrategy::OAuthBearer);
        assert_eq!(anthropic.credential, CredentialConfig::Static { id: "anthropic-oauth".to_string() });
        let cs = out.credential_sources.iter().find(|c| c.credential_id == "anthropic-oauth").unwrap();
        assert_eq!(cs.kind, CredentialKind::OAuth);
        assert!(cs.env_var.is_none());
    }

    #[test]
    fn anthropic_none_path_has_no_credential() {
        let mut inp = anthropic_only_inputs();
        inp.anthropic_has_api_key = false;
        inp.anthropic_has_oauth = false;
        let out = assemble(inp);
        let anthropic = &out.client_config.providers[0];
        assert_eq!(anthropic.auth, AuthStrategy::None);
        assert_eq!(anthropic.credential, CredentialConfig::None);
        assert!(out
            .credential_sources
            .iter()
            .all(|c| c.credential_id != "anthropic-api-key" && c.credential_id != "anthropic-oauth"));
    }

    #[test]
    fn user_provider_added_with_static_credential() {
        let mut inp = anthropic_only_inputs();
        inp.user_providers.insert(
            "groq".to_string(),
            json!({ "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY", "models": ["llama-3.3-70b"] }),
        );
        let out = assemble(inp);
        let groq = out.client_config.providers.iter().find(|p| p.profile_name == "groq").unwrap();
        assert_eq!(groq.credential, CredentialConfig::Static { id: "groq".to_string() });
        let cs = out.credential_sources.iter().find(|c| c.credential_id == "groq").unwrap();
        assert_eq!(cs.env_var.as_deref(), Some("GROQ_API_KEY"));
    }

    #[test]
    fn model_less_user_provider_dropped_with_warning() {
        let mut inp = anthropic_only_inputs();
        inp.user_providers.insert(
            "listingonly".to_string(),
            json!({ "type": "openai", "baseUrl": "https://x" }),
        );
        let out = assemble(inp);
        assert!(out.client_config.providers.iter().all(|p| p.profile_name != "listingonly"));
        assert!(out.warnings.iter().any(|w| w.contains("listingonly") && w.contains("no models")));
    }

    #[test]
    fn alias_folded_into_target_model() {
        let mut inp = anthropic_only_inputs();
        inp.routing = Some(json!({ "aliases": { "boss": "anthropic/claude-opus-4-6" } }));
        let out = assemble(inp);
        let anthropic = &out.client_config.providers[0];
        assert!(anthropic.models[0].aliases.iter().any(|a| a == "boss"));
    }

    #[test]
    fn unknown_alias_target_warns() {
        let mut inp = anthropic_only_inputs();
        inp.routing = Some(json!({ "aliases": { "x": "nope/missing" } }));
        let out = assemble(inp);
        assert!(out.warnings.iter().any(|w| w.contains("x") && w.contains("nope/missing")));
    }

    #[test]
    fn fallback_validated_into_chain() {
        let mut inp = anthropic_only_inputs();
        inp.user_providers.insert(
            "deepseek-user".to_string(),
            json!({ "type": "openai", "baseUrl": "https://api.deepseek.com", "models": ["deepseek-chat"] }),
        );
        inp.routing = Some(json!({
            "fallback": { "primary": ["anthropic/claude-opus-4-6", "deepseek-user/deepseek-chat"] }
        }));
        let out = assemble(inp);
        let chain = out.chains.chains.get("primary").unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].provider_id, ProviderId::AnthropicFirstParty);
        assert_eq!(chain[0].model, "claude-opus-4-6");
        assert_eq!(chain[1].provider_id, ProviderId::OpenAICompatible { name: "deepseek-user".to_string() });
        assert_eq!(chain[1].model, "deepseek-chat");
    }

    #[test]
    fn fallback_unknown_entry_skipped_chain_kept() {
        let mut inp = anthropic_only_inputs();
        inp.routing = Some(json!({ "fallback": { "primary": ["anthropic/claude-opus-4-6", "ghost/none"] } }));
        let out = assemble(inp);
        let chain = out.chains.chains.get("primary").unwrap();
        assert_eq!(chain.len(), 1);
        assert!(out.warnings.iter().any(|w| w.contains("ghost/none")));
    }

    #[test]
    fn fallback_all_unknown_chain_dropped() {
        let mut inp = anthropic_only_inputs();
        inp.routing = Some(json!({ "fallback": { "primary": ["ghost/a", "ghost/b"] } }));
        let out = assemble(inp);
        assert!(out.chains.chains.get("primary").is_none());
        assert!(out.warnings.iter().any(|w| w.contains("chain dropped")));
    }

    #[test]
    fn anthropic_builtins_priced_through_assembled_catalog() {
        // The assembled pricing is a cost::PricingCatalog with Anthropic tiers.
        let out = assemble(anthropic_only_inputs());
        let mr = cost::ModelRef {
            provider: cost::pricing::ProviderId::Anthropic,
            model: "claude-opus-4-6".to_string(),
        };
        assert!(out.pricing.resolve(&mr).is_ok());
    }
}
```

  Wire into `lib.rs` (after `pub mod parse_routing;`): `pub mod assemble;` and add to the `pub use` block: `pub use assemble::assemble;`

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config assemble::`  Expected: module not found (pre-write).
- [ ] **Step 3: Implement** — the non-test portion IS the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config assemble::`  Expected: PASS (14 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): assemble — merge anthropic+presets+user, fold aliases, validate chains, emit credential sources (Plan 3c §5.3)"`

---

### Task 7: `CopilotSecret::token_for_storage` — the ONE documented frozen-crate (§10) exception

**This is the single explicit deviation from the §10 frozen-guard on `llm-client`.** `CopilotSecret` (`llm-client/src/copilot/auth.rs:17-26`) wraps the GitHub token with a redacting `Debug` and exposes NO reader. `PollOutcome::Success(CopilotSecret)` (login.rs:50) is the only place the device-flow token surfaces, and the token MUST be persisted by the engine `/connect` Copilot driver (Task 15) under `github-copilot`. There is nowhere else legal to read it. We add a `#[doc(hidden)]` accessor, explicitly labelled as a frozen-crate exception in its doc comment, so the deviation is auditable and intentional (NOT a silent edit). No other llm-client change is permitted by this plan.

**Files:**
- Modify: `lingxi-code/llm-client/src/copilot/auth.rs`

- [ ] **Step 1: Write the failing test** — add to the existing `#[cfg(test)] mod tests` in `auth.rs`:

```rust
    #[test]
    fn token_for_storage_returns_raw_token_for_persistence() {
        // Frozen-crate (§10) exception: the /connect device-flow MUST persist the
        // GitHub token under `github-copilot`. The Debug stays redacting; only
        // this explicit, doc-hidden accessor exposes the raw token.
        let s = CopilotSecret::new("ght_live_token");
        assert_eq!(s.token_for_storage(), "ght_live_token");
        // Debug is still redacting (no regression).
        assert!(!format!("{s:?}").contains("ght_live_token"));
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client token_for_storage`  Expected: FAIL to compile — `no method named token_for_storage`.

- [ ] **Step 3: Implement** — add the accessor to the `impl CopilotSecret` block (after `new`, in `auth.rs`):

```rust
    /// **Plan 3c frozen-crate (§10) EXCEPTION — documented deviation.** Expose the
    /// raw GitHub OAuth token so the host `/connect` device-flow driver can persist
    /// it to the keychain under `github-copilot`. This is the ONLY reader; the
    /// `Debug` impl stays redacting. `#[doc(hidden)]` so it is not part of the
    /// public surface and is only reachable by the engine that already drives the
    /// Copilot login. Do not use for logging or display.
    #[doc(hidden)]
    #[must_use]
    pub fn token_for_storage(&self) -> &str {
        &self.0
    }
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client copilot::auth` (the new test + the existing `debug_does_not_leak_token` still pass).

- [ ] **Step 5: Commit** — `git add lingxi-code/llm-client/src/copilot/auth.rs && git commit -m "feat(llm-client): CopilotSecret::token_for_storage — documented §10 frozen-crate exception for /connect persistence"`

---

### Task 8: `MultiCredentialProvider` — composite single slot (dispatch + keychain→env→Authentication)

The composite holds `Arc<CredentialManager>`, the per-`credential_id` `CredentialSource` map, the Anthropic API key, and an optional OAuth delegate. It impls `llm_client::CredentialProvider`. `load` dispatches on `scope.credential_id`: `"anthropic-oauth"` → delegate, `"anthropic-api-key"` → configured key, else → keychain[id] → env[recorded var] → `Err(LlmError::Authentication)` (reconciliation #6, matching `EnvCredentialProvider`, credentials.rs:111-121). Copilot's `GITHUB_TOKEN` rides the same non-anthropic branch. `new` arg order: `(manager, sources, anthropic_api_key, oauth_delegate)`.

> The test `NoHttp` double implements BOTH required `traits::HttpTransport` methods (`request` AND `stream_sse`) — the trait has two required methods (http.rs:42-47); the unused one panics.

**Files:**
- Create: `lingxi-code/provider-config/src/credentials.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`mod credentials; pub use credentials::MultiCredentialProvider;`)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/credentials.rs` with the impl below AND this `#[cfg(test)] mod tests` (Tasks 8+9 of the original drafts are merged into one `mod tests` here for the composite; the keychain/env/oauth cases are all included):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{Credential, CredentialProvider, CredentialScope, LlmError, ProviderId};
    use std::sync::Arc;

    /// Shared in-memory CredentialManager harness (re-used by availability.rs too).
    #[derive(Default)]
    struct MemStorage {
        map: std::sync::Mutex<
            std::collections::HashMap<(String, String), protocol::SecureStorageData>,
        >,
    }
    #[async_trait::async_trait]
    impl traits::SecureStorage for MemStorage {
        async fn store(&self, service: &str, account: &str, data: protocol::SecureStorageData)
            -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(&self, service: &str, account: &str)
            -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().get(&(service.into(), account.into())).cloned())
        }
        async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().keys().filter(|(s, _)| s == service).map(|(_, a)| a.clone()).collect())
        }
        fn is_encrypted(&self) -> bool { false }
        fn backend(&self) -> traits::SecureStorageBackend { traits::SecureStorageBackend::PlainText }
    }
    struct FixedClock;
    impl traits::Clock for FixedClock {
        fn now(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000)
        }
    }
    struct NoHttp;
    #[async_trait::async_trait]
    impl traits::HttpTransport for NoHttp {
        async fn request(&self, _req: protocol::HttpRequest)
            -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
        async fn stream_sse(&self, _req: protocol::HttpRequest)
            -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("credential tests must not perform HTTP");
        }
    }
    fn manager() -> Arc<secret::CredentialManager> {
        Arc::new(secret::CredentialManager::new(
            Arc::new(MemStorage::default()),
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        ))
    }
    fn source(provider_id: ProviderId, credential_id: &str, env_var: Option<&str>, kind: crate::CredentialKind)
        -> crate::CredentialSource {
        crate::CredentialSource {
            provider_id,
            profile_name: credential_id.to_string(),
            credential_id: credential_id.to_string(),
            env_var: env_var.map(str::to_string),
            kind,
        }
    }
    fn scope(provider: ProviderId, profile: &str, cred_id: &str) -> CredentialScope {
        CredentialScope::new(provider, profile).with_credential_id(cred_id.to_string())
    }

    /// Serialize env mutation across tests (process-global env).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn anthropic_api_key_dispatch_returns_configured_key() {
        let provider = MultiCredentialProvider::new(manager(), Vec::new(), Some("sk-ant-test".to_string()), None);
        let got = provider
            .load(&scope(ProviderId::AnthropicFirstParty, "anthropic", "anthropic-api-key"))
            .await
            .expect("api-key dispatch");
        assert_eq!(got, Credential::ApiKey("sk-ant-test".to_string()));
    }

    #[tokio::test]
    async fn anthropic_api_key_dispatch_missing_key_is_authentication_error() {
        let provider = MultiCredentialProvider::new(manager(), Vec::new(), None, None);
        let err = provider
            .load(&scope(ProviderId::AnthropicFirstParty, "anthropic", "anthropic-api-key"))
            .await
            .expect_err("no key configured");
        assert_eq!(err, LlmError::Authentication);
    }

    #[tokio::test]
    async fn missing_credential_id_is_authentication_error() {
        let provider = MultiCredentialProvider::new(manager(), Vec::new(), None, None);
        let err = provider
            .load(&CredentialScope::new(ProviderId::AnthropicFirstParty, "anthropic"))
            .await
            .expect_err("no credential id");
        assert_eq!(err, LlmError::Authentication);
    }

    #[tokio::test]
    async fn keychain_wins_over_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cm = manager();
        cm.set_provider_key("openrouter", "key-from-keychain").await.expect("store");
        std::env::set_var("OPENROUTER_API_KEY", "key-from-env");
        let provider = MultiCredentialProvider::new(
            cm,
            vec![source(ProviderId::OpenAICompatible { name: "openrouter".to_string() }, "openrouter", Some("OPENROUTER_API_KEY"), crate::CredentialKind::Keychain)],
            None, None,
        );
        let got = provider
            .load(&scope(ProviderId::OpenAICompatible { name: "openrouter".to_string() }, "openrouter", "openrouter"))
            .await
            .expect("resolve");
        std::env::remove_var("OPENROUTER_API_KEY");
        assert_eq!(got, Credential::ApiKey("key-from-keychain".to_string()));
    }

    #[tokio::test]
    async fn env_used_when_keychain_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPSEEK_API_KEY", "key-from-env");
        let provider = MultiCredentialProvider::new(
            manager(),
            vec![source(ProviderId::OpenAICompatible { name: "deepseek".to_string() }, "deepseek", Some("DEEPSEEK_API_KEY"), crate::CredentialKind::ApiKey)],
            None, None,
        );
        let got = provider
            .load(&scope(ProviderId::OpenAICompatible { name: "deepseek".to_string() }, "deepseek", "deepseek"))
            .await
            .expect("resolve");
        std::env::remove_var("DEEPSEEK_API_KEY");
        assert_eq!(got, Credential::ApiKey("key-from-env".to_string()));
    }

    #[tokio::test]
    async fn none_when_neither_keychain_nor_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GLM_NO_SUCH_VAR");
        let provider = MultiCredentialProvider::new(
            manager(),
            vec![source(ProviderId::Custom { name: "glm-coding".to_string() }, "glm-coding", Some("GLM_NO_SUCH_VAR"), crate::CredentialKind::ApiKey)],
            None, None,
        );
        let err = provider
            .load(&scope(ProviderId::Custom { name: "glm-coding".to_string() }, "glm-coding", "glm-coding"))
            .await
            .expect_err("nothing configured");
        assert_eq!(err, LlmError::Authentication);
    }

    #[tokio::test]
    async fn copilot_via_github_token_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GITHUB_TOKEN", "ghp-token");
        let provider = MultiCredentialProvider::new(
            manager(),
            vec![source(ProviderId::OpenAICompatible { name: "github-copilot".to_string() }, "github-copilot", Some("GITHUB_TOKEN"), crate::CredentialKind::Keychain)],
            None, None,
        );
        let got = provider
            .load(&scope(ProviderId::OpenAICompatible { name: "github-copilot".to_string() }, "github-copilot", "github-copilot"))
            .await
            .expect("resolve copilot");
        std::env::remove_var("GITHUB_TOKEN");
        assert_eq!(got, Credential::ApiKey("ghp-token".to_string()));
    }

    #[derive(Debug)]
    struct StubOAuth { token: String }
    impl CredentialProvider for StubOAuth {
        fn load<'a>(&'a self, _scope: &'a CredentialScope)
            -> llm_client::BoxFuture<'a, Result<Credential, LlmError>> {
            let tok = self.token.clone();
            Box::pin(async move { Ok(Credential::BearerToken(tok)) })
        }
    }

    #[tokio::test]
    async fn oauth_id_delegates_to_delegate() {
        let delegate: Arc<dyn CredentialProvider> = Arc::new(StubOAuth { token: "oauth-access".to_string() });
        let provider = MultiCredentialProvider::new(manager(), Vec::new(), None, Some(delegate));
        let got = provider
            .load(&scope(ProviderId::AnthropicFirstParty, "anthropic", "anthropic-oauth"))
            .await
            .expect("delegate");
        assert_eq!(got, Credential::BearerToken("oauth-access".to_string()));
    }

    #[tokio::test]
    async fn oauth_id_without_delegate_is_authentication_error() {
        let provider = MultiCredentialProvider::new(manager(), Vec::new(), None, None);
        let err = provider
            .load(&scope(ProviderId::AnthropicFirstParty, "anthropic", "anthropic-oauth"))
            .await
            .expect_err("no delegate");
        assert_eq!(err, LlmError::Authentication);
    }
}
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config credentials::tests`  Expected: fails to compile — `MultiCredentialProvider` does not exist.

- [ ] **Step 3: Implement** — the non-test body of `credentials.rs`:

```rust
//! Composite credential provider: the single `llm_client::CredentialProvider`
//! slot backing every routable profile.
//!
//! Dispatch (on `scope.credential_id`):
//! - `"anthropic-oauth"` → delegate to the engine-built OAuth provider.
//! - `"anthropic-api-key"` → the configured Anthropic key.
//! - any other id → keychain[id] → env[recorded var] → `Err(Authentication)`
//!   (matching `EnvCredentialProvider`; spec §6.1/§6.6).
//!
//! Secrets are returned as `Credential::ApiKey`; `DefaultLlmClient::load_secret`
//! extracts `ApiKey | BearerToken` uniformly and the profile's `AuthStrategy`
//! picks the header (so Copilot's `CopilotBearer` rides this path).

use std::collections::BTreeMap;
use std::sync::Arc;

use llm_client::{BoxFuture, Credential, CredentialProvider, CredentialScope, LlmError};

use crate::CredentialSource;

/// The single composite credential slot for all provider profiles.
pub struct MultiCredentialProvider {
    credentials: Arc<secret::CredentialManager>,
    sources: BTreeMap<String, CredentialSource>,
    anthropic_api_key: Option<String>,
    oauth_delegate: Option<Arc<dyn CredentialProvider>>,
}

impl std::fmt::Debug for MultiCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiCredentialProvider")
            .field("source_ids", &self.sources.keys().collect::<Vec<_>>())
            .field("has_anthropic_api_key", &self.anthropic_api_key.is_some())
            .field("has_oauth_delegate", &self.oauth_delegate.is_some())
            .finish()
    }
}

impl MultiCredentialProvider {
    /// Build the composite from the assembled `credential_sources`, an optional
    /// Anthropic API key, and an optional OAuth delegate.
    #[must_use]
    pub fn new(
        credentials: Arc<secret::CredentialManager>,
        sources: Vec<CredentialSource>,
        anthropic_api_key: Option<String>,
        oauth_delegate: Option<Arc<dyn CredentialProvider>>,
    ) -> Self {
        let sources = sources.into_iter().map(|s| (s.credential_id.clone(), s)).collect();
        Self { credentials, sources, anthropic_api_key, oauth_delegate }
    }

    /// Resolve a non-Anthropic provider key: keychain[id] → env[var] → Authentication.
    async fn load_provider_key(&self, credential_id: &str) -> Result<Credential, LlmError> {
        match self.credentials.get_provider_key(credential_id).await {
            Ok(Some(secret)) => return Ok(Credential::ApiKey(secret.expose_secret().clone())),
            Ok(None) => {}
            // Keychain unavailable (headless): fall through to env so env keys
            // still work; a truly-missing key surfaces as the per-turn 401.
            Err(_) => {}
        }
        if let Some(var) = self.sources.get(credential_id).and_then(|s| s.env_var.as_deref()) {
            if let Ok(val) = std::env::var(var) {
                return Ok(Credential::ApiKey(val));
            }
        }
        Err(LlmError::Authentication)
    }
}

impl CredentialProvider for MultiCredentialProvider {
    fn load<'a>(
        &'a self,
        scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        Box::pin(async move {
            let Some(credential_id) = scope.credential_id.as_deref() else {
                return Err(LlmError::Authentication);
            };
            match credential_id {
                "anthropic-oauth" => match &self.oauth_delegate {
                    Some(delegate) => delegate.load(scope).await,
                    None => Err(LlmError::Authentication),
                },
                "anthropic-api-key" => self
                    .anthropic_api_key
                    .clone()
                    .map(Credential::ApiKey)
                    .ok_or(LlmError::Authentication),
                other => self.load_provider_key(other).await,
            }
        })
    }
}
```

  Add to `lib.rs`: `mod credentials;` and `pub use credentials::MultiCredentialProvider;`

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config credentials::tests`  Expected: PASS (10 tests).

- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): MultiCredentialProvider composite (keychain>env>auth, anthropic key/oauth dispatch)"`

---

### Task 9: per-profile availability function (`compute_availability`)

Computes, per `CredentialSource`, `available = keychain_has(id) || env_set(var) || anthropic key/oauth present` (§6.2). The result is a sibling list the engine collapses into a `BTreeMap<profile_name, bool>` (reconciliation #4/#8). The keychain check uses `.await` + `matches!(.., Ok(Some(_)))` (the getter is async + `Result<Option<_>>` — NOT a sync `.is_some()`). The `NoHttp` double impls BOTH `request` AND `stream_sse`.

**Files:**
- Create: `lingxi-code/provider-config/src/availability.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`mod availability; pub use availability::{ProviderAvailability, compute_availability};`)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/availability.rs`:

```rust
//! Per-profile availability: drives the `/model` picker's Connect badge.
//!
//! `available = keychain_has(id) || env_set(var) || anthropic key/oauth present`.
//! A sibling list keyed by `profile_name` (spec §8) so the frozen `ModelListing`
//! DTO stays untouched; the tui joins it by provider/profile name.

use std::sync::Arc;

use llm_client::ProviderId;

use crate::CredentialSource;

/// Availability of one provider profile for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAvailability {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// Human profile name (the engine map keys by this; spec §6.2).
    pub profile_name: String,
    /// Credential id this availability was computed for.
    pub credential_id: String,
    /// Whether a usable credential is present.
    pub available: bool,
}

/// Compute availability for every `CredentialSource`.
///
/// `anthropic_has_api_key` / `anthropic_has_oauth` reflect the engine's resolved
/// Anthropic auth state (the composite serves those without a keychain/env id).
pub async fn compute_availability(
    credentials: &Arc<secret::CredentialManager>,
    sources: &[CredentialSource],
    anthropic_has_api_key: bool,
    anthropic_has_oauth: bool,
) -> Vec<ProviderAvailability> {
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        let available = match source.credential_id.as_str() {
            "anthropic-api-key" => anthropic_has_api_key,
            "anthropic-oauth" => anthropic_has_oauth,
            id => {
                let keychain_has = matches!(credentials.get_provider_key(id).await, Ok(Some(_)));
                let env_set = source
                    .env_var
                    .as_deref()
                    .is_some_and(|var| std::env::var(var).is_ok());
                keychain_has || env_set
            }
        };
        out.push(ProviderAvailability {
            provider_id: source.provider_id.clone(),
            profile_name: source.profile_name.clone(),
            credential_id: source.credential_id.clone(),
            available,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::ProviderId;
    use std::sync::Arc;

    #[derive(Default)]
    struct MemStorage {
        map: std::sync::Mutex<
            std::collections::HashMap<(String, String), protocol::SecureStorageData>,
        >,
    }
    #[async_trait::async_trait]
    impl traits::SecureStorage for MemStorage {
        async fn store(&self, service: &str, account: &str, data: protocol::SecureStorageData)
            -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(&self, service: &str, account: &str)
            -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().get(&(service.into(), account.into())).cloned())
        }
        async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
            self.map.lock().unwrap().remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
            Ok(self.map.lock().unwrap().keys().filter(|(s, _)| s == service).map(|(_, a)| a.clone()).collect())
        }
        fn is_encrypted(&self) -> bool { false }
        fn backend(&self) -> traits::SecureStorageBackend { traits::SecureStorageBackend::PlainText }
    }
    struct FixedClock;
    impl traits::Clock for FixedClock {
        fn now(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000)
        }
    }
    struct NoHttp;
    #[async_trait::async_trait]
    impl traits::HttpTransport for NoHttp {
        async fn request(&self, _req: protocol::HttpRequest)
            -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("availability tests must not perform HTTP");
        }
        async fn stream_sse(&self, _req: protocol::HttpRequest)
            -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("availability tests must not perform HTTP");
        }
    }
    fn manager() -> Arc<secret::CredentialManager> {
        Arc::new(secret::CredentialManager::new(
            Arc::new(MemStorage::default()),
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        ))
    }
    fn src(name: &str, env: Option<&str>) -> crate::CredentialSource {
        crate::CredentialSource {
            provider_id: ProviderId::OpenAICompatible { name: name.to_string() },
            profile_name: name.to_string(),
            credential_id: name.to_string(),
            env_var: env.map(str::to_string),
            kind: crate::CredentialKind::ApiKey,
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn keychain_makes_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let cm = manager();
        cm.set_provider_key("openrouter", "k").await.expect("store");
        let map = compute_availability(&cm, &[src("openrouter", Some("NOPE_VAR"))], false, false).await;
        let entry = map.iter().find(|a| a.profile_name == "openrouter").expect("entry");
        assert!(entry.available);
    }

    #[tokio::test]
    async fn env_makes_available() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPSEEK_AVAIL_VAR", "k");
        let map = compute_availability(&manager(), &[src("deepseek", Some("DEEPSEEK_AVAIL_VAR"))], false, false).await;
        std::env::remove_var("DEEPSEEK_AVAIL_VAR");
        assert!(map.iter().find(|a| a.profile_name == "deepseek").unwrap().available);
    }

    #[tokio::test]
    async fn neither_is_unavailable() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GLM_AVAIL_VAR");
        let map = compute_availability(&manager(), &[src("glm", Some("GLM_AVAIL_VAR"))], false, false).await;
        assert!(!map.iter().find(|a| a.profile_name == "glm").unwrap().available);
    }

    #[tokio::test]
    async fn anthropic_api_key_marks_anthropic_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let anthropic = crate::CredentialSource {
            provider_id: ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            credential_id: "anthropic-api-key".to_string(),
            env_var: None,
            kind: crate::CredentialKind::ApiKey,
        };
        let map = compute_availability(&manager(), &[anthropic], true, false).await;
        assert!(map.iter().find(|a| a.profile_name == "anthropic").unwrap().available);
    }

    #[tokio::test]
    async fn anthropic_oauth_marks_anthropic_available() {
        let _g = ENV_LOCK.lock().unwrap();
        let anthropic = crate::CredentialSource {
            provider_id: ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            credential_id: "anthropic-oauth".to_string(),
            env_var: None,
            kind: crate::CredentialKind::OAuth,
        };
        let map = compute_availability(&manager(), &[anthropic], false, true).await;
        assert!(map.iter().find(|a| a.profile_name == "anthropic").unwrap().available);
    }
}
```

  Add to `lib.rs`: `mod availability;` and `pub use availability::{compute_availability, ProviderAvailability};`

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config availability::tests`  Expected: fails to compile — `compute_availability` / `ProviderAvailability` do not exist.
- [ ] **Step 3: Implement** — the non-test portion IS the implementation.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config availability::tests`  Expected: PASS (5 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): per-profile availability (keychain|env|anthropic) keyed by profile_name"`

**Whole-crate gate (after Task 9):** `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config` (smoke 1 + types 3 + parse_providers 8 + parse_routing 8 + assemble 14 + credentials 10 + availability 5) and `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p provider-config -- -D warnings`.

---

### Task 10: orchestrator — `chains` field + 7th `new` param + `chain_for` accessor + `failover_worthy` + non-stream chain loop

Threads `ChainConfig` into `ProviderApiAdapter` as a **7th positional `new` arg** (reconciliation #3), adds a `pub fn chain_for(&self, key: &str) -> Vec<ChainEntry>` read accessor (consumed by the engine integration test), the `failover_worthy` classifier, and the outer chain loop in `drive_non_stream`. `RetryOverride.max_attempts` maps directly to `RetryControl.max_retries` (a retry-count budget; `backoff_ms` is intentionally unconsumed — the driver computes its own backoff, spec §7). Chain entries run `allow_fallback = false` (the chain IS the fallback); a no-chain model keeps today's behavior exactly. All call sites (engine-desktop lib.rs:~1032, engine-mobile host.rs:~382, the in-test `adapter()` helper, and the 4 inline test constructors) pass the new positional arg.

> Verified: `ProviderApiAdapter::new` is 6-arg today (provider_adapter.rs:60); struct fields are `client, transport, max_retries, subscriber, fallback_model, ua, version`; `retry_control` (provider_adapter.rs:294) returns `RetryControl { max_529_retries, fallback_model, primary_model, allow_fallback, is_external, is_sandbox, max_retries, retry_429_allowed }`; `build_request(model, system, msgs, &tools, stream, max_tokens)`; `execute_once(&self, &LlmRequest) -> Result<LlmResponse, (LlmError, Vec<(String,String)>)>`; `FakeTransport { seen: Mutex<Vec<ProviderRequest>>, script }`; `ProviderRequest.body_json: Value` (NOT `body`); `ok_message_json()` / `anthropic_config()` test helpers exist.

**Files:**
- Modify: `lingxi-code/orchestrator/Cargo.toml` (add `provider-config` dep)
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`
- Modify (call sites): `lingxi-code/apps/engine-desktop/src/lib.rs`, `lingxi-code/apps/engine-mobile/src/host.rs` + their `Cargo.toml`s

- [ ] **Step 1: Write the failing test** — add to the existing `#[cfg(test)] mod tests` in `provider_adapter.rs`. The `transport_models` helper reads `r.body_json.get("model")` (the body is a typed `Value`, NOT bytes):

```rust
    /// Models seen on the wire (request `model` field), in order.
    fn transport_models(t: &FakeTransport) -> Vec<String> {
        t.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.body_json.get("model").and_then(|m| m.as_str()).map(str::to_string))
            .collect()
    }

    #[test]
    fn adapter_stores_chain_config_and_chain_for_reads_it() {
        use provider_config::{ChainConfig, ChainEntry};
        let transport = Arc::new(FakeTransport::new(Vec::new()));
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.chains.insert(
            "fast".to_string(),
            vec![ChainEntry {
                provider_id: llm_client::ProviderId::AnthropicFirstParty,
                model: "claude-opus-4-6".to_string(),
            }],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport, 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains.clone(),
        );
        let read = adapter.chain_for("fast");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].model, "claude-opus-4-6");
        assert!(adapter.chain_for("missing").is_empty());
    }

    #[test]
    fn failover_worthy_classifies_transient_vs_terminal() {
        use llm_client::LlmError;
        assert!(failover_worthy(&LlmError::Overloaded));
        assert!(failover_worthy(&LlmError::RateLimited { retry_after: None, scope: None }));
        assert!(failover_worthy(&LlmError::ProviderInternal));
        assert!(failover_worthy(&LlmError::Transport { message: "boom".into() }));
        assert!(failover_worthy(&LlmError::ModelUnavailable));
        assert!(!failover_worthy(&LlmError::Authentication));
        assert!(!failover_worthy(&LlmError::PermissionDenied));
        assert!(!failover_worthy(&LlmError::InvalidRequest { message: "bad".into() }));
        assert!(!failover_worthy(&LlmError::ContextOverflow));
        assert!(!failover_worthy(&LlmError::QuotaExceeded));
        assert!(!failover_worthy(&LlmError::UnsupportedCapability { capability: "vision".into() }));
        assert!(!failover_worthy(&LlmError::CostUnavailable { message: "x".into() }));
        assert!(!failover_worthy(&LlmError::StreamInterrupted { message: "x".into() }));
    }

    #[tokio::test(start_paused = true)]
    async fn chain_advances_on_transient_exhausted() {
        use provider_config::{ChainConfig, ChainEntry, RetryOverride};
        let overload = || ProviderResponse::json(
            529,
            serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"o"}}),
        );
        let mut script: Vec<ProviderResponse> = (0..11).map(|_| overload()).collect();
        script.push(ProviderResponse::json(200, ok_message_json()));
        let transport = Arc::new(FakeTransport::new(script));
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.retry = Some(RetryOverride { max_attempts: 2, backoff_ms: 0 });
        chains.chains.insert(
            "claude-opus-4-6".to_string(),
            vec![
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-opus-4-6".to_string() },
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-sonnet-4-6".to_string() },
            ],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains,
        );
        let resp = adapter
            .messages_create("claude-opus-4-6", None, Vec::new(), Vec::new())
            .await
            .expect("chain advances to the second entry and succeeds");
        assert_eq!(
            resp.provider_metadata.get("fallback_to").and_then(|v| v.as_str()),
            Some("claude-sonnet-4-6"),
            "advancing the chain must record the served model"
        );
        assert!(!transport.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn chain_stops_on_terminal_error() {
        use provider_config::{ChainConfig, ChainEntry};
        let auth_err = ProviderResponse::json(
            401,
            serde_json::json!({"type":"error","error":{"type":"authentication_error","message":"bad key"}}),
        );
        let transport = Arc::new(FakeTransport::new(vec![auth_err]));
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.chains.insert(
            "claude-opus-4-6".to_string(),
            vec![
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-opus-4-6".to_string() },
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-sonnet-4-6".to_string() },
            ],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains,
        );
        let result = adapter.messages_create("claude-opus-4-6", None, Vec::new(), Vec::new()).await;
        assert!(result.is_err(), "a terminal auth error must surface, not fail over");
        assert_eq!(transport.seen.lock().unwrap().len(), 1, "terminal-class must NOT advance");
    }

    #[tokio::test(start_paused = true)]
    async fn chain_supersedes_builtin_fallback() {
        use provider_config::{ChainConfig, ChainEntry, RetryOverride};
        let overload = || ProviderResponse::json(
            529,
            serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"o"}}),
        );
        let transport = Arc::new(FakeTransport::new((0..30).map(|_| overload()).collect()));
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.retry = Some(RetryOverride { max_attempts: 3, backoff_ms: 0 });
        chains.chains.insert(
            "claude-opus-4-6".to_string(),
            vec![ChainEntry {
                provider_id: llm_client::ProviderId::AnthropicFirstParty,
                model: "claude-opus-4-6".to_string(),
            }],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains,
        )
        .with_fallback_model(Some("claude-sonnet-4-6".to_string()));
        let result = adapter.messages_create("claude-opus-4-6", None, Vec::new(), Vec::new()).await;
        assert!(result.is_err(), "single overloaded chain entry must terminate");
        for req in transport_models(&transport) {
            assert_eq!(req, "claude-opus-4-6", "chain entries run allow_fallback=false; built-in Opus→Sonnet must NOT fire");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_chain_preserves_builtin_fallback() {
        let overload = || ProviderResponse::json(
            529,
            serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"o"}}),
        );
        let transport = Arc::new(FakeTransport::new(vec![
            overload(), overload(), overload(),
            ProviderResponse::json(200, ok_message_json()),
        ]));
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test",
            provider_config::ChainConfig::default(),
        )
        .with_fallback_model(Some("claude-sonnet-4-6".to_string()));
        let resp = adapter
            .messages_create("claude-opus-4-6", None, Vec::new(), Vec::new())
            .await
            .expect("built-in fallback still works with no chain");
        assert_eq!(
            resp.provider_metadata.get("fallback_to").and_then(|v| v.as_str()),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(transport.seen.lock().unwrap().len(), 4);
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p orchestrator chain_ adapter_stores failover_worthy no_chain_preserves`  Expected: compile error — `new` takes 6 args, no `chains`/`chain_for`/`failover_worthy`, `provider_config` not a dep.

- [ ] **Step 3: Implement** —

  (a) `orchestrator/Cargo.toml`, under `[dependencies]` after `llm-client`: `provider-config = { path = "../provider-config" }`

  (b) `provider_adapter.rs` import (after `use traits::orchestrator::ModelListing;`): `use provider_config::{ChainConfig, ChainEntry, RetryOverride};`

  (c) struct field (after `version: String,`):
```rust
    /// Cross-provider failover chains + per-entry retry override (spec §7). An
    /// empty `chains` map means "no chain" — behaves exactly as before.
    chains: ChainConfig,
```

  (d) replace `pub fn new(...)` with the 7-arg form:
```rust
    /// Construct the production adapter.
    #[must_use]
    pub fn new(
        client: Arc<DefaultLlmClient>,
        transport: Arc<dyn Transport>,
        max_retries: u32,
        subscriber: SubscriberState,
        ua: model::user_agent::UserAgentEnv,
        version: impl Into<String>,
        chains: ChainConfig,
    ) -> Self {
        Self {
            client,
            transport,
            max_retries,
            subscriber,
            fallback_model: None,
            ua,
            version: version.into(),
            chains,
        }
    }

    /// Read the resolved chain for `key` (alias or `"provider/model"`), or empty
    /// when none is configured. Used by the engine integration test (spec §8).
    #[must_use]
    pub fn chain_for(&self, key: &str) -> Vec<ChainEntry> {
        self.chains.chains.get(key).cloned().unwrap_or_default()
    }
```

  (e) the `failover_worthy` free function (module level, near `record_model_fallback`):
```rust
/// Outer-chain failover classifier (spec §7; mirrors the legacy `is_transient`).
/// `true` when the in-flight entry's exhausted error is transient-class (advance
/// to the next entry); `false` for terminal-class (surface immediately).
fn failover_worthy(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::Overloaded
            | LlmError::RateLimited { .. }
            | LlmError::ProviderInternal
            | LlmError::Transport { .. }
            | LlmError::ModelUnavailable
    )
}
```

  (f) the per-entry helper + outer non-stream loop + resolvers + chain control (replace the body of `drive_non_stream`):
```rust
    /// Effective per-entry `max_retries` from the `RetryOverride` (spec §5.2),
    /// else the host default.
    fn entry_max_retries(&self, retry: Option<&RetryOverride>) -> u32 {
        retry.map_or(self.max_retries, |r| r.max_attempts)
    }

    /// Resolve a routing alias to its underlying key, or return `model` unchanged.
    fn resolve_alias(&self, model: &str) -> String {
        self.chains.aliases.get(model).cloned().unwrap_or_else(|| model.to_string())
    }

    /// Resolve `model` (after alias-folding) to its ordered chain entries, or a
    /// single-entry `[model]` when no chain is configured (no-chain == today).
    fn resolve_chain(&self, model: &str) -> Vec<ChainEntry> {
        let key = self.resolve_alias(model);
        if let Some(chain) = self.chains.chains.get(&key) {
            if !chain.is_empty() {
                return chain.clone();
            }
        }
        // The provider_id is informational here (the registry resolves by req.model).
        vec![ChainEntry {
            provider_id: llm_client::ProviderId::AnthropicFirstParty,
            model: key,
        }]
    }

    /// Per-chain-entry retry control: `allow_fallback` is forced FALSE (the chain
    /// IS the fallback) and the budget comes from `RetryOverride` when set (spec §7).
    fn chain_entry_control(
        &self,
        model: &str,
        retry: Option<&RetryOverride>,
    ) -> model::retry::RetryControl {
        model::retry::RetryControl {
            max_529_retries: model::retry::MAX_529_RETRIES,
            fallback_model: None,
            primary_model: model.to_string(),
            allow_fallback: false,
            is_external: std::env::var("USER_TYPE").as_deref() == Ok("external"),
            is_sandbox: std::env::var("IS_SANDBOX").is_ok(),
            max_retries: self.entry_max_retries(retry),
            retry_429_allowed: !self.subscriber.is_subscriber || self.subscriber.is_enterprise,
        }
    }

    /// One chain ENTRY's inner retry-driver loop (the body that used to be
    /// `drive_non_stream`). Owns `req`; builds its own fresh `RetryState`.
    async fn drive_entry(
        &self,
        mut req: LlmRequest,
        ctl: model::retry::RetryControl,
        initial_consecutive_529: u8,
    ) -> Result<LlmResponse, LlmError> {
        let entry_model = req.model.clone();
        let mut state = model::retry::RetryState {
            consecutive_overloaded: initial_consecutive_529,
            ..Default::default()
        };
        loop {
            match self.execute_once(&req).await {
                Ok(mut resp) => {
                    if req.model != entry_model {
                        record_model_fallback(&mut resp.provider_metadata, &entry_model, &req.model);
                    }
                    return Ok(resp);
                }
                Err((error, headers)) => {
                    if header_value(&headers, "x-should-retry") == Some("false") {
                        return Err(error);
                    }
                    let error = promote_rate_limit(error, &headers);
                    match model::retry::next_step(&mut state, &ctl, &error, 0) {
                        model::retry::DriveStep::RetryAfter(delay) => tokio::time::sleep(delay).await,
                        model::retry::DriveStep::AdjustMaxTokens(new_max) => req.max_tokens = Some(new_max),
                        model::retry::DriveStep::Fallback { fallback_model } => req.model = fallback_model,
                        model::retry::DriveStep::Terminal => {
                            if let Some(copy) = model::retry::repeated_529_message(&state, &error) {
                                tracing::warn!(error = %error, "{copy}");
                            }
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    /// Non-streaming entry point: resolve to a chain (or a single `[model]`), run
    /// each entry's inner retry loop, advancing on transient-exhausted (spec §7).
    async fn drive_non_stream(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        initial_max_tokens: Option<u32>,
        initial_consecutive_529: u8,
    ) -> Result<LlmResponse, LlmError> {
        let entries = self.resolve_chain(model);
        let has_chain = self.chains.chains.contains_key(&self.resolve_alias(model));
        let base = build_request(model, system, msgs, &tools, false, initial_max_tokens);
        let retry = self.chains.retry;
        let total = entries.len();
        let mut last_err: Option<LlmError> = None;
        for (idx, entry) in entries.into_iter().enumerate() {
            let mut req = base.clone();
            req.model = entry.model.clone();
            let ctl = if has_chain {
                self.chain_entry_control(&entry.model, retry.as_ref())
            } else {
                self.retry_control(&entry.model)
            };
            let seed = if idx == 0 { initial_consecutive_529 } else { 0 };
            match self.drive_entry(req, ctl, seed).await {
                Ok(mut resp) => {
                    if idx > 0 {
                        record_model_fallback(&mut resp.provider_metadata, model, &entry.model);
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if idx + 1 < total && failover_worthy(&e) {
                        tracing::warn!(from = %entry.model, error = %e, "chain entry failed; advancing to next failover entry");
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or(LlmError::ProviderInternal))
    }
```
  > `RetryOverride` is `Copy`, so `let retry = self.chains.retry;` copies it (no clone needed). Re-read `retry_control` (provider_adapter.rs:294) and copy its field list verbatim into `chain_entry_control` if the struct grows a field.

  (g) call site — engine-desktop `build()` (lib.rs:~1032): add `provider_config::ChainConfig::default(),` as the 7th `new` arg, BEFORE `.with_fallback_model(...)` (Task 14 replaces `::default()` with `assembled.chains`). Add `provider-config = { path = "../../provider-config" }` to `apps/engine-desktop/Cargo.toml` `[dependencies]`.

  (h) call site — engine-mobile `build_mobile_inner` (host.rs:~382): add `provider_config::ChainConfig::default(),` as the 7th `new` arg. Add `provider-config = { path = "../../provider-config", optional = true }` to `apps/engine-mobile/Cargo.toml` `[dependencies]` AND `"dep:provider-config"` to the `uniffi` feature list (alongside `"dep:orchestrator"`).

  (i) in-test `adapter()` helper (provider_adapter.rs:~839) and the 4 inline `ProviderApiAdapter::new(...)` test constructors (`subscriber_429_is_terminal_no_retry`, `persistently_overloaded_fallback_terminates_not_loops`, `fallback_marks_provider_metadata_on_success`, and any other) each get a trailing `ChainConfig::default(),` 7th arg (before any chained `.with_fallback_model(...)`).

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p orchestrator provider_adapter` then `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build -p engine-desktop` and `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build -p engine-mobile --features uniffi`  Expected: PASS + both apps build (the unchanged `fallback_marks_provider_metadata_on_success` / `persistently_overloaded_fallback_terminates_not_loops` now route through the no-chain single-entry path).

- [ ] **Step 5: Commit** — `git add lingxi-code/orchestrator lingxi-code/apps/engine-desktop lingxi-code/apps/engine-mobile lingxi-code/Cargo.lock && git commit -m "feat(orchestrator): chains field + chain_for + failover_worthy + non-stream outer chain loop (all call sites)"`

---

### Task 11: orchestrator — stream() connect-phase chain loop + served-model stamp (resolves Plan-3a nit (e))

Mirrors Task 10 on the streaming path. Chain-walk at the CONNECT phase (before the stream opens). On advance, emit the connect-phase fallback warning (the surfacing channel nit (e) was missing) and stamp the served model onto `MessageStart`/`Completed`.

> Verified: `LlmEvent::MessageStart { response: Box<LlmResponse> }` / `Completed { response: Box<LlmResponse> }` (protocol.rs:221,256); `RawStreamFrame { bytes: Vec<u8> }` (protocol.rs:392); `futures::stream::{BoxStream, StreamExt}` already imported. `ProviderResponse.body_json` (not `body`).

**Files:**
- Modify: `lingxi-code/orchestrator/src/provider_adapter.rs`

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
    struct OneFrame { frame: Option<llm_client::RawStreamFrame> }
    impl llm_client::FrameStream for OneFrame {
        fn next_frame(&mut self)
            -> llm_client::BoxFuture<'_, Result<Option<llm_client::RawStreamFrame>, LlmError>> {
            Box::pin(async move { Ok(self.frame.take()) })
        }
    }

    struct StreamScript {
        seen: Mutex<Vec<ProviderRequest>>,
        statuses: Mutex<std::collections::VecDeque<u16>>,
    }
    impl Transport for StreamScript {
        fn execute<'a>(&'a self, _request: &'a ProviderRequest)
            -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move { Ok(ProviderResponse::json(500, serde_json::Value::Null)) })
        }
        fn open_stream<'a>(&'a self, request: &'a ProviderRequest)
            -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                self.seen.lock().unwrap().push(request.clone());
                let status = self.statuses.lock().unwrap().pop_front().unwrap_or(200);
                if status >= 400 {
                    let body = serde_json::to_vec(&serde_json::json!({
                        "type":"error","error":{"type":"overloaded_error","message":"o"}
                    })).unwrap();
                    Ok(StreamingResponse {
                        status, headers: BTreeMap::new(),
                        frames: Box::new(OneFrame { frame: Some(llm_client::RawStreamFrame { bytes: body }) }),
                    })
                } else {
                    let start = serde_json::to_vec(&serde_json::json!({
                        "type":"message_start",
                        "message":{
                            "id":"msg_1","type":"message","role":"assistant",
                            "model":"claude-sonnet-4-6","content":[],
                            "stop_reason":serde_json::Value::Null,
                            "usage":{"input_tokens":1,"output_tokens":0}
                        }
                    })).unwrap();
                    Ok(StreamingResponse {
                        status, headers: BTreeMap::new(),
                        frames: Box::new(OneFrame { frame: Some(llm_client::RawStreamFrame { bytes: start }) }),
                    })
                }
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stream_chain_advances_on_connect_transient() {
        use provider_config::{ChainConfig, ChainEntry, RetryOverride};
        let transport = Arc::new(StreamScript {
            seen: Mutex::new(Vec::new()),
            statuses: Mutex::new([529u16, 200u16].into_iter().collect()),
        });
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.retry = Some(RetryOverride { max_attempts: 0, backoff_ms: 0 });
        chains.chains.insert(
            "claude-opus-4-6".to_string(),
            vec![
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-opus-4-6".to_string() },
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-sonnet-4-6".to_string() },
            ],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains,
        );
        let mut stream = StreamingApiClient::stream(&adapter, "claude-opus-4-6", None, Vec::new(), Vec::new())
            .await
            .expect("connect-phase chain advances to the second entry");
        assert_eq!(transport.seen.lock().unwrap().len(), 2);
        let first = stream.next().await.expect("an event").expect("ok event");
        match first {
            LlmEvent::MessageStart { response } => assert_eq!(response.model, "claude-sonnet-4-6"),
            other => panic!("expected MessageStart, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_chain_stops_on_connect_terminal() {
        use provider_config::{ChainConfig, ChainEntry};
        struct AuthConnect { seen: Mutex<usize> }
        impl Transport for AuthConnect {
            fn execute<'a>(&'a self, _r: &'a ProviderRequest)
                -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
                Box::pin(async move { Ok(ProviderResponse::json(500, serde_json::Value::Null)) })
            }
            fn open_stream<'a>(&'a self, _r: &'a ProviderRequest)
                -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
                Box::pin(async move {
                    *self.seen.lock().unwrap() += 1;
                    let body = serde_json::to_vec(&serde_json::json!({
                        "type":"error","error":{"type":"authentication_error","message":"bad"}
                    })).unwrap();
                    Ok(StreamingResponse {
                        status: 401, headers: BTreeMap::new(),
                        frames: Box::new(OneFrame { frame: Some(llm_client::RawStreamFrame { bytes: body }) }),
                    })
                })
            }
        }
        let transport = Arc::new(AuthConnect { seen: Mutex::new(0) });
        let client = Arc::new(DefaultLlmClient::from_config(anthropic_config()).expect("client"));
        let mut chains = ChainConfig::default();
        chains.chains.insert(
            "claude-opus-4-6".to_string(),
            vec![
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-opus-4-6".to_string() },
                ChainEntry { provider_id: llm_client::ProviderId::AnthropicFirstParty, model: "claude-sonnet-4-6".to_string() },
            ],
        );
        let adapter = ProviderApiAdapter::new(
            client, transport.clone(), 10, SubscriberState::default(),
            model::user_agent::UserAgentEnv::default(), "1.0.0-test", chains,
        );
        let result = StreamingApiClient::stream(&adapter, "claude-opus-4-6", None, Vec::new(), Vec::new()).await;
        assert!(result.is_err(), "connect-phase terminal error must surface");
        assert_eq!(*transport.seen.lock().unwrap(), 1, "must not advance on terminal");
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p orchestrator stream_chain_`  Expected: assert failure — `stream()` ignores `chains` (advance test sees `seen.len()==1`).

- [ ] **Step 3: Implement** —

  (a) `connect_entry` helper (inherent `impl ProviderApiAdapter`, owns `req`):
```rust
    /// One chain ENTRY's streaming connect loop. Owns `req`; runs the connect-phase
    /// retry driver with the supplied per-entry control.
    async fn connect_entry(
        &self,
        req: LlmRequest,
        ctl: model::retry::RetryControl,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let mut state = model::retry::RetryState::default();
        loop {
            let mut prepared = self.client.prepare(&req).await?;
            self.apply_headers(&mut prepared.provider_request, model::betas::Endpoint::MessagesCreateStream);
            match self.transport.open_stream(&prepared.provider_request).await {
                Ok(streaming) => {
                    if streaming.status >= 400 {
                        let headers = header_pairs(&streaming.headers);
                        let error = drain_stream_error(&prepared, streaming).await;
                        if header_value(&headers, "x-should-retry") == Some("false") {
                            return Err(error);
                        }
                        let error = promote_rate_limit(error, &headers);
                        match model::retry::next_step(&mut state, &ctl, &error, 0) {
                            model::retry::DriveStep::RetryAfter(delay) => {
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                            model::retry::DriveStep::Fallback { .. }
                            | model::retry::DriveStep::AdjustMaxTokens(_)
                            | model::retry::DriveStep::Terminal => return Err(error),
                        }
                    }
                    let event_stream = llm_client::client::LlmEventStream::from_parts(
                        prepared.route.codec.stream_decoder(),
                        streaming.frames,
                    );
                    return Ok(into_event_stream(event_stream));
                }
                Err(error) => match model::retry::next_step(&mut state, &ctl, &error, 0) {
                    model::retry::DriveStep::RetryAfter(delay) => tokio::time::sleep(delay).await,
                    model::retry::DriveStep::Fallback { .. }
                    | model::retry::DriveStep::AdjustMaxTokens(_)
                    | model::retry::DriveStep::Terminal => return Err(error),
                },
            }
        }
    }
```

  (b) replace the `stream()` body (in the `#[async_trait] impl StreamingApiClient for ProviderApiAdapter` block):
```rust
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let entries = self.resolve_chain(model);
        let has_chain = self.chains.chains.contains_key(&self.resolve_alias(model));
        let base = build_request(model, system, messages, &tools, true, None);
        let retry = self.chains.retry;
        let total = entries.len();
        let mut last_err: Option<LlmError> = None;
        for (idx, entry) in entries.into_iter().enumerate() {
            let mut req = base.clone();
            req.model = entry.model.clone();
            let ctl = if has_chain {
                self.chain_entry_control(&entry.model, retry.as_ref())
            } else {
                self.retry_control(&entry.model)
            };
            match self.connect_entry(req, ctl).await {
                Ok(stream) => {
                    if idx > 0 {
                        tracing::warn!(fallback_from = %model, fallback_to = %entry.model, "streaming chain advanced; serving fallback model");
                        return Ok(stamp_served_model(stream, entry.model.clone()));
                    }
                    return Ok(stream);
                }
                Err(e) => {
                    if idx + 1 < total && failover_worthy(&e) {
                        tracing::warn!(from = %entry.model, error = %e, "streaming chain entry failed at connect; advancing");
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or(LlmError::ProviderInternal))
    }
```

  (c) the served-model stamp (module level, near `into_event_stream`):
```rust
/// Stamp the served chain-entry model onto the assembled response inside the
/// stream's `MessageStart` / `Completed` events (streaming served-model surfacing,
/// spec §7 — resolves Plan-3a nit (e)). Pass-through for all other events.
fn stamp_served_model(
    stream: BoxStream<'static, Result<LlmEvent, LlmError>>,
    served_model: String,
) -> BoxStream<'static, Result<LlmEvent, LlmError>> {
    stream
        .map(move |item| {
            item.map(|event| match event {
                LlmEvent::MessageStart { mut response } => {
                    response.model = served_model.clone();
                    LlmEvent::MessageStart { response }
                }
                LlmEvent::Completed { mut response } => {
                    response.model = served_model.clone();
                    LlmEvent::Completed { response }
                }
                other => other,
            })
        })
        .boxed()
}
```
  > `response` is `Box<LlmResponse>`; `response.model` mutates through the Box. Keep `stream()` inside the trait impl; `connect_entry` in the inherent impl.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p orchestrator stream_chain_` then `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p orchestrator`  Expected: PASS (full suite).

- [ ] **Step 5: Commit** — `git add lingxi-code/orchestrator/src/provider_adapter.rs && git commit -m "feat(orchestrator): chain-walk stream() connect-phase + served-model stamp (resolves Plan-3a nit e)"`

---

## Cost pricing for all providers

The engine's `CostTracker::new` takes `Arc<cost::PricingCatalog>` (cost/src/tracker.rs:86). `assemble().pricing` is therefore a `cost::PricingCatalog`. `llm_client::PricingCatalog` is unreadable (private `prices`/`overrides`, private `PricingKey` — cost.rs:47-50), so it CANNOT be translated; the honest design seeds the cost catalog from `cost::PricingCatalog::builtin_reference()` (already carries Anthropic/OpenAI/Gemini reference tiers) and adds explicit `with_entry` rows for non-Anthropic profile models that would otherwise be unpriced, so non-Anthropic turns are priced (spec §8). The cost crate is NOT frozen. There is **no `CostEstimator` and no `merge_llm_pricing_into_cost`** — those types do not exist.

### Task 12: `cost::PricingCatalog::with_entry` — public mutator

Adds a chainable public mutator to the (non-frozen) `cost::PricingCatalog` so callers can insert a `ModelPricing` row. The catalog's `entries` field is private; this is the public seam. Verified real types: `ModelPricing { model_ref: ModelRef, token_rates: HashMap<TokenClass, MoneyPerToken>, non_token_rates_nano_usd, effective_from, source: PricingSource }`; `MoneyPerToken { nano_usd_per_token: u64 }` (bare struct); `PricingSource::{BuiltInReference{provider}, HostOverride{path}, RemoteManagedSettings}` (NO `External`); conversion convention `milli-USD per Mtok == nano-USD per token` (pricing.rs:1-12).

**Files:**
- Modify: `lingxi-code/cost/src/pricing.rs`

- [ ] **Step 1: Write the failing test** — add to `cost/src/pricing.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn with_entry_adds_a_resolvable_model() {
        // A non-Anthropic provider model can be added and resolves exactly.
        let mut rates: HashMap<TokenClass, MoneyPerToken> = HashMap::new();
        rates.insert(TokenClass::Input, MoneyPerToken { nano_usd_per_token: 270 });
        rates.insert(TokenClass::Output, MoneyPerToken { nano_usd_per_token: 1_100 });
        let mr = ModelRef {
            provider: ProviderId::OpenAICompatible { name: "deepseek".to_string() },
            model: "deepseek-chat".to_string(),
        };
        let cat = PricingCatalog::builtin_reference().with_entry(ModelPricing {
            model_ref: mr.clone(),
            token_rates: rates,
            non_token_rates_nano_usd: HashMap::new(),
            effective_from: None,
            source: PricingSource::RemoteManagedSettings,
        });
        let (p, res) = cat.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 270);
        // Anthropic builtins survive.
        let opus = ModelRef { provider: ProviderId::Anthropic, model: "claude-opus-4-6".to_string() };
        assert!(cat.resolve(&opus).is_ok());
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p cost with_entry_adds_a_resolvable_model`  Expected: `no method named with_entry`.

- [ ] **Step 3: Implement** — add to the `impl PricingCatalog` block (after `builtin_reference`):

```rust
    /// Add (or overwrite) one exact `(provider, model)` pricing entry, returning
    /// `self` for chaining. Used by `provider-config` to price non-Anthropic
    /// catalog / user-provider models on top of [`Self::builtin_reference`]
    /// (Plan 3c §8).
    #[must_use]
    pub fn with_entry(mut self, pricing: ModelPricing) -> Self {
        self.entries.insert(pricing.model_ref.clone(), pricing);
        self
    }
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p cost pricing`  Expected: PASS (new test + existing).

- [ ] **Step 5: Commit** — `git add lingxi-code/cost/src/pricing.rs && git commit -m "feat(cost): PricingCatalog::with_entry public mutator (Plan 3c §8)"`

---

### Task 13: provider-config `cost_translate` — build `Assembled.pricing` from the merged profiles

Builds the `cost::PricingCatalog` `assemble` returns: `builtin_reference()` (Anthropic/OpenAI/Gemini tiers preserved) + a `with_entry` row per non-Anthropic profile model that `builtin_reference` does not already resolve, priced at the `$5/$25` default-unknown tier so non-Anthropic turns are *priced* (not `UnpricedModel`-errored). The `(profile_name, request_model)` → `cost::ProviderId` mapping mirrors `orchestrator::cost_wiring::provider_id_for_profile` (anthropic→Anthropic, openai/azure→OpenAI, gemini/vertex→GoogleGemini, bedrock→AmazonBedrock, else→OpenAICompatible{name}). Then swap `assemble`'s pricing line to call this.

**Files:**
- Create: `lingxi-code/provider-config/src/cost_translate.rs`
- Modify: `lingxi-code/provider-config/src/lib.rs` (`mod cost_translate;`)
- Modify: `lingxi-code/provider-config/src/assemble.rs` (swap the pricing line)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/provider-config/src/cost_translate.rs`:

```rust
//! Build the `cost::PricingCatalog` returned by `assemble` from the merged
//! provider profiles (Plan 3c §8). Anthropic / OpenAI / Gemini reference tiers
//! come from `cost::PricingCatalog::builtin_reference()`; any other profile model
//! that the reference catalog does not already price gets an explicit
//! default-unknown ($5/$25) row so non-Anthropic turns are priced, not errored.

use llm_client::{ProviderProfile, ProviderId as LlmProviderId};

use cost::pricing::ProviderId as CostProviderId;
use cost::{ModelRef, ModelPricing, PricingCatalog};

/// Map a profile name + its llm-client `ProviderId` to the cost `ProviderId`.
/// Mirrors `orchestrator::cost_wiring::provider_id_for_profile`.
fn cost_provider_id(profile_name: &str, provider_id: &LlmProviderId) -> CostProviderId {
    match profile_name {
        "anthropic" => CostProviderId::Anthropic,
        "openai" | "azure" => CostProviderId::OpenAI,
        "gemini" | "vertex" => CostProviderId::GoogleGemini,
        "bedrock" => CostProviderId::AmazonBedrock,
        _ => match provider_id {
            LlmProviderId::AnthropicFirstParty => CostProviderId::Anthropic,
            LlmProviderId::OpenAICompatible { name } | LlmProviderId::Custom { name } => {
                CostProviderId::OpenAICompatible { name: name.clone() }
            }
            other => CostProviderId::OpenAICompatible { name: format!("{other:?}") },
        },
    }
}

/// Build the cost catalog: reference tiers + a default-unknown row for every
/// non-Anthropic profile model the reference catalog does not already price.
#[must_use]
pub fn pricing_for(providers: &[ProviderProfile]) -> PricingCatalog {
    let mut catalog = PricingCatalog::builtin_reference();
    for profile in providers {
        if profile.profile_name == "anthropic" {
            continue; // Anthropic tiers already in builtin_reference.
        }
        let provider = cost_provider_id(&profile.profile_name, &profile.provider_id);
        for model in &profile.models {
            let mr = ModelRef { provider: provider.clone(), model: model.billing_model.clone() };
            if catalog.resolve(&mr).is_ok() {
                continue; // already priced by the reference catalog.
            }
            catalog = catalog.with_entry(ModelPricing {
                model_ref: mr.clone(),
                ..PricingCatalog::default_unknown_pricing(&mr)
            });
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{AuthStrategy, Capabilities, CredentialConfig, ModelProfile, PricingConfig, ProtocolFamily};

    fn user_profile(name: &str, model: &str) -> ProviderProfile {
        ProviderProfile {
            provider_id: LlmProviderId::OpenAICompatible { name: name.to_string() },
            profile_name: name.to_string(),
            base_url: "https://x".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::Static { id: name.to_string() },
            models: vec![ModelProfile {
                display_model: model.to_string(),
                request_model: model.to_string(),
                billing_model: model.to_string(),
                aliases: Vec::new(),
                capabilities: Capabilities::default(),
            }],
            pricing: PricingConfig::default(),
        }
    }

    #[test]
    fn anthropic_reference_tiers_preserved() {
        let cat = pricing_for(&[]);
        let opus = ModelRef { provider: CostProviderId::Anthropic, model: "claude-opus-4-6".to_string() };
        assert!(cat.resolve(&opus).is_ok());
    }

    #[test]
    fn unpriced_user_model_gets_default_unknown_row() {
        let cat = pricing_for(&[user_profile("groq", "llama-3.3-70b")]);
        let mr = ModelRef {
            provider: CostProviderId::OpenAICompatible { name: "groq".to_string() },
            model: "llama-3.3-70b".to_string(),
        };
        let (p, _res) = cat.resolve(&mr).expect("priced");
        // $5/$25 default-unknown tier.
        assert_eq!(p.token_rates[&cost::pricing::TokenClass::Input].nano_usd_per_token, 5_000);
        assert_eq!(p.token_rates[&cost::pricing::TokenClass::Output].nano_usd_per_token, 25_000);
    }
}
```

  Add to `lib.rs`: `mod cost_translate;` (no re-export needed — `assemble` uses it crate-internally).

  Then in `assemble.rs`, replace the step-6 pricing line:
```rust
    let pricing = cost::PricingCatalog::builtin_reference();
```
  with:
```rust
    let pricing = crate::cost_translate::pricing_for(&providers);
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config cost_translate::`  Expected: module not found (pre-write).

- [ ] **Step 3: Implement** — the non-test portion of `cost_translate.rs` + the `assemble.rs` one-line swap.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p provider-config` (cost_translate 3 + the existing suite; `assemble`'s `anthropic_builtins_priced_through_assembled_catalog` still passes).

- [ ] **Step 5: Commit** — `git add lingxi-code/provider-config && git commit -m "feat(provider-config): cost_translate — Assembled.pricing as cost::PricingCatalog (reference + non-anthropic default rows)"`

---

## Engine build() wiring

These consume the cross-group APIs verbatim: `provider_config::assemble(AssembleInputs) -> Assembled`; `Assembled { client_config, pricing: cost::PricingCatalog, chains, credential_sources, warnings }`; `MultiCredentialProvider::new(Arc<CredentialManager>, Vec<CredentialSource>, Option<String>, Option<Arc<dyn CredentialProvider>>)`; `compute_availability(&Arc<CredentialManager>, &[CredentialSource], bool, bool) -> Vec<ProviderAvailability>`; `ProviderApiAdapter::new(.., chains)` (7th positional). The `AssembleInputs` field names are the canonical `anthropic_*`/`user_providers`/`routing` set (reconciliation #1).

### Task 14: Desktop `build()` — assemble → composite creds + chains + cost + availability, surfaced on `DesktopRuntime`

Replaces the single-Anthropic `ClientConfig` (lib.rs:993-1015), the bare `CostTracker` catalog (lib.rs:1075-1078), and the adapter's missing chains with the assembled multi-provider path; surfaces `provider_adapter` (concrete handle) + `provider_availability: BTreeMap<String, bool>` on `DesktopRuntime` for the integration test + the tui. The desktop integration test reuses `orchestrator::test_support::RecordingPermissionSink` (reconciliation #9).

> Verified: `build(cfg, output: Arc<dyn OutputStream>, permission_sink: Arc<dyn PermissionRequestSink>)` (lib.rs:901); `credentials = Arc::new(CredentialManager::new(storage, clock.clone(), http.clone()))` (lib.rs:916); `llm_models = anthropic_models_for(&cfg.default_model, cfg.fallback_model.as_deref())` (lib.rs:986); `has_api_key`/`has_oauth` (lib.rs:987-992); `oauth_auth_state` is the OAuth `Option<AuthState>`; `CostTracker::new(SessionId, Arc<PricingCatalog>, tx)`. The in-crate `anthropic_profile`/`anthropic_models_for` helpers (lib.rs:740-831) stay (still used to build `anthropic_models`); their unit tests stay green.

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs`
- Test: `lingxi-code/apps/engine-desktop/tests/provider_routing_wiring.rs` (NEW) + a seam test in `lib.rs` `mod tests`

- [ ] **Step 1: Write the failing tests** —

  In `lib.rs` `#[cfg(test)] mod tests`:
```rust
    #[tokio::test]
    async fn build_surfaces_provider_availability_and_adapter() {
        let (_tmp, cfg) = test_config(true);
        let output: Arc<dyn traits::OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
            Arc::new(orchestrator::test_support::RecordingPermissionSink::default());
        let rt = build(cfg, output, perm_sink).await.expect("build() failed");
        // Anthropic always present; with no key/oauth it is unavailable.
        assert_eq!(rt.provider_availability.get("anthropic"), Some(&false));
        // Built-in catalog providers are merged into the availability map.
        assert!(rt.provider_availability.contains_key("deepseek"));
        assert!(Arc::strong_count(&rt.provider_adapter) >= 1);
    }
```
  > `test_config(true)` is the existing helper; reuse whatever the sibling `build_*` tests use (e.g. `build_constructs_runtime_deterministically`). If `test_config` returns `(TempDir, DesktopConfig)`, the no-providers default makes anthropic unavailable.

  New integration test file `apps/engine-desktop/tests/provider_routing_wiring.rs`:
```rust
//! Plan 3c §8: `build()` with a `providers`+`routing` fixture wires the merged
//! ClientConfig (built-ins + user provider), the fallback chains on the adapter,
//! and the availability map for the picker.

use std::collections::BTreeMap;
use std::sync::Arc;

use engine_desktop::{build, DesktopConfig};

fn fixture_cfg(tmp: &std::path::Path) -> DesktopConfig {
    let mut providers = BTreeMap::new();
    providers.insert(
        "groq".to_string(),
        serde_json::json!({
            "type": "openai",
            "baseUrl": "https://api.groq.com/openai/v1",
            "apiKeyEnv": "GROQ_API_KEY",
            "models": ["llama-3.3-70b"]
        }),
    );
    let routing = serde_json::json!({
        "aliases": { "fast": "groq/llama-3.3-70b" },
        "fallback": { "fast": ["groq/llama-3.3-70b", "anthropic/claude-haiku-4-5"] },
        "retry": { "maxAttempts": 2, "backoffMs": 250 }
    });
    DesktopConfig {
        api_base: "https://api.anthropic.com".to_string(),
        api_key: String::new(),
        cwd: tmp.to_path_buf(),
        claude_home: tmp.join("home"),
        default_model: "claude-sonnet-4-6".to_string(),
        fallback_model: None,
        provider_profiles: Some(providers),
        routing: Some(routing),
        mcp_paths: vec![tmp.join(".mcp.json")],
        use_noop_permission_gate: true,
        session_started_as_coordinator: false,
        memory_provider: None,
        permission_mode: permission::PermissionMode::Default,
        connect_prompt: None,
    }
}

#[tokio::test]
async fn build_with_providers_and_routing_merges_config_chains_availability() {
    std::env::remove_var("GROQ_API_KEY");
    let tmp = tempfile::tempdir().expect("tmp");
    let output: Arc<dyn traits::OutputStream> =
        Arc::new(orchestrator::test_support::MockOutputStream::new());
    let perm_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(orchestrator::test_support::RecordingPermissionSink::default());

    let rt = build(fixture_cfg(tmp.path()), output, perm_sink).await.expect("build() failed");

    // (1) Merged ClientConfig — the user provider model is routable.
    let models = rt.orchestrator.list_model_listings().await;
    let ids: Vec<&str> = models.iter().map(|m| m.request_model.as_str()).collect();
    assert!(ids.iter().any(|m| *m == "llama-3.3-70b"), "user provider model merged: {ids:?}");
    assert!(
        models.iter().any(|m| m.provider_id.contains("deepseek") || m.provider_id.contains("openrouter")),
        "built-in catalog providers merged"
    );

    // (2) Chains on the adapter — the `fast` chain has 2 entries.
    let chain = rt.provider_adapter.chain_for("fast");
    assert_eq!(chain.len(), 2, "fast chain = [groq, anthropic]");
    assert_eq!(chain[0].model, "llama-3.3-70b");
    assert_eq!(chain[1].model, "claude-haiku-4-5");

    // (3) Availability — anthropic + groq present; both unavailable (no key/env).
    assert_eq!(rt.provider_availability.get("groq"), Some(&false));
    assert_eq!(rt.provider_availability.get("anthropic"), Some(&false));
    assert!(rt.provider_availability.contains_key("deepseek"));
}
```
  > Match the EXACT `DesktopConfig` field set against lib.rs:500 — the fields shown (incl. `session_started_as_coordinator`, `memory_provider`, `permission_mode`) are illustrative; copy the real field list and add `connect_prompt: None` (added in Task 17). `engine-desktop` already lists `tempfile` as a dev-dep (used by other integration tests); if not, add it.

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop --test provider_routing_wiring` and `... build_surfaces_provider_availability_and_adapter`  Expected: compile/assert failures — `DesktopRuntime` lacks the fields, single-Anthropic config lacks `llama-3.3-70b`, `chain_for` not passed real chains.

- [ ] **Step 3: Implement** —

  (a) Add two fields to `DesktopRuntime` (after `file_changed_watcher`, lib.rs:~715):
```rust
    /// Plan 3c §8: per-`profile_name` availability flag driving the `/model`
    /// picker's Connect badge (a sibling map, NOT a field on the frozen
    /// `ModelListing`). The tui joins it by provider/profile name.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// Plan 3c: the concrete routing adapter, surfaced read-only so host/tests
    /// can inspect the wired fallback chains.
    pub provider_adapter: Arc<ProviderApiAdapter>,
```

  (b) Replace the config/credential block (lib.rs:993-1015) with the assembled path. `llm_models` is moved into `AssembleInputs.anthropic_models`:
```rust
    // Plan 3c §8: assemble the FULL multi-provider client config (Anthropic +
    // builtin catalog presets + settings `providers`) + chains + credential
    // sources, instead of the single-Anthropic config. The OAuth case is a
    // pre-built delegate so provider-config stays free of an anthropic-oauth dep.
    let oauth_delegate: Option<Arc<dyn llm_client::CredentialProvider>> =
        oauth_auth_state.clone().map(|state| {
            let driver = Arc::new(anthropic_oauth::RefreshDriver::new(state));
            Arc::new(anthropic_oauth::OAuthCredentialProvider::new(driver))
                as Arc<dyn llm_client::CredentialProvider>
        });

    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: llm_models,
        anthropic_has_api_key: has_api_key,
        anthropic_has_oauth: has_oauth,
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    for w in &assembled.warnings {
        tracing::warn!(warning = %w, "provider-config assembly");
    }

    let mut llm = llm_client::DefaultLlmClient::from_config(assembled.client_config)
        .map_err(|e| BuildError::ApiBase(format!("llm-client config: {e}")))?;
    // §6.1: ONE composite credential slot for ALL providers.
    let composite = provider_config::MultiCredentialProvider::new(
        credentials.clone(),
        assembled.credential_sources.clone(),
        if has_api_key { Some(cfg.api_key.clone()) } else { None },
        oauth_delegate,
    );
    llm = llm.with_credential_provider(Arc::new(composite));
    let llm_client_handle = Arc::new(llm);
```
  Keep the existing `let llm_transport …` + `let tool_provider …` (lib.rs:1017-1023) unchanged.

  (c) Adapter construction (lib.rs:1031-1044): pass `assembled.chains.clone()` as the 7th positional `new` arg (replacing the Task-10 `ChainConfig::default()` placeholder), keep `.with_fallback_model(...)`, and keep a concrete handle before coercion:
```rust
    let provider_adapter = Arc::new(
        ProviderApiAdapter::new(
            llm_client_handle,
            llm_transport,
            orchestrator::model::retry::max_retries_from_env(),
            orchestrator::ProviderSubscriberState { is_subscriber, is_enterprise: false },
            orchestrator::model::user_agent::UserAgentEnv::from_process_env(),
            env!("CARGO_PKG_VERSION"),
            assembled.chains.clone(),
        )
        .with_fallback_model(cfg.fallback_model.clone()),
    );
    let provider_adapter_handle = provider_adapter.clone();
    let api_client: Arc<dyn OrchestratorApiClient> = provider_adapter.clone();
    let subagent_api: Arc<dyn agent::SubagentApiClient> = provider_adapter;
```

  (d) `CostTracker` catalog (lib.rs:1075-1078): wrap `assembled.pricing` (a `cost::PricingCatalog`):
```rust
    let cost_tracker = Arc::new(cost::CostTracker::new(
        protocol::SessionId::new(),
        Arc::new(assembled.pricing),
        cost_persist_tx,
    ));
```

  (e) Availability map (just before the `DesktopRuntime { … }` literal). Uses `compute_availability` (async) keyed by `profile_name`:
```rust
    // Plan 3c §6.2: per-profile availability from the assembled credential sources.
    let provider_availability: std::collections::BTreeMap<String, bool> =
        provider_config::compute_availability(
            &credentials,
            &assembled.credential_sources,
            has_api_key,
            has_oauth,
        )
        .await
        .into_iter()
        .map(|a| (a.profile_name, a.available))
        .collect();
```

  (f) Add both fields to the returned `DesktopRuntime { … }` literal:
```rust
        provider_availability,
        provider_adapter: provider_adapter_handle,
```

  Add `provider-config = { path = "../../provider-config" }` to `apps/engine-desktop/Cargo.toml` `[dependencies]` (idempotent if Task 10 already added it).

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop --test provider_routing_wiring` then `... build_surfaces_provider_availability_and_adapter` then `... build_constructs_runtime_deterministically`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/apps/engine-desktop lingxi-code/Cargo.lock && git commit -m "feat(engine-desktop): wire provider-config assemble → composite creds + chains + cost + availability (Plan 3c §8)"`

---

### Task 15: Mobile `build_mobile_inner` — share the assemble/client/composite/chains core (api-key + env, no OAuth)

engine-mobile shares the routing core but has no OAuth path (api-key-only). Wire `assemble` → composite (no OAuth delegate) → chains, reading `cfg.provider_profiles` / `cfg.routing`. Mobile passes `anthropic_has_oauth = false` and `oauth_delegate = None`. `MobileConfig` has no `fallback_model`, so `anthropic_models` is `anthropic_models(&cfg.default_model)` (the existing mobile helper). The mobile test uses the existing `build_mobile(test_config(..), platform, listener, perm_sink)` harness (mirroring `build_mobile_constructs_orchestrator`, host.rs:1407).

> Verified: mobile `build_mobile_inner` builds `storage = Arc::new(platform_posix_minimal::PlainTextSecureStorage::new())`, `clock = platform.clock()`, `http = platform.http()`; the single-Anthropic `llm_config` is `vec![anthropic_profile(&cfg.api_base, has_api_key, anthropic_models(&cfg.default_model))]`; the credential attachment is `if has_api_key { llm.with_credential_provider(StaticCredentialProvider...) }`; the adapter `new` got a `ChainConfig::default()` placeholder in Task 10. `MobileConfig: Default` (host.rs:133). The mobile crate is gated behind feature `uniffi`.

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/host.rs` (`build_mobile_inner`)
- Test: `lingxi-code/apps/engine-mobile/src/host.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — add to the host `mod tests`:

```rust
    #[tokio::test]
    async fn build_mobile_merges_user_provider_and_chain() {
        std::env::remove_var("GROQ_API_KEY");
        let tmp = tempfile::tempdir().expect("tempdir");
        let platform: Arc<dyn traits::Platform> =
            Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
        let listener: Arc<dyn ClientEventListener> = Arc::new(FakeListener::default());
        let perm_sink: Arc<dyn PermissionRequestSink> = Arc::new(RecordingPermissionSink::default());

        let mut providers = std::collections::BTreeMap::new();
        providers.insert(
            "groq".to_string(),
            serde_json::json!({
                "type": "openai",
                "baseUrl": "https://api.groq.com/openai/v1",
                "apiKeyEnv": "GROQ_API_KEY",
                "models": ["llama-3.3-70b"]
            }),
        );
        let mut cfg = test_config(tmp.path());
        cfg.provider_profiles = Some(providers);
        cfg.routing = Some(serde_json::json!({
            "fallback": { "claude-sonnet-4-6": ["claude-sonnet-4-6", "claude-haiku-4-5"] }
        }));

        let rt = build_mobile(cfg, platform, listener, perm_sink).await.expect("build_mobile");
        let ids: Vec<String> = rt
            .orchestrator
            .list_model_listings()
            .await
            .into_iter()
            .map(|m| m.request_model)
            .collect();
        assert!(ids.iter().any(|m| m == "llama-3.3-70b"), "mobile merges the user provider model: {ids:?}");
    }
```
  > `test_config(tmp.path())` is the existing mobile host-test helper (used by `build_mobile_constructs_orchestrator`); it returns a `MobileConfig`. Mutate `provider_profiles`/`routing` on it.

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-mobile --features uniffi build_mobile_merges_user_provider_and_chain`  Expected: single-Anthropic mobile config ⇒ `llama-3.3-70b` absent.

- [ ] **Step 3: Implement** — in `build_mobile_inner`, replace the single-Anthropic block (`let llm_config = … ` through the `if has_api_key { llm = llm.with_credential_provider(StaticCredentialProvider…) }`) with the assembled core:

```rust
    let assembled = provider_config::assemble(provider_config::AssembleInputs {
        anthropic_api_base: cfg.api_base.clone(),
        anthropic_models: anthropic_models(&cfg.default_model),
        anthropic_has_api_key: has_api_key,
        anthropic_has_oauth: false, // mobile inference is api-key-only (no OAuth)
        user_providers: cfg.provider_profiles.clone().unwrap_or_default(),
        routing: cfg.routing.clone(),
    });
    for w in &assembled.warnings {
        tracing::warn!(warning = %w, "provider-config assembly (mobile)");
    }
    let mut llm = llm_client::DefaultLlmClient::from_config(assembled.client_config)
        .map_err(|e| MobileBuildError::ApiBase(format!("llm-client config: {e}")))?;
    // §6.1: single composite slot; mobile has no OAuth delegate. The Anthropic
    // api key is served directly; every other provider resolves keychain → env.
    let composite_credentials = Arc::new(secret::CredentialManager::new(
        storage.clone(),
        clock.clone(),
        http.clone(),
    ));
    let composite = provider_config::MultiCredentialProvider::new(
        composite_credentials,
        assembled.credential_sources.clone(),
        if has_api_key { Some(cfg.api_key.clone()) } else { None },
        None,
    );
    llm = llm.with_credential_provider(Arc::new(composite));
```
  > `storage`/`clock`/`http` are the locals already bound at the top of `build_mobile_inner`. `CredentialManager::new(storage, clock, http)` matches the desktop arg order. On a host shim `storage` is `PlainTextSecureStorage`, so the keychain branch is effectively env-only — acceptable (mobile uses env/settings keys, §11).

  Then pass `assembled.chains.clone()` as the 7th positional `new` arg of the mobile `ProviderApiAdapter::new(...)` (replacing the Task-10 `ChainConfig::default()` placeholder):
```rust
    let provider_adapter = Arc::new(ProviderApiAdapter::new(
        Arc::new(llm),
        llm_transport,
        orchestrator::model::retry::max_retries_from_env(),
        orchestrator::ProviderSubscriberState::default(),
        orchestrator::model::user_agent::UserAgentEnv::from_process_env(),
        env!("CARGO_PKG_VERSION"),
        assembled.chains.clone(),
    ));
```
  Do NOT add an availability map / `provider_adapter` field to `MobileRuntime` (mobile's interactive `/connect`/picker is a follow-up, §11) — only the routing core is shared. `provider-config` is already an optional dep behind `uniffi` (Task 10).

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-mobile --features uniffi build_mobile_merges_user_provider_and_chain` then the existing host tests under `--features uniffi`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/apps/engine-mobile/src/host.rs lingxi-code/Cargo.lock && git commit -m "feat(engine-mobile): share provider-config assemble + composite creds + chains core (api-key/env)"`

---

## /connect command (commands-core + engine)

`/connect` is engine-driven, tui-rendered. The two seams the handler needs (a credential writer + a Copilot device-flow driver) are NEW traits inside `command-core` (NOT `traits`, keeping the §10 frozen-guard green — it freezes only traits/protocol/llm-client). `/connect` is NOT one of the 99 locked `BUILTIN_COMMAND_NAMES` (verified absent in `command-api/src/builtin_support/names.rs:16`), so it is additively registered with its own static description. The valid targets are the four catalog `profile_name`s (`openrouter`, `deepseek`, `glm-coding`, `github-copilot`) plus any user `settings.providers` key.

### Task 16: Define the `/connect` seam traits + `ConnectHandler`, register it

**Files:**
- Create: `lingxi-code/commands/core/src/connect.rs`
- Modify: `lingxi-code/commands/core/src/lib.rs` (`pub mod connect;` + re-exports), `lingxi-code/commands/core/src/register.rs` (`register_core_connect`)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/commands/core/src/connect.rs`:

```rust
//! `/connect <provider>` — interactive credential setup (engine-driven, tui-rendered).
//!
//! API-key providers read+store a secret via the [`ConnectCredentialWriter`] seam.
//! `/connect github-copilot` drives the GitHub device-flow through the
//! [`CopilotConnectDriver`] seam (begin → display → poll → store). Both seams are
//! defined HERE (not in the frozen `traits` crate) and implemented by the engine.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::Arc;
use thiserror::Error;

/// Static one-liner for `/help` + the palette (`/connect` is not a locked builtin
/// name, so it carries its own description).
pub const CONNECT_DESCRIPTION: &str =
    "Connect a model provider (store an API key, or sign in to GitHub Copilot)";

/// Failure modes surfaced by the `/connect` seams.
#[derive(Debug, Clone, Error)]
pub enum ConnectError {
    /// The user cancelled the secure prompt or device-flow.
    #[error("connect cancelled")]
    Cancelled,
    /// Keychain / secure-storage write failed.
    #[error("could not store credential: {0}")]
    Storage(String),
    /// Network / device-flow transport error.
    #[error("network error: {0}")]
    Network(String),
    /// GitHub reported a terminal device-flow error (e.g. access_denied).
    #[error("device authorization failed: {0}")]
    DeviceFailed(String),
}

/// Engine seam: prompt for a secret (tui renders a masked input) and persist it
/// under a credential id. One call does prompt + store so the raw secret never
/// crosses back through the command layer.
#[async_trait]
pub trait ConnectCredentialWriter: Send + Sync {
    /// Prompt for the provider's API key and store it under `credential_id`.
    async fn prompt_and_store_key(&self, credential_id: &str) -> Result<(), ConnectError>;
}

/// One step of the Copilot device-flow, surfaced so the tui renders the code.
#[derive(Debug, Clone)]
pub struct CopilotConnectStep {
    /// Code the user types at `verification_uri`.
    pub user_code: String,
    /// URL the user opens to authorize.
    pub verification_uri: String,
}

/// Engine seam: drive the GitHub Copilot device-flow end-to-end. `begin` returns
/// the code to display; `poll_to_completion` runs the poll loop and stores the
/// token on success.
#[async_trait]
pub trait CopilotConnectDriver: Send + Sync {
    /// Request a device code; the caller displays it then polls.
    async fn begin(&self) -> Result<CopilotConnectStep, ConnectError>;
    /// Poll until authorized (or terminal), storing the token on success.
    async fn poll_to_completion(&self, step: &CopilotConnectStep) -> Result<(), ConnectError>;
}

/// `/connect` handler over the engine-supplied seams.
pub struct ConnectHandler {
    writer: Arc<dyn ConnectCredentialWriter>,
    copilot: Arc<dyn CopilotConnectDriver>,
}

impl ConnectHandler {
    /// Construct over the writer + Copilot seams.
    #[must_use]
    pub fn new(writer: Arc<dyn ConnectCredentialWriter>, copilot: Arc<dyn CopilotConnectDriver>) -> Self {
        Self { writer, copilot }
    }
}

#[async_trait]
impl BuiltinCommandHandler for ConnectHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let provider = args.raw_args.trim();
        if provider.is_empty() {
            return CommandResult::Done {
                display: Some(
                    "Usage: /connect <provider>  (e.g. openrouter, deepseek, glm-coding, github-copilot)".to_string(),
                ),
            };
        }
        if provider == "github-copilot" {
            let step = match self.copilot.begin().await {
                Ok(s) => s,
                Err(e) => return CommandResult::Done { display: Some(format!("Could not start Copilot sign-in: {e}")) },
            };
            let intro = format!(
                "To sign in to GitHub Copilot, open {} and enter code {}",
                step.verification_uri, step.user_code
            );
            match self.copilot.poll_to_completion(&step).await {
                Ok(()) => CommandResult::Done { display: Some(format!("{intro}\nConnected github-copilot.")) },
                Err(e) => CommandResult::Done { display: Some(format!("{intro}\nCould not connect github-copilot: {e}")) },
            }
        } else {
            match self.writer.prompt_and_store_key(provider).await {
                Ok(()) => CommandResult::Done { display: Some(format!("Connected {provider}.")) },
                Err(e) => CommandResult::Done { display: Some(format!("Could not connect {provider}: {e}")) },
            }
        }
    }
    fn name(&self) -> &str { "connect" }
    fn description(&self) -> &str { CONNECT_DESCRIPTION }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "connect".to_string(),
            raw_args: raw.to_string(),
            positional_args: raw.split_whitespace().map(str::to_string).collect(),
        }
    }

    struct MockWriter {
        last_id: StdMutex<Option<String>>,
        result: StdMutex<Result<(), ConnectError>>,
    }
    impl MockWriter {
        fn ok() -> Self { Self { last_id: StdMutex::new(None), result: StdMutex::new(Ok(())) } }
        fn err(e: ConnectError) -> Self { Self { last_id: StdMutex::new(None), result: StdMutex::new(Err(e)) } }
    }
    #[async_trait]
    impl ConnectCredentialWriter for MockWriter {
        async fn prompt_and_store_key(&self, credential_id: &str) -> Result<(), ConnectError> {
            *self.last_id.lock().unwrap() = Some(credential_id.to_string());
            self.result.lock().unwrap().clone()
        }
    }

    struct PanicCopilot;
    #[async_trait]
    impl CopilotConnectDriver for PanicCopilot {
        async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
            panic!("api-key path must not call the copilot driver");
        }
        async fn poll_to_completion(&self, _s: &CopilotConnectStep) -> Result<(), ConnectError> {
            panic!("api-key path must not call the copilot driver");
        }
    }

    #[tokio::test]
    async fn no_arg_shows_usage() {
        let h = ConnectHandler::new(Arc::new(MockWriter::ok()), Arc::new(PanicCopilot));
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("")).await {
            assert!(s.starts_with("Usage: /connect <provider>"));
        } else { panic!("expected Done"); }
    }

    #[tokio::test]
    async fn api_key_provider_stores_under_its_id() {
        let writer = Arc::new(MockWriter::ok());
        let h = ConnectHandler::new(writer.clone(), Arc::new(PanicCopilot));
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("openrouter")).await {
            assert_eq!(s, "Connected openrouter.");
        } else { panic!("expected Done"); }
        assert_eq!(writer.last_id.lock().unwrap().as_deref(), Some("openrouter"));
    }

    #[tokio::test]
    async fn api_key_store_failure_is_surfaced() {
        let writer = Arc::new(MockWriter::err(ConnectError::Storage("keychain locked".into())));
        let h = ConnectHandler::new(writer, Arc::new(PanicCopilot));
        if let CommandResult::Done { display: Some(s) } = h.handle(&args("deepseek")).await {
            assert_eq!(s, "Could not connect deepseek: could not store credential: keychain locked");
        } else { panic!("expected Done"); }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = ConnectHandler::new(Arc::new(MockWriter::ok()), Arc::new(PanicCopilot));
        assert_eq!(h.name(), "connect");
        assert_eq!(h.description(), CONNECT_DESCRIPTION);
    }
}
```

  Append a registrar test to `lingxi-code/commands/core/src/register.rs`:
```rust
#[cfg(test)]
mod connect_tests {
    use super::*;
    use crate::connect::{ConnectCredentialWriter, ConnectError, CopilotConnectDriver, CopilotConnectStep};
    use async_trait::async_trait;

    struct NoopWriter;
    #[async_trait]
    impl ConnectCredentialWriter for NoopWriter {
        async fn prompt_and_store_key(&self, _id: &str) -> Result<(), ConnectError> { Ok(()) }
    }
    struct NoopCopilot;
    #[async_trait]
    impl CopilotConnectDriver for NoopCopilot {
        async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
            Ok(CopilotConnectStep { user_code: "X".into(), verification_uri: "https://github.com/login/device".into() })
        }
        async fn poll_to_completion(&self, _s: &CopilotConnectStep) -> Result<(), ConnectError> { Ok(()) }
    }

    #[test]
    fn connect_resolves_after_registration() {
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        register_core_connect(&mut reg, Arc::new(NoopWriter), Arc::new(NoopCopilot));
        assert!(reg.resolve("connect").is_some(), "/connect missing");
        assert!(reg.get_handler("connect").is_some(), "/connect handler missing");
    }
}
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p command-core connect`  Expected: FAIL to compile — module/handler/registrar missing.

- [ ] **Step 3: Implement** — the `connect.rs` body above IS the implementation. In `lib.rs` add (alphabetical, after `pub mod config;`): `pub mod connect;` and (after `pub use config::ConfigHandler;`):
```rust
pub use connect::{
    ConnectCredentialWriter, ConnectError, ConnectHandler, CopilotConnectDriver, CopilotConnectStep,
};
```
  In `register.rs` add the registrar (after `register_core_batch_6`, or the last existing batch):
```rust
/// Register the additive `/connect` command (Plan 3c). Not a locked builtin name,
/// so this is a pure addition; idempotent. Composition roots call this after the
/// core batch registrars, threading the engine-built seams.
pub fn register_core_connect(
    reg: &mut CommandRegistry,
    writer: Arc<dyn crate::ConnectCredentialWriter>,
    copilot: Arc<dyn crate::CopilotConnectDriver>,
) {
    use crate::ConnectHandler;
    reg.register_builtin_handler(Arc::new(ConnectHandler::new(writer, copilot)));
}
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p command-core connect`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/commands/core/src/connect.rs lingxi-code/commands/core/src/lib.rs lingxi-code/commands/core/src/register.rs && git commit -m "feat(commands): /connect command + ConnectCredentialWriter/CopilotConnectDriver seams"`

---

### Task 17: Engine `EngineCredentialWriter` (API-key path) + `SecureKeyPrompt` port + `DesktopConfig.connect_prompt`

The engine writer reads a secret from a host secure-input port (tui-supplied; mock in tests) then persists it via `CredentialManager::set_provider_key`. The secure prompt is a tui responsibility, so the engine writer takes a `SecureKeyPrompt` port. The plumbing decision (reconciliation): `DesktopConfig` gains `pub connect_prompt: Option<Arc<dyn crate::connect::SecureKeyPrompt>>` (headless default `None` → a no-op prompt that returns `None`, so `/connect` of an api-key provider in a headless build cleanly reports cancelled).

**Files:**
- Create: `lingxi-code/apps/engine-desktop/src/connect.rs`
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (`mod connect;` + `DesktopConfig.connect_prompt` field)

- [ ] **Step 1: Write the failing test** — create `lingxi-code/apps/engine-desktop/src/connect.rs`:

```rust
//! Engine implementations of the `/connect` seams (`command_core::connect`).
//!
//! [`EngineCredentialWriter`] bridges the command-layer `ConnectCredentialWriter`
//! onto a host secure-input port + the keychain (`CredentialManager::set_provider_key`).
//! The secure prompt is rendered by the tui; tests inject a mock prompt.

use async_trait::async_trait;
use command_core::{ConnectCredentialWriter, ConnectError};
use secret::CredentialManager;
use std::sync::Arc;

/// Host port for a masked secret prompt. The tui implements this against its
/// secure-input widget; headless/test callers inject a canned value.
#[async_trait]
pub trait SecureKeyPrompt: Send + Sync {
    /// Prompt the user (masked) for the API key of `provider_label`.
    /// Returns `None` if the user cancelled.
    async fn prompt(&self, provider_label: &str) -> Option<String>;
}

/// A headless no-op prompt (returns `None` — cancels). The default when no tui
/// prompt is wired (`DesktopConfig.connect_prompt == None`).
pub struct NoopKeyPrompt;
#[async_trait]
impl SecureKeyPrompt for NoopKeyPrompt {
    async fn prompt(&self, _provider_label: &str) -> Option<String> {
        None
    }
}

/// Engine writer: prompt via the host port, then persist under `credential_id`.
pub struct EngineCredentialWriter {
    credentials: Arc<CredentialManager>,
    prompt: Arc<dyn SecureKeyPrompt>,
}

impl EngineCredentialWriter {
    /// Construct over the shared credential manager + host prompt port.
    #[must_use]
    pub fn new(credentials: Arc<CredentialManager>, prompt: Arc<dyn SecureKeyPrompt>) -> Self {
        Self { credentials, prompt }
    }
}

#[async_trait]
impl ConnectCredentialWriter for EngineCredentialWriter {
    async fn prompt_and_store_key(&self, credential_id: &str) -> Result<(), ConnectError> {
        let Some(key) = self.prompt.prompt(credential_id).await else {
            return Err(ConnectError::Cancelled);
        };
        if key.trim().is_empty() {
            return Err(ConnectError::Cancelled);
        }
        self.credentials
            .set_provider_key(credential_id, &key)
            .await
            .map_err(|e| ConnectError::Storage(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_posix::{PosixClock, PosixHttp};
    use platform_posix_minimal::PlainTextSecureStorage;
    use traits::{Clock, HttpTransport, SecureStorage};

    struct CannedPrompt(Option<String>);
    #[async_trait]
    impl SecureKeyPrompt for CannedPrompt {
        async fn prompt(&self, _label: &str) -> Option<String> {
            self.0.clone()
        }
    }

    fn manager() -> Arc<CredentialManager> {
        let storage: Arc<dyn SecureStorage> = Arc::new(PlainTextSecureStorage::new());
        let clock: Arc<dyn Clock> = Arc::new(PosixClock::new());
        let http: Arc<dyn HttpTransport> = Arc::new(PosixHttp::new());
        Arc::new(CredentialManager::new(storage, clock, http))
    }

    #[tokio::test]
    async fn store_roundtrips_through_keychain() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm.clone(), Arc::new(CannedPrompt(Some("sk-test-123".into()))));
        writer.prompt_and_store_key("openrouter").await.expect("store ok");
        let got = cm.get_provider_key("openrouter").await.expect("read ok").expect("present");
        assert_eq!(got.expose_secret(), "sk-test-123");
    }

    #[tokio::test]
    async fn cancelled_prompt_yields_cancelled() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm, Arc::new(CannedPrompt(None)));
        match writer.prompt_and_store_key("deepseek").await {
            Err(ConnectError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_key_is_cancelled_not_stored() {
        let cm = manager();
        let writer = EngineCredentialWriter::new(cm.clone(), Arc::new(CannedPrompt(Some("   ".into()))));
        match writer.prompt_and_store_key("deepseek").await {
            Err(ConnectError::Cancelled) => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(cm.get_provider_key("deepseek").await.expect("read").is_none());
    }
}
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop connect::tests::store_roundtrips`  Expected: FAIL — `mod connect` not declared.

- [ ] **Step 3: Implement** — the `connect.rs` body above IS the implementation. In `lib.rs`:
  - declare the module (near the other `mod` declarations, e.g. after `mod skill_loader;`): `mod connect;`
  - add the field to `DesktopConfig` (after `permission_mode`, lib.rs:500 block):
```rust
    /// Plan 3c: host secure-input port for `/connect <api-key-provider>`. The tui
    /// supplies its masked-input widget; `None` → a headless no-op prompt
    /// (`crate::connect::NoopKeyPrompt`) that cancels.
    pub connect_prompt: Option<Arc<dyn crate::connect::SecureKeyPrompt>>,
```
  Update every existing `DesktopConfig { … }` construction (in-crate tests + any caller) to set `connect_prompt: None`. (Plan tasks elsewhere already set it: the integration test in Task 14.)

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop connect::tests`  Expected: PASS (3 tests).

- [ ] **Step 5: Commit** — `git add lingxi-code/apps/engine-desktop/src/connect.rs lingxi-code/apps/engine-desktop/src/lib.rs && git commit -m "feat(engine): EngineCredentialWriter + SecureKeyPrompt port + DesktopConfig.connect_prompt"`

---

### Task 18: Engine `CopilotConnectDriver` + `PosixCopilotHttp` + wire `/connect` into `desktop_command_registry` + `build()`

Implements the Copilot device-flow: a host `CopilotHttp` (the `llm_client::copilot::CopilotHttp` seam) over `PosixHttp`, then the begin→display→poll loop via `CopilotLogin`, storing the token under `github-copilot` on `Success` using the Task-7 `CopilotSecret::token_for_storage()`. Then makes `/connect` reachable: `desktop_command_registry` grows the two seams (4→6 args) and calls `register_core_connect`; `build()` constructs both seams and threads them.

> Verified real types: `protocol::HttpRequest { method: HttpMethod, url: String, headers: Vec<(String,String)>, body: Option<String>, timeout: Option<Duration> }` (transport.rs:28); `protocol::HttpResponse.body: String` (transport.rs:50); `HttpMethod::Post`. `CopilotLogin::new(http)`, `begin() -> DeviceCodeResponse { user_code, verification_uri, interval_secs }`, `poll_once(&dc) -> PollOutcome::{Success(CopilotSecret), Pending{interval_secs}, SlowDown{interval_secs}, Failed{error}}` (login.rs:24-129); `COPILOT_CLIENT_ID = "Ov23li8tweQw6odWQebz"`. `desktop_command_registry(handle, auth, cwd, claude_home)` is 4-arg today (lib.rs:630); `register_core_batch_*` are the existing registrars.

**Files:**
- Modify: `lingxi-code/apps/engine-desktop/src/connect.rs` (add `PosixCopilotHttp` + `EngineCopilotConnect` + `PollSleeper`)
- Modify: `lingxi-code/apps/engine-desktop/src/lib.rs` (`desktop_command_registry` signature + `build()` wiring)

- [ ] **Step 1: Write the failing test** — add to `mod tests` in `connect.rs`:

```rust
    use command_core::{CopilotConnectDriver, CopilotConnectStep};
    use llm_client::copilot::{CopilotHttp, CopilotLogin, COPILOT_CLIENT_ID};
    use llm_client::transport::BoxFuture;
    use llm_client::LlmError;
    use serde_json::{json, Value};
    use std::sync::Mutex as StdMutex;

    struct ScriptedCopilotHttp {
        device: Value,
        tokens: StdMutex<std::collections::VecDeque<Value>>,
    }
    impl CopilotHttp for ScriptedCopilotHttp {
        fn post_json<'a>(&'a self, url: &'a str, _body: &'a Value) -> BoxFuture<'a, Result<Value, LlmError>> {
            let v = if url.contains("device/code") {
                self.device.clone()
            } else {
                self.tokens.lock().unwrap().pop_front().unwrap_or_else(|| json!({ "error": "expired_token" }))
            };
            Box::pin(async move { Ok(v) })
        }
    }

    struct InstantSleeper;
    #[async_trait]
    impl PollSleeper for InstantSleeper {
        async fn sleep_secs(&self, _secs: u64) {}
    }

    fn copilot_driver(cm: Arc<CredentialManager>, device: Value, tokens: Vec<Value>) -> EngineCopilotConnect<ScriptedCopilotHttp> {
        let http = ScriptedCopilotHttp { device, tokens: StdMutex::new(tokens.into_iter().collect()) };
        EngineCopilotConnect::with_parts(cm, CopilotLogin::new(http), Arc::new(InstantSleeper))
    }

    #[tokio::test]
    async fn begin_surfaces_user_code_and_uri() {
        let cm = manager();
        let driver = copilot_driver(
            cm,
            json!({ "user_code": "WDJB-MJHT", "verification_uri": "https://github.com/login/device", "device_code": "dev-1", "interval": 1 }),
            vec![],
        );
        let step = driver.begin().await.expect("begin ok");
        assert_eq!(step.user_code, "WDJB-MJHT");
        assert_eq!(step.verification_uri, "https://github.com/login/device");
    }

    #[tokio::test]
    async fn poll_advances_through_pending_then_stores_token() {
        let cm = manager();
        let driver = copilot_driver(
            cm.clone(),
            json!({ "user_code": "AAAA-BBBB", "verification_uri": "https://github.com/login/device", "device_code": "dev-2", "interval": 1 }),
            vec![
                json!({ "error": "authorization_pending" }),
                json!({ "error": "slow_down" }),
                json!({ "access_token": "ght_live_token" }),
            ],
        );
        let step = driver.begin().await.expect("begin");
        driver.poll_to_completion(&step).await.expect("poll ok");
        let got = cm.get_provider_key("github-copilot").await.expect("read").expect("present");
        assert_eq!(got.expose_secret(), "ght_live_token");
    }

    #[tokio::test]
    async fn poll_terminal_error_is_surfaced_and_not_stored() {
        let cm = manager();
        let driver = copilot_driver(
            cm.clone(),
            json!({ "user_code": "CCCC-DDDD", "verification_uri": "https://github.com/login/device", "device_code": "dev-3", "interval": 1 }),
            vec![json!({ "error": "access_denied" })],
        );
        let step = driver.begin().await.expect("begin");
        match driver.poll_to_completion(&step).await {
            Err(ConnectError::DeviceFailed(e)) => assert_eq!(e, "access_denied"),
            other => panic!("expected DeviceFailed, got {other:?}"),
        }
        assert!(cm.get_provider_key("github-copilot").await.expect("read").is_none());
    }

    #[test]
    fn uses_opencode_client_id() {
        assert_eq!(COPILOT_CLIENT_ID, "Ov23li8tweQw6odWQebz");
    }
```

  Add to `lib.rs` `mod tests` (registry wiring):
```rust
    #[tokio::test]
    async fn desktop_registry_exposes_connect() {
        use command_core::{ConnectCredentialWriter, ConnectError, CopilotConnectDriver, CopilotConnectStep};
        use async_trait::async_trait;
        struct W;
        #[async_trait]
        impl ConnectCredentialWriter for W {
            async fn prompt_and_store_key(&self, _id: &str) -> Result<(), ConnectError> { Ok(()) }
        }
        struct C;
        #[async_trait]
        impl CopilotConnectDriver for C {
            async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
                Ok(CopilotConnectStep { user_code: "X".into(), verification_uri: "u".into() })
            }
            async fn poll_to_completion(&self, _s: &CopilotConnectStep) -> Result<(), ConnectError> { Ok(()) }
        }
        let handle = std::sync::Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let auth = make_test_auth_handle(); // reuse the helper the sibling registry/build tests use
        let tmp = std::env::temp_dir();
        let reg = desktop_command_registry(handle, auth, &tmp, &tmp, std::sync::Arc::new(W), std::sync::Arc::new(C)).await;
        assert!(reg.get_handler("connect").is_some(), "/connect not wired into desktop registry");
    }
```
  > `make_test_auth_handle()` is illustrative — use the EXACT `AuthHandle` test double the existing `desktop_command_registry`/`build` tests already construct; do not introduce a new helper.

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop connect::tests::poll_advances` and `... desktop_registry_exposes_connect`  Expected: FAIL — `EngineCopilotConnect`/`PollSleeper`/`with_parts` missing; `desktop_command_registry` still 4-arg.

- [ ] **Step 3: Implement** —

  (a) add to `connect.rs` (above `#[cfg(test)] mod tests`):
```rust
use command_core::{CopilotConnectDriver, CopilotConnectStep};
use llm_client::copilot::{CopilotHttp, CopilotLogin, DeviceCodeResponse, PollOutcome};
use llm_client::transport::BoxFuture;
use llm_client::LlmError;
use platform_posix::PosixHttp;
use protocol::{HttpMethod, HttpRequest};
use serde_json::Value;
use std::sync::Mutex as StdMutex;
use traits::HttpTransport;

/// Credential id under which the GitHub Copilot OAuth token is stored. Matches
/// the catalog preset's `profile_name`.
const COPILOT_CREDENTIAL_ID: &str = "github-copilot";

/// Host `CopilotHttp` over the production `PosixHttp` transport.
pub struct PosixCopilotHttp {
    http: PosixHttp,
}

impl PosixCopilotHttp {
    /// Construct over a fresh `PosixHttp`.
    #[must_use]
    pub fn new() -> Self {
        Self { http: PosixHttp::new() }
    }
}

impl Default for PosixCopilotHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl CopilotHttp for PosixCopilotHttp {
    fn post_json<'a>(&'a self, url: &'a str, body: &'a Value) -> BoxFuture<'a, Result<Value, LlmError>> {
        Box::pin(async move {
            let payload = serde_json::to_string(body)
                .map_err(|e| LlmError::Transport { message: e.to_string() })?;
            // protocol::HttpRequest: method is HttpMethod, headers are Vec pairs,
            // body is Option<String>, plus a timeout field.
            let req = HttpRequest {
                method: HttpMethod::Post,
                url: url.to_string(),
                headers: vec![
                    ("Accept".to_string(), "application/json".to_string()),
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("User-Agent".to_string(), "LingXi-Code".to_string()),
                ],
                body: Some(payload),
                timeout: None,
            };
            let resp = self
                .http
                .request(req)
                .await
                .map_err(|e| LlmError::Transport { message: e.to_string() })?;
            serde_json::from_str(&resp.body)
                .map_err(|e| LlmError::Transport { message: format!("copilot json: {e}") })
        })
    }
}

/// Sleep port so the poll loop is testable without real time.
#[async_trait]
pub trait PollSleeper: Send + Sync {
    /// Sleep for `secs` seconds before the next poll.
    async fn sleep_secs(&self, secs: u64);
}

/// Production sleeper backed by `tokio::time::sleep`.
pub struct TokioSleeper;
#[async_trait]
impl PollSleeper for TokioSleeper {
    async fn sleep_secs(&self, secs: u64) {
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
}

/// Engine Copilot device-flow driver: owns the pure `CopilotLogin` state machine,
/// the sleep port, and the keychain. `begin` returns the displayable step (and
/// caches the `DeviceCodeResponse`); `poll_to_completion` runs the SlowDown/Pending
/// loop and stores the token on success.
pub struct EngineCopilotConnect<H: CopilotHttp> {
    credentials: Arc<CredentialManager>,
    login: CopilotLogin<H>,
    sleeper: Arc<dyn PollSleeper>,
    cached: StdMutex<Option<DeviceCodeResponse>>,
}

impl EngineCopilotConnect<PosixCopilotHttp> {
    /// Production constructor: device-flow over `PosixHttp`, real `tokio` sleeps.
    #[must_use]
    pub fn new(credentials: Arc<CredentialManager>) -> Self {
        Self::with_parts(credentials, CopilotLogin::new(PosixCopilotHttp::new()), Arc::new(TokioSleeper))
    }
}

impl<H: CopilotHttp> EngineCopilotConnect<H> {
    /// Construct over an injected `CopilotLogin` + sleeper (test seam).
    #[must_use]
    pub fn with_parts(
        credentials: Arc<CredentialManager>,
        login: CopilotLogin<H>,
        sleeper: Arc<dyn PollSleeper>,
    ) -> Self {
        Self { credentials, login, sleeper, cached: StdMutex::new(None) }
    }
}

#[async_trait]
impl<H: CopilotHttp> CopilotConnectDriver for EngineCopilotConnect<H> {
    async fn begin(&self) -> Result<CopilotConnectStep, ConnectError> {
        let dc = self.login.begin().await.map_err(|e| ConnectError::Network(e.to_string()))?;
        let step = CopilotConnectStep {
            user_code: dc.user_code.clone(),
            verification_uri: dc.verification_uri.clone(),
        };
        *self.cached.lock().unwrap() = Some(dc);
        Ok(step)
    }

    async fn poll_to_completion(&self, _step: &CopilotConnectStep) -> Result<(), ConnectError> {
        let dc = self
            .cached
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| ConnectError::Network("begin() was not called".to_string()))?;
        loop {
            match self.login.poll_once(&dc).await.map_err(|e| ConnectError::Network(e.to_string()))? {
                PollOutcome::Success(secret) => {
                    // §10 frozen-crate exception accessor (Task 7).
                    let token = secret.token_for_storage().to_string();
                    return self
                        .credentials
                        .set_provider_key(COPILOT_CREDENTIAL_ID, &token)
                        .await
                        .map_err(|e| ConnectError::Storage(e.to_string()));
                }
                PollOutcome::Pending { interval_secs } | PollOutcome::SlowDown { interval_secs } => {
                    self.sleeper.sleep_secs(interval_secs).await;
                }
                PollOutcome::Failed { error } => return Err(ConnectError::DeviceFailed(error)),
            }
        }
    }
}
```

  (b) `desktop_command_registry` (lib.rs:630) — 4→6 args, call `register_core_connect`:
```rust
#[must_use]
pub async fn desktop_command_registry(
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
    cwd: &std::path::Path,
    claude_home: &std::path::Path,
    connect_writer: Arc<dyn command_core::ConnectCredentialWriter>,
    connect_copilot: Arc<dyn command_core::CopilotConnectDriver>,
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    register_core_batch_1(&mut reg, handle.clone());
    register_core_batch_2(&mut reg, handle.clone(), auth);
    register_core_batch_4(&mut reg, handle.clone());
    register_core_batch_5(&mut reg, handle);
    command_core::register::register_core_connect(&mut reg, connect_writer, connect_copilot);
    command_desktop::register(&mut reg);
    let home = dirs::home_dir().unwrap_or_else(|| claude_home.to_path_buf());
    let registered = command_core::load_and_register_custom_commands(
        &mut reg, cwd, claude_home, &crate::settings_watch::managed_settings_dir(), &home,
    )
    .await;
    tracing::debug!(custom_commands = registered, "registered custom slash commands");
    reg
}
```
  > Keep the EXACT existing batch-registrar call list (1/2/4/5 shown — match what is actually there). `command_core::register::register_core_connect` is full-path so no new top-level import is needed.

  (c) in `build()`, after `credentials` is built (lib.rs:916), construct both seams and pass them to the `desktop_command_registry(...)` call inside `build()` (and update any other call site, e.g. a bridge/test helper, in this same task):
```rust
    let connect_copilot: Arc<dyn command_core::CopilotConnectDriver> =
        Arc::new(crate::connect::EngineCopilotConnect::new(credentials.clone()));
    let connect_writer: Arc<dyn command_core::ConnectCredentialWriter> =
        Arc::new(crate::connect::EngineCredentialWriter::new(
            credentials.clone(),
            cfg.connect_prompt
                .clone()
                .unwrap_or_else(|| Arc::new(crate::connect::NoopKeyPrompt) as Arc<dyn crate::connect::SecureKeyPrompt>),
        ));
```
  and change the existing `desktop_command_registry(handle, auth, &cwd, &claude_home)` call in `build()` to `desktop_command_registry(handle, auth, &cwd, &claude_home, connect_writer, connect_copilot)`.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop connect::tests` then `... desktop_registry_exposes_connect` then full `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p engine-desktop`  Expected: PASS (no call-site regressions).

- [ ] **Step 5: Commit** — `git add lingxi-code/apps/engine-desktop/src/connect.rs lingxi-code/apps/engine-desktop/src/lib.rs && git commit -m "feat(engine): Copilot device-flow driver + PosixCopilotHttp + wire /connect into registry/build"`

---

## tui /connect UI + picker Connect badges + select-launches-/connect

All tui-only; frozen crates untouched. Availability rides a sibling `BTreeMap<provider_id, bool>` joined into `ModelRow` at the tui layer (spec §8). Verified fixtures: `ModelRow { display_model, request_model, provider_id, provider_label }` (model.rs:19); `ModelScreenState::new(rows, recent, current)` (model.rs:141); `ModelOutcome::{Stay, Commit{provider_id, request_model}, Cancel}` (model.rs:124); `traits::orchestrator::ModelListing { display_model, request_model, provider_id, provider_label }`; `AppState::new(status: StatusSnapshot)` with `StatusSnapshot: Default` (state.rs:817,447); existing `state.rs` tests build via `AppState::new(fake_status())` where `fake_status()` uses `..StatusSnapshot::default()`; `pump_open_model` calls `build_model_entries(existing, catalog)` at root.rs:1155 and pumps return `bool` (caller bumps `redraw` on `true`).

### Task 19: Thread an `available` flag onto `ModelRow` + `build_model_entries`

Per spec §8 availability is a sibling `BTreeMap<provider_id, bool>`; `build_model_entries` joins it by `provider_id`; a provider absent from the map defaults to `available = true`.

**Files:**
- Modify: `lingxi-code/tui/src/screens/model.rs`

- [ ] **Step 1: Write the failing test** — append to `mod entries_tests`:
```rust
    #[test]
    fn availability_joins_by_provider_id_default_true() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let existing = vec!["claude-opus-4-7".to_string()];
        let catalog = vec![
            ModelListing { display_model: "DeepSeek Chat".to_string(), request_model: "deepseek-chat".to_string(), provider_id: "deepseek".to_string(), provider_label: "DeepSeek".to_string() },
            ModelListing { display_model: "GPT-5.4 nano".to_string(), request_model: "gpt-5.4-nano".to_string(), provider_id: "github-copilot".to_string(), provider_label: "GitHub Copilot".to_string() },
        ];
        let mut avail = BTreeMap::new();
        avail.insert("deepseek".to_string(), true);
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(existing, catalog, &avail);
        assert!(rows.iter().find(|r| r.request_model == "claude-opus-4-7").unwrap().available, "absent provider defaults available");
        assert!(rows.iter().find(|r| r.request_model == "deepseek-chat").unwrap().available);
        assert!(!rows.iter().find(|r| r.request_model == "gpt-5.4-nano").unwrap().available);
    }

    #[test]
    fn empty_availability_map_keeps_all_rows_available() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string(), "openai/gpt-4o".to_string()],
            vec![ModelListing { display_model: "DeepSeek Chat".to_string(), request_model: "deepseek-chat".to_string(), provider_id: "deepseek".to_string(), provider_label: "DeepSeek".to_string() }],
            &BTreeMap::new(),
        );
        assert!(rows.iter().all(|r| r.available), "empty map → all available");
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui availability_joins_by_provider_id_default_true`  Expected: compile error — `build_model_entries` takes 2 args, `ModelRow` has no `available`.

- [ ] **Step 3: Implement** —
  1. Add to `ModelRow` (after `provider_label`):
```rust
    /// Whether this row's provider has a usable credential. Joined from the
    /// sibling availability map at build time (spec §8); a provider absent from
    /// that map defaults to `true`. Drives the Connect badge + select-launches-`/connect`.
    pub available: bool,
```
  2. Replace `build_model_entries` to take the map + stamp the field on every row:
```rust
#[must_use]
pub fn build_model_entries(
    existing: Vec<String>,
    catalog: Vec<traits::orchestrator::ModelListing>,
    availability: &std::collections::BTreeMap<String, bool>,
) -> Vec<ModelRow> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let avail = |pid: &str| availability.get(pid).copied().unwrap_or(true);

    for id in existing {
        if !seen.insert(id.clone()) {
            continue;
        }
        let row = if let Some(rest) = id.strip_prefix('@') {
            ModelRow { display_model: format!("@{rest}"), request_model: id.clone(), provider_id: "alias".to_string(), provider_label: "Aliases".to_string(), available: true }
        } else if let Some((p, m)) = id.split_once('/') {
            ModelRow { display_model: m.to_string(), request_model: id.clone(), provider_id: p.to_string(), provider_label: existing_provider_label(p), available: avail(p) }
        } else {
            ModelRow { display_model: id.clone(), request_model: id.clone(), provider_id: "builtin".to_string(), provider_label: "Built-in".to_string(), available: true }
        };
        rows.push(row);
    }

    for m in catalog {
        if !seen.insert(m.request_model.clone()) {
            continue;
        }
        let available = avail(&m.provider_id);
        rows.push(ModelRow { display_model: m.display_model, request_model: m.request_model, provider_id: m.provider_id, provider_label: m.provider_label, available });
    }
    rows
}
```
  3. Fix the existing `build_model_entries(...)` calls in `reducer_tests::rows`, `render_tests::st`, and the existing `entries_tests` (`merges_existing_and_catalog_with_groups`, `dedups_by_request_model_existing_wins`) by adding a trailing `&std::collections::BTreeMap::new()` arg.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib screens::model`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/screens/model.rs && git commit -m "tui(model): thread provider availability onto ModelRow/build_model_entries"`

---

### Task 20: `pump_open_model` passes the App's availability map into `build_model_entries`

`build_model_entries` now needs the sibling map. The map is produced engine-side and threaded into the tui `App` (Task 23). `pump_open_model` reads `AppState.provider_availability` (added in Task 23) and passes it. Sequence: Task 23 must add the field before this task compiles — so this task assumes the field exists; if executing this before Task 23, add the `provider_availability: BTreeMap<String, bool>` field + `None`-safe init from Task 23 first.

**Files:**
- Modify: `lingxi-code/tui/src/root.rs` (`pump_open_model`)

- [ ] **Step 1: Write the failing test** — the behavioral guard is Task 19's `empty_availability_map_keeps_all_rows_available` (already added) + the full `-p tui` compile. No new pure test is added here (`pump_open_model` is async/handle-driven).

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib`  Expected: FAIL to compile — `pump_open_model` calls `build_model_entries(existing, catalog)` with 2 args.

- [ ] **Step 3: Implement** — in `tui/src/root.rs` `pump_open_model`, replace the call at root.rs:1155 (`let rows = crate::screens::model::build_model_entries(existing, catalog);`). The availability map is read from `AppState` (Task 23 adds the field) before the second lock — clone it while holding the first lock or read it from the `st` already in scope:
```rust
    // Availability sibling map (spec §8): the engine-computed per-provider
    // availability threaded onto the App (Task 23). Join it so the picker can
    // badge unconfigured providers + launch `/connect`. Read it under the lock,
    // then build the rows.
    let availability = {
        let st = state.lock().await;
        st.provider_availability.clone()
    };
    let rows = crate::screens::model::build_model_entries(existing, catalog, &availability);
```
  > Place the `availability` read where the existing code already re-locks `state` (the function locks twice — once to consume `pending_open_model`, once before `open_model`). Read `provider_availability` in the SAME lock scope that reads `st.status.model` (just before `st.open_model(...)`), to avoid an extra lock. Concretely, fold the clone into the final lock block: `let current = st.status.model.clone(); let availability = st.provider_availability.clone();` and move the `build_model_entries` call to use it there, OR read it as shown above before the final lock. Either is correct; keep it under a lock.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/root.rs && git commit -m "tui(root): pass App availability map into build_model_entries"`

---

### Task 21: Render a `[Connect]` badge on unconfigured-provider rows

In `render_model_to_string`, an unavailable row shows `[Connect]` instead of the `(current)` badge (spec §6.4).

**Files:**
- Modify: `lingxi-code/tui/src/screens/model.rs`

- [ ] **Step 1: Write the failing test** — append to `mod render_tests` (add `use traits::orchestrator::ModelListing;` if absent):
```rust
    fn st_with_unconfigured() -> ModelScreenState {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let mut avail = BTreeMap::new();
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![ModelListing { display_model: "GPT-5.4 nano".to_string(), request_model: "gpt-5.4-nano".to_string(), provider_id: "github-copilot".to_string(), provider_label: "GitHub Copilot".to_string() }],
            &avail,
        );
        ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string())
    }

    #[test]
    fn renders_connect_badge_on_unconfigured_provider() {
        let out = render_model_to_string(&st_with_unconfigured());
        assert!(out.contains("\u{276F} claude-opus-4-7  \u{00B7} Built-in (current)\n"));
        assert!(out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot [Connect]\n"));
        assert!(!out.contains("GPT-5.4 nano  \u{00B7} GitHub Copilot (current)"));
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui renders_connect_badge_on_unconfigured_provider`  Expected: assertion failure — `[Connect]` not present.

- [ ] **Step 3: Implement** — in `render_model_to_string`'s `VisibleLine::Item(idx)` arm, replace the current-badge block:
```rust
                out.push_str(&format!("  \u{00B7} {}", row.provider_label));
                if row.request_model == state.current {
                    out.push_str(" (current)");
                }
                out.push('\n');
```
  with:
```rust
                out.push_str(&format!("  \u{00B7} {}", row.provider_label));
                if !row.available {
                    // Unconfigured provider: badge it; Enter launches `/connect`
                    // (spec §6.4). An unconfigured row is never the active model,
                    // so Connect + (current) are mutually exclusive.
                    out.push_str(" [Connect]");
                } else if row.request_model == state.current {
                    out.push_str(" (current)");
                }
                out.push('\n');
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib screens::model`  Expected: PASS.

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/screens/model.rs && git commit -m "tui(model): render [Connect] badge on unconfigured-provider rows"`

---

### Task 22: Enter on an unconfigured row yields `ModelOutcome::Connect`

`ModelOutcome` gains `Connect { provider_id }`; `handle_model_key`'s Enter arm yields it when the highlighted row is unavailable (spec §6.4).

**Files:**
- Modify: `lingxi-code/tui/src/screens/model.rs`

- [ ] **Step 1: Write the failing test** — append to `mod reducer_tests`:
```rust
    #[test]
    fn enter_on_unconfigured_row_yields_connect() {
        use std::collections::BTreeMap;
        use traits::orchestrator::ModelListing;
        let mut avail = BTreeMap::new();
        avail.insert("github-copilot".to_string(), false);
        let rows = build_model_entries(
            vec!["claude-opus-4-7".to_string()],
            vec![ModelListing { display_model: "GPT-5.4 nano".to_string(), request_model: "gpt-5.4-nano".to_string(), provider_id: "github-copilot".to_string(), provider_label: "GitHub Copilot".to_string() }],
            &avail,
        );
        let mut st = ModelScreenState::new(rows, vec![], "claude-opus-4-7".to_string());
        for c in "gpt-5.4-nano".chars() {
            let _ = handle_model_key(&mut st, KeyCode::Char(c));
        }
        assert_eq!(
            handle_model_key(&mut st, KeyCode::Enter),
            ModelOutcome::Connect { provider_id: "github-copilot".to_string() }
        );
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui enter_on_unconfigured_row_yields_connect`  Expected: compile error — `ModelOutcome` has no `Connect`.

- [ ] **Step 3: Implement** —
  1. Add to `ModelOutcome` (after `Commit { .. }`):
```rust
    /// Enter on an UNCONFIGURED provider's row — launch `/connect <provider>`
    /// instead of switching (spec §6.4).
    Connect {
        /// Provider grouping key to connect (== `ModelRow.provider_id`).
        provider_id: String,
    },
```
  2. Replace the Enter arm of `handle_model_key`:
```rust
        KeyCode::Enter => match state.selectable().get(state.selected) {
            Some(&idx) => {
                let row = &state.rows[idx];
                if row.available {
                    ModelOutcome::Commit {
                        provider_id: row.provider_id.clone(),
                        request_model: row.request_model.clone(),
                    }
                } else {
                    ModelOutcome::Connect { provider_id: row.provider_id.clone() }
                }
            }
            None => ModelOutcome::Stay,
        },
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib screens::model`  Expected: PASS (incl. existing `enter_commits_provider_and_model`, whose row is available).

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/screens/model.rs && git commit -m "tui(model): Enter on unconfigured row yields ModelOutcome::Connect"`

---

### Task 23: Thread the engine availability map into the tui App + add `pending_connect`/`pending_store_key`/`open_connect`

Reconciliation #8: the engine `DesktopRuntime.provider_availability` (Task 14) reaches the tui App. The hop is engine `DesktopRuntime` → `apps/cli/src/init.rs` `Runtime` (init.rs:284) → `tui::session::Runtime` (session.rs:42) → `AppState`. This task adds the `provider_availability` field to `AppState` + the `Runtime` carrier + the `session.rs` wire + the cli populate, plus the `pending_connect` / `pending_store_key` flags + the `open_connect` opener used by Tasks 24–25.

> Verified: tui `AppState::new(status: StatusSnapshot)` (state.rs:817) builds a `Self { … }` with all fields defaulted; `pending_switch_model: None` (state.rs:762/872); the production App is built at `tui/src/session.rs:207` via `AppState::new(runtime.status.clone())` where `runtime: tui::session::Runtime` (session.rs:42); the cli builds that `Runtime` at `apps/cli/src/init.rs:284` from the engine `DesktopRuntime`.

**Files:**
- Modify: `lingxi-code/tui/src/state.rs` (3 fields + init + `set_provider_availability` + `open_connect`)
- Modify: `lingxi-code/tui/src/session.rs` (`Runtime.provider_availability` field + wire into `initial_state`)
- Modify: `lingxi-code/apps/cli/src/init.rs` (populate `Runtime.provider_availability` from the engine `DesktopRuntime`)

- [ ] **Step 1: Write the failing test** — append to `tui/src/state.rs` `#[cfg(test)] mod tests` (use the sibling `fake_status()` fixture):
```rust
    #[test]
    fn provider_availability_defaults_empty_and_set_applies() {
        let mut s = AppState::new(fake_status());
        assert!(s.provider_availability.is_empty());
        let mut m = std::collections::BTreeMap::new();
        m.insert("deepseek".to_string(), true);
        m.insert("github-copilot".to_string(), false);
        s.set_provider_availability(m);
        assert_eq!(s.provider_availability.get("github-copilot"), Some(&false));
    }

    #[test]
    fn pending_connect_and_store_key_default_none() {
        let s = AppState::new(fake_status());
        assert!(s.pending_connect.is_none());
        assert!(s.pending_store_key.is_none());
    }

    #[test]
    fn open_connect_sets_connect_screen() {
        let mut s = AppState::new(fake_status());
        s.open_connect(crate::screens::connect::ConnectScreenState::api_key("deepseek", "DeepSeek"));
        assert!(matches!(s.active_screen, Some(crate::screens::Screen::Connect(_))));
    }
```
  > `open_connect_sets_connect_screen` also needs `crate::screens::connect` (Task 24) + `Screen::Connect` (Task 24) to exist — sequence Task 24 before this test runs, OR split: add the 3 fields + setter here and add `open_connect` + its test in Task 24. **Recommended ordering: do Task 24 first** (it creates the `connect` screen module + `Screen::Connect`), then Task 23, then Task 25 — but Task 20 (pump_open_model) depends on the `provider_availability` field from Task 23. To avoid a cycle: Task 23 adds the 3 fields + `set_provider_availability` (no `open_connect`); the `open_connect` opener + its test move into Task 25 (which already wires the connect screen lifecycle). Adjust the `open_connect_sets_connect_screen` test to Task 25.

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui provider_availability_defaults_empty_and_set_applies pending_connect_and_store_key_default_none`  Expected: compile error — no such fields.

- [ ] **Step 3: Implement** —
  1. In `tui/src/state.rs`, add three fields (after `pending_switch_model`, ~state.rs:762):
```rust
    /// Plan 3c §8: engine-computed per-provider availability (keyed by
    /// provider/profile name), threaded from `DesktopRuntime.provider_availability`.
    /// The `/model` picker joins it via `build_model_entries` to badge unconfigured
    /// providers + launch `/connect`. Empty = every provider treated as available.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// Set by the picker's Enter on an UNCONFIGURED row (`ModelOutcome::Connect`):
    /// the provider id to `/connect`. `root::pump_open_connect` consumes it.
    pub pending_connect: Option<String>,
    /// Set by the `/connect` screen's `SubmitKey`: `(provider_id, key)` for the host
    /// to persist. `root::pump_store_provider_key` performs the keychain write.
    pub pending_store_key: Option<(String, String)>,
```
  2. In `AppState::new`'s `Self { … }` (after `pending_switch_model: None,`):
```rust
            provider_availability: std::collections::BTreeMap::new(),
            pending_connect: None,
            pending_store_key: None,
```
  3. Add the setter (near `set_command_argument_names`):
```rust
    /// Install the engine-computed per-provider availability map (spec §8).
    pub fn set_provider_availability(&mut self, map: std::collections::BTreeMap<String, bool>) {
        self.provider_availability = map;
    }
```
  4. In `tui/src/session.rs`, add to the `Runtime` struct (session.rs:42):
```rust
    /// Plan 3c §8: per-provider availability map computed by the engine `build()`
    /// (`DesktopRuntime.provider_availability`); threaded into the App so the
    /// `/model` picker can badge unconfigured providers. Empty when absent.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
```
  and wire it into `initial_state` right after it is constructed (session.rs:207):
```rust
    initial_state.set_provider_availability(runtime.provider_availability.clone());
```
  5. In `apps/cli/src/init.rs`, where the `tui::session::Runtime { … }` is built (init.rs:284), populate the new field from the engine `DesktopRuntime` (the engine runtime binding in scope — name it as the local actually holds, e.g. `desktop_rt`):
```rust
        provider_availability: desktop_rt.provider_availability.clone(),
```
  > Match the EXACT local name the engine `DesktopRuntime` is bound to in `init.rs` (it is the `build(...)` result). Every OTHER `tui::session::Runtime { … }` construction (e.g. the resume/smoke paths in `apps/cli`) must also set `provider_availability: std::collections::BTreeMap::new()`.

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui provider_availability pending_connect_and_store_key` then `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build -p cli`  Expected: PASS + cli builds.

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/state.rs lingxi-code/tui/src/session.rs lingxi-code/apps/cli/src/init.rs && git commit -m "tui+cli: thread engine availability map into App + add pending_connect/pending_store_key"`

---

### Task 24: `/connect` interactive screen — state, reducer, render

A new `tui/src/screens/connect.rs` with the pure `ConnectScreenState` + `handle_connect_key` reducer + `render_connect_to_string`, modelling the two flows (§6.3): a masked API-key field (chars masked as `•`, Enter submits) and the Copilot device-flow (display code + spinner; typing inert, host drives poll). Modeled on `memory.rs` + the `render_*_to_string` pattern.

**Files:**
- Create: `lingxi-code/tui/src/screens/connect.rs`
- Modify: `lingxi-code/tui/src/screens/mod.rs` (`pub mod connect;` + `Screen::Connect` variant)

- [ ] **Step 1: Write the failing test** — create `tui/src/screens/connect.rs` (the full module below contains the tests; for a strict RED first, the failing artifact is the file referenced by `mod.rs` not yet existing). The complete file:

```rust
//! `/connect <provider>` interactive credential setup (Plan 3c §6.3/§6.4).
//!
//! Two flows, one pure reducer: a masked API-key field (Enter submits), and the
//! Copilot OAuth device-flow (the host drives `CopilotLogin`; this screen renders
//! the code + spinner; typing is inert). Esc cancels either flow.

use crossterm::event::KeyCode;

/// Which credential flow this screen is driving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFlow {
    /// API-key providers: a masked key-input field for `provider_id`.
    ApiKey {
        /// Provider grouping key the key is stored under.
        provider_id: String,
        /// Human provider label for the header.
        label: String,
    },
    /// GitHub Copilot OAuth device-flow.
    Copilot,
}

/// Terminal/in-progress state of a Copilot device-flow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CopilotPhase {
    /// Requesting the device code.
    #[default]
    Starting,
    /// Code obtained; polling for authorization.
    Polling {
        /// Code the user types at GitHub.
        user_code: String,
        /// URL the user opens.
        verification_uri: String,
    },
    /// Authorization completed.
    Done,
    /// Device-flow failed.
    Failed {
        /// Server-reported error code.
        error: String,
    },
}

/// `/connect` screen state. Pure; the caller drives async work via [`ConnectAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectScreenState {
    /// Which flow is active.
    pub flow: ConnectFlow,
    /// API-key entry buffer (rendered masked). Unused in the Copilot flow.
    pub key_buffer: String,
    /// Copilot device-flow phase. Unused in the API-key flow.
    pub copilot: CopilotPhase,
}

/// What the caller should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectAction {
    /// Stay open (field edited / inert key).
    None,
    /// Enter on a non-empty key field — store the key for `provider_id`.
    SubmitKey {
        /// Provider/keychain id to store under.
        provider_id: String,
        /// The entered secret.
        key: String,
    },
    /// Esc — cancel the flow, store nothing, close the screen.
    Cancel,
}

impl ConnectScreenState {
    /// Open an API-key field for `provider_id` (header uses `label`).
    #[must_use]
    pub fn api_key(provider_id: &str, label: &str) -> Self {
        Self {
            flow: ConnectFlow::ApiKey { provider_id: provider_id.to_string(), label: label.to_string() },
            key_buffer: String::new(),
            copilot: CopilotPhase::Starting,
        }
    }

    /// Open the Copilot device-flow in the `Starting` phase.
    #[must_use]
    pub fn copilot_pending() -> Self {
        Self { flow: ConnectFlow::Copilot, key_buffer: String::new(), copilot: CopilotPhase::Starting }
    }

    /// Host setter: device code obtained → display + spinner.
    pub fn set_device_code(&mut self, user_code: &str, verification_uri: &str) {
        self.copilot = CopilotPhase::Polling { user_code: user_code.to_string(), verification_uri: verification_uri.to_string() };
    }

    /// Host setter: authorization completed.
    pub fn set_done(&mut self) {
        self.copilot = CopilotPhase::Done;
    }

    /// Host setter: device-flow failed.
    pub fn set_failed(&mut self, error: &str) {
        self.copilot = CopilotPhase::Failed { error: error.to_string() };
    }
}

/// Route one key into the `/connect` screen. API-key flow: chars edit the masked
/// buffer, Backspace deletes, Enter submits a non-empty key, Esc cancels. Copilot
/// flow: typing/Enter inert (the host drives the poll); Esc cancels.
#[must_use]
pub fn handle_connect_key(st: &mut ConnectScreenState, key: KeyCode) -> ConnectAction {
    if key == KeyCode::Esc {
        return ConnectAction::Cancel;
    }
    match &st.flow {
        ConnectFlow::ApiKey { provider_id, .. } => match key {
            KeyCode::Char(c) => {
                st.key_buffer.push(c);
                ConnectAction::None
            }
            KeyCode::Backspace => {
                st.key_buffer.pop();
                ConnectAction::None
            }
            KeyCode::Enter => {
                if st.key_buffer.is_empty() {
                    ConnectAction::None
                } else {
                    ConnectAction::SubmitKey { provider_id: provider_id.clone(), key: st.key_buffer.clone() }
                }
            }
            _ => ConnectAction::None,
        },
        ConnectFlow::Copilot => ConnectAction::None,
    }
}

/// Render the `/connect` body (plain text; the iocraft layer wraps it).
#[must_use]
pub fn render_connect_to_string(st: &ConnectScreenState) -> String {
    let mut out = String::new();
    match &st.flow {
        ConnectFlow::ApiKey { label, .. } => {
            out.push_str(&format!("Connect {label}\n"));
            let mask: String = "\u{2022}".repeat(st.key_buffer.chars().count());
            out.push_str(&format!("Key: {mask}\n"));
            out.push_str("Paste your API key \u{00B7} Enter to save \u{00B7} Esc to cancel");
        }
        ConnectFlow::Copilot => {
            out.push_str("Connect GitHub Copilot\n");
            match &st.copilot {
                CopilotPhase::Starting => out.push_str("Requesting device code\u{2026}\n"),
                CopilotPhase::Polling { user_code, verification_uri } => {
                    out.push_str(&format!("Enter code: {user_code}\n"));
                    out.push_str(&format!("at {verification_uri}\n"));
                    out.push_str("Waiting for authorization\u{2026}\n");
                }
                CopilotPhase::Done => out.push_str("Authorized \u{2713}\n"),
                CopilotPhase::Failed { error } => out.push_str(&format!("Authorization failed: {error}\n")),
            }
            out.push_str("Esc to cancel");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn api_key_field_masks_and_submits() {
        let mut st = ConnectScreenState::api_key("deepseek", "DeepSeek");
        for c in "sk-secret".chars() {
            assert_eq!(handle_connect_key(&mut st, KeyCode::Char(c)), ConnectAction::None);
        }
        let out = render_connect_to_string(&st);
        assert!(out.contains("Connect DeepSeek"));
        assert!(out.contains("Key: \u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"));
        assert!(!out.contains("sk-secret"), "raw key must never render");
        assert_eq!(
            handle_connect_key(&mut st, KeyCode::Enter),
            ConnectAction::SubmitKey { provider_id: "deepseek".to_string(), key: "sk-secret".to_string() }
        );
    }

    #[test]
    fn api_key_backspace_and_empty_enter_inert() {
        let mut st = ConnectScreenState::api_key("openrouter", "OpenRouter");
        let _ = handle_connect_key(&mut st, KeyCode::Char('a'));
        let _ = handle_connect_key(&mut st, KeyCode::Backspace);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Enter), ConnectAction::None);
    }

    #[test]
    fn esc_cancels() {
        let mut st = ConnectScreenState::api_key("deepseek", "DeepSeek");
        assert_eq!(handle_connect_key(&mut st, KeyCode::Esc), ConnectAction::Cancel);
    }

    #[test]
    fn copilot_renders_device_code_and_spinner() {
        let mut st = ConnectScreenState::copilot_pending();
        st.set_device_code("WDJB-MJHT", "https://github.com/login/device");
        let out = render_connect_to_string(&st);
        assert!(out.contains("Connect GitHub Copilot"));
        assert!(out.contains("Enter code: WDJB-MJHT"));
        assert!(out.contains("at https://github.com/login/device"));
        assert!(out.contains("Waiting for authorization"));
        assert_eq!(handle_connect_key(&mut st, KeyCode::Char('x')), ConnectAction::None);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Enter), ConnectAction::None);
        assert_eq!(handle_connect_key(&mut st, KeyCode::Esc), ConnectAction::Cancel);
    }

    #[test]
    fn copilot_failure_renders_error() {
        let mut st = ConnectScreenState::copilot_pending();
        st.set_failed("access_denied");
        let out = render_connect_to_string(&st);
        assert!(out.contains("Authorization failed: access_denied"));
    }
}
```

  Wire into `tui/src/screens/mod.rs`:
  1. `pub mod connect;` (near `pub mod model;`)
  2. add the variant to the `Screen` enum (after `Model(model::ModelScreenState),`):
```rust
    /// The `/connect <provider>` interactive credential screen (Plan 3c §6.3).
    /// Opened by the picker's `ModelOutcome::Connect` (via `pump_open_connect`) or
    /// a `/connect <provider>` prompt intercept.
    Connect(connect::ConnectScreenState),
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui connect::`  Expected: compile error — module/types undefined (pre-write).
- [ ] **Step 3: Implement** — the file above + the two `mod.rs` edits.
- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui connect::`  Expected: PASS (5 tests).
- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/screens/connect.rs lingxi-code/tui/src/screens/mod.rs && git commit -m "tui(connect): /connect screen — masked key field + Copilot device-flow render"`

---

### Task 25: Open `/connect` from the picker + a `/connect <provider>` prompt intercept + dispatch its keys

Wires the lifecycle: (a) the picker's `ModelOutcome::Connect` raises `pending_connect` + closes (root.rs:476 `Screen::Model` arm); (b) a `/connect <provider>` typed at the prompt raises `pending_connect` (app.rs intercept mirroring the `/model` intercept at app.rs:263 — coverage gap #3); (c) `pump_open_connect` consumes `pending_connect` and opens the `/connect` screen (API-key field for ordinary providers, Copilot device-flow for `github-copilot`); (d) the `Screen::Connect` key arm routes keys through `handle_connect_key` (Cancel → close; SubmitKey → raise `pending_store_key` + close). The `open_connect` opener + its test live here. The actual keychain write is the engine `EngineCredentialWriter` (Task 17) — `pump_store_provider_key` is a thin host pump that drains `pending_store_key` and calls it; for the tui-only increment it is documented (the engine seam already exists).

> Verified: redraw local is `needs_redraw`; pumps drive as `if pump_x(&state[, handle]).await { needs_redraw = true; }` (root.rs:1989); `Screen::Model` arm at root.rs:476; the `/model` prompt intercept at app.rs:263 clears `prompt_text`/`prompt_cursor` + raises the flag + `return false`.

**Files:**
- Modify: `lingxi-code/tui/src/state.rs` (`open_connect` + its test)
- Modify: `lingxi-code/tui/src/root.rs` (`Screen::Model` Connect branch; `Screen::Connect` key arm; `pump_open_connect`; `pump_store_provider_key`; drive both in the loop)
- Modify: `lingxi-code/tui/src/app.rs` (`/connect <provider>` prompt intercept)

- [ ] **Step 1: Write the failing test** — append to `tui/src/state.rs` `mod tests` (uses the Task-24 connect screen):
```rust
    #[test]
    fn open_connect_sets_connect_screen() {
        let mut s = AppState::new(fake_status());
        s.open_connect(crate::screens::connect::ConnectScreenState::api_key("deepseek", "DeepSeek"));
        assert!(matches!(s.active_screen, Some(crate::screens::Screen::Connect(_))));
    }
```

- [ ] **Step 2: Run it, expect failure** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui open_connect_sets_connect_screen`  Expected: compile error — no `open_connect`.

- [ ] **Step 3: Implement** —

  1. In `tui/src/state.rs`, add the opener (next to `open_model`):
```rust
    /// Open the `/connect` credential screen (API-key field or Copilot device-flow).
    /// Called by `root::pump_open_connect` after consuming `pending_connect`.
    pub fn open_connect(&mut self, state: crate::screens::connect::ConnectScreenState) {
        self.active_screen = Some(crate::screens::Screen::Connect(state));
        crate::telemetry::screen_opened("connect");
    }
```

  2. In `tui/src/root.rs` `Screen::Model` arm (root.rs:476), add the `Connect` branch:
```rust
                ModelOutcome::Connect { provider_id } => {
                    // Unconfigured provider: close the picker + raise the `/connect`
                    // flow (spec §6.4). `pump_open_connect` opens the screen.
                    st.pending_connect = Some(provider_id);
                    st.close_screen();
                }
```

  3. In `tui/src/root.rs`, add the `Screen::Connect` key arm to `handle_screen_key` (mirror the `Screen::Model` arm):
```rust
        Some(Screen::Connect(state)) => {
            // `/connect` screen. Esc → cancel + close; Enter on a non-empty key
            // → raise the host-side keychain store + close; Copilot typing inert.
            use crate::screens::connect::{handle_connect_key, ConnectAction};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_connect_key(state, ct_key.code) {
                ConnectAction::SubmitKey { provider_id, key } => {
                    st.pending_store_key = Some((provider_id, key));
                    st.close_screen();
                }
                ConnectAction::Cancel => st.close_screen(),
                ConnectAction::None => {}
            }
        }
```

  4. Add `pump_open_connect` (near `pump_open_model`):
```rust
/// Async `/connect` open pump. Consumes `AppState.pending_connect` (set by the
/// picker's `Connect` outcome or the `/connect <provider>` prompt intercept) and
/// opens the `/connect` screen: an API-key field for ordinary providers, or the
/// Copilot device-flow for `github-copilot`. Returns `true` iff a screen opened.
pub async fn pump_open_connect(state: &Arc<Mutex<AppState>>) -> bool {
    let provider = {
        let mut st = state.lock().await;
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            return false;
        }
        match st.pending_connect.take() {
            Some(p) => p,
            None => return false,
        }
    };
    let screen = if provider == "github-copilot" {
        crate::screens::connect::ConnectScreenState::copilot_pending()
    } else {
        // The picker carried the human label, but the flag only holds the id; the
        // header reads "Connect <id>" (the engine /connect group resolves the
        // canonical label on the registry path).
        crate::screens::connect::ConnectScreenState::api_key(&provider, &provider)
    };
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_connect = Some(provider);
        return false;
    }
    st.open_connect(screen);
    true
}

/// Async key-store pump: drains `AppState.pending_store_key` (set by the
/// `/connect` screen's `SubmitKey`) and persists it via the engine
/// `EngineCredentialWriter` seam (Task 17). The writer is wired through the
/// command registry's `/connect` handler; this pump performs the keychain write
/// out of the sync key path. Returns `true` iff a key was stored (triggers a
/// redraw + picker availability refresh on next open).
pub async fn pump_store_provider_key(state: &Arc<Mutex<AppState>>) -> bool {
    let pending = {
        let mut st = state.lock().await;
        st.pending_store_key.take()
    };
    let Some((provider_id, key)) = pending else {
        return false;
    };
    // The engine credential writer owns the keychain write. The tui dispatches
    // the masked key it collected; on a headless / no-writer build this is a
    // no-op log. The host wiring binds the writer when it constructs the App.
    tracing::info!(provider = %provider_id, "storing /connect provider key (len {})", key.len());
    // NOTE: the concrete `set_provider_key` call is performed by the engine
    // `EngineCredentialWriter` (apps/engine-desktop/src/connect.rs); the host
    // binds it to this pump when building the App. Until bound, the key is
    // dropped after logging (the user re-runs /connect). This keeps the tui
    // increment compilable + tested; the host binding is the engine /connect group.
    true
}
```

  5. Drive both pumps in the ticker loop next to `pump_open_model` (root.rs:1989):
```rust
                    if pump_open_connect(&state).await {
                        needs_redraw = true;
                    }
                    if pump_store_provider_key(&state).await {
                        needs_redraw = true;
                    }
```

  6. In `tui/src/app.rs`, add the `/connect <provider>` prompt intercept right after the `/model` intercept (app.rs:263-268):
```rust
            // `/connect <provider>` opens the interactive credential screen. Like
            // `/model`, opening needs an async step (the device-flow / keychain),
            // so we RAISE `pending_connect`; `root::pump_open_connect` opens the
            // screen on the next tick. A bare `/connect` (no arg) falls through to
            // the engine `ConnectHandler`, which renders the usage line.
            {
                let trimmed = st.prompt_text.trim();
                if let Some(rest) = trimmed.strip_prefix("/connect ") {
                    let provider = rest.trim();
                    if !provider.is_empty() {
                        st.prompt_text.clear();
                        st.prompt_cursor = 0;
                        st.pending_connect = Some(provider.to_string());
                        return false;
                    }
                }
            }
```

- [ ] **Step 4: Run it, expect pass** — `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui --lib`  Expected: PASS (whole tui lib compiles + green).

- [ ] **Step 5: Commit** — `git add lingxi-code/tui/src/state.rs lingxi-code/tui/src/root.rs lingxi-code/tui/src/app.rs && git commit -m "tui(connect): open /connect from picker + prompt intercept + dispatch keys + key-store pump"`

**Final tui gate (after Task 25):** `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p tui` and `CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p tui -- -D warnings`.

**Whole-workspace gate (after Task 25):** `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test` (all crates) + `CARGO_PROFILE_DEV_DEBUG=0 cargo build -p engine-mobile --features uniffi`.

---

## Self-review notes

### Spec section → task coverage map

| Spec section | Tasks |
|---|---|
| §4.1 provider-config crate (scaffold, types, parse, assemble, composite) | 2, 3, 4, 5, 6, 8, 9, 13 |
| §4.2 secret `set/get_provider_key` | 1 |
| §4.3 orchestrator chain-walking | 10, 11 |
| §4.4 engine `build()` (desktop + mobile) | 14, 15 |
| §4.5 `/connect` command + picker split | 16, 17, 18, 24, 25 |
| §5.1 `parse_user_providers` (openai/anthropic/gemini; skip+warn; model-less drop) | 4, 6 |
| §5.2 `parse_routing` (aliases/fallback/retry, 3-tuple) | 5, 6 |
| §5.3 `assemble` (merge, fold aliases, validate chains, credential_sources, pricing) | 6, 13 |
| §5.4 uniform `Static{id}` credential model | 6 |
| §6.1 `MultiCredentialProvider` (composite, OAuth delegate) | 8, 14, 15 |
| §6.2 availability (keychain|env|anthropic) | 9, 14 |
| §6.3 `/connect` flow (API-key + Copilot device-flow) | 16, 17, 18, 24 |
| §6.4 picker Connect badge + select-launches-`/connect` | 19, 21, 22, 25 |
| §7 fallback-chain execution (advance/stop, supersede built-in, per-entry budget, served-model surfacing incl. nit (e)) | 10, 11 |
| §8 engine wiring (assemble→client→composite→cost→chains→availability; availability sibling map) | 12, 13, 14, 15, 19, 20, 23 |
| §8 cost pricing for non-Anthropic | 12, 13, 14, 15 |
| §9 error handling (warnings→tracing; auth/401; chain exhaustion; /connect failures) | 6, 8, 10, 16, 18 |
| §10 testing + frozen-guard (one documented CopilotSecret exception) | 7 + every task's TDD steps |
| §10 frozen-crate exception (`CopilotSecret::token_for_storage`) | 7 |
| §11 non-goals (no wildcard; mobile `/connect` UI deferred; listing-only deferred) | 6 (drop+warn), 15 |
| §12 implementation order | 1 → 9 (provider-config) → 10–11 (orchestrator) → 12–13 (cost) → 14–15 (engine) → 16–18 (/connect) → 19–25 (tui) |

Every spec §4–§10 component maps to at least one task. The two resolved decisions are real tasks: COST = the "Cost pricing for all providers" group (Tasks 12–13) wired into the existing `CostTracker` path in Tasks 14–15 (no `CostEstimator`, no `merge_llm_pricing_into_cost`); COPILOT = Task 7 (`CopilotSecret::token_for_storage`, the one documented §10 exception) consumed by Task 18.

### Critic P0/P1 fixes applied

- **Adapter chains API (P0 #3):** `ProviderApiAdapter::new` takes `chains: ChainConfig` as the 7th positional arg + a `pub fn chain_for(&self, key) -> Vec<ChainEntry>` accessor (Task 10). No `.with_chains(..)` anywhere; engine/mobile/test call sites pass it positionally (Tasks 10, 14, 15). Verified against provider_adapter.rs:60 (6-arg today).
- **`AssembleInputs` canonical field names (P0 #4):** `anthropic_api_base`, `anthropic_models`, `anthropic_has_api_key`, `anthropic_has_oauth`, `user_providers`, `routing` (Task 3); engine call sites rewritten (Tasks 14, 15); no `default_model`/`fallback_model` fields. Confirmed `DesktopConfig` real fields at lib.rs:500 and `llm_models = anthropic_models_for(...)` at lib.rs:986.
- **`CredentialSource.profile_name` (P1 #6):** added (Task 3), set in `assemble` (Task 6: anthropic→"anthropic", presets/users→profile_name); availability fn + engine map key by `profile_name` (Tasks 9, 14).
- **Async availability call (P1, ordering #1):** `compute_availability` uses `.await` + `matches!(.., Ok(Some(_)))` (Task 9); the engine uses `compute_availability(...).await` (Task 14) — NOT a sync `.is_some()`.
- **Test doubles impl both `request` + `stream_sse` (P0 #5):** every `NoHttp` in Tasks 1, 8, 9 implements both required `traits::HttpTransport` methods (the unused panics). Verified the trait has 2 required methods (http.rs:42-47).
- **Dedupe provider-config Cargo.toml (P1 #9 / ordering #3):** Task 2 adds the FULL dep set (llm-client, secret, protocol, cost, serde, serde_json, tracing + dev tokio/async-trait/traits); Tasks 3–9, 13 add modules only.
- **Availability map → tui App (coverage gap #4 / discrepancy):** Task 23 threads `DesktopRuntime.provider_availability` → cli `Runtime` → `tui::session::Runtime` → `AppState.provider_availability`; Task 20 reads it instead of `BTreeMap::new()`. Verified the hop: session.rs:42/207, init.rs:284.
- **`parse_routing` 3-tuple (P1 #5):** `(ChainConfig, raw_fallback: BTreeMap, warnings)` (Task 5); `assemble` validates raw fallback into `chains` (Task 6); consistent across consumers.
- **`CredentialProvider::load` → `Result<Credential, LlmError>` (P0 #4-confirmed):** missing → `Err(LlmError::Authentication)` everywhere; prose + code aligned (Task 8).
- **Model-less user providers drop+warn (P1 #7):** Task 6; listing-only noted as deferred (reconciliation #7, spec §11).
- **CostEstimator language deleted (P0 #1):** no `CostEstimator`/`merge_llm_pricing_into_cost`; the cost group (Tasks 12–13) extends `cost::PricingCatalog` with `with_entry` and returns `Assembled.pricing: cost::PricingCatalog`, wired into `CostTracker::new(SessionId, Arc<PricingCatalog>, tx)` (Task 14). Verified cost API at pricing.rs (private fields, `MoneyPerToken{nano_usd_per_token}`, no `PricingSource::External`).
- **Copilot token reader (P0 #2):** Task 7 = `#[doc(hidden)] pub fn token_for_storage` on `CopilotSecret`, labelled the one §10 exception; Task 18's driver calls it (replacing the placeholder `expose_for_storage()`). Verified `CopilotSecret(String)` has no reader (auth.rs:17-26).
- **`desktop_command_registry` 4→6 args (P1 #8):** Task 18 updates the signature + all call sites (build body + the registry test) atomically.
- **`/connect` prompt-text intercept (coverage gap #3):** Task 25 adds the app.rs `/connect <provider>` intercept mirroring `/model` (app.rs:263).

### P2 items verified against real code (file:line)

- **protocol HTTP field names (P2):** `protocol::HttpRequest { method: HttpMethod, url, headers: Vec<(String,String)>, body: Option<String>, timeout: Option<Duration> }` and `HttpResponse.body: String` — `protocol/src/transport.rs:28,44,50`. Task 18's `PosixCopilotHttp` uses `HttpMethod::Post` + Vec headers + `body: Some(String)` + `timeout: None` (the draft's BTreeMap/`Vec<u8>` shape was wrong and is corrected).
- **`ProviderRequest` body shape (P2):** `llm_client::ProviderRequest.body_json: Value` (NOT `body`) — `llm-client/src/protocol.rs:336-346`. Task 10's `transport_models` reads `r.body_json.get("model")`. `ProviderResponse.body_json: Value`, `ProviderResponse::json(status, value)` — protocol.rs:362-386.
- **engine-mobile feature + MobileConfig (P2):** the host feature is **`uniffi`** (gates `dep:orchestrator`/`dep:secret`/etc.) — `apps/engine-mobile/Cargo.toml:21-41`. `MobileConfig` has `provider_profiles` + `routing` + `api_base` + `default_model` + `api_key` and impls `Default`; **no `fallback_model`** — `apps/engine-mobile/src/host.rs:110-145`. The mobile test harness uses `build_mobile(test_config(..), platform, listener, perm_sink)` — host.rs:1407. Tasks 10, 15 use `--features uniffi` and `anthropic_models(&cfg.default_model)`.
- **no-op `PermissionRequestSink` (P2):** `orchestrator::test_support::RecordingPermissionSink` exists and impls `client_adapter::PermissionRequestSink` (required method `async fn emit_request(&self, PermissionRequestDto)`) — used by the mobile tests at host.rs:1413. Tasks 14 reuse it; no new engine-desktop `test_support` module is added (reconciliation #9).
- **tui picker test fixture (P2):** `ModelScreenState::new(rows, recent, current)` (model.rs:141); existing `render_tests::st()` (model.rs:380-391); `ModelRow { display_model, request_model, provider_id, provider_label }` (model.rs:19); `ModelOutcome::{Stay, Commit, Cancel}` (model.rs:124); `traits::orchestrator::ModelListing { display_model, request_model, provider_id, provider_label }`. Tasks 19–22 use these exactly.
- **tui `AppState` fixture (P2):** `AppState::new(status: StatusSnapshot)` (state.rs:817); `StatusSnapshot: Default` (state.rs:447); existing tests build via `AppState::new(fake_status())` where `fake_status()` uses `..StatusSnapshot::default()` (state.rs:1183). Tasks 23/25 use `AppState::new(fake_status())`.
- **tui redraw var + pump driver (P2):** the loop local is `needs_redraw`; pumps drive as `if pump_x(&state[, handle]).await { needs_redraw = true; }` (root.rs:1989). The `/model` intercept clears `prompt_text`/`prompt_cursor` + raises the flag + `return false` (app.rs:263). Tasks 20, 25 mirror these.
- **cost wiring path (decision a):** `CostTracker::new(SessionId, Arc<PricingCatalog>, mpsc::Sender)` (cost/src/tracker.rs:86); the engine wraps `cost::PricingCatalog::builtin_reference()` today (lib.rs:1075). Task 14 swaps in `Arc::new(assembled.pricing)`. The `(profile, model) → cost::ProviderId` mapping mirrors `orchestrator::cost_wiring::provider_id_for_profile` (cost_wiring.rs:53).

### Residual assumptions the executor must confirm

1. **Exact `DesktopConfig { … }` field list** at lib.rs:500 — Task 14's integration-test fixture + Task 17's `connect_prompt: None` must match every real field (the plan shows an illustrative set incl. `session_started_as_coordinator`/`memory_provider`/`permission_mode`). Copy the real list.
2. **The engine `DesktopRuntime` local name in `apps/cli/src/init.rs`** (the `build(...)` result) — Task 23 reads `<that>.provider_availability`. Also set `provider_availability: BTreeMap::new()` on every OTHER `tui::session::Runtime { … }` construction in `apps/cli`.
3. **The `AuthHandle` test double** the existing `desktop_command_registry`/`build` tests construct — Task 18's `desktop_registry_exposes_connect` must reuse it (the plan's `make_test_auth_handle()` is a placeholder name).
4. **The exact in-`build()` `desktop_command_registry(...)` call site + any bridge/test helper call site** — Task 18 updates all of them to the 6-arg form in one commit.
5. **The 4 inline `ProviderApiAdapter::new(...)` test constructors + the `adapter()` helper** in provider_adapter.rs — Task 10 adds the 7th `ChainConfig::default()` arg to each; confirm the full set (the plan names the three with `.with_fallback_model`).
6. **`pump_store_provider_key` host binding** — Task 25 leaves the concrete `EngineCredentialWriter.set_provider_key` call as a host-bound follow-up (the seam exists from Task 17). If the executor wants the key persisted end-to-end in this increment, bind the writer into the pump where the App is constructed (apps/cli) — otherwise the masked key is logged + dropped and the user re-runs `/connect`. This is the one deliberately-deferred edge in the tui-only scope (spec §6.3 "no restart" still holds for env keys + the registry `/connect` handler path).
7. **`anthropic_models(&cfg.default_model)`** (mobile) vs `anthropic_models_for(&cfg.default_model, fallback)` (desktop) — confirm the mobile helper name (host.rs uses `anthropic_models`); mobile has no fallback model so the single-arg form is correct.
