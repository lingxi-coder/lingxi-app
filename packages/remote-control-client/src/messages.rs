//! Parameter and result types of the `lingxi-rc/1` RPC methods, and the
//! [`RemoteRecord`] union that carries session content. They mirror
//! `packages/bridge-client/src/remote/messages.ts` field for field (snake_case);
//! `ClientEvent` and the DTOs it uses come from the pinned `client-protocol`.

use std::collections::BTreeMap;

use client::protocol::ask_user_question::AskUserQuestionRequestDto;
use client::protocol::commands::ImageRefDto;
use client::protocol::computer_access::ComputerAccessRequestDto;
use client::protocol::controls::{ConversationControlsDto, ReasoningSelectionDto};
use client::protocol::listings::{ModelDetailsDto, SessionModeDto};
use client::protocol::message::MessageDto;
use client::protocol::permission::{PermissionRequest, PermissionResponseDto};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decode::{decode_client_event, DecodedEvent};
use crate::media::RemoteMediaCapabilities;
use crate::signaling::DevicePlatform;

/// Method names of the protocol.
pub mod methods {
    /// Viewer request: the handshake.
    pub const INITIALIZE: &str = "initialize";
    /// Host notification: the pending device was approved.
    pub const AUTH_APPROVED: &str = "auth.approved";
    /// Host notification: the user said no.
    pub const AUTH_DENIED: &str = "auth.denied";
    /// Viewer request: the list of remote-enabled sessions.
    pub const SESSIONS_LIST: &str = "sessions.list";
    /// Viewer request: the list, then `sessions.changed` updates.
    pub const SESSIONS_WATCH: &str = "sessions.watch";
    /// Host notification: the full session list changed.
    pub const SESSIONS_CHANGED: &str = "sessions.changed";
    /// Viewer request: attach to a session.
    pub const SESSION_ATTACH: &str = "session.attach";
    /// Host notification: a snapshot page.
    pub const SESSION_SNAPSHOT: &str = "session.snapshot";
    /// Host notification: the next record.
    pub const SESSION_EVENT: &str = "session.event";
    /// Host notification: the pending set changed.
    pub const SESSION_PENDING: &str = "session.pending";
    /// Host notification: the base changed; a fresh snapshot follows.
    pub const SESSION_RESET: &str = "session.reset";
    /// Host notification: the session is no longer shared.
    pub const SESSION_CLOSED: &str = "session.closed";
    /// Viewer request: leave a session.
    pub const SESSION_DETACH: &str = "session.detach";
    /// Viewer notification: foreground state of a session.
    pub const SESSION_PRESENCE: &str = "session.presence";
    /// Viewer request: older messages.
    pub const SESSION_HISTORY: &str = "session.history";
    /// Viewer request: fetch an image.
    pub const IMAGE_GET: &str = "image.get";
    /// Viewer request: send a prompt.
    pub const PROMPT_SEND: &str = "prompt.send";
    /// Viewer request: cancel the running turn.
    pub const TURN_CANCEL: &str = "turn.cancel";
    /// Viewer request: answer a permission request.
    pub const PERMISSION_RESPOND: &str = "permission.respond";
    /// Viewer request: answer a question.
    pub const QUESTION_ANSWER: &str = "question.answer";
    /// Viewer request: dismiss a question.
    pub const QUESTION_CANCEL: &str = "question.cancel";
    /// Viewer request: change the model.
    pub const CONTROLS_SET_MODEL: &str = "controls.set_model";
    /// Viewer request: change reasoning/effort.
    pub const CONTROLS_SET_REASONING: &str = "controls.set_reasoning";
    /// Viewer request: change the permission mode.
    pub const CONTROLS_SET_PERMISSION_MODE: &str = "controls.set_permission_mode";
    /// Viewer request: list models.
    pub const MODELS_LIST: &str = "models.list";
    /// Viewer request (Bot-aware viewers only): the Bots with Remote Control on.
    pub const BOTS_LIST: &str = "bots.list";
    /// Viewer request (Bot-aware viewers only): the list, then `bots.changed` updates.
    pub const BOTS_WATCH: &str = "bots.watch";
    /// Host notification (Bot-aware viewers only): the full Bot list changed.
    pub const BOTS_CHANGED: &str = "bots.changed";
    /// Viewer request (Bot-aware viewers only): the chats of one Bot.
    pub const BOT_THREADS_LIST: &str = "bot.threads.list";
    /// Viewer request (Bot-aware viewers only): start a new chat of a Bot.
    pub const BOT_THREAD_CREATE: &str = "bot.thread.create";
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Name and build of the viewer app, sent in `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAppInfo {
    /// App version.
    pub version: String,
    /// What kind of app.
    pub platform: DevicePlatform,
    /// Explicitly declared Remote Control media support, when implemented by
    /// this viewer build. Approval and platform alone never imply support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_capabilities: Option<RemoteMediaCapabilities>,
    /// Protocol extensions this viewer understands, such as
    /// [`REMOTE_FEATURE_BOTS`](crate::version::REMOTE_FEATURE_BOTS). The host
    /// sends nothing an extension adds to a viewer that did not declare it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
}

/// The desktop, as described to an approved viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteHostInfo {
    /// Display name of the computer.
    pub name: String,
    /// Operating-system family; the same value as `HostRecord.platform`.
    pub platform: String,
    /// Desktop app version.
    pub app_version: String,
}

/// `initialize`: the first request on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeParams {
    /// `REMOTE_PROTOCOL_MAJOR`; a different major is answered with `upgrade_required`.
    pub protocol: u32,
    /// Major of the viewer's `ClientEvent` decoder.
    pub client_protocol_version: u32,
    /// The viewer app.
    pub app: RemoteAppInfo,
    /// Push registration of this install, so the host can address and exclude it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_installation_id: Option<String>,
}

/// Answer to `initialize`. A device the desktop has not approved gets
/// `pending_approval`, and until it is approved every method except `initialize`
/// and `ping` fails with `not_approved`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InitializeResult {
    /// The device is known and approved.
    Approved {
        /// The desktop.
        host: RemoteHostInfo,
    },
    /// The user must confirm the pairing code on the desktop.
    PendingApproval {
        /// The six-digit code the desktop shows.
        pairing_code: String,
    },
}

/// `auth.approved` (host to viewer): the pending device was approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthApprovedParams {
    /// The desktop.
    pub host: RemoteHostInfo,
}

/// `auth.denied` (host to viewer): the user said no; the host closes the connection after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthDeniedParams {
    /// Why (e.g. `denied`, `revoked`, `disabled`).
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Session list
// ---------------------------------------------------------------------------

/// Where a remote-enabled session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSessionStatus {
    /// A turn is running.
    Running,
    /// Waiting for a decision.
    RequiresAction,
    /// Open and idle.
    Idle,
    /// Not loaded.
    Dormant,
}

/// What a session is waiting on while `requires_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePendingAction {
    /// A permission request.
    Permission,
    /// A question.
    Question,
    /// Computer access (display only).
    ComputerAccess,
}

/// One remote-enabled session in `sessions.list` / `sessions.changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSessionSummary {
    /// Session id.
    pub session_id: String,
    /// Title.
    pub title: String,
    /// Basename of the project directory.
    pub project_label: String,
    /// Status.
    pub status: RemoteSessionStatus,
    /// What it waits on, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_action: Option<RemotePendingAction>,
    /// Active model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Number of viewers attached right now.
    #[serde(default)]
    pub viewers: u32,
    /// The Bot this chat belongs to; set only in `bot.threads.list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

/// `sessions.list` / `sessions.watch` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsListResult {
    /// The sessions.
    pub sessions: Vec<RemoteSessionSummary>,
}

