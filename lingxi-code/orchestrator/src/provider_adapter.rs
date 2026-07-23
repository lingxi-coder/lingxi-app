//! Thin orchestrator-seam adapter over [`llm_client::ApiService`].
//!
//! The provider drive loop (prepare → header injection → `transport.execute()` /
//! `open_stream()` → `codec.decode_response()` + the retry/rate-limit/betas
//! machinery) lives in `llm_client::ApiService`. This module holds an
//! `Arc<ApiService>` and implements the orchestrator's seam traits
//! ([`OrchestratorApiClient`], [`StreamingApiClient`], [`agent::SubagentApiClient`])
//! by delegating each method 1:1 — the only orchestrator-domain logic kept here is
//! the catalog→`ModelListing` projection and the `RateLimitSnapshot` mapping.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use crate::model::rate_limit::{RateLimitInfo, RawUtilization};
use async_trait::async_trait;
use futures::stream::BoxStream;
use llm_client::{LlmError, LlmEvent, LlmResponse};
use protocol::ConversationMessage;
use std::sync::Arc;

/// Subscriber-state seed for [`llm_client::ApiService`]'s 429 gate. Re-exported
/// from llm-client so existing `orchestrator::provider_adapter::SubscriberState`
/// import paths (the composition roots) keep resolving after the drive loop moved.
pub use llm_client::SubscriberState;

