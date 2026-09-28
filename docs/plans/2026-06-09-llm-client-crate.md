# LLM Client Crate Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a reusable `llm-client` Rust crate that owns provider-neutral LLM request/response types, config/registry resolution, auth seams, transport abstractions, retry/error taxonomy, redaction, and cost estimation foundations for the first implementation wave.

**Architecture:** Create `lingxi-code/llm-client` as a new workspace crate with no dependency on LingXi runtime crates (`api-client`, `providers`, `orchestrator`, `telemetry`, existing `cost`, or existing `protocol`). The crate starts with stable canonical types and deterministic pure modules, then adds provider codec/route/transport seams behind interfaces so later migration can replace the old provider/API-client seams directly.

**Tech Stack:** Rust 2021, workspace MSRV 1.82, `serde`, `serde_json`, `thiserror`, `futures`, optional `async-trait` only if needed internally, optional feature-gated `reqwest`/`tower`/cloud-auth follow-ups. Tests use `cargo test -p llm-client` from `lingxi-code`.

---

## Context

- Design spec: `docs/superpowers/specs/2026-06-09-llm-client-crate-design.md`.
- Workspace manifest: `Cargo.toml`.
- Existing reference crates:
  - `lingxi-code/api-client`: Anthropic wire DTOs, SSE parser, retry behavior.
  - `lingxi-code/providers`: provider traits, codecs, registry/profile routing.
  - `lingxi-code/cost`: pricing and cost calculation concepts.
  - `lingxi-code/orchestrator/src/provider_adapter.rs`: current runtime boundary to replace later.
- New crate location: `lingxi-code/llm-client`.
- Hard dependency boundary: `llm-client` must not depend on LingXi runtime crates (`api-client`, `providers`, `orchestrator`, `telemetry`, existing `cost`, existing `protocol`).
- Baseline verified before plan execution: `cargo test --workspace --no-run` passed in `lingxi-code`.

## Implementation Notes

- Follow TDD. For every behavior task, write the test, run it and see it fail, then implement the minimal code.
- Use `#![forbid(unsafe_code)]` in `llm-client/src/lib.rs`.
- Add `[lints] workspace = true` in `llm-client/Cargo.toml`.
- Keep the first batch focused on compile-safe reusable foundations. Provider network migration comes after the crate API and pure behaviors are locked.
- Do not commit automatically unless the human explicitly asks; this repo instruction overrides the generic plan-template commit step.

---

### Task 1: Workspace Crate Skeleton

**Files:**
- Modify: `Cargo.toml`
- Create: `lingxi-code/llm-client/Cargo.toml`
- Create: `lingxi-code/llm-client/src/lib.rs`
- Test: Cargo package resolution via `cargo test -p llm-client --no-run`

**Step 1: Write the failing package-resolution test**

Run before creating the crate:

```bash
cargo test -p llm-client --no-run
```

Expected: FAIL with an error like `package ID specification 'llm-client' did not match any packages`.

**Step 2: Add `llm-client` to the workspace**

In `Cargo.toml`, add `"llm-client",` to both `members` and `default-members` near the other flat engine crates.

**Step 3: Create `lingxi-code/llm-client/Cargo.toml`**

```toml
[package]
name = "llm-client"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[features]
default = []
transport-reqwest = []
tower = []
aws-auth = []
gcp-auth = []
azure-auth = []

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
futures = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

**Step 4: Create `lingxi-code/llm-client/src/lib.rs`**

```rust
//! Reusable LLM provider communication client.
//!
//! This crate owns provider-neutral request/response types, configuration,
//! route construction, authentication seams, transport abstractions, retry
//! classification, redaction, and per-call usage/cost estimation.

#![forbid(unsafe_code)]

pub mod error;
```

**Step 5: Create the minimal error module**

Create `lingxi-code/llm-client/src/error.rs`:

```rust
//! Provider-neutral error taxonomy.

use std::time::Duration;

