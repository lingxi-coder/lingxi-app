//! Engine-independent, host-owned access to SDK audio services.
//! Profiles and secrets arrive only from trusted native/main-process code.

use base64::{engine::general_purpose::STANDARD, Engine};
use llm_runtime::services::sdk::{self, audio};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_AUDIO_BYTES: usize = 12 * 1024 * 1024;

pub mod media_attachment;
pub mod media_cloud;
pub mod media_configuration;
pub mod media_host;
pub mod media_native;
pub mod media_store;
pub mod realtime;

pub use media_attachment::{AppMediaConversationAttachment, AudioConfigurationSnapshot};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Session identity and routing context used by `follow_session` audio requests.
pub struct SessionContext {
    /// Identifier of the conversation that supplied this audio context.
    pub session_id: String,
    /// Exact provider profile selected for the session.
    pub profile_id: String,
    /// Account/quota scope forwarded with requests on the session route.
    pub account_scope: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Cloud route selection carried by an audio request.
pub struct CloudSelection {
    /// Route binding: `follow_session` uses `session`; `explicit_profile` uses `profile_id`.
    pub binding: String,
    /// Exact provider profile for `explicit_profile`; ignored for `follow_session`.
    pub profile_id: Option<String>,
    /// Optional provider model identifier, checked against the selected operation's capabilities.
    pub model_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Strict camelCase request envelope shared by audio capability and media operations.
pub struct AudioRequest {
    /// Caller-supplied identity, unique among active operations and accepted by [`AudioProviderHost::cancel`].
    pub operation_id: String,
    /// Audio operation kind: `recognition`, `speech`, or `realtime`.
    pub kind: String,
    /// Provider profile and optional model selection.
    pub cloud: CloudSelection,
    /// Required when `cloud.binding` is `follow_session`.
    pub session: Option<SessionContext>,
    /// Optional locale hint translated through the selected provider's audio capabilities.
    pub language: Option<String>,
    /// Optional provider voice identifier for speech synthesis.
    pub voice: Option<String>,
    /// Requested synthesis rate; currently only finite normal speed (`1.0`) is supported.
    pub rate: Option<f32>,
    /// Realtime interaction mode, when present: `turn_based` or `interruptible`.
    pub interaction: Option<String>,
    /// Operation deadline in milliseconds; defaults to 30 seconds and is capped at 30 minutes.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Maximum accepted input or collected output size in bytes; defaults to 12 MiB.
    #[serde(default = "default_limit")]
    pub max_payload_bytes: usize,
}

fn default_timeout() -> u64 {
    30_000
}
fn default_limit() -> usize {
    MAX_AUDIO_BYTES
}

/// No secrets are exposed in Debug, responses, or provider capability snapshots.
pub struct AudioProviderHost {
    client: sdk::LlmClient,
    credential_ids: BTreeMap<String, String>,
    bound_services: llm_runtime::services::ProviderServices,
    pending: Arc<Mutex<BTreeMap<String, CancellationToken>>>,
}

struct AudioSelection {
    route: audio::AudioRoute,
    client: sdk::LlmClient,
    credential: sdk::protocol::Secret<String>,
    account: llm_runtime::services::SelectedServiceAccount,
}

struct AudioCallFailure {
    value: Value,
    provider: Option<audio::AudioError>,
}
impl AudioCallFailure {
    fn local(kind: &str, message: &str) -> Self {
        Self {
            value: failure(kind, message),
            provider: None,
        }
    }
    fn provider(error: audio::AudioError) -> Self {
        Self {
            value: failure("provider_error", "selected audio provider request failed"),
            provider: Some(error),
        }
    }
    fn into_value(self) -> Value {
        self.provider.map_or(self.value, provider_failure)
    }
}

impl AudioProviderHost {
    /// Bind to the running session's actual SDK services and credential sources.
    /// Project/managed profile overrides stay inside Rust; secrets do not cross FFI.
    pub fn from_provider_services(
        services: llm_runtime::services::ProviderServices,
        credential_ids: BTreeMap<String, String>,
    ) -> Self {
        Self {
            client: services.client().clone(),
            credential_ids,
            bound_services: services,
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_empty() -> Self {
        Self::test_with_transport(Arc::new(http_client::provider_transport().unwrap()), None)
    }

    #[cfg(test)]
    pub(crate) fn test_with_transport(
        transport: Arc<dyn sdk::Transport>,
        key: Option<&str>,
    ) -> Self {
        use lingxi_core::host::provider_accounts::{
            AccountAuthKind, AccountMetadata, CredentialRef,
        };
        let mut assembled = provider_config::assemble(provider_config::AssembleInputs {
            official_catalog: None,
            anthropic_api_base: "https://api.anthropic.com".into(),
            anthropic_models: vec![],
            anthropic_has_api_key: false,
            anthropic_has_oauth: false,
            user_providers: BTreeMap::new(),
            routing: None,
        });
        let accounts = key
            .map(|_| {
                vec![AccountMetadata {
                    account_id: "00000000-0000-4000-8000-000000000001".into(),
                    provider_id: "openai".into(),
                    display_name: "Fixture".into(),
                    auth_kind: AccountAuthKind::ApiKey,
                    execution_binding_id: "openai".into(),
                    credential_ref: CredentialRef {
                        store_scope: "fixture".into(),
                        secret_ref: "acct.00000000-0000-4000-8000-000000000001".into(),
                    },
                    enabled: true,
                    user_priority: 0,
                    config_revision: 1,
                    identity_generation: 1,
                }]
            })
            .unwrap_or_default();
        provider_config::assemble_account_profiles(&mut assembled, &accounts).unwrap();
        let pool = Arc::new(llm_runtime::accounts::AccountPool::new(
            accounts,
            Arc::new(llm_runtime::accounts::MemoryAccountPoolStore::default()),
        ));
        let mut model = llm_runtime::ModelRuntime::from_config(assembled.client_config)
            .unwrap()
            .with_account_pool(pool);
        if let Some(key) = key {
            model = model.with_credential_provider(Arc::new(
                llm_runtime::StaticCredentialProvider::new(llm_runtime::Credential::ApiKey(
                    key.into(),
                )),
            ));
        }
        let ids = assembled
            .credential_sources
            .into_iter()
            .map(|source| (source.profile_name, source.credential_id))
            .collect();
        Self::from_provider_services(
            model
                .provider_services(llm_runtime::Region::International, transport)
                .unwrap(),
            ids,
        )
    }

    /// Exact immutable SDK client snapshot supplied to shared media routing.
    pub(crate) fn client_snapshot(&self) -> sdk::ClientSnapshot {
        self.client.snapshot()
    }

    /// The host must continue to match its owning account catalog.
    pub(crate) fn profile_is_current(&self, profile: &str) -> bool {
        self.bound_services.profile_is_current(profile)
    }

    /// Cancel an active transcription, synthesis, or realtime operation by its request identity.
    pub fn cancel(&self, operation_id: &str) {
        if let Some(cancel) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(operation_id)
        {
            cancel.cancel();
        }
    }

    fn parse(&self, request: &str) -> Result<AudioRequest, Value> {
        if request.len() > MAX_REQUEST_BYTES {
            return Err(failure("invalid_request", "audio request is too large"));
        }
        let request: AudioRequest = serde_json::from_str(request)
            .map_err(|_| failure("invalid_request", "invalid audio request"))?;
        if request.operation_id.is_empty()
            || request.timeout_ms == 0
            || request.timeout_ms > 1_800_000
            || request.max_payload_bytes == 0
            || request.max_payload_bytes > MAX_AUDIO_BYTES
        {
            return Err(failure(
                "invalid_request",
                "audio operation requires identity, deadline and bounded payload",
            ));
        }
        if request
            .interaction
            .as_deref()
            .is_some_and(|mode| !matches!(mode, "turn_based" | "interruptible"))
        {
            return Err(failure(
                "invalid_request",
                "unknown realtime interaction mode",
            ));
        }
        Ok(request)
    }

    fn resolve(&self, request: &AudioRequest) -> Result<audio::AudioRoute, Value> {
        let (profile, account) = match request.cloud.binding.as_str() {
            "follow_session" => {
                let session = request
                    .session
                    .as_ref()
                    .filter(|s| {
                        !s.session_id.is_empty()
                            && !s.profile_id.is_empty()
                            && !s.account_scope.is_empty()
                    })
                    .ok_or_else(|| {
                        failure(
                            "needs_configuration",
                            "select a session or explicitly choose an audio provider",
                        )
                    })?;
                (session.profile_id.clone(), session.account_scope.clone())
            }
            "explicit_profile" => {
                let profile = request
                    .cloud
                    .profile_id
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        failure(
                            "needs_configuration",
                            "select an exact audio provider profile",
                        )
                    })?;
                (
                    profile.clone(),
                    lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE.into(),
                )
            }
            _ => return Err(failure("invalid_request", "unknown cloud audio binding")),
        };
        Ok(audio::AudioRoute::new(profile, account))
    }

