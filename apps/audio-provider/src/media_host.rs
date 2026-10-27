//! App-scoped media facade shared by the Agent runtime, native UI, and
//! Remote Control adapter.

use async_trait::async_trait;
use media_api::{
    MediaAudioChannel, MediaCapabilities, MediaConnector, MediaConnectorId, MediaContent,
    MediaDevicePorts, MediaError, MediaErrorKind, MediaOperation, MediaOperationContext,
    MediaOperationId, MediaOwner, MediaRef, MediaResourceStore, MediaResult, MediaService,
    MediaSession, MediaSessionMode, MediaSessionRef, MediaSessionRequest, MediaSource,
    MediaSourceRef,
};
use media_runtime::{AudioRoutingSettings, OfflineAudioBackend, UnifiedMediaService};
use std::sync::{Arc, RwLock};

const LOCAL_MICROPHONE_SOURCE_ID: &str = "media-runtime:microphone";
const LOCAL_CAMERA_SOURCE_ID: &str = "media-runtime:camera";

struct RemoteConnectorState {
    connector: Option<Arc<dyn MediaConnector>>,
    supported_modes: Vec<MediaSessionMode>,
}

/// Dynamically installed product connector. The shared `MediaService` keeps
/// this one router for its lifetime while Remote Control installs or removes
/// the Firebase-backed adapter as the authenticated connection changes.
pub struct MediaConnectorRouter {
    local: Arc<media_runtime::LocalMediaConnector>,
    local_modes: RwLock<Vec<MediaSessionMode>>,
    remote: RwLock<RemoteConnectorState>,
    id: MediaConnectorId,
}

impl MediaConnectorRouter {
    /// Compose one always-available local connector with an optional aggregate
    /// product connector. Both share the exact app-scoped device/store inputs.
    #[must_use]
    pub fn new(
        local: Arc<media_runtime::LocalMediaConnector>,
        local_modes: Vec<MediaSessionMode>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            local,
            local_modes: RwLock::new(local_modes),
            remote: RwLock::new(RemoteConnectorState {
                connector: None,
                supported_modes: Vec::new(),
            }),
            id: MediaConnectorId(id.into()),
        }
    }

    /// Install the product aggregate and its currently connected route modes.
    pub fn install_remote(
        &self,
        connector: Arc<dyn MediaConnector>,
        supported_modes: Vec<MediaSessionMode>,
    ) {
        *self.remote.write().unwrap_or_else(|e| e.into_inner()) = RemoteConnectorState {
            connector: Some(connector),
            supported_modes,
        };
    }

    /// Update authenticated remote route modes without replacing the aggregate.
    pub fn update_remote_modes(&self, supported_modes: Vec<MediaSessionMode>) {
        let mut remote = self.remote.write().unwrap_or_else(|e| e.into_inner());
        remote.supported_modes = if remote.connector.is_some() {
            supported_modes
        } else {
            Vec::new()
        };
    }

    /// Remove the product aggregate while preserving local device sessions.
    pub fn clear_remote(&self) {
        *self.remote.write().unwrap_or_else(|e| e.into_inner()) = RemoteConnectorState {
            connector: None,
            supported_modes: Vec::new(),
        };
    }

    /// Replace the structurally supported local session modes after the native
    /// audio capability snapshot changes.
    pub fn update_local_modes(&self, modes: Vec<MediaSessionMode>) -> Result<(), MediaError> {
        self.local.update_supported_modes(modes.clone())?;
        *self.local_modes.write().unwrap_or_else(|e| e.into_inner()) = modes;
        Ok(())
    }

    /// Current union used by the shared runtime's capability projection.
    pub fn supported_modes(&self) -> Vec<MediaSessionMode> {
        let mut modes = self
            .local_modes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        modes.extend(
            self.remote
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .supported_modes
                .iter()
                .copied(),
        );
        deduplicate_modes(modes)
    }

    fn connectors(&self) -> (Arc<dyn MediaConnector>, Option<Arc<dyn MediaConnector>>) {
        (
            self.local.clone(),
            self.remote
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .connector
                .clone(),
        )
    }

    fn connector_matches_sources(sources: &[MediaSource], request: &MediaSessionRequest) -> bool {
        let selected = request
            .audio_source
            .iter()
            .chain(request.video_source.iter())
            .collect::<Vec<_>>();
        !selected.is_empty()
            && selected.iter().all(|reference| {
                sources.iter().any(|source| {
                    &source.reference == *reference
                        && source
                            .supported_operations
                            .contains(&media_api::MediaOperationKind::SessionStart)
                })
            })
    }

    fn local_matches_sources(
        additional_sources: &[MediaSource],
        request: &MediaSessionRequest,
    ) -> bool {
        let has_selection = request.audio_source.is_some() || request.video_source.is_some();
        has_selection
            && request
                .audio_source
                .as_ref()
                .is_none_or(|source| source.0 .0 == LOCAL_MICROPHONE_SOURCE_ID)
            && request.video_source.as_ref().is_none_or(|source| {
                source.0 .0 == LOCAL_CAMERA_SOURCE_ID
                    || additional_sources.iter().any(|candidate| {
                        candidate.reference == *source
                            && candidate
                                .supported_operations
                                .contains(&media_api::MediaOperationKind::SessionStart)
                    })
            })
    }

    fn selected_sources(request: &MediaSessionRequest) -> Vec<&MediaSourceRef> {
        request
            .audio_source
            .iter()
            .chain(request.video_source.iter())
            .collect()
    }
}

