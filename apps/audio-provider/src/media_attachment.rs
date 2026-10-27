//! Adapts one owner-bound media session to the current Agent realtime runner.
//!
//! Provider state and configuration arrive through host-owned registries. This
//! adapter never reserves an Agent turn or creates a second native capture.

use crate::{
    media_cloud::AudioProviderCloudRegistry,
    realtime::{prepare_provider_realtime, PreparedRealtimeProvider},
    AudioProviderHost, AudioRequest, CloudSelection, SessionContext,
};
use async_trait::async_trait;
use bytes::Bytes;
use llm_runtime::services::sdk;
use llm_runtime::services::sdk::realtime::{
    RealtimeAudioFormat, RealtimeClose, RealtimeEvent, RealtimeInput,
};
use media_api::{
    MediaAudioChannel, MediaEventCursor, MediaEventKind, MediaFrame, MediaFramePayload, MediaKind,
    MediaOperation, MediaOperationContext, MediaOperationId, MediaOperationKind, MediaOwner,
    MediaPcmFrame, MediaResourceRef, MediaResult, MediaService, MediaSessionRef, MediaSource,
    MediaSourceKind, MediaSourceRef, MediaSourceUsage,
};
use media_runtime::audio::Pcm16StreamConverter;
use orchestrator::{
    MediaAgentCapabilityContext, MediaAgentHandoffRequest, MediaConversationAttachment,
    RealtimeAgentContext, RealtimeAgentInput, RealtimeAgentLimits, RealtimeAgentSessionIo,
};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const AGENT_OUTPUT_CAPACITY: usize = 32;
const MAX_NATIVE_AUDIO_CHUNK_BYTES: usize = 256 * 1024;
const MAX_IMAGE_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_IMAGE_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMAGE_INPUTS: usize = 512;
const LIVE_IMAGE_INTERVAL: Duration = Duration::from_millis(1000);
const LIVE_IMAGE_WAIT_MS: u64 = 1000;
const MEDIA_OBSERVE_LIMIT: u16 = 32;
const OUTPUT_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
const PENDING_AUDIO_PREROLL: Duration = Duration::from_millis(240);
const SPEECH_RMS_THRESHOLD: u64 = 700;
const SPEECH_MIN_DURATION: Duration = Duration::from_millis(80);
const SPEECH_ENDPOINT_SILENCE: Duration = Duration::from_millis(650);
const SPEECH_MAX_TURN: Duration = Duration::from_secs(25);
static NEXT_MEDIA_OPERATION: AtomicU64 = AtomicU64::new(1);

/// Synchronous read of the saved, bare audio-v4 settings snapshot.
pub type AudioConfigurationSnapshot = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

/// Connects the selected native realtime provider to a media session that
/// Harness has already opened for the current trusted Agent owner.
pub struct AppMediaConversationAttachment {
    providers: Arc<AudioProviderCloudRegistry>,
    media: Arc<dyn MediaService>,
    configuration: AudioConfigurationSnapshot,
}

impl AppMediaConversationAttachment {
    /// Compose the exact provider registry, shared media service, and v4
    /// configuration snapshot callback used by UI and Agent conversation.
    #[must_use]
    pub fn new(
        providers: Arc<AudioProviderCloudRegistry>,
        media: Arc<dyn MediaService>,
        configuration: AudioConfigurationSnapshot,
    ) -> Arc<Self> {
        Arc::new(Self {
            providers,
            media,
            configuration,
        })
    }

    fn configured_route(
        &self,
        context: &MediaAgentCapabilityContext,
    ) -> Option<(Arc<AudioProviderHost>, AudioRequest, bool)> {
        let profile_name = context.profile_name.as_deref()?.trim();
        let account_scope = context.account_scope.as_deref()?.trim();
        if profile_name.is_empty()
            || account_scope.is_empty()
            || context.session_id.trim().is_empty()
        {
            return None;
        }
        let provider = self
            .providers
            .provider_for_session(&context.session_id, profile_name, account_scope)
            .ok()?;
        let configuration = (self.configuration)().ok()?;
        if configuration.len() > 512 * 1024 {
            return None;
        }
        let snapshot: AudioConfigurationV4 = serde_json::from_str(&configuration).ok()?;
        let identity = MediaOperationId::new(0, 0);
        let prepared = RealtimeAgentContext {
            session_id: context.session_id.clone(),
            profile_name: context.profile_name.clone(),
            account_scope: context.account_scope.clone(),
            instructions: String::new(),
            history: Vec::new(),
            tools: Vec::new(),
        };
        let request = request_from_snapshot(
            &identity,
            &prepared,
            snapshot,
            media_api::DEFAULT_MAX_MEDIA_PAYLOAD_BYTES,
        )
        .ok()?;
        let image_input_supported = image_input_supported_for_request(&provider, &request)?;
        Some((provider, request, image_input_supported))
    }

    fn media_supports_live_video(&self, session_id: &str) -> bool {
        let owner = MediaOwner::Session {
            session_id: session_id.to_owned(),
        };
        let capabilities = self.media.capabilities_for(&owner);
        capabilities.supports(MediaOperationKind::SessionStart)
            && capabilities.supports(MediaOperationKind::Sources)
            && capabilities.supports(MediaOperationKind::Wait)
            && capabilities
                .supported_media_kinds
                .contains(&MediaKind::Video)
    }

    fn media_supports_live_audio(&self, session_id: &str) -> bool {
        let owner = MediaOwner::Session {
            session_id: session_id.to_owned(),
        };
        let capabilities = self.media.capabilities_for(&owner);
        capabilities.supports(MediaOperationKind::SessionStart)
            && capabilities
                .supported_media_kinds
                .contains(&MediaKind::Audio)
    }
}