    async fn select_audio(&self, request: &AudioRequest) -> Result<AudioSelection, Value> {
        let route = self.resolve(request)?;
        let selected = self
            .bound_services
            .resolve_audio_account(
                &route.profile_name,
                operation(&request.kind)?,
                request.cloud.model_id.as_deref(),
                (request.cloud.binding == "follow_session"
                    && route.account_scope
                        != lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE)
                    .then_some(route.account_scope.as_str()),
            )
            .await
            .map_err(service_failure)?;
        Ok(AudioSelection {
            route: audio::AudioRoute::new(
                selected.actual_profile.clone(),
                selected.account_scope.clone(),
            ),
            client: selected.services.client().clone(),
            credential: selected.credential.clone(),
            account: selected,
        })
    }

    async fn audio_capability_preview(
        &self,
        profile: &str,
        operation: audio::AudioOperation,
        model: Option<&str>,
    ) -> Result<Vec<audio::AudioCapabilities>, Value> {
        match self
            .bound_services
            .audio_account_capabilities(profile, operation, model)
            .await
        {
            Ok(caps) => Ok(caps),
            // An empty account directory still permits structural metadata for
            // the configuration UI. Business dispatch must use the resolver.
            Err(llm_runtime::LlmError::AllAccountsUnavailable { .. }) => Ok(Vec::new()),
            Err(error) => Err(service_failure(error)),
        }
    }

