//! Signaling records and rules of `lingxi-rc/1`: what the viewer and the host
//! write into the Realtime Database, where, how those records are signed, and
//! how untrusted data read back from RTDB is validated.
//!
//! Signing inputs are plain text, one field per line, joined with `\n` (no
//! trailing newline). Free text is `b64u(UTF-8)`, an absent field is `-`, and the
//! SDP enters as `b64u(SHA-256(UTF-8 sdp))` so its `a=fingerprint` is signed too.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::clock::Clock;
use crate::encoding::{b64u_decode, b64u_encode, is_b64u, sha256};
use crate::error::ProtocolError;
use crate::keys::{id_from_key, verify_signature, Signer};
use crate::version::{OFFER_MAX_AGE_MS, ONLINE_WINDOW_MS, SDP_MAX_CHARS};

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// What kind of app is on the viewer side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DevicePlatform {
    /// A browser.
    Web,
    /// The Android app.
    Android,
    /// The iOS app.
    Ios,
}

impl DevicePlatform {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Android => "android",
            Self::Ios => "ios",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "web" => Some(Self::Web),
            "android" => Some(Self::Android),
            "ios" => Some(Self::Ios),
            _ => None,
        }
    }
}

/// How a connection carries its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    /// A WebRTC DataChannel.
    Webrtc,
    /// The encrypted RTDB tunnel.
    Tunnel,
}

impl TransportKind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webrtc => "webrtc",
            Self::Tunnel => "tunnel",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "webrtc" => Some(Self::Webrtc),
            "tunnel" => Some(Self::Tunnel),
            _ => None,
        }
    }
}

/// Why a host refused a connection (`AnswerRecord.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerRejectReason {
    /// The device was removed on the host.
    Revoked,
    /// Remote control is switched off on the host.
    Disabled,
    /// The host has too many connections.
    Capacity,
    /// The protocol major differs.
    UpgradeRequired,
    /// The offer's signature or id did not verify.
    BadSignature,
    /// The offer's `created_at` is too far from the host's clock.
    Expired,
    /// The host cannot take the connection right now.
    Busy,
}

impl AnswerRejectReason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
            Self::Capacity => "capacity",
            Self::UpgradeRequired => "upgrade_required",
            Self::BadSignature => "bad_signature",
            Self::Expired => "expired",
            Self::Busy => "busy",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "revoked" => Self::Revoked,
            "disabled" => Self::Disabled,
            "capacity" => Self::Capacity,
            "upgrade_required" => Self::UpgradeRequired,
            "bad_signature" => Self::BadSignature,
            "expired" => Self::Expired,
            "busy" => Self::Busy,
            _ => return None,
        })
    }
}

/// Whether an answer accepts or refuses the offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerStatus {
    /// The host takes the connection.
    Accepted,
    /// The host refuses it; see the reason.
    Rejected,
}

impl AnswerStatus {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

/// The viewer's identity as carried in an offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferDevice {
    /// `id_from_key(key)`.
    pub id: String,
    /// `b64u` of the 32-byte raw Ed25519 device key.
    pub key: String,
    /// Display name of the device.
    pub name: String,
    /// What kind of app it is.
    pub platform: DevicePlatform,
    /// Version of the app.
    pub app_version: String,
}

/// The signed part of an offer (everything but `created_at` and `sig`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferDraft {
    /// Record version, always 1.
    pub v: u32,
    /// `Tunnel`: the viewer already knows a direct connection will not work.
    pub transport: TransportKind,
    /// The complete offer SDP after ICE gathering; required for `webrtc`, absent for `tunnel`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp: Option<String>,
    /// The viewer's identity.
    pub device: OfferDevice,
    /// `b64u` of the viewer's 32-byte X25519 public key.
    pub eph: String,
    /// `b64u` of 16 random bytes.
    pub nonce: String,
}

/// `to_host/{host_id}/{conn_id}/offer`, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferRecord {
    /// The signed fields.
    pub draft: OfferDraft,
    /// Server time of the write; not part of the signature.
    pub created_at: u64,
    /// `b64u` Ed25519 signature by the device key over [`offer_signing_input`].
    pub sig: String,
}

/// A signed offer ready to be written; `created_at` is the server-timestamp placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferWrite {
    /// The signed fields.
    pub draft: OfferDraft,
    /// `b64u` signature.
    pub sig: String,
}