fn image_input_supported_for_request(
    provider: &AudioProviderHost,
    request: &AudioRequest,
) -> Option<bool> {
    let route = provider.resolve(request).ok()?;
    let capabilities = provider
        .client_snapshot()
        .audio()
        .capabilities(&route)
        .ok()?;
    let model = capabilities
        .model(
            sdk::audio::AudioOperation::NativeRealtime,
            request.cloud.model_id.as_deref(),
        )
        .ok()?;
    if !capabilities.agent_conversation
        || request
            .voice
            .as_ref()
            .is_some_and(|voice| !model.voices.iter().any(|known| &known.id == voice))
    {
        return None;
    }
    // The current provider-neutral realtime input supports images only through
    // the OpenAI agent connector. The live connection repeats this check after
    // negotiating its actual codec capabilities.
    Some(capabilities.provider_id == "openai")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioConfigurationV4 {
    schema_version: u32,
    language: Option<String>,
    rate: Option<f32>,
    conversation: AudioConversationV4,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioConversationV4 {
    mode: String,
    interaction: String,
    cloud: CloudSelection,
    voice: Option<AudioVoiceV4>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioVoiceV4 {
    source: String,
    id: String,
    model_id: Option<String>,
    profile_id: Option<String>,
}

fn request_from_snapshot(
    operation_identity: &MediaOperationId,
    prepared: &RealtimeAgentContext,
    snapshot: AudioConfigurationV4,
    max_payload_bytes: u64,
) -> Result<AudioRequest, String> {
    if snapshot.schema_version != 4 {
        return Err("native Agent realtime requires audio configuration v4".into());
    }
    if snapshot.conversation.mode != "realtime" {
        return Err("the saved audio conversation mode does not select native realtime".into());
    }
    if !matches!(
        snapshot.conversation.interaction.as_str(),
        "turn_based" | "interruptible"
    ) {
        return Err("the saved realtime interaction mode is unsupported".into());
    }
    let rate = snapshot.rate.unwrap_or(1.0);
    if !rate.is_finite() || (rate - 1.0).abs() > f32::EPSILON {
        return Err("native realtime does not support a speech rate override".into());
    }

    let mut cloud = snapshot.conversation.cloud;
    let current_profile = prepared
        .profile_name
        .as_deref()
        .filter(|profile| !profile.trim().is_empty());
    let current_account_scope = prepared
        .account_scope
        .as_deref()
        .filter(|scope| !scope.trim().is_empty());
    match cloud.binding.as_str() {
        "follow_session" => {
            if current_profile.is_none() || current_account_scope.is_none() {
                return Err("the current Agent provider context is incomplete".into());
            }
            if let Some(selected) = cloud.profile_id.as_deref() {
                if current_profile != Some(selected) {
                    return Err(
                        "the saved realtime profile is stale for the current Agent session".into(),
                    );
                }
            }
            cloud.profile_id = None;
        }
        "explicit_profile" => {
            if cloud
                .profile_id
                .as_deref()
                .is_none_or(|profile| profile.trim().is_empty())
            {
                return Err("the saved realtime provider profile is missing".into());
            }
        }
        _ => return Err("the saved realtime cloud binding is invalid".into()),
    }

    let voice = if let Some(voice) = snapshot.conversation.voice {
        if voice.source != "provider" || voice.id.trim().is_empty() {
            return Err("native realtime requires a provider voice selection".into());
        }
        let selected_profile = if cloud.binding == "explicit_profile" {
            cloud.profile_id.as_deref()
        } else {
            current_profile
        };
        if voice
            .profile_id
            .as_deref()
            .is_some_and(|profile| Some(profile) != selected_profile)
        {
            return Err("the saved realtime voice belongs to another provider profile".into());
        }
        if voice.model_id.as_deref().is_some_and(|model| {
            cloud
                .model_id
                .as_deref()
                .is_some_and(|selected| selected != model)
        }) {
            return Err("the saved realtime voice belongs to another audio model".into());
        }
        if cloud.model_id.is_none() {
            cloud.model_id.clone_from(&voice.model_id);
        }
        Some(voice.id)
    } else {
        None
    };

    let session = SessionContext {
        session_id: prepared.session_id.clone(),
        profile_id: current_profile.unwrap_or_default().to_owned(),
        account_scope: current_account_scope.unwrap_or_default().to_owned(),
    };
    let operation_id = format!(
        "media:{}:{}:{}",
        operation_identity.id, operation_identity.generation, operation_identity.service_epoch,
    );
    if operation_id.len() > 256 {
        return Err("media Agent operation identity exceeds its bound".into());
    }
    let max_payload_bytes = usize::try_from(max_payload_bytes)
        .unwrap_or(usize::MAX)
        .min(12 * 1024 * 1024);
    if max_payload_bytes < 2 {
        return Err("the media host payload limit is too small for PCM16".into());
    }
    Ok(AudioRequest {
        operation_id,
        kind: "realtime".into(),
        cloud,
        session: Some(session),
        language: snapshot.language,
        voice,
        rate: Some(rate),
        interaction: Some(snapshot.conversation.interaction),
        timeout_ms: 60_000,
        max_payload_bytes,
    })
}

#[async_trait]
impl MediaConversationAttachment for AppMediaConversationAttachment {
    fn supported_modes(&self) -> Vec<media_api::MediaSessionMode> {
        // This is the structural union. The exact current session and selected
        // provider are checked by `supports_mode` at tool preflight time.
        vec![
            media_api::MediaSessionMode::Audio,
            media_api::MediaSessionMode::AudioVideo,
        ]
    }

    fn supported_modes_for_session(
        &self,
        context: &MediaAgentCapabilityContext,
    ) -> Vec<media_api::MediaSessionMode> {
        let Some((_, _, image_input_supported)) = self.configured_route(context) else {
            return Vec::new();
        };
        if !self.media_supports_live_audio(&context.session_id) {
            return Vec::new();
        }
        let mut modes = vec![media_api::MediaSessionMode::Audio];
        if image_input_supported && self.media_supports_live_video(&context.session_id) {
            modes.push(media_api::MediaSessionMode::AudioVideo);
        }
        modes
    }

    fn supports_mode(
        &self,
        context: &MediaAgentCapabilityContext,
        mode: media_api::MediaSessionMode,
    ) -> bool {
        match mode {
            media_api::MediaSessionMode::Video => false,
            media_api::MediaSessionMode::Audio => {
                self.configured_route(context).is_some()
                    && self.media_supports_live_audio(&context.session_id)
            }
            media_api::MediaSessionMode::AudioVideo => {
                self.configured_route(context)
                    .is_some_and(|(_, _, image_input_supported)| {
                        image_input_supported && self.media_supports_live_video(&context.session_id)
                    })
            }
        }
    }

    async fn open(
        &self,
        request: MediaAgentHandoffRequest,
        prepared: RealtimeAgentContext,
        audio: Arc<dyn MediaAudioChannel>,
    ) -> Result<RealtimeAgentSessionIo, String> {
        let owner = MediaOwner::Session {
            session_id: prepared.session_id.clone(),
        };
        let capabilities = self.media.capabilities_for(&owner);
        if request.operation_id.service_epoch != capabilities.service_epoch {
            return Err("media Agent handoff belongs to a stale service epoch".into());
        }
        if request.mode == media_api::MediaSessionMode::Video {
            return Err("native Agent realtime requires an audio-enabled media session".into());
        }
        let requests_video = request.mode == media_api::MediaSessionMode::AudioVideo;
        let configuration = (self.configuration)()?;
        if configuration.len() > 512 * 1024 {
            return Err("audio configuration snapshot exceeds its bound".into());
        }
        let snapshot: AudioConfigurationV4 = serde_json::from_str(&configuration)
            .map_err(|_| "audio configuration snapshot is invalid".to_owned())?;
        let request_context = request_from_snapshot(
            &request.operation_id,
            &prepared,
            snapshot,
            capabilities.max_payload_bytes,
        )?;
        let profile_name = prepared
            .profile_name
            .as_deref()
            .filter(|profile| !profile.trim().is_empty())
            .ok_or_else(|| "the current Agent provider profile is unavailable".to_owned())?;
        let account_scope = prepared
            .account_scope
            .as_deref()
            .filter(|scope| !scope.trim().is_empty())
            .ok_or_else(|| "the current Agent account scope is unavailable".to_owned())?;
        let provider = self
            .providers
            .provider_for_session(&prepared.session_id, profile_name, account_scope)
            .map_err(|_| "the current Agent session has no matching provider binding".to_owned())?;
        let image_input_supported = image_input_supported_for_request(&provider, &request_context)
            .ok_or_else(|| "the selected realtime profile or model is unsupported".to_owned())?;
        if requests_video && !image_input_supported {
            return Err("the selected realtime provider cannot accept live video frames".into());
        }
        if requests_video
            && (!capabilities.supports(MediaOperationKind::SessionStart)
                || !capabilities.supports(MediaOperationKind::Sources)
                || !capabilities.supports(MediaOperationKind::Wait)
                || !capabilities
                    .supported_media_kinds
                    .contains(&MediaKind::Video))
        {
            return Err("the media host cannot observe video for this Conversation session".into());
        }
        let first_frame = timeout(Duration::from_secs(2), audio.read())
            .await
            .map_err(|_| "native media audio input did not become available".to_owned())?
            .map_err(|error| error.message)?
            .ok_or_else(|| "native media audio input ended before realtime setup".to_owned())?;
        validate_capture_frame(&first_frame, capabilities.max_payload_bytes)?;
        let interaction = if request_context.interaction.as_deref() == Some("interruptible") {
            InteractionMode::Interruptible
        } else {
            InteractionMode::TurnBased
        };
        let media_session = request.session.clone();
        let max_media_payload_bytes = capabilities.max_payload_bytes;
        let connected = prepare_provider_realtime(&provider, request_context, prepared)
            .await
            .map_err(|error| {
                error["error"]["message"]
                    .as_str()
                    .unwrap_or("native realtime provider setup failed")
                    .to_owned()
            })?;
        if requests_video && !connected.image_input_supported {
            let _ = connected
                .control
                .abort(RealtimeClose::normal(
                    "selected realtime provider cannot accept images",
                ))
                .await;
            return Err("the selected realtime provider cannot accept live video frames".into());
        }
        let PreparedRealtimeProvider {
            control,
            events,
            driver,
            inputs,
            input,
            input_format,
            output_format,
            max_chunk_bytes,
            image_input_supported,
            cancel,
            _registration: registration,
            ..
        } = connected;
        let (output, output_rx) = mpsc::channel(AGENT_OUTPUT_CAPACITY);
        let driver = tokio::spawn(driver.run());
        let input_paused = Arc::new(AtomicBool::new(false));
        let playback = Arc::new(Mutex::new(PlaybackState::default()));
        let io_gate = Arc::new(AsyncMutex::new(()));
        let provider_control = control.clone();
        let capture_task = {
            let audio = audio.clone();
            let input = input.clone();
            let input_paused = input_paused.clone();
            let playback = playback.clone();
            let io_gate = io_gate.clone();
            let cancel = cancel.child_token();
            let provider_control = provider_control.clone();
            tokio::spawn(async move {
                let result = capture_audio(
                    audio,
                    first_frame,
                    input,
                    input_paused,
                    playback,
                    io_gate,
                    input_format,
                    max_chunk_bytes,
                    max_media_payload_bytes,
                    interaction,
                    cancel,
                )
                .await;
                if result.is_err() {
                    tracing::warn!(
                        "native media audio input ended; closing the Agent realtime session"
                    );
                    let _ = provider_control
                        .abort(RealtimeClose::normal("media audio capture failed"))
                        .await;
                }
                result
            })
        };
        let video_task = if requests_video {
            let media = self.media.clone();
            let input = input.clone();
            let owner = owner.clone();
            let session = media_session;
            let service_epoch = capabilities.service_epoch;
            let max_payload_bytes = max_media_payload_bytes;
            let cancel = cancel.child_token();
            let video_control = control.clone();
            Some(tokio::spawn(async move {
                let result = sample_live_video(
                    media,
                    owner,
                    session,
                    service_epoch,
                    max_payload_bytes,
                    input,
                    cancel,
                )
                .await;
                if result.is_err() {
                    tracing::warn!(
                        "live media video sampling failed; closing the Agent realtime session"
                    );
                    let _ = video_control
                        .abort(RealtimeClose::normal("media video sampling failed"))
                        .await;
                }
                result
            }))
        } else {
            None
        };
        let output_control = control.clone();
        let output_input = input.clone();
        let output_paused = input_paused.clone();
        let output_playback = playback.clone();
        let output_gate = io_gate.clone();
        let output_cancel = cancel.child_token();
        let cleanup_audio = audio.clone();
        let output_task = tokio::spawn(async move {
            render_agent_output(
                output_rx,
                audio,
                output_input,
                output_paused,
                output_playback,
                output_gate,
                output_control,
                output_format,
                max_chunk_bytes,
                interaction,
                output_cancel,
            )
            .await
        });

        let supervisor_cancel = cancel.clone();
        let supervisor_control = control.clone();
        tokio::spawn(async move {
            let output_result = output_task.await;
            supervisor_cancel.cancel();
            if !matches!(output_result, Ok(Ok(()))) {
                let _ = cleanup_audio.interrupt().await;
                let _ = supervisor_control
                    .abort(RealtimeClose::normal("media Agent realtime closed"))
                    .await;
            }
            wait_or_abort(capture_task).await;
            if let Some(video_task) = video_task {
                wait_or_abort(video_task).await;
            }
            wait_or_abort(driver).await;
            registration.unregister();
            if !matches!(output_result, Ok(Ok(()))) {
                tracing::debug!("media Agent realtime output pump ended with an error");
            }
        });

        let mut limits = RealtimeAgentLimits::default();
        limits.max_image_inputs = MAX_IMAGE_INPUTS;
        limits.max_image_bytes_per_input =
            usize::try_from(MAX_IMAGE_INPUT_BYTES).unwrap_or(usize::MAX);
        limits.max_image_bytes_per_session = MAX_IMAGE_TOTAL_BYTES;
        Ok(RealtimeAgentSessionIo {
            control,
            events,
            inputs,
            output,
            limits,
            image_input_supported,
        })
    }
}

#[derive(Clone, Copy)]
enum InteractionMode {
    TurnBased,
    Interruptible,
}

#[derive(Default)]
struct PlaybackState {
    item_id: Option<String>,
    sample_rate_hz: u32,
    accepted_sample_frames: u64,
    played_sample_frames: u64,
    generation: u64,
    interrupting: bool,
    interrupted: bool,
}

struct PendingCaptureChunk {
    bytes: Vec<u8>,
    duration: Duration,
    voiced: bool,
}

async fn capture_audio(
    audio: Arc<dyn MediaAudioChannel>,
    first_frame: MediaFrame,
    input: mpsc::Sender<RealtimeAgentInput>,
    input_paused: Arc<AtomicBool>,
    playback: Arc<Mutex<PlaybackState>>,
    io_gate: Arc<AsyncMutex<()>>,
    format: RealtimeAudioFormat,
    max_chunk_bytes: usize,
    max_payload_bytes: u64,
    interaction: InteractionMode,
    cancel: CancellationToken,
) -> Result<(), String> {
    let RealtimeAudioFormat::Pcm16 { sample_rate_hz } = format else {
        return Err("native realtime input format is not PCM16".into());
    };
    if !(8_000..=192_000).contains(&sample_rate_hz) {
        return Err("native realtime input sample rate is unsupported".into());
    }
    let mut converter: Option<Pcm16StreamConverter> = None;
    let mut source_geometry = None;
    let mut preroll = VecDeque::<PendingCaptureChunk>::new();
    let mut preroll_duration = Duration::ZERO;
    let mut speech_active = false;
    let mut speech_duration = Duration::ZERO;
    let mut silence_duration = Duration::ZERO;
    let mut turn_started = Instant::now();
    let mut first_frame = Some(first_frame);
    loop {
        let frame = if let Some(frame) = first_frame.take() {
            Some(frame)
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                frame = audio.read() => frame.map_err(|error| error.message)?,
            }
        };
        let Some(frame) = frame else {
            if let Some(converter) = converter.as_mut() {
                let tail = converter.finish().map_err(|error| error.to_string())?;
                if speech_active && speech_duration >= SPEECH_MIN_DURATION {
                    if !tail.is_empty() {
                        enqueue_audio(&input, tail, sample_rate_hz, max_chunk_bytes)?;
                    }
                    commit_input(&input)?;
                }
            }
            return Err("native media audio input ended during realtime".into());
        };
        validate_capture_frame(&frame, max_payload_bytes)?;
        let MediaFramePayload::Audio(pcm) = frame.payload else {
            return Err("media audio channel returned a non-audio frame".into());
        };
        let geometry = (pcm.sample_rate_hz, pcm.channels);
        if source_geometry.is_some_and(|previous| previous != geometry) {
            return Err("native media audio format changed during realtime".into());
        }
        source_geometry = Some(geometry);
        if converter.is_none() {
            converter = Some(
                Pcm16StreamConverter::new(pcm.sample_rate_hz, pcm.channels, sample_rate_hz, 1)
                    .map_err(|error| error.to_string())?,
            );
        }
        let stream = converter
            .as_mut()
            .ok_or_else(|| "native PCM converter did not initialize".to_owned())?;
        let max_input_frames = pcm.sample_rate_hz as usize;
        let input_frame_bytes = usize::from(pcm.channels) * 2;
        let input_chunk_bytes = max_input_frames.saturating_mul(input_frame_bytes);
        for input_chunk in pcm.pcm16le.chunks(input_chunk_bytes.max(input_frame_bytes)) {
            let aligned_len = input_chunk.len() - (input_chunk.len() % input_frame_bytes);
            if aligned_len == 0 {
                continue;
            }
            let converted = stream
                .push(&input_chunk[..aligned_len])
                .map_err(|error| error.to_string())?;
            let converted = converted_chunks(converted, max_chunk_bytes);
            for bytes in converted {
                let duration =
                    Duration::from_secs_f64((bytes.len() / 2) as f64 / f64::from(sample_rate_hz));
                if input_paused.load(Ordering::Acquire) {
                    preroll.clear();
                    preroll_duration = Duration::ZERO;
                    speech_active = false;
                    speech_duration = Duration::ZERO;
                    silence_duration = Duration::ZERO;
                    continue;
                }
                let voiced = pcm_rms(&bytes) >= SPEECH_RMS_THRESHOLD;
                let mut current_sent = false;
                if !speech_active {
                    preroll.push_back(PendingCaptureChunk {
                        bytes: bytes.clone(),
                        duration,
                        voiced,
                    });
                    preroll_duration = preroll_duration.saturating_add(duration);
                    while preroll_duration > PENDING_AUDIO_PREROLL {
                        if let Some(old) = preroll.pop_front() {
                            preroll_duration = preroll_duration.saturating_sub(old.duration);
                        } else {
                            break;
                        }
                    }
                    if !voiced {
                        continue;
                    }
                    speech_active = true;
                    speech_duration = Duration::ZERO;
                    silence_duration = Duration::ZERO;
                    turn_started = Instant::now();
                    if matches!(interaction, InteractionMode::Interruptible)
                        && playback_active(&playback)
                    {
                        interrupt_playback(&audio, &input, &playback, &io_gate, &cancel).await?;
                    }
                    while let Some(chunk) = preroll.pop_front() {
                        enqueue_audio(&input, chunk.bytes, sample_rate_hz, max_chunk_bytes)?;
                        if chunk.voiced {
                            speech_duration = speech_duration.saturating_add(chunk.duration);
                        }
                    }
                    preroll_duration = Duration::ZERO;
                    current_sent = true;
                }
                if !current_sent {
                    enqueue_audio(&input, bytes, sample_rate_hz, max_chunk_bytes)?;
                    speech_duration = speech_duration.saturating_add(duration);
                }
                if voiced {
                    silence_duration = Duration::ZERO;
                } else {
                    silence_duration = silence_duration.saturating_add(duration);
                }
                if speech_duration >= SPEECH_MIN_DURATION
                    && (silence_duration >= SPEECH_ENDPOINT_SILENCE
                        || turn_started.elapsed() >= SPEECH_MAX_TURN)
                {
                    commit_input(&input)?;
                    speech_active = false;
                    speech_duration = Duration::ZERO;
                    silence_duration = Duration::ZERO;
                    preroll.clear();
                    preroll_duration = Duration::ZERO;
                    if matches!(interaction, InteractionMode::TurnBased) {
                        input_paused.store(true, Ordering::Release);
                    }
                }
            }
        }
    }
}

fn validate_capture_frame(frame: &MediaFrame, max_payload_bytes: u64) -> Result<(), String> {
    frame
        .validate(max_payload_bytes)
        .map_err(|error| error.message)?;
    let MediaFramePayload::Audio(pcm) = &frame.payload else {
        return Err("media audio channel returned a non-audio frame".into());
    };
    if !(8_000..=192_000).contains(&pcm.sample_rate_hz)
        || !matches!(pcm.channels, 1 | 2)
        || pcm.pcm16le.is_empty()
        || !pcm
            .pcm16le
            .len()
            .is_multiple_of(usize::from(pcm.channels) * 2)
    {
        return Err("native media audio geometry is unsupported".into());
    }
    Ok(())
}

async fn render_agent_output(
    mut events: mpsc::Receiver<RealtimeEvent>,
    audio: Arc<dyn MediaAudioChannel>,
    input: mpsc::Sender<RealtimeAgentInput>,
    input_paused: Arc<AtomicBool>,
    playback: Arc<Mutex<PlaybackState>>,
    io_gate: Arc<AsyncMutex<()>>,
    control: llm_runtime::services::sdk::realtime::RealtimeControl,
    output_format: RealtimeAudioFormat,
    max_chunk_bytes: usize,
    interaction: InteractionMode,
    cancel: CancellationToken,
) -> Result<(), String> {
    let RealtimeAudioFormat::Pcm16 { sample_rate_hz } = output_format else {
        return Err("native realtime output format is not PCM16".into());
    };
    if !(8_000..=192_000).contains(&sample_rate_hz) {
        return Err("native realtime output sample rate is unsupported".into());
    }
    let mut interruption_pending = false;
    let mut interrupted_seen = false;
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            event = events.recv() => match event { Some(event) => event, None => break },
        };
        match event {
            RealtimeEvent::AudioDelta { .. }
                if !interrupted_seen && playback_was_interrupted(&playback) => {}
            RealtimeEvent::AudioDelta { .. } if interruption_pending && !interrupted_seen => {}
            RealtimeEvent::AudioDelta {
                data,
                format:
                    RealtimeAudioFormat::Pcm16 {
                        sample_rate_hz: event_rate,
                    },
                item_id,
            } => {
                interruption_pending = false;
                interrupted_seen = false;
                if event_rate != sample_rate_hz || data.is_empty() || data.len() % 2 != 0 {
                    return Err("provider realtime audio delta has an invalid PCM16 layout".into());
                }
                if data.len() > max_chunk_bytes.saturating_mul(4) {
                    return Err("provider realtime audio delta exceeds its payload bound".into());
                }
                let (previous_item, has_playback) = {
                    let state = playback
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (state.item_id.clone(), state.accepted_sample_frames > 0)
                };
                if previous_item.as_ref() != item_id.as_ref() && has_playback {
                    finish_playback_item(&audio, &input, &playback, &io_gate, &input_paused, false)
                        .await?;
                }
                let playback_generation = {
                    let _write_guard = io_gate.lock().await;
                    let mut state = playback
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if state.item_id.is_none() {
                        state.generation = state.generation.saturating_add(1);
                        state.interrupting = false;
                        state.interrupted = false;
                        state.item_id = item_id.clone();
                        state.sample_rate_hz = sample_rate_hz;
                        state.accepted_sample_frames = 0;
                        state.played_sample_frames = 0;
                    }
                    state.generation
                };
                for chunk in data.chunks(max_chunk_bytes - (max_chunk_bytes % 2)) {
                    write_audio_chunk(
                        &audio,
                        &playback,
                        &io_gate,
                        chunk.to_vec(),
                        sample_rate_hz,
                        max_chunk_bytes,
                        &cancel,
                        playback_generation,
                    )
                    .await?;
                }
                if matches!(interaction, InteractionMode::TurnBased) {
                    input_paused.store(true, Ordering::Release);
                }
            }
            RealtimeEvent::AudioDelta { .. } => {
                return Err("provider realtime emitted an unsupported audio format".into());
            }
            RealtimeEvent::TurnCompleted {
                turn_id,
                status: Some(status),
            } if matches!(status.as_str(), "cancelled" | "interrupted") => {
                let interrupted_status = status == "interrupted";
                handle_provider_interruption(
                    RealtimeEvent::TurnCompleted {
                        turn_id,
                        status: Some(status),
                    },
                    &audio,
                    &input,
                    &playback,
                    &io_gate,
                    &input_paused,
                    &cancel,
                    control.capabilities().audio_truncation,
                )
                .await?;
                interruption_pending = true;
                interrupted_seen = interrupted_status;
            }
            RealtimeEvent::TurnCompleted { .. } => {
                finish_playback_item(&audio, &input, &playback, &io_gate, &input_paused, true)
                    .await?;
                input_paused.store(false, Ordering::Release);
                interruption_pending = false;
                interrupted_seen = false;
            }
            event @ (RealtimeEvent::UserSpeechStarted | RealtimeEvent::Interrupted) => {
                let interrupted = matches!(&event, RealtimeEvent::Interrupted);
                handle_provider_interruption(
                    event,
                    &audio,
                    &input,
                    &playback,
                    &io_gate,
                    &input_paused,
                    &cancel,
                    control.capabilities().audio_truncation,
                )
                .await?;
                interruption_pending = true;
                interrupted_seen = interrupted;
            }
            RealtimeEvent::TurnStarted { .. } => {
                interruption_pending = false;
                interrupted_seen = false;
                let _write_guard = io_gate.lock().await;
                let mut state = playback
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.interrupting {
                    state.interrupted = false;
                }
            }
            RealtimeEvent::ProviderError { .. } | RealtimeEvent::ConnectionInterrupted { .. } => {
                return Err("native realtime provider output failed".into());
            }
            _ => {}
        }
    }
    if playback_active(&playback) {
        finish_playback_item(&audio, &input, &playback, &io_gate, &input_paused, false).await?;
    }
    let _ = control
        .close(RealtimeClose::normal("media realtime output ended"))
        .await;
    Ok(())
}

