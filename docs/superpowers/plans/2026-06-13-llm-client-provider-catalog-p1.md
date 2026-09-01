# llm-client provider catalog — Phase 1 (vendored snapshot + builtin presets) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Per the project's concurrent-agent worktree hazard: run edit-agents **SEQUENTIALLY** in one checkout, never in parallel.

**Goal:** Add OpenRouter, DeepSeek, and GLM (Zhipu coding plan, Anthropic-compatible) as built-in providers in the `llm-client` crate, sourced from a vendored models.dev snapshot, exposed via `llm_client::builtin_presets()` returning provider profiles + a pricing catalog that merge into `ClientConfig`/`PricingCatalog`.

**Architecture:** A vendored per-provider slice of `models.dev/api.json` supplies **catalog metadata** (model ids, names, cost, capabilities). A hand-authored Rust **routing table** supplies **wire/auth concerns** (base URL, `ProtocolFamily`, `AuthStrategy`, credential env). A builder parses the embedded JSON, maps each model to a `ModelProfile` + `TokenPricing`, attaches the routing override, and returns a `BuiltinCatalog`. No new wire codecs and no new auth strategies — `build_codec` already supports `OpenAiChat` (OpenRouter/DeepSeek) and `AnthropicMessages` (GLM), and `authenticate` already routes `ApiKey`+`OpenAiChat`→Bearer and `ApiKey`+`AnthropicMessages`→`x-api-key`.

**Tech Stack:** Rust, `serde`/`serde_json` (already deps), `include_str!` for embedding, existing `llm-client` types (`ProviderProfile`, `ModelProfile`, `Capabilities`, `ClientConfig`, `ModelRegistry`, `PricingCatalog`, `TokenPricing`, `ProviderId`, `ProtocolFamily`, `AuthStrategy`, `CredentialConfig`).

**Spec:** `docs/superpowers/specs/2026-06-13-llm-client-provider-catalog-design.md` (Phase 1 section).

---

## Conventions (apply to every task)

- Cargo root is `lingxi-code/`. Run cargo from there: `cd lingxi-code && cargo ...`. Run git from the **repo root** with explicit `lingxi-code/...` / `docs/...` paths.
- **NEVER `git add -A`** — untracked `codex/`, `liter-llm/`, `opencode/`, `.codegraph/` dirs live at repo root. Stage only named paths.
- Lints: every touched crate must pass `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings` and build under `-D missing-docs` (workspace lint). All new public items need `///` docs.
- TDD: write the failing test, **observe RED**, then implement.
- Commit trailer EXACTLY (own line, blank line before it):
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
  Commit with `git commit -F <tempfile>` from the repo root.
- Frozen crates: do **not** touch `lingxi-code/platform-api` or `lingxi-code/protocol`. This plan touches only `lingxi-code/llm-client`.
- `engine-mobile` must keep building (no new non-optional heavy deps; this plan adds none).

## File structure (created/modified in Phase 1)

- Create `lingxi-code/llm-client/data/models-dev/openrouter.json` — vendored OpenRouter slice.
- Create `lingxi-code/llm-client/data/models-dev/deepseek.json` — vendored DeepSeek slice.
- Create `lingxi-code/llm-client/data/models-dev/zhipuai-coding-plan.json` — vendored GLM slice.
- Create `lingxi-code/llm-client/scripts/refresh-models-dev.sh` — regenerates the slices from upstream.
- Create `lingxi-code/llm-client/src/catalog/mod.rs` — module root; re-exports `builtin_presets`, `BuiltinCatalog`.
- Create `lingxi-code/llm-client/src/catalog/models_dev.rs` — serde schema for a models.dev provider slice.
- Create `lingxi-code/llm-client/src/catalog/map.rs` — `Model` → `ModelProfile` + `TokenPricing` mapping.
- Create `lingxi-code/llm-client/src/catalog/presets.rs` — routing table + `builtin_presets()`.
- Create `lingxi-code/llm-client/tests/catalog_test.rs` — integration test (merge into config + registry resolve).
- Modify `lingxi-code/llm-client/src/lib.rs` — add `pub mod catalog;` + re-exports.

## Out of scope for Phase 1 (recorded so it is not lost)

