//! Sessions as a viewer sees them: the list of shared sessions and, per session,
//! the attach / snapshot / live-record state machine of
//! `apps/web/src/remote/remoteClient.ts`.
//!
//! What is left to the UI layer is turning `messages` plus `records` into rows
//! (the TypeScript `TranscriptBuilder`); this module only keeps them complete and
//! in order: it attaches (resuming from the last `(epoch, seq)` when it can),
//! assembles the snapshot the desktop sends in pieces, holds live records back
//! while that is going on, applies them in order, and offers the commands.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use client::protocol::commands::ImageRefDto;
use client::protocol::controls::{ConversationControlsDto, ReasoningSelectionDto};
use client::protocol::events::ClientEvent;
use client::protocol::message::MessageDto;
use client::protocol::permission::PermissionResponseDto;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::connection::{ConnectionListener, ConnectionPhase, ConnectionState, ViewerConnection};
use crate::encoding::random_id;
use crate::messages::{
    methods, ImageGetResult, ModelsListResult, PermissionModeId, PromptSendResult, RemoteCursor,
    RemotePending, RemoteRecord, RemoteSeqRecord, RemoteSessionSummary, RemoteTurnState,
    SessionAttachResult, SessionHistoryResult, SessionSnapshotParams, SessionsChangedParams,
    SessionsListResult,
};
use crate::rpc::{RemoteErrorCode, RpcError};
use crate::text::display_text;

/// Longest wait for `session.attach` to answer.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(60);
/// After `session.reset` the desktop sends a new snapshot by itself; if none comes, ask for one.
const RESET_SNAPSHOT_WAIT: Duration = Duration::from_secs(3);
/// Messages asked for per `session.history` page.
const HISTORY_PAGE: u32 = 100;
/// Live records kept while a snapshot is still being assembled.
const MAX_BUFFERED_EVENTS: usize = 5000;
/// Images a prompt may carry.
pub const MAX_IMAGE_ATTACHMENTS: usize = 4;
/// Media types the desktop accepts for prompt images.
pub const SUPPORTED_IMAGE_MEDIA_TYPES: [&str; 4] =
    ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Whether a prompt image of this media type is accepted.
pub fn is_supported_image_media_type(media_type: &str) -> bool {
    SUPPORTED_IMAGE_MEDIA_TYPES.contains(&media_type)
}

/// What a session controller needs from the connection; [`ViewerConnection`] implements it.
#[async_trait]
pub trait SessionIo: Send + Sync {
    /// Send a request on the ready link.
    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError>;
    /// Send a notification (dropped when not connected).
    async fn notify(&self, method: &str, params: Option<Value>);
    /// Whether the link is ready.
    fn connected(&self) -> bool;
}

#[async_trait]
impl SessionIo for ViewerConnection {
    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        ViewerConnection::request(self, method, params, timeout).await
    }

    async fn notify(&self, method: &str, params: Option<Value>) {
        ViewerConnection::notify(self, method, params).await;
    }

    fn connected(&self) -> bool {
        self.state().phase == ConnectionPhase::Ready
    }
}

/// Where a session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Attaching, or waiting for the snapshot.
    Loading,
    /// Up to date and following.
    Live,
    /// The connection is down; what is held is what was last received.
    Offline,
    /// The desktop refused the attach; the error says why.
    Error,
    /// The session is no longer shared.
    Closed,
}

/// An error code and a message safe to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError {
    /// The protocol's error code.
    pub code: String,
    /// A message cut for display.
    pub message: String,
}

fn session_error(error: &RpcError) -> SessionError {
    SessionError {
        code: error.code.as_str().to_owned(),
        message: display_text(&error.message, 200),
    }
}

/// Whether a prompt sent from this device is still on its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingStatus {
    /// Waiting for the desktop to accept it.
    Sending,
    /// The desktop did not take it; it can be retried or dismissed.
    Failed,
}

/// A prompt this device sent that its own record has not come back for yet.
#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingPrompt {
    /// Also the `client_request_id`, so a retry is recognised as a repeat.
    pub id: String,
    /// The text.
    pub text: String,
    /// Attached images.
    pub images: Vec<ImageRefDto>,
    /// State.
    pub status: OutgoingStatus,
    /// The turn the desktop assigned.
    pub turn_id: Option<u64>,
    /// Error code of a failure.
    pub error: Option<String>,
}