/// Production adapter: a thin holder of [`llm_client::ApiService`] (which owns the
/// provider drive loop) that implements the orchestrator's seam traits.
pub struct ProviderApiAdapter {
    service: Arc<llm_client::ApiService>,
    /// (M4 cc2.1.198) The session's initial effort level from CLI `--effort`
    /// (binary: session state `thinkingConfig: SF(a.effort)` → request
    /// `output_config.effort`). `None` (the default) keeps main-loop request
    /// bodies byte-identical to before this field existed.
    initial_effort: std::sync::RwLock<Option<serde_json::Value>>,
    /// (`/fast`) Session-scoped fast-mode toggle, shared (same `Arc`) with the
    /// [`ConversationOrchestrator`] so the handle's `set_fast_mode` flip is seen
    /// here on the next turn. When set AND the active model supports fast mode
    /// (opus-4-7 / opus-4-8), the MAIN-loop stream sends `speed:"fast"`. The
    /// default flag is always `false`, so bodies stay byte-identical until a
    /// live `/fast` toggle flips it.
    fast_mode: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Whether `model` is a fast-mode-capable Opus tier (mirrors the
/// `supports_fast_mode` column of `run.rs::model_capabilities`).
fn model_supports_fast_mode(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("opus-4-7") || m.contains("opus-4-8")
}

impl ProviderApiAdapter {
    /// Wrap a constructed [`llm_client::ApiService`]. The composition roots build
    /// the service via `ApiService::new_with_routing(...).with_*(...)` and hand the
    /// `Arc` here; every trait method delegates 1:1 to it.
    #[must_use]
    pub fn new(service: Arc<llm_client::ApiService>) -> Self {
        Self {
            service,
            initial_effort: std::sync::RwLock::new(None),
            fast_mode: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// (`/fast`) Share the session's fast-mode flag with this adapter (the same
    /// `Arc<AtomicBool>` the [`ConversationOrchestrator`] holds). When the flag
    /// is set and the active model supports fast mode, the MAIN-loop stream
    /// carries `speed:"fast"`. Without this the flag is a private always-`false`
    /// default, so bodies are byte-identical.
    #[must_use]
    pub fn with_fast_mode(mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.fast_mode = flag;
        self
    }

    /// (M4 cc2.1.198) Set the session's initial effort (CLI `--effort`,
    /// already validated/normalized by the CLI to one of
    /// low/medium/high/xhigh/max). The MAIN-loop [`StreamingApiClient::stream`]
    /// impl then carries it as `output_config.effort` (the service adds the
    /// `effort-2025-11-24` beta whenever the body has effort). Subagent calls
    /// keep their own per-spawn effort resolution and are unaffected.
    #[must_use]
    pub fn with_initial_effort(mut self, effort: Option<serde_json::Value>) -> Self {
        self.initial_effort = std::sync::RwLock::new(effort);
        self
    }

    fn current_effort(&self) -> Option<serde_json::Value> {
        self.initial_effort
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Return the most recently observed rate-limit header snapshot (the internal
    /// nine-field [`RateLimitInfo`]). Delegates to the service. Kept as an inherent
    /// method so both the `OrchestratorApiClient::last_rate_limit_info` (three-field
    /// projection) and `last_rate_limit_full` (full snapshot) trait overrides can
    /// reach it.
    #[must_use]
    pub fn last_rate_limit_info(&self) -> Option<RateLimitInfo> {
        self.service.last_rate_limit_info()
    }
}
// ── Trait implementations ─────────────────────────────────────────────────────

#[async_trait]
impl tool_api::McpTokenCounter for ProviderApiAdapter {
    async fn count_mcp_content_tokens(
        &self,
        model: &str,
        content: &serde_json::Value,
    ) -> Result<Option<u64>, String> {
        let blocks = match content {
            serde_json::Value::String(text) => {
                vec![protocol::ContentBlock::Text { text: text.clone() }]
            }
            serde_json::Value::Array(values) => values
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(serde_json::Value::as_str) == Some("text")).then(
                        || protocol::ContentBlock::Text {
                            text: block
                                .get("text")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        },
                    )
                })
                .collect(),
            _ => return Ok(None),
        };
        if blocks.is_empty() {
            return Ok(None);
        }
        let message = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: blocks,
            is_meta: false,
        };
        self.service
            .count_tokens_exact(model, None, None, vec![message], Vec::new())
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    fn active_betas(&self) -> Vec<String> {
        self.service.active_custom_betas().to_vec()
    }

    fn set_thinking_config(&self, thinking: llm_client::model::thinking::ThinkingConfig) {
        self.service.set_thinking(thinking);
    }

    fn set_effort(&self, effort: Option<serde_json::Value>) {
        *self
            .initial_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effort;
    }

    async fn messages_create(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create(model, profile, system, msgs, tools)
            .await
    }

    async fn count_tokens(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<u64, LlmError> {
        self.service
            .count_tokens(model, profile, system, msgs, tools)
            .await
    }

    async fn count_tokens_exact(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Option<u64>, LlmError> {
        self.service
            .count_tokens_exact(model, profile, system, msgs, tools)
            .await
    }

    async fn messages_create_with_opts(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        max_tokens: u32,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create_with_opts(model, profile, system, msgs, tools, max_tokens)
            .await
    }

    async fn messages_create_with_fallback(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        fallback_model: Option<&str>,
        is_subscriber: bool,
        is_enterprise: bool,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create_with_fallback(
                model,
                profile,
                system,
                msgs,
                tools,
                fallback_model,
                is_subscriber,
                is_enterprise,
            )
            .await
    }

    async fn messages_create_seeded(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create_seeded(
                model,
                profile,
                system,
                msgs,
                tools,
                initial_consecutive_overloaded,
            )
            .await
    }

    fn available_models(&self) -> Vec<String> {
        self.service.available_models()
    }

    fn list_model_listings(&self) -> Vec<traits::orchestrator::ModelListing> {
        catalog_model_listings()
    }

    /// Return the most recently observed rate-limit header snapshot.
    ///
    /// Delegates to [`Self::last_rate_limit_info`] and maps the internal
    /// `RateLimitInfo` struct into the public [`traits::RateLimitSnapshot`]
    /// (all three fields: `rate_limit_type`, `overage_status`, and
    /// `overage_disabled_reason`).
    fn last_rate_limit_info(&self) -> Option<traits::RateLimitSnapshot> {
        self.last_rate_limit_info()
            .map(|info| traits::RateLimitSnapshot {
                rate_limit_type: info.rate_limit_type,
                overage_status: info.overage_status,
                overage_disabled_reason: info.overage_disabled_reason,
            })
    }

    fn last_request_id(&self) -> Option<String> {
        self.service.last_request_id()
    }

    fn last_retry_count(&self) -> u32 {
        self.service.last_retry_count()
    }

    /// Task 8 (llm-client future-work batch 3): expose the FULL internal
    /// nine-field snapshot for the turn drivers' `emit_rate_limit` seam.
    /// Delegates to the inherent [`Self::last_rate_limit_info`] (which
    /// already returns the internal `RateLimitInfo`); the trait method of
    /// the same name above keeps its three-field projection untouched.
    fn last_rate_limit_full(&self) -> Option<RateLimitInfo> {
        self.last_rate_limit_info()
    }

    /// Task 2 (llm-client future-work batch 5): expose the raw per-window
    /// snapshot cached by `record_rate_limit_from_headers` for the turn
    /// drivers' `emit_raw_utilization` seam.
    fn last_raw_utilization(&self) -> Option<RawUtilization> {
        self.service.last_raw_utilization()
    }

    /// Task 6 (llm-client future-work batch 5): expose the limits copy
    /// composed by `record_rate_limit_from_429` from the most recent
    /// 429 error response's unified headers, for the orchestrator's
    /// terminal-429 re-map (claude-code `errors.ts:480-524`).
    fn last_rate_limit_error_message(&self) -> Option<String> {
        self.service.last_rate_limit_error_message()
    }

    async fn prewarm_responses_websocket(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<(), LlmError> {
        self.service
            .prewarm_responses_websocket(model, profile, system, messages, tools)
            .await
    }

    async fn close_responses_websocket_session(&self) -> Result<(), LlmError> {
        self.service.close_responses_websocket_session().await
    }
}

/// Build the grouped-picker listing for EVERY provider.
///
/// First-party Anthropic is NOT part of the models.dev preset catalog
/// (`builtin_presets`); in a live session it enters via
/// `anthropic_provider_profile`. We surface it here too so this seam is
/// provider-COMPLETE on its own — it previously omitted Anthropic, which is why
/// `list_available_models` had to carry a Claude-only hardcoded fallback. With
/// Anthropic in the catalog that special case is gone. Anthropic is listed first
/// to keep the picker's Claude-first ordering for catalog-only (no live config)
/// callers.
fn catalog_model_listings() -> Vec<traits::orchestrator::ModelListing> {
    // Build one listing, defaulting an absent description to a known per-model
    // parity blurb for the Claude family (models.dev / the Anthropic profiles
    // carry no such string).
    let row = |display_model: String,
               request_model: String,
               label: String,
               provider: String,
               description: Option<String>,
               supports_reasoning: bool| {
        let description =
            description.or_else(|| model_description(&request_model).map(str::to_string));
        traits::orchestrator::ModelListing {
            display_model,
            request_model,
            provider_label: label,
            provider_id: provider,
            description,
            supports_reasoning,
        }
    };

    // 1. First-party Anthropic (not in the preset catalog).
    let mut listings: Vec<traits::orchestrator::ModelListing> =
        llm_client::anthropic_model_profiles()
            .into_iter()
            .map(|m| {
                let supports_reasoning = m.capabilities.reasoning;
                row(
                    m.display_model,
                    m.request_model,
                    provider_label("anthropic").to_string(),
                    "anthropic".to_string(),
                    m.description,
                    supports_reasoning,
                )
            })
            .collect();

    // 2. Static models.dev presets (OpenAI, Gemini, DeepSeek, …). EXCLUDE models
    //    that don't support tool calls (`tool_call=false` — image/TTS/audio
    //    models, gpt-3.5-turbo, gpt-5-chat-latest, the aion-labs set, ~85
    //    OpenRouter passthroughs). The agent sends tools on EVERY turn, so such a
    //    model can never complete an agentic turn — it hard-fails "unsupported
    //    capability: tools" (llm-client `validate_capabilities`). Offering it in
    //    the `/model` picker is offering a permanently-broken pick; it stays
    //    resolvable by explicit id for any non-agentic caller.
    let catalog = llm_client::builtin_presets();
    if let Ok(registry) = llm_client::ModelRegistry::from_config(llm_client::ClientConfig {
        providers: catalog.providers,
    }) {
        listings.extend(
            registry
                .available_models()
                .into_iter()
                .filter(|m| m.capabilities.tools)
                .map(|m| {
                    let label = provider_label(&m.profile_name).to_string();
                    let supports_reasoning = m.capabilities.reasoning;
                    row(
                        m.display_model,
                        m.request_model,
                        label,
                        m.profile_name,
                        m.description,
                        supports_reasoning,
                    )
                }),
        );
    }
    listings
}

/// Cached `request_model -> display_model` map over the full static catalog
/// (Anthropic first-party + models.dev presets), built once on first use.
fn catalog_display_names() -> &'static std::collections::HashMap<String, String> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<String, String>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        catalog_model_listings()
            .into_iter()
            .map(|m| (m.request_model, m.display_model))
            .collect()
    })
}

/// The catalog display name for `request_model` (e.g. `deepseek-v4-pro` ->
/// `"DeepSeek V4 Pro"`), or `None` for an id not in the catalog.
///
/// Feeds the `<env>` identity line's marketing-name slot for NON-Claude models
/// — Claude ids resolve their name via [`crate::prompt::env_meta::
/// marketing_name_for_model`] (byte-parity), and only when THAT returns `None`
/// (a non-Claude model) does the builder fall back to this so the line reads
/// the strong "powered by the model named {name}" form instead of the weak
/// id-only fallback. Cached, so it is cheap to call per turn.
#[must_use]
pub(crate) fn display_name_for_model(request_model: &str) -> Option<String> {
    catalog_display_names().get(request_model).cloned()
}

/// Cached `request_model -> UNIQUE provider profile`. The value is `None` when
/// the same wire id is served by MORE than one provider (ambiguous), so callers
/// don't guess. Built once from the full catalog.
fn catalog_provider_profiles() -> &'static std::collections::HashMap<String, Option<String>> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<String, Option<String>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        for l in catalog_model_listings() {
            let request_model = l.request_model;
            let provider = l.provider_id;
            match map.get_mut(&request_model) {
                // Already seen under a DIFFERENT provider → ambiguous.
                Some(existing) => {
                    if existing.as_deref() != Some(provider.as_str()) {
                        *existing = None;
                    }
                }
                None => {
                    map.insert(request_model, Some(provider));
                }
            }
        }
        map
    })
}