/// The host as named in an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerHost {
    /// `id_from_key(key)`.
    pub id: String,
    /// `b64u` of the 32-byte raw Ed25519 host key.
    pub key: String,
}

/// The signed part of an answer (everything but `created_at` and `sig`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerDraft {
    /// Record version, always 1.
    pub v: u32,
    /// Accepted or rejected.
    pub status: AnswerStatus,
    /// Why the host refused (rejected answers only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<AnswerRejectReason>,
    /// The transport the host chose; required when accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportKind>,
    /// The complete answer SDP; required for an accepted `webrtc` answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp: Option<String>,
    /// The host's identity.
    pub host: AnswerHost,
    /// `b64u` of the host's X25519 public key; required for an accepted `tunnel` answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eph: Option<String>,
    /// `b64u` of 16 random bytes.
    pub nonce: String,
}

/// `to_viewer/{host_id}/{conn_id}/answer`, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerRecord {
    /// The signed fields.
    pub draft: AnswerDraft,
    /// Server time of the write; not part of the signature.
    pub created_at: u64,
    /// `b64u` Ed25519 signature by the host key over [`answer_signing_input`].
    pub sig: String,
}

/// A signed answer ready to be written; `created_at` is the server-timestamp placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerWrite {
    /// The signed fields.
    pub draft: AnswerDraft,
    /// `b64u` signature.
    pub sig: String,
}

/// `hosts/{host_id}`, written by the desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    /// Record version, always 1.
    pub v: u32,
    /// Display name of the computer.
    pub name: String,
    /// Operating-system family of the desktop.
    pub platform: String,
    /// Version of the desktop app.
    pub app_version: String,
    /// Major of the `client-protocol` contract the host speaks.
    pub client_protocol_version: u32,
    /// `b64u` of the 32-byte raw Ed25519 host key.
    pub host_key: String,
    /// Whether the host says it is up.
    pub online: bool,
    /// Server time of the last heartbeat.
    pub last_seen: u64,
    /// Server time the host process started.
    pub started_at: u64,
}

/// The value RTDB replaces with its own clock (ms since the epoch) on write.
pub fn server_timestamp() -> Value {
    json!({ ".sv": "timestamp" })
}

fn with_created_at<T: Serialize>(draft: &T, sig: &str) -> Value {
    let mut value = serde_json::to_value(draft).expect("records serialize");
    let object = value.as_object_mut().expect("records are objects");
    object.insert("created_at".into(), server_timestamp());
    object.insert("sig".into(), Value::String(sig.to_owned()));
    value
}

impl OfferWrite {
    /// The JSON to PUT at the offer path.
    pub fn to_value(&self) -> Value {
        with_created_at(&self.draft, &self.sig)
    }
}

impl AnswerWrite {
    /// The JSON to PUT at the answer path.
    pub fn to_value(&self) -> Value {
        with_created_at(&self.draft, &self.sig)
    }
}

impl OfferRecord {
    /// The JSON form of the record as stored (with a concrete `created_at`).
    pub fn to_value(&self) -> Value {
        let mut value = serde_json::to_value(&self.draft).expect("records serialize");
        let object = value.as_object_mut().expect("records are objects");
        object.insert("created_at".into(), json!(self.created_at));
        object.insert("sig".into(), Value::String(self.sig.clone()));
        value
    }
}

impl AnswerRecord {
    /// The JSON form of the record as stored (with a concrete `created_at`).
    pub fn to_value(&self) -> Value {
        let mut value = serde_json::to_value(&self.draft).expect("records serialize");
        let object = value.as_object_mut().expect("records are objects");
        object.insert("created_at".into(), json!(self.created_at));
        object.insert("sig".into(), Value::String(self.sig.clone()));
        value
    }
}

// ---------------------------------------------------------------------------
// RTDB paths
// ---------------------------------------------------------------------------

const ROOT: &str = "remote-control";
const MAX_SEGMENT_BYTES: usize = 768;
/// Largest sequence number a frame key carries (ten decimal digits).
pub const MAX_FRAME_SEQ: u64 = 9_999_999_999;

/// `true` when `value` may be used as one RTDB path segment.
pub fn is_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SEGMENT_BYTES
        && !value.chars().any(|c| {
            matches!(c, '.' | '#' | '$' | '[' | ']' | '/') || c <= '\u{1f}' || c == '\u{7f}'
        })
}

