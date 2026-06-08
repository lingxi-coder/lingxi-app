//! Pricing catalog: model references, token classes, per-token rates, and the
//! resolver that maps a [`ModelRef`] to a [`ModelPricing`] entry.
//!
//! Rates are stored in **nano-USD per token** so all arithmetic stays in
//! `u64`. Conversion convention:
//!
//! ```text
//! milli-USD per Mtok  ==  nano-USD per token   (10^-3 / 10^6 = 10^-9)
//! ```
//!
//! For example, "Opus 4.6 at $5 per million input tokens" is
//! `5_000` milli-USD per Mtok, which is also `5_000` nano-USD per token.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use thiserror::Error;

/// Fully qualified model identity: which provider, which model name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    /// The provider hosting this model.
    pub provider: ProviderId,
    /// The provider-scoped model name (e.g. `claude-opus-4-6`).
    pub model: String,
}

/// The set of model providers the cost system recognises.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    /// Anthropic first-party API.
    Anthropic,
    /// `OpenAI` first-party API.
    OpenAI,
    /// Google Gemini first-party API.
    GoogleGemini,
    /// Amazon Bedrock (Claude-on-Bedrock) — distinct list prices from the
    /// first-party Anthropic API; model ids are Bedrock-namespaced
    /// (e.g. `anthropic.claude-3-5-sonnet-20241022-v2:0`).
    AmazonBedrock,
    /// An `OpenAI`-compatible endpoint identified by name (e.g. `together`).
    OpenAICompatible {
        /// Display name for the `OpenAI`-compatible provider.
        name: String,
    },
    /// A custom provider identified by name.
    Custom {
        /// Display name for the custom provider.
        name: String,
    },
}

/// Classification of a billable token category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenClass {
    /// Input (prompt) tokens.
    Input,
    /// Output (completion) tokens.
    Output,
    /// Tokens written into the prompt cache.
    CacheWrite,
    /// Tokens read from the prompt cache (typically discounted).
    CacheRead,
    /// Reasoning / thinking output tokens (some providers bill separately).
    ReasoningOutput,
}

/// Non-token billable units (e.g. per-request server-side tool charges).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NonTokenBillableUnit {
    /// One server-side web search request.
    WebSearchRequest,
}

/// A unit price expressed in nano-USD per token.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoneyPerToken {
    /// Price per token in nano-USD (1 USD = 10^9 nano-USD).
    pub nano_usd_per_token: u64,
}

/// All rate information needed to cost one model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// The model these rates apply to.
    pub model_ref: ModelRef,
    /// Per-token rates keyed by [`TokenClass`].
    pub token_rates: HashMap<TokenClass, MoneyPerToken>,
    /// Per-unit rates for non-token billable units (nano-USD per unit).
    pub non_token_rates_nano_usd: HashMap<NonTokenBillableUnit, u64>,
    /// Optional effective-from timestamp for time-bounded rate sheets.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_from: Option<SystemTime>,
    /// Where this entry came from (audit trail).
    pub source: PricingSource,
}

/// Provenance for a [`ModelPricing`] entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PricingSource {
    /// Compiled-in reference rates for a known provider.
    BuiltInReference {
        /// The provider whose builtin sheet was used.
        provider: ProviderId,
    },
    /// A host-supplied override loaded from disk.
    HostOverride {
        /// Path to the override file that supplied this entry.
        path: PathBuf,
    },
    /// Pricing fetched from a remote managed-settings endpoint.
    RemoteManagedSettings,
}

/// How a lookup resolved against the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingResolution {
    /// The catalog held an exact entry for the requested model.
    ExactModel {
        /// The exact model that matched.
        model_ref: ModelRef,
    },
    /// No exact entry; a provider-level default was used.
    ProviderDefault {
        /// The originally requested model.
        requested: ModelRef,
    },
    /// Neither exact nor provider default; the model is unpriced.
    UnpricedModel {
        /// The originally requested model.
        requested: ModelRef,
    },
}