    /// Return a JSON capability/readiness snapshot for a serialized [`AudioRequest`].
    ///
    /// The result describes the selected profile, supported operation, model/voice
    /// choices, and credential readiness without exposing credential material.
    pub async fn capabilities(&self, input: &str) -> String {
        let result = async {
            let request = self.parse(input)?;
            let route = self.resolve(&request)?;
            let operation = operation(&request.kind)?;
            let services = &self.bound_services;
            let (caps, account_credential_error) = {
                let candidates = self.audio_capability_preview(&route.profile_name, operation, request.cloud.model_id.as_deref()).await?;
                let mut ready = None;
                let mut credential_error = None;
                for candidate in &candidates {
                    match services.service_credential_source(&candidate.profile_name).await {
                        Ok(llm_runtime::CredentialSource::None | llm_runtime::CredentialSource::Unknown) => {},
                        Ok(_) => { ready = Some(candidate.clone()); break; },
                        Err(error) => credential_error = Some(service_failure(error)),
                    }
                }
                let present = ready.is_some();
                let caps = ready.or_else(|| candidates.into_iter().next()).or_else(|| self.client.audio().capabilities(&route).ok())
                    .ok_or_else(|| failure("unsupported", "the selected audio provider profile is unsupported"))?;
                (caps, if present { None } else { Some(credential_error.unwrap_or_else(|| failure("needs_configuration", "configure an eligible audio account credential"))) })
            };
            let descriptor = caps.operation(operation);
            let adapter_supported = if operation == audio::AudioOperation::NativeRealtime { caps.agent_conversation } else { caps.supports(operation) };
            let model = caps.model(operation, request.cloud.model_id.as_deref());
            let voice_supported = request.voice.as_ref().is_none_or(|voice| model.as_ref().is_ok_and(|model| model.voices.is_empty() || model.voices.iter().any(|entry| &entry.id == voice)));
            let format_supported = operation != audio::AudioOperation::Synthesis || model.as_ref().is_ok_and(|model| model.formats.contains(&audio::AudioFormat::Pcm16Le));
            let rate_supported = request.rate.is_none_or(|rate| rate.is_finite() && (rate - 1.0).abs() <= f32::EPSILON);
            let interaction_supported = request.interaction.as_deref() != Some("interruptible") || caps.native_realtime_contract.as_ref().is_some_and(|contract| contract.audio_truncation);
            let supported = adapter_supported;
            let configuration_reason = if model.is_err() { Some("select a supported audio model") }
                else if !voice_supported { Some("select a supported audio voice") }
                else if !format_supported { Some("select a model that returns playable PCM audio") }
                else if !rate_supported { Some("this provider requires normal speech speed") }
                else if !interaction_supported { Some("this provider requires turn-based conversation") }
                else { None };
            let credential_error = if supported && configuration_reason.is_none() {
                account_credential_error
            } else { None };
            let readiness = if !supported { "unsupported" } else if configuration_reason.is_some() { "needs_configuration" }
                else if credential_error.as_ref().is_some_and(|error| error["error"]["kind"] == "unavailable") { "unavailable" }
                else if credential_error.is_some() { "needs_configuration" } else { "ready" };
            let reason = if !supported { Some("selected profile does not implement this audio operation".to_owned()) }
                else { configuration_reason.map(str::to_owned).or_else(|| credential_error.as_ref().and_then(|error| error["error"]["message"].as_str().map(str::to_owned))) };
            let model_id = request.cloud.model_id.as_deref().or_else(|| descriptor.and_then(|d| d.default_model.as_deref()));
            let models = descriptor.map(|d| d.models.iter().map(|model| json!({"id":model.id,"voices":model.voices})).collect::<Vec<_>>()).unwrap_or_default();
            Ok::<_,Value>(json!({"supported":supported,"readiness":readiness,
                "reason":reason,
                "credentialId":self.credential_ids.get(&route.profile_name),
                "profileId":route.profile_name,"providerId":caps.provider_id,"modelId":model_id,
                "models":models,"streaming":caps.supports(audio::AudioOperation::LiveAsr),"realtime":caps.agent_conversation,"capabilities":caps.native_realtime_contract}))
        }.await;
        result.unwrap_or_else(|error| error).to_string()
    }