fn seg<'a>(value: &'a str, label: &str) -> Result<&'a str, ProtocolError> {
    if is_path_segment(value) {
        Ok(value)
    } else {
        Err(ProtocolError::new(format!("invalid {label}")))
    }
}

fn join(uid: &str, rest: &[&str]) -> Result<String, ProtocolError> {
    let mut path = format!("/{ROOT}/{}", seg(uid, "uid")?);
    for part in rest {
        path.push('/');
        path.push_str(part);
    }
    Ok(path)
}

/// `/remote-control/{uid}/hosts` — the viewer lists the user's computers here.
pub fn hosts_path(uid: &str) -> Result<String, ProtocolError> {
    join(uid, &["hosts"])
}

/// `/remote-control/{uid}/hosts/{host_id}`.
pub fn host_path(uid: &str, host_id: &str) -> Result<String, ProtocolError> {
    join(uid, &["hosts", seg(host_id, "host_id")?])
}

/// `/remote-control/{uid}/to_host/{host_id}` — the host's inbox; the host keeps one SSE here.
pub fn to_host_path(uid: &str, host_id: &str) -> Result<String, ProtocolError> {
    join(uid, &["to_host", seg(host_id, "host_id")?])
}

/// `…/to_host/{host_id}/{conn_id}` — everything one connection sends to the host.
pub fn to_host_conn_path(uid: &str, host_id: &str, conn_id: &str) -> Result<String, ProtocolError> {
    join(
        uid,
        &[
            "to_host",
            seg(host_id, "host_id")?,
            seg(conn_id, "conn_id")?,
        ],
    )
}

/// `…/to_host/{host_id}/{conn_id}/offer`.
pub fn offer_path(uid: &str, host_id: &str, conn_id: &str) -> Result<String, ProtocolError> {
    Ok(format!(
        "{}/offer",
        to_host_conn_path(uid, host_id, conn_id)?
    ))
}

/// `…/to_host/{host_id}/{conn_id}/up` — tunnel frames, viewer to host.
pub fn up_path(uid: &str, host_id: &str, conn_id: &str) -> Result<String, ProtocolError> {
    Ok(format!("{}/up", to_host_conn_path(uid, host_id, conn_id)?))
}

/// `/remote-control/{uid}/to_viewer/{host_id}` — all answers and tunnel frames for one host's viewers.
pub fn to_viewer_path(uid: &str, host_id: &str) -> Result<String, ProtocolError> {
    join(uid, &["to_viewer", seg(host_id, "host_id")?])
}

/// `…/to_viewer/{host_id}/{conn_id}` — everything the host sends one connection.
pub fn to_viewer_conn_path(
    uid: &str,
    host_id: &str,
    conn_id: &str,
) -> Result<String, ProtocolError> {
    join(
        uid,
        &[
            "to_viewer",
            seg(host_id, "host_id")?,
            seg(conn_id, "conn_id")?,
        ],
    )
}

/// `…/to_viewer/{host_id}/{conn_id}/answer`.
pub fn answer_path(uid: &str, host_id: &str, conn_id: &str) -> Result<String, ProtocolError> {
    Ok(format!(
        "{}/answer",
        to_viewer_conn_path(uid, host_id, conn_id)?
    ))
}

/// `…/to_viewer/{host_id}/{conn_id}/down` — tunnel frames, host to viewer.
pub fn down_path(uid: &str, host_id: &str, conn_id: &str) -> Result<String, ProtocolError> {
    Ok(format!(
        "{}/down",
        to_viewer_conn_path(uid, host_id, conn_id)?
    ))
}

/// Key of tunnel frame `seq` under `up`/`down`: `f0`, `f1`, … (the prefix keeps RTDB from returning arrays).
pub fn frame_key(seq: u64) -> Result<String, ProtocolError> {
    if seq > MAX_FRAME_SEQ {
        return Err(ProtocolError::new("tunnel frame sequence out of range"));
    }
    Ok(format!("f{seq}"))
}

fn canonical_number(digits: &str) -> Option<u64> {
    let valid = !digits.is_empty()
        && digits.len() <= 10
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !valid {
        return None;
    }
    digits.parse().ok().filter(|seq| *seq <= MAX_FRAME_SEQ)
}

