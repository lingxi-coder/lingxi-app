# LLM Providers — P2 (Selection / Wiring) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the orchestrator's model calls to a provider chosen by a `provider/model` string — via a `ModelRouter`/`ProviderRegistry` with built-in `anthropic`/`openai`/`gemini` profiles plus settings-declared profiles — while keeping bare/`claude-*` models byte-identical to today's Anthropic path.

**Architecture:** A new `ModelSpec` parser and `ProviderRegistry<T>` (implementing an object-safe `ModelRouter`) live in the `providers` crate. The orchestrator's `ProviderApiAdapter` (built in P1) evolves to hold `Arc<dyn ModelRouter>` and resolve the provider per call. A `providers` object field is added to the settings schema. `apps/cli/src/init.rs` loads settings, builds the registry, and swaps the live `api_client` from `AnthropicProviderAdapter` to the routed `ProviderApiAdapter`. In P2 only the `anthropic` provider is constructible (P1's `AnthropicLlmProvider`); `openai`/`gemini` resolve to a clear "codec not available until P3/P4" error — no fake codecs.

**Tech Stack:** Rust 1.82.0 (pinned). `async-trait`, `serde_json`, `thiserror`. Builds on P1's `providers` crate (`LlmProvider`, `AnthropicLlmProvider`, `CanonicalRequest`, `Capabilities`, `Auth`) and `orchestrator::ProviderApiAdapter`. Reuses `api_client::ApiError`, `platform_api::{HttpTransport, HttpError}`, `lingxi_core::settings`.

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§5 selection, §6 wiring, §12 P2).

**Conventions (read first):**
- Run all `cargo`/`scripts` commands from `lingxi-code/`. Rust 1.82.0.
- Workspace lints: `missing_docs = "warn"` + `clippy::pedantic = "warn"`; gate is `cargo clippy --all-targets -- -D warnings`. Every `pub` item needs a `///` doc; inherent constructors returning a value need `#[must_use]`; code must be pedantic-clean. Paste the code blocks verbatim.
- Do **NOT** modify the `traits/` crate (frozen). Do **NOT** add variants to `api_client::ApiError` (avoid cross-crate churn) — reuse existing variants as directed.
- Known pre-existing issue (NOT in scope): `cargo clippy -p orchestrator`/`-p cli` surface 2 `tool-api` clippy errors (`tool-api/src/util/{ids.rs:15, path_validation.rs:70}`); they predate this feature and are separately tracked. Assess only the files you change. `cargo clippy -p providers --all-targets -- -D warnings` is clean (no tool-api in its graph).

**P1 recap (already on branch `llm-providers`, tag `llm-p1`):**
- `providers` crate exports: `LlmProvider`, `WireCodec`, `SseDecoder`, `GenericClient`, `Capabilities`/`ReasoningSupport`/`SystemStyle`, `Auth`, `CodecError`, `CanonicalRequest`/`DEFAULT_MAX_TOKENS`, `AnthropicLlmProvider`.
- `orchestrator::ProviderApiAdapter` wraps `Arc<dyn LlmProvider>` and implements `OrchestratorApiClient` + `StreamingApiClient`. **P2 changes it to wrap `Arc<dyn ModelRouter>`.**
- `AnthropicLlmProvider<T: HttpTransport + Send + Sync + 'static>::new(api_key, base_url, Arc<T>)`.

---

## File Structure

**`providers` crate (new modules):**
- `src/model_spec.rs` — `ModelSpec` (`provider/model` parsing + back-compat).
- `src/profile.rs` — `ProviderKind`, `ProviderProfile`, `parse_profiles`.
- `src/registry.rs` — `ModelRouter` trait, `Resolved`, `ProviderRegistry<T>`.
- `src/lib.rs` — re-exports.

**`orchestrator` crate (modify):**
- `src/provider_adapter.rs` — evolve `ProviderApiAdapter` to hold `Arc<dyn ModelRouter>`; update tests.

**`core` crate (modify, 3 files):**
- `src/settings/schema.rs` — add `providers` field.
- `src/settings/merger.rs` — deep-merge `providers`.
- `src/settings/tracer.rs` — add `providers` provenance entry.

**`apps/cli` crate (modify):**
- `src/init.rs` — load settings, build `ProviderRegistry`, swap `api_client`.

---

## Task 1: `ModelSpec` parser

**Files:**
- Create: `lingxi-code/providers/src/model_spec.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Write `model_spec.rs` with tests**

Create `lingxi-code/providers/src/model_spec.rs`:

```rust
//! Parse a model string into a `(profile, model)` pair.
//!
//! `"openai/gpt-4o"` → profile `openai`, model `gpt-4o`. A string with no `/`
//! (or any `claude-*` string, even if it contains a `/`) resolves to the
//! built-in `anthropic` profile, so every model string used before P2
//! routes unchanged.

/// The default profile name used for bare / `claude-*` model strings.
pub const DEFAULT_PROFILE: &str = "anthropic";

/// A parsed model selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Provider profile name to resolve (e.g. `anthropic`, `openai`, `groq`).
    pub profile: String,
    /// Provider-local model id, with any `profile/` prefix stripped.
    pub model: String,
}