    /// Transcribe bounded recorded audio and return the operation result as JSON.
    ///
    /// The request kind must be `recognition`; `mime_type` must be one of the
    /// supported recorded formats (`audio/wav`, `audio/x-wav`, `audio/mp4`,
    /// `audio/m4a`, `audio/mpeg`, `audio/flac`, or `audio/webm`).
    pub async fn transcribe(&self, input: &str, bytes: Vec<u8>, mime_type: &str) -> String {
        self.run(input, |request, route, client, options| async move {
            if request.kind != "recognition"
                || bytes.is_empty()
                || bytes.len() > request.max_payload_bytes
            {
                return Err(AudioCallFailure::local(
                    "invalid_request",
                    "invalid bounded transcription input",
                ));
            }
            let extension = match mime_type {
                "audio/wav" | "audio/x-wav" => "wav",
                "audio/mp4" | "audio/m4a" => "m4a",
                "audio/mpeg" => "mp3",
                "audio/flac" => "flac",
                "audio/webm" => "webm",
                _ => return Err(AudioCallFailure::local("unsupported", "unsupported recorded audio format")),
            };
            let caps = client.audio().capabilities(&route).map_err(AudioCallFailure::provider)?;
            let language = caps.operation(audio::AudioOperation::FileTranscription)
                .and_then(|descriptor| descriptor.language_hint_for_locale(request.language.as_deref()));
            let output = client.audio()
                .transcribe(
                    &route,
                    audio::AudioInput::from_bytes(
                        format!("recording.{extension}"),
                        mime_type,
                        bytes,
                    ),
                    &audio::TranscriptionRequest {
                        model: request.cloud.model_id,
                        language,
                        raw_format: None,
                    },
                    &options,
                )
                .await
                .map_err(AudioCallFailure::provider)?;
            Ok(json!({"text":output.text,"language":output.language,"usage":output.usage,"usageContext":{"operationId":request.operation_id,"profileId":route.profile_name,"accountScope":route.account_scope,"modelId":output.model,"providerId":output.provider_id}}))
        })
        .await
    }