/// Sequence number of a frame key, or `None` for anything else. Besides the
/// canonical `f12` this accepts a bare `12`, which is how an array-coerced
/// RTDB value presents the same frame.
pub fn parse_frame_key(key: &str) -> Option<u64> {
    canonical_number(key.strip_prefix('f').unwrap_or(key))
}

/// `…/up/f{seq}`.
pub fn up_frame_path(
    uid: &str,
    host_id: &str,
    conn_id: &str,
    seq: u64,
) -> Result<String, ProtocolError> {
    Ok(format!(
        "{}/{}",
        up_path(uid, host_id, conn_id)?,
        frame_key(seq)?
    ))
}

/// `…/down/f{seq}`.
pub fn down_frame_path(
    uid: &str,
    host_id: &str,
    conn_id: &str,
    seq: u64,
) -> Result<String, ProtocolError> {
    Ok(format!(
        "{}/{}",
        down_path(uid, host_id, conn_id)?,
        frame_key(seq)?
    ))
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// First line of an offer's signing input.
pub const OFFER_SIGNING_DOMAIN: &str = "lingxi-rc-offer/1";
/// First line of an answer's signing input.
pub const ANSWER_SIGNING_DOMAIN: &str = "lingxi-rc-answer/1";

/// The path coordinates a record is bound to; all three enter the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningContext {
    /// The user's Firebase uid.
    pub uid: String,
    /// The host's id.
    pub host_id: String,
    /// The connection id.
    pub conn_id: String,
}

fn line<'a>(value: &'a str, label: &str) -> Result<&'a str, ProtocolError> {
    if value.contains(['\r', '\n']) {
        Err(ProtocolError::new(format!("{label} contains a line break")))
    } else {
        Ok(value)
    }
}

fn text(value: &str) -> String {
    b64u_encode(value.as_bytes())
}

fn sdp_digest(sdp: Option<&str>) -> String {
    match sdp {
        None => "-".to_owned(),
        Some(sdp) => b64u_encode(&sha256(sdp.as_bytes())),
    }
}

/// The exact text the device key signs for an offer.
pub fn offer_signing_input(
    ctx: &SigningContext,
    offer: &OfferDraft,
) -> Result<String, ProtocolError> {
    let device = &offer.device;
    let lines = [
        OFFER_SIGNING_DOMAIN.to_owned(),
        line(&ctx.uid, "uid")?.to_owned(),
        line(&ctx.host_id, "host_id")?.to_owned(),
        line(&ctx.conn_id, "conn_id")?.to_owned(),
        line(&device.id, "device.id")?.to_owned(),
        line(&device.key, "device.key")?.to_owned(),
        text(&device.name),
        device.platform.as_str().to_owned(),
        text(&device.app_version),
        offer.transport.as_str().to_owned(),
        sdp_digest(offer.sdp.as_deref()),
        line(&offer.eph, "eph")?.to_owned(),
        line(&offer.nonce, "nonce")?.to_owned(),
    ];
    Ok(lines.join("\n"))
}

/// The exact text the host key signs for an answer. `offer_nonce` is the nonce
/// of the offer being answered, which ties the answer to that one offer.
pub fn answer_signing_input(
    ctx: &SigningContext,
    offer_nonce: &str,
    answer: &AnswerDraft,
) -> Result<String, ProtocolError> {
    let lines = [
        ANSWER_SIGNING_DOMAIN.to_owned(),
        line(&ctx.uid, "uid")?.to_owned(),
        line(&ctx.host_id, "host_id")?.to_owned(),
        line(&ctx.conn_id, "conn_id")?.to_owned(),
        line(offer_nonce, "offer nonce")?.to_owned(),
        answer.status.as_str().to_owned(),
        answer
            .reason
            .map_or("-", AnswerRejectReason::as_str)
            .to_owned(),
        answer
            .transport
            .map_or("-", TransportKind::as_str)
            .to_owned(),
        sdp_digest(answer.sdp.as_deref()),
        line(answer.eph.as_deref().unwrap_or("-"), "eph")?.to_owned(),
        line(&answer.nonce, "nonce")?.to_owned(),
    ];
    Ok(lines.join("\n"))
}