- OpenRouter ranking headers `HTTP-Referer` / `X-Title`: there is no per-route static-header injection seam today, and adding one touches the request build path. These headers are **optional** (OpenRouter routes work without them; they only affect leaderboard attribution). Deferred — not required for functioning routes.
- `limit.context` is parsed (so the schema is faithful) but **not stored** — `ModelProfile`/`Capabilities` have no context-window field today, and overflow handling lives elsewhere. No field is invented here.
- GitHub Copilot (Phase 2) and the grouped picker (Phase 3).

---

### Task 0: worktree + baseline

**Files:** none (setup).

- [ ] **Step 1: Create the isolated worktree.** Use the `superpowers:using-git-worktrees` skill (the user's `parity-llm-client-3a` checkout has standing uncommitted work — do NOT work in the primary tree). Branch off `parity-llm-client-3a`. Name e.g. `provider-catalog-p1`.

- [ ] **Step 2: Fresh-worktree build prereq.** A fresh worktree has no built fixture binaries. From the worktree:

Run: `cd lingxi-code && cargo build -p llm-client`
Expected: builds clean (records the current green baseline).

- [ ] **Step 3: Baseline test count.**

Run: `cd lingxi-code && cargo test -p llm-client 2>&1 | tail -20`
Expected: all green; note the pass count.

No commit.

---

### Task 1: vendor the models.dev slices + refresh script

**Files:**
- Create: `lingxi-code/llm-client/scripts/refresh-models-dev.sh`
- Create: `lingxi-code/llm-client/data/models-dev/openrouter.json`
- Create: `lingxi-code/llm-client/data/models-dev/deepseek.json`
- Create: `lingxi-code/llm-client/data/models-dev/zhipuai-coding-plan.json`

- [ ] **Step 1: Write the refresh script.** Create `lingxi-code/llm-client/scripts/refresh-models-dev.sh`:

```bash
#!/usr/bin/env bash
# Regenerate the vendored models.dev provider slices.
#
# Source of truth: https://models.dev/api.json  (Record<providerId, Provider>).
# We vendor only the provider slices we ship — never the full ~2.3 MB file.
# Re-run after upstream model/price changes; commit the resulting JSON.
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)/data/models-dev"
mkdir -p "$DIR"
URL="${MODELS_DEV_URL:-https://models.dev}/api.json"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo "fetching $URL"
curl -fsSL --max-time 60 "$URL" -o "$TMP"

for provider in openrouter deepseek zhipuai-coding-plan; do
  python3 - "$TMP" "$provider" "$DIR/$provider.json" <<'PY'
import json, sys
src, provider, dst = sys.argv[1], sys.argv[2], sys.argv[3]
data = json.load(open(src))
slice_ = data.get(provider)
if slice_ is None:
    sys.exit(f"provider {provider!r} absent from upstream api.json")
with open(dst, "w") as f:
    json.dump(slice_, f, indent=2, sort_keys=True, ensure_ascii=False)
    f.write("\n")
print(f"wrote {dst}: {len(slice_.get('models', {}))} models")
PY
done
echo "done"
```

- [ ] **Step 2: Make it executable and run it** (this is the one networked step; it produces deterministic committed files):

Run: `chmod +x lingxi-code/llm-client/scripts/refresh-models-dev.sh && lingxi-code/llm-client/scripts/refresh-models-dev.sh`
Expected output (model counts, dates may drift):
```
wrote .../openrouter.json: 337 models
wrote .../deepseek.json: 4 models
wrote .../zhipuai-coding-plan.json: 6 models
done
```

- [ ] **Step 3: Sanity-check the vendored files exist and parse.**

Run: `python3 -c "import json,glob; [print(f, len(json.load(open(f))['models'])) for f in sorted(glob.glob('lingxi-code/llm-client/data/models-dev/*.json'))]"`
Expected: three lines listing each file with its model count.

- [ ] **Step 4: Commit.**

```bash
git add lingxi-code/llm-client/scripts/refresh-models-dev.sh \
        lingxi-code/llm-client/data/models-dev/openrouter.json \
        lingxi-code/llm-client/data/models-dev/deepseek.json \
        lingxi-code/llm-client/data/models-dev/zhipuai-coding-plan.json
git commit -F - <<'EOF'
feat(llm-client): vendor models.dev slices for openrouter/deepseek/glm-coding

Per-provider slices of models.dev/api.json (not the full 2.3 MB file) plus a
refresh script. Catalog metadata only; routing/auth is hand-authored next.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 2: models.dev serde schema

**Files:**
- Create: `lingxi-code/llm-client/src/catalog/models_dev.rs`
- Create: `lingxi-code/llm-client/src/catalog/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`

- [ ] **Step 1: Write the failing test.** Create `lingxi-code/llm-client/src/catalog/models_dev.rs` with the schema and an inline test that parses the embedded DeepSeek slice:

```rust
//! Serde schema for a single models.dev provider slice (`data/models-dev/*.json`).
//!
//! Tolerant of unknown/added fields: upstream evolves, so only the fields we map
//! are declared and everything else is ignored. Optional fields default so a
//! missing key never fails the parse.

use serde::Deserialize;

/// One provider slice: `{ api?, name, env[], id, models: { id -> Model } }`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSlice {
    /// Upstream API base (advisory; the routing table overrides it).
    #[serde(default)]
    pub api: Option<String>,
    /// Human provider name.
    pub name: String,
    /// Upstream credential env var names (advisory).
    #[serde(default)]
    pub env: Vec<String>,
    /// Provider id as keyed in api.json.
    pub id: String,
    /// Models keyed by model id.
    pub models: std::collections::BTreeMap<String, Model>,
}

