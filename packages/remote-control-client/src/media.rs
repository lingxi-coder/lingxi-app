//! Product-layer media negotiation over an already authenticated Remote Control
//! RPC link. Only SDP and host-issued media references cross this control plane;
//! continuous media uses an independent WebRTC peer connection.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::rpc::RpcError;

/// The one host-initiated request a viewer may answer on the approved control link.
pub const MEDIA_OFFER_METHOD: &str = "media.offer";
/// The second and only other host-to-viewer product request method.
pub const MEDIA_CONTROL_METHOD: &str = "media.control";
/// Host-bound notification carrying actual native playback progress.
pub const MEDIA_PLAYBACK_RECEIPT_METHOD: &str = "media.playback_receipt";
/// Bound an SDP exchange before RPC framing or a peer implementation sees it.
pub const MEDIA_SDP_MAX_CHARS: usize = 64 * 1024;
/// Bound opaque host-issued session/source references before storing or forwarding them.
pub const MEDIA_REFERENCE_MAX_CHARS: usize = 128;
/// Largest native capture/render payload accepted by the product media adapter.
pub const MEDIA_PAYLOAD_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Product audio receipts are bounded to one hour of 20 ms encoded frames.
pub const MEDIA_AUDIO_SEQUENCE_MAX: u64 = 180_000;
/// Maximum cumulative mono sample frames at the 48 kHz RTC boundary for one hour.
pub const MEDIA_AUDIO_SAMPLE_FRAMES_MAX: u64 = MEDIA_AUDIO_SEQUENCE_MAX * 960;
/// Maximum generation/counter that remains exactly representable by JavaScript.
pub const MEDIA_COUNTER_MAX: u64 = 9_007_199_254_740_991;

/// Native source category represented by one exact per-route remote reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMediaSourceKind {
    /// Remote microphone PCM input.
    Microphone,
    /// Remote camera input.
    Camera,
    /// Remote screen-share input.
    Screen,
}

/// Build the only accepted remote-source spelling for one host-issued UUID route.
pub fn remote_media_source_ref(
    route_id: &str,
    kind: RemoteMediaSourceKind,
) -> Result<String, RpcError> {
    if !valid_route_uuid(route_id) {
        return Err(RpcError::new(
            crate::rpc::RemoteErrorCode::InvalidParams,
            "invalid media route ID",
        ));
    }
    let suffix = match kind {
        RemoteMediaSourceKind::Microphone => "microphone",
        RemoteMediaSourceKind::Camera => "camera",
        RemoteMediaSourceKind::Screen => "screen",
    };
    Ok(format!("remote_{route_id}_{suffix}"))
}

/// Parse an exact source ref created by [`remote_media_source_ref`].
pub fn parse_remote_media_source_ref(reference: &str) -> Option<(&str, RemoteMediaSourceKind)> {
    let value = reference.strip_prefix("remote_")?;
    let (route_id, kind) = value.split_once('_')?;
    if !valid_route_uuid(route_id) {
        return None;
    }
    let kind = match kind {
        "microphone" => RemoteMediaSourceKind::Microphone,
        "camera" => RemoteMediaSourceKind::Camera,
        "screen" => RemoteMediaSourceKind::Screen,
        _ => return None,
    };
    Some((route_id, kind))
}

fn valid_route_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

/// One product-configured ICE server. Credentials stay in this product layer
/// and are never placed in `media-api` or transcript/model DTOs.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteIceServer {
    /// Standard STUN/TURN URLs. The active WebRTC backend subset is checked
    /// before opening a native device lease.
    pub urls: Vec<String>,
    /// TURN username, when required.
    pub username: Option<String>,
    /// TURN credential, when required.
    pub credential: Option<String>,
}

impl std::fmt::Debug for RemoteIceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteIceServer")
            .field("url_count", &self.urls.len())
            .field("has_username", &self.username.is_some())
            .field("has_credential", &self.credential.is_some())
            .finish()
    }
}