/// One session as the viewer holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionState {
    /// The session.
    pub session_id: String,
    /// Where it stands.
    pub status: SessionStatus,
    /// Why the attach failed.
    pub error: Option<SessionError>,
    /// The base conversation (from `first_message_index` on).
    pub messages: Vec<MessageDto>,
    /// Index of the first held message.
    pub first_message_index: u64,
    /// Records after the base, ordered by `seq`.
    pub records: Vec<RemoteSeqRecord>,
    /// What the session waits for an answer to.
    pub pending: RemotePending,
    /// Whether a turn runs.
    pub turn: RemoteTurnState,
    /// Model, reasoning and permission controls.
    pub controls: Option<ConversationControlsDto>,
    /// Prompts on their way.
    pub outgoing: Vec<OutgoingPrompt>,
    /// Whether older messages can be fetched.
    pub has_older: bool,
    /// Whether they are being fetched.
    pub loading_older: bool,
    /// An answer lost the race to another device (cleared by the next change).
    pub answered_elsewhere: bool,
}

impl SessionState {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            status: SessionStatus::Loading,
            error: None,
            messages: Vec::new(),
            first_message_index: 0,
            records: Vec::new(),
            pending: RemotePending::default(),
            turn: RemoteTurnState::default(),
            controls: None,
            outgoing: Vec::new(),
            has_older: false,
            loading_older: false,
            answered_elsewhere: false,
        }
    }
}

/// Hears about changes of a session, in order, from one task. Must not block.
pub trait SessionListener: Send + Sync {
    /// The session's state changed.
    fn on_session_state(&self, state: &SessionState);
}

#[derive(Clone)]
struct Cursor(RemoteCursor);

struct Assembly {
    epoch: String,
    first: u64,
    messages: Vec<MessageDto>,
    records: Vec<RemoteSeqRecord>,
}

struct Core {
    state: SessionState,
    active: bool,
    attach_id: u64,
    cursor: Option<Cursor>,
    assembly: Option<Assembly>,
    held: Vec<RemoteSeqRecord>,
    awaiting_attach: bool,
    foreground: bool,
    seen_turns: Vec<u64>,
    reset_timer: Option<JoinHandle<()>>,
}

struct SessionInner {
    session_id: String,
    io: Arc<dyn SessionIo>,
    core: Mutex<Core>,
    listeners: Mutex<Vec<(u64, Arc<dyn SessionListener>)>>,
    next_listener: AtomicU64,
    delivery: mpsc::UnboundedSender<SessionState>,
    images: Mutex<HashMap<String, String>>,
}

/// One session. Cheap to clone.
#[derive(Clone)]
pub struct SessionController {
    inner: Arc<SessionInner>,
}

impl std::fmt::Debug for SessionController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionController")
            .field("session_id", &self.inner.session_id)
            .finish_non_exhaustive()
    }
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    serde_json::from_value(value)
        .map_err(|e| RpcError::new(RemoteErrorCode::Internal, format!("unexpected result: {e}")))
}

impl SessionController {
    /// A controller for `session_id`. Must be created inside a Tokio runtime.
    pub fn new(session_id: impl Into<String>, io: Arc<dyn SessionIo>) -> Self {
        let session_id = session_id.into();
        let (delivery, mut receiver) = mpsc::unbounded_channel::<SessionState>();
        let inner = Arc::new(SessionInner {
            core: Mutex::new(Core {
                state: SessionState::new(&session_id),
                active: false,
                attach_id: 0,
                cursor: None,
                assembly: None,
                held: Vec::new(),
                awaiting_attach: false,
                foreground: true,
                seen_turns: Vec::new(),
                reset_timer: None,
            }),
            session_id,
            io,
            listeners: Mutex::new(Vec::new()),
            next_listener: AtomicU64::new(1),
            delivery,
            images: Mutex::new(HashMap::new()),
        });
        let weak: Weak<SessionInner> = Arc::downgrade(&inner);
        tokio::spawn(async move {
            while let Some(state) = receiver.recv().await {
                let Some(inner) = weak.upgrade() else { return };
                let listeners: Vec<Arc<dyn SessionListener>> = inner
                    .listeners
                    .lock()
                    .expect("listeners")
                    .iter()
                    .map(|(_, l)| Arc::clone(l))
                    .collect();
                for listener in listeners {
                    listener.on_session_state(&state);
                }
            }
        });
        Self { inner }
    }

    /// The session id.
    pub fn session_id(&self) -> &str {
        &self.inner.session_id
    }

    /// The current state.
    pub fn state(&self) -> SessionState {
        self.inner.core.lock().expect("core").state.clone()
    }

    /// Register a listener; returns an id for [`remove_listener`](Self::remove_listener).
    pub fn add_listener(&self, listener: Arc<dyn SessionListener>) -> u64 {
        let id = self.inner.next_listener.fetch_add(1, Ordering::SeqCst);
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .push((id, listener));
        id
    }

    /// Unregister a listener.
    pub fn remove_listener(&self, id: u64) {
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .retain(|(i, _)| *i != id);
    }