async fn write_audio_chunk(
    audio: &Arc<dyn MediaAudioChannel>,
    playback: &Arc<Mutex<PlaybackState>>,
    io_gate: &Arc<AsyncMutex<()>>,
    bytes: Vec<u8>,
    sample_rate_hz: u32,
    max_bytes: usize,
    cancel: &CancellationToken,
    playback_generation: u64,
) -> Result<(), String> {
    let mut offset_frames = 0usize;
    let total_frames = bytes.len() / 2;
    while offset_frames < total_frames {
        let remaining = &bytes[offset_frames * 2..];
        let remaining_frames = remaining.len() / 2;
        let write_bytes = remaining
            .len()
            .min(max_bytes - (max_bytes % 2))
            .min(remaining_frames.saturating_mul(2));
        let frame_count = write_bytes / 2;
        if frame_count == 0 {
            return Err("native realtime produced an empty render frame".into());
        }
        let write_guard = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            guard = io_gate.lock() => guard,
        };
        let can_write = {
            let state = playback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.generation == playback_generation && !state.interrupting && !state.interrupted
        };
        if !can_write {
            drop(write_guard);
            return Ok(());
        }
        let receipt = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = audio.write(MediaPcmFrame {
                sample_rate_hz,
                channels: 1,
                pcm16le: remaining[..write_bytes].to_vec(),
            }) => result.map_err(|error| error.message)?,
        };
        if usize::try_from(receipt.accepted_sample_frames).unwrap_or(usize::MAX) > frame_count {
            return Err("native renderer returned inconsistent playback progress".into());
        }
        let accepted = receipt.accepted_sample_frames as usize;
        offset_frames += accepted;
        let (generation_matches, interrupted, invalid_progress) = {
            let mut state = playback
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.generation != playback_generation {
                (false, false, false)
            } else {
                state.accepted_sample_frames = state
                    .accepted_sample_frames
                    .saturating_add(u64::from(receipt.accepted_sample_frames));
                state.played_sample_frames = state
                    .played_sample_frames
                    .saturating_add(u64::from(receipt.played_sample_frames));
                (
                    true,
                    state.interrupting || state.interrupted,
                    state.played_sample_frames > state.accepted_sample_frames,
                )
            }
        };
        drop(write_guard);
        if !generation_matches {
            return Ok(());
        }
        if invalid_progress {
            return Err("native renderer reported playback beyond accepted audio".into());
        }
        if interrupted {
            return Ok(());
        }
        if accepted == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    Ok(())
}

