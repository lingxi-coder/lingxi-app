//! Device operations of the `window.lingxi.v2` bridge (`device.*`).
//!
//! Every operation runs the same ladder: parse+clamp the page payload →
//! [`LocalAppsHostBroker::authorize_declared_capability`] (manifest-declared,
//! then persisted → session → prompt) → dispatch into the live
//! [`crate::local_apps_device::SharedDeviceCapabilities`] handle → envelope
//! the result as JSON with media returned as base64 (the page turns it into
//! a Blob URL; see `lib/lingxi-bridge.js`). Media responses are capped at
//! [`MAX_DEVICE_MEDIA_RESULT_BYTES`] — `evaluateJavaScript` delivers the
//! envelope as one string, and multi-MB strings are where the WebView hurts.

use super::{BridgeFailure, LocalAppsHostBroker};
use base64::Engine as _;
use client_protocol::local_apps::AppCapabilityKindDto;
use local_apps::AppCapability;
use platform_api::{
    CalendarError, CalendarEvent, CalendarQuery, CameraError, CameraPosition, CapturePhotoOpts,
    ClipboardError, ContactsError, ContactsQuery, DeepLinkError, DeviceStatusError, HapticError,
    HapticStyle, LocationError, NotificationError, NotificationRequest, ShareError, SharePayload,
    ShareResult, SttError, SttOpts, TtsError, TtsOpts, VoiceError, VoiceRecorder, VoiceRecording,
    VoiceRecordingOpts,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// Cap on the base64 body of one media response. A default-preset photo is
/// ~400-700 KB base64 and a 5-minute 16 kHz AAC mono recording ~1.6 MB, both
/// comfortably inside; past ~8 MB `evaluateJavaScript` delivery visibly
/// stalls the page, so 4 MiB is the contract.
pub(super) const MAX_DEVICE_MEDIA_RESULT_BYTES: usize = 4 * 1024 * 1024;

const PHOTO_MIN_DIMENSION: u32 = 256;
const PHOTO_MAX_DIMENSION: u32 = 2048;
const PHOTO_DEFAULT_DIMENSION: u32 = 1280;
const PHOTO_MIN_QUALITY: f32 = 0.5;
const PHOTO_MAX_QUALITY: f32 = 0.92;
const PHOTO_DEFAULT_QUALITY: f32 = 0.8;
const RECORD_MIN_DURATION_MS: u64 = 1_000;
const RECORD_MAX_DURATION_MS: u64 = 300_000;
const RECORD_DEFAULT_DURATION_MS: u64 = 120_000;
/// How long a watchdog-finished recording waits for the page to collect it.
const FINISHED_RECORDING_TTL: Duration = Duration::from_secs(60);
const LOCATION_TIMEOUT: Duration = Duration::from_secs(30);
const RECORD_SAMPLE_RATE_HZ: u32 = 16_000;
const RECORD_FORMAT: &str = "m4a";
const NOTIFICATION_TITLE_MAX_CHARS: usize = 100;
const NOTIFICATION_BODY_MAX_CHARS: usize = 500;
const NOTIFICATION_TAG_MAX_LEN: usize = 64;
const CLIPBOARD_TEXT_MAX_CHARS: usize = 100_000;
const SHARE_TEXT_MAX_CHARS: usize = 20_000;
const SHARE_URL_MAX_CHARS: usize = 4_096;
const TTS_TEXT_MAX_CHARS: usize = 10_000;
const DEEP_LINK_MAX_CHARS: usize = 4_096;
const CALENDAR_MAX_RANGE_MS: u64 = 366 * 24 * 60 * 60 * 1_000;
const CALENDAR_MAX_LIMIT: u32 = 100;
const CONTACTS_QUERY_MAX_CHARS: usize = 200;
const CONTACTS_MAX_LIMIT: u32 = 50;

// First-use prompt reasons, one per capability (reach the sheet verbatim
// through `AppCapabilityRequestDto.reason`).
const REASON_CAMERA: &str = "应用请求使用相机拍摄一张照片。";
const REASON_PHOTO_LIBRARY: &str = "应用请求从相册选择一张图片。";
const REASON_MICROPHONE: &str = "应用请求使用麦克风录音。";
const REASON_LOCATION: &str = "应用请求获取一次当前位置。";
const REASON_NOTIFICATIONS: &str = "应用请求发送本地通知。";
const REASON_TRANSCRIBE: &str = "应用请求使用麦克风把你说的话转写成文字。";
const REASON_CLIPBOARD: &str = "应用请求读取或写入系统剪贴板。";
const REASON_SHARE: &str = "应用请求打开系统分享面板。";
const REASON_TTS: &str = "应用请求将文字转换为语音。";
const REASON_HAPTICS: &str = "应用请求触发一次短促的触觉反馈。";
const REASON_DEEP_LINK: &str = "应用请求打开一个外部链接。";
const REASON_CALENDAR: &str = "应用请求读取你指定时间范围内的日历事件。";
const REASON_CONTACTS: &str = "应用请求搜索你的联系人信息。";
const REASON_MEDIA: &str = "应用请求读取它自己刚刚获取的媒体内容。";

/// The single in-flight `device.recordAudio*` session.
pub(super) struct ActiveRecording {
    app_id: String,
    started: Instant,
    watchdog: tokio::task::JoinHandle<()>,
    finished: Option<FinishedRecording>,
    /// The recorder this session was STARTED on, pinned for its lifetime.
    ///
    /// The one place a live device handle must NOT be re-read per call: a
    /// recording spans two bridge calls, and `profile_apps` swaps the whole
    /// device set on every engine (re)build. Resolving the recorder again at
    /// stop time would call a fresh `VoiceImpl` that was never started —
    /// losing the audio and stranding the shared audio-session lease on the
    /// old object with no handle left that can release it.
    voice: Arc<dyn VoiceRecorder>,
}

/// A recording the duration watchdog already stopped, parked until the page
/// collects it (or [`FINISHED_RECORDING_TTL`] expires).
struct FinishedRecording {
    recording: VoiceRecording,
    duration_ms: u64,
    at: Instant,
    auto_stopped: bool,
}

fn invalid(message: impl Into<String>) -> BridgeFailure {
    BridgeFailure::coded("invalid_request", message.into())
}

fn unavailable(what: &str) -> BridgeFailure {
    BridgeFailure::coded(
        "capability_unavailable",
        format!("{what} is not available on this device/build"),
    )
}

fn map_camera_error(error: CameraError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        CameraError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        CameraError::Cancelled => BridgeFailure::coded("cancelled", message),
        CameraError::DeviceUnavailable => BridgeFailure::coded("device_unavailable", message),
        CameraError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_voice_error(error: VoiceError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        VoiceError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        VoiceError::Busy => BridgeFailure::coded("audio_session_busy", message),
        VoiceError::NotRecording => BridgeFailure::coded("not_recording", message),
        VoiceError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_location_error(error: LocationError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        LocationError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        LocationError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        LocationError::Timeout => BridgeFailure::coded("timeout", message),
        LocationError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_notification_error(error: NotificationError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        NotificationError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        NotificationError::Other(_) => BridgeFailure::from(message),
    }
}

/// Base64-encode media, enforcing [`MAX_DEVICE_MEDIA_RESULT_BYTES`].
fn encode_media(bytes: &[u8]) -> Result<String, BridgeFailure> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    if encoded.len() > MAX_DEVICE_MEDIA_RESULT_BYTES {
        return Err(BridgeFailure::coded(
            "media_too_large",
            format!(
                "media payload is {} base64 bytes (limit {MAX_DEVICE_MEDIA_RESULT_BYTES})",
                encoded.len()
            ),
        ));
    }
    Ok(encoded)
}

/// Parse the page's photo scaling knobs, clamped to the contract ranges.
fn photo_scaling(payload: &Value) -> Result<(u32, f32), BridgeFailure> {
    let max_dimension = match payload.get("maxDimension") {
        None | Some(Value::Null) => PHOTO_DEFAULT_DIMENSION,
        Some(value) => u32::try_from(
            value
                .as_u64()
                .ok_or_else(|| invalid("maxDimension must be a positive integer"))?,
        )
        .unwrap_or(u32::MAX),
    }
    .clamp(PHOTO_MIN_DIMENSION, PHOTO_MAX_DIMENSION);
    let quality = match payload.get("quality") {
        None | Some(Value::Null) => PHOTO_DEFAULT_QUALITY,
        Some(value) => value
            .as_f64()
            .ok_or_else(|| invalid("quality must be a number"))? as f32,
    }
    .clamp(PHOTO_MIN_QUALITY, PHOTO_MAX_QUALITY);
    Ok((max_dimension, quality))
}

fn map_stt_error(error: SttError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        SttError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        SttError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        // The SAME code recording reports for the same cause: an app told to
        // branch on `audio_session_busy` must not have to learn a second
        // name for "the mic is in use".
        SttError::Busy => BridgeFailure::coded("audio_session_busy", message),
        // Distinct from an error the app should surface as a failure: the
        // mic simply heard nothing, which a UI usually retries silently.
        SttError::NoSpeech => BridgeFailure::coded("no_speech", message),
        SttError::Retriable(_) => BridgeFailure::coded("retriable", message),
        SttError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_clipboard_error(error: ClipboardError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        ClipboardError::Unsupported => BridgeFailure::coded("unsupported", message),
        ClipboardError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_share_error(error: ShareError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        ShareError::Unsupported => BridgeFailure::coded("unsupported", message),
        ShareError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_tts_error(error: TtsError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        TtsError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        TtsError::SynthesisFailed(_) => BridgeFailure::coded("synthesis_failed", message),
        TtsError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_device_status_error(error: DeviceStatusError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        DeviceStatusError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        DeviceStatusError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_haptic_error(error: HapticError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        HapticError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        HapticError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_deep_link_error(error: DeepLinkError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        DeepLinkError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        DeepLinkError::Rejected(_) => BridgeFailure::coded("rejected", message),
        DeepLinkError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_calendar_error(error: CalendarError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        CalendarError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        CalendarError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        CalendarError::Invalid(_) => BridgeFailure::coded("invalid_request", message),
        CalendarError::Other(_) => BridgeFailure::from(message),
    }
}

fn map_contacts_error(error: ContactsError) -> BridgeFailure {
    let message = error.to_string();
    match error {
        ContactsError::Unavailable => BridgeFailure::coded("device_unavailable", message),
        ContactsError::PermissionDenied => BridgeFailure::coded("permission_denied", message),
        ContactsError::Invalid(_) => BridgeFailure::coded("invalid_request", message),
        ContactsError::Other(_) => BridgeFailure::from(message),
    }
}

fn calendar_query(payload: &Value) -> Result<CalendarQuery, BridgeFailure> {
    let start_ms = payload
        .get("startMs")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("startMs must be a non-negative integer"))?;
    let end_ms = payload
        .get("endMs")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("endMs must be a non-negative integer"))?;
    if end_ms <= start_ms || end_ms - start_ms > CALENDAR_MAX_RANGE_MS {
        return Err(invalid(format!(
            "calendar range must be 1..={CALENDAR_MAX_RANGE_MS} milliseconds"
        )));
    }
    let limit = payload
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
        .unwrap_or(50)
        .clamp(1, CALENDAR_MAX_LIMIT);
    Ok(CalendarQuery {
        start_ms,
        end_ms,
        limit,
    })
}

fn contacts_query(payload: &Value) -> Result<ContactsQuery, BridgeFailure> {
    let query = payload
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("query is required"))?
        .trim()
        .to_string();
    if query.is_empty() || query.chars().count() > CONTACTS_QUERY_MAX_CHARS {
        return Err(invalid(format!(
            "query must be 1..={CONTACTS_QUERY_MAX_CHARS} characters"
        )));
    }
    let limit = payload
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
        .unwrap_or(20)
        .clamp(1, CONTACTS_MAX_LIMIT);
    Ok(ContactsQuery { query, limit })
}

fn haptic_style(value: &Value) -> Result<(HapticStyle, &'static str), BridgeFailure> {
    let raw = value
        .get("style")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("style is required"))?;
    let style = match raw {
        "light" => (HapticStyle::Light, "light"),
        "medium" => (HapticStyle::Medium, "medium"),
        "heavy" => (HapticStyle::Heavy, "heavy"),
        "success" => (HapticStyle::Success, "success"),
        "warning" => (HapticStyle::Warning, "warning"),
        "error" => (HapticStyle::Error, "error"),
        _ => {
            return Err(invalid(
                "style must be light|medium|heavy|success|warning|error",
            ))
        }
    };
    Ok(style)
}

fn validated_deep_link(value: &Value) -> Result<String, BridgeFailure> {
    let raw = value
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("url is required"))?;
    if raw.is_empty() || raw.chars().count() > DEEP_LINK_MAX_CHARS {
        return Err(invalid(format!(
            "url must be 1..={DEEP_LINK_MAX_CHARS} characters"
        )));
    }
    let parsed =
        reqwest::Url::parse(raw).map_err(|error| invalid(format!("invalid URL: {error}")))?;
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(invalid("deep links must not contain username or password"));
    }
    match parsed.scheme() {
        "http" | "https" => {
            if parsed.host_str().is_none() {
                return Err(invalid("http(s) deep links require a host"));
            }
        }
        "mailto" | "tel" => {}
        _ => return Err(invalid("deep link scheme is not allowed")),
    }
    Ok(parsed.to_string())
}

fn valid_notification_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= NOTIFICATION_TAG_MAX_LEN
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

impl LocalAppsHostBroker {
    fn devices(&self) -> Result<crate::local_apps_device::DeviceCapabilities, BridgeFailure> {
        self.device
            .get()
            .map(|cell| cell.current())
            .ok_or_else(|| unavailable("the device capability set"))
    }

    /// Retain one capture and build the JSON envelope for it.
    ///
    /// The envelope carries BOTH the base64 (so the page can render it right
    /// away as a Blob URL) and a `mediaId` handle (so `llm.chat` can attach
    /// it without pushing megabytes back through the WebView request path).
    fn media_envelope(
        &self,
        app_id: &str,
        media_type: &str,
        bytes: Vec<u8>,
        extra: Value,
    ) -> Result<Value, BridgeFailure> {
        let base64_body = encode_media(&bytes)?;
        let handle = self.media.put(
            app_id,
            self.request_id("media"),
            crate::local_apps_device::MediaEntry {
                media_type: media_type.to_string(),
                bytes: std::sync::Arc::new(bytes),
            },
        );
        let mut envelope = json!({
            "mimeType": media_type,
            "base64": base64_body,
            "mediaId": handle,
        });
        if let (Some(target), Some(extra)) = (envelope.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        Ok(envelope)
    }

    /// Look up a retained capture for `llm.chat`.
    pub(super) fn media_entry(
        &self,
        app_id: &str,
        handle: &str,
    ) -> Option<crate::local_apps_device::MediaEntry> {
        self.media.get(app_id, handle)
    }

    pub(super) fn clear_media(&self, app_id: &str) {
        self.media.clear_app(app_id);
    }

    pub(super) async fn capture_photo_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Camera,
            AppCapabilityKindDto::Camera,
            REASON_CAMERA,
        )
        .await?;
        let camera = self
            .devices()?
            .camera
            .ok_or_else(|| unavailable("the camera"))?;
        let (max_dimension, quality) = photo_scaling(payload)?;
        let position = match payload.get("camera").and_then(Value::as_str) {
            None | Some("back") => CameraPosition::Back,
            Some("front") => CameraPosition::Front,
            Some(other) => {
                return Err(invalid(format!("camera must be front|back, got {other:?}")));
            }
        };
        let allow_editing = payload
            .get("allowEditing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let image = camera
            .capture_photo_sized(
                CapturePhotoOpts {
                    position,
                    allow_editing,
                },
                max_dimension,
                quality,
            )
            .await
            .map_err(map_camera_error)?;
        self.media_envelope(
            app_id,
            "image/jpeg",
            image.jpeg_bytes,
            json!({ "width": image.width, "height": image.height }),
        )
    }

    pub(super) async fn pick_image_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::PhotoLibrary,
            AppCapabilityKindDto::PhotoLibrary,
            REASON_PHOTO_LIBRARY,
        )
        .await?;
        let camera = self
            .devices()?
            .camera
            .ok_or_else(|| unavailable("the photo library"))?;
        let (max_dimension, quality) = photo_scaling(payload)?;
        let image = camera
            .pick_from_library_sized(max_dimension, quality)
            .await
            .map_err(map_camera_error)?;
        self.media_envelope(
            app_id,
            "image/jpeg",
            image.jpeg_bytes,
            json!({ "width": image.width, "height": image.height }),
        )
    }

    pub(super) async fn record_audio_start_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Microphone,
            AppCapabilityKindDto::Microphone,
            REASON_MICROPHONE,
        )
        .await?;
        let voice = self
            .devices()?
            .voice
            .ok_or_else(|| unavailable("the microphone"))?;
        let max_duration_ms = match payload.get("maxDurationMs") {
            None | Some(Value::Null) => RECORD_DEFAULT_DURATION_MS,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| invalid("maxDurationMs must be a positive integer"))?,
        }
        .clamp(RECORD_MIN_DURATION_MS, RECORD_MAX_DURATION_MS);

        // Serializes STARTS only, and never blocks: a start crosses into
        // Swift, and the first mic use of an app's life begins with an OS
        // permission alert whose think time is the user's. Holding the state
        // lock across that would block `force_stop_recording` — awaited by a
        // runtime stop — and every other app's `recordAudioStop` until the
        // user answered. `try_lock` keeps the fast `audio_session_busy`
        // answer instead of converting it into a second hang.
        let _start_gate = match self.recording_start.try_lock() {
            Ok(gate) => gate,
            Err(_) => {
                return Err(BridgeFailure::coded(
                    "audio_session_busy",
                    "another recording is already starting",
                ));
            }
        };

        // Short critical section: decide, and take the orphan OUT. The
        // native calls below run with no state lock held.
        let orphan = {
            let mut guard = self.recording.lock().await;
            if let Some(active) = guard.as_ref() {
                // A foreign session blocks a start while it is still running
                // AND while it is parked but still collectable: the reclaim
                // below discards the orphan's bytes outright, so exempting a
                // parked recording here would let one app silently destroy
                // audio another app is still entitled to fetch — with no
                // error on either side.
                let collectable = active
                    .finished
                    .as_ref()
                    .is_some_and(|finished| finished.at.elapsed() <= FINISHED_RECORDING_TTL);
                if active.app_id != app_id && (active.finished.is_none() || collectable) {
                    return Err(BridgeFailure::coded(
                        "audio_session_busy",
                        "another app currently holds the recorder",
                    ));
                }
            }
            guard.take()
        };

        // A same-app restart (a reloaded page) or an expired parked recording
        // is reclaimed rather than fatal — the orphan's bytes are discarded
        // and, crucially, the native audio-session lease is released on the
        // recorder the orphan actually started on.
        let mut replaced_active = false;
        if let Some(orphan) = orphan {
            orphan.watchdog.abort();
            if orphan.finished.is_none() {
                replaced_active = true;
                let _ = orphan.voice.stop_recording().await;
            }
        }
        voice
            .start_recording(VoiceRecordingOpts {
                sample_rate_hz: RECORD_SAMPLE_RATE_HZ,
                format: RECORD_FORMAT.into(),
            })
            .await
            .map_err(map_voice_error)?;

        let watchdog = {
            let recording_cell = self.recording.clone();
            let voice = voice.clone();
            let app = app_id.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(max_duration_ms)).await;
                let mut guard = recording_cell.lock().await;
                let Some(active) = guard.as_mut() else { return };
                if active.app_id != app || active.finished.is_some() {
                    return;
                }
                let duration_ms =
                    u64::try_from(active.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                match voice.stop_recording().await {
                    Ok(recording) => {
                        active.finished = Some(FinishedRecording {
                            recording,
                            duration_ms,
                            at: Instant::now(),
                            auto_stopped: true,
                        });
                    }
                    // The native side already lost the session; nothing to park.
                    Err(_) => *guard = None,
                }
            })
        };
        *self.recording.lock().await = Some(ActiveRecording {
            app_id: app_id.to_string(),
            started: Instant::now(),
            watchdog,
            finished: None,
            voice,
        });
        Ok(json!({
            "started": true,
            "maxDurationMs": max_duration_ms,
            "replacedActive": replaced_active,
        }))
    }

    // Stopping deliberately re-checks NO permission: the grant gated the
    // capture; refusing to RELEASE the microphone because an allow-once
    // grant was consumed would keep it hot instead.
    pub(super) async fn record_audio_stop_value(
        &self,
        app_id: &str,
    ) -> Result<Value, BridgeFailure> {
        let mut guard = self.recording.lock().await;
        match guard.take() {
            None => Err(BridgeFailure::coded(
                "not_recording",
                "no recording is active",
            )),
            Some(active) if active.app_id != app_id => {
                let refused =
                    BridgeFailure::coded("not_recording", "another app owns the active recording");
                *guard = Some(active);
                Err(refused)
            }
            Some(mut active) => {
                let finished = match active.finished.take() {
                    Some(finished) => {
                        active.watchdog.abort();
                        if finished.at.elapsed() > FINISHED_RECORDING_TTL {
                            return Err(BridgeFailure::coded(
                                "not_recording",
                                "the auto-stopped recording expired uncollected",
                            ));
                        }
                        finished
                    }
                    None => {
                        let duration_ms =
                            u64::try_from(active.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                        // Stop on the pinned recorder, and only give up the
                        // session once it has actually stopped. Taking the
                        // entry (and aborting the watchdog) before this call
                        // meant a transient native failure left the lease
                        // open with nothing left to reclaim it: a retried
                        // stop answered `not_recording`, and the runtime-stop
                        // hook found no session to force-stop.
                        let recording = match active.voice.stop_recording().await {
                            Ok(recording) => recording,
                            Err(error) => {
                                *guard = Some(active);
                                return Err(map_voice_error(error));
                            }
                        };
                        active.watchdog.abort();
                        FinishedRecording {
                            recording,
                            duration_ms,
                            at: Instant::now(),
                            auto_stopped: false,
                        }
                    }
                };
                let mime_type = finished.recording.mime_type.clone();
                self.media_envelope(
                    app_id,
                    &mime_type,
                    finished.recording.audio_bytes,
                    json!({
                        "durationMs": finished.duration_ms,
                        "autoStopped": finished.auto_stopped,
                    }),
                )
            }
        }
    }

    /// Reclaim the recorder when `app_id`'s runtime stops (user stop, quota
    /// eviction, process exit) so the native audio-session lease never
    /// outlives the page that opened it.
    pub(super) async fn force_stop_recording(&self, app_id: &str) {
        let mut guard = self.recording.lock().await;
        let owned = matches!(guard.as_ref(), Some(active) if active.app_id == app_id);
        if !owned {
            return;
        }
        let active = guard.take().expect("checked above");
        active.watchdog.abort();
        if active.finished.is_none() {
            // The recorder the session started on — the live device cell may
            // already hold a different connection's `VoiceImpl`, which would
            // leave this one's audio-session lease open forever.
            let _ = active.voice.stop_recording().await;
        }
    }

    /// Listen once and return what was said.
    ///
    /// This is the audio story on this stack, and it is not an accident:
    /// the conversation protocol has no audio content block, and
    /// `SpeechToText::transcribe` opens the microphone for one utterance
    /// rather than transcribing a file — so a recorded m4a cannot be sent to
    /// a model no matter how it is packaged. An app that wants voice input
    /// transcribes here and sends the text.
    ///
    /// Rides `Microphone`: it is the same hardware and the same user-visible
    /// risk, so a second capability would be a distinction without a
    /// difference.
    pub(super) async fn transcribe_speech_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Microphone,
            AppCapabilityKindDto::Microphone,
            REASON_TRANSCRIBE,
        )
        .await?;
        let stt = self
            .devices()?
            .stt
            .ok_or_else(|| unavailable("speech recognition"))?;
        let language = match payload.get("language") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| invalid("language must be a BCP-47 string"))?
                    .to_string(),
            ),
        };
        let transcript = stt
            .transcribe(SttOpts { language })
            .await
            .map_err(map_stt_error)?;
        Ok(json!({
            "text": transcript.text,
            "language": transcript.language,
            "confidence": transcript.confidence,
        }))
    }

    pub(super) async fn get_location_value(&self, app_id: &str) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Location,
            AppCapabilityKindDto::Location,
            REASON_LOCATION,
        )
        .await?;
        let location = self
            .devices()?
            .location
            .ok_or_else(|| unavailable("location services"))?;
        let fix = tokio::time::timeout(LOCATION_TIMEOUT, location.current_location())
            .await
            .map_err(|_| BridgeFailure::coded("timeout", "the location request timed out"))?
            .map_err(map_location_error)?;
        Ok(json!({
            "latitude": fix.latitude,
            "longitude": fix.longitude,
            "accuracyM": fix.accuracy_m,
            "timestampMs": fix.timestamp_ms,
        }))
    }

    pub(super) async fn post_notification_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Notifications,
            AppCapabilityKindDto::Notifications,
            REASON_NOTIFICATIONS,
        )
        .await?;
        let notifications = self
            .devices()?
            .notifications
            .ok_or_else(|| unavailable("notifications"))?;
        let title = payload
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("title is required"))?;
        if title.trim().is_empty() || title.chars().count() > NOTIFICATION_TITLE_MAX_CHARS {
            return Err(invalid(format!(
                "title must be 1..={NOTIFICATION_TITLE_MAX_CHARS} characters"
            )));
        }
        let body = payload
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("body is required"))?;
        if body.trim().is_empty() || body.chars().count() > NOTIFICATION_BODY_MAX_CHARS {
            return Err(invalid(format!(
                "body must be 1..={NOTIFICATION_BODY_MAX_CHARS} characters"
            )));
        }
        let page_tag = match payload.get("tag") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let tag = value
                    .as_str()
                    .ok_or_else(|| invalid("tag must be a string"))?;
                if !valid_notification_tag(tag) {
                    return Err(invalid("tag must match ^[a-z0-9][a-z0-9_-]{0,63}$"));
                }
                Some(tag.to_string())
            }
        };
        // The app-scoped prefix is applied HERE, before the native layer, so
        // no app can address (and replace) another app's — or the
        // assistant's — notifications.
        // The page's own tag is what comes back, NOT the composed identifier.
        // The composed form contains `.`, which this very function's grammar
        // rejects — echoing it would hand the page a value it cannot pass
        // back, breaking exactly the replace/dedupe round-trip a `tag` is for.
        // (An app that supplied no tag gets its minted one back and can
        // replace with it, because the same prefix is re-derived here.)
        let echoed_tag = page_tag.unwrap_or_else(|| self.request_id("n"));
        notifications
            .notify(NotificationRequest {
                title: title.to_string(),
                body: body.to_string(),
                tag: Some(format!("local-app.{app_id}.{echoed_tag}")),
            })
            .await
            .map_err(map_notification_error)?;
        Ok(json!({ "posted": true, "tag": echoed_tag }))
    }

    pub(super) async fn clipboard_get_text_value(
        &self,
        app_id: &str,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Clipboard,
            AppCapabilityKindDto::Clipboard,
            REASON_CLIPBOARD,
        )
        .await?;
        let clipboard = self
            .devices()?
            .clipboard
            .ok_or_else(|| unavailable("the clipboard"))?;
        Ok(json!({
            "text": clipboard.get_text().await.map_err(map_clipboard_error)?,
        }))
    }

    pub(super) async fn clipboard_set_text_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Clipboard,
            AppCapabilityKindDto::Clipboard,
            REASON_CLIPBOARD,
        )
        .await?;
        let text = payload
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("text is required"))?;
        if text.chars().count() > CLIPBOARD_TEXT_MAX_CHARS {
            return Err(invalid(format!(
                "text must be at most {CLIPBOARD_TEXT_MAX_CHARS} characters"
            )));
        }
        let clipboard = self
            .devices()?
            .clipboard
            .ok_or_else(|| unavailable("the clipboard"))?;
        clipboard
            .set_text(text.to_string())
            .await
            .map_err(map_clipboard_error)?;
        Ok(json!({ "written": true }))
    }

    pub(super) async fn share_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Share,
            AppCapabilityKindDto::Share,
            REASON_SHARE,
        )
        .await?;
        let text = match payload.get("text") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| invalid("text must be a string"))?;
                if text.chars().count() > SHARE_TEXT_MAX_CHARS {
                    return Err(invalid(format!(
                        "text must be at most {SHARE_TEXT_MAX_CHARS} characters"
                    )));
                }
                Some(text.to_string())
            }
        };
        let url = match payload.get("url") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let url = value
                    .as_str()
                    .ok_or_else(|| invalid("url must be a string"))?;
                if url.chars().count() > SHARE_URL_MAX_CHARS {
                    return Err(invalid(format!(
                        "url must be at most {SHARE_URL_MAX_CHARS} characters"
                    )));
                }
                Some(url.to_string())
            }
        };
        let image_bytes = match payload.get("mediaId") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let media_id = value
                    .as_str()
                    .ok_or_else(|| invalid("mediaId must be a string"))?;
                let entry = self.media_entry(app_id, media_id).ok_or_else(|| {
                    BridgeFailure::coded(
                        "media_not_found",
                        format!("mediaId {media_id:?} is unknown or has expired"),
                    )
                })?;
                if !entry.media_type.starts_with("image/") {
                    return Err(invalid("mediaId must refer to an image"));
                }
                Some((*entry.bytes).clone())
            }
        };
        if text.as_deref().is_none_or(str::is_empty)
            && url.as_deref().is_none_or(str::is_empty)
            && image_bytes.is_none()
        {
            return Err(invalid("share requires text, url, or image mediaId"));
        }
        let share = self
            .devices()?
            .share
            .ok_or_else(|| unavailable("the system share sheet"))?;
        let result = share
            .share(SharePayload {
                text,
                image_bytes,
                url,
            })
            .await
            .map_err(map_share_error)?;
        Ok(match result {
            ShareResult::Success => json!({ "shared": true, "cancelled": false }),
            ShareResult::Cancelled => json!({ "shared": false, "cancelled": true }),
        })
    }

    pub(super) async fn synthesize_speech_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::TextToSpeech,
            AppCapabilityKindDto::TextToSpeech,
            REASON_TTS,
        )
        .await?;
        let text = payload
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("text is required"))?;
        if text.trim().is_empty() || text.chars().count() > TTS_TEXT_MAX_CHARS {
            return Err(invalid(format!(
                "text must be 1..={TTS_TEXT_MAX_CHARS} characters"
            )));
        }
        let voice = match payload.get("voice") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| invalid("voice must be a string"))?
                    .to_string(),
            ),
        };
        let tts = self
            .devices()?
            .tts
            .ok_or_else(|| unavailable("text-to-speech"))?;
        let audio = tts
            .synthesize(TtsOpts {
                text: text.to_string(),
                voice,
            })
            .await
            .map_err(map_tts_error)?;
        self.media_envelope(
            app_id,
            "audio/pcm",
            audio.pcm,
            json!({ "sampleRateHz": audio.sample_rate_hz }),
        )
    }

    pub(super) async fn device_status_value(&self, app_id: &str) -> Result<Value, BridgeFailure> {
        self.ensure_declared_capability(app_id, AppCapability::DeviceStatus)?;
        let provider = self
            .devices()?
            .device_status
            .ok_or_else(|| unavailable("device status"))?;
        serde_json::to_value(provider.status().await.map_err(map_device_status_error)?)
            .map_err(|error| BridgeFailure::from(format!("serialize device status: {error}")))
    }

    pub(super) async fn haptics_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Haptics,
            AppCapabilityKindDto::Haptics,
            REASON_HAPTICS,
        )
        .await?;
        let (style, wire_style) = haptic_style(payload)?;
        let haptics = self
            .devices()?
            .haptics
            .ok_or_else(|| unavailable("haptics"))?;
        haptics.trigger(style).await.map_err(map_haptic_error)?;
        Ok(json!({ "triggered": true, "style": wire_style }))
    }

    pub(super) async fn deep_link_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::DeepLink,
            AppCapabilityKindDto::DeepLink,
            REASON_DEEP_LINK,
        )
        .await?;
        let url = validated_deep_link(payload)?;
        let opener = self
            .devices()?
            .deep_link
            .ok_or_else(|| unavailable("deep links"))?;
        opener
            .open(url.clone())
            .await
            .map_err(map_deep_link_error)?;
        Ok(json!({ "opened": true, "url": url }))
    }

    pub(super) async fn calendar_list_events_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Calendar,
            AppCapabilityKindDto::Calendar,
            REASON_CALENDAR,
        )
        .await?;
        let query = calendar_query(payload)?;
        let calendar = self
            .devices()?
            .calendar
            .ok_or_else(|| unavailable("calendar"))?;
        let events: Vec<CalendarEvent> = calendar
            .list_events(query)
            .await
            .map_err(map_calendar_error)?;
        serde_json::to_value(events)
            .map_err(|error| BridgeFailure::from(format!("serialize calendar events: {error}")))
    }

    pub(super) async fn contacts_search_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Contacts,
            AppCapabilityKindDto::Contacts,
            REASON_CONTACTS,
        )
        .await?;
        let query = contacts_query(payload)?;
        let contacts = self
            .devices()?
            .contacts
            .ok_or_else(|| unavailable("contacts"))?
            .search(query)
            .await
            .map_err(map_contacts_error)?;
        serde_json::to_value(contacts)
            .map_err(|error| BridgeFailure::from(format!("serialize contacts: {error}")))
    }

    pub(super) async fn media_value(
        &self,
        app_id: &str,
        payload: &Value,
    ) -> Result<Value, BridgeFailure> {
        self.authorize_declared_capability(
            app_id,
            AppCapability::Media,
            AppCapabilityKindDto::Media,
            REASON_MEDIA,
        )
        .await?;
        let handle = payload
            .get("mediaId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("mediaId is required"))?;
        if handle.is_empty() || handle.len() > 128 {
            return Err(invalid("mediaId must be 1..=128 bytes"));
        }
        let entry = self
            .media_entry(app_id, handle)
            .ok_or_else(|| BridgeFailure::coded("not_found", "mediaId is unknown or expired"))?;
        let base64 = encode_media(&entry.bytes)?;
        Ok(json!({
            "mimeType": entry.media_type,
            "base64": base64,
            "mediaId": handle,
            "bytes": entry.bytes.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::local_apps_device::{DeviceCapabilities, SharedDeviceCapabilities};
    use crate::local_apps_host::LocalAppsHostBroker;
    use async_trait::async_trait;
    use base64::Engine as _;
    use client_adapter::{ClientEventSink, MockSink};
    use client_protocol::events::ClientEvent;
    use client_protocol::local_apps::{
        AppAuthorizationDecisionDto, AppBridgeOperationDto, AppBridgeRequestDto, AppEventDto,
    };
    use local_apps::test_support::FixedClock;
    use local_apps::{
        load_manifest, load_permissions, save_manifest, save_permissions, AppCapability,
        AppDependencyRecord, AppDependencyState, AppLayout, AppRuntimeProfile, AppService,
        AppSurface, NoopAppEventObserver, APPS_SCHEMA_VERSION,
    };
    use platform_api::{
        CalendarError, CalendarEvent, CalendarProvider, CalendarQuery, CameraControl, CameraError,
        CapturePhotoOpts, CapturedImage, Clipboard, ClipboardError, Contact, ContactsError,
        ContactsProvider, ContactsQuery, LocationError, LocationFix, LocationProvider,
        NotificationError, NotificationRequest, NotificationService, ShareError, SharePayload,
        ShareResult, SharingService, TextToSpeech, TtsAudio, TtsError, TtsOpts, VoiceError,
        VoiceRecorder, VoiceRecording, VoiceRecordingOpts,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time::timeout;

    // ---- fakes ------------------------------------------------------------

    #[derive(Default)]
    struct FakeCamera {
        /// (max_dimension, jpeg_quality) the bridge handed us, per call.
        sized_calls: StdMutex<Vec<(u32, f32)>>,
        /// Bytes the next capture/pick returns.
        bytes: StdMutex<Vec<u8>>,
        error: StdMutex<Option<CameraError>>,
    }

    impl FakeCamera {
        fn with_bytes(bytes: Vec<u8>) -> Arc<Self> {
            let fake = Self::default();
            *fake.bytes.lock().unwrap() = bytes;
            Arc::new(fake)
        }

        fn failing(error: CameraError) -> Arc<Self> {
            let fake = Self::default();
            *fake.error.lock().unwrap() = Some(error);
            Arc::new(fake)
        }

        fn image(&self) -> Result<CapturedImage, CameraError> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(CapturedImage {
                jpeg_bytes: self.bytes.lock().unwrap().clone(),
                width: 640,
                height: 480,
            })
        }
    }

    #[async_trait]
    impl CameraControl for FakeCamera {
        async fn capture_photo(
            &self,
            _opts: CapturePhotoOpts,
        ) -> Result<CapturedImage, CameraError> {
            self.image()
        }

        async fn pick_from_library(&self) -> Result<CapturedImage, CameraError> {
            self.image()
        }

        async fn capture_photo_sized(
            &self,
            _opts: CapturePhotoOpts,
            max_dimension: u32,
            jpeg_quality: f32,
        ) -> Result<CapturedImage, CameraError> {
            self.sized_calls
                .lock()
                .unwrap()
                .push((max_dimension, jpeg_quality));
            self.image()
        }

        async fn pick_from_library_sized(
            &self,
            max_dimension: u32,
            jpeg_quality: f32,
        ) -> Result<CapturedImage, CameraError> {
            self.sized_calls
                .lock()
                .unwrap()
                .push((max_dimension, jpeg_quality));
            self.image()
        }
    }

    #[derive(Default)]
    struct FakeClipboard {
        text: StdMutex<Option<String>>,
    }

    #[async_trait]
    impl Clipboard for FakeClipboard {
        async fn set_text(&self, text: String) -> Result<(), ClipboardError> {
            *self.text.lock().unwrap() = Some(text);
            Ok(())
        }

        async fn get_text(&self) -> Result<Option<String>, ClipboardError> {
            Ok(self.text.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    struct FakeShare {
        payloads: StdMutex<Vec<SharePayload>>,
    }

    #[async_trait]
    impl SharingService for FakeShare {
        async fn share(&self, payload: SharePayload) -> Result<ShareResult, ShareError> {
            self.payloads.lock().unwrap().push(payload);
            Ok(ShareResult::Success)
        }
    }

    struct FakeTts;

    #[async_trait]
    impl TextToSpeech for FakeTts {
        async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError> {
            Ok(TtsAudio {
                pcm: opts.text.into_bytes(),
                sample_rate_hz: 24_000,
            })
        }
    }

    #[derive(Default)]
    struct FakeCalendar {
        queries: StdMutex<Vec<CalendarQuery>>,
        events: Vec<CalendarEvent>,
    }

    #[async_trait]
    impl CalendarProvider for FakeCalendar {
        async fn list_events(
            &self,
            query: CalendarQuery,
        ) -> Result<Vec<CalendarEvent>, CalendarError> {
            self.queries.lock().unwrap().push(query);
            Ok(self.events.clone())
        }
    }

    #[derive(Default)]
    struct FakeContacts {
        queries: StdMutex<Vec<ContactsQuery>>,
        contacts: Vec<Contact>,
    }

    #[async_trait]
    impl ContactsProvider for FakeContacts {
        async fn search(&self, query: ContactsQuery) -> Result<Vec<Contact>, ContactsError> {
            self.queries.lock().unwrap().push(query);
            Ok(self.contacts.clone())
        }
    }

    #[derive(Default)]
    struct FakeVoice {
        recording: AtomicBool,
        stopped: AtomicBool,
        /// Held closed to stand in for the OS microphone permission alert:
        /// `start_recording` does not return until the test opens it.
        start_gate: Option<Arc<tokio::sync::Notify>>,
    }

    #[async_trait]
    impl VoiceRecorder for FakeVoice {
        async fn start_recording(&self, _opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
            if let Some(gate) = self.start_gate.clone() {
                gate.notified().await;
            }
            self.recording.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            if !self.recording.swap(false, Ordering::SeqCst) {
                return Err(VoiceError::NotRecording);
            }
            self.stopped.store(true, Ordering::SeqCst);
            Ok(VoiceRecording {
                audio_bytes: vec![7, 7, 7],
                mime_type: "audio/m4a".into(),
            })
        }

        async fn is_recording(&self) -> bool {
            self.recording.load(Ordering::SeqCst)
        }
    }

    struct FakeLocation {
        hang: bool,
        error: Option<LocationError>,
    }

    #[async_trait]
    impl LocationProvider for FakeLocation {
        async fn current_location(&self) -> Result<LocationFix, LocationError> {
            if self.hang {
                std::future::pending::<()>().await;
            }
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            Ok(LocationFix {
                latitude: 31.2304,
                longitude: 121.4737,
                accuracy_m: Some(65.0),
                timestamp_ms: 1_753_000_000_000,
            })
        }
    }

    #[derive(Default)]
    struct FakeNotifications {
        requests: StdMutex<Vec<NotificationRequest>>,
    }

    #[async_trait]
    impl NotificationService for FakeNotifications {
        async fn notify(&self, req: NotificationRequest) -> Result<(), NotificationError> {
            self.requests.lock().unwrap().push(req);
            Ok(())
        }
    }

    // ---- harness ----------------------------------------------------------

    struct Harness {
        _root: TempDir,
        broker: Arc<LocalAppsHostBroker>,
        sink: Arc<MockSink>,
        service: Arc<AppService>,
        app_id: String,
        layout: AppLayout,
    }

    async fn harness(devices: DeviceCapabilities) -> Harness {
        let root = TempDir::new().expect("tempdir");
        let service = Arc::new(
            AppService::load(
                root.path(),
                Arc::new(FixedClock::new(1)),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .expect("load app service"),
        );
        let sink = MockSink::arc();
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            sink.clone() as Arc<dyn ClientEventSink>,
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_device(Arc::new(SharedDeviceCapabilities::new(devices)))
            .is_ok());
        let record = service
            .create_app(Some("Device"), "a device test app", None)
            .await
            .expect("create app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        let static_dist = root
            .path()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR);
        std::fs::create_dir_all(&static_dist).expect("static dist");
        std::fs::write(static_dist.join("index.html"), "<html>ok</html>").expect("index");
        let record = prepare_launchable_runtime_fixture(&service, record, &layout).await;
        Harness {
            _root: root,
            broker,
            sink,
            service,
            app_id: record.id,
            layout,
        }
    }

    async fn prepare_launchable_runtime_fixture(
        service: &Arc<AppService>,
        record: local_apps::AppRecord,
        layout: &AppLayout,
    ) -> local_apps::AppRecord {
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            AppRuntimeProfile::ReactDom,
        )
        .expect("react dom binding");
        let workspace = layout.root().join(layout.workspace_rel());
        crate::local_apps_build::scaffold_workspace_initialized(
            layout,
            crate::local_apps_build::LocalAppBuildTarget::ReactDomR2,
            true,
        )
        .expect("scaffold workspace");
        let scaffold = crate::local_app_runtime_profiles::scaffold_artifacts_for_binding(&binding)
            .expect("runtime profile scaffold");
        for (relative, bytes) in &scaffold.files {
            let path = workspace.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create scaffold parent");
            }
            std::fs::write(path, bytes).expect("write scaffold file");
        }

        let requested_bytes =
            std::fs::read(workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL))
                .expect("read requested dependencies");
        let package_bytes = std::fs::read(
            workspace.join(crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL),
        )
        .expect("read effective package");
        let lockfile_bytes =
            std::fs::read(workspace.join(crate::local_app_runtime_profiles::LOCKFILE_FILE_REL))
                .expect("read lockfile");
        let sbom_bytes = br#"{
  "spdxVersion": "SPDX-2.3",
  "SPDXID": "SPDXRef-DOCUMENT",
  "name": "device-op-fixture",
  "dataLicense": "CC0-1.0",
  "documentNamespace": "https://example.invalid/spdx/device-op-fixture"
}
"#;
        let snapshot = crate::local_app_runtime_profiles::snapshot_artifacts_for_binding(
            &binding,
            crate::local_app_runtime_profiles::hash_bytes(&requested_bytes),
            crate::local_app_runtime_profiles::hash_bytes(&package_bytes),
            crate::local_app_runtime_profiles::hash_bytes(&lockfile_bytes),
            crate::local_app_runtime_profiles::hash_bytes(b"device-op-fixture-tree"),
            sbom_bytes,
        )
        .expect("dependency snapshot");
        for (relative, bytes) in &snapshot.files {
            let path = workspace.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create snapshot parent");
            }
            std::fs::write(path, bytes).expect("write snapshot file");
        }

        let mut manifest = load_manifest(layout).expect("manifest");
        manifest.surface = Some(AppSurface::Dom);
        manifest.template_origin = Some(local_apps::AppTemplateOrigin {
            plugin_id: local_apps::AppTemplateOrigin::BUILTIN_PLUGIN_ID.into(),
            plugin_version: "builtin".into(),
            template_id: format!(
                "{}-r{}",
                binding.family.as_str().replace('_', "-"),
                binding.revision
            ),
            template_sha256: binding.contract_sha256.clone(),
        });
        manifest.runtime_profile = Some(binding);
        manifest.dependency_snapshot = Some(snapshot.snapshot);
        save_manifest(layout, &manifest).expect("save runtime manifest");
        local_apps::storage::save_dependency_record(
            layout.root(),
            &AppDependencyRecord {
                schema_version: APPS_SCHEMA_VERSION,
                app_id: record.id.clone(),
                state: AppDependencyState::Ready,
                lockfile_sha256: manifest
                    .dependency_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.lockfile_sha256.clone()),
                toolchain_key: manifest
                    .dependency_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.toolchain_key.clone()),
                install_attempts: 1,
                last_error: None,
                updated_at_ms: record.updated_at_ms,
            },
        )
        .expect("save dependency record");

        let build_root = layout.root().join(layout.build_rel(false));
        let output_root = build_root.join(crate::local_apps_build::VITE_OUTPUT_DIR);
        let output_sha256 = digest_tree(&output_root);
        let build_receipt = json!({
            "version": 3,
            "buildId": output_sha256,
            "buildKey": "device-op-fixture",
            "runtimeContractSha256": manifest.runtime_contract_hash().expect("runtime hash"),
            "dependencySnapshotSha256": manifest
                .dependency_snapshot_hash()
                .expect("dependency hash"),
            "outputSha256": output_sha256,
        });
        std::fs::write(
            build_root.join("build.json"),
            serde_json::to_vec_pretty(&build_receipt).expect("serialize build receipt"),
        )
        .expect("write build receipt");
        service
            .commit_scaffold(&record.id, &record.name, &record.brief, None)
            .await
            .expect("commit formed fixture")
    }

    fn digest_tree(root: &std::path::Path) -> String {
        let mut files = Vec::new();
        collect_tree_files(root, &mut files);
        files.sort();
        let mut hasher = Sha256::new();
        for path in files {
            let relative = path.strip_prefix(root).expect("relative output path");
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update([0]);
            hasher.update(std::fs::read(&path).expect("read output file"));
            hasher.update([0]);
        }
        format!("{:x}", hasher.finalize())
    }

    fn collect_tree_files(current: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        let metadata = std::fs::symlink_metadata(current).expect("inspect output path");
        assert!(
            !metadata.file_type().is_symlink(),
            "build output must not contain symlinks"
        );
        if metadata.is_dir() {
            for entry in std::fs::read_dir(current).expect("read output directory") {
                let entry = entry.expect("read output entry");
                collect_tree_files(&entry.path(), files);
            }
        } else if metadata.is_file() {
            files.push(current.to_path_buf());
        } else {
            panic!("build output must be regular files");
        }
    }

    fn declare(h: &Harness, capability: AppCapability) {
        let mut manifest = load_manifest(&h.layout).expect("manifest");
        manifest.capabilities.push(capability);
        save_manifest(&h.layout, &manifest).expect("declare");
    }

    fn grant(h: &Harness, capability: AppCapability) {
        let mut permissions = load_permissions(&h.layout).expect("permissions");
        permissions.grant(capability);
        save_permissions(&h.layout, &permissions).expect("grant");
    }

    fn declare_and_grant(h: &Harness, capability: AppCapability) {
        declare(h, capability);
        grant(h, capability);
    }

    async fn execute(
        h: &Harness,
        operation: AppBridgeOperationDto,
        payload: Value,
    ) -> (bool, Value, Option<String>, Option<String>) {
        h.broker
            .execute_bridge(AppBridgeRequestDto {
                request_id: "req-1".into(),
                app_id: h.app_id.clone(),
                operation,
                payload_json: Some(payload.to_string()),
            })
            .await;
        let response = h
            .sink
            .events()
            .await
            .into_iter()
            .rev()
            .find_map(|event| match event {
                ClientEvent::AppEvent {
                    event: AppEventDto::AppBridgeResponse { response },
                } => Some(response),
                _ => None,
            })
            .expect("a bridge response event");
        let result = response
            .result_json
            .as_deref()
            .map(|body| serde_json::from_str(body).expect("result json"))
            .unwrap_or(Value::Null);
        (response.ok, result, response.error, response.error_code)
    }

    // ---- capture / pick ---------------------------------------------------

    #[tokio::test]
    async fn capture_photo_returns_the_downscaled_jpeg_as_base64() {
        let camera = FakeCamera::with_bytes(vec![1, 2, 3]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, result, error, code) =
            execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["mimeType"], "image/jpeg");
        assert_eq!(result["width"], 640);
        assert_eq!(result["height"], 480);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result["base64"].as_str().expect("base64"))
            .expect("decodes");
        assert_eq!(bytes, vec![1, 2, 3]);
        assert_eq!(
            camera.sized_calls.lock().unwrap().as_slice(),
            &[(1280, 0.8)],
            "the defaults reach the native scaler"
        );
    }

    #[tokio::test]
    async fn capture_photo_clamps_dimension_and_quality() {
        let camera = FakeCamera::with_bytes(vec![1]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, _, _, _) = execute(
            &h,
            AppBridgeOperationDto::CapturePhoto,
            json!({"maxDimension": 99_999, "quality": 0.1}),
        )
        .await;
        assert!(ok);
        assert_eq!(
            camera.sized_calls.lock().unwrap().as_slice(),
            &[(2048, 0.5)]
        );
    }

    #[tokio::test]
    async fn an_oversized_media_result_is_refused() {
        let camera = FakeCamera::with_bytes(vec![0u8; 5 * 1024 * 1024]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("media_too_large"));
    }

    #[tokio::test]
    async fn pick_image_requires_its_own_photo_library_capability() {
        let camera = FakeCamera::with_bytes(vec![1]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        // Camera declared+granted — but PICK rides PhotoLibrary, a different
        // OS authorization surface, so it must still be refused.
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::PickImage, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("capability_not_declared"));
    }

    #[tokio::test]
    async fn a_missing_device_handle_fails_typed() {
        let h = harness(DeviceCapabilities::default()).await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("capability_unavailable"));
    }

    #[tokio::test]
    async fn calendar_events_are_bounded_and_require_calendar_capability() {
        let calendar = Arc::new(FakeCalendar {
            events: vec![CalendarEvent {
                id: "event-1".into(),
                title: "设计评审".into(),
                start_ms: 1_000,
                end_ms: 2_000,
                all_day: false,
                location: Some("会议室".into()),
                notes: None,
                calendar: Some("工作".into()),
            }],
            ..FakeCalendar::default()
        });
        let h = harness(DeviceCapabilities {
            calendar: Some(calendar.clone()),
            ..DeviceCapabilities::default()
        })
        .await;

        let (ok, _, _, code) = execute(
            &h,
            AppBridgeOperationDto::CalendarListEvents,
            json!({"startMs": 0, "endMs": 86_400_000}),
        )
        .await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("capability_not_declared"));

        declare_and_grant(&h, AppCapability::Calendar);
        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::CalendarListEvents,
            json!({"startMs": 0, "endMs": 86_400_000, "limit": 10_000}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result[0]["title"], "设计评审");
        assert_eq!(calendar.queries.lock().unwrap()[0].limit, 100);
    }

    #[tokio::test]
    async fn contacts_search_is_trimmed_and_bounded() {
        let contacts = Arc::new(FakeContacts {
            contacts: vec![Contact {
                id: "contact-1".into(),
                display_name: "林夕".into(),
                phones: vec!["13800000000".into()],
                emails: vec!["lingxi@example.com".into()],
            }],
            ..FakeContacts::default()
        });
        let h = harness(DeviceCapabilities {
            contacts: Some(contacts.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Contacts);

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::ContactsSearch,
            json!({"query": "  林夕 ", "limit": 500}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result[0]["display_name"], "林夕");
        let query = &contacts.queries.lock().unwrap()[0];
        assert_eq!(query.query, "林夕");
        assert_eq!(query.limit, 50);
    }

    #[tokio::test]
    async fn media_get_retrieves_only_the_app_owned_handle() {
        let camera = FakeCamera::with_bytes(vec![4, 5, 6]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);
        let (ok, capture, error, code) =
            execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        let media_id = capture["mediaId"].as_str().expect("media id").to_string();

        declare_and_grant(&h, AppCapability::Media);
        let (ok, media, error, code) = execute(
            &h,
            AppBridgeOperationDto::MediaGet,
            json!({"mediaId": media_id}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(media["mimeType"], "image/jpeg");
        assert_eq!(media["bytes"], 3);

        let (ok, _, _, code) = execute(
            &h,
            AppBridgeOperationDto::MediaGet,
            json!({"mediaId": "other-app-handle"}),
        )
        .await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("not_found"));
    }

    #[tokio::test]
    async fn an_os_level_denial_maps_to_permission_denied() {
        let camera = FakeCamera::failing(CameraError::PermissionDenied);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("permission_denied"));
    }

    #[tokio::test]
    async fn a_first_use_prompt_allows_once_and_proceeds() {
        let camera = FakeCamera::with_bytes(vec![5]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare(&h, AppCapability::Camera);

        let resolver = {
            let sink = h.sink.clone();
            let broker = h.broker.clone();
            tokio::spawn(async move {
                loop {
                    for event in sink.events().await {
                        if let ClientEvent::AppEvent {
                            event: AppEventDto::AppCapabilityRequested { request },
                        } = event
                        {
                            assert!(
                                broker
                                    .resolve_capability(
                                        &request.request_id,
                                        AppAuthorizationDecisionDto::AllowOnce,
                                    )
                                    .await
                            );
                            return;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };

        let (ok, result, error, code) = timeout(
            Duration::from_secs(5),
            execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})),
        )
        .await
        .expect("prompt resolves");
        assert!(ok, "{error:?} {code:?}");
        assert!(result["base64"].is_string());
        resolver.await.expect("resolver completes");
    }

    // ---- recording --------------------------------------------------------

    #[tokio::test]
    async fn record_stop_returns_the_recording_with_duration() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, started, _, _) =
            execute(&h, AppBridgeOperationDto::RecordAudioStart, json!({})).await;
        assert!(ok);
        assert_eq!(started["started"], true);
        assert_eq!(started["maxDurationMs"], 120_000);

        let (ok, result, error, code) =
            execute(&h, AppBridgeOperationDto::RecordAudioStop, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["mimeType"], "audio/m4a");
        assert!(result["durationMs"].is_u64());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result["base64"].as_str().expect("base64"))
            .expect("decodes");
        assert_eq!(bytes, vec![7, 7, 7]);
        assert!(voice.stopped.load(Ordering::SeqCst));
    }

    /// The first mic use of an app's life begins with an OS permission
    /// alert, and the user may leave it on screen indefinitely. Nothing that
    /// RECLAIMS the recorder may sit behind that: a runtime stop awaits
    /// `force_stop_recording`, so holding the state lock across the native
    /// start would hang an app teardown on an unanswered system dialog.
    #[tokio::test]
    async fn a_start_waiting_on_the_os_permission_alert_does_not_block_a_reclaim() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let voice = Arc::new(FakeVoice {
            start_gate: Some(gate.clone()),
            ..FakeVoice::default()
        });
        let h = harness(DeviceCapabilities {
            voice: Some(voice.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let starting = {
            let broker = h.broker.clone();
            let app_id = h.app_id.clone();
            tokio::spawn(async move {
                broker
                    .execute_bridge(AppBridgeRequestDto {
                        request_id: "start-1".into(),
                        app_id,
                        operation: AppBridgeOperationDto::RecordAudioStart,
                        payload_json: Some("{}".into()),
                    })
                    .await;
            })
        };
        // Let the start reach the (blocked) native call.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !starting.is_finished(),
            "the fixture start must still be pending"
        );

        // The reclaim path must answer while that alert is still up.
        timeout(
            Duration::from_millis(500),
            h.broker.force_stop_recording(&h.app_id),
        )
        .await
        .expect(
            "force_stop_recording blocked behind an in-flight start — a runtime stop would \
             hang on an unanswered OS permission alert",
        );

        gate.notify_one();
        starting.await.expect("the start completes");
    }

    #[tokio::test]
    async fn a_stop_without_a_recording_is_typed() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::RecordAudioStop, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("not_recording"));
    }

    #[tokio::test]
    async fn a_recording_held_by_another_app_is_busy() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, _, _, _) = execute(&h, AppBridgeOperationDto::RecordAudioStart, json!({})).await;
        assert!(ok);

        // A second app on the same broker. Declared+granted so only the
        // cross-app hold can refuse it.
        let record = h
            .service
            .create_app(Some("Second"), "second app", None)
            .await
            .expect("second app");
        let layout = AppLayout::new(h.layout.root().to_path_buf(), record.id.clone())
            .expect("second layout");
        let mut manifest = load_manifest(&layout).expect("second manifest");
        manifest.capabilities.push(AppCapability::Microphone);
        save_manifest(&layout, &manifest).expect("second declare");
        let mut permissions = load_permissions(&layout).expect("second permissions");
        permissions.grant(AppCapability::Microphone);
        save_permissions(&layout, &permissions).expect("second grant");
        h.broker
            .execute_bridge(AppBridgeRequestDto {
                request_id: "req-b".into(),
                app_id: record.id,
                operation: AppBridgeOperationDto::RecordAudioStart,
                payload_json: Some("{}".into()),
            })
            .await;
        let response = h
            .sink
            .events()
            .await
            .into_iter()
            .rev()
            .find_map(|event| match event {
                ClientEvent::AppEvent {
                    event: AppEventDto::AppBridgeResponse { response },
                } if response.request_id == "req-b" => Some(response),
                _ => None,
            })
            .expect("second response");
        assert!(!response.ok);
        assert_eq!(response.error_code.as_deref(), Some("audio_session_busy"));
    }

    #[tokio::test]
    async fn a_same_app_restart_replaces_the_orphaned_recording() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, _, _, _) = execute(&h, AppBridgeOperationDto::RecordAudioStart, json!({})).await;
        assert!(ok);
        // A reloaded page starts again: the orphan is reclaimed, not fatal.
        let (ok, restarted, error, code) =
            execute(&h, AppBridgeOperationDto::RecordAudioStart, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(restarted["replacedActive"], true);
        // The replaced session's bytes are discarded; the new one still stops.
        let (ok, _, _, _) = execute(&h, AppBridgeOperationDto::RecordAudioStop, json!({})).await;
        assert!(ok);
    }

    #[tokio::test(start_paused = true)]
    async fn the_watchdog_auto_stops_and_caches_the_recording() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, _, _, _) = execute(
            &h,
            AppBridgeOperationDto::RecordAudioStart,
            json!({"maxDurationMs": 1_000}),
        )
        .await;
        assert!(ok);

        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            voice.stopped.load(Ordering::SeqCst),
            "the watchdog must stop the native recorder at the duration cap"
        );

        let (ok, result, error, code) =
            execute(&h, AppBridgeOperationDto::RecordAudioStop, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["autoStopped"], true);
        assert!(result["base64"].is_string());
    }

    #[tokio::test]
    async fn stopping_the_runtime_reclaims_an_active_recording() {
        let voice = Arc::new(FakeVoice::default());
        let h = harness(DeviceCapabilities {
            voice: Some(voice.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let started = h
            .broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "start"}))
            .await
            .expect("runtime starts");
        assert_eq!(started["state"], "running");
        let (ok, _, _, _) = execute(&h, AppBridgeOperationDto::RecordAudioStart, json!({})).await;
        assert!(ok);

        h.broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "stop"}))
            .await
            .expect("runtime stops");
        assert!(
            voice.stopped.load(Ordering::SeqCst),
            "a runtime stop must release the recorder (and its audio-session lease)"
        );
        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::RecordAudioStop, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("not_recording"));
    }

    // ---- location / notifications -----------------------------------------

    struct FakeStt(String);

    #[async_trait]
    impl platform_api::SpeechToText for FakeStt {
        async fn transcribe(
            &self,
            _opts: platform_api::SttOpts,
        ) -> Result<platform_api::SttTranscript, platform_api::SttError> {
            Ok(platform_api::SttTranscript {
                text: self.0.clone(),
                language: Some("zh-CN".into()),
                confidence: Some(0.9),
            })
        }
    }

    /// The audio path that actually exists on this stack: listen, transcribe,
    /// hand back text the app can send to the model.
    #[tokio::test]
    async fn transcribe_speech_returns_text_under_the_microphone_capability() {
        let h = harness(DeviceCapabilities {
            stt: Some(Arc::new(FakeStt("明天下午三点开会".into()))),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Microphone);

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::TranscribeSpeech,
            json!({"language": "zh-CN"}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["text"], "明天下午三点开会");
        assert_eq!(result["language"], "zh-CN");
    }

    #[tokio::test]
    async fn transcribe_speech_is_refused_when_the_microphone_is_undeclared() {
        let h = harness(DeviceCapabilities {
            stt: Some(Arc::new(FakeStt("不该到这里".into()))),
            ..DeviceCapabilities::default()
        })
        .await;

        let (ok, _, _, code) =
            execute(&h, AppBridgeOperationDto::TranscribeSpeech, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("capability_not_declared"));
    }

    /// A capture is addressable right after it is taken, and dies with the
    /// page that took it.
    #[tokio::test]
    async fn a_capture_publishes_a_media_handle_that_a_runtime_stop_clears() {
        let camera = FakeCamera::with_bytes(vec![4, 5, 6]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Camera);

        let (ok, result, _, _) = execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})).await;
        assert!(ok);
        let media_id = result["mediaId"].as_str().expect("mediaId").to_string();
        let entry = h
            .broker
            .media_entry(&h.app_id, &media_id)
            .expect("the capture is addressable");
        assert_eq!(*entry.bytes, vec![4, 5, 6]);
        assert_eq!(entry.media_type, "image/jpeg");

        h.broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "start"}))
            .await
            .expect("start");
        h.broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "stop"}))
            .await
            .expect("stop");
        assert!(
            h.broker.media_entry(&h.app_id, &media_id).is_none(),
            "handles are a live page's hand-off buffer, not storage"
        );
    }

    /// "Allow for this session" must not outlive the session. The grant
    /// lives only in memory, so a stale one behaves as "always allow" while
    /// staying invisible to permissions.json and unrevokable short of a full
    /// reset — the opposite of what the user was asked.
    #[tokio::test]
    async fn a_session_grant_does_not_survive_the_runtime_it_was_given_in() {
        let camera = FakeCamera::with_bytes(vec![1]);
        let h = harness(DeviceCapabilities {
            camera: Some(camera),
            ..DeviceCapabilities::default()
        })
        .await;
        declare(&h, AppCapability::Camera);
        h.broker
            .session_permissions
            .lock()
            .await
            .grant(&h.app_id, AppCapability::Camera);

        // The grant is live: no prompt, straight through.
        let (ok, _, error, code) = timeout(
            Duration::from_secs(2),
            execute(&h, AppBridgeOperationDto::CapturePhoto, json!({})),
        )
        .await
        .expect("a session grant answers without a prompt");
        assert!(ok, "{error:?} {code:?}");

        h.broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "start"}))
            .await
            .expect("start");
        h.broker
            .manage_runtime_value(json!({"app_id": h.app_id, "action": "stop"}))
            .await
            .expect("stop");

        assert!(
            !h.broker
                .session_permissions
                .lock()
                .await
                .allows(&h.app_id, AppCapability::Camera),
            "the session grant must lapse with the runtime it was given in"
        );
    }

    #[tokio::test]
    async fn get_location_returns_the_fix() {
        let h = harness(DeviceCapabilities {
            location: Some(Arc::new(FakeLocation {
                hang: false,
                error: None,
            })),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Location);

        let (ok, result, error, code) =
            execute(&h, AppBridgeOperationDto::GetLocation, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["latitude"], 31.2304);
        assert_eq!(result["longitude"], 121.4737);
        assert_eq!(result["accuracyM"], 65.0);
        assert_eq!(result["timestampMs"], 1_753_000_000_000u64);
    }

    #[tokio::test(start_paused = true)]
    async fn get_location_times_out_typed() {
        let h = harness(DeviceCapabilities {
            location: Some(Arc::new(FakeLocation {
                hang: true,
                error: None,
            })),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Location);

        let (ok, _, _, code) = execute(&h, AppBridgeOperationDto::GetLocation, json!({})).await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn post_notification_prefixes_the_identifier_per_app() {
        let notifications = Arc::new(FakeNotifications::default());
        let h = harness(DeviceCapabilities {
            notifications: Some(notifications.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Notifications);

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::PostNotification,
            json!({"title": "提醒", "body": "该喝水了", "tag": "hydrate"}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        let expected_tag = format!("local-app.{}.hydrate", h.app_id);
        {
            let requests = notifications.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].title, "提醒");
            assert_eq!(requests[0].body, "该喝水了");
            assert_eq!(
                requests[0].tag.as_deref(),
                Some(expected_tag.as_str()),
                "the app-scoped prefix must be applied BEFORE the request reaches \
                 the native layer, so no app can replace another app's (or the \
                 assistant's) notification"
            );
        }

        // The page gets ITS OWN tag back, never the composed identifier: a
        // `tag` exists to be passed back so a later post REPLACES this one,
        // and the composed form contains `.`, which this operation's own
        // grammar rejects. Echoing it would hand the page a value that fails
        // validation on the very next call.
        assert_eq!(result["tag"], "hydrate");
        let (ok, second, error, code) = execute(
            &h,
            AppBridgeOperationDto::PostNotification,
            json!({
                "title": "提醒",
                "body": "还是该喝水了",
                "tag": result["tag"].as_str().expect("tag"),
            }),
        )
        .await;
        assert!(
            ok,
            "the returned tag must be re-postable: {error:?} {code:?}"
        );
        assert_eq!(second["tag"], "hydrate");
        let requests = notifications.requests.lock().unwrap();
        assert_eq!(
            requests[1].tag.as_deref(),
            Some(expected_tag.as_str()),
            "a replace must resolve to the SAME native identifier as the first post"
        );
    }

    #[tokio::test]
    async fn post_notification_rejects_an_illegal_tag_or_oversized_title() {
        let notifications = Arc::new(FakeNotifications::default());
        let h = harness(DeviceCapabilities {
            notifications: Some(notifications.clone()),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Notifications);

        let (ok, _, _, code) = execute(
            &h,
            AppBridgeOperationDto::PostNotification,
            json!({"title": "t", "body": "b", "tag": "../escape"}),
        )
        .await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("invalid_request"));

        let (ok, _, _, code) = execute(
            &h,
            AppBridgeOperationDto::PostNotification,
            json!({"title": "字".repeat(101), "body": "b"}),
        )
        .await;
        assert!(!ok);
        assert_eq!(code.as_deref(), Some("invalid_request"));
        assert!(notifications.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn clipboard_share_and_tts_use_declared_native_capabilities() {
        let clipboard = Arc::new(FakeClipboard::default());
        let share = Arc::new(FakeShare::default());
        let h = harness(DeviceCapabilities {
            clipboard: Some(clipboard.clone()),
            share: Some(share.clone()),
            tts: Some(Arc::new(FakeTts)),
            ..DeviceCapabilities::default()
        })
        .await;
        declare_and_grant(&h, AppCapability::Clipboard);
        declare_and_grant(&h, AppCapability::Share);
        declare_and_grant(&h, AppCapability::TextToSpeech);

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::ClipboardSetText,
            json!({"text": "copied"}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["written"], true);
        assert_eq!(clipboard.text.lock().unwrap().as_deref(), Some("copied"));

        let (ok, result, error, code) =
            execute(&h, AppBridgeOperationDto::ClipboardGetText, json!({})).await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["text"], "copied");

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::Share,
            json!({"text": "share me", "url": "https://example.com"}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["shared"], true);
        let payloads = share.payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].text.as_deref(), Some("share me"));
        assert_eq!(payloads[0].url.as_deref(), Some("https://example.com"));
        drop(payloads);

        let (ok, result, error, code) = execute(
            &h,
            AppBridgeOperationDto::SynthesizeSpeech,
            json!({"text": "speech", "voice": "default"}),
        )
        .await;
        assert!(ok, "{error:?} {code:?}");
        assert_eq!(result["mimeType"], "audio/pcm");
        assert_eq!(result["sampleRateHz"], 24_000);
        let encoded = result["base64"].as_str().expect("audio base64");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            b"speech"
        );
    }
}