    /// The screen is showing this session: attach now if connected, and again after every reconnect.
    pub fn activate(&self) {
        self.inner.core.lock().expect("core").active = true;
        if self.inner.io.connected() {
            self.spawn_attach();
        }
    }

    /// The screen left the session.
    pub fn deactivate(&self) {
        {
            let mut core = self.inner.core.lock().expect("core");
            if !core.active {
                return;
            }
            core.active = false;
            core.attach_id += 1;
            if let Some(timer) = core.reset_timer.take() {
                timer.abort();
            }
        }
        if self.inner.io.connected() {
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let _ = inner
                    .io
                    .request(
                        methods::SESSION_DETACH,
                        Some(json!({ "session_id": inner.session_id })),
                        None,
                    )
                    .await;
            });
        }
    }

    /// The connection became ready again.
    pub fn connection_ready(&self) {
        if self.inner.core.lock().expect("core").active {
            self.spawn_attach();
        }
    }

    /// The connection is down.
    pub fn connection_lost(&self) {
        let mut core = self.inner.core.lock().expect("core");
        core.attach_id += 1;
        core.assembly = None;
        core.awaiting_attach = false;
        if matches!(
            core.state.status,
            SessionStatus::Loading | SessionStatus::Live
        ) {
            self.inner
                .commit(&mut core, |s| s.status = SessionStatus::Offline);
        }
    }

    /// Tell the desktop whether the screen is in the foreground (it uses this for notifications).
    pub fn set_foreground(&self, foreground: bool) {
        let live = {
            let mut core = self.inner.core.lock().expect("core");
            core.foreground = foreground;
            core.state.status == SessionStatus::Live
        };
        if live {
            self.inner.send_presence();
        }
    }

    fn spawn_attach(&self) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.attach().await });
    }

    // -----------------------------------------------------------------------
    // From the desktop (called by `RemoteClient`, public for custom routing)
    // -----------------------------------------------------------------------

    /// A `session.snapshot` notification.
    pub fn handle_snapshot(&self, params: SessionSnapshotParams) {
        let mut core = self.inner.core.lock().expect("core");
        match params {
            SessionSnapshotParams::Begin {
                epoch,
                first_message_index,
                ..
            } => {
                core.assembly = Some(Assembly {
                    epoch,
                    first: first_message_index,
                    messages: Vec::new(),
                    records: Vec::new(),
                });
                if let Some(timer) = core.reset_timer.take() {
                    timer.abort();
                }
                self.inner.commit(&mut core, |s| {
                    s.status = SessionStatus::Loading;
                    s.error = None;
                });
            }
            SessionSnapshotParams::Messages { messages, .. } => {
                if let Some(assembly) = &mut core.assembly {
                    assembly.messages.extend(messages);
                }
            }
            SessionSnapshotParams::Records { records, .. } => {
                if let Some(assembly) = &mut core.assembly {
                    assembly.records.extend(records);
                }
            }
            SessionSnapshotParams::End {
                cursor,
                pending,
                turn,
                controls,
                ..
            } => {
                let Some(assembly) = core.assembly.take() else {
                    return;
                };
                let mut records = assembly.records;
                records.sort_by_key(|r| r.seq);
                core.seen_turns = records
                    .iter()
                    .filter_map(|r| match &r.record {
                        RemoteRecord::Prompt { turn_id, .. } => Some(*turn_id),
                        _ => None,
                    })
                    .collect();
                let _ = assembly.epoch;
                core.cursor = Some(Cursor(cursor));
                let seen = core.seen_turns.clone();
                let controls = controls.or_else(|| controls_from_records(&records));
                let first = assembly.first;
                let messages = assembly.messages;
                self.inner.commit(&mut core, move |s| {
                    s.status = SessionStatus::Live;
                    s.error = None;
                    s.messages = messages;
                    s.first_message_index = first;
                    s.records = records;
                    s.pending = pending;
                    s.turn = turn;
                    s.controls = controls;
                    s.has_older = first > 0;
                    s.loading_older = false;
                    s.outgoing
                        .retain(|p| p.turn_id.is_none_or(|t| !seen.contains(&t)));
                });
                self.inner.flush_held(&mut core);
                let awaiting = core.awaiting_attach;
                drop(core);
                if !awaiting {
                    self.inner.send_presence();
                }
            }
        }
    }

    /// A `session.event` notification.
    pub fn handle_event(&self, item: RemoteSeqRecord) {
        let mut core = self.inner.core.lock().expect("core");
        if core.assembly.is_some()
            || core.awaiting_attach
            || core.state.status == SessionStatus::Loading
        {
            if core.held.len() < MAX_BUFFERED_EVENTS {
                core.held.push(item);
            }
            return;
        }
        self.inner.apply_live(&mut core, item);
    }

    /// A `session.pending` notification.
    pub fn handle_pending(&self, pending: RemotePending) {
        let mut core = self.inner.core.lock().expect("core");
        self.inner.commit(&mut core, |s| {
            s.pending = pending;
            s.answered_elsewhere = false;
        });
    }

    /// A `session.reset` notification: the base changed; a new snapshot follows.
    pub fn handle_reset(&self) {
        let mut core = self.inner.core.lock().expect("core");
        core.cursor = None;
        core.held.clear();
        core.assembly = None;
        core.seen_turns.clear();
        if let Some(timer) = core.reset_timer.take() {
            timer.abort();
        }
        self.inner.commit(&mut core, |s| {
            s.status = SessionStatus::Loading;
            s.messages.clear();
            s.records.clear();
            s.first_message_index = 0;
            s.pending = RemotePending::default();
            s.turn = RemoteTurnState::default();
            s.has_older = false;
        });
        let inner = Arc::clone(&self.inner);
        core.reset_timer = Some(tokio::spawn(async move {
            tokio::time::sleep(RESET_SNAPSHOT_WAIT).await;
            let ask = {
                let core = inner.core.lock().expect("core");
                core.active
                    && core.assembly.is_none()
                    && core.state.status == SessionStatus::Loading
            };
            if ask {
                inner.attach().await;
            }
        }));
    }

    /// A `session.closed` notification.
    pub fn handle_closed(&self) {
        let mut core = self.inner.core.lock().expect("core");
        core.attach_id += 1;
        self.inner
            .commit(&mut core, |s| s.status = SessionStatus::Closed);
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    /// Send a prompt. While a turn runs the desktop takes plain text only, so images
    /// are left out then; unsupported types and more than four images are dropped.
    pub async fn send_prompt(&self, text: &str, images: Vec<ImageRefDto>) {
        let running = self.state().turn.running;
        let attachments: Vec<ImageRefDto> = if running {
            Vec::new()
        } else {
            images
                .into_iter()
                .filter(|i| is_supported_image_media_type(&i.media_type))
                .take(MAX_IMAGE_ATTACHMENTS)
                .collect()
        };
        if text.trim().is_empty() && attachments.is_empty() {
            return;
        }
        let prompt = OutgoingPrompt {
            id: random_id(),
            text: text.to_owned(),
            images: attachments,
            status: OutgoingStatus::Sending,
            turn_id: None,
            error: None,
        };
        {
            let mut core = self.inner.core.lock().expect("core");
            let prompt = prompt.clone();
            self.inner.commit(&mut core, |s| s.outgoing.push(prompt));
        }
        self.inner.dispatch_prompt(&prompt).await;
    }

    /// Send a failed prompt again with the same request id, so the desktop can tell it is a repeat.
    pub async fn retry_prompt(&self, id: &str) {
        let prompt = {
            let mut core = self.inner.core.lock().expect("core");
            let Some(prompt) = core
                .state
                .outgoing
                .iter()
                .find(|p| p.id == id && p.status == OutgoingStatus::Failed)
                .cloned()
            else {
                return;
            };
            self.inner.update_outgoing(&mut core, id, |p| {
                p.status = OutgoingStatus::Sending;
                p.error = None;
            });
            prompt
        };
        self.inner.dispatch_prompt(&prompt).await;
    }

    /// Remove a prompt from the outgoing list.
    pub fn dismiss_prompt(&self, id: &str) {
        let mut core = self.inner.core.lock().expect("core");
        self.inner
            .commit(&mut core, |s| s.outgoing.retain(|p| p.id != id));
    }

    /// Cancel the running turn.
    pub async fn cancel_turn(&self) -> Result<(), RpcError> {
        let turn_id = self.state().turn.turn_id;
        let mut params = json!({ "session_id": self.inner.session_id });
        if let Some(turn_id) = turn_id {
            params["turn_id"] = json!(turn_id);
        }
        self.inner
            .io
            .request(methods::TURN_CANCEL, Some(params), None)
            .await
            .map(drop)
    }

    /// Allow once or deny. Always-allow is not offered from a remote device.
    pub async fn respond_permission(&self, request_id: u64, allow: bool) -> Result<(), RpcError> {
        let decision = if allow {
            PermissionResponseDto::AllowOnce
        } else {
            PermissionResponseDto::Deny
        };
        self.inner
            .answer(
                methods::PERMISSION_RESPOND,
                json!({
                    "session_id": self.inner.session_id,
                    "request_id": request_id,
                    "decision": decision,
                }),
            )
            .await
    }

    /// Answer a question: question text to the chosen label(s) (several joined by ", ").
    pub async fn answer_question(
        &self,
        request_id: u64,
        answers: BTreeMap<String, String>,
    ) -> Result<(), RpcError> {
        self.inner
            .answer(
                methods::QUESTION_ANSWER,
                json!({
                    "session_id": self.inner.session_id,
                    "request_id": request_id,
                    "answers": answers,
                }),
            )
            .await
    }

    /// Dismiss a question.
    pub async fn cancel_question(&self, request_id: u64) -> Result<(), RpcError> {
        self.inner
            .answer(
                methods::QUESTION_CANCEL,
                json!({ "session_id": self.inner.session_id, "request_id": request_id }),
            )
            .await
    }

    /// Change the model.
    pub async fn set_model(&self, model: &str) -> Result<(), RpcError> {
        self.command(
            methods::CONTROLS_SET_MODEL,
            json!({ "session_id": self.inner.session_id, "model": model }),
        )
        .await
    }

    /// Change reasoning.
    pub async fn set_reasoning(&self, selection: &ReasoningSelectionDto) -> Result<(), RpcError> {
        self.command(
            methods::CONTROLS_SET_REASONING,
            json!({ "session_id": self.inner.session_id, "selection": selection }),
        )
        .await
    }

    /// Change the permission mode. `bypassPermissions` is never sent.
    pub async fn set_permission_mode(&self, mode: PermissionModeId) -> Result<(), RpcError> {
        if mode == PermissionModeId::BypassPermissions {
            return Err(RpcError::new(
                RemoteErrorCode::Forbidden,
                "bypassPermissions cannot be set from a remote device",
            ));
        }
        self.command(
            methods::CONTROLS_SET_PERMISSION_MODE,
            json!({ "session_id": self.inner.session_id, "mode": mode }),
        )
        .await
    }

    async fn command(&self, method: &str, params: Value) -> Result<(), RpcError> {
        self.inner
            .io
            .request(method, Some(params), None)
            .await
            .map(drop)
    }

    /// Fetch the page of messages before the ones held.
    pub async fn load_older(&self) {
        let first = {
            let mut core = self.inner.core.lock().expect("core");
            if !core.state.has_older || core.state.loading_older {
                return;
            }
            let first = core.state.first_message_index;
            self.inner.commit(&mut core, |s| s.loading_older = true);
            first
        };
        let result = self
            .inner
            .io
            .request(
                methods::SESSION_HISTORY,
                Some(json!({
                    "session_id": self.inner.session_id,
                    "before_index": first,
                    "limit": HISTORY_PAGE,
                })),
                None,
            )
            .await
            .and_then(parse::<SessionHistoryResult>);
        let mut core = self.inner.core.lock().expect("core");
        match result {
            Ok(page) => self.inner.commit(&mut core, |s| {
                let mut messages = page.messages;
                messages.append(&mut s.messages);
                s.messages = messages;
                s.first_message_index = page.first_index;
                s.has_older = page.has_more && page.first_index > 0;
                s.loading_older = false;
            }),
            Err(_) => self.inner.commit(&mut core, |s| s.loading_older = false),
        }
    }

    /// The bytes of an image the desktop keeps, as a `data:` URL; fetched once.
    pub async fn fetch_image(&self, image_ref: &str) -> Result<String, RpcError> {
        if let Some(url) = self.inner.images.lock().expect("images").get(image_ref) {
            return Ok(url.clone());
        }
        let result: ImageGetResult = parse(
            self.inner
                .io
                .request(methods::IMAGE_GET, Some(json!({ "ref": image_ref })), None)
                .await?,
        )?;
        let valid_base64 = !result.base64.is_empty()
            && result
                .base64
                .trim_end_matches('=')
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
            && result.base64.len() - result.base64.trim_end_matches('=').len() <= 2;
        if !is_supported_image_media_type(&result.media_type) || !valid_base64 {
            return Err(RpcError::new(
                RemoteErrorCode::Internal,
                "unsupported image",
            ));
        }
        let url = format!("data:{};base64,{}", result.media_type, result.base64);
        self.inner
            .images
            .lock()
            .expect("images")
            .insert(image_ref.to_owned(), url.clone());
        Ok(url)
    }
}