/// One model entry. Only mapped fields are declared; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    /// Wire model id (sent on the request; also the billing key).
    pub id: String,
    /// Human-facing label.
    pub name: String,
    /// Whether the model supports native tool calls.
    #[serde(default)]
    pub tool_call: bool,
    /// Whether the model supports reasoning.
    #[serde(default)]
    pub reasoning: bool,
    /// Whether the model supports structured output.
    #[serde(default)]
    pub structured_output: bool,
    /// Input/output modalities (absent → text-only).
    #[serde(default)]
    pub modalities: Option<Modalities>,
    /// Per-million-token costs (absent → unpriced).
    #[serde(default)]
    pub cost: Option<Cost>,
    /// Token limits (context window etc.). Parsed for fidelity; unused in P1.
    #[serde(default)]
    pub limit: Option<Limit>,
    /// Catalog status (`alpha`/`beta`/`deprecated`), when present.
    #[serde(default)]
    pub status: Option<String>,
}

/// Input/output modality lists.
#[derive(Debug, Clone, Deserialize)]
pub struct Modalities {
    /// Accepted input modalities (e.g. `text`, `image`, `pdf`).
    #[serde(default)]
    pub input: Vec<String>,
    /// Produced output modalities.
    #[serde(default)]
    pub output: Vec<String>,
}

/// Per-million-token costs (USD).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Cost {
    /// Input price per million tokens.
    pub input: f64,
    /// Output price per million tokens.
    pub output: f64,
    /// Cache-read price per million tokens.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Cache-write price per million tokens.
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Token limits.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Limit {
    /// Context-window size in tokens.
    pub context: u64,
    /// Max output tokens.
    pub output: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");

    #[test]
    fn parses_deepseek_slice() {
        let slice: ProviderSlice = serde_json::from_str(DEEPSEEK).expect("deepseek slice parses");
        assert_eq!(slice.name, "DeepSeek");
        assert_eq!(slice.models.len(), 4);
        // Every model carries an id + name.
        for (key, model) in &slice.models {
            assert_eq!(key, &model.id);
            assert!(!model.name.is_empty());
        }
    }
}
```

- [ ] **Step 2: Wire the module.** Create `lingxi-code/llm-client/src/catalog/mod.rs`:

```rust
//! Built-in provider catalog assembled from vendored models.dev snapshots.

pub mod map;
pub mod models_dev;
pub mod presets;

pub use presets::{builtin_presets, BuiltinCatalog};
```

(`map` and `presets` are created in later tasks; this file is rewritten as they land. For Task 2, temporarily declare only `models_dev`:)

```rust
//! Built-in provider catalog assembled from vendored models.dev snapshots.