/// Per-session transport policy resolved by the product host.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteMediaConfig {
    /// STUN/TURN servers for this media peer, resolved for the verified owner.
    pub ice_servers: Vec<RemoteIceServer>,
    /// Whether local candidates must use TURN; failed TURN never falls back.
    pub relay_only: bool,
    /// Maximum wait for local candidates before using the gathered SDP.
    pub gather_timeout_ms: u64,
    /// Maximum time to wait for the actual peer connection to become active.
    pub connect_timeout_ms: u64,
    /// Maximum individual captured or rendered payload size.
    pub max_payload_bytes: u64,
}

impl std::fmt::Debug for RemoteMediaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteMediaConfig")
            .field("ice_server_count", &self.ice_servers.len())
            .field("relay_only", &self.relay_only)
            .field("gather_timeout_ms", &self.gather_timeout_ms)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("max_payload_bytes", &self.max_payload_bytes)
            .finish()
    }
}

impl RemoteMediaConfig {
    /// Validate provider bounds before constructing sockets or opening devices.
    pub fn validate(&self) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media configuration",
            )
        };
        let mut url_count = 0usize;
        if self.ice_servers.len() > 8
            || self.gather_timeout_ms == 0
            || self.gather_timeout_ms > 15_000
            || self.connect_timeout_ms == 0
            || self.connect_timeout_ms > 60_000
            || self.max_payload_bytes == 0
            || self.max_payload_bytes > MEDIA_PAYLOAD_MAX_BYTES
        {
            return Err(invalid());
        }
        let mut has_turn = false;
        for server in &self.ice_servers {
            url_count = url_count.saturating_add(server.urls.len());
            let has_server_turn = server.urls.iter().any(|url| {
                url.split_once(':').is_some_and(|(scheme, _)| {
                    matches!(scheme.to_ascii_lowercase().as_str(), "turn" | "turns")
                })
            });
            if server.urls.is_empty()
                || url_count > 16
                || server
                    .urls
                    .iter()
                    .any(|url| url.is_empty() || url.len() > 2048 || !valid_ice_url(url))
                || server
                    .username
                    .as_ref()
                    .is_some_and(|value| value.len() > 1024)
                || server
                    .credential
                    .as_ref()
                    .is_some_and(|value| value.len() > 1024)
                || server
                    .username
                    .as_ref()
                    .is_some_and(|value| value.chars().any(char::is_control))
                || server
                    .credential
                    .as_ref()
                    .is_some_and(|value| value.chars().any(char::is_control))
                || (has_server_turn
                    && (server.username.as_deref().unwrap_or_default().is_empty()
                        || server.credential.as_deref().unwrap_or_default().is_empty()))
                || (!has_server_turn && (server.username.is_some() || server.credential.is_some()))
                || (server.username.is_some() != server.credential.is_some())
            {
                return Err(invalid());
            }
            if has_server_turn {
                has_turn = true;
            }
        }
        if self.relay_only && !has_turn {
            return Err(invalid());
        }
        Ok(())
    }

    /// Reject valid ICE URI forms that the currently pinned WebRTC driver
    /// cannot use. Keep this separate from syntax validation so a future
    /// transport implementation can enable secure/TCP ICE without changing
    /// the product configuration format.
    pub fn validate_supported_transport(&self) -> Result<(), RpcError> {
        let unsupported = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::Internal,
                "the configured ICE transport is not supported by this WebRTC backend",
            )
        };
        for url in self.ice_servers.iter().flat_map(|server| &server.urls) {
            let Some((scheme, authority_and_query)) = url.split_once(':') else {
                return Err(unsupported());
            };
            let scheme = scheme.to_ascii_lowercase();
            let (authority, query) = authority_and_query
                .split_once('?')
                .map_or((authority_and_query, None), |(authority, query)| {
                    (authority, Some(query))
                });
            let supported = match (scheme.as_str(), query) {
                ("stun", None) => true,
                ("turn", None | Some("transport=udp")) => true,
                _ => false,
            };
            if !supported || authority.is_empty() {
                return Err(unsupported());
            }
        }
        Ok(())
    }
}