/// Errors emitted by the cost layer.
#[derive(Debug, Clone, Error)]
pub enum CostError {
    /// Lookup failed: no exact entry and no provider default for this model.
    #[error("unpriced model: {0:?}")]
    UnpricedModel(ModelRef),
    /// The model is not in the catalog and has no provider default.
    ///
    /// Display string is byte-for-byte locked against claude-code:
    /// `"Cost tracking unavailable for {model}"`. See spec §5 lines 496-497.
    #[error("Cost tracking unavailable for {model}")]
    UnknownModel {
        /// The model identifier that was not found.
        model: String,
    },
    /// The session's cumulative cost has crossed the configured budget limit.
    ///
    /// Display string is byte-for-byte locked against claude-code:
    /// `"Budget exceeded (${current:.2}); stopped."`. See spec §5 lines 498-500.
    /// `limit` and `current` are in USD (post-conversion from nano-USD); the
    /// dollar-formatted `current` uses
    /// [`nano_usd_to_dollars_format`](crate::nano_usd_to_dollars_format).
    #[error("Budget exceeded (${current:.2}); stopped.")]
    BudgetExceeded {
        /// The configured limit, in USD.
        limit: f64,
        /// The current cumulative cost, in USD.
        current: f64,
    },
}

/// Basis-points discount applied to cost when `is_batch_request = true`.
///
/// 5000 bps = 50% off. **M3 never applies this discount** because
/// `is_batch_request` is always `false` in M3 (the `/v1/messages/batches`
/// endpoint is M4). M4 will multiply `cost_nano_usd` by
/// `(10000 - BATCH_DISCOUNT_BPS) / 10000` when batches fire.
pub const BATCH_DISCOUNT_BPS: u32 = 5000;

/// Format a nano-USD amount as a 2-decimal dollar string, e.g.
/// `1_500_000_000` → `"$1.50"`.
///
/// Used by the budget-exceeded error string. `f64` lossiness on cents-precision
/// is acceptable here because this output is for **display only** and never
/// feeds back into accumulating arithmetic (cost storage stays `u64` per v3 §17).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn nano_usd_to_dollars_format(nano: u64) -> String {
    format!("${:.2}", (nano as f64) / 1_000_000_000.0)
}

/// Strip date/provider/ARN suffixes from a wire model string, yielding the
/// canonical short name the catalog is keyed on.
///
/// Direct port of claude-code's `firstPartyNameToCanonical`
/// (`utils/model/model.ts:217-270`): lowercases, then substring-matches the
/// known families (most-specific version first so `claude-opus-4-6` wins over
/// `claude-opus-4`), and finally applies the `/(claude-(\d+-\d+-)?\w+)/`
/// regex fallback. Because matching is by substring, ARN / inference-profile
/// ids resolve directly — e.g. both `claude-opus-4-6-20251101` and
/// `us.anthropic.claude-opus-4-6-v1:0` collapse to `claude-opus-4-6`. (The TS
/// `getCanonicalName` additionally runs `resolveOverriddenModel` to expand
/// custom Bedrock inference-profile ARNs back to a 1P id; that settings-driven
/// lookup lives outside the cost crate, but the standard ARN forms above are
/// already handled by the substring match here.)
#[must_use]
pub fn first_party_name_to_canonical(name: &str) -> String {
    let name = name.to_lowercase();
    // Claude 4+ — order matters: check more specific versions first.
    if name.contains("claude-opus-4-6") {
        return "claude-opus-4-6".into();
    }
    if name.contains("claude-opus-4-5") {
        return "claude-opus-4-5".into();
    }
    if name.contains("claude-opus-4-1") {
        return "claude-opus-4-1".into();
    }
    if name.contains("claude-opus-4") {
        return "claude-opus-4".into();
    }
    if name.contains("claude-sonnet-4-6") {
        return "claude-sonnet-4-6".into();
    }
    if name.contains("claude-sonnet-4-5") {
        return "claude-sonnet-4-5".into();
    }
    if name.contains("claude-sonnet-4") {
        return "claude-sonnet-4".into();
    }
    if name.contains("claude-haiku-4-5") {
        return "claude-haiku-4-5".into();
    }
    // Claude 3.x models use the claude-3-{family} scheme.
    if name.contains("claude-3-7-sonnet") {
        return "claude-3-7-sonnet".into();
    }
    if name.contains("claude-3-5-sonnet") {
        return "claude-3-5-sonnet".into();
    }
    if name.contains("claude-3-5-haiku") {
        return "claude-3-5-haiku".into();
    }
    if name.contains("claude-3-opus") {
        return "claude-3-opus".into();
    }
    if name.contains("claude-3-sonnet") {
        return "claude-3-sonnet".into();
    }
    if name.contains("claude-3-haiku") {
        return "claude-3-haiku".into();
    }
    // Regex fallback: /(claude-(\d+-\d+-)?\w+)/ — first `claude-` followed by an
    // optional `\d+-\d+-` group then a `\w+` ([A-Za-z0-9_]+) run.
    if let Some(m) = claude_regex_fallback(&name) {
        return m;
    }
    // No pattern matched — return the (lowercased) original, mirroring TS.
    name
}