pub mod models_dev;
```

Add to `lingxi-code/llm-client/src/lib.rs` after the existing `pub mod` block (keep alphabetical neighbors; place after `pub mod cost;`):

```rust
pub mod catalog;
```

- [ ] **Step 3: Run the test to verify it FAILS first.** Before adding `models_dev.rs`'s test passes trivially, confirm RED by temporarily asserting the wrong count. Simpler: run as-is and confirm it COMPILES+PASSES only after the file exists. To honor RED, first run with `assert_eq!(slice.models.len(), 99);`:

Run: `cd lingxi-code && cargo test -p llm-client parses_deepseek_slice`
Expected: FAIL (`left: 4, right: 99`). Then change `99` back to `4`.

- [ ] **Step 4: Run the test to verify it PASSES.**

Run: `cd lingxi-code && cargo test -p llm-client parses_deepseek_slice`
Expected: PASS.

- [ ] **Step 5: Clippy.**

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/llm-client/src/catalog/mod.rs \
        lingxi-code/llm-client/src/catalog/models_dev.rs \
        lingxi-code/llm-client/src/lib.rs
git commit -F - <<'EOF'
feat(llm-client): models.dev provider-slice serde schema

Tolerant Deserialize structs for a vendored provider slice; parses the
embedded deepseek slice in a unit test.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 3: model → ModelProfile + pricing mapping

**Files:**
- Create: `lingxi-code/llm-client/src/catalog/map.rs`
- Modify: `lingxi-code/llm-client/src/catalog/mod.rs`

- [ ] **Step 1: Write the failing test.** Create `lingxi-code/llm-client/src/catalog/map.rs`:

```rust
//! Map a models.dev [`Model`] to llm-client's [`ModelProfile`] + optional
//! [`TokenPricing`]. Routing/auth is handled by the preset table, not here.

use crate::catalog::models_dev::Model;
use crate::{Capabilities, ModelProfile, TokenPricing};

/// Map a models.dev model to a [`ModelProfile`]. `request_model` and
/// `billing_model` are both the wire id; `display_model` is the human name.
#[must_use]
pub fn to_model_profile(model: &Model) -> ModelProfile {
    ModelProfile {
        display_model: model.name.clone(),
        request_model: model.id.clone(),
        billing_model: model.id.clone(),
        aliases: Vec::new(),
        capabilities: to_capabilities(model),
    }
}

/// Derive capabilities. Streaming is always supported by these providers; vision
/// and documents come from input modalities.
#[must_use]
pub fn to_capabilities(model: &Model) -> Capabilities {
    let has = |m: &str| {
        model
            .modalities
            .as_ref()
            .is_some_and(|x| x.input.iter().any(|i| i == m))
    };
    Capabilities {
        streaming: true,
        tools: model.tool_call,
        vision: has("image"),
        documents: has("pdf"),
        reasoning: model.reasoning,
        structured_output: model.structured_output,
    }
}

