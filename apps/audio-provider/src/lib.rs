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

pub mod realtime;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionContext {
    pub session_id: String,
    pub profile_id: String,
    pub account_scope: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudSelection {
    pub binding: String,
    pub profile_id: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioRequest {
    pub operation_id: String,
    pub kind: String,
    pub cloud: CloudSelection,
    pub session: Option<SessionContext>,
    pub language: Option<String>,
    pub voice: Option<String>,
    pub rate: Option<f32>,
    pub interaction: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
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
    storage: Option<Arc<dyn lingxi_core::host::SecureStorage>>,
    bound_services: Option<llm_runtime::services::ProviderServices>,
    keys: BTreeMap<String, sdk::protocol::Secret<String>>,
    pending: Arc<Mutex<BTreeMap<String, CancellationToken>>>,
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
            storage: None,
            bound_services: Some(services),
            keys: BTreeMap::new(),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn new(
        profiles_json: &str,
        region: &str,
        storage: Option<Arc<dyn lingxi_core::host::SecureStorage>>,
        keys: BTreeMap<String, String>,
    ) -> Result<Self, String> {
        if profiles_json.len() > 512 * 1024 {
            return Err("provider configuration is too large".into());
        }
        let user_providers = if profiles_json.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(profiles_json).map_err(|_| "invalid provider configuration")?
        };
        let region = match region {
            "china" => sdk::protocol::Region::ChinaMainland,
            "international" => sdk::protocol::Region::International,
            _ => return Err("audio requires an explicit supported usage region".into()),
        };
        let assembled = provider_config::assemble_for_region(
            provider_config::AssembleInputs {
                anthropic_api_base: "https://api.anthropic.com".into(),
                anthropic_models: vec![],
                anthropic_has_api_key: false,
                anthropic_has_oauth: false,
                user_providers,
                routing: None,
            },
            region,
        );
        if !assembled.warnings.is_empty() {
            return Err("invalid or unsupported provider configuration".into());
        }
        let credential_ids = assembled
            .credential_sources
            .into_iter()
            .map(|source| (source.profile_name, source.credential_id))
            .collect();
        let host = llm_runtime::ModelRuntime::from_config(assembled.client_config)
            .map_err(|_| "cannot build audio provider configuration")?;
        let transport =
            http_client::provider_transport().map_err(|_| "audio transport is unavailable")?;
        // Reuse the runtime's complete authenticator registry. Rebuilding an SDK
        // client with only its default authenticators rejects valid OAuth/GCP
        // profiles even when the requested audio route uses a separate API key.
        let services = host
            .provider_services(region, Arc::new(transport))
            .map_err(|_| "cannot build audio provider services")?;
        let client = services.client().clone();
        Ok(Self {
            client,
            credential_ids,
            storage,
            bound_services: None,
            keys: keys
                .into_iter()
                .map(|(id, key)| (id, sdk::protocol::Secret::new(key)))
                .collect(),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

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
                (profile.clone(), format!("profile:{profile}"))
            }
            _ => return Err(failure("invalid_request", "unknown cloud audio binding")),
        };
        Ok(audio::AudioRoute::new(profile, account))
    }

    async fn credential(&self, profile: &str) -> Result<sdk::protocol::Secret<String>, Value> {
        if let Some(services) = &self.bound_services {
            return services
                .service_credential(profile)
                .await
                .map_err(|error| match error {
                    llm_runtime::LlmError::UnsupportedCapability { .. } => failure(
                        "unsupported",
                        "selected profile authentication cannot provide audio credentials",
                    ),
                    llm_runtime::LlmError::Authentication { .. } => failure(
                        "needs_configuration",
                        "configure the selected provider's credential",
                    ),
                    _ => failure(
                        "unavailable",
                        "the selected provider's credential source is unavailable",
                    ),
                });
        }
        let id = self.credential_ids.get(profile).ok_or_else(|| {
            failure(
                "needs_configuration",
                "audio profile has no credential source",
            )
        })?;
        if let Some(key) = self.keys.get(id) {
            return Ok(key.clone());
        }
        let store = self.storage.as_ref().ok_or_else(|| {
            failure(
                "needs_configuration",
                "configure the selected provider's credential",
            )
        })?;
        let account = if id == "anthropic" || id == "anthropic-api-key" {
            "anthropic-api-key".into()
        } else {
            format!("provider-key-{id}")
        };
        let data = store
            .retrieve("lingxi", &account)
            .await
            .map_err(|_| failure("unavailable", "audio credential storage is unavailable"))?
            .ok_or_else(|| {
                failure(
                    "needs_configuration",
                    "configure the selected provider's credential",
                )
            })?;
        let key = String::from_utf8(data.expose_secret_bytes().to_vec())
            .map_err(|_| failure("unavailable", "audio credential encoding is invalid"))?;
        if key.is_empty() {
            return Err(failure(
                "needs_configuration",
                "configure the selected provider's credential",
            ));
        }
        Ok(sdk::protocol::Secret::new(key))
    }

    pub async fn capabilities(&self, input: &str) -> String {
        let result = async {
            let request = self.parse(input)?;
            let route = self.resolve(&request)?;
            let caps = self.client.audio().capabilities(&route).map_err(|_| failure("unsupported", "the selected audio provider profile is unsupported"))?;
            let operation = operation(&request.kind)?;
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
            let credential_error = if supported && configuration_reason.is_none() { self.credential(&route.profile_name).await.err() } else { None };
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

    pub async fn transcribe(&self, input: &str, bytes: Vec<u8>, mime_type: &str) -> String {
        self.run(input, |request, route, options| async move {
            if request.kind != "recognition"
                || bytes.is_empty()
                || bytes.len() > request.max_payload_bytes
            {
                return Err(failure(
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
                _ => return Err(failure("unsupported", "unsupported recorded audio format")),
            };
            let output = self
                .client
                .audio()
                .transcribe(
                    &route,
                    audio::AudioInput::from_bytes(
                        format!("recording.{extension}"),
                        mime_type,
                        bytes,
                    ),
                    &audio::TranscriptionRequest {
                        model: request.cloud.model_id,
                        language: request.language,
                        raw_format: None,
                    },
                    &options,
                )
                .await
                .map_err(provider_failure)?;
            Ok(json!({"text":output.text,"language":output.language,"usage":output.usage,"usageContext":{"operationId":request.operation_id,"profileId":route.profile_name,"accountScope":route.account_scope,"modelId":output.model,"providerId":output.provider_id}}))
        })
        .await
    }

    pub async fn synthesize(&self, input: &str, text: &str) -> String {
        self.run(input, |request,route,options| async move {
            if request.kind != "speech" { return Err(failure("invalid_request", "synthesis requires speech configuration")); }
            if text.trim().is_empty() || text.len() > request.max_payload_bytes.min(MAX_REQUEST_BYTES) { return Err(failure("invalid_request", "synthesis requires bounded nonempty text")); }
            let mut synthesis = audio::SynthesisRequest::new(text);
            synthesis.model=request.cloud.model_id; synthesis.language=request.language; synthesis.voice=request.voice;
            let output=self.client.audio().synthesize(&route,&synthesis,&options).await.map_err(provider_failure)?;
            let audio::AudioOutput::Stream(stream)=output else { return Err(failure("unsupported", "this route returns a provider URL rather than playable PCM")); };
            let audio=stream.collect(request.max_payload_bytes).await.map_err(provider_failure)?;
            if audio.metadata.format != audio::AudioFormat::Pcm16Le || audio.metadata.channels != Some(1)
                || audio.bytes.is_empty() || audio.bytes.len()%2!=0 { return Err(failure("unsupported", "provider did not return PCM16 mono audio")); }
            let rate=audio.metadata.sample_rate_hz.filter(|rate| *rate>0).ok_or_else(|| failure("unavailable", "provider PCM audio is missing its sample rate"))?;
            Ok(json!({"pcmBase64":STANDARD.encode(audio.bytes),"sampleRateHz":rate,"usage":audio.usage,"usageContext":{"operationId":request.operation_id,"profileId":audio.metadata.route.profile_name,"accountScope":audio.metadata.route.account_scope,"modelId":audio.metadata.model,"providerId":audio.metadata.provider_id}}))
        }).await
    }

    async fn run<F, Fut>(&self, input: &str, execute: F) -> String
    where
        F: FnOnce(AudioRequest, audio::AudioRoute, sdk::RequestOptions) -> Fut,
        Fut: std::future::Future<Output = Result<Value, Value>>,
    {
        let result=async {
            let request=self.parse(input)?;
            let route=self.resolve(&request)?;
            let caps=self.client.audio().capabilities(&route).map_err(|_| failure("unsupported","audio profile is unavailable"))?;
            if !caps.supports(operation(&request.kind)?) { return Err(failure("unsupported","selected provider does not support this audio operation")); }
            let op = operation(&request.kind)?;
            let model = caps.model(op, request.cloud.model_id.as_deref())
                .map_err(|_| failure("unsupported", "selected audio model is unsupported"))?;
            if request.voice.as_ref().is_some_and(|voice| !model.voices.is_empty() && !model.voices.iter().any(|entry| &entry.id == voice)) {
                return Err(failure("unsupported", "selected audio voice is unsupported"));
            }
            if op == audio::AudioOperation::Synthesis && (!model.formats.contains(&audio::AudioFormat::Pcm16Le) || request.rate.is_some_and(|rate| !rate.is_finite() || (rate - 1.0).abs() > f32::EPSILON)) {
                return Err(failure("unsupported", "selected audio output format or speech rate is unsupported"));
            }
            let key=self.credential(&route.profile_name).await?;
            let options=sdk::RequestOptions {credential:Some(key),account_scope:Some(route.account_scope.clone()),total_timeout:Some(Duration::from_millis(request.timeout_ms)),..Default::default()};
            let identity=request.operation_id.clone();
            let cancel=CancellationToken::new();
            {
                let mut pending=self.pending.lock().unwrap_or_else(|e| e.into_inner());
                if pending.contains_key(&identity) { return Err(failure("busy","audio operation identity is already active")); }
                pending.insert(identity.clone(),cancel.clone());
            }
            let _guard=PendingGuard{host:self,identity};
            tokio::select! { biased;
                _=cancel.cancelled()=>Err(failure("cancelled","audio operation cancelled")),
                result=tokio::time::timeout(Duration::from_millis(request.timeout_ms),execute(request,route,options)) => result.unwrap_or_else(|_| Err(failure("timeout","audio operation deadline exceeded")))
            }
        }.await;
        result.unwrap_or_else(|error| error).to_string()
    }
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
pub fn failure(kind: &str, message: &str) -> Value {
    json!({"error":{"kind":kind,"message":message}})
}
fn provider_failure(error: audio::AudioError) -> Value {
    json!({"error":{"kind":"provider_error","message":"selected audio provider request failed","dispatch":error.dispatch}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> AudioProviderHost {
        AudioProviderHost::new("{}", "international", None, BTreeMap::new()).unwrap()
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
        assert_eq!(route.account_scope, "profile:openai");
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