#[async_trait]
impl MediaConnector for MediaConnectorRouter {
    fn id(&self) -> MediaConnectorId {
        self.id.clone()
    }

    async fn sources(
        &self,
        context: MediaOperationContext,
    ) -> Result<Vec<MediaSource>, MediaError> {
        let (local, remote) = self.connectors();
        let mut sources = local.sources(context.clone()).await?;
        if let Some(remote) = remote {
            sources.extend(remote.sources(context).await?);
        }
        Ok(sources)
    }

    async fn start_session(
        &self,
        context: MediaOperationContext,
        request: MediaSessionRequest,
    ) -> Result<Box<dyn MediaSession>, MediaError> {
        if Self::selected_sources(&request).is_empty() {
            return self.local.start_session(context, request).await;
        }
        let (local, remote) = self.connectors();
        let local_sources = local.sources(context.clone()).await?;
        let matches_local = Self::local_matches_sources(&local_sources, &request);
        let Some(remote) = remote else {
            return if matches_local {
                local.start_session(context, request).await
            } else {
                Err(MediaError::new(
                    MediaErrorKind::NotFound,
                    "the selected source is not available on a configured media connector",
                ))
            };
        };
        let remote_sources = remote.sources(context.clone()).await?;
        let matches_remote = Self::connector_matches_sources(&remote_sources, &request);
        match (matches_local, matches_remote) {
            (true, false) => local.start_session(context, request).await,
            (false, true) => remote.start_session(context, request).await,
            (true, true) => Err(MediaError::new(
                MediaErrorKind::InvalidRequest,
                "a media source reference is ambiguous across connectors",
            )),
            (false, false) => Err(MediaError::new(
                MediaErrorKind::NotFound,
                "the selected source is not available on a configured media connector",
            )),
        }
    }
}

fn deduplicate_modes(modes: Vec<MediaSessionMode>) -> Vec<MediaSessionMode> {
    let mut result = Vec::new();
    for mode in modes {
        if !result.contains(&mode) {
            result.push(mode);
        }
    }
    result
}

/// One app process's common media service, native device ports, and resource
/// store. Agent sessions, native UI calls, and the Remote Control adapter all
/// keep this same object.
pub struct AppMediaHost {
    service: Arc<UnifiedMediaService>,
    ports: Arc<dyn MediaDevicePorts>,
    resources: Arc<dyn MediaResourceStore>,
    connector: Arc<MediaConnectorRouter>,
    cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
    audio_configuration_json: RwLock<Option<String>>,
}