/// Why signing failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    /// A field cannot be put into the signing input.
    #[error(transparent)]
    Input(#[from] ProtocolError),
    /// The signer failed.
    #[error(transparent)]
    Signer(#[from] crate::keys::SignerError),
}

/// Sign an offer with the device key.
pub async fn sign_offer(
    ctx: &SigningContext,
    signer: &dyn Signer,
    draft: OfferDraft,
) -> Result<OfferWrite, SignError> {
    let input = offer_signing_input(ctx, &draft)?;
    let signature = signer.sign(input.as_bytes()).await?;
    Ok(OfferWrite {
        draft,
        sig: b64u_encode(&signature),
    })
}

/// Sign an answer with the host key.
pub async fn sign_answer(
    ctx: &SigningContext,
    signer: &dyn Signer,
    offer_nonce: &str,
    draft: AnswerDraft,
) -> Result<AnswerWrite, SignError> {
    let input = answer_signing_input(ctx, offer_nonce, &draft)?;
    let signature = signer.sign(input.as_bytes()).await?;
    Ok(AnswerWrite {
        draft,
        sig: b64u_encode(&signature),
    })
}

/// Why a record failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifyFailure {
    /// Not a record of the expected shape or size; drop it, do not answer.
    BadShape,
    /// An id does not match its key, or the host id is not the one asked for.
    IdMismatch,
    /// The host key differs from the pinned (or advertised) one.
    KeyMismatch,
    /// The signature does not verify.
    BadSignature,
    /// `created_at` is further from the local clock than the allowed age.
    Expired,
}

impl VerifyFailure {
    /// The TypeScript spelling, used in failure details.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadShape => "bad_shape",
            Self::IdMismatch => "id_mismatch",
            Self::KeyMismatch => "key_mismatch",
            Self::BadSignature => "bad_signature",
            Self::Expired => "expired",
        }
    }
}

/// A verification failure with its optional detail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .detail.as_deref().map_or_else(|| reason.as_str().to_owned(), |d| format!("{}: {d}", reason.as_str())))]
pub struct VerifyError {
    /// What failed.
    pub reason: VerifyFailure,
    /// Human-readable detail (no secrets).
    pub detail: Option<String>,
}

fn fail(reason: VerifyFailure, detail: Option<&str>) -> VerifyError {
    VerifyError {
        reason,
        detail: detail.map(str::to_owned),
    }
}

fn time_distance(a: u64, b: u64) -> u64 {
    a.abs_diff(b)
}

/// Steps 1-4 of the host's offer check, in the plan's order: shape and size,
/// device id against device key, signature, then `created_at` against the local
/// clock. Step 5, "each `conn_id` is handled once", is the caller's.
///
/// Suggested host reaction: `BadShape` is dropped silently; `IdMismatch` and
/// `BadSignature` are answered `rejected/bad_signature`; `Expired` is answered
/// `rejected/expired`.
pub fn verify_offer(
    ctx: &SigningContext,
    record: &Value,
    now_ms: u64,
    max_age_ms: Option<u64>,
) -> Result<OfferRecord, VerifyError> {
    let offer = parse_offer_record(record).map_err(|e| fail(VerifyFailure::BadShape, Some(&e)))?;
    let device_key = b64u_decode(&offer.draft.device.key)
        .map_err(|_| fail(VerifyFailure::BadShape, Some("device.key")))?;
    if id_from_key(&device_key) != offer.draft.device.id {
        return Err(fail(
            VerifyFailure::IdMismatch,
            Some("device.id does not match device.key"),
        ));
    }
    let input = offer_signing_input(ctx, &offer.draft)
        .map_err(|e| fail(VerifyFailure::BadShape, Some(&e.0)))?;
    let signature = b64u_decode(&offer.sig).unwrap_or_default();
    if !verify_signature(&device_key, &signature, input.as_bytes()) {
        return Err(fail(VerifyFailure::BadSignature, None));
    }
    if time_distance(now_ms, offer.created_at) > max_age_ms.unwrap_or(OFFER_MAX_AGE_MS) {
        return Err(fail(VerifyFailure::Expired, None));
    }
    Ok(offer)
}