impl ModelSpec {
    /// Parse a model string. Back-compat: no `/`, or a `claude-*` model,
    /// maps to the [`DEFAULT_PROFILE`] (`anthropic`) with the full string as
    /// the model id.
    #[must_use]
    pub fn parse(input: &str) -> Self {
        if input.starts_with("claude-") {
            return Self {
                profile: DEFAULT_PROFILE.to_string(),
                model: input.to_string(),
            };
        }
        match input.split_once('/') {
            Some((profile, model)) if !profile.is_empty() && !model.is_empty() => Self {
                profile: profile.to_string(),
                model: model.to_string(),
            },
            _ => Self {
                profile: DEFAULT_PROFILE.to_string(),
                model: input.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_splits_profile_and_model() {
        let s = ModelSpec::parse("openai/gpt-4o");
        assert_eq!(s.profile, "openai");
        assert_eq!(s.model, "gpt-4o");
    }

    #[test]
    fn bare_string_is_anthropic_backcompat() {
        let s = ModelSpec::parse("claude-opus-4-7");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "claude-opus-4-7");
    }

    #[test]
    fn claude_with_slash_stays_anthropic() {
        // A claude model id is never reinterpreted as profile/model.
        let s = ModelSpec::parse("claude-3-5/sonnet");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "claude-3-5/sonnet");
    }

    #[test]
    fn non_claude_no_slash_is_anthropic_profile() {
        let s = ModelSpec::parse("some-model");
        assert_eq!(s.profile, "anthropic");
        assert_eq!(s.model, "some-model");
    }

    #[test]
    fn custom_profile_name() {
        let s = ModelSpec::parse("groq/llama-3.3-70b");
        assert_eq!(s.profile, "groq");
        assert_eq!(s.model, "llama-3.3-70b");
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`: add `pub mod model_spec;` (keep modules alphabetical — after `pub mod error;`, before `pub mod provider;`) and add `pub use model_spec::{ModelSpec, DEFAULT_PROFILE};` (after `pub use error::CodecError;`).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers model_spec` → 5 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/model_spec.rs providers/src/lib.rs
git commit -m "feat(llm-p2): ModelSpec parser (provider/model + claude back-compat)"
```

---

## Task 2: `ProviderProfile` + `parse_profiles`

**Files:**
- Create: `lingxi-code/providers/src/profile.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Write `profile.rs` with tests**

Create `lingxi-code/providers/src/profile.rs`:

```rust
//! Provider profile config — the typed form of a settings `providers` entry,
//! plus the built-in defaults.

use crate::error::CodecError;
use std::collections::BTreeMap;

/// Which wire format / codec a profile uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Anthropic Messages API (the only kind constructible in P2).
    Anthropic,
    /// OpenAI / OpenAI-compatible chat completions (codec lands in P3).
    OpenAi,
    /// Google Gemini generateContent (codec lands in P4).
    Gemini,
}

impl ProviderKind {
    /// Parse the settings `type` string.
    ///
    /// # Errors
    /// Returns [`CodecError::Unsupported`] for an unknown type.
    pub fn parse(s: &str) -> Result<Self, CodecError> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "gemini" => Ok(Self::Gemini),
            other => Err(CodecError::Unsupported(format!(
                "unknown provider type {other:?} (expected anthropic|openai|gemini)"
            ))),
        }
    }
}

/// One provider profile: a wire format plus its endpoint + key source.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    /// Wire format / codec.
    pub kind: ProviderKind,
    /// Optional base URL override (for OpenAI-compatible / custom endpoints).
    pub base_url: Option<String>,
    /// Name of the env var holding the API key (e.g. `OPENAI_API_KEY`).
    /// `None` means no auth (e.g. a local Ollama endpoint).
    pub api_key_env: Option<String>,
}

/// Built-in profiles, keyed by name. `anthropic` uses `anthropic_base` (the
/// CLI's resolved Anthropic base URL); `openai`/`gemini` use their first-party
/// defaults (their codecs are not available until P3/P4).
#[must_use]
pub fn builtin_profiles(anthropic_base: Option<String>) -> BTreeMap<String, ProviderProfile> {
    let mut m = BTreeMap::new();
    m.insert(
        "anthropic".to_string(),
        ProviderProfile {
            kind: ProviderKind::Anthropic,
            base_url: anthropic_base,
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
        },
    );
    m.insert(
        "openai".to_string(),
        ProviderProfile {
            kind: ProviderKind::OpenAi,
            base_url: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        },
    );
    m.insert(
        "gemini".to_string(),
        ProviderProfile {
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_env: Some("GEMINI_API_KEY".to_string()),
        },
    );
    m
}

/// Parse the settings `providers` object into typed profiles. Each entry is
/// `{ "type": "openai", "baseUrl"?: "...", "apiKeyEnv"?: "..."|null }`.
///
/// # Errors
/// Returns [`CodecError::Unsupported`] if an entry is not an object, lacks a
/// string `type`, or has an unknown `type`.
pub fn parse_profiles(
    raw: Option<&BTreeMap<String, serde_json::Value>>,
) -> Result<BTreeMap<String, ProviderProfile>, CodecError> {
    let mut out = BTreeMap::new();
    let Some(raw) = raw else {
        return Ok(out);
    };
    for (name, value) in raw {
        let obj = value.as_object().ok_or_else(|| {
            CodecError::Unsupported(format!("provider profile {name:?} must be an object"))
        })?;
        let type_str = obj.get("type").and_then(serde_json::Value::as_str).ok_or_else(|| {
            CodecError::Unsupported(format!("provider profile {name:?} is missing a string \"type\""))
        })?;
        let kind = ProviderKind::parse(type_str)?;
        let base_url = obj
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // `apiKeyEnv` may be a string or explicit null (no auth).
        let api_key_env = match obj.get("apiKeyEnv") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        out.insert(
            name.clone(),
            ProviderProfile {
                kind,
                base_url,
                api_key_env,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtins_present() {
        let b = builtin_profiles(Some("https://api.anthropic.com".to_string()));
        assert_eq!(b["anthropic"].kind, ProviderKind::Anthropic);
        assert_eq!(b["openai"].kind, ProviderKind::OpenAi);
        assert_eq!(b["gemini"].kind, ProviderKind::Gemini);
        assert_eq!(b["anthropic"].base_url.as_deref(), Some("https://api.anthropic.com"));
    }

    #[test]
    fn parse_none_is_empty() {
        assert!(parse_profiles(None).unwrap().is_empty());
    }

    #[test]
    fn parse_openai_compatible_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "groq".to_string(),
            json!({"type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["groq"].kind, ProviderKind::OpenAi);
        assert_eq!(p["groq"].base_url.as_deref(), Some("https://api.groq.com/openai/v1"));
        assert_eq!(p["groq"].api_key_env.as_deref(), Some("GROQ_API_KEY"));
    }

    #[test]
    fn parse_null_api_key_env_means_no_auth() {
        let mut raw = BTreeMap::new();
        raw.insert("ollama".to_string(), json!({"type": "openai", "baseUrl": "http://localhost:11434/v1", "apiKeyEnv": null}));
        let p = parse_profiles(Some(&raw)).unwrap();
        assert!(p["ollama"].api_key_env.is_none());
    }

    #[test]
    fn parse_unknown_type_errors() {
        let mut raw = BTreeMap::new();
        raw.insert("x".to_string(), json!({"type": "mistral"}));
        assert!(parse_profiles(Some(&raw)).is_err());
    }

    #[test]
    fn parse_missing_type_errors() {
        let mut raw = BTreeMap::new();
        raw.insert("x".to_string(), json!({"baseUrl": "http://x"}));
        assert!(parse_profiles(Some(&raw)).is_err());
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`: add `pub mod profile;` (after `pub mod model_spec;`) and `pub use profile::{builtin_profiles, parse_profiles, ProviderKind, ProviderProfile};` (after the `model_spec` re-export).

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p providers profile` → 6 tests pass.
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/profile.rs providers/src/lib.rs
git commit -m "feat(llm-p2): ProviderProfile/ProviderKind + settings-profile parser + built-ins"
```

---

## Task 3: `ModelRouter` + `ProviderRegistry`

**Files:**
- Create: `lingxi-code/providers/src/registry.rs`
- Modify: `lingxi-code/providers/src/lib.rs`

- [ ] **Step 1: Write the test first (TDD)**

Create `lingxi-code/providers/src/registry.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockTransport;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn registry(extra: BTreeMap<String, ProviderProfile>) -> ProviderRegistry<MockTransport> {
        let mut profiles = builtin_profiles(Some("https://mock.local".to_string()));
        profiles.extend(extra);
        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string());
        ProviderRegistry::new(profiles, env, Arc::new(MockTransport::responding(200, "")))
    }

    #[test]
    fn resolves_anthropic_and_strips_nothing_for_bare_model() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("claude-opus-4-7").expect("anthropic resolves");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider.id(), cost::ProviderId::Anthropic);
    }

    #[test]
    fn resolves_anthropic_for_explicit_prefix_and_strips_it() {
        let r = registry(BTreeMap::new());
        let resolved = r.resolve("anthropic/claude-sonnet-4-6").expect("resolves");
        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider.id(), cost::ProviderId::Anthropic);
    }

    #[test]
    fn openai_profile_errors_codec_unavailable_in_p2() {
        let r = registry(BTreeMap::new());
        let err = r.resolve("openai/gpt-4o").expect_err("no openai codec in P2");
        match err {
            api_client::ApiError::Http(platform_api::HttpError::InvalidRequest(msg)) => {
                assert!(msg.contains("openai"), "msg: {msg}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn unknown_profile_errors() {
        let r = registry(BTreeMap::new());
        assert!(r.resolve("nope/x").is_err());
    }

    #[test]
    fn anthropic_provider_is_cached() {
        let r = registry(BTreeMap::new());
        let a = r.resolve("claude-opus-4-7").unwrap();
        let b = r.resolve("claude-opus-4-7").unwrap();
        assert!(Arc::ptr_eq(&a.provider, &b.provider), "same Arc reused from cache");
    }

    #[test]
    fn available_profiles_lists_builtins() {
        let r = registry(BTreeMap::new());
        let mut got = r.available_profiles();
        got.sort();
        assert_eq!(got, vec!["anthropic".to_string(), "gemini".to_string(), "openai".to_string()]);
    }
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p providers --lib registry 2>&1 | head -20`
Expected: FAIL — `cannot find ... ProviderRegistry` / `ModelRouter` / `Resolved`.

- [ ] **Step 3: Write the implementation above the test module**

Prepend to `lingxi-code/providers/src/registry.rs`:

```rust
//! `ProviderRegistry` resolves a model string to a provider via `ModelSpec`,
//! caching one `LlmProvider` per profile. P2 constructs only the `anthropic`
//! provider; `openai`/`gemini` profiles resolve to a clear "codec not
//! available until P3/P4" error rather than a fake stub.

use crate::anthropic::AnthropicLlmProvider;
use crate::model_spec::ModelSpec;
use crate::profile::{ProviderKind, ProviderProfile};
use crate::provider::LlmProvider;
use api_client::ApiError;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use platform_api::{HttpError, HttpTransport};

/// A resolved provider plus the provider-local model id (prefix stripped).
pub struct Resolved {
    /// The provider to drive the request.
    pub provider: Arc<dyn LlmProvider>,
    /// Provider-local model id (e.g. `gpt-4o`, `claude-opus-4-7`).
    pub model: String,
}

/// Routes a model string to a provider. Object-safe so the orchestrator's
/// `ProviderApiAdapter` can hold an `Arc<dyn ModelRouter>`.
pub trait ModelRouter: Send + Sync {
    /// Resolve `model` to a provider + local model id.
    ///
    /// # Errors
    /// Returns an [`ApiError`] if the profile is unknown or its codec is not
    /// available yet.
    fn resolve(&self, model: &str) -> Result<Resolved, ApiError>;

    /// Profile names available for selection (for `/model` listing).
    fn available_profiles(&self) -> Vec<String>;
}

/// Builds and caches providers from a set of profiles, over a single shared
/// transport `T`.
pub struct ProviderRegistry<T: HttpTransport + Send + Sync + 'static> {
    profiles: BTreeMap<String, ProviderProfile>,
    env: BTreeMap<String, String>,
    transport: Arc<T>,
    cache: Mutex<BTreeMap<String, Arc<dyn LlmProvider>>>,
}

impl<T: HttpTransport + Send + Sync + 'static> ProviderRegistry<T> {
    /// Construct from a profile set (built-ins + settings), an env snapshot
    /// (for API-key lookup), and the shared transport.
    #[must_use]
    pub fn new(
        profiles: BTreeMap<String, ProviderProfile>,
        env: BTreeMap<String, String>,
        transport: Arc<T>,
    ) -> Self {
        Self {
            profiles,
            env,
            transport,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    fn api_key_for(&self, profile: &ProviderProfile) -> String {
        profile
            .api_key_env
            .as_ref()
            .and_then(|var| self.env.get(var))
            .cloned()
            .unwrap_or_default()
    }

    fn build(&self, name: &str, profile: &ProviderProfile) -> Result<Arc<dyn LlmProvider>, ApiError> {
        match profile.kind {
            ProviderKind::Anthropic => {
                let key = self.api_key_for(profile);
                let provider = AnthropicLlmProvider::new(
                    key,
                    profile.base_url.clone(),
                    self.transport.clone(),
                );
                Ok(Arc::new(provider) as Arc<dyn LlmProvider>)
            }
            ProviderKind::OpenAi => Err(ApiError::Http(HttpError::InvalidRequest(format!(
                "provider profile {name:?} uses the openai codec, which is not available until P3"
            )))),
            ProviderKind::Gemini => Err(ApiError::Http(HttpError::InvalidRequest(format!(
                "provider profile {name:?} uses the gemini codec, which is not available until P4"
            )))),
        }
    }
}

impl<T: HttpTransport + Send + Sync + 'static> ModelRouter for ProviderRegistry<T> {
    fn resolve(&self, model: &str) -> Result<Resolved, ApiError> {
        let spec = ModelSpec::parse(model);
        let profile = self.profiles.get(&spec.profile).ok_or_else(|| {
            ApiError::Http(HttpError::InvalidRequest(format!(
                "unknown provider profile {:?}; configure it under settings `providers`",
                spec.profile
            )))
        })?;

        // Cache one provider per profile.
        let mut cache = self.cache.lock().expect("registry cache mutex poisoned");
        if let Some(existing) = cache.get(&spec.profile) {
            return Ok(Resolved {
                provider: existing.clone(),
                model: spec.model,
            });
        }
        let provider = self.build(&spec.profile, profile)?;
        cache.insert(spec.profile.clone(), provider.clone());
        Ok(Resolved {
            provider,
            model: spec.model,
        })
    }

    fn available_profiles(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

In `lingxi-code/providers/src/lib.rs`: add `pub mod registry;` (after `pub mod provider;`) and `pub use registry::{ModelRouter, ProviderRegistry, Resolved};` (after the `provider` re-export).

- [ ] **Step 5: Test + lint + commit**

Run: `cargo test -p providers` → all pass (11 from P1 + 5 model_spec + 6 profile + 6 registry).
Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
```bash
git add providers/src/registry.rs providers/src/lib.rs
git commit -m "feat(llm-p2): ModelRouter + ProviderRegistry (anthropic live, openai/gemini -> P3/P4 error)"
```

---

## Task 4: Evolve `ProviderApiAdapter` to route via `ModelRouter`

**Files:**
- Modify (replace whole file): `lingxi-code/orchestrator/src/provider_adapter.rs`

In P1 the adapter held a single `Arc<dyn LlmProvider>`. P2 changes it to hold `Arc<dyn ModelRouter>` and resolve per call (per spec §6). The tests change to a `StubRouter`.

- [ ] **Step 1: Replace `provider_adapter.rs` entirely**

Overwrite `lingxi-code/orchestrator/src/provider_adapter.rs` with:

```rust
//! Bridge: adapt a `providers::ModelRouter` to the orchestrator's
//! `OrchestratorApiClient` / `StreamingApiClient` traits. Each call parses the
//! model string, resolves the provider via the router, and delegates with the
//! provider-local model id.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use api_client::types::{MessageResponse, StreamEvent};
use api_client::ApiError;
use async_trait::async_trait;
use futures::stream::BoxStream;
use protocol::ConversationMessage;
use providers::{CanonicalRequest, ModelRouter};
use std::sync::Arc;

/// Adapts an `Arc<dyn ModelRouter>` to the orchestrator's API-client traits.
pub struct ProviderApiAdapter {
    router: Arc<dyn ModelRouter>,
}

impl ProviderApiAdapter {
    /// Wrap a router.
    #[must_use]
    pub fn new(router: Arc<dyn ModelRouter>) -> Self {
        Self { router }
    }
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        let resolved = self.router.resolve(model)?;
        let mut req = CanonicalRequest::new(resolved.model);
        req.system = system.map(str::to_string);
        req.messages = msgs;
        resolved.provider.complete(req).await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
        let resolved = self.router.resolve(model)?;
        let mut req = CanonicalRequest::new(resolved.model);
        req.system = system.map(str::to_string);
        req.messages = messages;
        req.tools = tools;
        req.stream = true;
        resolved.provider.stream(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::{ContentBlockApi, UsageApi};
    use futures::StreamExt;
    use providers::{Capabilities, LlmProvider, Resolved};
    use std::sync::Mutex;

    /// Records the canonical request it received and returns a canned response.
    struct StubProvider {
        seen_model: Mutex<Option<String>>,
        seen_system: Mutex<Option<String>>,
        seen_tools_len: Mutex<Option<usize>>,
        seen_stream_flag: Mutex<Option<bool>>,
        caps: Capabilities,
    }

    impl StubProvider {
        fn new() -> Self {
            Self {
                seen_model: Mutex::new(None),
                seen_system: Mutex::new(None),
                seen_tools_len: Mutex::new(None),
                seen_stream_flag: Mutex::new(None),
                caps: Capabilities::anthropic(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn id(&self) -> cost::ProviderId {
            cost::ProviderId::OpenAI
        }
        fn capabilities(&self) -> &Capabilities {
            &self.caps
        }
        async fn complete(&self, req: CanonicalRequest) -> Result<MessageResponse, ApiError> {
            *self.seen_model.lock().unwrap() = Some(req.model.clone());
            *self.seen_system.lock().unwrap() = req.system.clone();
            Ok(MessageResponse {
                id: "stub".to_string(),
                model: req.model,
                content: vec![ContentBlockApi::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: UsageApi::default(),
            })
        }
        async fn stream(
            &self,
            req: CanonicalRequest,
        ) -> Result<BoxStream<'static, Result<StreamEvent, ApiError>>, ApiError> {
            *self.seen_tools_len.lock().unwrap() = Some(req.tools.len());
            *self.seen_stream_flag.lock().unwrap() = Some(req.stream);
            Ok(futures::stream::empty::<Result<StreamEvent, ApiError>>().boxed())
        }
    }

    /// Router that records the model string it was asked to resolve and always
    /// returns the same stub provider with the prefix stripped.
    struct StubRouter {
        provider: Arc<StubProvider>,
        seen_resolve: Mutex<Option<String>>,
    }

    impl ModelRouter for StubRouter {
        fn resolve(&self, model: &str) -> Result<Resolved, ApiError> {
            *self.seen_resolve.lock().unwrap() = Some(model.to_string());
            // Strip a leading `profile/` so the stub sees the local id.
            let local = model.split_once('/').map_or(model, |(_, m)| m).to_string();
            Ok(Resolved {
                provider: self.provider.clone(),
                model: local,
            })
        }
        fn available_profiles(&self) -> Vec<String> {
            vec!["stub".to_string()]
        }
    }

    #[tokio::test]
    async fn bridge_resolves_and_forwards_local_model() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router.clone());
        let resp = adapter
            .messages_create("openai/gpt-4o", Some("sys"), Vec::new())
            .await
            .expect("ok");
        // Router saw the full string; provider saw the stripped local id.
        assert_eq!(router.seen_resolve.lock().unwrap().as_deref(), Some("openai/gpt-4o"));
        assert_eq!(provider.seen_model.lock().unwrap().as_deref(), Some("gpt-4o"));
        assert_eq!(provider.seen_system.lock().unwrap().as_deref(), Some("sys"));
        assert_eq!(resp.model, "gpt-4o");
    }

    #[tokio::test]
    async fn bridge_forwards_stream_tools_and_flag() {
        let provider = Arc::new(StubProvider::new());
        let router = Arc::new(StubRouter {
            provider: provider.clone(),
            seen_resolve: Mutex::new(None),
        });
        let adapter = ProviderApiAdapter::new(router);
        let tools = vec![serde_json::json!({"name": "Read"})];
        let _s = adapter
            .stream("gemini/gemini-2.0-flash", Some("sys"), Vec::new(), tools)
            .await
            .expect("stream");
        assert_eq!(*provider.seen_tools_len.lock().unwrap(), Some(1));
        assert_eq!(*provider.seen_stream_flag.lock().unwrap(), Some(true));
    }
}
```

- [ ] **Step 2: Test + commit**

Run: `cargo test -p orchestrator provider_adapter` → 2 tests pass.
Run: `cargo build -p orchestrator --tests 2>&1 | grep -i "unused\|warning"` → no provider_adapter warnings.
Run: `cargo test -p orchestrator` → all pass (back-compat: the adapter type change must not break other orchestrator tests).
```bash
git add orchestrator/src/provider_adapter.rs
git commit -m "feat(llm-p2): ProviderApiAdapter routes per-call via ModelRouter"
```

NOTE: `cargo clippy -p orchestrator` fails only on the pre-existing `tool-api` errors; confirm no new diagnostics in `provider_adapter.rs` via the `cargo build --tests` warning check above.

---

## Task 5: Settings `providers` field

**Files:**
- Modify: `lingxi-code/core/src/settings/schema.rs`
- Modify: `lingxi-code/core/src/settings/merger.rs`
- Modify: `lingxi-code/core/src/settings/tracer.rs`

- [ ] **Step 1: Add the field to `SettingsJson`**

In `lingxi-code/core/src/settings/schema.rs`, the `SettingsJson` struct currently ends with the `model` field:

```rust
    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
```

Replace that with (adds `providers` after `model`):

```rust
    /// Scalar field (later source wins). Default model alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Object-merge field (deep-merge). LingXi extension (claude-code has no
    /// such key): named LLM provider profiles, e.g.
    /// `{ "groq": { "type": "openai", "baseUrl": "...", "apiKeyEnv": "GROQ_API_KEY" } }`.
    /// Parsed into typed profiles by `providers::parse_profiles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, Value>>,
}
```

(`BTreeMap` and `Value` are already imported in `schema.rs` — they're used by `sandbox`/`hooks`. If a build error says otherwise, add `use std::collections::BTreeMap;` / `use serde_json::Value;` to match the existing imports.)

- [ ] **Step 2: Add a test for round-trip + unknown-field tolerance**

In `schema.rs`'s `#[cfg(test)] mod tests` (add a test function):

```rust
    #[test]
    fn providers_field_roundtrips() {
        let raw = r#"{"providers":{"groq":{"type":"openai","baseUrl":"https://api.groq.com/openai/v1","apiKeyEnv":"GROQ_API_KEY"}}}"#;
        let parsed: SettingsJson = serde_json::from_str(raw).expect("parse");
        assert!(parsed.providers.is_some());
        let back = serde_json::to_string(&parsed).expect("serialize");
        assert!(back.contains("\"providers\""));
    }
```

(If `schema.rs` has no `tests` module, add `#[cfg(test)] mod tests { use super::*; <the test> }` at the end of the file.)

- [ ] **Step 3: Deep-merge `providers` in `merger.rs`**

In `lingxi-code/core/src/settings/merger.rs`, the `merge` function ends with:

```rust
        telemetry_enabled: next.telemetry_enabled.or(prev.telemetry_enabled),
        model: next.model.or(prev.model),
    }
}
```

Replace with (adds the `providers` deep-merge line):

```rust
        telemetry_enabled: next.telemetry_enabled.or(prev.telemetry_enabled),
        model: next.model.or(prev.model),
        providers: deep_merge_object(prev.providers, next.providers),
    }
}
```

- [ ] **Step 4: Add a merger test**

In `merger.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn providers_deep_merge_combines_profiles() {
        use serde_json::json;
        use std::collections::BTreeMap;
        let mut p = BTreeMap::new();
        p.insert("groq".to_string(), json!({"type": "openai"}));
        let mut n = BTreeMap::new();
        n.insert("ollama".to_string(), json!({"type": "openai"}));
        let prev = SettingsJson { providers: Some(p), ..Default::default() };
        let next = SettingsJson { providers: Some(n), ..Default::default() };
        let merged = merge(prev, next).providers.unwrap();
        assert!(merged.contains_key("groq") && merged.contains_key("ollama"));
    }
```

- [ ] **Step 5: Add `providers` to the tracer field list**

In `lingxi-code/core/src/settings/tracer.rs`, find the field-list array (the `record_layer` body lists `("model", layer.model.is_some())` last):

```rust
        ("telemetryEnabled", layer.telemetry_enabled.is_some()),
        ("model", layer.model.is_some()),
```

Replace with:

```rust
        ("telemetryEnabled", layer.telemetry_enabled.is_some()),
        ("model", layer.model.is_some()),
        ("providers", layer.providers.is_some()),
```

(The surrounding array is a `[(&str, bool); N]` literal — adding one element is fine; if the array length is annotated, update the count.)

- [ ] **Step 6: Test + lint + commit**

Run: `cargo test -p core settings` → all pass (incl. the 2 new tests).
Run: `cargo clippy -p core --all-targets -- -D warnings` → clean (`engine` does not depend on `tool-api`; if it does and the pre-existing errors surface, confirm none are in `core/src/settings`).
```bash
git add core/src/settings/schema.rs core/src/settings/merger.rs core/src/settings/tracer.rs
git commit -m "feat(llm-p2): settings `providers` object field (schema + deep-merge + provenance)"
```

---

## Task 6: Wire the registry into `init.rs`

**Files:**
- Modify: `lingxi-code/apps/cli/src/init.rs`

`init.rs` currently builds `api_client` from `AnthropicProviderAdapter` (lines ~127-129) and does NOT load settings. P2 loads settings, builds a `ProviderRegistry`, and swaps `api_client` to the routed `ProviderApiAdapter`. Anthropic stays the default, byte-identical.

- [ ] **Step 1: Read the current head of `build_runtime`**

Read `lingxi-code/apps/cli/src/init.rs` lines 100-135 to see the exact `api_base` / `api_key` / `provider` / `api_client` / `tool_provider` lines and the imports at the top of the file.

- [ ] **Step 2: Add imports**

At the top of `init.rs`, ensure these are imported (add any missing):
- `use providers::{builtin_profiles, parse_profiles, ProviderRegistry, ModelRouter};`
- `use orchestrator::ProviderApiAdapter;` (or `orchestrator::provider_adapter::ProviderApiAdapter` — match how other orchestrator items are imported in this file; `ProviderApiAdapter` is re-exported at the crate root).
- The existing import line that brings in `AnthropicProviderAdapter` (around line 34) can keep `AnthropicProviderAdapter` for now or drop it if it becomes unused (the compiler will warn; remove it if unused to satisfy `-D warnings`).

- [ ] **Step 3: Replace the `api_client` construction**

The current block (around lines 122-134) is:

```rust
    let provider = AnthropicProvider::new(api_key.clone(), Some(api_base.clone()));
    let api_client: Arc<dyn OrchestratorApiClient> =
        Arc::new(AnthropicProviderAdapter::new(provider, http.clone()));
    // M8-P6: the tool context's WebSearch tool builds `POST /v1/messages`
    // requests through its own `Arc<AnthropicProvider>`. `AnthropicProvider`
    // isn't `Clone`, so build a second cheap instance (it only stores the
    // api key + base URL).
    let tool_provider = Arc::new(AnthropicProvider::new(api_key, Some(api_base.clone())));
```

Replace with (build the registry; keep `tool_provider` Anthropic):

```rust
    // M-LLM-P2: route the orchestrator's model calls through a ProviderRegistry
    // keyed by a `provider/model` string. Built-in profiles (anthropic/openai/
    // gemini) plus any settings-declared `providers` profiles. Bare / `claude-*`
    // models resolve to the built-in `anthropic` profile → AnthropicLlmProvider,
    // which delegates verbatim to AnthropicProvider (byte-identical to the prior
    // AnthropicProviderAdapter path). openai/gemini profiles surface a clear
    // "codec not available until P3/P4" error when selected.
    let env_snapshot: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let settings_providers = load_provider_profiles(&cwd_for_settings(argv));
    let mut profiles = builtin_profiles(Some(api_base.clone()));
    match parse_profiles(settings_providers.as_ref()) {
        Ok(extra) => profiles.extend(extra),
        Err(e) => tracing::warn!(error = %e, "ignoring malformed settings `providers` block"),
    }
    let registry = Arc::new(ProviderRegistry::new(profiles, env_snapshot, http.clone()));
    let api_client: Arc<dyn OrchestratorApiClient> =
        Arc::new(ProviderApiAdapter::new(registry as Arc<dyn ModelRouter>));
    // The WebSearch tool still builds Anthropic `POST /v1/messages` requests via
    // its own provider (server-side web search is Anthropic-only in v1).
    let tool_provider = Arc::new(AnthropicProvider::new(api_key, Some(api_base.clone())));
```

(`api_key.clone()` is no longer needed for the orchestrator path — the registry reads `ANTHROPIC_API_KEY` from `env_snapshot`. Keep `let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();` above for `tool_provider`. If `api_key.clone()` becomes the only use, the compiler will guide; pass `api_key` by value to `tool_provider` as shown.)

- [ ] **Step 4: Add the settings-loading helpers**

Add these free functions near the top of `init.rs` (after the imports, before `build_runtime`). They load the merged settings `providers` block, resilient to missing files:

```rust
/// Resolve the project dir used for settings lookup (argv `--cwd` or process cwd).
fn cwd_for_settings(argv: &Argv) -> std::path::PathBuf {
    argv.cwd
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Load the merged settings `providers` object (project + user + env layers).
/// Returns `None` if settings can't be loaded or no `providers` block is set —
/// callers then fall back to built-in profiles only.
fn load_provider_profiles(
    project_dir: &std::path::Path,
) -> Option<std::collections::BTreeMap<String, serde_json::Value>> {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let inputs = lingxi_core::settings::LoadInputs {
        env: &env,
        project_dir,
        defaults: lingxi_core::settings::schema::SettingsJson::default(),
    };
    lingxi_core::settings::Settings::load(inputs)
        .ok()
        .and_then(|eff| eff.settings.providers)
}
```

(Confirm the exact paths: `lingxi_core::settings::{LoadInputs, Settings}` and `lingxi_core::settings::schema::SettingsJson` — match how `lingxi_core::settings` is referenced elsewhere; adjust the `use`/path if the crate re-exports them at a shorter path. `argv.cwd` is the `--cwd` flag field seen in the `Argv` struct.)

- [ ] **Step 5: Build + test the CLI crate**

Run: `cargo build -p cli` → compiles (fix any unused-import `-D warnings` issues, e.g. drop `AnthropicProviderAdapter` from the import if now unused).
Run: `cargo test -p cli` → all pass (incl. `build_runtime_with_defaults`, which now exercises the registry path with the default `claude-opus-4-7` model → anthropic profile).

- [ ] **Step 6: Commit**

```bash
git add apps/cli/src/init.rs
git commit -m "feat(llm-p2): wire ProviderRegistry into init.rs (anthropic default, settings profiles)"
```

---

## Task 7: Back-compat gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt -p providers -p orchestrator -p core -p cli`
Run: `git status --porcelain` — if files changed, `git add -A && git commit -m "style(llm-p2): cargo fmt"`.

- [ ] **Step 2: Clippy (own crates)**

Run: `cargo clippy -p providers --all-targets -- -D warnings` → clean.
Run: `cargo build -p orchestrator -p cli --tests 2>&1 | grep -i "warning"` → no warnings in `provider_adapter.rs` / `init.rs` (the `tool-api` clippy errors are pre-existing and out of scope; a plain build does not deny them).

- [ ] **Step 3: Tests**

Run: `cargo test -p providers` → all pass.
Run: `cargo test -p orchestrator` → all pass.
Run: `cargo test -p core settings` → all pass.
Run: `cargo test -p cli` → all pass.

- [ ] **Step 4: Back-compat — the critical gate**

Run the end-to-end / parity suites that exercise the Anthropic path through the orchestrator:
Run: `cargo test -p test-harness 2>&1 | grep -E "FAILED|test result:" | tail -20`
Expected: 0 failed. (Bare/`claude-*` models route to the `anthropic` built-in profile → `AnthropicLlmProvider` → `AnthropicProvider`, byte-identical to the prior `AnthropicProviderAdapter`, so e2e/parity tests must stay green.)

- [ ] **Step 5: Workspace build + deps**

Run: `cargo build --workspace` → Finished.
Run: `bash scripts/check-deps.sh` → `check-deps: OK — 73 workspace crates, no §8.1 dependency violations` (no new crate; `cli`/`orchestrator` → `providers`/`engine` edges already allowed).

- [ ] **Step 6: Tag (local only, no push)**

```bash
git tag -a llm-p2 -m "LLM Providers P2 (Selection/wiring): ModelSpec + ProviderRegistry/ModelRouter + settings providers field + init.rs routing. Anthropic byte-identical; openai/gemini -> P3/P4 error."
git --no-pager tag -l "llm-p2"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§5 / §6 / §12 P2):**
- `ModelSpec` (`provider/model` + claude back-compat) → Task 1.
- Built-in profiles + settings profiles, `ProviderProfile` → Task 2.
- `ProviderRegistry`/`ModelRouter`, resolve+cache, openai/gemini deferred error → Task 3.
- Per-call provider resolution from the model string (§6) → Task 4 (adapter) + Task 3 (router).
- Settings `providers` object field (deep-merge, optional, camelCase) → Task 5.
- Composition wiring in `init.rs`; `tool_provider` stays Anthropic; Anthropic default preserved → Task 6.
- Back-compat (bare/`claude-*` byte-identical; existing tests green) → Task 7 Step 4.
- *Deferred (correctly):* OpenAI/Gemini codecs (P3/P4 — registry returns a clear error, no stub); richer `list_available_models` / `/model` UX (P6); settings-declared custom profiles are parsed + registered but only become *usable* once their codec exists (P3/P4). `switch_model` needs no change — it sets the session model string, which the per-call router then routes correctly.

**2. Placeholder scan:** none — every code step has complete content; every run step has an exact command + expected result. The `init.rs` edits (Task 6) reference the actual current lines and instruct reading them first (Step 1) because that file evolves; the replacement blocks are complete.

**3. Type consistency:** `ModelSpec{profile, model}`, `ProviderProfile{kind, base_url, api_key_env}`, `ProviderKind::{Anthropic,OpenAi,Gemini}`, `ModelRouter::{resolve→Resolved, available_profiles}`, `Resolved{provider, model}`, `ProviderRegistry::new(profiles, env, transport)`, `ProviderApiAdapter::new(Arc<dyn ModelRouter>)` are named identically across tasks. `Resolved.model` is always the provider-local id (prefix stripped); `CanonicalRequest::new(resolved.model)` consumes it. The registry is generic over the transport `T` (matching `AnthropicLlmProvider<T>`) and erases to `Arc<dyn ModelRouter>` at the `init.rs` seam — consistent with P1's `Arc<dyn LlmProvider>` erasure pattern. Errors reuse `ApiError::Http(HttpError::InvalidRequest(..))` (no new `ApiError` variant added).