/// Public provider-neutral error type for LLM client operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    /// Authentication failed or credentials are missing/invalid.
    #[error("authentication failed")]
    Authentication,
    /// Caller is authenticated but not allowed to perform the request.
    #[error("permission denied")]
    PermissionDenied,
    /// Provider rejected the request as invalid.
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    /// Provider rate-limited the request.
    #[error("rate limited")]
    RateLimited {
        /// Optional server-provided retry-after duration.
        retry_after: Option<Duration>,
        /// Optional provider-specific rate-limit scope.
        scope: Option<String>,
    },
    /// Provider quota or billing limit was exceeded.
    #[error("quota exceeded")]
    QuotaExceeded,
    /// Request exceeded provider/model context limits.
    #[error("context overflow")]
    ContextOverflow,
    /// Requested model is unavailable.
    #[error("model unavailable")]
    ModelUnavailable,
    /// Provider returned a transient/internal failure.
    #[error("provider internal error")]
    ProviderInternal,
    /// Transport failed before a provider response was decoded.
    #[error("transport error: {message}")]
    Transport { message: String },
    /// A stream failed after semantic events had been yielded.
    #[error("stream interrupted: {message}")]
    StreamInterrupted { message: String },
    /// Pricing was required but unavailable.
    #[error("cost unavailable: {message}")]
    CostUnavailable { message: String },
    /// Request used a capability the route/model does not support.
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability { capability: String },
}
```

**Step 6: Run package-resolution verification**

Run:

```bash
cargo test -p llm-client --no-run
```

Expected: PASS and compile the empty crate.

---

### Task 2: Canonical Provider/Model/Usage/Cost Types

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/types.rs`
- Test: `lingxi-code/llm-client/tests/types_test.rs`

**Step 1: Write failing tests for canonical identity and usage defaults**

Create `lingxi-code/llm-client/tests/types_test.rs`:

```rust
use llm_client::{CostEstimate, PricingModelRef, ProviderId, TokenUsage, Usage};

#[test]
fn provider_id_serializes_first_class_variants() {
    let provider = ProviderId::OpenAICompatible {
        name: "openrouter".to_string(),
    };

    let json = serde_json::to_value(&provider).expect("serialize provider id");

    assert_eq!(json, serde_json::json!({"open_ai_compatible":{"name":"openrouter"}}));
}

#[test]
fn token_usage_defaults_keep_billable_buckets_independent() {
    let usage = Usage {
        billable_tokens: TokenUsage {
            input: 10,
            output: 7,
            cache_write: 3,
            cache_read: 5,
            reasoning_output: 2,
        },
        context_tokens: Some(30),
        provider_reported_total_tokens: Some(99),
        ..Usage::default()
    };

    assert_eq!(usage.billable_tokens.input, 10);
    assert_eq!(usage.billable_tokens.output, 7);
    assert_eq!(usage.billable_tokens.cache_write, 3);
    assert_eq!(usage.billable_tokens.cache_read, 5);
    assert_eq!(usage.billable_tokens.reasoning_output, 2);
    assert_eq!(usage.context_tokens, Some(30));
    assert_eq!(usage.provider_reported_total_tokens, Some(99));
}

#[test]
fn cost_estimate_can_mark_unknown_pricing_without_dropping_usage() {
    let estimate = CostEstimate::unestimated(PricingModelRef {
        pricing_provider_id: ProviderId::AnthropicFirstParty,
        billing_model: "unknown-model".to_string(),
        request_model: "unknown-model".to_string(),
        display_model: "Unknown Model".to_string(),
    });

    assert!(!estimate.estimated);
    assert_eq!(estimate.total_cost_usd, None);
    assert_eq!(estimate.pricing_model.billing_model, "unknown-model");
}
```

**Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p llm-client --test types_test
```

Expected: FAIL because `types` exports do not exist.

**Step 3: Implement canonical types**

Create `lingxi-code/llm-client/src/types.rs` with:

```rust
//! Provider-neutral public types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Explicit provider identity resolved before request execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    AnthropicFirstParty,
    OpenAI,
    OpenAICompatible { name: String },
    Gemini,
    VertexGemini,
    VertexClaude,
    BedrockClaude,
    AzureOpenAI,
    Custom { name: String },
}

/// Concrete pricing identity emitted by route resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingModelRef {
    pub pricing_provider_id: ProviderId,
    pub billing_model: String,
    pub request_model: String,
    pub display_model: String,
}

/// Independent billable token buckets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub reasoning_output: u64,
}