async fn finish_playback_item(
    audio: &Arc<dyn MediaAudioChannel>,
    input: &mpsc::Sender<RealtimeAgentInput>,
    playback: &Arc<Mutex<PlaybackState>>,
    io_gate: &Arc<AsyncMutex<()>>,
    input_paused: &AtomicBool,
    resume_input: bool,
) -> Result<(), String> {
    let (generation, item_id) = {
        let _write_guard = io_gate.lock().await;
        let mut state = playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.interrupting
            || state.interrupted
            || (state.item_id.is_none() && state.accepted_sample_frames == 0)
        {
            return Ok(());
        }
        if state.accepted_sample_frames == 0 {
            state.item_id = None;
            state.sample_rate_hz = 0;
            state.played_sample_frames = 0;
            state.generation = state.generation.saturating_add(1);
            return Ok(());
        }
        (state.generation, state.item_id.clone())
    };
    let receipt = match timeout(OUTPUT_FLUSH_TIMEOUT, audio.flush()).await {
        Ok(Ok(receipt)) => receipt,
        Ok(Err(_)) if playback_flush_preempted(playback, generation) => return Ok(()),
        Ok(Err(error)) => return Err(error.message),
        Err(_) if playback_flush_preempted(playback, generation) => return Ok(()),
        Err(_) => return Err("native playback completion timed out".to_owned()),
    };
    let (item_id, accepted, played) = {
        let mut state = playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != generation
            || state.interrupting
            || state.interrupted
            || state.item_id != item_id
        {
            return Ok(());
        }
        state.played_sample_frames = state
            .played_sample_frames
            .saturating_add(u64::from(receipt.played_sample_frames));
        let values = (
            state.item_id.clone(),
            state.accepted_sample_frames,
            state.played_sample_frames,
        );
        state.item_id = None;
        state.sample_rate_hz = 0;
        state.accepted_sample_frames = 0;
        state.played_sample_frames = 0;
        state.generation = state.generation.saturating_add(1);
        values
    };
    if played > accepted {
        return Err("native renderer reported playback beyond accepted audio".into());
    }
    if accepted > 0 && played < accepted {
        return Err("native renderer did not confirm playback completion".into());
    }
    if accepted > 0 {
        enqueue_input(input, RealtimeAgentInput::PlaybackCompleted { item_id })?;
    }
    if resume_input {
        input_paused.store(false, Ordering::Release);
    }
    Ok(())
}