fn valid_ice_url(url: &str) -> bool {
    if url
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
        || url.contains('@')
        || url.contains('#')
    {
        return false;
    }
    let Some((scheme, authority_and_query)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    let turn = matches!(scheme.as_str(), "turn" | "turns");
    if !turn && !matches!(scheme.as_str(), "stun" | "stuns") {
        return false;
    }
    let (authority, query) = authority_and_query
        .split_once('?')
        .map_or((authority_and_query, None), |(authority, query)| {
            (authority, Some(query))
        });
    if !valid_ice_authority(authority) {
        return false;
    }
    match query {
        None => true,
        Some(query) if turn => matches!(query, "transport=udp" | "transport=tcp"),
        Some(_) => false,
    }
}

fn valid_ice_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('/') {
        return false;
    }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some(end) = bracketed.find(']') else {
            return false;
        };
        let host = &bracketed[..end];
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        let suffix = &bracketed[end + 1..];
        let port = if suffix.is_empty() {
            None
        } else if let Some(port) = suffix.strip_prefix(':') {
            Some(port)
        } else {
            return false;
        };
        (host, port)
    } else {
        let mut pieces = authority.split(':');
        let host = pieces.next().unwrap_or_default();
        let port = pieces.next();
        if pieces.next().is_some() {
            return false;
        }
        (host, port)
    };

    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    if host.parse::<std::net::Ipv4Addr>().is_err()
        && !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return false;
    }
    port.is_none_or(|port| {
        !port.is_empty()
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port != 0)
    })
}

/// Product-owned resolver for per-session ICE/transport policy. The returned
/// credentials are used only by the RTC constructor and are redacted from logs.
#[async_trait]
pub trait RemoteMediaConfigProvider: Send + Sync {
    /// Resolve transport settings for this verified connection and offer.
    async fn resolve(
        &self,
        connection: &RemoteMediaConnection,
        offer: &RemoteMediaOffer,
    ) -> Result<RemoteMediaConfig, RpcError>;
}

/// Requested media carried by the independent WebRTC peer connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMediaMode {
    /// Audio transport only.
    Audio,
    /// Video transport only.
    Video,
    /// Audio and video transport.
    AudioVideo,
}

/// Media features a viewer handler explicitly implements. A generic approved
/// Remote Control connection does not imply this capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaCapabilities {
    /// Version of the product media offer/control handlers.
    pub version: u32,
    /// Supported independent-peer modes.
    pub modes: Vec<RemoteMediaMode>,
    /// Actual source categories understood by the handler.
    pub source_kinds: Vec<RemoteMediaSourceKind>,
}

impl RemoteMediaCapabilities {
    /// Validate a peer's explicit, bounded capability declaration.
    pub fn validate(&self) -> Result<(), RpcError> {
        let has_audio = self
            .source_kinds
            .contains(&RemoteMediaSourceKind::Microphone);
        let has_video = self.source_kinds.iter().any(|kind| {
            matches!(
                kind,
                RemoteMediaSourceKind::Camera | RemoteMediaSourceKind::Screen
            )
        });
        let audio_mode = self.modes.iter().any(|mode| mode.audio());
        let video_mode = self.modes.iter().any(|mode| mode.video());
        if self.version != 1
            || self.modes.is_empty()
            || self.modes.len() > 3
            || self.source_kinds.is_empty()
            || self.source_kinds.len() > 3
            || self
                .modes
                .iter()
                .enumerate()
                .any(|(index, mode)| self.modes[..index].contains(mode))
            || self
                .source_kinds
                .iter()
                .enumerate()
                .any(|(index, kind)| self.source_kinds[..index].contains(kind))
            || audio_mode != has_audio
            || video_mode != has_video
        {
            return Err(RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid remote media capability declaration",
            ));
        }
        Ok(())
    }
}

impl RemoteMediaMode {
    pub(crate) fn audio(self) -> bool {
        matches!(self, Self::Audio | Self::AudioVideo)
    }

    pub(crate) fn video(self) -> bool {
        matches!(self, Self::Video | Self::AudioVideo)
    }
}

/// Why the media session was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMediaIntent {
    /// Observe media without attaching it to an active agent turn.
    Observation,
    /// Attach media to the current conversation or active turn.
    Conversation,
}