/// Inputs of [`verify_answer`].
#[derive(Debug, Clone)]
pub struct VerifyAnswerOptions<'a> {
    /// The path coordinates the answer is bound to.
    pub ctx: &'a SigningContext,
    /// The value read from `…/answer`; anything at all.
    pub record: &'a Value,
    /// Nonce (`b64u`) of the offer this answer must belong to.
    pub offer_nonce: &'a str,
    /// Host key (`b64u`) the answer must be signed with: the one pinned at
    /// pairing, or on first pairing `hosts/{host_id}.host_key`.
    pub expected_host_key: &'a str,
    /// Local time in ms, only used with `max_age_ms`.
    pub now_ms: u64,
    /// When set, `created_at` must be within this distance of `now_ms`.
    pub max_age_ms: Option<u64>,
}

/// The viewer's answer check: shape, then the host id (it must be the one
/// dialled and derive from the key), then the key (pinned or advertised), then
/// the signature, then optionally the age.
pub fn verify_answer(options: &VerifyAnswerOptions<'_>) -> Result<AnswerRecord, VerifyError> {
    let answer =
        parse_answer_record(options.record).map_err(|e| fail(VerifyFailure::BadShape, Some(&e)))?;
    if answer.draft.host.id != options.ctx.host_id {
        return Err(fail(
            VerifyFailure::IdMismatch,
            Some("answer is from a different host"),
        ));
    }
    let host_key = b64u_decode(&answer.draft.host.key)
        .map_err(|_| fail(VerifyFailure::BadShape, Some("host.key")))?;
    if id_from_key(&host_key) != answer.draft.host.id {
        return Err(fail(
            VerifyFailure::IdMismatch,
            Some("host.id does not match host.key"),
        ));
    }
    if answer.draft.host.key != options.expected_host_key {
        return Err(fail(VerifyFailure::KeyMismatch, None));
    }
    let input = answer_signing_input(options.ctx, options.offer_nonce, &answer.draft)
        .map_err(|e| fail(VerifyFailure::BadShape, Some(&e.0)))?;
    let signature = b64u_decode(&answer.sig).unwrap_or_default();
    if !verify_signature(&host_key, &signature, input.as_bytes()) {
        return Err(fail(VerifyFailure::BadSignature, None));
    }
    if let Some(max_age) = options.max_age_ms {
        if time_distance(options.now_ms, answer.created_at) > max_age {
            return Err(fail(VerifyFailure::Expired, None));
        }
    }
    Ok(answer)
}

// ---------------------------------------------------------------------------
// Parsing untrusted records
// ---------------------------------------------------------------------------

const NAME_MAX_CHARS: usize = 128;
const VERSION_MAX_CHARS: usize = 64;
const PLATFORM_MAX_CHARS: usize = 32;

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn display_unsafe(c: char) -> bool {
    c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c) || c == '\u{2028}' || c == '\u{2029}'
}

fn display_text(value: Option<&Value>, max: usize) -> Option<&str> {
    let text = value?.as_str()?;
    let length = utf16_len(text);
    (length >= 1 && length <= max && !text.chars().any(display_unsafe)).then_some(text)
}

fn is_id(text: &str) -> bool {
    text.len() == 22
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn timestamp(value: Option<&Value>) -> Option<u64> {
    let number = value?.as_f64()?;
    (number.is_finite() && number >= 0.0).then_some(number as u64)
}

fn b64u_field(value: Option<&Value>, bytes: usize) -> Option<&str> {
    let text = value?.as_str()?;
    is_b64u(text, bytes).then_some(text)
}

fn optional_sdp(value: Option<&Value>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(Value::String(sdp)) if !sdp.is_empty() && utf16_len(sdp) <= SDP_MAX_CHARS => {
            Ok(Some(sdp.clone()))
        }
        Some(_) => Err("sdp: not a string of 1..16384 characters".into()),
    }
}

fn is_version_one(value: Option<&Value>) -> bool {
    value.and_then(Value::as_u64) == Some(1)
}

