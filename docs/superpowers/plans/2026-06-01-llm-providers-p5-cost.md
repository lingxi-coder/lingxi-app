# LLM Providers — P5 (Cost / Pricing) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attribute cost correctly for OpenAI + Gemini turns — add their reference price tables to the cost catalog and make `cost_wiring` derive the `ProviderId` + local model id from the `provider/model` string (instead of hardcoding Anthropic), while leaving Anthropic cost byte-identical.

**Architecture:** Two small, low-risk changes. (1) `cost/src/pricing.rs`: a generic `insert_priced(provider, …)` helper + OpenAI/Gemini reference entries in `builtin_reference()` (Anthropic entries untouched). (2) `orchestrator/src/cost_wiring.rs`: `provider_from_model` + `model_ref_from_string` reuse `providers::ModelSpec::parse` to map the profile prefix → `cost::ProviderId` and strip the prefix off the model id (so it matches the new price-table keys). An unpriced model still degrades gracefully via the catalog's existing `UnpricedModel` path — cost is attributed to the right provider, never invented.

**Tech Stack:** Rust 1.82.0. `cost::pricing::{ProviderId, ModelRef, ModelPricing, PricingCatalog, TokenClass, MoneyPerToken, PricingSource}`, `providers::ModelSpec` (orchestrator already depends on `providers` from P2).

**Spec:** `docs/superpowers/specs/2026-06-01-llm-providers-design.md` (§8 cost/pricing, §12 P5).

**Conventions (read first):**
- Run all commands from `lingxi-code/`. Rust 1.82.0.
- Gate: `cargo clippy -p cost -p orchestrator --all-targets -- -D warnings` — NOTE `cost` lints cleanly (no tool-api in its graph); `orchestrator` is clippy-clean in its own source (the only `-D` blockers are the PRE-EXISTING `tool-api`-lib lints, already fixed in P2, so orchestrator clippy is clean). Every `pub` item needs a `///` doc; pedantic-clean.
- Do NOT modify `traits/` (frozen) or `api-client/`.

**Cost facts (confirmed):**
- `insert_anthropic(model, input, output, cache_write, cache_read)` — rates are **milli-USD per Mtok = nano-USD per token**. e.g. Sonnet `$3/$15` → `3_000 / 15_000`.
- `ModelPricing { model_ref, token_rates: HashMap<TokenClass, MoneyPerToken{nano_usd_per_token}>, non_token_rates_nano_usd, effective_from, source: PricingSource }`.
- `PricingCatalog.resolve(mr)` → exact entry, else provider default, else `Err(CostError::UnpricedModel)`. The cost tracker already tolerates the unpriced path.
- **Cost-time model string keeps the prefix:** `orchestrator/src/turn_loop.rs:58` calls `model_ref_from_string(&model)` where `model` is the session model (`s.model`) — the full `provider/model` string (the registry strips the prefix only when building a `CanonicalRequest`, not the session model). So `ModelSpec::parse` recovers the profile + local id at cost time.
- **Anthropic back-compat:** `ModelSpec::parse("claude-opus-4-7")` → `{profile:"anthropic", model:"claude-opus-4-7"}` (claude-* keeps the full string as the model), so `ModelRef{Anthropic, "claude-opus-4-7"}` still matches the existing Anthropic price entry. Identical for `anthropic/claude-…`.

---

## File Structure
- Modify: `cost/src/pricing.rs` — add `insert_priced` + OpenAI/Gemini entries + tests.
- Modify: `orchestrator/src/cost_wiring.rs` — prefix-aware `provider_from_model` / `model_ref_from_string` + tests.

---

## Task 1: OpenAI + Gemini reference price tables

**Files:**
- Modify: `lingxi-code/cost/src/pricing.rs`

- [ ] **Step 1: Add a generic `insert_priced` helper**

In `lingxi-code/cost/src/pricing.rs`, immediately after the existing `insert_anthropic` method (it ends with the `self.entries.insert(...)` block closing around line 277), add this generic helper inside the same `impl PricingCatalog` block:

```rust
    /// Insert a priced model entry for any provider. Unlike
    /// [`Self::insert_anthropic`], this adds no Anthropic-specific
    /// non-token (web-search) rate — `OpenAI` / `Gemini` bill only tokens in
    /// v1. Rates are milli-USD per Mtok (= nano-USD per token).
    fn insert_priced(
        &mut self,
        provider: ProviderId,
        model: &str,
        input_per_mtok_milli_usd: u64,
        output_per_mtok_milli_usd: u64,
        cache_write_per_mtok_milli_usd: u64,
        cache_read_per_mtok_milli_usd: u64,
    ) {
        let mr = ModelRef {
            provider: provider.clone(),
            model: model.into(),
        };
        let mut rates: HashMap<TokenClass, MoneyPerToken> = HashMap::new();
        rates.insert(
            TokenClass::Input,
            MoneyPerToken { nano_usd_per_token: input_per_mtok_milli_usd },
        );
        rates.insert(
            TokenClass::Output,
            MoneyPerToken { nano_usd_per_token: output_per_mtok_milli_usd },
        );
        rates.insert(
            TokenClass::CacheWrite,
            MoneyPerToken { nano_usd_per_token: cache_write_per_mtok_milli_usd },
        );
        rates.insert(
            TokenClass::CacheRead,
            MoneyPerToken { nano_usd_per_token: cache_read_per_mtok_milli_usd },
        );
        self.entries.insert(
            mr.clone(),
            ModelPricing {
                model_ref: mr,
                token_rates: rates,
                non_token_rates_nano_usd: HashMap::new(),
                effective_from: None,
                source: PricingSource::BuiltInReference { provider },
            },
        );
    }
```

- [ ] **Step 2: Add OpenAI + Gemini entries to `builtin_reference`**

In `builtin_reference()`, right before the final `c` (after the last `c.insert_anthropic(...)` line), add:

```rust
        // OpenAI reference tiers — approximate published list prices
        // (milli-USD per Mtok; cache_read = cached-input discount, cache_write
        // unused since OpenAI usage reports only cached read tokens).
        c.insert_priced(ProviderId::OpenAI, "gpt-4o", 2_500, 10_000, 2_500, 1_250);
        c.insert_priced(ProviderId::OpenAI, "gpt-4o-mini", 150, 600, 150, 75);
        c.insert_priced(ProviderId::OpenAI, "gpt-4.1", 2_000, 8_000, 2_000, 500);
        c.insert_priced(ProviderId::OpenAI, "gpt-4.1-mini", 400, 1_600, 400, 100);
        // Google Gemini reference tiers — approximate published list prices.
        c.insert_priced(ProviderId::GoogleGemini, "gemini-2.0-flash", 100, 400, 100, 25);
        c.insert_priced(ProviderId::GoogleGemini, "gemini-1.5-pro", 1_250, 5_000, 1_250, 312);
        c.insert_priced(ProviderId::GoogleGemini, "gemini-1.5-flash", 75, 300, 75, 18);
```

- [ ] **Step 3: Add tests**

In `pricing.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn builtin_has_openai_gpt_4o() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::OpenAI, model: "gpt-4o".into() };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 2_500);
        assert_eq!(p.token_rates[&TokenClass::Output].nano_usd_per_token, 10_000);
    }

    #[test]
    fn builtin_has_gemini_flash() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::GoogleGemini, model: "gemini-2.0-flash".into() };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Output].nano_usd_per_token, 400);
    }

    #[test]
    fn unknown_openai_model_is_unpriced_not_misattributed() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef { provider: ProviderId::OpenAI, model: "gpt-9-ultra".into() };
        // No exact entry and no OpenAI provider-default registered → UnpricedModel
        // (cost attributes to OpenAI but invents no rate).
        assert!(c.resolve(&mr).is_err());
    }
```

- [ ] **Step 4: Test + lint + commit**