/// A bounded host control request on the authenticated Remote Control link.
/// It carries no media bytes; the independent RTC peer carries continuous AV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMediaControlAction {
    /// Wait until the requested audio boundary has actually played.
    FlushAudio,
    /// Stop pending output and return actual progress before the cut.
    InterruptAudio,
    /// Ask the native peer to interrupt playback and release its media device
    /// lease before the host closes its independent RTC peer.
    Close,
}

/// Host-to-viewer flush or interruption request for one active media session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaControlRequest {
    /// Product media wire version.
    pub v: u32,
    /// Host-issued opaque media session reference.
    pub media_session_ref: String,
    /// Host operation generation; rejects control from a stale operation.
    pub generation: u64,
    /// Requested action.
    pub action: RemoteMediaControlAction,
    /// Last encoded audio sequence included in this barrier.
    pub target_audio_sequence: u64,
    /// Cumulative 48 kHz sample frames through this barrier.
    pub target_sample_frames: u64,
}

/// Actual remote speaker progress returned after a flush or interruption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaControlResult {
    /// Product media wire version.
    pub v: u32,
    /// Same host-issued session reference as the control request.
    pub media_session_ref: String,
    /// Same host operation generation as the control request.
    pub generation: u64,
    /// Last decoded audio sequence reaching the native output path.
    pub audio_sequence: u64,
    /// Cumulative decoded sample frames accepted by the native output path.
    pub accepted_sample_frames: u64,
    /// Cumulative frames confirmed played by the native output callback.
    pub played_sample_frames: u64,
    /// Timestamp of the last native render callback, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_at_unix_ms: Option<u64>,
}

/// Viewer-to-host playback progress notification. It is private connection
/// control metadata, never transcript or general viewer traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaPlaybackReceipt {
    /// Product media wire version.
    pub v: u32,
    /// Host-issued opaque media session reference.
    pub media_session_ref: String,
    /// Host operation generation; receipts from older generations are rejected.
    pub generation: u64,
    /// Monotonic decoded audio frame sequence for this session generation.
    pub audio_sequence: u64,
    /// Cumulative decoded sample frames accepted by the native output path.
    pub accepted_sample_frames: u64,
    /// Cumulative sample frames confirmed played by the native output callback.
    pub played_sample_frames: u64,
    /// Timestamp of the last native render callback, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_at_unix_ms: Option<u64>,
}

fn valid_counter(value: u64) -> bool {
    value <= MEDIA_COUNTER_MAX
}

fn valid_progress(sequence: u64, accepted: u64, played: u64) -> bool {
    valid_counter(sequence)
        && sequence <= MEDIA_AUDIO_SEQUENCE_MAX
        && accepted <= MEDIA_AUDIO_SAMPLE_FRAMES_MAX
        && played <= accepted
}

impl RemoteMediaControlRequest {
    /// Validate product control fields and their bounded progress counters.
    pub fn validate(&self) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media control request",
            )
        };
        if self.v != 1
            || !valid_reference(&self.media_session_ref)
            || self.generation == 0
            || !valid_counter(self.generation)
            || match self.action {
                RemoteMediaControlAction::Close => {
                    self.target_audio_sequence != 0 || self.target_sample_frames != 0
                }
                RemoteMediaControlAction::FlushAudio | RemoteMediaControlAction::InterruptAudio => {
                    self.target_audio_sequence > MEDIA_AUDIO_SEQUENCE_MAX
                        || self.target_sample_frames > MEDIA_AUDIO_SAMPLE_FRAMES_MAX
                        || (self.target_audio_sequence == 0) != (self.target_sample_frames == 0)
                }
            }
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// Validate references, generation and bounded audio barrier counters.
    pub fn validate_for(&self, offer: &RemoteMediaOffer) -> Result<(), RpcError> {
        self.validate()?;
        if self.media_session_ref != offer.media_session_ref || self.generation != offer.generation
        {
            return Err(RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media control request",
            ));
        }
        Ok(())
    }
}