/// Validate a value read from `…/offer`. Unknown extra fields are ignored.
pub fn parse_offer_record(raw: &Value) -> Result<OfferRecord, String> {
    let object = raw.as_object().ok_or("offer: not an object")?;
    if !is_version_one(object.get("v")) {
        return Err("offer: unsupported v".into());
    }
    let transport = object
        .get("transport")
        .and_then(Value::as_str)
        .and_then(TransportKind::parse)
        .ok_or("offer: bad transport")?;
    let sdp = optional_sdp(object.get("sdp"))?;
    match transport {
        TransportKind::Webrtc if sdp.is_none() => return Err("offer: webrtc needs an sdp".into()),
        TransportKind::Tunnel if sdp.is_some() => {
            return Err("offer: tunnel must not carry an sdp".into())
        }
        _ => {}
    }
    let device = object
        .get("device")
        .and_then(Value::as_object)
        .ok_or("offer: device is not an object")?;
    let id = device
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_id(id))
        .ok_or("offer: bad device.id")?;
    let key = b64u_field(device.get("key"), 32).ok_or("offer: bad device.key")?;
    let name = display_text(device.get("name"), NAME_MAX_CHARS).ok_or("offer: bad device.name")?;
    let platform = device
        .get("platform")
        .and_then(Value::as_str)
        .and_then(DevicePlatform::parse)
        .ok_or("offer: bad device.platform")?;
    let app_version = display_text(device.get("app_version"), VERSION_MAX_CHARS)
        .ok_or("offer: bad device.app_version")?;
    let eph = b64u_field(object.get("eph"), 32).ok_or("offer: bad eph")?;
    let nonce = b64u_field(object.get("nonce"), 16).ok_or("offer: bad nonce")?;
    let created_at = timestamp(object.get("created_at")).ok_or("offer: bad created_at")?;
    let sig = b64u_field(object.get("sig"), 64).ok_or("offer: bad sig")?;
    Ok(OfferRecord {
        draft: OfferDraft {
            v: 1,
            transport,
            sdp,
            device: OfferDevice {
                id: id.to_owned(),
                key: key.to_owned(),
                name: name.to_owned(),
                platform,
                app_version: app_version.to_owned(),
            },
            eph: eph.to_owned(),
            nonce: nonce.to_owned(),
        },
        created_at,
        sig: sig.to_owned(),
    })
}

/// Validate a value read from `…/answer`. Unknown extra fields are ignored.
pub fn parse_answer_record(raw: &Value) -> Result<AnswerRecord, String> {
    let object = raw.as_object().ok_or("answer: not an object")?;
    if !is_version_one(object.get("v")) {
        return Err("answer: unsupported v".into());
    }
    let status = match object.get("status").and_then(Value::as_str) {
        Some("accepted") => AnswerStatus::Accepted,
        Some("rejected") => AnswerStatus::Rejected,
        _ => return Err("answer: bad status".into()),
    };
    let reason = match object.get("reason") {
        None => None,
        Some(value) => match (status, value.as_str().and_then(AnswerRejectReason::parse)) {
            (AnswerStatus::Rejected, Some(reason)) => Some(reason),
            _ => return Err("answer: bad reason".into()),
        },
    };
    let transport = match object.get("transport") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .and_then(TransportKind::parse)
                .ok_or("answer: bad transport")?,
        ),
    };
    let sdp = optional_sdp(object.get("sdp"))?;
    let eph = match object.get("eph") {
        None => None,
        Some(_) => Some(b64u_field(object.get("eph"), 32).ok_or("answer: bad eph")?),
    };
    if status == AnswerStatus::Accepted {
        match transport.ok_or("answer: accepted without transport")? {
            TransportKind::Webrtc => {
                if sdp.is_none() {
                    return Err("answer: webrtc needs an sdp".into());
                }
            }
            TransportKind::Tunnel => {
                if sdp.is_some() {
                    return Err("answer: tunnel must not carry an sdp".into());
                }
                if eph.is_none() {
                    return Err("answer: tunnel needs an eph".into());
                }
            }
        }
    }
    let host = object
        .get("host")
        .and_then(Value::as_object)
        .ok_or("answer: host is not an object")?;
    let host_id = host
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_id(id))
        .ok_or("answer: bad host.id")?;
    let host_key = b64u_field(host.get("key"), 32).ok_or("answer: bad host.key")?;
    let nonce = b64u_field(object.get("nonce"), 16).ok_or("answer: bad nonce")?;
    let created_at = timestamp(object.get("created_at")).ok_or("answer: bad created_at")?;
    let sig = b64u_field(object.get("sig"), 64).ok_or("answer: bad sig")?;
    Ok(AnswerRecord {
        draft: AnswerDraft {
            v: 1,
            status,
            reason,
            transport,
            sdp,
            host: AnswerHost {
                id: host_id.to_owned(),
                key: host_key.to_owned(),
            },
            eph: eph.map(str::to_owned),
            nonce: nonce.to_owned(),
        },
        created_at,
        sig: sig.to_owned(),
    })
}

