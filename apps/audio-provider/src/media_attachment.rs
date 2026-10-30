//! Adapts one owner-bound media session to the current Agent realtime runner.
//!
//! Provider state and configuration arrive through host-owned registries. This
//! adapter never reserves an Agent turn or creates a second native capture.

use crate::{
    media_cloud::AudioProviderCloudRegistry,
    realtime::{prepare_provider_realtime, PreparedRealtimeProvider, Registration},
    AudioProviderHost, AudioRequest, CloudSelection, SessionContext,
};
use async_trait::async_trait;
use bytes::Bytes;
use llm_runtime::services::sdk;
use llm_runtime::services::sdk::realtime::{
    RealtimeAudioFormat, RealtimeClose, RealtimeControl, RealtimeDriver, RealtimeEvent,
    RealtimeEvents, RealtimeInput,
};
use media_api::{
    MediaAttachmentId, MediaAudioChannel, MediaEventCursor, MediaEventKind, MediaFrame,
    MediaFramePayload, MediaKind, MediaOperation, MediaOperationContext, MediaOperationId,
    MediaOperationKind, MediaOwner, MediaPcmFrame, MediaResourceRef, MediaResult, MediaService,
    MediaSessionRef, MediaSource, MediaSourceKind, MediaSourceRef, MediaSourceUsage,
};
use media_runtime::audio::Pcm16StreamConverter;
use media_runtime::{MediaAttachmentHandle, MediaAttachmentPermit};
use orchestrator::{
    MediaAgentCapabilityContext, MediaAgentHandoffRequest, MediaConversationAttachment,
    RealtimeAgentContext, RealtimeAgentInput, RealtimeAgentLimits, RealtimeAgentSessionIo,
    ScheduledMediaChild, ScheduledMediaChildRoute, ScheduledMediaParentCheck,
};
use serde::Deserialize;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
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

type AttachmentResolver = dyn Fn(&MediaAttachmentId) -> Option<MediaAttachmentHandle> + Send + Sync;

struct MediaPreparedProvider {
    control: RealtimeControl,
    events: RealtimeEvents,
    driver: RealtimeDriver,
    inputs: mpsc::Receiver<RealtimeAgentInput>,
    input: mpsc::Sender<RealtimeAgentInput>,
    input_format: RealtimeAudioFormat,
    output_format: RealtimeAudioFormat,
    max_chunk_bytes: usize,
    image_input_supported: bool,
    cancel: CancellationToken,
    registration: Option<Arc<Registration>>,
}

impl From<PreparedRealtimeProvider> for MediaPreparedProvider {
    fn from(value: PreparedRealtimeProvider) -> Self {
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
            _registration,
            ..
        } = value;
        Self {
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
            registration: Some(_registration),
        }
    }
}

#[cfg(test)]
type TestProviderConnector = dyn Fn(
        Arc<AudioProviderHost>,
        AudioRequest,
        RealtimeAgentContext,
    )
        -> Pin<Box<dyn Future<Output = Result<MediaPreparedProvider, serde_json::Value>> + Send>>
    + Send
    + Sync;

// `work` must be dropped before the permit when this future is cancelled.
// In particular, dropping a provider connect or driver releases its transport.
struct AttachmentFencedFuture<F> {
    work: Option<Pin<Box<F>>>,
    permit: Option<MediaAttachmentPermit>,
}

impl<F> AttachmentFencedFuture<F> {
    fn new(work: F, permit: MediaAttachmentPermit) -> Self {
        Self {
            work: Some(Box::pin(work)),
            permit: Some(permit),
        }
    }
}

impl<F: Future> Future for AttachmentFencedFuture<F> {
    type Output = (F::Output, MediaAttachmentPermit);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = match this
            .work
            .as_mut()
            .expect("fenced future polled twice")
            .as_mut()
            .poll(cx)
        {
            Poll::Ready(result) => result,
            Poll::Pending => return Poll::Pending,
        };
        // A completed future may still retain transport state until dropped.
        drop(this.work.take());
        Poll::Ready((result, this.permit.take().expect("fence permit present")))
    }
}

// Once connected, dropping this value must release the provider before the
// attachment permit if a validation branch returns early.
struct AttachmentFencedValue<T> {
    value: T,
    permit: MediaAttachmentPermit,
}

async fn prepare_with_attachment_fence<F: Future>(
    attachment: &MediaAttachmentHandle,
    permit: MediaAttachmentPermit,
    work: F,
) -> Option<AttachmentFencedValue<F::Output>> {
    // Biased selection stops polling the connector once closed admission is
    // observed. A connector poll already executing concurrently with revoke
    // cannot be preempted; the permit prevents a confirmed drain until its
    // future or resulting driver has released its local transport.
    let work = async {
        tokio::select! {
            biased;
            _ = attachment.wait_for_admission_closed() => None,
            result = work => Some(result),
        }
    };
    let (result, permit) = AttachmentFencedFuture::new(work, permit).await;
    result.map(|value| AttachmentFencedValue { value, permit })
}

// The revocation watcher lives with the driver, independently of the output
// supervisor. Poll both futures together: `abort` waits for the driver to
// acknowledge, so awaiting it while the driver is unpolled would deadlock.
async fn run_driver_until_closed(
    driver: RealtimeDriver,
    control: RealtimeControl,
    attachment: MediaAttachmentHandle,
) -> Result<(), sdk::realtime::RealtimeError> {
    let driver = driver.run();
    let revoke = async {
        attachment.wait_for_admission_closed().await;
        let _ = control
            .abort(RealtimeClose::normal("media attachment admission closed"))
            .await;
    };
    tokio::pin!(driver);
    tokio::pin!(revoke);
    tokio::select! {
        biased;
        _ = &mut revoke => driver.await,
        result = &mut driver => result,
    }
}

/// Connects the selected native realtime provider to a media session that
/// Harness has already opened for the current trusted Agent owner.
pub struct AppMediaConversationAttachment {
    providers: Arc<AudioProviderCloudRegistry>,
    attachment_for_id: Arc<AttachmentResolver>,
    configuration: AudioConfigurationSnapshot,
    #[cfg(test)]
    test_provider_connector: Option<Arc<TestProviderConnector>>,
    #[cfg(test)]
    test_supervisor_observed_output: Option<Arc<tokio::sync::Notify>>,
}

impl AppMediaConversationAttachment {
    /// Compose the exact provider registry, shared media service, and v4
    /// configuration snapshot callback used by UI and Agent conversation.
    #[must_use]
    pub fn new(
        providers: Arc<AudioProviderCloudRegistry>,
        attachment: MediaAttachmentHandle,
        configuration: AudioConfigurationSnapshot,
    ) -> Arc<Self> {
        let attachment_for_id = Arc::new(move |attachment_id: &MediaAttachmentId| {
            if attachment.id() != attachment_id || !attachment.admission_is_open() {
                return None;
            }
            Some(attachment.clone())
        });
        Arc::new(Self {
            providers,
            attachment_for_id,
            configuration,
            #[cfg(test)]
            test_provider_connector: None,
            #[cfg(test)]
            test_supervisor_observed_output: None,
        })
    }