fn controls_from_records(records: &[RemoteSeqRecord]) -> Option<ConversationControlsDto> {
    let mut controls = None;
    for item in records {
        if let RemoteRecord::Event { event, .. } = &item.record {
            if let Some(ClientEvent::ConversationControlsChanged { controls: c }) = event.known() {
                controls = Some(c.clone());
            }
        }
    }
    controls
}

impl SessionInner {
    /// Change the state and queue it for the listeners. The caller holds the core lock,
    /// so listener order is the order of changes.
    fn commit(&self, core: &mut Core, change: impl FnOnce(&mut SessionState)) {
        change(&mut core.state);
        let _ = self.delivery.send(core.state.clone());
    }

    fn update_outgoing(&self, core: &mut Core, id: &str, change: impl FnOnce(&mut OutgoingPrompt)) {
        self.commit(core, |s| {
            if let Some(prompt) = s.outgoing.iter_mut().find(|p| p.id == id) {
                change(prompt);
            }
        });
    }

    fn send_presence(self: &Arc<Self>) {
        let foreground = self.core.lock().expect("core").foreground;
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            inner
                .io
                .notify(
                    methods::SESSION_PRESENCE,
                    Some(json!({ "session_id": inner.session_id, "foreground": foreground })),
                )
                .await;
        });
    }

    async fn attach(self: &Arc<Self>) {
        while self.attach_once().await {}
    }

    /// One attach; `true` asks for another one without resuming.
    async fn attach_once(self: &Arc<Self>) -> bool {
        let (id, resume) = {
            let mut core = self.core.lock().expect("core");
            core.attach_id += 1;
            if let Some(timer) = core.reset_timer.take() {
                timer.abort();
            }
            core.assembly = None;
            core.held.clear();
            core.awaiting_attach = true;
            self.commit(&mut core, |s| {
                s.status = SessionStatus::Loading;
                s.error = None;
            });
            let resume = core
                .cursor
                .as_ref()
                .map(|c| json!({ "epoch": c.0.epoch, "after_seq": c.0.seq }));
            (core.attach_id, resume)
        };
        let mut params = json!({ "session_id": self.session_id });
        if let Some(resume) = resume {
            params["resume"] = resume;
        }
        let result = self
            .io
            .request(methods::SESSION_ATTACH, Some(params), Some(ATTACH_TIMEOUT))
            .await
            .and_then(parse::<SessionAttachResult>);
        let mut core = self.core.lock().expect("core");
        if id != core.attach_id {
            return false;
        }
        core.awaiting_attach = false;
        match result {
            Ok(SessionAttachResult::Delta { epoch, pending }) => {
                if core.cursor.as_ref().is_none_or(|c| c.0.epoch != epoch) {
                    // A delta for a base we do not have cannot be applied: start over without resuming.
                    core.cursor = None;
                    return true;
                }
                self.commit(&mut core, |s| {
                    s.status = SessionStatus::Live;
                    s.pending = pending;
                });
                self.flush_held(&mut core);
                drop(core);
                self.send_presence();
            }
            Ok(SessionAttachResult::Snapshot { epoch }) => {
                let complete = core.state.status == SessionStatus::Live
                    && core.cursor.as_ref().is_some_and(|c| c.0.epoch == epoch);
                drop(core);
                if complete {
                    // The snapshot was complete before the reply was handled.
                    self.send_presence();
                }
            }
            Err(error) => {
                if error.code == RemoteErrorCode::Closed {
                    self.commit(&mut core, |s| s.status = SessionStatus::Offline);
                } else {
                    let failure = session_error(&error);
                    self.commit(&mut core, |s| {
                        s.status = SessionStatus::Error;
                        s.error = Some(failure);
                    });
                }
            }
        }
        false
    }

    fn flush_held(&self, core: &mut Core) {
        let mut held = std::mem::take(&mut core.held);
        held.sort_by_key(|r| r.seq);
        for item in held {
            self.apply_live(core, item);
        }
    }

    fn apply_live(&self, core: &mut Core, item: RemoteSeqRecord) {
        let Some(cursor) = &core.cursor else { return };
        if item.seq <= cursor.0.seq {
            return; // already covered
        }
        let epoch = cursor.0.epoch.clone();
        core.cursor = Some(Cursor(RemoteCursor {
            epoch,
            seq: item.seq,
        }));
        if let RemoteRecord::Prompt { turn_id, .. } = &item.record {
            core.seen_turns.push(*turn_id);
        }
        let record = item.record.clone();
        self.commit(core, move |s| {
            s.records.push(item);
            match &record {
                RemoteRecord::Prompt { turn_id, .. } => {
                    s.outgoing.retain(|p| p.turn_id != Some(*turn_id));
                }
                RemoteRecord::Event { event, .. } => match event.known() {
                    Some(ClientEvent::TurnStarted { turn_id }) => {
                        s.turn = RemoteTurnState {
                            running: true,
                            turn_id: *turn_id,
                        };
                    }
                    Some(ClientEvent::TurnEnded { .. }) => s.turn = RemoteTurnState::default(),
                    Some(ClientEvent::ConversationControlsChanged { controls }) => {
                        s.controls = Some(controls.clone());
                    }
                    Some(ClientEvent::ModelChanged { model, .. }) => {
                        if let Some(controls) = &mut s.controls {
                            controls.qualified_model = model.clone();
                        }
                    }
                    Some(ClientEvent::PermissionModeChanged { mode }) => {
                        if let Some(controls) = &mut s.controls {
                            controls.permission.requested = mode.clone();
                            controls.permission.effective = mode.clone();
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        });
    }

    async fn dispatch_prompt(&self, prompt: &OutgoingPrompt) {
        let mut params = json!({
            "session_id": self.session_id,
            "text": prompt.text,
            "client_request_id": prompt.id,
        });
        if !prompt.images.is_empty() {
            params["images"] = json!(prompt.images);
        }
        let result = self
            .io
            .request(methods::PROMPT_SEND, Some(params), None)
            .await
            .and_then(parse::<PromptSendResult>);
        let mut core = self.core.lock().expect("core");
        match result {
            Ok(sent) => {
                if core.seen_turns.contains(&sent.turn_id) {
                    self.commit(&mut core, |s| s.outgoing.retain(|p| p.id != prompt.id));
                } else {
                    self.update_outgoing(&mut core, &prompt.id, |p| p.turn_id = Some(sent.turn_id));
                }
            }
            Err(error) => {
                let code = error.code.as_str().to_owned();
                self.update_outgoing(&mut core, &prompt.id, |p| {
                    p.status = OutgoingStatus::Failed;
                    p.error = Some(code);
                });
            }
        }
    }

    /// First answer wins: losing the race to another device is not an error, only a quiet note.
    async fn answer(&self, method: &str, params: Value) -> Result<(), RpcError> {
        match self.io.request(method, Some(params), None).await {
            Ok(_) => Ok(()),
            Err(error) if error.code == RemoteErrorCode::NotPending => {
                let mut core = self.core.lock().expect("core");
                self.commit(&mut core, |s| s.answered_elsewhere = true);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

// ---------------------------------------------------------------------------
// RemoteClient
// ---------------------------------------------------------------------------

/// Hears about the list of shared sessions. Must not block.
pub trait RemoteClientListener: Send + Sync {
    /// The list changed (`None` until the first list arrives).
    fn on_sessions(&self, sessions: &[RemoteSessionSummary]);
}

fn clean_summary(mut summary: RemoteSessionSummary) -> RemoteSessionSummary {
    summary.title = display_text(&summary.title, 120);
    summary.project_label = display_text(&summary.project_label, 60);
    summary.model = summary.model.map(|m| display_text(&m, 80));
    summary
}

struct ClientInner {
    io: Arc<dyn SessionIo>,
    sessions: Mutex<Option<Vec<RemoteSessionSummary>>>,
    controllers: Mutex<HashMap<String, SessionController>>,
    listeners: Mutex<Vec<(u64, Arc<dyn RemoteClientListener>)>>,
    next_listener: AtomicU64,
    models: tokio::sync::Mutex<Option<ModelsListResult>>,
}

/// Everything above one connection that the screens share: the list of shared
/// sessions, the per-session controllers (kept so a session's `(epoch, seq)`
/// survives going back to the list and a reconnect), and the routing of host
/// notifications to them. Cheap to clone.
#[derive(Clone)]
pub struct RemoteClient {
    inner: Arc<ClientInner>,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteClient").finish_non_exhaustive()
    }
}

struct Router(Weak<ClientInner>);

impl ConnectionListener for Router {
    fn on_state(&self, state: &ConnectionState) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        if state.phase != ConnectionPhase::Ready {
            let controllers: Vec<SessionController> = inner
                .controllers
                .lock()
                .expect("controllers")
                .values()
                .cloned()
                .collect();
            for controller in controllers {
                controller.connection_lost();
            }
        }
    }

    fn on_ready(&self) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        RemoteClient { inner }.ready();
    }

    fn on_notification(&self, method: &str, params: &Value) {
        let Some(inner) = self.0.upgrade() else {
            return;
        };
        RemoteClient { inner }.route(method, params);
    }
}

impl RemoteClient {
    /// A client over any [`SessionIo`].
    pub fn new(io: Arc<dyn SessionIo>) -> Self {
        Self {
            inner: Arc::new(ClientInner {
                io,
                sessions: Mutex::new(None),
                controllers: Mutex::new(HashMap::new()),
                listeners: Mutex::new(Vec::new()),
                next_listener: AtomicU64::new(1),
                models: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// A client over a connection, registered as one of its listeners.
    pub fn for_connection(connection: &ViewerConnection) -> Self {
        let client = Self::new(Arc::new(connection.clone()));
        connection.add_listener(Arc::new(Router(Arc::downgrade(&client.inner))));
        client
    }

    /// The listener to register with the connection when it was not built with
    /// [`for_connection`](Self::for_connection).
    pub fn connection_listener(&self) -> Arc<dyn ConnectionListener> {
        Arc::new(Router(Arc::downgrade(&self.inner)))
    }

    /// The shared sessions; `None` until the first list arrives.
    pub fn sessions(&self) -> Option<Vec<RemoteSessionSummary>> {
        self.inner.sessions.lock().expect("sessions").clone()
    }

    /// Register a listener of the list.
    pub fn add_listener(&self, listener: Arc<dyn RemoteClientListener>) -> u64 {
        let id = self.inner.next_listener.fetch_add(1, Ordering::SeqCst);
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .push((id, listener));
        id
    }

    /// Unregister a listener.
    pub fn remove_listener(&self, id: u64) {
        self.inner
            .listeners
            .lock()
            .expect("listeners")
            .retain(|(i, _)| *i != id);
    }

    /// The controller for a session, created on first use and kept.
    pub fn session(&self, session_id: &str) -> SessionController {
        self.inner
            .controllers
            .lock()
            .expect("controllers")
            .entry(session_id.to_owned())
            .or_insert_with(|| SessionController::new(session_id, Arc::clone(&self.inner.io)))
            .clone()
    }

    /// The models the desktop offers; asked once per connection.
    pub async fn models(&self) -> Result<ModelsListResult, RpcError> {
        let mut cache = self.inner.models.lock().await;
        if let Some(models) = cache.as_ref() {
            return Ok(models.clone());
        }
        let models: ModelsListResult = parse(
            self.inner
                .io
                .request(methods::MODELS_LIST, Some(json!({})), None)
                .await?,
        )?;
        *cache = Some(models.clone());
        Ok(models)
    }

    fn publish(&self, sessions: Vec<RemoteSessionSummary>) {
        let sessions: Vec<RemoteSessionSummary> = sessions.into_iter().map(clean_summary).collect();
        *self.inner.sessions.lock().expect("sessions") = Some(sessions.clone());
        let listeners: Vec<Arc<dyn RemoteClientListener>> = self
            .inner
            .listeners
            .lock()
            .expect("listeners")
            .iter()
            .map(|(_, l)| Arc::clone(l))
            .collect();
        for listener in listeners {
            listener.on_sessions(&sessions);
        }
    }

    fn ready(&self) {
        let client = self.clone();
        tokio::spawn(async move {
            *client.inner.models.lock().await = None;
            if let Ok(value) = client
                .inner
                .io
                .request(methods::SESSIONS_WATCH, Some(json!({})), None)
                .await
            {
                if let Ok(result) = serde_json::from_value::<SessionsListResult>(value) {
                    client.publish(result.sessions);
                }
            }
        });
        let controllers: Vec<SessionController> = self
            .inner
            .controllers
            .lock()
            .expect("controllers")
            .values()
            .cloned()
            .collect();
        for controller in controllers {
            controller.connection_ready();
        }
    }

    fn route(&self, method: &str, params: &Value) {
        if method == methods::SESSIONS_CHANGED {
            if let Ok(changed) = serde_json::from_value::<SessionsChangedParams>(params.clone()) {
                self.publish(changed.sessions);
            }
            return;
        }
        let Some(session_id) = params.get("session_id").and_then(Value::as_str) else {
            return;
        };
        let controller = self
            .inner
            .controllers
            .lock()
            .expect("controllers")
            .get(session_id)
            .cloned();
        let Some(controller) = controller else { return };
        match method {
            methods::SESSION_SNAPSHOT => {
                if let Ok(snapshot) = serde_json::from_value(params.clone()) {
                    controller.handle_snapshot(snapshot);
                }
            }
            methods::SESSION_EVENT => {
                if let Ok(event) =
                    serde_json::from_value::<crate::messages::SessionEventParams>(params.clone())
                {
                    controller.handle_event(RemoteSeqRecord {
                        seq: event.seq,
                        record: event.record,
                    });
                }
            }
            methods::SESSION_PENDING => {
                if let Ok(pending) =
                    serde_json::from_value::<crate::messages::SessionPendingParams>(params.clone())
                {
                    controller.handle_pending(pending.pending);
                }
            }
            methods::SESSION_RESET => controller.handle_reset(),
            methods::SESSION_CLOSED => controller.handle_closed(),
            _ => {}
        }
    }

    /// Detach every session (the screens are gone).
    pub fn dispose(&self) {
        let controllers: Vec<SessionController> = self
            .inner
            .controllers
            .lock()
            .expect("controllers")
            .values()
            .cloned()
            .collect();
        for controller in controllers {
            controller.deactivate();
        }
    }
}