impl AppMediaHost {
    /// Compose the shared media facade around the app's existing device lanes.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        resources: Arc<dyn MediaResourceStore>,
        audio: Arc<dyn media_api::audio::AudioService>,
        camera: Option<Arc<dyn media_api::camera::CameraControl>>,
        ports: Arc<dyn MediaDevicePorts>,
        local_modes: Vec<MediaSessionMode>,
        cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
        offline_audio: Vec<OfflineAudioBackend>,
        audio_routing: AudioRoutingSettings,
        service_epoch: u64,
        max_payload_bytes: u64,
    ) -> Result<Arc<Self>, MediaError> {
        let local = Arc::new(media_runtime::LocalMediaConnector::new(
            ports.clone(),
            resources.clone(),
            Vec::new(),
            local_modes.clone(),
        )?);
        let connector = Arc::new(MediaConnectorRouter::new(
            local,
            local_modes.clone(),
            "lingxi.app-media",
        ));
        let service = MediaRuntimeBuilderProduct::build(
            resources.clone(),
            audio,
            camera,
            connector.clone(),
            local_modes,
            cloud_audio.clone(),
            offline_audio,
            audio_routing,
            service_epoch,
            max_payload_bytes,
        );
        Ok(Arc::new(Self {
            service,
            ports,
            resources,
            connector,
            cloud_audio,
            audio_configuration_json: RwLock::new(None),
        }))
    }

    /// The exact app-scoped service injected into Agent runtimes and UI calls.
    #[must_use]
    pub fn media_service(self: &Arc<Self>) -> Arc<dyn MediaService> {
        // The app host itself is the service facade, so every consumer keeps
        // the same app-scoped ports/store/router and session registry alive.
        // The runtime instance is delegated to below.
        //
        // This method's receiver is changed to `&Arc<Self>` so the Arc can be
        // coerced without constructing a second service view.
        self.clone()
    }

    /// The same routed audio backend that powers MediaService, for legacy
    /// Computer Use consumers which must share route/lease authority.
    #[must_use]
    pub fn audio_service(&self) -> Arc<dyn media_api::audio::AudioService> {
        self.service.audio_service()
    }

    /// Native transport ports used by the configured RTC connector.
    #[must_use]
    pub fn device_ports(&self) -> Arc<dyn MediaDevicePorts> {
        self.ports.clone()
    }

    /// Host store used by tools and connector events for opaque media refs.
    #[must_use]
    pub fn resource_store(&self) -> Arc<dyn MediaResourceStore> {
        self.resources.clone()
    }

    /// Stable connector slot which Remote Control may install or clear.
    #[must_use]
    pub fn connector_router(&self) -> Arc<MediaConnectorRouter> {
        self.connector.clone()
    }

    /// Install the product's aggregate connector and publish only the session
    /// modes that its currently authenticated routes can actually start.
    /// Existing sessions retain their own connector-owned handles.
    pub fn install_connector(
        &self,
        connector: Arc<dyn MediaConnector>,
        supported_modes: impl IntoIterator<Item = MediaSessionMode>,
    ) {
        self.connector
            .install_remote(connector, supported_modes.into_iter().collect());
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Replace the active aggregate's route-derived mode union after a viewer
    /// route is registered or removed without replacing its connector Arc.
    pub fn update_remote_session_modes(&self, supported_modes: Vec<MediaSessionMode>) {
        self.connector.update_remote_modes(supported_modes);
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Remove future source discovery and session starts while allowing any
    /// explicitly stopped or still-owned session handles to finish teardown.
    pub fn clear_connector(&self) {
        self.connector.clear_remote();
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
    }

    /// Apply new owner-independent local session support while retaining the
    /// authenticated product aggregate and its route-derived modes.
    pub fn update_local_session_modes(
        &self,
        modes: Vec<MediaSessionMode>,
    ) -> Result<(), MediaError> {
        self.connector.update_local_modes(modes)?;
        self.service.update_connector(
            Some(self.connector.clone()),
            self.connector.supported_modes(),
        );
        Ok(())
    }

    /// Trusted provider/session registry used by the common audio route policy.
    #[must_use]
    pub fn cloud_audio_registry(&self) -> Arc<super::media_cloud::AudioProviderCloudRegistry> {
        self.cloud_audio.clone()
    }

    /// Register an authenticated incoming session under its original owner.
    pub async fn attach_incoming_session(
        &self,
        context: MediaOperationContext,
        session: Box<dyn MediaSession>,
    ) -> Result<MediaSessionRef, MediaError> {
        self.service.attach_session(context, session).await
    }

    /// Resolve an already attached session's owner-checked PCM bridge. This
    /// returns the existing RTC/device channel and never opens another lease.
    pub async fn session_audio(
        &self,
        context: MediaOperationContext,
        session: MediaSessionRef,
    ) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        self.service.session_audio(context, session).await
    }

    /// Change the active audio-v4 route snapshot without replacing the service
    /// or its resource/session registries.
    pub fn update_audio_routing(&self, settings: AudioRoutingSettings) {
        self.service.update_audio_routing(settings);
    }

    /// Parse and atomically apply one current v4 settings snapshot. Invalid
    /// JSON/schema never replaces the last active route table.
    pub fn update_audio_routing_v4(&self, configuration_json: &str) -> Result<(), String> {
        let settings = super::media_configuration::audio_routing_settings_from_v4_json_checked(
            configuration_json,
        )?;
        self.update_audio_routing(settings);
        *self
            .audio_configuration_json
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(configuration_json.to_owned());
        Ok(())
    }

    /// Current bare v4 settings snapshot used by the selected native realtime
    /// Agent adapter. It is app-owned state and contains no credentials.
    pub fn audio_configuration_json(&self) -> Result<String, String> {
        self.audio_configuration_json
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| "audio settings have not been received from the app host".into())
    }
}

#[async_trait]
impl MediaService for AppMediaHost {
    fn capabilities(&self) -> MediaCapabilities {
        self.service.capabilities()
    }

    fn capabilities_for(&self, owner: &MediaOwner) -> MediaCapabilities {
        self.service.capabilities_for(owner)
    }

    async fn execute(
        &self,
        context: MediaOperationContext,
        operation: MediaOperation,
    ) -> Result<MediaResult, MediaError> {
        self.service.execute(context, operation).await
    }