fn playback_flush_preempted(playback: &Arc<Mutex<PlaybackState>>, generation: u64) -> bool {
    let state = playback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.generation != generation || state.interrupting || state.interrupted
}

struct PlaybackProgress {
    item_id: Option<String>,
    sample_rate_hz: u32,
    accepted_sample_frames: u64,
    played_sample_frames: u64,
}

/// Stop native output at an interrupt barrier and return only progress the
/// renderer confirmed. Repeated provider interruption events become no-ops
/// after the first call clears the active item.
async fn interrupt_native_output(
    audio: &Arc<dyn MediaAudioChannel>,
    playback: &Arc<Mutex<PlaybackState>>,
    io_gate: &Arc<AsyncMutex<()>>,
) -> Result<Option<PlaybackProgress>, String> {
    let generation = {
        let mut state = playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.interrupting || state.interrupted {
            return Ok(None);
        }
        state.interrupting = true;
        state.interrupted = true;
        state.generation
    };
    let _guard = io_gate.lock().await;
    let (item_id, sample_rate_hz, accepted_sample_frames, played_sample_frames) = {
        let mut state = playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != generation || !state.interrupting {
            return Ok(None);
        }
        if state.accepted_sample_frames == 0 {
            state.item_id = None;
            state.sample_rate_hz = 0;
            state.played_sample_frames = 0;
            state.generation = state.generation.saturating_add(1);
            state.interrupting = false;
            return Ok(None);
        }
        (
            state.item_id.clone(),
            state.sample_rate_hz,
            state.accepted_sample_frames,
            state.played_sample_frames,
        )
    };
    let receipt = audio.interrupt().await.map_err(|error| error.message)?;
    let progress = PlaybackProgress {
        item_id,
        sample_rate_hz,
        accepted_sample_frames,
        played_sample_frames: played_sample_frames
            .saturating_add(u64::from(receipt.played_sample_frames)),
    };
    {
        let mut state = playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.item_id = None;
        state.sample_rate_hz = 0;
        state.accepted_sample_frames = 0;
        state.played_sample_frames = 0;
        state.generation = state.generation.saturating_add(1);
        state.interrupting = false;
        state.interrupted = true;
    }
    drop(_guard);
    if progress.played_sample_frames > progress.accepted_sample_frames {
        return Err("native renderer reported playback beyond accepted audio".into());
    }
    Ok(Some(progress))
}

fn truncation_input(progress: PlaybackProgress) -> Option<RealtimeAgentInput> {
    let item_id = progress
        .item_id
        .filter(|_| progress.sample_rate_hz > 0 && progress.played_sample_frames > 0)?;
    let audio_end_ms = u32::try_from(
        progress.played_sample_frames.saturating_mul(1000) / u64::from(progress.sample_rate_hz),
    )
    .unwrap_or(u32::MAX);
    Some(RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
        item_id,
        content_index: 0,
        audio_end_ms,
    }))
}

async fn handle_provider_interruption(
    event: RealtimeEvent,
    audio: &Arc<dyn MediaAudioChannel>,
    input: &mpsc::Sender<RealtimeAgentInput>,
    playback: &Arc<Mutex<PlaybackState>>,
    io_gate: &Arc<AsyncMutex<()>>,
    input_paused: &AtomicBool,
    cancel: &CancellationToken,
    can_truncate: bool,
) -> Result<(), String> {
    let should_interrupt = match event {
        RealtimeEvent::TurnCompleted { status, .. } => {
            matches!(status.as_deref(), Some("cancelled" | "interrupted"))
        }
        RealtimeEvent::Interrupted | RealtimeEvent::UserSpeechStarted => true,
        _ => false,
    };
    if should_interrupt {
        if let Some(progress) = interrupt_native_output(audio, playback, io_gate).await? {
            if can_truncate {
                if let Some(truncate) = truncation_input(progress) {
                    enqueue_inputs_wait(input, vec![truncate], cancel).await?;
                }
            }
        }
    }
    input_paused.store(false, Ordering::Release);
    Ok(())
}