Run: `cargo test -p cost` → all pass (existing Anthropic tests + the 3 new). If a pre-existing test asserts an exact catalog entry count, update the count to include the 7 new entries (otherwise no change).
Run: `cargo clippy -p cost --all-targets -- -D warnings` → clean.
```bash
git add cost/src/pricing.rs
git commit -m "feat(llm-p5): OpenAI + Gemini reference price tables"
```

---

## Task 2: Prefix-aware cost attribution

**Files:**
- Modify: `lingxi-code/orchestrator/src/cost_wiring.rs`

- [ ] **Step 1: Replace `provider_from_model` + `model_ref_from_string`**

In `lingxi-code/orchestrator/src/cost_wiring.rs`, add the `providers::ModelSpec` import at the top (next to the other `use` lines):

```rust
use providers::ModelSpec;
```

Replace the existing `provider_from_model` and `model_ref_from_string` functions (and their doc comments) with:

```rust
/// Map a provider-profile name to its cost [`ProviderId`].
///
/// Mirrors the registry's choice: built-in `anthropic`/`openai`/`gemini` map
/// to their first-party ids; any other (settings-declared) profile name is an
/// `OpenAI`-compatible endpoint.
#[must_use]
fn provider_id_for_profile(profile: &str) -> ProviderId {
    match profile {
        "anthropic" => ProviderId::Anthropic,
        "openai" => ProviderId::OpenAI,
        "gemini" => ProviderId::GoogleGemini,
        other => ProviderId::OpenAICompatible { name: other.to_string() },
    }
}

/// Resolve a model-name string to its `ProviderId` by parsing the
/// `provider/model` prefix (bare / `claude-*` → Anthropic, for back-compat).
#[must_use]
pub(crate) fn provider_from_model(model: &str) -> ProviderId {
    provider_id_for_profile(&ModelSpec::parse(model).profile)
}

/// Build a fully-qualified [`ModelRef`] from a model string: the prefix selects
/// the provider, and the local model id (prefix stripped) is what the price
/// catalog is keyed on. `claude-*` / bare strings keep the full string as the
/// model id, so Anthropic cost attribution is byte-identical to before.
#[must_use]
pub(crate) fn model_ref_from_string(model: &str) -> ModelRef {
    let spec = ModelSpec::parse(model);
    ModelRef {
        provider: provider_id_for_profile(&spec.profile),
        model: spec.model,
    }
}
```

- [ ] **Step 2: Replace the now-stale tests**

In `cost_wiring.rs`'s `#[cfg(test)] mod tests`, REPLACE `provider_always_anthropic_in_v070` and `model_ref_carries_provider_and_string` with:

```rust
    #[test]
    fn provider_from_model_maps_prefixes() {
        assert_eq!(provider_from_model("claude-opus-4-7"), ProviderId::Anthropic);
        assert_eq!(provider_from_model("anthropic/claude-opus-4-7"), ProviderId::Anthropic);
        assert_eq!(provider_from_model("openai/gpt-4o"), ProviderId::OpenAI);
        assert_eq!(provider_from_model("gemini/gemini-2.0-flash"), ProviderId::GoogleGemini);
        assert_eq!(provider_from_model("some-bare-model"), ProviderId::Anthropic);
        assert_eq!(
            provider_from_model("groq/llama-3.3-70b"),
            ProviderId::OpenAICompatible { name: "groq".to_string() }
        );
    }

    #[test]
    fn model_ref_strips_prefix_for_priced_lookup() {
        // Prefixed → provider + stripped local id (matches price-table keys).
        let mr = model_ref_from_string("openai/gpt-4o");
        assert_eq!(mr.provider, ProviderId::OpenAI);
        assert_eq!(mr.model, "gpt-4o");
        // Anthropic back-compat: full string kept as the model id.
        let mr = model_ref_from_string("claude-opus-4-7");
        assert_eq!(mr.provider, ProviderId::Anthropic);
        assert_eq!(mr.model, "claude-opus-4-7");
    }
```

(Keep the `translates_tokens_one_to_one` test unchanged.)