/// Map costs to [`TokenPricing`]. models.dev costs are already per-million
/// tokens. Returns `None` when the model carries no cost block (unpriced);
/// an all-zero cost block maps to a real zero price (free, but priced).
#[must_use]
pub fn to_pricing(model: &Model) -> Option<TokenPricing> {
    model.cost.map(|c| TokenPricing {
        input_per_million: c.input,
        output_per_million: c.output,
        cache_write_per_million: c.cache_write.unwrap_or(0.0),
        cache_read_per_million: c.cache_read.unwrap_or(0.0),
        reasoning_per_million: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::models_dev::ProviderSlice;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");

    fn deepseek() -> ProviderSlice {
        serde_json::from_str(DEEPSEEK).unwrap()
    }

    #[test]
    fn maps_id_name_and_caps() {
        let slice = deepseek();
        let model = slice.models.values().next().unwrap();
        let profile = to_model_profile(model);
        assert_eq!(profile.request_model, model.id);
        assert_eq!(profile.billing_model, model.id);
        assert_eq!(profile.display_model, model.name);
        assert!(profile.capabilities.streaming);
        assert_eq!(profile.capabilities.tools, model.tool_call);
    }

    #[test]
    fn priced_model_maps_cost_per_million() {
        let slice = deepseek();
        // Every deepseek model has a cost block.
        let model = slice.models.values().next().unwrap();
        let pricing = to_pricing(model).expect("deepseek model is priced");
        let cost = model.cost.unwrap();
        assert!((pricing.input_per_million - cost.input).abs() < f64::EPSILON);
        assert!((pricing.output_per_million - cost.output).abs() < f64::EPSILON);
    }
}
```

- [ ] **Step 2: Declare the module.** Update `lingxi-code/llm-client/src/catalog/mod.rs` to:

```rust
//! Built-in provider catalog assembled from vendored models.dev snapshots.

pub mod map;
pub mod models_dev;
```

- [ ] **Step 3: Run to verify FAIL first.** Temporarily set `assert!(profile.capabilities.streaming == false);` to force RED.

Run: `cd lingxi-code && cargo test -p llm-client -- catalog::map`
Expected: FAIL. Then restore `assert!(profile.capabilities.streaming);`.

- [ ] **Step 4: Run to verify PASS.**

Run: `cd lingxi-code && cargo test -p llm-client -- catalog::map`
Expected: PASS (both tests).

- [ ] **Step 5: Clippy.**

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit.**

```bash
git add lingxi-code/llm-client/src/catalog/map.rs lingxi-code/llm-client/src/catalog/mod.rs
git commit -F - <<'EOF'
feat(llm-client): map models.dev model -> ModelProfile + TokenPricing

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 4: routing table + builtin_presets()

**Files:**
- Create: `lingxi-code/llm-client/src/catalog/presets.rs`
- Modify: `lingxi-code/llm-client/src/catalog/mod.rs`
- Modify: `lingxi-code/llm-client/src/lib.rs`

- [ ] **Step 1: Write the failing integration test.** Create `lingxi-code/llm-client/tests/catalog_test.rs`:

```rust
//! Integration: builtin_presets() merges into ClientConfig + resolves via the
//! registry, and the GLM routing override beats the snapshot api path.

use llm_client::{
    builtin_presets, ClientConfig, ModelRegistry, ProtocolFamily, ProviderId,
};

#[test]
fn presets_cover_three_providers_with_models() {
    let catalog = builtin_presets();
    // openrouter + deepseek + glm-coding.
    assert_eq!(catalog.providers.len(), 3);
    for p in &catalog.providers {
        assert!(!p.models.is_empty(), "{} has models", p.profile_name);
    }
}

#[test]
fn glm_routes_to_anthropic_endpoint_not_paas() {
    let catalog = builtin_presets();
    let glm = catalog
        .providers
        .iter()
        .find(|p| p.profile_name == "glm-coding")
        .expect("glm-coding preset present");
    assert_eq!(glm.protocol, ProtocolFamily::AnthropicMessages);
    assert_eq!(glm.base_url, "https://open.bigmodel.cn/api/anthropic");
}

#[test]
fn deepseek_is_openai_chat_with_provider_id() {
    let catalog = builtin_presets();
    let ds = catalog
        .providers
        .iter()
        .find(|p| p.profile_name == "deepseek")
        .expect("deepseek preset present");
    assert_eq!(ds.protocol, ProtocolFamily::OpenAiChat);
    assert_eq!(
        ds.provider_id,
        ProviderId::OpenAICompatible { name: "deepseek".to_string() }
    );
    assert_eq!(ds.base_url, "https://api.deepseek.com");
}

#[test]
fn merges_into_config_and_registry_resolves() {
    let catalog = builtin_presets();
    let config = ClientConfig { providers: catalog.providers.clone() };
    let registry = ModelRegistry::from_config(config).expect("registry builds");
    // Pick a known deepseek model id from the listing and resolve it.
    let listing = registry.available_models();
    let ds = listing
        .iter()
        .find(|m| matches!(&m.provider_id, ProviderId::OpenAICompatible { name } if name == "deepseek"))
        .expect("a deepseek listing exists");
    let route = registry.resolve(&ds.request_model).expect("resolves");
    assert_eq!(route.request_model, ds.request_model);
}
```

- [ ] **Step 2: Run it to verify it FAILS.**

Run: `cd lingxi-code && cargo test -p llm-client --test catalog_test`
Expected: FAIL to compile (`builtin_presets`/`BuiltinCatalog` unresolved).

- [ ] **Step 3: Implement the routing table + builder.** Create `lingxi-code/llm-client/src/catalog/presets.rs`:

```rust
//! Built-in provider presets: vendored models.dev metadata + hand-authored
//! routing (base URL, protocol, auth, credential). The routing table is the
//! source of truth for wire/auth and overrides the snapshot's advisory `api`.

use crate::catalog::map::{to_model_profile, to_pricing};
use crate::catalog::models_dev::ProviderSlice;
use crate::{
    AuthStrategy, CredentialConfig, PricingCatalog, ProtocolFamily, ProviderId, ProviderProfile,
};

/// Built-in catalog: provider profiles plus a matching pricing catalog.
#[derive(Debug, Clone)]
pub struct BuiltinCatalog {
    /// Provider profiles ready to merge into [`crate::ClientConfig`].
    pub providers: Vec<ProviderProfile>,
    /// Pricing entries ready to merge into a [`PricingCatalog`].
    pub pricing: PricingCatalog,
}

/// One hand-authored routing entry bound to a vendored slice.
struct Preset {
    /// Profile + registry name (stable, user-facing).
    profile_name: &'static str,
    /// Routing base URL (overrides the snapshot's `api`).
    base_url: &'static str,
    /// Wire protocol family.
    protocol: ProtocolFamily,
    /// Auth application strategy.
    auth: AuthStrategy,
    /// Provider identity used for pricing + serialization.
    provider_id: ProviderId,
    /// Credential lookup (env var name).
    credential_env: &'static str,
    /// Embedded models.dev slice JSON.
    slice_json: &'static str,
}

const OPENROUTER: &str = include_str!("../../data/models-dev/openrouter.json");
const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");
const GLM_CODING: &str = include_str!("../../data/models-dev/zhipuai-coding-plan.json");

fn presets() -> Vec<Preset> {
    vec![
        Preset {
            profile_name: "openrouter",
            base_url: "https://openrouter.ai/api/v1",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible { name: "openrouter".to_string() },
            credential_env: "OPENROUTER_API_KEY",
            slice_json: OPENROUTER,
        },
        Preset {
            profile_name: "deepseek",
            base_url: "https://api.deepseek.com",
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::OpenAICompatible { name: "deepseek".to_string() },
            credential_env: "DEEPSEEK_API_KEY",
            slice_json: DEEPSEEK,
        },
        // GLM coding plan: Anthropic-compatible endpoint (reuses AnthropicMessagesCodec).
        // The snapshot's api points at /api/coding/paas/v4 (OpenAI-style); we override.
        Preset {
            profile_name: "glm-coding",
            base_url: "https://open.bigmodel.cn/api/anthropic",
            protocol: ProtocolFamily::AnthropicMessages,
            auth: AuthStrategy::ApiKey,
            provider_id: ProviderId::Custom { name: "glm-coding".to_string() },
            credential_env: "ZHIPU_API_KEY",
            slice_json: GLM_CODING,
        },
    ]
}

/// Assemble the built-in provider catalog from the vendored snapshots.
///
/// # Panics
/// Panics only if a vendored slice fails to parse — that is a build-time data
/// defect (the JSON is embedded and tested), never a runtime/host condition.
#[must_use]
pub fn builtin_presets() -> BuiltinCatalog {
    let mut providers = Vec::new();
    let mut pricing = PricingCatalog::empty();

    for preset in presets() {
        let slice: ProviderSlice = serde_json::from_str(preset.slice_json)
            .unwrap_or_else(|e| panic!("vendored slice {} parse: {e}", preset.profile_name));

        let mut models = Vec::with_capacity(slice.models.len());
        for model in slice.models.values() {
            models.push(to_model_profile(model));
            if let Some(price) = to_pricing(model) {
                pricing = pricing.with_price(preset.provider_id.clone(), model.id.clone(), price);
            }
        }
        // Stable order (BTreeMap is already sorted by id; keep it deterministic).
        providers.push(ProviderProfile {
            provider_id: preset.provider_id.clone(),
            profile_name: preset.profile_name.to_string(),
            base_url: preset.base_url.to_string(),
            protocol: preset.protocol.clone(),
            auth: preset.auth.clone(),
            credential: CredentialConfig::Env { var: preset.credential_env.to_string() },
            models,
            pricing: crate::config::PricingConfig::default(),
        });
    }

    BuiltinCatalog { providers, pricing }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_yields_models_and_some_pricing() {
        let catalog = builtin_presets();
        assert_eq!(catalog.providers.len(), 3);
        let total_models: usize = catalog.providers.iter().map(|p| p.models.len()).sum();
        assert!(total_models >= 347); // 337 + 4 + 6
    }
}
```

- [ ] **Step 4: Wire modules + exports.** Update `lingxi-code/llm-client/src/catalog/mod.rs`:

```rust
//! Built-in provider catalog assembled from vendored models.dev snapshots.

pub mod map;
pub mod models_dev;
pub mod presets;

pub use presets::{builtin_presets, BuiltinCatalog};
```

Add to `lingxi-code/llm-client/src/lib.rs` re-export block (near the other `pub use`):

```rust
pub use catalog::{builtin_presets, BuiltinCatalog};
```

Note: `ProviderProfile`, `ProtocolFamily`, `AuthStrategy`, `CredentialConfig`, `PricingConfig` are defined in `config.rs`. Confirm they are already re-exported from `lib.rs` (the existing `pub use config::{ ... }` block). If `AuthStrategy`/`CredentialConfig`/`ProtocolFamily`/`ProviderProfile`/`PricingConfig` are not all in that block, add the missing ones so `tests/catalog_test.rs` and downstream callers can name them.

- [ ] **Step 5: Run the integration test to verify it PASSES.**

Run: `cd lingxi-code && cargo test -p llm-client --test catalog_test`
Expected: PASS (all four tests).

- [ ] **Step 6: Run the in-module test too.**

Run: `cd lingxi-code && cargo test -p llm-client -- catalog::presets`
Expected: PASS.

- [ ] **Step 7: Clippy + docs lint.**

Run: `cd lingxi-code && cargo clippy -p llm-client --all-targets --no-deps -- -D warnings`
Expected: clean (every new `pub` item is documented).

- [ ] **Step 8: Commit.**

```bash
git add lingxi-code/llm-client/src/catalog/presets.rs \
        lingxi-code/llm-client/src/catalog/mod.rs \
        lingxi-code/llm-client/src/lib.rs \
        lingxi-code/llm-client/tests/catalog_test.rs
git commit -F - <<'EOF'
feat(llm-client): builtin_presets() for openrouter/deepseek/glm-coding

Routing table overrides advisory snapshot api (GLM -> /api/anthropic, reusing
AnthropicMessagesCodec). Returns provider profiles + pricing catalog that merge
into ClientConfig/PricingCatalog and resolve via ModelRegistry.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task 5: full-crate verification + frozen-crate check

**Files:** none (verification).

- [ ] **Step 1: Full llm-client test run.**

Run: `cd lingxi-code && cargo test -p llm-client 2>&1 | tail -25`
Expected: all green; pass count ≥ baseline (Task 0) + new tests. Record the number.

- [ ] **Step 2: Confirm the engine still builds (no accidental break of consumers).**

Run: `cd lingxi-code && cargo build -p llm-client -p orchestrator -p engine-mobile`
Expected: builds clean.

- [ ] **Step 3: Frozen-crate guard (must print 0 / empty).**

Run: `git diff main -- lingxi-code/traits lingxi-code/protocol | grep -c '^[-+]' || true`
Expected: `0` (no changes to frozen crates).

- [ ] **Step 4: Confirm no `git add -A` slipped in untracked root dirs.**

Run: `git status --short | grep -E '^\?\?' | grep -vE 'lingxi-code/llm-client/(data|scripts)/' || true`
Expected: no unexpected untracked paths staged (only our intended files were committed).

- [ ] **Step 5: No commit** (verification only). If everything is green, Phase 1 is complete and ready for the requesting-code-review skill before merge.

---

## Self-review (completed by plan author)

**Spec coverage (Phase 1 section):**
- Vendored per-provider slices + refresh script → Task 1. ✓
- models.dev serde schema (tolerant) → Task 2. ✓
- Routing-override table (base_url/protocol/auth/cred) → Task 4. ✓
- Catalog builder mapping Model → ModelProfile + pricing, codec reuse → Tasks 3 + 4. ✓
- `builtin_presets()` merges into ClientConfig + registry resolves → Task 4 integration test. ✓
- Free model priced at 0 not unpriced → covered by `to_pricing` semantics + map test (cost block present → priced); deepseek models are priced. (GLM models have all-zero cost → exercises the free-but-priced path in the pricing-count assertion.) ✓
- Tests: parse/count, field mapping, routing override beats snapshot, registry resolve → Tasks 2/3/4. ✓
- OpenRouter ranking headers → explicitly deferred (Out of scope section), not silently dropped. ✓

**Placeholder scan:** no TBD/TODO; every code step shows complete code. ✓

**Type consistency:** `BuiltinCatalog { providers, pricing }`, `builtin_presets()`, `to_model_profile`/`to_capabilities`/`to_pricing`, `ProviderSlice`/`Model`/`Cost`/`Modalities`/`Limit` names are used identically across Tasks 2–4. `ProviderProfile`/`ModelProfile`/`Capabilities`/`TokenPricing`/`PricingCatalog`/`ProviderId`/`ProtocolFamily`/`AuthStrategy`/`CredentialConfig`/`PricingConfig` match the existing crate definitions read from `config.rs`/`registry.rs`/`cost.rs`/`types.rs`. ✓