    /// Compose against the live host attachment slot. Each operation receives
    /// the exact scoped service matching its trusted context ID; a stale context
    /// cannot follow the slot to a replacement attachment.
    #[must_use]
    pub fn new_for_attachment_slot(
        providers: Arc<AudioProviderCloudRegistry>,
        attachment_slot: Arc<std::sync::RwLock<Option<MediaAttachmentHandle>>>,
        configuration: AudioConfigurationSnapshot,
    ) -> Arc<Self> {
        let attachment_for_id = Arc::new(move |attachment_id: &MediaAttachmentId| {
            let attachment = attachment_slot
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone()?;
            if attachment.id() != attachment_id || !attachment.admission_is_open() {
                return None;
            }
            Some(attachment)
        });
        Arc::new(Self {
            providers,
            attachment_for_id,
            configuration,
            #[cfg(test)]
            test_provider_connector: None,
            #[cfg(test)]
            test_supervisor_observed_output: None,
        })
    }

    fn media_for_attachment(
        &self,
        attachment_id: &MediaAttachmentId,
    ) -> Option<Arc<dyn MediaService>> {
        (self.attachment_for_id)(attachment_id)
            .map(|attachment| attachment.service())
            .filter(|media| media.attachment_id().as_ref() == Some(attachment_id))
    }

    async fn prepare_realtime_provider(
        &self,
        provider: Arc<AudioProviderHost>,
        request: AudioRequest,
        prepared: RealtimeAgentContext,
    ) -> Result<MediaPreparedProvider, serde_json::Value> {
        #[cfg(test)]
        if let Some(connector) = &self.test_provider_connector {
            return connector(provider, request, prepared).await;
        }
        prepare_provider_realtime(&provider, request, prepared)
            .await
            .map(MediaPreparedProvider::from)
    }