/// `sessions.changed` (host to viewer): always the full list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsChangedParams {
    /// The sessions.
    pub sessions: Vec<RemoteSessionSummary>,
}

// ---------------------------------------------------------------------------
// Session content
// ---------------------------------------------------------------------------

/// A position in a session's record stream: `epoch` identifies the base, `seq`
/// counts records after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCursor {
    /// `runtimeId` plus the sequence of the base event; a new base is a new epoch.
    pub epoch: String,
    /// Sequence of the last record applied.
    pub seq: u64,
}

/// An image the host keeps and the viewer fetches with `image.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteImageRef {
    /// The reference to pass to `image.get`.
    #[serde(rename = "ref")]
    pub image_ref: String,
    /// MIME type.
    pub media_type: String,
}

/// Who started a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePromptOrigin {
    /// Typed on the desktop.
    Desktop,
    /// Sent from a remote device.
    Device,
    /// A scheduled run.
    Scheduled,
    /// A task another Bot handed over. The host sends it only to a viewer that
    /// declared [`REMOTE_FEATURE_BOTS`](crate::version::REMOTE_FEATURE_BOTS);
    /// any other viewer is sent `scheduled`.
    Bot,
}

/// One entry of a session's record stream. Prompts are records of their own
/// because the engine's event stream does not contain the user's prompts.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteRecord {
    /// An engine event, passed through the host's remote filter.
    Event {
        /// The event (or a placeholder when this client cannot decode it).
        event: DecodedEvent,
        /// Large fields were cut.
        truncated: bool,
    },
    /// A user prompt, from the host's prompt journal.
    Prompt {
        /// Turn id.
        turn_id: u64,
        /// Prompt text.
        text: String,
        /// Attached images.
        images: Vec<RemoteImageRef>,
        /// Who sent it.
        origin: RemotePromptOrigin,
        /// Name of the sending device when `origin` is `device`.
        device_name: Option<String>,
        /// Name of the sending Bot when `origin` is `bot`.
        from_bot_name: Option<String>,
    },
    /// A permission request.
    PermissionRequest {
        /// The request.
        request: PermissionRequest,
    },
    /// Shown only; computer access can be granted on the desktop alone.
    ComputerAccessRequest {
        /// The request.
        request: ComputerAccessRequestDto,
    },
    /// An event too large to send (over 1 MiB).
    Omitted {
        /// The omitted event's type.
        event_type: Option<String>,
        /// Its size.
        size_bytes: Option<u64>,
    },
    /// A record type a newer host sends that this client does not know.
    Unsupported {
        /// The record's `type` tag.
        record_type: String,
    },
}