async fn interrupt_playback(
    audio: &Arc<dyn MediaAudioChannel>,
    input: &mpsc::Sender<RealtimeAgentInput>,
    playback: &Arc<Mutex<PlaybackState>>,
    io_gate: &Arc<AsyncMutex<()>>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let progress = interrupt_native_output(audio, playback, io_gate).await?;
    let mut inputs = Vec::with_capacity(2);
    if let Some(truncate) = progress.and_then(truncation_input) {
        inputs.push(truncate);
    }
    inputs.push(RealtimeAgentInput::Provider(RealtimeInput::Interrupt));
    enqueue_inputs_wait(input, inputs, cancel).await
}

fn commit_input(input: &mpsc::Sender<RealtimeAgentInput>) -> Result<(), String> {
    enqueue_inputs(
        input,
        [
            RealtimeAgentInput::Provider(RealtimeInput::CommitAudio),
            RealtimeAgentInput::Provider(RealtimeInput::ContinueResponse),
        ],
    )
}

fn enqueue_audio(
    input: &mpsc::Sender<RealtimeAgentInput>,
    bytes: Vec<u8>,
    sample_rate_hz: u32,
    max_chunk_bytes: usize,
) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let chunk_size = max_chunk_bytes.min(MAX_NATIVE_AUDIO_CHUNK_BYTES);
    let chunk_size = chunk_size - (chunk_size % 2);
    if chunk_size == 0 {
        return Err("native realtime audio payload bound is too small".into());
    }
    for chunk in bytes.chunks(chunk_size) {
        enqueue_input(
            input,
            RealtimeAgentInput::Provider(RealtimeInput::Audio {
                data: Bytes::copy_from_slice(chunk),
                format: RealtimeAudioFormat::Pcm16 { sample_rate_hz },
            }),
        )?;
    }
    Ok(())
}

fn enqueue_input(
    input: &mpsc::Sender<RealtimeAgentInput>,
    value: RealtimeAgentInput,
) -> Result<(), String> {
    input
        .try_send(value)
        .map_err(|_| "native realtime input queue is full or closed".into())
}

fn enqueue_inputs<const N: usize>(
    input: &mpsc::Sender<RealtimeAgentInput>,
    values: [RealtimeAgentInput; N],
) -> Result<(), String> {
    let permits = input
        .try_reserve_many(N)
        .map_err(|_| "native realtime input queue is full or closed".to_owned())?;
    for (permit, value) in permits.zip(values) {
        permit.send(value);
    }
    Ok(())
}

async fn enqueue_inputs_wait(
    input: &mpsc::Sender<RealtimeAgentInput>,
    values: Vec<RealtimeAgentInput>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let count = values.len();
    let permits = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        result = input.reserve_many(count) => {
            result.map_err(|_| "native realtime input queue is closed".to_owned())?
        }
    };
    for (permit, value) in permits.zip(values) {
        permit.send(value);
    }
    Ok(())
}

fn converted_chunks(bytes: Vec<u8>, max_bytes: usize) -> Vec<Vec<u8>> {
    let chunk_size = max_bytes.min(MAX_NATIVE_AUDIO_CHUNK_BYTES);
    let chunk_size = chunk_size - (chunk_size % 2);
    if chunk_size == 0 {
        return Vec::new();
    }
    bytes.chunks(chunk_size).map(<[u8]>::to_vec).collect()
}

fn pcm_rms(bytes: &[u8]) -> u64 {
    let mut squares = 0u128;
    let mut samples = 0u128;
    for sample in bytes.chunks_exact(2) {
        let value = i16::from_le_bytes([sample[0], sample[1]]) as i64;
        squares = squares.saturating_add((value * value) as u128);
        samples += 1;
    }
    if samples == 0 {
        return 0;
    }
    ((squares / samples) as f64).sqrt() as u64
}

fn playback_active(playback: &Arc<Mutex<PlaybackState>>) -> bool {
    playback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .accepted_sample_frames
        > 0
}

fn playback_was_interrupted(playback: &Arc<Mutex<PlaybackState>>) -> bool {
    playback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .interrupted
}

async fn sample_live_video(
    media: Arc<dyn MediaService>,
    owner: MediaOwner,
    session: MediaSessionRef,
    service_epoch: u64,
    max_payload_bytes: u64,
    input: mpsc::Sender<RealtimeAgentInput>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let target = MediaResourceRef::Session(session);
    let capabilities = media.capabilities_for(&owner);
    if capabilities.service_epoch != service_epoch
        || !capabilities.supports(MediaOperationKind::Sources)
        || !capabilities.supports(MediaOperationKind::Wait)
        || !capabilities
            .supported_media_kinds
            .contains(&MediaKind::Video)
    {
        return Err("the media service cannot discover and observe live video sources".into());
    }
    let source_context = media_context(&owner, service_epoch, max_payload_bytes, 1500);
    let sources = match media
        .execute(
            source_context,
            MediaOperation::Sources {
                kind: None,
                usage: Some(MediaSourceUsage::Session),
            },
        )
        .await
    {
        Ok(MediaResult::Sources { sources }) => sources,
        _ => return Err("the media host could not list authorized video sources".into()),
    };
    let allowed_video_sources = sources
        .into_iter()
        .filter(is_live_video_source)
        .map(|source| source.reference)
        .collect::<Vec<MediaSourceRef>>();
    if allowed_video_sources.is_empty() {
        return Err("the media session has no authorized video source".into());
    }
    let mut cursor: Option<MediaEventCursor> = None;
    let mut last_sample = Instant::now()
        .checked_sub(LIVE_IMAGE_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut images = 0usize;
    let mut bytes_sent = 0usize;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        if video_input_budget_reached(images, bytes_sent) {
            return Err("live video input budget exhausted".into());
        }
        let capabilities = media.capabilities_for(&owner);
        if capabilities.service_epoch != service_epoch
            || !capabilities.supports(MediaOperationKind::Wait)
        {
            return Err("the media service cannot observe the live video session".into());
        }
        let context = media_context(&owner, service_epoch, max_payload_bytes, 1500);
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = media.execute(context.clone(), MediaOperation::Wait {
                target: target.clone(),
                after: cursor,
                timeout_ms: LIVE_IMAGE_WAIT_MS,
                limit: MEDIA_OBSERVE_LIMIT,
            }) => result,
        };
        let batch = match result {
            Ok(MediaResult::Events { batch }) => batch,
            _ => return Err("the media video observation failed".into()),
        };
        if batch.validate().is_err() || batch.target != target {
            return Err("the media video observation returned an invalid event batch".into());
        }
        cursor = batch.next_cursor.or(cursor);
        if last_sample.elapsed() < LIVE_IMAGE_INTERVAL {
            continue;
        }
        let mut newest = None;
        for event in batch.events.iter().rev() {
            let MediaEventKind::Frame { source, asset } = &event.kind else {
                continue;
            };
            if !allowed_video_sources
                .iter()
                .any(|allowed| allowed == source)
            {
                return Err("the media session emitted a frame from an unauthorized source".into());
            }
            if asset.metadata.kind != MediaKind::Image
                || !timestamp_is_fresh(event.occurred_at_unix_ms, 2500)
            {
                continue;
            }
            let Some(captured_at_unix_ms) = asset.metadata.captured_at_unix_ms else {
                continue;
            };
            if !timestamp_is_fresh(captured_at_unix_ms, 2500) {
                continue;
            }
            newest = Some((asset.clone(), captured_at_unix_ms));
            break;
        }
        let Some((asset, captured_at_unix_ms)) = newest else {
            continue;
        };
        if asset.metadata.byte_len > MAX_IMAGE_INPUT_BYTES
            || asset.metadata.byte_len > max_payload_bytes
            || !matches!(
                asset.metadata.mime_type.as_str(),
                "image/jpeg" | "image/png" | "image/webp"
            )
        {
            continue;
        }
        let read_context = media_context(&owner, service_epoch, max_payload_bytes, 1500);
        let content = match tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = media.read_media(read_context, asset.reference.clone()) => result,
        } {
            Ok(content) => content,
            Err(_) => return Err("the media host could not read an authorized video frame".into()),
        };
        if content.reference != asset.reference
            || content.metadata.kind != MediaKind::Image
            || content.metadata.mime_type != asset.metadata.mime_type
            || content.metadata.captured_at_unix_ms != Some(captured_at_unix_ms)
            || content.validate(max_payload_bytes).is_err()
            || u64::try_from(content.bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES
            || !timestamp_is_fresh(captured_at_unix_ms, 2500)
        {
            return Err("the media host returned invalid or stale video bytes".into());
        }
        if input
            .try_send(RealtimeAgentInput::Provider(RealtimeInput::Image {
                data: Bytes::from(content.bytes),
                mime_type: content.metadata.mime_type,
            }))
            .is_ok()
        {
            last_sample = Instant::now();
            images += 1;
            bytes_sent = bytes_sent
                .saturating_add(usize::try_from(asset.metadata.byte_len).unwrap_or(usize::MAX));
        }
    }
}