- [ ] **Step 3: Test + lint + commit**

Run: `cargo test -p orchestrator cost_wiring` → all pass.
Run: `cargo test -p orchestrator` → ALL pass (back-compat: nothing else broke).
Run: `cargo build -p orchestrator --tests 2>&1 | grep -i "warning.*cost_wiring"` → empty.
```bash
git add orchestrator/src/cost_wiring.rs
git commit -m "feat(llm-p5): prefix-aware cost attribution (provider/model -> ProviderId + local id)"
```

NOTE: `cargo clippy -p orchestrator --all-targets -- -D warnings` should be clean now that the tool-api lib lints were fixed in P2; if it surfaces only `tool-api`-test or other pre-existing `tool-*` lint debt, that's out of scope — confirm no diagnostics in `cost_wiring.rs` via the build-warning check above.

---

## Task 3: Back-compat gate + finalize

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt -p cost -p orchestrator`
Run: `git status --porcelain | grep -v test_entry` — if changed, `git add` the fmt'd files + `git commit -m "style(llm-p5): cargo fmt"`.

- [ ] **Step 2: Lint + tests**

Run: `cargo clippy -p cost --all-targets -- -D warnings` → clean.
Run: `cargo test -p cost` → all pass.
Run: `cargo test -p orchestrator` → all pass (incl. `cost_wiring` + the existing cost-recording turn-loop tests).

- [ ] **Step 3: Back-compat gate**

Run: `cargo test -p test-harness 2>&1 | grep -E "FAILED|error\[|test result:" | grep -v "0 failed" | head` → empty (Anthropic/OpenAI/Gemini parity + the cost-event parity suites unchanged).
Run: `cargo build --workspace` → Finished.
Run: `bash scripts/check-deps.sh` → `check-deps: OK — 73 workspace crates, no §8.1 dependency violations`.

- [ ] **Step 4: Tag (local only, no push)**

```bash
git tag -a llm-p5 -m "LLM Providers P5 (cost): OpenAI + Gemini reference price tables + prefix-aware cost_wiring (ProviderId + stripped model id). Anthropic cost byte-identical."
git --no-pager tag -l "llm-p5"
```

---

## Self-Review (completed during planning)

**1. Spec coverage (§8 / §12 P5):**
- OpenAI + Gemini price tables in the cost catalog → Task 1.
- Prefix-aware `model_ref_from_string` (provider prefix → `ProviderId`; local model id for table lookup) → Task 2.
- Unknown model → unpriced under the correct provider (catalog returns `UnpricedModel`, no invented rate) → Task 1 Step 3 test + the catalog's existing fallback.
- Per-provider cache-token mapping: already handled — `usage_api_to_cost_usage` maps `cache_read_input_tokens`→`cache_read` (OpenAI/Gemini populate this on decode); the new tables carry per-provider `CacheRead` rates.
- Anthropic cost byte-identical (claude-*/bare → `{Anthropic, full-string}`, matches existing entries; Anthropic table untouched) → Task 2 back-compat test + Task 3 gate.
- *Deferred (correctly):* streaming-path cost (streaming isn't wired into `init.rs`; `new` is batched) and custom-profile-kind precision (a custom name maps to `OpenAICompatible{name}`, which has no price table → unpriced-but-attributed; acceptable). Exact list-price precision is non-critical (reference rates).

**2. Placeholder scan:** none — every code step is complete; every run step has an exact command + expected result. Prices are explicit reference values (flagged "approximate published list prices").

**3. Type consistency:** `insert_priced(provider, model, in, out, cw, cr)` mirrors `insert_anthropic`'s signature (minus the Anthropic-only web-search rate); `provider_id_for_profile(&str)->ProviderId` is shared by `provider_from_model` + `model_ref_from_string`; `ModelSpec::parse(&str)->ModelSpec{profile, model}` is P1/P2's existing API. `ProviderId::OpenAICompatible{name}` is the cost-crate variant the registry already uses for custom profiles, keeping cost attribution consistent with the live provider.