impl<'de> Deserialize<'de> for RemoteRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("record is not an object"))?;
        let record_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("record has no type"))?
            .to_owned();
        let mut take = |key: &str| object.remove(key);
        fn field<T: serde::de::DeserializeOwned, E: Error>(
            value: Option<Value>,
            key: &str,
        ) -> Result<T, E> {
            serde_json::from_value(
                value.ok_or_else(|| E::custom(format!("record: missing {key}")))?,
            )
            .map_err(E::custom)
        }
        fn optional<T: serde::de::DeserializeOwned, E: Error>(
            value: Option<Value>,
        ) -> Result<Option<T>, E> {
            match value {
                None | Some(Value::Null) => Ok(None),
                Some(v) => serde_json::from_value(v).map(Some).map_err(E::custom),
            }
        }
        Ok(match record_type.as_str() {
            "event" => Self::Event {
                event: decode_client_event(
                    take("event").ok_or_else(|| D::Error::custom("record: missing event"))?,
                ),
                truncated: optional::<bool, D::Error>(take("truncated"))?.unwrap_or(false),
            },
            "prompt" => Self::Prompt {
                turn_id: field(take("turn_id"), "turn_id")?,
                text: field(take("text"), "text")?,
                images: optional(take("images"))?.unwrap_or_default(),
                origin: field(take("origin"), "origin")?,
                device_name: optional(take("device_name"))?,
                from_bot_name: optional(take("from_bot_name"))?,
            },
            "permission_request" => Self::PermissionRequest {
                request: field(take("request"), "request")?,
            },
            "computer_access_request" => Self::ComputerAccessRequest {
                request: field(take("request"), "request")?,
            },
            "omitted" => Self::Omitted {
                event_type: optional(take("event_type"))?,
                size_bytes: optional(take("size_bytes"))?,
            },
            _ => Self::Unsupported { record_type },
        })
    }
}