    fn configured_route(
        &self,
        context: &MediaAgentCapabilityContext,
    ) -> Option<(
        Arc<AudioProviderHost>,
        AudioRequest,
        bool,
        Arc<dyn MediaService>,
    )> {
        let profile_name = context.profile_name.as_deref()?.trim();
        let account_scope = context.account_scope.as_deref()?.trim();
        if profile_name.is_empty()
            || account_scope.is_empty()
            || context.session_id.trim().is_empty()
        {
            return None;
        }
        let attachment_id = context.attachment_id.as_ref()?;
        let media = self.media_for_attachment(attachment_id)?;
        let provider = self
            .providers
            .provider_for_attached_session(
                attachment_id,
                &context.session_id,
                profile_name,
                account_scope,
            )
            .ok()?;
        let configuration = (self.configuration)().ok()?;
        if configuration.len() > 512 * 1024 {
            return None;
        }
        let snapshot: AudioConfigurationV4 = serde_json::from_str(&configuration).ok()?;
        let identity = MediaOperationId::new(0, 0);
        let prepared = RealtimeAgentContext {
            session_id: context.session_id.clone(),
            attachment_id: context.attachment_id.clone(),
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
        Some((provider, request, image_input_supported, media))
    }

    fn media_supports_live_video(media: &dyn MediaService, session_id: &str) -> bool {
        let owner = MediaOwner::Session {
            session_id: session_id.to_owned(),
        };
        let capabilities = media.capabilities_for(&owner);
        capabilities.supports(MediaOperationKind::SessionStart)
            && capabilities.supports(MediaOperationKind::Sources)
            && capabilities.supports(MediaOperationKind::Wait)
            && capabilities
                .supported_media_kinds
                .contains(&MediaKind::Video)
    }

    fn media_supports_live_audio(media: &dyn MediaService, session_id: &str) -> bool {
        let owner = MediaOwner::Session {
            session_id: session_id.to_owned(),
        };
        let capabilities = media.capabilities_for(&owner);
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

struct AppScheduledMediaChildRoute {
    providers: Arc<AudioProviderCloudRegistry>,
    parent: MediaAttachmentHandle,
    child: MediaAttachmentHandle,
}

impl ScheduledMediaChildRoute for AppScheduledMediaChildRoute {
    fn bind_session(
        &self,
        parent_session_id: &str,
        child_session_id: &str,
        profile_name: &str,
        account_scope: &str,
        parent_is_current: ScheduledMediaParentCheck,
    ) -> Result<(), String> {
        let parent_owner = MediaOwner::Session {
            session_id: parent_session_id.to_owned(),
        };
        let child_owner = MediaOwner::Session {
            session_id: child_session_id.to_owned(),
        };
        // Both permits fence the registry update against either attachment's
        // revocation; neither string can select a different host attachment.
        let parent_permit = self
            .parent
            .authorize_owner(&parent_owner)
            .map_err(|_| "the scheduled media parent is no longer active".to_owned())?;
        let child_permit = self
            .child
            .authorize_owner(&child_owner)
            .map_err(|_| "the scheduled media child is no longer active".to_owned())?;
        self.providers
            .register_scheduled_child_session(
                &parent_permit,
                &child_permit,
                profile_name,
                account_scope,
                parent_is_current,
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn release_after_media_drain(&self) -> Result<(), String> {
        if !self.child.media_drain_confirmed() {
            return Err("the scheduled media child drain is unconfirmed".into());
        }
        self.providers.unregister_attachment(self.child.id());
        Ok(())
    }
}

#[async_trait]
impl MediaConversationAttachment for AppMediaConversationAttachment {
    fn fork_scheduled_child(
        &self,
        parent: &MediaAttachmentHandle,
    ) -> Result<Option<ScheduledMediaChild>, String> {
        // Resolve the exact parent owned by this host adapter; mint the child
        // from that handle here so callers cannot substitute an unrelated scope.
        let parent = (self.attachment_for_id)(parent.id())
            .ok_or_else(|| "the scheduled media parent is no longer active".to_owned())?;
        let child = parent.fork_child().map_err(|error| error.to_string())?;
        let route = Arc::new(AppScheduledMediaChildRoute {
            providers: self.providers.clone(),
            parent,
            child: child.clone(),
        });
        Ok(Some(ScheduledMediaChild {
            attachment: child,
            route,
        }))
    }

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
        let Some((_, _, image_input_supported, media)) = self.configured_route(context) else {
            return Vec::new();
        };
        if !Self::media_supports_live_audio(media.as_ref(), &context.session_id) {
            return Vec::new();
        }
        let mut modes = vec![media_api::MediaSessionMode::Audio];
        if image_input_supported
            && Self::media_supports_live_video(media.as_ref(), &context.session_id)
        {
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
                self.configured_route(context)
                    .is_some_and(|(_, _, _, media)| {
                        Self::media_supports_live_audio(media.as_ref(), &context.session_id)
                    })
            }
            media_api::MediaSessionMode::AudioVideo => self.configured_route(context).is_some_and(
                |(_, _, image_input_supported, media)| {
                    image_input_supported
                        && Self::media_supports_live_video(media.as_ref(), &context.session_id)
                },
            ),
        }
    }

    async fn open(
        &self,
        request: MediaAgentHandoffRequest,
        prepared: RealtimeAgentContext,
        audio: Arc<dyn MediaAudioChannel>,
    ) -> Result<RealtimeAgentSessionIo, String> {
        let attachment_id = prepared.attachment_id.as_ref().ok_or_else(|| {
            "the current Agent engine attachment identity is unavailable".to_owned()
        })?;
        let attachment = (self.attachment_for_id)(attachment_id)
            .ok_or_else(|| "the current Agent attachment is no longer active".to_owned())?;
        let owner = MediaOwner::Session {
            session_id: prepared.session_id.clone(),
        };
        // Admission is atomic with begin_revoke. Hold this permit across the
        // entire provider connection and the spawned transport driver.
        let permit = attachment
            .authorize_owner(&owner)
            .map_err(|_| "the current Agent attachment is no longer active".to_owned())?;
        let media = attachment.service();
        let capabilities = media.capabilities_for(&owner);
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
            .provider_for_attached_session(
                attachment_id,
                &prepared.session_id,
                profile_name,
                account_scope,
            )
            .map_err(|_| {
                "the current Agent attachment has no matching provider binding".to_owned()
            })?;
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
        let first_read = async {
            tokio::select! {
                biased;
                _ = attachment.wait_for_admission_closed() => {
                    Err("the current Agent attachment is no longer active".to_owned())
                }
                result = timeout(Duration::from_secs(2), audio.read()) => result
                    .map_err(|_| "native media audio input did not become available".to_owned())?
                    .map_err(|error| error.message)?
                    .ok_or_else(|| "native media audio input ended before realtime setup".to_owned()),
            }
        };
        let (first_frame, permit) = AttachmentFencedFuture::new(first_read, permit).await;
        let first_frame = first_frame?;
        validate_capture_frame(&first_frame, capabilities.max_payload_bytes)?;
        let interaction = if request_context.interaction.as_deref() == Some("interruptible") {
            InteractionMode::Interruptible
        } else {
            InteractionMode::TurnBased
        };
        let media_session = request.session.clone();
        let max_media_payload_bytes = capabilities.max_payload_bytes;
        let connected = prepare_with_attachment_fence(
            &attachment,
            permit,
            self.prepare_realtime_provider(provider, request_context, prepared),
        )
        .await
        .ok_or_else(|| "the current Agent attachment is no longer active".to_owned())?;
        let AttachmentFencedValue { value, permit } = connected;
        let connected = value.map_err(|error| {
            error["error"]["message"]
                .as_str()
                .unwrap_or("native realtime provider setup failed")
                .to_owned()
        })?;
        let connected = AttachmentFencedValue {
            value: connected,
            permit,
        };
        if !attachment.admission_is_open() {
            // Dropping the unstarted driver releases its provider connection.
            return Err("the current Agent attachment is no longer active".into());
        }
        if requests_video && !connected.value.image_input_supported {
            // No driver is running yet; awaiting control.abort here would
            // wait forever for an acknowledgement that cannot be produced.
            return Err("the selected realtime provider cannot accept live video frames".into());
        }
        let AttachmentFencedValue {
            value: connected,
            permit,
        } = connected;
        let MediaPreparedProvider {
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
            registration,
        } = connected;
        let (output, output_rx) = mpsc::channel(AGENT_OUTPUT_CAPACITY);
        // Construct the fence before spawning: even an unpolled or aborted task
        // drops the driver (and its connection) before releasing the permit.
        let driver_work = AttachmentFencedFuture::new(
            run_driver_until_closed(driver, control.clone(), attachment.clone()),
            permit,
        );
        let driver = tokio::spawn(async move {
            let (result, permit) = driver_work.await;
            drop(permit);
            result
        });
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
            let media = media.clone();
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
        let mut output_task = tokio::spawn(async move {
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
        #[cfg(test)]
        let test_supervisor_observed_output = self.test_supervisor_observed_output.clone();
        tokio::spawn(async move {
            let output_result = tokio::select! {
                biased;
                _ = attachment.wait_for_admission_closed() => None,
                result = &mut output_task => Some(result),
            };
            #[cfg(test)]
            if matches!(&output_result, Some(Ok(Ok(())))) {
                if let Some(observed) = test_supervisor_observed_output {
                    observed.notify_one();
                }
            }
            supervisor_cancel.cancel();
            if !matches!(&output_result, Some(Ok(Ok(())))) {
                let _ = supervisor_control
                    .abort(RealtimeClose::normal("media Agent realtime closed"))
                    .await;
                let _ = cleanup_audio.interrupt().await;
            }
            if output_result.is_none() {
                wait_or_abort(output_task).await;
            }
            wait_or_abort(capture_task).await;
            if let Some(video_task) = video_task {
                wait_or_abort(video_task).await;
            }
            wait_or_abort(driver).await;
            if let Some(registration) = registration {
                registration.unregister();
            }
            if !matches!(&output_result, Some(Ok(Ok(())))) {
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
    use media_runtime::MediaAttachmentHost;
    use std::sync::atomic::AtomicU32;
    use tokio::sync::{Mutex as TokioMutex, Notify};

    #[derive(Default)]
    struct ScheduledAudioWire {
        requests: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl sdk::Transport for ScheduledAudioWire {
        async fn send(
            &self,
            request: sdk::HttpRequest,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            self.transcript(request.url)
        }
        async fn send_stream(
            &self,
            mut request: sdk::HttpStreamRequest,
        ) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            use futures::StreamExt;
            while let Some(chunk) = request.body.next().await {
                let _ = chunk?;
            }
            self.transcript(request.url)
        }
    }

    impl ScheduledAudioWire {
        fn transcript(&self, url: String) -> Result<sdk::StreamResponse, sdk::protocol::LlmError> {
            use futures::StreamExt;
            assert!(url.ends_with("/audio/transcriptions"));
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(sdk::StreamResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                body: futures::stream::once(async {
                    Ok(bytes::Bytes::from_static(
                        br#"{"text":"scheduled child transcript"}"#,
                    ))
                })
                .boxed(),
            })
        }
    }

    async fn scheduled_test_transcribe(
        store: &crate::media_store::BoundedMediaResourceStore,
        attachment: &MediaAttachmentHandle,
        owner: &MediaOwner,
        operation_id: u64,
    ) -> Result<MediaResult, MediaError> {
        use media_api::MediaResourceStore;
        let payload = media_api::MediaPayload {
            metadata: media_api::MediaMetadata {
                kind: MediaKind::Audio,
                mime_type: "audio/wav".into(),
                byte_len: 3,
                captured_at_unix_ms: None,
                duration_ms: None,
                width_px: None,
                height_px: None,
                sample_rate_hz: None,
                channels: None,
            },
            bytes: vec![1, 2, 3],
        };
        let scoped_context = MediaOperationContext::new(
            MediaOperationId::new(operation_id, 1),
            owner.clone().in_attachment(attachment.id().clone()),
        );
        let asset = store.put(&scoped_context, payload).await?;
        attachment
            .service()
            .execute(
                MediaOperationContext::new(
                    MediaOperationId::new(operation_id + 1, 1),
                    owner.clone(),
                ),
                MediaOperation::Transcribe {
                    input: media_api::TranscriptionInput::StoredMedia(asset.reference),
                    language: None,
                },
            )
            .await
    }

    struct IdleScheduledAudio;

    #[async_trait::async_trait]
    impl media_api::audio::AudioService for IdleScheduledAudio {
        fn capabilities(&self) -> media_api::audio::AudioCapabilitySnapshot {
            media_api::audio::AudioCapabilitySnapshot {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _: media_api::audio::AudioOperationContext,
            operation: media_api::audio::AudioOperation,
        ) -> Result<media_api::audio::AudioOperationSuccess, media_api::audio::AudioError> {
            match operation {
                media_api::audio::AudioOperation::Status { .. } => {
                    Ok(media_api::audio::AudioOperationSuccess::Status {
                        status: media_api::audio::AudioStatus::default(),
                    })
                }
                media_api::audio::AudioOperation::EndOwner => {
                    Ok(media_api::audio::AudioOperationSuccess::OwnerEnded)
                }
                _ => Err(media_api::audio::AudioError::new(
                    media_api::audio::AudioErrorKind::Unsupported,
                    "scheduled media fixture only supports idle status",
                )),
            }
        }

        async fn cancel(
            &self,
            _: media_api::audio::AudioOperationContext,
        ) -> Result<(), media_api::audio::AudioError> {
            Ok(())
        }
    }

    struct NoScheduledDevices;

    #[async_trait::async_trait]
    impl media_api::MediaDevicePorts for NoScheduledDevices {
        async fn open(
            &self,
            _: MediaOperationContext,
            _: media_api::MediaSessionRequest,
        ) -> Result<Arc<dyn media_api::MediaDeviceSession>, MediaError> {
            Err(MediaError::new(
                media_api::MediaErrorKind::Unavailable,
                "scheduled media fixture does not open devices",
            ))
        }
    }

    #[tokio::test]
    async fn scheduled_child_uses_parent_authority_with_independent_route_and_drain() {
        use media_api::audio::AudioOperationKind;
        use media_api::MediaReadinessState;
        use media_runtime::{
            AudioRouteMode, AudioRoutingSettings, CloudAudioHost, CloudBinding,
            MediaAttachmentRevocation,
        };

        let registry = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let mut routing = AudioRoutingSettings::default();
        routing.operations.insert(
            AudioOperationKind::Transcribe,
            AudioRouteMode::Cloud {
                binding: CloudBinding::FollowSession,
                model: None,
            },
        );
        let store = Arc::new(crate::media_store::BoundedMediaResourceStore::default());
        let app_host = crate::media_host::AppMediaHost::new(
            store.clone(),
            Arc::new(IdleScheduledAudio),
            None,
            Arc::new(NoScheduledDevices),
            Vec::new(),
            registry.clone(),
            Vec::new(),
            routing,
            1,
            1024,
        )
        .unwrap();
        let host = MediaAttachmentHost::new();
        let (parent, shared_registry) = app_host.create_mobile_media_attachment(&host).unwrap();
        assert!(Arc::ptr_eq(&registry, &shared_registry));
        let parent_owner = MediaOwner::Session {
            session_id: "parent-session".into(),
        };
        let wire = Arc::new(ScheduledAudioWire::default());
        let profiles = sdk::builtin_providers()
            .unwrap()
            .into_iter()
            .filter(|profile| profile.profile_name == "openai")
            .collect::<Vec<_>>();
        let client = sdk::LlmClientBuilder::with_transport(wire.clone(), &profiles)
            .with_region(sdk::protocol::Region::International)
            .build()
            .unwrap();
        let provider = Arc::new(AudioProviderHost {
            client,
            credential_ids: std::collections::BTreeMap::from([("openai".into(), "openai".into())]),
            storage: None,
            bound_services: None,
            keys: std::collections::BTreeMap::from([(
                "openai".into(),
                sdk::protocol::Secret::new("host-test-key".into()),
            )]),
            pending: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
        });
        let parent_permit = parent.authorize_owner(&parent_owner).unwrap();
        registry
            .register_attached_session(&parent_permit, "openai", "profile:openai", provider.clone())
            .unwrap();
        drop(parent_permit);
        let configuration: AudioConfigurationSnapshot = Arc::new(|| Ok("{}".into()));
        let adapter =
            AppMediaConversationAttachment::new(registry.clone(), parent.clone(), configuration);
        let (other_engine, _) = app_host.create_mobile_media_attachment(&host).unwrap();
        assert!(adapter.fork_scheduled_child(&other_engine).is_err());
        registry
            .register_profile("other", provider.clone())
            .unwrap();
        let ScheduledMediaChild {
            attachment: child,
            route,
        } = adapter.fork_scheduled_child(&parent).unwrap().unwrap();
        let ScheduledMediaChild {
            attachment: sibling,
            route: sibling_route,
        } = adapter.fork_scheduled_child(&parent).unwrap().unwrap();
        let mut child_fence = child.retain_for_host_cleanup().unwrap();
        let mut sibling_fence = sibling.retain_for_host_cleanup().unwrap();
        let child_owner = MediaOwner::Session {
            session_id: "parent-session".into(),
        };
        let sibling_owner = MediaOwner::Session {
            session_id: "new-scheduled-session".into(),
        };
        let readiness = |attachment: &MediaAttachmentHandle, owner: &MediaOwner| {
            attachment
                .service()
                .capabilities_for(owner)
                .readiness
                .into_iter()
                .find(|entry| entry.operation == MediaOperationKind::Transcribe)
                .map(|entry| entry.state)
        };
        assert_eq!(
            readiness(&child, &child_owner),
            Some(MediaReadinessState::Unavailable)
        );
        assert!(route
            .bind_session(
                "parent-session",
                "parent-session",
                "other",
                "profile:other",
                Arc::new(|| true)
            )
            .is_err());
        assert!(route
            .bind_session(
                "wrong-parent-session",
                "parent-session",
                "openai",
                "profile:openai",
                Arc::new(|| true),
            )
            .is_err());
        assert!(route
            .bind_session(
                "parent-session",
                "parent-session",
                "openai",
                "profile:other",
                Arc::new(|| true),
            )
            .is_err());
        let parent_current = Arc::new(AtomicBool::new(true));
        let current_check: ScheduledMediaParentCheck = {
            let parent_current = parent_current.clone();
            Arc::new(move || parent_current.load(Ordering::SeqCst))
        };
        route
            .bind_session(
                "parent-session",
                "parent-session",
                "openai",
                "profile:openai",
                current_check.clone(),
            )
            .unwrap();
        sibling_route
            .bind_session(
                "parent-session",
                "new-scheduled-session",
                "openai",
                "profile:openai",
                current_check,
            )
            .unwrap();
        assert_eq!(
            readiness(&parent, &parent_owner),
            Some(MediaReadinessState::Ready)
        );
        assert_eq!(
            readiness(&child, &child_owner),
            Some(MediaReadinessState::Ready)
        );
        assert_eq!(
            readiness(&sibling, &sibling_owner),
            Some(MediaReadinessState::Ready)
        );
        for (generation, attachment, owner) in
            [(1, &child, &child_owner), (2, &sibling, &sibling_owner)]
        {
            let service = attachment.service();
            let capabilities = service.capabilities_for(owner);
            assert!(matches!(
                service
                    .execute(
                        MediaOperationContext::new(
                            MediaOperationId::new(generation, capabilities.service_epoch),
                            owner.clone()
                        ),
                        MediaOperation::Sources {
                            kind: None,
                            usage: None
                        },
                    )
                    .await,
                Ok(MediaResult::Sources { .. })
            ));
            assert!(CloudAudioHost::resolve(
                registry.as_ref(),
                &owner.clone().in_attachment(attachment.id().clone()),
                &CloudBinding::FollowSession,
            )
            .is_ok());
        }
        assert!(matches!(
            scheduled_test_transcribe(&store, &child, &child_owner, 10).await,
            Ok(MediaResult::Transcript { .. })
        ));
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
        assert!(CloudAudioHost::resolve(
            registry.as_ref(),
            &child_owner.clone().in_attachment(child.id().clone()),
            &CloudBinding::ExplicitProfile {
                profile_name: "other".into()
            },
        )
        .is_err());
        child.begin_revoke();
        assert!(route.release_after_media_drain().is_err());
        assert!(
            registry
                .provider_for_attached_session(
                    child.id(),
                    "parent-session",
                    "openai",
                    "profile:openai",
                )
                .is_ok()
        );
        assert_eq!(
            child.drain_media(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        route.release_after_media_drain().unwrap();
        child_fence.confirm().unwrap();
        assert_eq!(
            child.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        assert!(
            registry
                .provider_for_attached_session(
                    child.id(),
                    "parent-session",
                    "openai",
                    "profile:openai",
                )
                .is_err()
        );
        assert!(registry
            .provider_for_attached_session(
                parent.id(),
                "parent-session",
                "openai",
                "profile:openai",
            )
            .is_ok());
        assert!(registry
            .provider_for_attached_session(
                sibling.id(),
                "new-scheduled-session",
                "openai",
                "profile:openai",
            )
            .is_ok());
        assert_eq!(
            readiness(&sibling, &sibling_owner),
            Some(MediaReadinessState::Ready)
        );

        assert!(matches!(
            scheduled_test_transcribe(&store, &parent, &parent_owner, 20).await,
            Ok(MediaResult::Transcript { .. })
        ));
        assert_eq!(wire.requests.load(Ordering::SeqCst), 2);

        // Even a route resolved before the change cannot fetch credentials
        // after its exact parent binding generation has been replaced.
        let cached_sibling_route = CloudAudioHost::resolve(
            registry.as_ref(),
            &sibling_owner.clone().in_attachment(sibling.id().clone()),
            &CloudBinding::FollowSession,
        )
        .unwrap();
        let stale_context = MediaOperationContext::new(
            MediaOperationId::new(3, 1),
            sibling_owner.clone().in_attachment(sibling.id().clone()),
        );
        let wrong_context = MediaOperationContext::new(
            MediaOperationId::new(4, 1),
            parent_owner.clone().in_attachment(parent.id().clone()),
        );
        let Err(wrong_error) = cached_sibling_route.request_options(&wrong_context).await else {
            panic!("a child route must not provide credentials for a different owner");
        };
        assert_eq!(
            wrong_error.kind,
            media_api::audio::AudioErrorKind::InvalidRequest
        );
        parent_current.store(false, Ordering::SeqCst);
        assert_eq!(
            readiness(&sibling, &sibling_owner),
            Some(MediaReadinessState::Unavailable)
        );
        assert_eq!(
            readiness(&parent, &parent_owner),
            Some(MediaReadinessState::Ready)
        );
        assert!(cached_sibling_route
            .request_options(&stale_context)
            .await
            .is_err());
        parent_current.store(true, Ordering::SeqCst);
        assert_eq!(
            readiness(&sibling, &sibling_owner),
            Some(MediaReadinessState::Ready)
        );
        // Rebinding the parent invalidates the previously derived child route,
        // even if the logical profile strings happen to remain the same.
        let permit = parent.authorize_owner(&parent_owner).unwrap();
        registry
            .register_attached_session(&permit, "openai", "profile:openai", provider)
            .unwrap();
        drop(permit);
        assert!(registry
            .provider_for_attached_session(
                sibling.id(),
                "new-scheduled-session",
                "openai",
                "profile:openai",
            )
            .is_err());
        assert_eq!(
            readiness(&sibling, &sibling_owner),
            Some(MediaReadinessState::Unavailable)
        );
        let Err(stale_error) = cached_sibling_route.request_options(&stale_context).await else {
            panic!("rebound parent must invalidate the cached scheduled route");
        };
        assert_eq!(
            stale_error.kind,
            media_api::audio::AudioErrorKind::Unavailable
        );
        sibling.begin_revoke();
        assert_eq!(
            sibling.drain_media(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        sibling_route.release_after_media_drain().unwrap();
        sibling_fence.confirm().unwrap();
        assert_eq!(
            sibling.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        assert!(registry
            .provider_for_attached_session(
                parent.id(),
                "parent-session",
                "openai",
                "profile:openai",
            )
            .is_ok());
        assert_eq!(
            parent.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        registry.unregister_attachment(parent.id());
        assert_eq!(
            other_engine.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn scheduled_child_timeout_retains_its_exact_route_until_confirmed_drain() {
        use media_runtime::MediaAttachmentRevocation;
        let fixture = open_fixture(false, false, false);
        let parent = fixture.attachment;
        let adapter = fixture.adapter;
        let ScheduledMediaChild {
            attachment: child,
            route,
        } = adapter.fork_scheduled_child(&parent).unwrap().unwrap();
        let mut fence = child.retain_for_host_cleanup().unwrap();
        route
            .bind_session(
                "open-session",
                "child-session",
                "openai",
                "profile:openai",
                Arc::new(|| true),
            )
            .unwrap();
        let owner = MediaOwner::Session {
            session_id: "child-session".into(),
        };
        let permit = child.authorize_owner(&owner).unwrap();
        child.begin_revoke();
        assert!(matches!(
            child.drain_media(Duration::from_millis(20)).await,
            MediaAttachmentRevocation::Unknown { .. }
        ));
        assert!(route.release_after_media_drain().is_err());
        assert!(adapter
            .providers
            .provider_for_attached_session(child.id(), "child-session", "openai", "profile:openai",)
            .is_ok());
        drop(permit);
        assert_eq!(
            child.drain_media(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        route.release_after_media_drain().unwrap();
        fence.confirm().unwrap();
        assert_eq!(
            child.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
        assert!(adapter
            .providers
            .provider_for_attached_session(child.id(), "child-session", "openai", "profile:openai",)
            .is_err());
        assert!(adapter
            .providers
            .provider_for_attached_session(parent.id(), "open-session", "openai", "profile:openai",)
            .is_ok());
        assert_eq!(
            parent.revoke(Duration::from_secs(2)).await,
            MediaAttachmentRevocation::Confirmed
        );
    }

    struct FailingScheduledStatus;

    #[async_trait::async_trait]
    impl MediaService for FailingScheduledStatus {
        fn capabilities(&self) -> media_api::MediaCapabilities {
            EmptyMediaService.capabilities()
        }
        async fn execute(
            &self,
            _: MediaOperationContext,
            _: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            Err(MediaError::new(
                media_api::MediaErrorKind::Unavailable,
                "status failed",
            ))
        }
        async fn read_media(
            &self,
            _: MediaOperationContext,
            _: media_api::MediaRef,
        ) -> Result<media_api::MediaContent, MediaError> {
            Err(MediaError::new(
                media_api::MediaErrorKind::Unavailable,
                "unused",
            ))
        }
        async fn cancel(&self, _: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn scheduled_child_unknown_drain_keeps_parent_and_child_routes() {
        let host = MediaAttachmentHost::new();
        let parent = host.attach(Arc::new(FailingScheduledStatus)).unwrap();
        let registry = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let provider = Arc::new(
            AudioProviderHost::new(
                "{}",
                "international",
                None,
                std::collections::BTreeMap::new(),
            )
            .unwrap(),
        );
        let parent_owner = MediaOwner::Session {
            session_id: "parent-session".into(),
        };
        let permit = parent.authorize_owner(&parent_owner).unwrap();
        registry
            .register_attached_session(&permit, "openai", "profile:openai", provider)
            .unwrap();
        drop(permit);
        let adapter = AppMediaConversationAttachment::new(
            registry.clone(),
            parent.clone(),
            Arc::new(|| Ok("{}".into())),
        );
        let ScheduledMediaChild {
            attachment: child,
            route,
        } = adapter.fork_scheduled_child(&parent).unwrap().unwrap();
        let mut fence = child.retain_for_host_cleanup().unwrap();
        route
            .bind_session(
                "parent-session",
                "child-session",
                "openai",
                "profile:openai",
                Arc::new(|| true),
            )
            .unwrap();
        let owner = MediaOwner::Session {
            session_id: "child-session".into(),
        };
        assert!(child
            .service()
            .execute(
                MediaOperationContext::new(MediaOperationId::new(1, 1), owner),
                MediaOperation::Status { target: None },
            )
            .await
            .is_err());
        child.begin_revoke();
        assert!(matches!(
            child.drain_media(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert_eq!(child.phase(), media_runtime::MediaAttachmentPhase::Unknown);
        assert!(route.release_after_media_drain().is_err());
        assert!(fence.confirm().is_err());
        assert!(registry
            .provider_for_attached_session(child.id(), "child-session", "openai", "profile:openai",)
            .is_ok());
        assert!(registry
            .provider_for_attached_session(
                parent.id(),
                "parent-session",
                "openai",
                "profile:openai",
            )
            .is_ok());
    }

    struct EmptyMediaService;

    #[async_trait::async_trait]
    impl media_api::MediaService for EmptyMediaService {
        fn capabilities(&self) -> media_api::MediaCapabilities {
            media_api::MediaCapabilities {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: vec![
                    MediaOperationKind::SessionStart,
                    MediaOperationKind::Status,
                ],
                supported_media_kinds: vec![MediaKind::Audio],
                readiness: Vec::new(),
                max_payload_bytes: 1024 * 1024,
            }
        }

        async fn execute(
            &self,
            _context: MediaOperationContext,
            operation: MediaOperation,
        ) -> Result<MediaResult, MediaError> {
            if matches!(operation, MediaOperation::Status { .. }) {
                return Ok(MediaResult::Status {
                    resources: Vec::new(),
                });
            }
            Err(MediaError::new(
                media_api::MediaErrorKind::Unsupported,
                "empty test media service",
            ))
        }

        async fn read_media(
            &self,
            _context: MediaOperationContext,
            _media: media_api::MediaRef,
        ) -> Result<media_api::MediaContent, MediaError> {
            Err(MediaError::new(
                media_api::MediaErrorKind::Unsupported,
                "empty test media service",
            ))
        }

        async fn cancel(&self, _context: MediaOperationContext) -> Result<(), MediaError> {
            Ok(())
        }
    }

    #[test]
    fn desktop_media_resolver_does_not_follow_old_handoff_after_slot_rotation() {
        let host = MediaAttachmentHost::new();
        let service: Arc<dyn MediaService> = Arc::new(EmptyMediaService);
        let old = host.attach(service.clone()).expect("old attachment");
        let replacement = host.attach(service).expect("replacement attachment");
        let slot = Arc::new(std::sync::RwLock::new(Some(old.clone())));
        let configuration: AudioConfigurationSnapshot = Arc::new(|| Ok("{}".to_owned()));
        let providers = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let adapter = AppMediaConversationAttachment::new_for_attachment_slot(
            providers.clone(),
            slot.clone(),
            configuration.clone(),
        );
        let fixed_adapter =
            AppMediaConversationAttachment::new(providers, old.clone(), configuration);

        let fixed_old_service = fixed_adapter
            .media_for_attachment(old.id())
            .expect("mobile binding resolves its exact active handle");
        assert_eq!(fixed_old_service.attachment_id().as_ref(), Some(old.id()));
        let old_service = adapter
            .media_for_attachment(old.id())
            .expect("current context resolves the old exact service");
        assert_eq!(old_service.attachment_id().as_ref(), Some(old.id()));

        old.begin_revoke();
        *slot.write().unwrap() = Some(replacement.clone());
        assert!(fixed_adapter.media_for_attachment(old.id()).is_none());
        assert!(adapter.media_for_attachment(old.id()).is_none());
        let replacement_service = adapter
            .media_for_attachment(replacement.id())
            .expect("new context resolves the replacement exact service");
        assert_eq!(
            replacement_service.attachment_id().as_ref(),
            Some(replacement.id()),
        );
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct ControlledAudioChannel {
        first: TokioMutex<Option<MediaFrame>>,
        pending_read_started: Notify,
        pending_read_dropped: Arc<AtomicBool>,
    }

    #[async_trait]
    impl MediaAudioChannel for ControlledAudioChannel {
        async fn read(&self) -> Result<Option<MediaFrame>, MediaError> {
            if let Some(frame) = self.first.lock().await.take() {
                return Ok(Some(frame));
            }
            let _read = DropFlag(self.pending_read_dropped.clone());
            self.pending_read_started.notify_one();
            std::future::pending::<Result<Option<MediaFrame>, MediaError>>().await
        }

        async fn write(
            &self,
            _frame: MediaPcmFrame,
        ) -> Result<media_api::MediaRenderReceipt, MediaError> {
            Ok(media_api::MediaRenderReceipt {
                accepted_sample_frames: 0,
                played_sample_frames: 0,
                rendered_at_unix_ms: Some(unix_now_ms()),
            })
        }

        async fn flush(&self) -> Result<media_api::MediaRenderReceipt, MediaError> {
            self.write(MediaPcmFrame {
                sample_rate_hz: 24_000,
                channels: 1,
                pcm16le: Vec::new(),
            })
            .await
        }

        async fn interrupt(&self) -> Result<media_api::MediaRenderReceipt, MediaError> {
            self.flush().await
        }
    }

    struct ProviderTestState {
        cancel: CancellationToken,
        supervisor_observed_output: Arc<Notify>,
        hold_connect: bool,
        connect_started: Notify,
        connect_release: Notify,
        connect_resumed: AtomicBool,
        connect_dropped: Arc<AtomicBool>,
        send_started: Notify,
        abort_started: Notify,
        abort_called: AtomicBool,
        abort_release: std::sync::Condvar,
        abort_allowed: Mutex<bool>,
        sink_dropped: AtomicBool,
    }

    impl ProviderTestState {
        fn new(hold_connect: bool, hold_abort: bool) -> Arc<Self> {
            Arc::new(Self {
                cancel: CancellationToken::new(),
                supervisor_observed_output: Arc::new(Notify::new()),
                hold_connect,
                connect_started: Notify::new(),
                connect_release: Notify::new(),
                connect_resumed: AtomicBool::new(false),
                connect_dropped: Arc::new(AtomicBool::new(false)),
                send_started: Notify::new(),
                abort_started: Notify::new(),
                abort_called: AtomicBool::new(false),
                abort_release: std::sync::Condvar::new(),
                abort_allowed: Mutex::new(!hold_abort),
                sink_dropped: AtomicBool::new(false),
            })
        }

        fn release_abort(&self) {
            *self.abort_allowed.lock().unwrap() = true;
            self.abort_release.notify_all();
        }
    }

    struct TestRealtimeSink(Arc<ProviderTestState>);

    impl Drop for TestRealtimeSink {
        fn drop(&mut self) {
            self.0.sink_dropped.store(true, Ordering::Release);
        }
    }

    #[async_trait]
    impl sdk::realtime::RealtimeSink for TestRealtimeSink {
        async fn send(
            &mut self,
            _frame: sdk::realtime::RealtimeFrame,
        ) -> Result<(), sdk::realtime::RealtimeError> {
            self.0.send_started.notify_one();
            Ok(())
        }
        async fn ping(&mut self, _payload: Bytes) -> Result<(), sdk::realtime::RealtimeError> {
            Ok(())
        }
        async fn close(
            &mut self,
            _close: RealtimeClose,
        ) -> Result<(), sdk::realtime::RealtimeError> {
            Ok(())
        }
        fn abort(&mut self) {
            self.0.abort_called.store(true, Ordering::Release);
            self.0.abort_started.notify_one();
            let mut allowed = self.0.abort_allowed.lock().unwrap();
            while !*allowed {
                allowed = self.0.abort_release.wait(allowed).unwrap();
            }
        }
    }

    struct TestRealtimeTransport(Arc<ProviderTestState>);

    #[async_trait]
    impl sdk::realtime::RealtimeTransport for TestRealtimeTransport {
        async fn connect(
            &self,
            _request: sdk::realtime::RealtimeConnectRequest,
        ) -> Result<sdk::realtime::RealtimeConnection, sdk::realtime::RealtimeError> {
            use futures::StreamExt;
            let _connect = DropFlag(self.0.connect_dropped.clone());
            self.0.connect_started.notify_one();
            if self.0.hold_connect {
                self.0.connect_release.notified().await;
                self.0.connect_resumed.store(true, Ordering::Release);
            }
            Ok(sdk::realtime::RealtimeConnection {
                outbound: Box::new(TestRealtimeSink(self.0.clone())),
                inbound: futures::stream::pending().boxed(),
            })
        }
    }

    struct TestRealtimeCodec;

    impl sdk::realtime::RealtimeCodec for TestRealtimeCodec {
        fn encode(
            &self,
            _input: &RealtimeInput,
        ) -> Result<Vec<sdk::realtime::RealtimeFrame>, sdk::realtime::RealtimeError> {
            Ok(vec![sdk::realtime::RealtimeFrame::text("test-frame")])
        }
        fn decode(
            &self,
            _frame: sdk::realtime::RealtimeFrame,
        ) -> Result<Vec<RealtimeEvent>, sdk::realtime::RealtimeError> {
            Ok(Vec::new())
        }
    }

    struct OpenFixture {
        attachment: MediaAttachmentHandle,
        adapter: Arc<AppMediaConversationAttachment>,
        request: MediaAgentHandoffRequest,
        prepared: RealtimeAgentContext,
        audio: Arc<ControlledAudioChannel>,
        provider: Arc<ProviderTestState>,
    }

    fn open_fixture(hold_first_frame: bool, hold_connect: bool, hold_abort: bool) -> OpenFixture {
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let providers = Arc::new(AudioProviderCloudRegistry::new("wav"));
        let owner = MediaOwner::Session {
            session_id: "open-session".into(),
        };
        let permit = attachment.authorize_owner(&owner).unwrap();
        let provider_host = Arc::new(
            AudioProviderHost::new(
                "{}",
                "international",
                None,
                std::collections::BTreeMap::new(),
            )
            .unwrap(),
        );
        providers
            .register_attached_session(&permit, "openai", "profile:openai", provider_host)
            .unwrap();
        drop(permit);
        let configuration: AudioConfigurationSnapshot = Arc::new(|| {
            Ok(
            r#"{"schemaVersion":4,"language":"auto","rate":1.0,"conversation":{"mode":"realtime","interaction":"turn_based","cloud":{"binding":"follow_session"}}}"#.into()
        )
        });
        let mut adapter =
            AppMediaConversationAttachment::new(providers, attachment.clone(), configuration);
        let provider = ProviderTestState::new(hold_connect, hold_abort);
        Arc::get_mut(&mut adapter)
            .unwrap()
            .test_supervisor_observed_output = Some(provider.supervisor_observed_output.clone());
        let provider_for_connect = provider.clone();
        Arc::get_mut(&mut adapter).unwrap().test_provider_connector = Some(Arc::new(
            move |_host: Arc<AudioProviderHost>,
                  _request: AudioRequest,
                  _prepared: RealtimeAgentContext|
                  -> Pin<
                Box<dyn Future<Output = Result<MediaPreparedProvider, serde_json::Value>> + Send>,
            > {
                let provider = provider_for_connect.clone();
                Box::pin(async move {
                    let transport = TestRealtimeTransport(provider.clone());
                    let (session, driver) = sdk::realtime::RealtimeSession::connect(
                        &transport,
                        sdk::realtime::RealtimeConnectRequest {
                            endpoint: "wss://test.invalid".into(),
                            headers: Vec::new(),
                            max_frame_bytes: 1024 * 1024,
                        },
                        Arc::new(TestRealtimeCodec),
                        sdk::realtime::RealtimeLimits::default(),
                    )
                    .await
                    .map_err(|error| crate::failure("provider_error", &error.to_string()))?;
                    let (control, events) = session.into_parts();
                    let (input, inputs) = mpsc::channel(32);
                    Ok(MediaPreparedProvider {
                        control,
                        events,
                        driver,
                        input,
                        inputs,
                        input_format: RealtimeAudioFormat::Pcm16 {
                            sample_rate_hz: 24_000,
                        },
                        output_format: RealtimeAudioFormat::Pcm16 {
                            sample_rate_hz: 24_000,
                        },
                        max_chunk_bytes: MAX_NATIVE_AUDIO_CHUNK_BYTES,
                        image_input_supported: true,
                        cancel: provider.cancel.clone(),
                        registration: None,
                    })
                })
            },
        ));
        let request = MediaAgentHandoffRequest {
            operation_id: MediaOperationId::new(1, 1),
            session: MediaSessionRef(media_api::MediaSessionId::from("open-session-media")),
            mode: media_api::MediaSessionMode::Audio,
        };
        let prepared = RealtimeAgentContext {
            session_id: "open-session".into(),
            profile_name: Some("openai".into()),
            account_scope: Some("profile:openai".into()),
            attachment_id: Some(attachment.id().clone()),
            instructions: String::new(),
            history: Vec::new(),
            tools: Vec::new(),
        };
        let audio = Arc::new(ControlledAudioChannel {
            first: TokioMutex::new((!hold_first_frame).then(|| captured_audio(1, 8_000))),
            pending_read_started: Notify::new(),
            pending_read_dropped: Arc::new(AtomicBool::new(false)),
        });
        OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        }
    }

    #[tokio::test]
    async fn real_open_cancels_a_held_first_frame_before_any_provider_connect() {
        let fixture = open_fixture(true, false, false);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let open_audio = audio.clone();
        let open = tokio::spawn(async move { adapter.open(request, prepared, open_audio).await });
        tokio::time::timeout(
            Duration::from_secs(1),
            audio.pending_read_started.notified(),
        )
        .await
        .expect("open must start the first audio read");
        attachment.begin_revoke();
        assert!(tokio::time::timeout(Duration::from_secs(1), open)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(audio.pending_read_dropped.load(Ordering::Acquire));
        assert!(!provider.connect_dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn real_open_cancels_a_held_sdk_connect_when_attachment_revokes() {
        let fixture = open_fixture(false, true, false);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let open = tokio::spawn(async move { adapter.open(request, prepared, audio).await });
        tokio::time::timeout(Duration::from_secs(1), provider.connect_started.notified())
            .await
            .expect("open must enter the held provider connect");
        attachment.begin_revoke();
        assert!(tokio::time::timeout(Duration::from_secs(1), open)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(provider.connect_dropped.load(Ordering::Acquire));
        assert!(!provider.sink_dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test]
    async fn real_open_prioritizes_revoke_when_connect_also_becomes_ready() {
        let fixture = open_fixture(false, true, false);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let mut open = Box::pin(adapter.open(request, prepared, audio));
        assert!(futures::poll!(open.as_mut()).is_pending());
        attachment.begin_revoke();
        provider.connect_release.notify_one();
        assert!(open.await.is_err());
        assert!(!provider.connect_resumed.load(Ordering::Acquire));
        assert!(provider.connect_dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_open_revoke_waits_until_the_sdk_driver_aborts_and_drops_its_sink() {
        let fixture = open_fixture(false, false, true);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let io = adapter.open(request, prepared, audio).await.unwrap();
        attachment.begin_revoke();
        let abort_started =
            tokio::time::timeout(Duration::from_secs(1), provider.abort_started.notified()).await;
        let pending = attachment.revoke(Duration::from_millis(20)).await;
        let sink_was_live = !provider.sink_dropped.load(Ordering::Acquire);
        // Always release the deliberately stalled fake before assertions, so a
        // failing assertion cannot strand a Tokio worker in a synchronous hook.
        provider.release_abort();
        assert!(abort_started.is_ok());
        assert!(matches!(
            pending,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert!(sink_was_live);
        assert_eq!(
            attachment.revoke(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert!(provider.abort_called.load(Ordering::Acquire));
        assert!(provider.sink_dropped.load(Ordering::Acquire));
        drop(io);
    }

    #[tokio::test]
    async fn real_open_revocation_aborts_driver_after_output_already_finished() {
        let fixture = open_fixture(false, false, false);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let io = adapter.open(request, prepared, audio).await.unwrap();

        // Cancelling the provider-side token causes render_agent_output to
        // return Ok. The test hook fires only once the supervisor has chosen
        // that output-completed branch, before revoke is requested.
        provider.cancel.cancel();
        tokio::time::timeout(
            Duration::from_secs(1),
            provider.supervisor_observed_output.notified(),
        )
        .await
        .expect("supervisor must first observe normal output completion");

        // The still-running SDK driver can send after the output task ended.
        // This ensures the test actually exercises the gap in the old design.
        io.control
            .send(RealtimeInput::Text("after output ended".into()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), provider.send_started.notified())
            .await
            .expect("the live driver must dispatch the test frame");
        assert!(!provider.sink_dropped.load(Ordering::Acquire));

        attachment.begin_revoke();
        tokio::time::timeout(Duration::from_secs(1), provider.abort_started.notified())
            .await
            .expect("revoke must abort before the two-second cleanup timeout");
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert!(provider.abort_called.load(Ordering::Acquire));
        assert!(provider.sink_dropped.load(Ordering::Acquire));
        drop(io);
    }

    #[tokio::test]
    async fn real_open_revoke_joins_driver_when_another_abort_has_already_started() {
        let fixture = open_fixture(false, false, false);
        let OpenFixture {
            attachment,
            adapter,
            request,
            prepared,
            audio,
            provider,
        } = fixture;
        let io = adapter.open(request, prepared, audio).await.unwrap();

        // This first poll sends abort but does not yield to the driver. The
        // revocation watcher will see an already-consumed abort sender and
        // must still poll and join the same driver to release the fence.
        let mut abort = Box::pin(io.control.abort(RealtimeClose::normal("other owner")));
        assert!(futures::poll!(abort.as_mut()).is_pending());
        attachment.begin_revoke();
        assert!(tokio::time::timeout(Duration::from_secs(1), abort)
            .await
            .expect("driver must acknowledge the first abort")
            .is_ok());
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert!(provider.abort_called.load(Ordering::Acquire));
        assert!(provider.sink_dropped.load(Ordering::Acquire));
        drop(io);
    }

    #[tokio::test]
    async fn runtime_shutdown_drops_real_open_driver_before_confirming_revoke() {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let fixture = open_fixture(false, false, false);
            let OpenFixture {
                attachment,
                adapter,
                request,
                prepared,
                audio,
                provider,
            } = fixture;
            let io = runtime
                .block_on(adapter.open(request, prepared, audio))
                .unwrap();
            ready_tx.send((attachment, provider)).ok().unwrap();
            shutdown_rx.recv().unwrap();
            // Keep the returned IO alive until after runtime shutdown so the
            // driver is not closed merely by dropping the output sender.
            runtime.shutdown_background();
            drop(io);
        });
        let (attachment, provider) = ready_rx.await.unwrap();
        attachment.begin_revoke();
        assert!(matches!(
            attachment.revoke(Duration::from_millis(20)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert!(!provider.sink_dropped.load(Ordering::Acquire));
        shutdown_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            attachment.revoke(Duration::from_secs(2)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed
        );
        assert!(provider.sink_dropped.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn held_provider_connect_fences_revoke_until_the_future_is_cancelled() {
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let owner = MediaOwner::Session {
            session_id: "held-connect".into(),
        };
        let permit = attachment.authorize_owner(&owner).unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let drop_flag = dropped.clone();
        let mut connect = Box::pin(prepare_with_attachment_fence(
            &attachment,
            permit,
            async move {
                let _transport = DropFlag(drop_flag);
                std::future::pending::<()>().await;
            },
        ));

        // Poll exactly once, then hold the connect without polling it again.
        assert!(futures::poll!(connect.as_mut()).is_pending());
        attachment.begin_revoke();
        assert!(matches!(
            attachment.revoke(Duration::from_millis(10)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert!(!dropped.load(Ordering::Acquire));
        drop(connect);
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed,
        );
    }

    #[tokio::test]
    async fn revoke_cancels_a_held_provider_connect_without_waiting_for_turn_cleanup() {
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let owner = MediaOwner::Session {
            session_id: "held-connect".into(),
        };
        let permit = attachment.authorize_owner(&owner).unwrap();
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let connect_started = started.clone();
        let drop_flag = dropped.clone();
        let task_attachment = attachment.clone();
        let task = tokio::spawn(async move {
            prepare_with_attachment_fence(&task_attachment, permit, async move {
                let _transport = DropFlag(drop_flag);
                connect_started.notify_one();
                std::future::pending::<()>().await;
            })
            .await
            .is_none()
        });
        started.notified().await;
        attachment.begin_revoke();
        assert!(tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap());
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed,
        );
    }

    #[tokio::test]
    async fn revoke_waits_for_spawned_provider_driver_to_drop_its_connection() {
        let host = MediaAttachmentHost::new();
        let attachment = host.attach(Arc::new(EmptyMediaService)).unwrap();
        let owner = MediaOwner::Session {
            session_id: "held-driver".into(),
        };
        let permit = attachment.authorize_owner(&owner).unwrap();
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let driver_started = started.clone();
        let drop_flag = dropped.clone();
        let work = AttachmentFencedFuture::new(
            async move {
                let _connection = DropFlag(drop_flag);
                driver_started.notify_one();
                std::future::pending::<()>().await;
            },
            permit,
        );
        let driver = tokio::spawn(async move {
            let (_, permit) = work.await;
            drop(permit);
        });
        started.notified().await;
        attachment.begin_revoke();
        assert!(matches!(
            attachment.revoke(Duration::from_millis(10)).await,
            media_runtime::MediaAttachmentRevocation::Unknown { .. }
        ));
        assert!(!dropped.load(Ordering::Acquire));
        driver.abort();
        assert!(driver.await.unwrap_err().is_cancelled());
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(
            attachment.revoke(Duration::from_secs(1)).await,
            media_runtime::MediaAttachmentRevocation::Confirmed,
        );
    }

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