    /// Synthesize bounded nonempty text and return playable PCM audio as JSON.
    ///
    /// The request kind must be `speech`; the selected route must return mono
    /// PCM16 audio with a positive sample rate.
    pub async fn synthesize(&self, input: &str, text: &str) -> String {
        self.run(input, |request,route,client,options| async move {
            if request.kind != "speech" { return Err(AudioCallFailure::local("invalid_request", "synthesis requires speech configuration")); }
            if text.trim().is_empty() || text.len() > request.max_payload_bytes.min(MAX_REQUEST_BYTES) { return Err(AudioCallFailure::local("invalid_request", "synthesis requires bounded nonempty text")); }
            let mut synthesis = audio::SynthesisRequest::new(text);
            let caps = client.audio().capabilities(&route).map_err(AudioCallFailure::provider)?;
            synthesis.language = caps.operation(audio::AudioOperation::Synthesis)
                .and_then(|descriptor| descriptor.language_hint_for_locale(request.language.as_deref()));
            synthesis.model=request.cloud.model_id; synthesis.voice=request.voice;
            let output=client.audio().synthesize(&route,&synthesis,&options).await.map_err(AudioCallFailure::provider)?;
            let audio::AudioOutput::Stream(stream)=output else { return Err(AudioCallFailure::local("unsupported", "this route returns a provider URL rather than playable PCM")); };
            let audio=stream.collect(request.max_payload_bytes).await.map_err(AudioCallFailure::provider)?;
            if audio.metadata.format != audio::AudioFormat::Pcm16Le || audio.metadata.channels != Some(1)
                || audio.bytes.is_empty() || audio.bytes.len()%2!=0 { return Err(AudioCallFailure::local("unsupported", "provider did not return PCM16 mono audio")); }
            let rate=audio.metadata.sample_rate_hz.filter(|rate| *rate>0).ok_or_else(|| AudioCallFailure::local("unavailable", "provider PCM audio is missing its sample rate"))?;
            Ok(json!({"pcmBase64":STANDARD.encode(audio.bytes),"sampleRateHz":rate,"usage":audio.usage,"usageContext":{"operationId":request.operation_id,"profileId":audio.metadata.route.profile_name,"accountScope":audio.metadata.route.account_scope,"modelId":audio.metadata.model,"providerId":audio.metadata.provider_id}}))
        }).await
    }