impl Serialize for RemoteRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Event { event, truncated } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "event")?;
                map.serialize_entry("event", event)?;
                if *truncated {
                    map.serialize_entry("truncated", &true)?;
                }
                map.end()
            }
            Self::Prompt {
                turn_id,
                text,
                images,
                origin,
                device_name,
                from_bot_name,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "prompt")?;
                map.serialize_entry("turn_id", turn_id)?;
                map.serialize_entry("text", text)?;
                if !images.is_empty() {
                    map.serialize_entry("images", images)?;
                }
                map.serialize_entry("origin", origin)?;
                if let Some(name) = device_name {
                    map.serialize_entry("device_name", name)?;
                }
                if let Some(name) = from_bot_name {
                    map.serialize_entry("from_bot_name", name)?;
                }
                map.end()
            }
            Self::PermissionRequest { request } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "permission_request")?;
                map.serialize_entry("request", request)?;
                map.end()
            }
            Self::ComputerAccessRequest { request } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "computer_access_request")?;
                map.serialize_entry("request", request)?;
                map.end()
            }
            Self::Omitted {
                event_type,
                size_bytes,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "omitted")?;
                if let Some(t) = event_type {
                    map.serialize_entry("event_type", t)?;
                }
                if let Some(n) = size_bytes {
                    map.serialize_entry("size_bytes", n)?;
                }
                map.end()
            }
            Self::Unsupported { record_type } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("type", record_type)?;
                map.end()
            }
        }
    }
}

/// A record with its position in the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteSeqRecord {
    /// Position.
    pub seq: u64,
    /// The record.
    pub record: RemoteRecord,
}

/// Everything the session is waiting for an answer to; always the complete set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RemotePending {
    /// Permission requests.
    #[serde(default)]
    pub permissions: Vec<PermissionRequest>,
    /// Questions.
    #[serde(default)]
    pub questions: Vec<AskUserQuestionRequestDto>,
    /// Display only.
    #[serde(default)]
    pub computer_access: Vec<ComputerAccessRequestDto>,
}

/// Whether a turn is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemoteTurnState {
    /// A turn is running.
    pub running: bool,
    /// Its id, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

/// What the viewer already has, so the host can answer with a delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCursor {
    /// The epoch the viewer's state belongs to.
    pub epoch: String,
    /// The last sequence the viewer applied.
    pub after_seq: u64,
}

/// `session.attach` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAttachParams {
    /// Session id.
    pub session_id: String,
    /// Resume point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumeCursor>,
}

/// Answer to `session.attach`. `delta`: the viewer's state is still valid; records
/// after `resume.after_seq` follow as `session.event`, and `pending` is the full
/// pending set. `snapshot`: `session.snapshot` notifications follow.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SessionAttachResult {
    /// Resume from the viewer's cursor.
    Delta {
        /// The epoch.
        epoch: String,
        /// The full pending set.
        pending: RemotePending,
    },
    /// A snapshot follows.
    Snapshot {
        /// The epoch.
        epoch: String,
    },
}

impl<'de> Deserialize<'de> for SessionAttachResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        // Keep pending DTOs out of the internally tagged enum's Content buffer:
        // question text uses JSON RawValue, just like snapshot message blocks.
        #[derive(Deserialize)]
        struct Fields {
            mode: String,
            epoch: String,
            #[serde(default)]
            pending: Value,
        }
        let fields = Fields::deserialize(deserializer)?;
        match fields.mode.as_str() {
            "delta" => Ok(Self::Delta {
                epoch: fields.epoch,
                pending: serde_json::from_value(fields.pending).map_err(D::Error::custom)?,
            }),
            "snapshot" => Ok(Self::Snapshot {
                epoch: fields.epoch,
            }),
            mode => Err(D::Error::custom(format!("unknown attach mode: {mode}"))),
        }
    }
}

/// `session.snapshot` (host to viewer): `begin`, then `messages` pages, then
/// `records` pages, then `end`, all in order on the one connection.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum SessionSnapshotParams {
    /// Start of a snapshot.
    Begin {
        /// Session id.
        session_id: String,
        /// The epoch.
        epoch: String,
        /// Session mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_mode: Option<SessionModeDto>,
        /// Index of the first message sent; above 0 when older history must be paged.
        first_message_index: u64,
    },
    /// The base conversation, a page at a time.
    Messages {
        /// Session id.
        session_id: String,
        /// The page.
        messages: Vec<MessageDto>,
    },
    /// Records after the base, a page at a time.
    Records {
        /// Session id.
        session_id: String,
        /// The page.
        records: Vec<RemoteSeqRecord>,
    },
    /// End of a snapshot.
    End {
        /// Session id.
        session_id: String,
        /// Where the stream stands.
        cursor: RemoteCursor,
        /// The full pending set.
        pending: RemotePending,
        /// Turn state.
        turn: RemoteTurnState,
        /// Absent while the session has no controls yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        controls: Option<ConversationControlsDto>,
    },
}