/// Port of the `/(claude-(\d+-\d+-)?\w+)/` regex fallback used by
/// [`first_party_name_to_canonical`]. Returns the first match (with the
/// `claude-` prefix) or `None`. Implemented by hand to avoid a regex dep.
fn claude_regex_fallback(name: &str) -> Option<String> {
    const PREFIX: &str = "claude-";
    let start = name.find(PREFIX)?;
    let after = &name.as_bytes()[start + PREFIX.len()..];
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let word_run = |from: usize| {
        let mut k = from;
        while k < after.len() && is_word(after[k]) {
            k += 1;
        }
        k
    };
    // Greedily try the optional `\d+-\d+-` group first (regex prefers it).
    let group_end = {
        let mut j = 0;
        let s1 = j;
        while j < after.len() && after[j].is_ascii_digit() {
            j += 1;
        }
        if j > s1 && j < after.len() && after[j] == b'-' {
            j += 1;
            let s2 = j;
            while j < after.len() && after[j].is_ascii_digit() {
                j += 1;
            }
            if j > s2 && j < after.len() && after[j] == b'-' {
                j += 1;
                Some(j)
            } else {
                None
            }
        } else {
            None
        }
    };
    // `\w+` after the optional group; if it can't match, backtrack and try
    // `\w+` from position 0 (digits are word chars, so this still succeeds).
    let mut end = 0;
    if let Some(g) = group_end {
        let w = word_run(g);
        if w > g {
            end = w;
        }
    }
    if end == 0 {
        let w = word_run(0);
        if w == 0 {
            return None;
        }
        end = w;
    }
    std::str::from_utf8(&after[..end])
        .ok()
        .map(|s| format!("{PREFIX}{s}"))
}

/// Catalog mapping [`ModelRef`] to [`ModelPricing`], with provider-level
/// fallback for models without explicit entries.
pub struct PricingCatalog {
    entries: HashMap<ModelRef, ModelPricing>,
    provider_defaults: HashMap<ProviderId, ModelPricing>,
}