/// Normalized usage returned by provider codecs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub billable_tokens: TokenUsage,
    pub context_tokens: Option<u64>,
    pub provider_reported_total_tokens: Option<u64>,
    pub server_tool_use: Option<ServerToolUsage>,
    #[serde(default)]
    pub provider_metadata: Value,
}

/// Provider-side server tool usage counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerToolUsage {
    pub web_search_requests: u64,
}

/// Per-call cost estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub pricing_model: PricingModelRef,
    pub total_cost_usd: Option<f64>,
    pub input_cost_usd: Option<f64>,
    pub output_cost_usd: Option<f64>,
    pub cache_read_cost_usd: Option<f64>,
    pub cache_write_cost_usd: Option<f64>,
    pub reasoning_cost_usd: Option<f64>,
    pub estimated: bool,
    pub pricing_source: Option<String>,
}

impl CostEstimate {
    /// Build the default unknown-pricing result used by `MarkUnestimated`.
    #[must_use]
    pub fn unestimated(pricing_model: PricingModelRef) -> Self {
        Self {
            pricing_model,
            total_cost_usd: None,
            input_cost_usd: None,
            output_cost_usd: None,
            cache_read_cost_usd: None,
            cache_write_cost_usd: None,
            reasoning_cost_usd: None,
            estimated: false,
            pricing_source: None,
        }
    }
}
```

Update `lib.rs`:

```rust
pub mod error;
pub mod types;

pub use error::LlmError;
pub use types::{CostEstimate, PricingModelRef, ProviderId, ServerToolUsage, TokenUsage, Usage};
```

**Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test -p llm-client --test types_test
```

Expected: PASS.

---

### Task 3: Config Profiles and Registry Model Listings

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/config.rs`
- Create: `lingxi-code/llm-client/src/registry.rs`
- Test: `lingxi-code/llm-client/tests/registry_test.rs`

**Step 1: Write failing registry tests**

Create `lingxi-code/llm-client/tests/registry_test.rs`:

```rust
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, ModelRegistry,
    PricingConfig, ProviderId, ProviderProfile, ProtocolFamily,
};

fn test_config() -> ClientConfig {
    ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            profile_name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::Env {
                var: "OPENROUTER_API_KEY".to_string(),
            },
            models: vec![ModelProfile {
                display_model: "Claude via OpenRouter".to_string(),
                request_model: "anthropic/claude-sonnet-4".to_string(),
                billing_model: "claude-sonnet-4".to_string(),
                aliases: vec!["or-sonnet".to_string()],
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    vision: false,
                    documents: false,
                    reasoning: false,
                    structured_output: true,
                },
            }],
            pricing: PricingConfig::default(),
        }],
    }
}

#[test]
fn available_models_lists_configured_profile_models_and_aliases() {
    let registry = ModelRegistry::from_config(test_config()).expect("registry");

    let listings = registry.available_models();

    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].profile_name, "openrouter");
    assert_eq!(listings[0].display_model, "Claude via OpenRouter");
    assert_eq!(listings[0].request_model, "anthropic/claude-sonnet-4");
    assert_eq!(listings[0].billing_model, "claude-sonnet-4");
    assert_eq!(listings[0].aliases, vec!["or-sonnet"]);
    assert!(listings[0].capabilities.streaming);
}

#[test]
fn resolve_uses_alias_without_model_string_provider_guessing() {
    let registry = ModelRegistry::from_config(test_config()).expect("registry");

    let route = registry.resolve("or-sonnet").expect("route");

    assert_eq!(route.profile_name, "openrouter");
    assert_eq!(route.request_model, "anthropic/claude-sonnet-4");
    assert_eq!(route.pricing_model.billing_model, "claude-sonnet-4");
    assert_eq!(
        route.pricing_model.pricing_provider_id,
        ProviderId::OpenAICompatible {
            name: "openrouter".to_string(),
        }
    );
}
```

**Step 2: Run tests to verify RED**

Run:

```bash
cargo test -p llm-client --test registry_test
```

Expected: FAIL because config/registry types do not exist.

**Step 3: Implement config models**

Create `lingxi-code/llm-client/src/config.rs` with config structs/enums used by the tests:

```rust
//! Serde-friendly client configuration.