impl<'de> Deserialize<'de> for SessionSnapshotParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        // An internally tagged derive buffers fields through Serde's Content
        // deserializer. Harness text/message DTOs need JSON's RawValue support,
        // which that buffer does not provide. Decode each field from JSON Value
        // directly instead, as we do for RemoteRecord above.
        let mut value = Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("snapshot is not an object"))?;
        fn field<T: serde::de::DeserializeOwned, E: Error>(
            object: &mut serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<T, E> {
            serde_json::from_value(
                object
                    .remove(name)
                    .ok_or_else(|| E::custom(format!("snapshot: missing {name}")))?,
            )
            .map_err(E::custom)
        }
        fn optional<T: serde::de::DeserializeOwned, E: Error>(
            object: &mut serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<Option<T>, E> {
            match object.remove(name) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => serde_json::from_value(value).map(Some).map_err(E::custom),
            }
        }
        let phase: String = field(object, "phase")?;
        let session_id = field(object, "session_id")?;
        Ok(match phase.as_str() {
            "begin" => Self::Begin {
                session_id,
                epoch: field(object, "epoch")?,
                session_mode: optional(object, "session_mode")?,
                first_message_index: field(object, "first_message_index")?,
            },
            "messages" => Self::Messages {
                session_id,
                messages: field(object, "messages")?,
            },
            "records" => Self::Records {
                session_id,
                records: field(object, "records")?,
            },
            "end" => Self::End {
                session_id,
                cursor: field(object, "cursor")?,
                pending: field(object, "pending")?,
                turn: field(object, "turn")?,
                controls: optional(object, "controls")?,
            },
            _ => return Err(D::Error::custom(format!("unknown snapshot phase: {phase}"))),
        })
    }
}

/// `session.event` (host to viewer): the next record of a session the viewer is attached to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEventParams {
    /// Session id.
    pub session_id: String,
    /// Position.
    pub seq: u64,
    /// The record.
    pub record: RemoteRecord,
}

/// `session.pending` (host to viewer): the pending set changed (replaces the previous one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPendingParams {
    /// Session id.
    pub session_id: String,
    /// The full pending set.
    pub pending: RemotePending,
}

/// `session.reset` (host to viewer): the base changed; a fresh snapshot follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResetParams {
    /// Session id.
    pub session_id: String,
}

/// `session.closed` (host to viewer): the session is no longer remote-enabled or was ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClosedParams {
    /// Session id.
    pub session_id: String,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `session.detach` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetachParams {
    /// Session id.
    pub session_id: String,
}

/// `session.presence` (notification, viewer to host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPresenceParams {
    /// Session id.
    pub session_id: String,
    /// Whether this viewer is looking at the session.
    pub foreground: bool,
}

/// `session.history` (request): older messages than the snapshot included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHistoryParams {
    /// Session id.
    pub session_id: String,
    /// Index into the base messages; the page ends just before it.
    pub before_index: u64,
    /// Page size.
    pub limit: u32,
}

/// `session.history` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionHistoryResult {
    /// The messages.
    pub messages: Vec<MessageDto>,
    /// Index of `messages[0]` in the base conversation.
    pub first_index: u64,
    /// Whether anything older remains.
    pub has_more: bool,
}

/// `image.get` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGetParams {
    /// The ref a record carries.
    #[serde(rename = "ref")]
    pub image_ref: String,
}

/// `image.get` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGetResult {
    /// MIME type.
    pub media_type: String,
    /// The bytes, base64.
    pub base64: String,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `prompt.send` (request). While a turn runs the host takes plain text only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSendParams {
    /// Session id.
    pub session_id: String,
    /// Prompt text.
    pub text: String,
    /// At most 4, each already shrunk to 2 MiB or less by the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageRefDto>>,
    /// Chosen by the viewer; makes a retry after a reconnect recognizable.
    pub client_request_id: String,
}

/// `prompt.send` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSendResult {
    /// Assigned by the host (a random 53-bit integer).
    pub turn_id: u64,
}

/// `turn.cancel` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCancelParams {
    /// Session id.
    pub session_id: String,
    /// The turn to cancel, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

/// `permission.respond` (request). First answer wins; a late one fails with `not_pending`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRespondParams {
    /// Session id.
    pub session_id: String,
    /// The request answered.
    pub request_id: u64,
    /// The host refuses `allow_always` unless enabled, and never switches to bypass.
    pub decision: PermissionResponseDto,
}