    async fn run<F, Fut>(&self, input: &str, execute: F) -> String
    where
        F: FnOnce(AudioRequest, audio::AudioRoute, sdk::LlmClient, sdk::RequestOptions) -> Fut,
        Fut: std::future::Future<Output = Result<Value, AudioCallFailure>>,
    {
        let request = match self.parse(input) {
            Ok(request) => request,
            Err(error) => return error.to_string(),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(request.timeout_ms);
        let identity = request.operation_id.clone();
        let cancel = CancellationToken::new();
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending.contains_key(&identity) {
                return failure("busy", "audio operation identity is already active").to_string();
            }
            pending.insert(identity.clone(), cancel.clone());
        }
        let _guard = PendingGuard {
            host: self,
            identity,
        };
        let work = async {
            let route = self.resolve(&request)?;
            let op = operation(&request.kind)?;
            // Validate request controls from metadata before touching credentials
            // or claiming a recovery probe. The shared resolver still owns the
            // concrete account choice and validates its operation/model support.
            let mut previews = self
                .audio_capability_preview(
                    &route.profile_name,
                    op,
                    request.cloud.model_id.as_deref(),
                )
                .await?;
            if previews.is_empty() {
                if let Ok(caps) = self.client.audio().capabilities(&route) {
                    previews.push(caps);
                }
            }
            if !previews.is_empty()
                && previews
                    .iter()
                    .all(|caps| validate_audio_controls(&request, op, caps).is_err())
            {
                if let Err(error) = validate_audio_controls(&request, op, &previews[0]) {
                    return Err(error);
                }
            }
            let mut selected = self.select_audio(&request).await?;
            let caps = selected
                .client
                .audio()
                .capabilities(&selected.route)
                .map_err(|_| failure("unsupported", "audio profile is unavailable"))?;
            validate_audio_controls(&request, op, &caps)?;
            let options = sdk::RequestOptions {
                credential: Some(selected.credential.clone()),
                account_scope: Some(selected.route.account_scope.clone()),
                total_timeout: Some(
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                ),
                ..Default::default()
            };
            let result = execute(request, selected.route, selected.client, options).await;
            match &result {
                Ok(_) => selected.account.succeeded().await,
                Err(error) => {
                    if let Some(error) = error.provider.as_ref() {
                        selected.account.failed_audio(error).await;
                    }
                }
            }
            result.map_err(AudioCallFailure::into_value)
        };
        let result = tokio::select! { biased;
            _ = cancel.cancelled() => Err(failure("cancelled", "audio operation cancelled")),
            result = tokio::time::timeout_at(deadline, work) => result.unwrap_or_else(|_| Err(failure("timeout", "audio operation deadline exceeded"))),
        };
        result.unwrap_or_else(|error| error).to_string()
    }
}

fn validate_audio_controls(
    request: &AudioRequest,
    operation: audio::AudioOperation,
    caps: &audio::AudioCapabilities,
) -> Result<(), Value> {
    if !caps.supports(operation) {
        return Err(failure(
            "unsupported",
            "selected provider does not support this audio operation",
        ));
    }
    let model = caps
        .model(operation, request.cloud.model_id.as_deref())
        .map_err(|_| failure("unsupported", "selected audio model is unsupported"))?;
    if request.voice.as_ref().is_some_and(|voice| {
        !model.voices.is_empty() && !model.voices.iter().any(|entry| &entry.id == voice)
    }) {
        return Err(failure(
            "unsupported",
            "selected audio voice is unsupported",
        ));
    }
    if operation == audio::AudioOperation::Synthesis
        && (!model.formats.contains(&audio::AudioFormat::Pcm16Le)
            || request
                .rate
                .is_some_and(|rate| !rate.is_finite() || (rate - 1.0).abs() > f32::EPSILON))
    {
        return Err(failure(
            "unsupported",
            "selected audio output format or speech rate is unsupported",
        ));
    }
    Ok(())
}

struct PendingGuard<'a> {
    host: &'a AudioProviderHost,
    identity: String,
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.host
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.identity);
    }
}
fn operation(kind: &str) -> Result<audio::AudioOperation, Value> {
    match kind {
        "recognition" => Ok(audio::AudioOperation::FileTranscription),
        "speech" => Ok(audio::AudioOperation::Synthesis),
        "realtime" => Ok(audio::AudioOperation::NativeRealtime),
        _ => Err(failure("invalid_request", "unknown audio operation kind")),
    }
}
/// Construct the standard structured audio error object returned by this host.
pub fn failure(kind: &str, message: &str) -> Value {
    json!({"error":{"kind":kind,"message":message}})
}
fn service_failure(error: llm_runtime::LlmError) -> Value {
    match error {
        llm_runtime::LlmError::AllAccountsUnavailable {
            accounts,
            earliest_recovery_ms,
        } => {
            json!({"error":{"kind":"all_accounts_unavailable","message":"all eligible audio accounts are unavailable",
            "accounts":accounts.iter().map(|account| account.as_public_failure()).collect::<Vec<_>>(),"earliestRecoveryMs":earliest_recovery_ms}})
        }
        llm_runtime::LlmError::UnsupportedCapability { .. } => failure(
            "unsupported",
            "selected audio operation is unavailable for this account",
        ),
        llm_runtime::LlmError::Authentication { .. } | llm_runtime::LlmError::OAuthRefreshDead => {
            failure(
                "needs_configuration",
                "the selected audio account requires authentication",
            )
        }
        llm_runtime::LlmError::InvalidRequest { .. } => failure(
            "invalid_request",
            "the audio account binding is no longer valid",
        ),
        _ => failure("unavailable", "the selected audio account is unavailable"),
    }
}