/// The UNIQUE provider profile that serves `request_model` in the catalog, or
/// `None` when the id is unknown OR served by more than one provider. Lets
/// cost/telemetry attribute a bare wire id whose live session profile is unknown
/// (e.g. after a cross-provider `--resume` clears it) to its REAL provider
/// instead of blindly defaulting to Anthropic.
#[must_use]
pub(crate) fn provider_for_model(request_model: &str) -> Option<String> {
    catalog_provider_profiles()
        .get(request_model)
        .cloned()
        .flatten()
}

/// Known one-line description for a built-in model wire id, mirroring the
/// claude-code `/model` picker blurbs (`modelOptions.ts`). Matched by a
/// case-insensitive family substring so dated ids (`claude-opus-4-7`, etc.) and
/// short aliases (`opus`, `sonnet`, `haiku`) both resolve. Returns `None` for
/// any id we don't recognize (then the row renders with no sub-line).
#[must_use]
pub(crate) fn model_description(request_model: &str) -> Option<&'static str> {
    let id = request_model.to_ascii_lowercase();
    if id.contains("opus") {
        Some("Best for everyday, complex tasks")
    } else if id.contains("haiku") {
        Some("Fastest for quick answers")
    } else if id.contains("sonnet") {
        Some("Efficient for routine tasks")
    } else {
        None
    }
}