/// `question.answer` (request): `answers` maps question text to the chosen label(s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswerParams {
    /// Session id.
    pub session_id: String,
    /// The question answered.
    pub request_id: u64,
    /// Question text to chosen label(s).
    pub answers: BTreeMap<String, String>,
}

/// `question.cancel` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionCancelParams {
    /// Session id.
    pub session_id: String,
    /// The question dismissed.
    pub request_id: u64,
}

/// `controls.set_model` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlsSetModelParams {
    /// Session id.
    pub session_id: String,
    /// The model.
    pub model: String,
}

/// `controls.set_reasoning` (request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlsSetReasoningParams {
    /// Session id.
    pub session_id: String,
    /// The selection.
    pub selection: ReasoningSelectionDto,
}

/// Live session permission modes accepted by the engine (their wire ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionModeId {
    /// Ask for everything not allowed.
    #[serde(rename = "default")]
    Default,
    /// Accept edits automatically.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Plan only.
    #[serde(rename = "plan")]
    Plan,
    /// Engine-decided automatic mode.
    #[serde(rename = "auto")]
    Auto,
    /// Never ask; deny what is not allowed.
    #[serde(rename = "dontAsk")]
    DontAsk,
    /// Allow everything. The host refuses this from a remote device.
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
}

/// `controls.set_permission_mode` (request); `bypassPermissions` is refused with `forbidden`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlsSetPermissionModeParams {
    /// Session id.
    pub session_id: String,
    /// The mode.
    pub mode: PermissionModeId,
}

/// `models.list` (request).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelsListParams {
    /// The session whose available models are wanted, if they differ per session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// `models.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsListResult {
    /// Model ids.
    pub models: Vec<String>,
    /// The current model.
    pub current: String,
    /// Details, when the host sends them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ModelDetailsDto>>,
}

// ---------------------------------------------------------------------------
// Bots
// ---------------------------------------------------------------------------
//
// Only a viewer that declared `REMOTE_FEATURE_BOTS` in `initialize.app.features`
// may call these; any other gets the unknown-method error. A Bot's chats are
// ordinary sessions, reached through `bot.threads.list` and then used with the
// `session.*` and command methods unchanged. They never appear in
// `sessions.list`.

/// Where a Bot stands, derived from its chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteBotStatus {
    /// No chat is running or waiting.
    Idle,
    /// A chat is running a turn.
    Running,
    /// A chat is waiting for an answer.
    RequiresAction,
}

/// One Bot whose Remote Control switch is on, in `bots.list` / `bots.changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteBotSummary {
    /// Bot id.
    pub bot_id: String,
    /// Display name.
    pub name: String,
    /// What the Bot is for.
    pub description: String,
    /// Status.
    pub status: RemoteBotStatus,
    /// Chats waiting for an answer right now. Missing, or anything but a count, reads as none.
    #[serde(default, deserialize_with = "count_or_zero")]
    pub waiting_threads: u32,
    /// Epoch milliseconds of the Bot's latest chat activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<u64>,
}

/// A count, and zero for anything else: the Bot stays in the list with a bad number.
fn count_or_zero<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(0))
}

/// `bots.list` / `bots.watch` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotsListResult {
    /// The Bots.
    pub bots: Vec<RemoteBotSummary>,
}

/// `bots.changed` (host to viewer): always the full list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotsChangedParams {
    /// The Bots.
    pub bots: Vec<RemoteBotSummary>,
}

/// `bot.threads.list` (request): the Bot's chats, newest first, at most 50.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotThreadsListParams {
    /// Bot id.
    pub bot_id: String,
}

/// `bot.threads.list` result. `project_label` is the Bot's name and `bot_id` is
/// set on every entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotThreadsListResult {
    /// The chats.
    pub sessions: Vec<RemoteSessionSummary>,
}

/// `bot.thread.create` (request): start a new chat of the Bot. A retry with the
/// same `client_request_id` returns the same chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotThreadCreateParams {
    /// Bot id.
    pub bot_id: String,
    /// Chosen by the viewer; makes a retry after a reconnect recognizable.
    pub client_request_id: String,
}

/// `bot.thread.create` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotThreadCreateResult {
    /// The new chat's session id.
    pub session_id: String,
}
