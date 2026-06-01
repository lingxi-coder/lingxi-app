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
        // $3/$15 tier — Sonnet variants.
        c.insert_anthropic("claude-sonnet-4-6", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-sonnet-4-5", 3_000, 15_000, 3_750, 300);
        c.insert_anthropic("claude-3-7-sonnet", 3_000, 15_000, 3_750, 300);
        // $5/$25 tier — Opus 4.5.
        c.insert_anthropic("claude-opus-4-5", 5_000, 25_000, 6_250, 500);
        // $5/$25 — Opus 4.6 standard.
        c.insert_anthropic("claude-opus-4-6", 5_000, 25_000, 6_250, 500);
        // $15/$75 — Opus 4 / 4.1.
        c.insert_anthropic("claude-opus-4-1", 15_000, 75_000, 18_750, 1_500);
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
        c.insert_priced(ProviderId::GoogleGemini, "gemini-2.0-flash", 100, 400, 100, 25);
        c.insert_priced(ProviderId::GoogleGemini, "gemini-1.5-pro", 1_250, 5_000, 1_250, 312);
        c.insert_priced(ProviderId::GoogleGemini, "gemini-1.5-flash", 75, 300, 75, 18);
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

    /// Resolve a [`ModelRef`] to its pricing entry.
    ///
    /// Prefers exact-model lookup, then falls back to a provider default if
    /// one is registered. Returns [`CostError::UnpricedModel`] when neither
    /// matches.
    pub fn resolve(&self, mr: &ModelRef) -> Result<(ModelPricing, PricingResolution), CostError> {
        if let Some(p) = self.entries.get(mr) {
            return Ok((
                p.clone(),
                PricingResolution::ExactModel {
                    model_ref: mr.clone(),
                },
            ));
        }
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
}