impl RemoteMediaControlResult {
    /// Validate the result and ensure it belongs to the exact requested barrier.
    pub fn validate_for(&self, request: &RemoteMediaControlRequest) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media control result",
            )
        };
        if self.v != 1
            || self.media_session_ref != request.media_session_ref
            || self.generation != request.generation
            || !valid_progress(
                self.audio_sequence,
                self.accepted_sample_frames,
                self.played_sample_frames,
            )
            || (request.action == RemoteMediaControlAction::FlushAudio
                && self.played_sample_frames < request.target_sample_frames)
            || (request.action == RemoteMediaControlAction::InterruptAudio
                && self.played_sample_frames > request.target_sample_frames)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl RemoteMediaPlaybackReceipt {
    /// Validate one bounded playback receipt before owner/ordering checks.
    pub fn validate(&self) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media playback receipt",
            )
        };
        if self.v != 1
            || !valid_reference(&self.media_session_ref)
            || !valid_counter(self.generation)
            || self.generation == 0
            || !valid_progress(
                self.audio_sequence,
                self.accepted_sample_frames,
                self.played_sample_frames,
            )
            || self.audio_sequence == 0
            || self.played_sample_frames == 0
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Host-authenticated context supplied by the live Remote Control connection.
/// These values are never taken from the untrusted media request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMediaConnection {
    /// The Firebase account uid used by the signed Remote Control offer.
    pub account_uid: String,
    /// Self-certifying id of the pinned host.
    pub host_id: String,
    /// Self-certifying id of this device.
    pub device_id: String,
    /// Unique id of this signed Remote Control connection.
    pub connection_id: String,
}

/// Host-to-viewer media negotiation request over the approved control link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaOffer {
    /// Product media wire version.
    pub v: u32,
    /// Host-issued opaque media session reference.
    pub media_session_ref: String,
    /// Host session this media session belongs to.
    pub session_id: String,
    /// Host operation generation used to reject delayed control messages.
    pub generation: u64,
    /// Media kinds negotiated on the independent peer connection.
    pub mode: RemoteMediaMode,
    /// Conversation attribution for this session.
    pub intent: RemoteMediaIntent,
    /// Optional host-issued source reference for the viewer microphone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_source_ref: Option<String>,
    /// Optional host-issued source reference for the viewer camera or screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_source_ref: Option<String>,
    /// Complete, bounded non-trickle SDP offer.
    pub offer_sdp: String,
}

/// Viewer-to-host result for [`RemoteMediaOffer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteMediaAnswer {
    /// Product media wire version.
    pub v: u32,
    /// The same host-issued media session reference from the offer.
    pub media_session_ref: String,
    /// Complete, bounded non-trickle SDP answer.
    pub answer_sdp: String,
}