fn video_input_budget_reached(images: usize, bytes_sent: usize) -> bool {
    images >= MAX_IMAGE_INPUTS || bytes_sent >= MAX_IMAGE_TOTAL_BYTES
}

fn is_live_video_source(source: &MediaSource) -> bool {
    source.usages.contains(&MediaSourceUsage::Session)
        && source
            .supported_operations
            .contains(&MediaOperationKind::SessionStart)
        && match source.kind {
            MediaSourceKind::Camera | MediaSourceKind::Screen => true,
            MediaSourceKind::Remote => source.media_kind == Some(MediaKind::Video),
            MediaSourceKind::Microphone | MediaSourceKind::Speaker | MediaSourceKind::Library => {
                false
            }
        }
}

fn media_context(
    owner: &MediaOwner,
    service_epoch: u64,
    max_payload_bytes: u64,
    timeout_ms: u64,
) -> MediaOperationContext {
    let generation = NEXT_MEDIA_OPERATION.fetch_add(1, Ordering::Relaxed).max(1);
    MediaOperationContext::new(
        MediaOperationId::new(generation, service_epoch),
        owner.clone(),
    )
    .with_timeout_budget_ms(timeout_ms)
    .with_max_payload_bytes(max_payload_bytes)
}

fn timestamp_is_fresh(timestamp: u64, max_age_ms: u64) -> bool {
    let now = unix_now_ms();
    timestamp <= now.saturating_add(100) && now.saturating_sub(timestamp) <= max_age_ms
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

async fn wait_or_abort<T: 'static>(task: JoinHandle<T>) {
    let mut task = task;
    if timeout(Duration::from_secs(2), &mut task).await.is_err() {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_api::{MediaError, MediaSourceId, MediaSourceRef};
    use std::sync::atomic::AtomicU32;
    use tokio::sync::{Mutex as TokioMutex, Notify};

    struct FakeAudioChannel {
        frames: TokioMutex<VecDeque<media_api::MediaFrame>>,
        writes: Mutex<Vec<MediaPcmFrame>>,
        flush_played_frames: AtomicU32,
        hold_flush: AtomicBool,
        cancel_flush_on_release: AtomicBool,
        flush_started: Notify,
        flush_release: Notify,
        interrupt_played_frames: AtomicU32,
        interrupt_calls: AtomicU32,
    }

    impl FakeAudioChannel {
        fn new(frames: Vec<media_api::MediaFrame>) -> Self {
            Self {
                frames: TokioMutex::new(frames.into()),
                writes: Mutex::new(Vec::new()),
                flush_played_frames: AtomicU32::new(0),
                hold_flush: AtomicBool::new(false),
                cancel_flush_on_release: AtomicBool::new(false),
                flush_started: Notify::new(),
                flush_release: Notify::new(),
                interrupt_played_frames: AtomicU32::new(0),
                interrupt_calls: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl MediaAudioChannel for FakeAudioChannel {
        async fn read(&self) -> Result<Option<media_api::MediaFrame>, MediaError> {
            Ok(self.frames.lock().await.pop_front())
        }

        async fn write(
            &self,
            frame: MediaPcmFrame,
        ) -> Result<media_api::MediaRenderReceipt, MediaError> {
            let accepted = frame.pcm16le.len() / (usize::from(frame.channels) * 2);
            self.writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(frame);
            Ok(media_api::MediaRenderReceipt {
                accepted_sample_frames: u32::try_from(accepted).unwrap_or(u32::MAX),
                played_sample_frames: 0,
                rendered_at_unix_ms: None,
            })
        }

        async fn flush(&self) -> Result<media_api::MediaRenderReceipt, MediaError> {
            self.flush_started.notify_one();
            if self.hold_flush.load(Ordering::Acquire) {
                self.flush_release.notified().await;
            }
            if self.cancel_flush_on_release.load(Ordering::Acquire) {
                return Err(MediaError::new(
                    media_api::MediaErrorKind::Cancelled,
                    "native flush was interrupted",
                ));
            }
            Ok(media_api::MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: self.flush_played_frames.swap(0, Ordering::AcqRel),
                rendered_at_unix_ms: Some(unix_now_ms()),
            })
        }

        async fn interrupt(&self) -> Result<media_api::MediaRenderReceipt, MediaError> {
            self.interrupt_calls.fetch_add(1, Ordering::AcqRel);
            Ok(media_api::MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: self.interrupt_played_frames.swap(0, Ordering::AcqRel),
                rendered_at_unix_ms: Some(unix_now_ms()),
            })
        }
    }

    fn captured_audio(sequence: u64, amplitude: i16) -> media_api::MediaFrame {
        let mut bytes = Vec::with_capacity(480 * 2);
        for _ in 0..480 {
            bytes.extend_from_slice(&amplitude.to_le_bytes());
        }
        media_api::MediaFrame {
            source: MediaSourceRef(MediaSourceId::from("test-microphone")),
            sequence,
            captured_at_unix_ms: unix_now_ms(),
            payload: MediaFramePayload::Audio(MediaPcmFrame {
                sample_rate_hz: 24_000,
                channels: 1,
                pcm16le: bytes,
            }),
        }
    }

    fn video_session_source(
        id: &str,
        kind: MediaSourceKind,
        media_kind: Option<MediaKind>,
        usages: Vec<MediaSourceUsage>,
    ) -> MediaSource {
        MediaSource {
            reference: MediaSourceRef(MediaSourceId::from(id)),
            kind,
            media_kind,
            label: id.to_owned(),
            usages,
            supported_operations: vec![MediaOperationKind::SessionStart],
            is_default: false,
        }
    }

    #[test]
    fn camera_image_source_is_allowed_for_live_video_session() {
        let camera = video_session_source(
            "media-runtime:camera",
            MediaSourceKind::Camera,
            Some(MediaKind::Image),
            vec![MediaSourceUsage::Capture, MediaSourceUsage::Session],
        );
        assert!(is_live_video_source(&camera));
        let library = video_session_source(
            "media-runtime:library",
            MediaSourceKind::Library,
            Some(MediaKind::Image),
            vec![MediaSourceUsage::Capture, MediaSourceUsage::ReadImage],
        );
        assert!(!is_live_video_source(&library));
        let remote_audio = video_session_source(
            "remote:audio",
            MediaSourceKind::Remote,
            Some(MediaKind::Audio),
            vec![MediaSourceUsage::Session],
        );
        assert!(!is_live_video_source(&remote_audio));
    }

    #[test]
    fn live_video_budget_exhaustion_is_terminal() {
        assert!(video_input_budget_reached(MAX_IMAGE_INPUTS, 0));
        assert!(video_input_budget_reached(0, MAX_IMAGE_TOTAL_BYTES));
        assert!(!video_input_budget_reached(
            MAX_IMAGE_INPUTS - 1,
            MAX_IMAGE_TOTAL_BYTES - 1
        ));
    }

    #[test]
    fn capture_geometry_is_rejected_before_provider_setup() {
        let mut frame = captured_audio(1, 1_000);
        if let MediaFramePayload::Audio(pcm) = &mut frame.payload {
            pcm.channels = 3;
        }
        assert!(validate_capture_frame(&frame, 1024 * 1024).is_err());
    }

    #[tokio::test]
    async fn capture_endpoint_ignores_short_silence_and_commits_after_endpoint() {
        let first_frame = captured_audio(1, 8_000);
        let mut frames: Vec<_> = (2..=4)
            .map(|sequence| captured_audio(sequence, 8_000))
            .collect();
        frames.extend((5..=8).map(|sequence| captured_audio(sequence, 0)));
        frames.push(captured_audio(9, 9_000));
        frames.extend((10..=42).map(|sequence| captured_audio(sequence, 0)));
        let audio = Arc::new(FakeAudioChannel::new(frames));
        let (input, mut inputs) = mpsc::channel(128);
        let mut capture = tokio::spawn(capture_audio(
            audio,
            first_frame,
            input,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(PlaybackState::default())),
            Arc::new(AsyncMutex::new(())),
            RealtimeAudioFormat::Pcm16 {
                sample_rate_hz: 24_000,
            },
            MAX_NATIVE_AUDIO_CHUNK_BYTES,
            1024 * 1024,
            InteractionMode::TurnBased,
            CancellationToken::new(),
        ));
        let mut commit_count = 0;
        let mut continue_count = 0;
        loop {
            tokio::select! {
                result = &mut capture => {
                    assert!(result.unwrap().is_err());
                    break;
                }
                value = inputs.recv() => {
                    let Some(RealtimeAgentInput::Provider(value)) = value else { continue };
                    match value {
                        RealtimeInput::CommitAudio => commit_count += 1,
                        RealtimeInput::ContinueResponse => continue_count += 1,
                        _ => {},
                    }
                }
            }
        }
        while let Ok(value) = inputs.try_recv() {
            if let RealtimeAgentInput::Provider(value) = value {
                match value {
                    RealtimeInput::CommitAudio => commit_count += 1,
                    RealtimeInput::ContinueResponse => continue_count += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(commit_count, 1);
        assert_eq!(continue_count, 1);
    }

    #[tokio::test]
    async fn playback_ack_waits_for_flush_confirmation() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio.flush_played_frames.store(480, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(4);
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-1".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 0,
            ..PlaybackState::default()
        }));
        let io_gate = Arc::new(AsyncMutex::new(()));
        let input_paused = AtomicBool::new(true);
        finish_playback_item(&audio, &input, &playback, &io_gate, &input_paused, true)
            .await
            .unwrap();
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::PlaybackCompleted { item_id: Some(item) }) if item == "item-1"
        ));
    }

    #[tokio::test]
    async fn playback_flush_rejects_progress_beyond_accepted_audio() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio.flush_played_frames.store(481, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(4);
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-overflow".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 0,
            ..PlaybackState::default()
        }));
        let result = finish_playback_item(
            &audio,
            &input,
            &playback,
            &Arc::new(AsyncMutex::new(())),
            &AtomicBool::new(true),
            true,
        )
        .await;
        assert!(result.is_err());
        assert!(inputs.try_recv().is_err());
    }

    #[tokio::test]
    async fn barge_in_interrupts_while_flush_is_pending_and_ignores_late_flush() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio.hold_flush.store(true, Ordering::Release);
        fake_audio
            .cancel_flush_on_release
            .store(true, Ordering::Release);
        fake_audio
            .interrupt_played_frames
            .store(120, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(4);
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-flushing".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 80,
            ..PlaybackState::default()
        }));
        let io_gate = Arc::new(AsyncMutex::new(()));
        let input_paused = AtomicBool::new(true);
        let flush_started = fake_audio.flush_started.notified();
        tokio::pin!(flush_started);
        let finishing = {
            let audio = audio.clone();
            let input = input.clone();
            let playback = playback.clone();
            let io_gate = io_gate.clone();
            tokio::spawn(async move {
                finish_playback_item(&audio, &input, &playback, &io_gate, &input_paused, false)
                    .await
            })
        };
        flush_started.await;

        let mut interrupting = {
            let audio = audio.clone();
            let input = input.clone();
            let playback = playback.clone();
            let io_gate = io_gate.clone();
            tokio::spawn(async move {
                interrupt_playback(
                    &audio,
                    &input,
                    &playback,
                    &io_gate,
                    &CancellationToken::new(),
                )
                .await
            })
        };
        let interruption_before_release = timeout(Duration::from_secs(1), &mut interrupting)
            .await
            .ok();
        let interrupted_before_flush_release = interruption_before_release.is_some();
        fake_audio.flush_release.notify_one();
        let finish_result = finishing.await.unwrap();
        assert!(finish_result.is_ok());
        let interrupt_result = match interruption_before_release {
            Some(result) => result,
            None => interrupting.await,
        };
        assert!(interrupt_result.unwrap().is_ok());
        assert!(
            interrupted_before_flush_release,
            "interrupt waited behind the pending flush"
        );
        assert_eq!(fake_audio.interrupt_calls.load(Ordering::Acquire), 1);
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
                item_id,
                audio_end_ms: 8,
                ..
            })) if item_id == "item-flushing"
        ));
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::Interrupt))
        ));
        assert!(inputs.try_recv().is_err());
        assert!(!playback_active(&playback));
    }

    #[tokio::test]
    async fn interrupt_truncates_to_confirmed_partial_playback_without_stopping_capture() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio
            .interrupt_played_frames
            .store(40, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(4);
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-live".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 80,
            ..PlaybackState::default()
        }));
        interrupt_playback(
            &audio,
            &input,
            &playback,
            &Arc::new(AsyncMutex::new(())),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
                item_id,
                audio_end_ms: 5,
                ..
            })) if item_id == "item-live"
        ));
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::Interrupt))
        ));
    }

    #[tokio::test]
    async fn cancelled_turn_then_interrupted_reconciles_partial_playback_once() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio
            .interrupt_played_frames
            .store(160, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(4);
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-cancelled".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 80,
            ..PlaybackState::default()
        }));
        let io_gate = Arc::new(AsyncMutex::new(()));
        let input_paused = AtomicBool::new(true);

        handle_provider_interruption(
            RealtimeEvent::TurnCompleted {
                turn_id: Some("turn-cancelled".into()),
                status: Some("cancelled".into()),
            },
            &audio,
            &input,
            &playback,
            &io_gate,
            &input_paused,
            &CancellationToken::new(),
            true,
        )
        .await
        .unwrap();
        handle_provider_interruption(
            RealtimeEvent::Interrupted,
            &audio,
            &input,
            &playback,
            &io_gate,
            &input_paused,
            &CancellationToken::new(),
            true,
        )
        .await
        .unwrap();

        assert_eq!(fake_audio.interrupt_calls.load(Ordering::Acquire), 1);
        assert!(!input_paused.load(Ordering::Acquire));
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
                item_id,
                audio_end_ms: 10,
                ..
            })) if item_id == "item-cancelled"
        ));
        assert!(inputs.try_recv().is_err());
        assert!(!playback_active(&playback));
    }

    #[tokio::test]
    async fn user_speech_interrupt_waits_for_input_capacity_without_aborting() {
        let fake_audio = Arc::new(FakeAudioChannel::new(Vec::new()));
        fake_audio
            .interrupt_played_frames
            .store(120, Ordering::Release);
        let audio: Arc<dyn MediaAudioChannel> = fake_audio.clone();
        let (input, mut inputs) = mpsc::channel(1);
        input
            .try_send(RealtimeAgentInput::Provider(RealtimeInput::CommitAudio))
            .unwrap();
        let playback = Arc::new(Mutex::new(PlaybackState {
            item_id: Some("item-barge-in".into()),
            sample_rate_hz: 24_000,
            accepted_sample_frames: 480,
            played_sample_frames: 0,
            ..PlaybackState::default()
        }));
        let io_gate = Arc::new(AsyncMutex::new(()));
        let input_paused = Arc::new(AtomicBool::new(true));
        let task = {
            let audio = audio.clone();
            let input = input.clone();
            let playback = playback.clone();
            let io_gate = io_gate.clone();
            let input_paused = input_paused.clone();
            tokio::spawn(async move {
                handle_provider_interruption(
                    RealtimeEvent::UserSpeechStarted,
                    &audio,
                    &input,
                    &playback,
                    &io_gate,
                    &input_paused,
                    &CancellationToken::new(),
                    true,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::CommitAudio))
        ));
        assert!(task.await.unwrap().is_ok());
        assert!(matches!(
            inputs.recv().await,
            Some(RealtimeAgentInput::Provider(RealtimeInput::TruncateAudio {
                item_id,
                audio_end_ms: 5,
                ..
            })) if item_id == "item-barge-in"
        ));
        assert!(inputs.try_recv().is_err());
        assert_eq!(fake_audio.interrupt_calls.load(Ordering::Acquire), 1);
        assert!(!input_paused.load(Ordering::Acquire));
    }
}