fn provider_failure(error: audio::AudioError) -> Value {
    use sdk::providers::openai::audio::AudioError as OpenAiError;
    // Preserve a useful status without returning provider bodies, identifiers,
    // or error messages that could echo credentials supplied in a request.
    let status = error
        .source
        .as_deref()
        .and_then(|source| source.downcast_ref::<OpenAiError>())
        .and_then(|source| match source {
            OpenAiError::Provider { status, .. }
            | OpenAiError::Llm(sdk::protocol::LlmError::ProviderResponse { status, .. }) => {
                Some(*status)
            }
            _ => None,
        });
    let (kind, message) = match error.kind {
        audio::AudioErrorKind::InvalidRequest => {
            ("invalid_request", "audio request validation failed")
        }
        audio::AudioErrorKind::Unsupported => {
            ("unsupported", "selected audio operation is unsupported")
        }
        audio::AudioErrorKind::Unavailable => {
            ("unavailable", "selected audio service is unavailable")
        }
        audio::AudioErrorKind::MediaTooLarge => (
            "media_too_large",
            "audio payload exceeds the provider limit",
        ),
        audio::AudioErrorKind::Provider => {
            ("provider_error", "selected audio provider request failed")
        }
    };
    let message = status
        .map(|status| format!("audio provider returned HTTP {status}"))
        .unwrap_or_else(|| message.to_owned());
    json!({"error":{"kind":kind,"message":message,"dispatch":error.dispatch}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NativeWireCapture {
        sent: Mutex<Vec<(String, Vec<u8>)>>,
    }
    #[async_trait::async_trait]
    impl sdk::Transport for NativeWireCapture {
        async fn send(
            &self,
            request: sdk::HttpRequest,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            self.respond(request.url, request.body.to_vec())
        }
        async fn send_stream(
            &self,
            mut request: sdk::HttpStreamRequest,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            use futures::StreamExt;
            let mut bytes = Vec::new();
            while let Some(chunk) = request.body.next().await {
                bytes.extend_from_slice(&chunk?);
            }
            self.respond(request.url, bytes)
        }
    }
    impl NativeWireCapture {
        fn respond(
            &self,
            url: String,
            body: Vec<u8>,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            use futures::StreamExt;
            let transcription = url.ends_with("/audio/transcriptions");
            self.sent.lock().unwrap().push((url, body));
            let response = if transcription {
                bytes::Bytes::from_static(br#"{"text":"audio contract verified"}"#)
            } else {
                bytes::Bytes::from_static(&[0, 0, 1, 0])
            };
            Ok(sdk::StreamResponse {
                status: 200,
                headers: vec![(
                    "content-type".into(),
                    if transcription {
                        "application/json"
                    } else {
                        "audio/pcm"
                    }
                    .into(),
                )],
                body: futures::stream::once(async move { Ok(response) }).boxed(),
            })
        }
    }
    #[tokio::test]
    async fn device_locale_is_translated_before_native_audio_dispatch() {
        let wire = Arc::new(NativeWireCapture::default());
        let host = AudioProviderHost::test_with_transport(wire.clone(), Some("host-test-key"));
        let mut synthesis = request("speech");
        synthesis["language"] = json!("en-US");
        let result: Value =
            serde_json::from_str(&host.synthesize(&synthesis.to_string(), "hello").await).unwrap();
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(result["sampleRateHz"], 24000);
        let selected_scope = result["usageContext"]["accountScope"]
            .as_str()
            .unwrap()
            .to_owned();
        let identity: llm_runtime::accounts::AccountResourceScope =
            serde_json::from_str(&selected_scope).unwrap();
        assert_eq!(identity.account_id, "00000000-0000-4000-8000-000000000001");
        assert_eq!(
            result["usageContext"]["profileId"],
            format!("account.{}", identity.account_id)
        );
        let mut recognition = request("recognition");
        recognition["language"] = json!("en-US");
        recognition["cloud"] = json!({"binding":"follow_session"});
        recognition["session"] = json!({"sessionId":"fixture-session","profileId":format!("account.{}", identity.account_id),"accountScope":selected_scope});
        let result: Value = serde_json::from_str(
            &host
                .transcribe(&recognition.to_string(), vec![1, 2, 3], "audio/wav")
                .await,
        )
        .unwrap();
        assert_eq!(result["text"], "audio contract verified");
        assert_eq!(result["usageContext"]["accountScope"], selected_scope);
        let sent = wire.sent.lock().unwrap();
        let speech: Value = serde_json::from_slice(&sent[0].1).unwrap();
        assert!(speech.get("language").is_none());
        assert_eq!(speech["response_format"], "pcm");
        let multipart = String::from_utf8_lossy(&sent[1].1);
        assert!(multipart.contains("name=\"language\"\r\n\r\nen\r\n"));
        assert!(!multipart.contains("en-US"));
    }

    #[test]
    fn provider_failure_preserves_status_without_echoing_sensitive_details() {
        let error = audio::AudioError {
            kind: audio::AudioErrorKind::Provider,
            dispatch: audio::AudioDispatch::Rejected,
            message: "private-canary".into(),
            source: Some(Box::new(
                sdk::providers::openai::audio::AudioError::Provider {
                    status: 429,
                    request_id: Some("private-canary".into()),
                    body: json!({"error":{"message":"private-canary"}}),
                },
            )),
        };
        let output = provider_failure(error);
        assert_eq!(
            output["error"]["message"],
            "audio provider returned HTTP 429"
        );
        assert!(!output.to_string().contains("private-canary"));
    }

    fn host() -> AudioProviderHost {
        AudioProviderHost::test_empty()
    }

    fn request(kind: &str) -> Value {
        json!({"operationId":"test-operation","kind":kind,
            "cloud":{"binding":"explicit_profile","profileId":"openai","modelId":null}})
    }

    #[test]
    fn following_session_requires_exact_profile_and_account() {
        let host = host();
        let mut value = request("speech");
        value["cloud"] = json!({"binding":"follow_session"});
        let parsed = host.parse(&value.to_string()).unwrap();
        assert_eq!(
            host.resolve(&parsed).unwrap_err()["error"]["kind"],
            "needs_configuration"
        );
        value["session"] =
            json!({"sessionId":"s1","profileId":"exact-profile","accountScope":"account-2"});
        let route = host
            .resolve(&host.parse(&value.to_string()).unwrap())
            .unwrap();
        assert_eq!(route.profile_name, "exact-profile");
        assert_eq!(route.account_scope, "account-2");
    }

    #[test]
    fn explicit_profile_ignores_chat_session_and_enforces_bounds() {
        let host = host();
        let mut value = request("speech");
        value["session"] = json!({"sessionId":"s1","profileId":"different","accountScope":"wrong"});
        let route = host
            .resolve(&host.parse(&value.to_string()).unwrap())
            .unwrap();
        assert_eq!(route.profile_name, "openai");
        assert_eq!(
            route.account_scope,
            lingxi_core::host::provider_accounts::UNBOUND_ACCOUNT_SCOPE
        );
        value["maxPayloadBytes"] = json!(MAX_AUDIO_BYTES + 1);
        assert!(host.parse(&value.to_string()).is_err());
    }

    #[tokio::test]
    async fn unsupported_model_and_rate_fail_before_credentials_or_dispatch() {
        let host = host();
        let mut value = request("speech");
        value["cloud"]["modelId"] = json!("chat-only-model");
        let output: Value =
            serde_json::from_str(&host.synthesize(&value.to_string(), "hello").await).unwrap();
        assert_eq!(output["error"]["kind"], "unsupported");
        value["cloud"]["modelId"] = Value::Null;
        value["rate"] = json!(2.0);
        let output: Value =
            serde_json::from_str(&host.synthesize(&value.to_string(), "hello").await).unwrap();
        assert_eq!(output["error"]["kind"], "unsupported");
        assert!(host.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_managed_directory_never_dispatches_with_a_base_credential() {
        let wire = Arc::new(NativeWireCapture::default());
        let host = AudioProviderHost::test_with_transport(wire.clone(), None);
        let result: Value = serde_json::from_str(
            &host
                .synthesize(&request("speech").to_string(), "hello")
                .await,
        )
        .unwrap();
        assert_eq!(result["error"]["kind"], "all_accounts_unavailable");
        assert!(wire.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn capability_preflight_does_not_expose_or_guess_credentials() {
        let host = host();
        let value = request("speech");
        let output: Value =
            serde_json::from_str(&host.capabilities(&value.to_string()).await).unwrap();
        assert_eq!(output["supported"], true);
        assert_eq!(output["readiness"], "needs_configuration");
        assert_eq!(output["profileId"], "openai");
        assert!(output.get("apiKey").is_none());
        assert!(host.pending.lock().unwrap().is_empty());
        let mut invalid = request("speech");
        invalid["cloud"]["modelId"] = json!("chat-only-model");
        let invalid: Value =
            serde_json::from_str(&host.capabilities(&invalid.to_string()).await).unwrap();
        assert_eq!(invalid["supported"], true);
        assert_eq!(invalid["readiness"], "needs_configuration");
        assert_eq!(invalid["reason"], "select a supported audio model");
    }
}