use crate::ProviderId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub provider_id: ProviderId,
    pub profile_name: String,
    pub base_url: String,
    pub protocol: ProtocolFamily,
    pub auth: AuthStrategy,
    pub credential: CredentialConfig,
    #[serde(default)]
    pub models: Vec<ModelProfile>,
    #[serde(default)]
    pub pricing: PricingConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChat,
    GeminiGenerateContent,
    VertexGemini,
    VertexClaude,
    BedrockClaude,
    AzureOpenAi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrategy {
    ApiKey,
    Bearer,
    OAuthBearer,
    AwsSigV4,
    GcpToken,
    AzureToken,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CredentialConfig {
    Env { var: String },
    Static { id: String },
    HostManaged { id: String },
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    pub display_model: String,
    pub request_model: String,
    pub billing_model: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub streaming: bool,
    pub tools: bool,
    pub vision: bool,
    pub documents: bool,
    pub reasoning: bool,
    pub structured_output: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingConfig {
    pub require_priced: bool,
}
```

**Step 4: Implement registry resolution**

Create `lingxi-code/llm-client/src/registry.rs` with:

```rust
//! Model registry and route identity resolution.

use crate::{Capabilities, ClientConfig, LlmError, PricingModelRef, ProviderId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelListing {
    pub provider_id: ProviderId,
    pub profile_name: String,
    pub display_model: String,
    pub request_model: String,
    pub billing_model: String,
    pub aliases: Vec<String>,
    pub capabilities: Capabilities,
    pub pricing_known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub provider_id: ProviderId,
    pub profile_name: String,
    pub request_model: String,
    pub display_model: String,
    pub pricing_model: PricingModelRef,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    config: ClientConfig,
}

impl ModelRegistry {
    pub fn from_config(config: ClientConfig) -> Result<Self, LlmError> {
        if config.providers.iter().any(|provider| provider.models.is_empty()) {
            return Err(LlmError::InvalidRequest {
                message: "provider profile must declare at least one model".to_string(),
            });
        }
        Ok(Self { config })
    }

    #[must_use]
    pub fn available_models(&self) -> Vec<ModelListing> {
        self.config
            .providers
            .iter()
            .flat_map(|provider| {
                provider.models.iter().map(|model| ModelListing {
                    provider_id: provider.provider_id.clone(),
                    profile_name: provider.profile_name.clone(),
                    display_model: model.display_model.clone(),
                    request_model: model.request_model.clone(),
                    billing_model: model.billing_model.clone(),
                    aliases: model.aliases.clone(),
                    capabilities: model.capabilities,
                    pricing_known: !provider.pricing.require_priced,
                })
            })
            .collect()
    }

    pub fn resolve(&self, requested: &str) -> Result<ResolvedRoute, LlmError> {
        for provider in &self.config.providers {
            for model in &provider.models {
                let matches = model.display_model == requested
                    || model.request_model == requested
                    || model.aliases.iter().any(|alias| alias == requested);
                if matches {
                    return Ok(ResolvedRoute {
                        provider_id: provider.provider_id.clone(),
                        profile_name: provider.profile_name.clone(),
                        request_model: model.request_model.clone(),
                        display_model: model.display_model.clone(),
                        pricing_model: PricingModelRef {
                            pricing_provider_id: provider.provider_id.clone(),
                            billing_model: model.billing_model.clone(),
                            request_model: model.request_model.clone(),
                            display_model: model.display_model.clone(),
                        },
                        capabilities: model.capabilities,
                    });
                }
            }
        }

        Err(LlmError::ModelUnavailable)
    }
}
```

Update `lib.rs` exports:

```rust
pub mod config;
pub mod error;
pub mod registry;
pub mod types;

pub use config::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, PricingConfig,
    ProtocolFamily, ProviderProfile,
};
pub use error::LlmError;
pub use registry::{ModelListing, ModelRegistry, ResolvedRoute};
pub use types::{CostEstimate, PricingModelRef, ProviderId, ServerToolUsage, TokenUsage, Usage};
```

**Step 5: Run tests to verify GREEN**

Run:

```bash
cargo test -p llm-client --test registry_test
cargo test -p llm-client --test types_test
```

Expected: PASS.

---

### Task 4: Credential Providers and Request-Aware Authenticators

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/credentials.rs`
- Create: `lingxi-code/llm-client/src/auth.rs`
- Create: `lingxi-code/llm-client/src/transport.rs`
- Test: `lingxi-code/llm-client/tests/auth_test.rs`

**Step 1: Write failing auth tests**

Create tests that prove:
- `EnvCredentialProvider` loads an env var by scope.
- `StaticCredentialProvider` returns a redacted credential debug string.
- `ApiKeyAuthenticator` inserts `x-api-key` after URL/body are prepared.
- `BearerAuthenticator` inserts `Authorization: Bearer ...`.

Run:

```bash
cargo test -p llm-client --test auth_test
```

Expected: FAIL because modules do not exist.

**Step 2: Implement minimal transport request type**

Create `PreparedRequest` with `url: String`, `headers: BTreeMap<String, String>`, and `body: Vec<u8>`.

**Step 3: Implement credentials**

Implement:
- `CredentialScope { provider_id, profile_name }`
- `Credential::ApiKey(String)` and `Credential::BearerToken(String)`
- `CredentialProvider` trait with boxed future or simple synchronous `load` for first pass
- `StaticCredentialProvider`
- `EnvCredentialProvider`
- redacted `Debug` for `Credential`

**Step 4: Implement authenticators**

Implement:
- `Authenticator` trait receiving and returning `PreparedRequest`
- `ApiKeyAuthenticator`
- `BearerAuthenticator`

**Step 5: Verify**

Run:

```bash
cargo test -p llm-client --test auth_test
```

Expected: PASS.

---

### Task 5: Redaction Utilities

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/redaction.rs`
- Test: `lingxi-code/llm-client/tests/redaction_test.rs`

**Step 1: Write failing redaction tests**

Tests should prove:
- `Authorization` and `x-api-key` header values become `[REDACTED]`.
- Query params named `api_key`, `key`, `access_token`, `refresh_token`, and `signature` are redacted.
- JSON fields `api_key`, `access_token`, `refresh_token`, `secret_access_key`, and `authorization` are redacted recursively.

Run:

```bash
cargo test -p llm-client --test redaction_test
```

Expected: FAIL.

**Step 2: Implement `Redactor`**

Create:
- `Redactor::default()`
- `redact_headers(&BTreeMap<String, String>) -> BTreeMap<String, String>`
- `redact_url(&str) -> String`
- `redact_json(&serde_json::Value) -> serde_json::Value`

**Step 3: Verify**

Run:

```bash
cargo test -p llm-client --test redaction_test
```

Expected: PASS.

---

### Task 6: Cost Estimator and Unknown-Pricing Policies

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/cost.rs`
- Test: `lingxi-code/llm-client/tests/cost_test.rs`

**Step 1: Write failing cost tests**

Tests should prove:
- Exact `(ProviderId, billing_model)` match computes input/output/cache/reasoning cost independently.
- External override catalog wins over built-in catalog.
- `MarkUnestimated` returns a response-compatible unestimated cost.
- `RequirePriced` returns `LlmError::CostUnavailable`.

Run:

```bash
cargo test -p llm-client --test cost_test
```

Expected: FAIL.

**Step 2: Implement pricing structures**

Create:
- `PricingPolicy::{MarkUnestimated, ApplyFallbackTier, RequirePriced}`
- `TokenPricing { input_per_million, output_per_million, cache_write_per_million, cache_read_per_million, reasoning_per_million }`
- `PricingCatalog` with exact map and optional overrides
- `CostEstimator::estimate(pricing_model, usage) -> Result<CostEstimate, LlmError>`

**Step 3: Verify**

Run:

```bash
cargo test -p llm-client --test cost_test
```

Expected: PASS.

---

### Task 7: Protocol and Stream Decoder Traits

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/protocol.rs`
- Create: `lingxi-code/llm-client/src/transport.rs` if not already complete
- Test: `lingxi-code/llm-client/tests/protocol_test.rs`

**Step 1: Write failing protocol tests**

Tests should prove:
- A dummy protocol can encode `LlmRequest` into a `PreparedBody`.
- A dummy stream decoder maps raw SSE frames to ordered `LlmEvent` values.
- Unsupported capabilities fail before transport.

Run:

```bash
cargo test -p llm-client --test protocol_test
```

Expected: FAIL.

**Step 2: Implement canonical request/response/event types**

Add minimal:
- `LlmRequest`
- `LlmResponse`
- `LlmEvent`
- `Message`
- `ContentBlock`
- `ToolDeclaration`
- `ToolChoice`
- `ResponseFormat`
- `ReasoningConfig`

**Step 3: Implement protocol traits**

Add:
- `Protocol`
- `StreamDecoder`
- `PreparedBody`
- `RawResponse`
- `RawStreamFrame`

Prefer explicit boxed futures only when async is required; pure encode/decode stays synchronous.

**Step 4: Verify**

Run:

```bash
cargo test -p llm-client --test protocol_test
```

Expected: PASS.

---

### Task 8: Retry Classification

**Files:**
- Modify: `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/retry.rs`
- Test: `lingxi-code/llm-client/tests/retry_test.rs`

**Step 1: Write failing retry tests**

Tests should prove:
- 429, 500, 502, 503, 504, and 529 are retryable.
- `retry-after` header parses seconds.
- auth, permission, invalid request, context overflow, unsupported capability, and cost unavailability are not retryable.
- already-yielding stream interruption is not replayed.

Run:

```bash
cargo test -p llm-client --test retry_test
```

Expected: FAIL.

**Step 2: Implement retry policy**

Add:
- `RetryPolicy`
- `RetryDecision`
- `ResponseMetadata { status, headers }`
- `classify_error(&LlmError) -> RetryDecision`
- `classify_response(&ResponseMetadata) -> RetryDecision`

**Step 3: Verify**

Run:

```bash
cargo test -p llm-client --test retry_test
```

Expected: PASS.

---

### Task 9: Anthropic Usage Normalization Fixture

**Files:**
- Modify: `lingxi-code/llm-client/src/protocol.rs`
- Create: `lingxi-code/llm-client/src/protocol/anthropic.rs` or `lingxi-code/llm-client/src/anthropic.rs`
- Test: `lingxi-code/llm-client/tests/anthropic_usage_test.rs`

**Step 1: Write failing Anthropic usage tests**

Use a provider-response JSON fixture inline or from `liter-llm/fixtures/providers/anthropic_chat.json` if stable. Test that:
- `input_tokens` maps to `billable_tokens.input`.
- `output_tokens` maps to `billable_tokens.output`.
- `cache_creation_input_tokens` maps to `billable_tokens.cache_write`.
- `cache_read_input_tokens` maps to `billable_tokens.cache_read`.
- `server_tool_use.web_search_requests` is preserved.

Run:

```bash
cargo test -p llm-client --test anthropic_usage_test
```

Expected: FAIL.

**Step 2: Implement minimal Anthropic usage normalizer**

Add a pure function such as:

```rust
pub fn normalize_anthropic_usage(value: &serde_json::Value) -> Usage
```

Do not implement network transport in this task.

**Step 3: Verify**

Run:

```bash
cargo test -p llm-client --test anthropic_usage_test
```

Expected: PASS.

---

### Task 10: First-Wave Final Verification

**Files:**
- All files changed by previous tasks.

**Step 1: Run crate tests**

Run:

```bash
cargo test -p llm-client
```

Expected: PASS.

**Step 2: Run crate clippy**

Run:

```bash
cargo clippy -p llm-client -- -D warnings
```

Expected: PASS.

**Step 3: Run workspace compile check**

Run:

```bash
cargo test --workspace --no-run
```

Expected: PASS.

**Step 4: Inspect dependency boundary**

Run:

```bash
cargo tree -p llm-client
```

Expected: no dependency on `api-client`, `providers`, `orchestrator`, `telemetry`, existing `cost`, or existing `protocol`.

---

## Future Follow-Up Plan After This Foundation

This plan intentionally creates the reusable crate foundation first. After it passes, write a second implementation plan for:

1. Anthropic Messages protocol complete/stream fixtures.
2. OpenAI Chat and OpenAI-compatible Chat fixtures.
3. Gemini generateContent fixtures and schema-dialect validation.
4. Default route builder and `LlmClient` high-level facade.
5. LingXi composition-root migration from `ProviderRegistry` / `ProviderApiAdapter` to `llm_client::LlmClient`.
6. Deletion-gated removal of old Anthropic communication paths after parity tests pass.