/// Validate a value read from `hosts/{host_id}`. Unknown extra fields are ignored.
pub fn parse_host_record(raw: &Value) -> Result<HostRecord, String> {
    let object = raw.as_object().ok_or("host: not an object")?;
    if !is_version_one(object.get("v")) {
        return Err("host: unsupported v".into());
    }
    let name = display_text(object.get("name"), NAME_MAX_CHARS).ok_or("host: bad name")?;
    let platform =
        display_text(object.get("platform"), PLATFORM_MAX_CHARS).ok_or("host: bad platform")?;
    let app_version = display_text(object.get("app_version"), VERSION_MAX_CHARS)
        .ok_or("host: bad app_version")?;
    let protocol = object
        .get("client_protocol_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("host: bad client_protocol_version")?;
    let host_key = b64u_field(object.get("host_key"), 32).ok_or("host: bad host_key")?;
    let online = object
        .get("online")
        .and_then(Value::as_bool)
        .ok_or("host: bad online")?;
    let last_seen = timestamp(object.get("last_seen")).ok_or("host: bad last_seen")?;
    let started_at = timestamp(object.get("started_at")).ok_or("host: bad started_at")?;
    Ok(HostRecord {
        v: 1,
        name: name.to_owned(),
        platform: platform.to_owned(),
        app_version: app_version.to_owned(),
        client_protocol_version: protocol,
        host_key: host_key.to_owned(),
        online,
        last_seen,
        started_at,
    })
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

/// Whether a host looks online. `online` must be set, and either its last
/// heartbeat was *received* (`received_at`, local ms) under 90 s ago, or, when
/// no update has been received yet (first load), `last_seen` is within 90 s of
/// `now_ms` in either direction, which tolerates clock skew.
pub fn is_host_online(online: bool, last_seen: u64, now_ms: u64, received_at: Option<u64>) -> bool {
    if !online {
        return false;
    }
    match received_at {
        Some(received) => now_ms.saturating_sub(received) < ONLINE_WINDOW_MS,
        None => time_distance(now_ms, last_seen) < ONLINE_WINDOW_MS,
    }
}

/// Remembers when each host's heartbeat last *changed* on this machine, so
/// [`is_host_online`] can use local receive time instead of the host's clock.
/// Feed it every host record the viewer sees; re-check [`HostPresence::is_online`]
/// on a timer, since a dead host produces no events.
pub struct HostPresence {
    clock: Arc<dyn Clock>,
    hosts: std::collections::HashMap<String, (HostRecord, Option<u64>)>,
}

impl HostPresence {
    /// An empty tracker.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            hosts: std::collections::HashMap::new(),
        }
    }

    /// Record an observed host record (the initial load and every later change).
    pub fn observe(&mut self, host_id: &str, record: HostRecord) {
        let now = self.clock.now_ms();
        let received_at = match self.hosts.get(host_id) {
            None => None,
            Some((known, _)) if known.last_seen != record.last_seen => Some(now),
            Some((_, received)) => *received,
        };
        self.hosts.insert(host_id.to_owned(), (record, received_at));
    }

    /// Forget a host that disappeared from the listing.
    pub fn forget(&mut self, host_id: &str) {
        self.hosts.remove(host_id);
    }

    /// `false` for hosts never observed.
    pub fn is_online(&self, host_id: &str) -> bool {
        self.hosts.get(host_id).is_some_and(|(record, received)| {
            is_host_online(
                record.online,
                record.last_seen,
                self.clock.now_ms(),
                *received,
            )
        })
    }
}

/// Parse every entry of a `hosts` listing, skipping malformed records.
pub fn parse_hosts_listing(raw: &Value) -> Vec<(String, HostRecord)> {
    let mut hosts: Vec<(String, HostRecord)> = match raw {
        Value::Object(map) => map
            .iter()
            .filter_map(|(id, value)| {
                parse_host_record(value)
                    .ok()
                    .map(|record| (id.clone(), record))
            })
            .collect(),
        _ => Vec::new(),
    };
    hosts.sort_by(|a, b| a.0.cmp(&b.0));
    hosts
}