/// Human provider header for a catalog profile name.
fn provider_label(profile_name: &str) -> &str {
    match profile_name {
        "anthropic" => "Anthropic",
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "glm-coding" => "GLM (coding)",
        "zai" => "Z.AI",
        "openai" => "OpenAI",
        "openai-chatgpt" => "OpenAI (ChatGPT login)",
        "github-copilot" => "GitHub Copilot",
        "gemini" => "Google Gemini",
        "zhipuai-coding-plan" => "GLM (coding)",
        other => other,
    }
}

/// Subagent API seam — delegates 1:1 to the service. Subagent calls never carry
/// a provider profile, so `profile` is always `None`.
#[async_trait]
impl agent::SubagentApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create(model, None, system, messages, tools)
            .await
    }

    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // Subagent stream: fast mode is a main-loop-only tier, so `speed=None`.
        self.service
            .stream(model, None, system, messages, tools, effort, None)
            .await
    }

    async fn messages_create_stream_forced(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        self.service
            .stream_forced(model, None, system, messages, tools, forced_tool, effort)
            .await
    }

    // ── Provider-routed variants (dual-LLM dual-PROVIDER) ──────────────────
    // These forward the per-spawn `profile` to the multi-provider service so a
    // dual-LLM candidate's resolved provider is honored, instead of dropping the
    // profile (which forced every subagent onto the default provider). The
    // default-trait impls delegate to the profile-less methods above; these
    // overrides are the single place the subagent path becomes provider-aware.

    async fn messages_create_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.service
            .messages_create(model, profile, system, messages, tools)
            .await
    }

    async fn messages_create_stream_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // Subagent stream: fast mode is a main-loop-only tier, so `speed=None`.
        self.service
            .stream(model, profile, system, messages, tools, effort, None)
            .await
    }

    async fn messages_create_stream_forced_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        self.service
            .stream_forced(model, profile, system, messages, tools, forced_tool, effort)
            .await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // (M4 cc2.1.198) The MAIN loop carries the session's initial effort
        // (CLI `--effort` → `output_config.effort`); `None` (no flag) keeps
        // the pre-M4 body byte-identical.
        // (/fast) When the shared fast-mode flag is set AND the active model
        // supports fast mode (opus-4-7 / opus-4-8), send `speed:"fast"` — the
        // service's `beta_context` reads it back to add the fast-mode beta.
        // `None` (flag off, or an unsupported model) keeps the body unchanged.
        let speed = if self.fast_mode.load(std::sync::atomic::Ordering::SeqCst)
            && model_supports_fast_mode(model)
        {
            Some("fast".to_string())
        } else {
            None
        };
        self.service
            .stream(
                model,
                profile,
                system,
                messages,
                tools,
                self.current_effort(),
                speed,
            )
            .await
    }

    fn last_retry_count(&self) -> u32 {
        self.service.last_retry_count()
    }
}
// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::model::user_agent::UserAgentEnv;
    use llm_client::DefaultLlmClient;
    use llm_client::{
        ApiService, AuthStrategy, BoxFuture, Capabilities, ClientConfig, CredentialConfig,
        LlmError, ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
        ProviderRequest, ProviderResponse, StreamingResponse, Transport,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ── FakeTransport (duplicated into the orchestrator-domain tests) ─────────────

    /// A fake Transport that returns a scripted sequence of responses (or errors).
    struct FakeTransport {
        /// Pre-recorded responses returned in order; cycles to last entry.
        responses: Mutex<Vec<FakeResponse>>,
        /// All requests received, in order.
        seen: Mutex<Vec<ProviderRequest>>,
    }

    #[derive(Clone)]
    #[allow(dead_code)]
    enum FakeResponse {
        Ok(ProviderResponse),
        Err(LlmError),
    }

    impl FakeTransport {
        /// Return the same response on every call.
        fn always(resp: ProviderResponse) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(vec![FakeResponse::Ok(resp)]),
                seen: Mutex::new(vec![]),
            })
        }

        #[allow(dead_code)]
        fn seen_count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl Transport for FakeTransport {
        fn execute<'a>(
            &'a self,
            request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            let mut seen = self.seen.lock().unwrap();
            seen.push(request.clone());
            let idx = (seen.len() - 1).min({
                let resps = self.responses.lock().unwrap();
                resps.len().saturating_sub(1)
            });
            let resp = {
                let resps = self.responses.lock().unwrap();
                resps[idx].clone()
            };
            Box::pin(async move {
                match resp {
                    FakeResponse::Ok(r) => Ok(r),
                    FakeResponse::Err(e) => Err(e),
                }
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "open_stream not scripted".to_string(),
                })
            })
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn ok_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": "msg_test",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
    }

    /// Build the thin `ProviderApiAdapter` over an `ApiService` constructed exactly
    /// as the original `make_adapter` did (a single anthropic profile, ApiKey auth).
    fn make_adapter(transport: Arc<dyn Transport>) -> ProviderApiAdapter {
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new(Arc::new(ApiService::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )))
    }

    #[test]
    fn list_model_listings_exposes_catalog() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let listings = OrchestratorApiClient::list_model_listings(&adapter);
        // The first-party Anthropic family plus the static llm-client catalog
        // (openrouter + deepseek + glm-coding + zai + github-copilot) yields well
        // over 100 model rows.
        assert!(
            listings.len() >= 100,
            "expected >=100 catalog listings, got {}",
            listings.len()
        );
        // Each provider appears with its hand-authored label.
        let label_for = |id: &str| -> Option<String> {
            listings
                .iter()
                .find(|l| l.provider_id == id)
                .map(|l| l.provider_label.clone())
        };
        assert_eq!(label_for("openrouter").as_deref(), Some("OpenRouter"));
        assert_eq!(label_for("deepseek").as_deref(), Some("DeepSeek"));
        assert_eq!(label_for("glm-coding").as_deref(), Some("GLM (coding)"));
        assert_eq!(
            label_for("github-copilot").as_deref(),
            Some("GitHub Copilot")
        );
        // First-party Anthropic is now part of the catalog (it used to live as a
        // Claude-only hardcoded fallback in `list_available_models`).
        assert_eq!(label_for("anthropic").as_deref(), Some("Anthropic"));
        assert!(
            listings
                .iter()
                .any(|l| l.provider_id == "anthropic" && l.request_model == "claude-opus-4-8"),
            "expected the first-party Anthropic Claude family in the catalog"
        );
        // (model-no-row-descriptions) Claude-family ids in the catalog (e.g.
        // openrouter's `*opus*`/`*sonnet*`/`*haiku*`) pick up a built-in blurb so
        // the picker renders a dimmed sub-line. At least one listing carries one.
        assert!(
            listings.iter().any(|l| l.description.is_some()),
            "expected at least one catalog listing to carry a description"
        );
    }

    #[test]
    fn list_model_listings_excludes_no_tool_models() {
        // Models with `tool_call=false` (image/TTS models, gpt-3.5-turbo, …) can
        // never complete an agentic turn — the agent always sends tools — so they
        // must NOT appear in the /model picker catalog. Tool-capable models stay.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let listings = OrchestratorApiClient::list_model_listings(&adapter);
        let has = |id: &str| listings.iter().any(|l| l.request_model == id);

        // Excluded: genuinely tool_call=false in the vendored models.dev slices.
        assert!(
            !has("gpt-5-chat-latest"),
            "no-tool OpenAI chat model must be hidden"
        );
        assert!(
            !has("gemini-2.5-flash-image"),
            "no-tool Gemini image model must be hidden"
        );
        // Kept: tool-capable models remain selectable — incl. gpt-3.5-turbo,
        // whose tool_call=false was a DATA error (it supports function calling).
        assert!(has("deepseek-chat"), "tool-capable model must remain");
        assert!(has("gpt-5.2"), "tool-capable OpenAI model must remain");
        assert!(
            has("gpt-3.5-turbo"),
            "gpt-3.5-turbo supports tools — must remain"
        );
    }

    #[test]
    fn list_model_listings_surfaces_reasoning_capability() {
        // The picker's thinking indicator reads `supports_reasoning` off each
        // listing (populated from the catalog `capabilities.reasoning`).
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let listings = OrchestratorApiClient::list_model_listings(&adapter);
        let reasoning = |id: &str| {
            listings
                .iter()
                .find(|l| l.request_model == id)
                .map(|l| l.supports_reasoning)
        };
        // Claude 4.x/5 support extended thinking.
        assert_eq!(reasoning("claude-opus-4-8"), Some(true));
        // A visible (tool-capable) but non-thinking model is flagged false.
        assert_eq!(reasoning("qwen/qwen3-coder:free"), Some(false));
    }

    #[test]
    fn model_description_matches_known_families() {
        assert_eq!(
            model_description("claude-opus-4-7"),
            Some("Best for everyday, complex tasks")
        );
        assert_eq!(
            model_description("anthropic/claude-sonnet-4-6"),
            Some("Efficient for routine tasks")
        );
        assert_eq!(
            model_description("claude-3-5-haiku"),
            Some("Fastest for quick answers")
        );
        assert_eq!(model_description("gpt-4o"), None);
    }

    #[tokio::test]
    async fn subagent_api_client_seam_forwards_through_trait_object() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let seam: Arc<dyn agent::SubagentApiClient> = Arc::new(adapter);
        let result = seam
            .messages_create(
                "claude-sonnet-4-20250514",
                Some("sys"),
                Vec::new(),
                Vec::new(),
            )
            .await;
        // May succeed or fail with UnsupportedCapability if stream not configured,
        // but must not panic.
        let _ = result;
    }

    /// The trait default (used by mocks / non-routing impls) is the byte/4
    /// approximation over the conversation text: `(system + msg text) / 4`,
    /// floored at 1.
    #[tokio::test]
    async fn count_tokens_default_impl_is_byte_over_four_approximation() {
        let mock = crate::test_support::MockApiClient::new(vec![]);
        // system = 8 bytes; one user message of 40 bytes → (8 + 40) / 4 = 12.
        let msgs = vec![protocol::ConversationMessage::user(
            protocol::MessageId::new(),
            "1234567890123456789012345678901234567890".to_string(),
        )];
        let count = OrchestratorApiClient::count_tokens(
            &mock,
            "any-model",
            None,
            Some("12345678"),
            msgs,
            Vec::new(),
        )
        .await
        .expect("default count_tokens ok");
        assert_eq!(count, 12, "(8 system + 40 user) / 4 = 12 tokens");
    }

    // ── Fix 2: RepeatedOverloaded → LlmError::Overloaded { repeated: true } → OrchestratorError ──

    /// Fix 2 end-to-end: a scripted transport that returns 529 three times triggers
    /// the external non-sandbox `DriveStep::RepeatedOverloaded` branch, which the
    /// adapter surfaces as `LlmError::Overloaded { repeated: true }`.  The
    /// `From<LlmError>` conversion on `OrchestratorError` then produces
    /// `OrchestratorError::RepeatedOverloaded` whose Display equals the byte-locked
    /// `"Repeated 529 Overloaded errors"` copy (`errors.ts:166`).
    #[tokio::test]
    async fn repeated_529_terminal_maps_to_byte_locked_copy() {
        use crate::error::{OrchestratorError, REPEATED_529_ERROR_MESSAGE};

        // Transport that always returns 529 overloaded.
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::always(ProviderResponse::json(529, overloaded_body));
        // make_adapter wires user_type=Some("external") in the UserAgentEnv but
        // resolve_retry_control reads USER_TYPE from ResolveRetryEnv::from_process_env().
        // Set the env vars temporarily to gate allow_fallback + is_external.
        // std::env::set_var is deprecated (Rust 2024) but not removed; acceptable
        // in test-only code.
        #[allow(deprecated)]
        std::env::set_var("USER_TYPE", "external");
        #[allow(deprecated)]
        std::env::set_var("FALLBACK_FOR_ALL_PRIMARY_MODELS", "1");
        #[allow(deprecated)]
        std::env::remove_var("IS_SANDBOX");

        let adapter = make_adapter(transport);
        let llm_result = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await;

        // Clean up before any assert that might panic.
        #[allow(deprecated)]
        std::env::remove_var("FALLBACK_FOR_ALL_PRIMARY_MODELS");
        #[allow(deprecated)]
        std::env::remove_var("USER_TYPE");

        // The adapter must return Err(LlmError::Overloaded { repeated: true }).
        match &llm_result {
            Err(LlmError::Overloaded { repeated: true }) => {} // correct
            other => {
                panic!("expected Err(LlmError::Overloaded {{ repeated: true }}), got {other:?}")
            }
        }

        // The OrchestratorError conversion must yield RepeatedOverloaded.
        let orch_err: OrchestratorError = llm_result.unwrap_err().into();
        assert!(
            matches!(orch_err, OrchestratorError::RepeatedOverloaded),
            "OrchestratorError must be RepeatedOverloaded, got {orch_err:?}"
        );
        assert_eq!(
            orch_err.to_string(),
            REPEATED_529_ERROR_MESSAGE,
            "Display must equal the byte-locked copy"
        );
    }

    // ── 3c-T3: LlmResponse.cost populated from cost estimator ─────────────────

    fn make_adapter_with_estimator(transport: Arc<dyn Transport>) -> ProviderApiAdapter {
        use crate::cost_wiring::llm_catalog_from_cost;
        use cost::pricing::PricingCatalog as CostCatalog;
        use llm_client::{CostEstimator, PricingPolicy};
        #[allow(deprecated)]
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated));

        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        description: None,
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new(Arc::new(ApiService::new_with_estimator(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
            Some(estimator),
        )))
    }

    /// 3c-T3: adapter populates response.cost for a priced model.
    ///
    /// claude-sonnet-4 billing_model → catalog hit → cost is Some(estimate with
    /// total_cost_usd present).
    #[tokio::test]
    async fn cost_populated_for_priced_model() {
        let response_json = serde_json::json!({
            "id": "msg_cost_test",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = make_adapter_with_estimator(transport);
        let resp = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");
        let cost = resp.cost.expect("cost must be Some for a priced model");
        let total = cost.total_cost_usd.expect("total_cost_usd must be Some");
        // claude-sonnet-4: input 3_000 nano → 3.0 usd/M × 1M + output 15_000 → 15.0 × 1M = 18.0
        assert!(
            (total - 18.0).abs() < 1e-9,
            "expected total $18.0 for 1M in + 1M out at $3/$15, got ${total}"
        );
    }

    /// 3c-T3: unknown billing model → cost stays None (no error).
    ///
    /// The adapter uses a model profile whose billing_model ("claude-sonnet-4")
    /// IS in the catalog; to test the None path we use a profile with a
    /// billing_model that has no entry.
    #[tokio::test]
    async fn cost_none_for_unpriced_model() {
        // Build an adapter with an estimator but a billing model not in the catalog.
        use crate::cost_wiring::llm_catalog_from_cost;
        use cost::pricing::PricingCatalog as CostCatalog;
        use llm_client::{CostEstimator, PricingPolicy};
        #[allow(deprecated)]
        std::env::set_var("ADAPTER_TEST_KEY2", "test-key");
        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated));

        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY2".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-future-9999".to_string(),
                        request_model: "claude-future-9999".to_string(),
                        // billing_model not in any catalog entry
                        billing_model: "claude-future-9999".to_string(),
                        aliases: vec![],
                        description: None,
                        capabilities: Capabilities {
                            reasoning: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                    supports_websockets: false,
                    supports_websocket_compression: false,
                    websocket_connect_timeout_ms: None,
                }],
            })
            .expect("client"),
        );
        let response_json = serde_json::json!({
            "id": "msg_unpriced",
            "model": "claude-future-9999",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = ProviderApiAdapter::new(Arc::new(ApiService::new_with_estimator(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
            Some(estimator),
        )));
        let resp = adapter
            .messages_create("claude-future-9999", None, None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        assert!(
            resp.cost.is_none(),
            "unpriced billing_model must leave cost = None; got {:?}",
            resp.cost
        );
    }

    /// 3c-T3: cost tracker recording is unchanged (existing CostTracker tests still pass).
    ///
    /// When no estimator is wired, response.cost stays None — backward-compat.
    #[tokio::test]
    async fn no_estimator_leaves_cost_none() {
        let response_json = serde_json::json!({
            "id": "msg_no_est",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        // make_adapter wires None estimator (ApiService::new default path)
        let adapter = make_adapter(transport);
        let resp = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");
        assert!(resp.cost.is_none(), "no estimator → cost must be None");
    }

    // ── Task 5 Part B: OrchestratorApiClient::last_rate_limit_info ──────────────

    /// `OrchestratorApiClient::last_rate_limit_info` returns the adapter's stored
    /// rate-limit info mapped into a `traits::RateLimitSnapshot`.
    ///
    /// After a 2xx response with unified headers the snapshot must carry all
    /// three fields: `rate_limit_type`, `overage_status`, and
    /// `overage_disabled_reason`.
    #[tokio::test]
    async fn orchestrator_api_client_last_rate_limit_info_returns_stored_info() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim".to_string(),
            "five_hour".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-status".to_string(),
            "allowed_warning".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-disabled-reason".to_string(),
            "out_of_credits".to_string(),
        );

        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers,
            body_json: ok_response_json(),
            request_id: None,
        });
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");

        // Via the OrchestratorApiClient trait method (RateLimitSnapshot).
        let snapshot = OrchestratorApiClient::last_rate_limit_info(&adapter)
            .expect("must be Some after 2xx with unified headers");
        assert_eq!(
            snapshot.rate_limit_type.as_deref(),
            Some("five_hour"),
            "rate_limit_type must round-trip through the snapshot"
        );
        assert_eq!(
            snapshot.overage_status.as_deref(),
            Some("allowed_warning"),
            "overage_status must round-trip through the snapshot"
        );
        assert_eq!(
            snapshot.overage_disabled_reason.as_deref(),
            Some("out_of_credits"),
            "overage_disabled_reason must round-trip through the snapshot"
        );
    }

    /// `OrchestratorApiClient::last_rate_limit_info` returns `None` before any
    /// response with unified headers.
    #[tokio::test]
    async fn orchestrator_api_client_last_rate_limit_info_none_initially() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create(
                "claude-sonnet-4-20250514",
                None,
                None,
                Vec::new(),
                Vec::new(),
            )
            .await
            .expect("ok");

        // No unified headers → trait method also returns None.
        assert!(
            OrchestratorApiClient::last_rate_limit_info(&adapter).is_none(),
            "trait method must return None when adapter has no unified header snapshot"
        );
    }

    #[test]
    fn model_supports_fast_mode_gates_on_opus_fast_tier() {
        // Only the opus-4-7 / opus-4-8 fast tier supports fast mode (mirrors
        // run.rs::model_capabilities). Case-insensitive.
        assert!(model_supports_fast_mode("claude-opus-4-8"));
        assert!(model_supports_fast_mode("claude-opus-4-7"));
        assert!(model_supports_fast_mode("CLAUDE-OPUS-4-8"));
        assert!(!model_supports_fast_mode("claude-sonnet-4-20250514"));
        assert!(!model_supports_fast_mode("claude-opus-4-1"));
        assert!(!model_supports_fast_mode("gpt-5.2"));
    }

    #[test]
    fn live_effort_replaces_and_clears_the_startup_value() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport).with_initial_effort(Some(serde_json::json!("low")));
        assert_eq!(adapter.current_effort(), Some(serde_json::json!("low")));

        OrchestratorApiClient::set_effort(&adapter, Some(serde_json::json!("high")));
        assert_eq!(adapter.current_effort(), Some(serde_json::json!("high")));

        OrchestratorApiClient::set_effort(&adapter, None);
        assert_eq!(adapter.current_effort(), None);
    }
}