    async fn attach_session(
        &self,
        context: MediaOperationContext,
        session: Box<dyn MediaSession>,
    ) -> Result<MediaSessionRef, MediaError> {
        self.service.attach_session(context, session).await
    }

    async fn session_audio(
        &self,
        context: MediaOperationContext,
        session: MediaSessionRef,
    ) -> Result<Arc<dyn MediaAudioChannel>, MediaError> {
        self.service.session_audio(context, session).await
    }

    async fn read_media(
        &self,
        context: MediaOperationContext,
        media: MediaRef,
    ) -> Result<MediaContent, MediaError> {
        self.service.read_media(context, media).await
    }

    async fn cancel(&self, identity: MediaOperationId) -> Result<(), MediaError> {
        self.service.cancel(identity).await
    }
}

struct MediaRuntimeBuilderProduct;

impl MediaRuntimeBuilderProduct {
    #[allow(clippy::too_many_arguments)]
    fn build(
        resources: Arc<dyn MediaResourceStore>,
        audio: Arc<dyn media_api::audio::AudioService>,
        camera: Option<Arc<dyn media_api::camera::CameraControl>>,
        connector: Arc<MediaConnectorRouter>,
        supported_modes: Vec<MediaSessionMode>,
        cloud_audio: Arc<super::media_cloud::AudioProviderCloudRegistry>,
        offline_audio: Vec<OfflineAudioBackend>,
        audio_routing: AudioRoutingSettings,
        service_epoch: u64,
        max_payload_bytes: u64,
    ) -> Arc<UnifiedMediaService> {
        let builder = media_runtime::MediaRuntimeBuilder::new(resources).audio(audio);
        let builder = if let Some(camera) = camera {
            builder.camera(camera)
        } else {
            builder
        };
        builder
            .connector(connector, supported_modes)
            .offline_audio(offline_audio)
            .audio_routing(audio_routing)
            .service_epoch(service_epoch)
            .max_payload_bytes(max_payload_bytes)
            .cloud_audio_host(cloud_audio)
            .build()
    }
}

impl std::fmt::Debug for AppMediaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppMediaHost(<app-scoped>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use media_api::audio::{
        AudioCapabilitySnapshot, AudioError, AudioErrorKind, AudioOperation, AudioOperationContext,
        AudioOperationId, AudioOperationSuccess, AudioService,
    };

    struct UnsupportedAudio;

    #[async_trait]
    impl AudioService for UnsupportedAudio {
        fn capabilities(&self) -> AudioCapabilitySnapshot {
            AudioCapabilitySnapshot {
                service_epoch: 1,
                support_revision: 1,
                supported_operations: Vec::new(),
                readiness: Vec::new(),
                max_payload_bytes: 1024,
            }
        }

        async fn execute(
            &self,
            _context: AudioOperationContext,
            _operation: AudioOperation,
        ) -> Result<AudioOperationSuccess, AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "the capability projection test does not execute audio",
            ))
        }

        async fn cancel(&self, _identity: AudioOperationId) -> Result<(), AudioError> {
            Ok(())
        }
    }

    struct UnopenedDevicePorts;

    #[async_trait]
    impl MediaDevicePorts for UnopenedDevicePorts {
        async fn open(
            &self,
            _context: MediaOperationContext,
            _request: MediaSessionRequest,
        ) -> Result<Arc<dyn media_api::MediaDeviceSession>, MediaError> {
            Err(MediaError::new(
                MediaErrorKind::Unavailable,
                "the capability projection test does not open devices",
            ))
        }
    }

    #[test]
    fn local_audio_session_is_advertised_only_after_explicit_device_modes_arrive() {
        let host = AppMediaHost::new(
            Arc::new(super::super::media_store::BoundedMediaResourceStore::default()),
            Arc::new(UnsupportedAudio),
            None,
            Arc::new(UnopenedDevicePorts),
            Vec::new(),
            Arc::new(super::super::media_cloud::AudioProviderCloudRegistry::new(
                "wav",
            )),
            Vec::new(),
            AudioRoutingSettings::default(),
            1,
            1024,
        )
        .expect("an app host may start before desktop helper capabilities arrive");
        let owner = MediaOwner::Session {
            session_id: "desktop-session".into(),
        };
        assert!(
            !host
                .media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "absence of a native media-session capability must not advertise local audio"
        );

        host.update_local_session_modes(vec![MediaSessionMode::Audio])
            .expect("the helper's explicit duplex mode is applied");
        assert!(
            host.media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "the exact helper mode must make local audio sessions available"
        );

        host.update_local_session_modes(Vec::new())
            .expect("helper loss clears its local mode inventory");
        assert!(
            !host
                .media_service()
                .capabilities_for(&owner)
                .supports(media_api::MediaOperationKind::SessionStart),
            "a disconnected native helper must remove local audio support"
        );
    }
}