impl PricingCatalog {
    /// Construct an empty catalog with no entries and no provider defaults.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
            provider_defaults: HashMap::new(),
        }
    }

    /// Builtin reference rates covering current Anthropic tiers.
    ///
    /// All rates are nano-USD per token (see module docs for conversion).
    #[must_use]
    pub fn builtin_reference() -> Self {
        let mut c = Self::empty();
        // $3/$15 tier — Sonnet variants (COST_TIER_3_15, modelCost.ts:36-42).
        c.insert_anthropic("claude-sonnet-4-6", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-sonnet-4-5", 3_000, 15_000, 3_750, 300);
        // claude-sonnet-4 ($3/$15) — modelCost.ts:113-114.
        c.insert_anthropic("claude-sonnet-4", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-3-7-sonnet", 3_000, 15_000, 3_750, 300);
        // claude-3-5-sonnet ($3/$15) — modelCost.ts:109-110.
        c.insert_anthropic("claude-3-5-sonnet", 3_000, 15_000, 3_750, 300);
        // $5/$25 tier — Opus 4.5.
        c.insert_anthropic("claude-opus-4-5", 5_000, 25_000, 6_250, 500);
        // $5/$25 — Opus 4.6 standard.
        c.insert_anthropic("claude-opus-4-6", 5_000, 25_000, 6_250, 500);
        // $15/$75 — Opus 4 / 4.1 (COST_TIER_15_75, modelCost.ts:45-51).
        c.insert_anthropic("claude-opus-4-1", 15_000, 75_000, 18_750, 1_500);
        // claude-opus-4 ($15/$75) — modelCost.ts:119.
        c.insert_anthropic("claude-opus-4", 15_000, 75_000, 18_750, 1_500);
        // Haiku 4.5 — $1/$5.
        c.insert_anthropic("claude-haiku-4-5", 1_000, 5_000, 1_250, 100);
        // Haiku 3.5 — $0.80/$4.
        c.insert_anthropic("claude-3-5-haiku", 800, 4_000, 1_000, 80);
        // OpenAI reference tiers — approximate published list prices
        // (milli-USD per Mtok; cache_read = cached-input discount, cache_write
        // unused since OpenAI usage reports only cached read tokens).
        c.insert_priced(ProviderId::OpenAI, "gpt-4o", 2_500, 10_000, 2_500, 1_250);
        c.insert_priced(ProviderId::OpenAI, "gpt-4o-mini", 150, 600, 150, 75);
        c.insert_priced(ProviderId::OpenAI, "gpt-4.1", 2_000, 8_000, 2_000, 500);
        c.insert_priced(ProviderId::OpenAI, "gpt-4.1-mini", 400, 1_600, 400, 100);
        // Google Gemini reference tiers — approximate published list prices.
        c.insert_priced(
            ProviderId::GoogleGemini,
            "gemini-2.0-flash",
            100,
            400,
            100,
            25,
        );
        c.insert_priced(
            ProviderId::GoogleGemini,
            "gemini-1.5-pro",
            1_250,
            5_000,
            1_250,
            312,
        );
        c.insert_priced(
            ProviderId::GoogleGemini,
            "gemini-1.5-flash",
            75,
            300,
            75,
            18,
        );
        // Amazon Bedrock (Claude) — AWS published list prices (us-east-1),
        // keyed on the Bedrock model id (the part after `bedrock/`). Cache
        // rates mirror the equivalent Anthropic tier as a reference estimate.
        c.insert_priced(
            ProviderId::AmazonBedrock,
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            3_000,
            15_000,
            3_750,
            300,
        );
        c.insert_priced(
            ProviderId::AmazonBedrock,
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
            3_000,
            15_000,
            3_750,
            300,
        );
        c.insert_priced(
            ProviderId::AmazonBedrock,
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            800,
            4_000,
            1_000,
            80,
        );
        c.insert_priced(
            ProviderId::AmazonBedrock,
            "anthropic.claude-3-opus-20240229-v1:0",
            15_000,
            75_000,
            18_750,
            1_500,
        );
        c
    }

    fn insert_anthropic(
        &mut self,
        model: &str,
        input_per_mtok_milli_usd: u64,
        output_per_mtok_milli_usd: u64,
        cache_write_per_mtok_milli_usd: u64,
        cache_read_per_mtok_milli_usd: u64,
    ) {
        // milli-USD per Mtok = nano-USD per token (10^-3 / 10^6 = 10^-9).
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: model.into(),
        };
        let mut rates: HashMap<TokenClass, MoneyPerToken> = HashMap::new();
        rates.insert(
            TokenClass::Input,
            MoneyPerToken {
                nano_usd_per_token: input_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::Output,
            MoneyPerToken {
                nano_usd_per_token: output_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::CacheWrite,
            MoneyPerToken {
                nano_usd_per_token: cache_write_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::CacheRead,
            MoneyPerToken {
                nano_usd_per_token: cache_read_per_mtok_milli_usd,
            },
        );

        let mut non_token: HashMap<NonTokenBillableUnit, u64> = HashMap::new();
        non_token.insert(NonTokenBillableUnit::WebSearchRequest, 10_000_000); // $0.01/request

        self.entries.insert(
            mr.clone(),
            ModelPricing {
                model_ref: mr,
                token_rates: rates,
                non_token_rates_nano_usd: non_token,
                effective_from: None,
                source: PricingSource::BuiltInReference {
                    provider: ProviderId::Anthropic,
                },
            },
        );
    }

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
            MoneyPerToken {
                nano_usd_per_token: input_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::Output,
            MoneyPerToken {
                nano_usd_per_token: output_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::CacheWrite,
            MoneyPerToken {
                nano_usd_per_token: cache_write_per_mtok_milli_usd,
            },
        );
        rates.insert(
            TokenClass::CacheRead,
            MoneyPerToken {
                nano_usd_per_token: cache_read_per_mtok_milli_usd,
            },
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

    /// Resolve a [`ModelRef`] to its pricing entry.
    ///
    /// Lookup order:
    /// 1. Exact match on the full `(provider, model)` key (preserves
    ///    provider-namespaced entries such as Bedrock model ids).
    /// 2. Canonicalized match: strip date/provider/ARN suffixes via
    ///    [`first_party_name_to_canonical`] (mirrors TS `getCanonicalName`,
    ///    `model.ts:279-283`) and retry — first under the requested provider,
    ///    then under [`ProviderId::Anthropic`] for `claude-*` names (the TS
    ///    `MODEL_COSTS` table is provider-agnostic, so a Bedrock/3P wire id
    ///    like `us.anthropic.claude-opus-4-6-v1:0` still prices as the
    ///    first-party Claude tier).
    /// 3. Provider default if one is registered.
    ///
    /// Returns [`CostError::UnpricedModel`] when nothing matches.
    pub fn resolve(&self, mr: &ModelRef) -> Result<(ModelPricing, PricingResolution), CostError> {
        // 1. Exact match on the wire id.
        if let Some(p) = self.entries.get(mr) {
            return Ok((
                p.clone(),
                PricingResolution::ExactModel {
                    model_ref: mr.clone(),
                },
            ));
        }
        // 2. Canonicalize the wire id and retry.
        let canonical = first_party_name_to_canonical(&mr.model);
        if canonical != mr.model {
            let canon_ref = ModelRef {
                provider: mr.provider.clone(),
                model: canonical.clone(),
            };
            if let Some(p) = self.entries.get(&canon_ref) {
                return Ok((
                    p.clone(),
                    PricingResolution::ExactModel {
                        model_ref: canon_ref,
                    },
                ));
            }
            // Cross-provider fallback: `claude-*` canonical names live in the
            // Anthropic table regardless of the requesting provider.
            if canonical.starts_with("claude-") && mr.provider != ProviderId::Anthropic {
                let anthropic_ref = ModelRef {
                    provider: ProviderId::Anthropic,
                    model: canonical,
                };
                if let Some(p) = self.entries.get(&anthropic_ref) {
                    return Ok((
                        p.clone(),
                        PricingResolution::ExactModel {
                            model_ref: anthropic_ref,
                        },
                    ));
                }
            }
        }
        // 3. Provider default.
        if let Some(p) = self.provider_defaults.get(&mr.provider) {
            return Ok((
                p.clone(),
                PricingResolution::ProviderDefault {
                    requested: mr.clone(),
                },
            ));
        }
        Err(CostError::UnpricedModel(mr.clone()))
    }

    /// Pricing tier billed for a model with no catalog entry, so unknown
    /// models are never billed at zero.
    ///
    /// Mirrors claude-code's `DEFAULT_UNKNOWN_MODEL_COST = COST_TIER_5_25`
    /// (`utils/modelCost.ts:53-60,89`): on a `MODEL_COSTS` miss `getModelCosts`
    /// returns `MODEL_COSTS[canonical(defaultMainLoopModel)] ?? COST_TIER_5_25`
    /// (`modelCost.ts:155-163`). The default main-loop model is itself Opus 4.6
    /// ($5/$25), so both branches yield the same $5/$25 tier; we use the
    /// constant directly since settings access lives outside the cost crate.
    /// Rates are nano-USD per token (= milli-USD per Mtok); web search is
    /// $0.01/request.
    #[must_use]
    pub fn default_unknown_pricing(mr: &ModelRef) -> ModelPricing {
        let mut rates: HashMap<TokenClass, MoneyPerToken> = HashMap::new();
        rates.insert(
            TokenClass::Input,
            MoneyPerToken {
                nano_usd_per_token: 5_000,
            },
        );
        rates.insert(
            TokenClass::Output,
            MoneyPerToken {
                nano_usd_per_token: 25_000,
            },
        );
        rates.insert(
            TokenClass::CacheWrite,
            MoneyPerToken {
                nano_usd_per_token: 6_250,
            },
        );
        rates.insert(
            TokenClass::CacheRead,
            MoneyPerToken {
                nano_usd_per_token: 500,
            },
        );
        let mut non_token: HashMap<NonTokenBillableUnit, u64> = HashMap::new();
        non_token.insert(NonTokenBillableUnit::WebSearchRequest, 10_000_000);
        ModelPricing {
            model_ref: mr.clone(),
            token_rates: rates,
            non_token_rates_nano_usd: non_token,
            effective_from: None,
            source: PricingSource::BuiltInReference {
                provider: mr.provider.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_opus_4_6() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 5_000);
    }

    #[test]
    fn unpriced_model_errors() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-nonexistent".into(),
        };
        assert!(matches!(
            c.resolve(&mr).unwrap_err(),
            CostError::UnpricedModel(_)
        ));
    }

    #[test]
    fn bedrock_claude_sonnet_is_priced() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::AmazonBedrock,
            model: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 3_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            15_000
        );
    }

    #[test]
    fn builtin_has_openai_gpt_4o() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::OpenAI,
            model: "gpt-4o".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 2_500);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            10_000
        );
    }

    #[test]
    fn builtin_has_gemini_flash() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::GoogleGemini,
            model: "gemini-2.0-flash".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Output].nano_usd_per_token, 400);
    }

    #[test]
    fn unknown_openai_model_is_unpriced_not_misattributed() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::OpenAI,
            model: "gpt-9-ultra".into(),
        };
        // No exact entry and no OpenAI provider-default registered → UnpricedModel
        // (cost attributes to OpenAI but invents no rate).
        assert!(c.resolve(&mr).is_err());
    }

    // ----- COST.2: canonicalization (port of getCanonicalName) -----

    #[test]
    fn canonicalize_strips_date_suffix() {
        // claude-opus-4-6-20251101 -> claude-opus-4-6 (model.ts:221-223).
        assert_eq!(
            first_party_name_to_canonical("claude-opus-4-6-20251101"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn canonicalize_strips_provider_arn() {
        // us.anthropic.claude-opus-4-6-v1:0 -> claude-opus-4-6 via substring match.
        assert_eq!(
            first_party_name_to_canonical("us.anthropic.claude-opus-4-6-v1:0"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn canonicalize_orders_specific_before_general() {
        // The 4-6 / 4-5 / 4-1 checks must win over the bare claude-opus-4 check.
        assert_eq!(
            first_party_name_to_canonical("claude-opus-4-1-20250805"),
            "claude-opus-4-1"
        );
        assert_eq!(
            first_party_name_to_canonical("claude-opus-4-20250514"),
            "claude-opus-4"
        );
        assert_eq!(
            first_party_name_to_canonical("claude-sonnet-4-20250514"),
            "claude-sonnet-4"
        );
    }

    #[test]
    fn canonicalize_regex_fallback_and_passthrough() {
        // Falls through the explicit families to the /(claude-(\d+-\d+-)?\w+)/ rule.
        assert_eq!(
            first_party_name_to_canonical("claude-3-9-zephyr-20260101"),
            "claude-3-9-zephyr"
        );
        assert_eq!(
            first_party_name_to_canonical("claude-mystery-preview"),
            "claude-mystery"
        );
        // No `claude-` token at all → returned (lowercased) unchanged.
        assert_eq!(first_party_name_to_canonical("GPT-9-Ultra"), "gpt-9-ultra");
    }

    #[test]
    fn resolve_canonicalizes_date_suffixed_anthropic_id() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6-20251101".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        // Resolves to the claude-opus-4-6 $5/$25 tier.
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 5_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            25_000
        );
    }

    #[test]
    fn resolve_canonicalizes_arn_cross_provider() {
        let c = PricingCatalog::builtin_reference();
        // A Bedrock ARN with no exact entry falls through canonicalization to
        // the first-party Anthropic claude-opus-4-6 tier ($5/$25).
        let mr = ModelRef {
            provider: ProviderId::AmazonBedrock,
            model: "us.anthropic.claude-opus-4-6-v1:0".into(),
        };
        let (p, res) = c.resolve(&mr).unwrap();
        assert!(matches!(res, PricingResolution::ExactModel { .. }));
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 5_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            25_000
        );
    }

    // ----- COST.2: the three newly-added models -----

    #[test]
    fn builtin_has_claude_3_5_sonnet() {
        let c = PricingCatalog::builtin_reference();
        // Both the canonical name and a date-suffixed wire id price at $3/$15.
        for model in ["claude-3-5-sonnet", "claude-3-5-sonnet-20241022"] {
            let mr = ModelRef {
                provider: ProviderId::Anthropic,
                model: model.into(),
            };
            let (p, _) = c.resolve(&mr).unwrap();
            assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 3_000);
            assert_eq!(
                p.token_rates[&TokenClass::Output].nano_usd_per_token,
                15_000
            );
        }
    }

    #[test]
    fn builtin_has_claude_sonnet_4() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-sonnet-4".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 3_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            15_000
        );
    }

    #[test]
    fn builtin_has_claude_opus_4() {
        let c = PricingCatalog::builtin_reference();
        // claude-opus-4 prices at $15/$75 — and must NOT collide with the 4.5/4.6
        // $5/$25 tiers (ordering of the canonicalization checks).
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-20250514".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 15_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            75_000
        );
    }

    #[test]
    fn default_unknown_pricing_is_5_25_tier() {
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-future-unknown".into(),
        };
        let p = PricingCatalog::default_unknown_pricing(&mr);
        assert_eq!(p.token_rates[&TokenClass::Input].nano_usd_per_token, 5_000);
        assert_eq!(
            p.token_rates[&TokenClass::Output].nano_usd_per_token,
            25_000
        );
        assert_eq!(
            p.token_rates[&TokenClass::CacheWrite].nano_usd_per_token,
            6_250
        );
        assert_eq!(p.token_rates[&TokenClass::CacheRead].nano_usd_per_token, 500);
    }
}