fn valid_reference(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MEDIA_REFERENCE_MAX_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_session_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn valid_sdp(sdp: &str, mode: RemoteMediaMode) -> bool {
    if sdp.is_empty() || sdp.len() > MEDIA_SDP_MAX_CHARS || !sdp.is_ascii() {
        return false;
    }
    let audio = sdp.lines().any(|line| line.starts_with("m=audio "));
    let video = sdp.lines().any(|line| line.starts_with("m=video "));
    sdp.starts_with("v=0\r\n") && audio == mode.audio() && video == mode.video()
}

impl RemoteMediaOffer {
    /// Validate request bounds before touching native device ports or creating a peer.
    pub fn validate(&self) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media offer",
            )
        };
        if self.v != 1
            || !valid_reference(&self.media_session_ref)
            || !valid_session_id(&self.session_id)
            || !valid_counter(self.generation)
            || self.generation == 0
            || self
                .audio_source_ref
                .as_ref()
                .is_some_and(|reference| !valid_reference(reference))
            || self
                .video_source_ref
                .as_ref()
                .is_some_and(|reference| !valid_reference(reference))
            || !valid_sdp(&self.offer_sdp, self.mode)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl RemoteMediaAnswer {
    /// Validate the viewer answer and ensure it is bound to the offered session.
    pub fn validate_for(&self, offer: &RemoteMediaOffer) -> Result<(), RpcError> {
        let invalid = || {
            RpcError::new(
                crate::rpc::RemoteErrorCode::InvalidParams,
                "invalid media answer",
            )
        };
        if self.v != 1
            || self.media_session_ref != offer.media_session_ref
            || !valid_sdp(&self.answer_sdp, offer.mode)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Handles one host offer on the viewer side. Implementations own the actual
/// answerer peer and owner-bound native ports. Calls must be idempotent per
/// `(connection_id, media_session_ref)` and stop all sessions when a connection
/// closes, including when close races an in-flight offer. The answer returns
/// after local SDP gathering; the active session's status owns the actual
/// connected/ready state once the host applies that answer.
#[async_trait]
pub trait RemoteMediaOfferHandler: Send + Sync {
    /// Capabilities provided by this concrete answerer. Returning `None`
    /// prevents the host from advertising media support for an approved link.
    fn capabilities(&self) -> Option<RemoteMediaCapabilities> {
        None
    }

    /// Accept a host offer and return after the separate peer has a gathered answer.
    async fn accept_offer(
        &self,
        connection: &RemoteMediaConnection,
        offer: RemoteMediaOffer,
        control: Arc<dyn RemoteMediaControlChannel>,
    ) -> Result<RemoteMediaAnswer, RpcError>;

    /// Handle an owner-bound flush/interrupt request over the approved control link.
    async fn control(
        &self,
        connection: &RemoteMediaConnection,
        request: RemoteMediaControlRequest,
    ) -> Result<RemoteMediaControlResult, RpcError>;

    /// Stop all media owned by this authenticated Remote Control connection.
    async fn connection_closed(&self, connection: &RemoteMediaConnection);
}

/// Narrow authenticated notification port exposed to one accepted media
/// session. Product handlers cannot use it to issue arbitrary RPC requests.
#[async_trait]
pub trait RemoteMediaControlChannel: Send + Sync {
    /// Report actual playback progress to the matching host connector.
    async fn playback_receipt(&self, receipt: RemoteMediaPlaybackReceipt) -> Result<(), RpcError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn offer(mode: RemoteMediaMode) -> RemoteMediaOffer {
        let (audio, video) = match mode {
            RemoteMediaMode::Audio => (true, false),
            RemoteMediaMode::Video => (false, true),
            RemoteMediaMode::AudioVideo => (true, true),
        };
        let mut sdp = String::from("v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n");
        if audio {
            sdp.push_str("m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n");
        }
        if video {
            sdp.push_str("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n");
        }
        RemoteMediaOffer {
            v: 1,
            media_session_ref: "media_0123456789abcdef".into(),
            session_id: "12121212-3434-4567-8899-aaaaaaaaaaaa".into(),
            generation: 1,
            mode,
            intent: RemoteMediaIntent::Conversation,
            audio_source_ref: None,
            video_source_ref: None,
            offer_sdp: sdp,
        }
    }

    fn media_config(url: &str) -> RemoteMediaConfig {
        let turn = url.to_ascii_lowercase().starts_with("turn:")
            || url.to_ascii_lowercase().starts_with("turns:");
        RemoteMediaConfig {
            ice_servers: vec![RemoteIceServer {
                urls: vec![url.into()],
                username: turn.then(|| "test-user".into()),
                credential: turn.then(|| "test-secret".into()),
            }],
            relay_only: turn,
            gather_timeout_ms: 1_000,
            connect_timeout_ms: 5_000,
            max_payload_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn media_config_parses_standard_ice_uri_schemes_and_preflights_driver_support() {
        for url in [
            "stun:stun.example.com:3478",
            "stuns:stun.example.com:5349",
            "turn:turn.example.com:3478?transport=udp",
            "turns:turn.example.com:5349?transport=tcp",
        ] {
            media_config(url)
                .validate()
                .expect("standard ICE URI syntax");
        }
        for url in [
            "stun:stun.example.com:3478",
            "turn:turn.example.com:3478",
            "turn:turn.example.com:3478?transport=udp",
        ] {
            media_config(url)
                .validate_supported_transport()
                .expect("UDP ICE is supported by the pinned driver");
        }
        for url in [
            "stuns:stun.example.com:5349",
            "turns:turn.example.com:5349?transport=tcp",
            "turn:turn.example.com:3478?transport=tcp",
        ] {
            assert!(media_config(url).validate_supported_transport().is_err());
        }
    }

    #[test]
    fn media_config_rejects_malformed_authorities_and_unpaired_credentials() {
        for url in [
            "stun:.example.com:3478",
            "stun:example.com:abc",
            "stun:example.com:65536",
            "stun:[not-ipv6]:3478",
            "turn:turn.example.com:3478?transport=udp&secret=x",
        ] {
            assert!(media_config(url).validate().is_err(), "accepted {url}");
        }
        let mut config = media_config("stun:stun.example.com:3478");
        config.ice_servers[0].username = Some("orphaned-user".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn remote_source_refs_are_exactly_scoped_to_uuid_routes_and_known_kinds() {
        let route = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
        let microphone = remote_media_source_ref(route, RemoteMediaSourceKind::Microphone).unwrap();
        assert_eq!(microphone, format!("remote_{route}_microphone"));
        assert_eq!(
            parse_remote_media_source_ref(&microphone),
            Some((route, RemoteMediaSourceKind::Microphone)),
        );
        assert!(parse_remote_media_source_ref("remote_route-1_microphone").is_none());
        assert!(parse_remote_media_source_ref(&format!("remote_{route}_speaker")).is_none());
        assert!(remote_media_source_ref("route-1", RemoteMediaSourceKind::Camera).is_err());
    }

    #[test]
    fn advertised_media_capabilities_are_versioned_bounded_and_truthful() {
        let capabilities = RemoteMediaCapabilities {
            version: 1,
            modes: vec![
                RemoteMediaMode::Audio,
                RemoteMediaMode::Video,
                RemoteMediaMode::AudioVideo,
            ],
            source_kinds: vec![
                RemoteMediaSourceKind::Microphone,
                RemoteMediaSourceKind::Camera,
            ],
        };
        capabilities
            .validate()
            .expect("native mic and camera handlers");
        let encoded = serde_json::to_value(&capabilities).unwrap();
        assert_eq!(encoded["version"], 1);
        assert_eq!(encoded["source_kinds"], json!(["microphone", "camera"]));

        let mut invented_camera = capabilities.clone();
        invented_camera.source_kinds = vec![RemoteMediaSourceKind::Microphone];
        assert!(invented_camera.validate().is_err());
        let mut duplicate = capabilities.clone();
        duplicate.source_kinds.push(RemoteMediaSourceKind::Camera);
        assert!(duplicate.validate().is_err());
        let mut unknown_version = capabilities;
        unknown_version.version = 2;
        assert!(unknown_version.validate().is_err());
    }

    #[test]
    fn media_offer_round_trips_each_explicit_mode() {
        for mode in [
            RemoteMediaMode::Audio,
            RemoteMediaMode::Video,
            RemoteMediaMode::AudioVideo,
        ] {
            let offer = offer(mode);
            offer.validate().expect("valid SDP shape");
            let value = serde_json::to_value(&offer).expect("serialized offer");
            let decoded: RemoteMediaOffer = serde_json::from_value(value).expect("decoded offer");
            assert_eq!(decoded, offer);
        }
    }

    #[test]
    fn media_offer_rejects_unexpected_fields_mismatched_tracks_and_oversized_sdp() {
        let valid = offer(RemoteMediaMode::Audio);
        let mut value = serde_json::to_value(&valid).expect("serialized offer");
        value["untrusted_owner"] = json!("another-account");
        assert!(serde_json::from_value::<RemoteMediaOffer>(value).is_err());

        let mut wrong_mode = offer(RemoteMediaMode::Audio);
        wrong_mode
            .offer_sdp
            .push_str("m=video 9 UDP/TLS/RTP/SAVPF 96\r\n");
        assert!(wrong_mode.validate().is_err());

        let mut oversized = offer(RemoteMediaMode::Audio);
        oversized
            .offer_sdp
            .push_str(&"a=x\r\n".repeat(MEDIA_SDP_MAX_CHARS / 4));
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn media_answer_must_echo_the_opaque_host_session_reference() {
        let offer = offer(RemoteMediaMode::Audio);
        let answer = RemoteMediaAnswer {
            v: 1,
            media_session_ref: "media_0123456789abcdef".into(),
            answer_sdp: offer.offer_sdp.clone(),
        };
        answer.validate_for(&offer).expect("matching session");
        let mut mismatch = answer;
        mismatch.media_session_ref = "media_other".into();
        assert!(mismatch.validate_for(&offer).is_err());
    }
}
