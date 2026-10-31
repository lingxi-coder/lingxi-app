//! The Bots of a computer as a viewer sees them: the list (`bots.watch`, then
//! `bots.changed`), the chats of a Bot a screen shows (`bot.threads.list`) and
//! starting a chat (`bot.thread.create`). The Rust twin of the Bot parts of
//! `apps/web/src/remote/remoteClient.ts`.
//!
//! Only a computer that has Bots answers any of it. One from before them answers
//! `bots.watch` with the unknown-method error: the state is then
//! [`BotsState::Unsupported`], and nothing more about Bots is asked of it until
//! the link is ready again. A Bot's chats are ordinary sessions: a screen uses
//! them through [`RemoteClient::session`] with the `session_id` listed here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::watch;

use super::{
    clean_summary, parse, session_error, ClientInner, RemoteClient, SessionError, BOT_NAME_CHARS,
};
use crate::encoding::random_id;
use crate::messages::{
    methods, BotThreadCreateParams, BotThreadCreateResult, BotThreadsListParams,
    BotThreadsListResult, RemoteBotSummary, RemoteSessionSummary,
};
use crate::rpc::{RemoteErrorCode, RpcError};
use crate::text::display_text;

/// How long a request that the link dropped under waits for the link to come back before it is given up.
const RECONNECT_WAIT: Duration = Duration::from_secs(30);
/// Longest description shown of a Bot, in characters.
const BOT_DESCRIPTION_CHARS: usize = 160;

/// What the viewer knows of the computer's Bots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BotsState {
    /// The computer has not answered yet.
    #[default]
    Unknown,
    /// The computer answered that it does not know Bots (a version from before
    /// them). It is asked again after every reconnect.
    Unsupported,
    /// The Bots whose Remote Control switch is on, as the computer sent them last.
    Bots(Vec<RemoteBotSummary>),
}

impl BotsState {
    /// The Bots, none while they are not known or the computer has none to give.
    pub fn known(&self) -> &[RemoteBotSummary] {
        match self {
            Self::Bots(bots) => bots,
            Self::Unknown | Self::Unsupported => &[],
        }
    }
}

/// What the viewer holds of one Bot's chats.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BotThreadsState {
    /// The Bot's chats, newest first, as the computer listed them last; `None`
    /// until it has.
    pub threads: Option<Vec<RemoteSessionSummary>>,
    /// Why the last listing failed; the next one that works clears it. The codes
    /// `unknown_session` and `not_remote_enabled` say that the Bot cannot be reached.
    pub error: Option<SessionError>,
}

/// What a [`RemoteClient`] keeps of the computer's Bots.
pub(super) struct BotsHub {
    state: Mutex<BotsState>,
    /// Held while the state changes and the listeners hear it, so that they hear
    /// the changes in the order they were made.
    changes: Mutex<()>,
    threads: Mutex<HashMap<String, BotThreads>>,
    /// The Bot each chat belongs to, for the chats listed to this viewer and the
    /// ones it started.
    thread_bots: Mutex<HashMap<String, String>>,
    /// Counts the times the link became ready; [`RemoteClient::create_bot_thread`]
    /// waits on it for the link to come back.
    readied: watch::Sender<u64>,
}

impl Default for BotsHub {
    fn default() -> Self {
        Self {
            state: Mutex::new(BotsState::Unknown),
            changes: Mutex::new(()),
            threads: Mutex::new(HashMap::new()),
            thread_bots: Mutex::new(HashMap::new()),
            readied: watch::channel(0).0,
        }
    }
}

/// What the computer sends for a Bot is shown on one line, cut to a length.
fn clean_bot(mut bot: RemoteBotSummary) -> RemoteBotSummary {
    bot.name = display_text(&bot.name, BOT_NAME_CHARS);
    bot.description = display_text(&bot.description, BOT_DESCRIPTION_CHARS);
    bot
}

/// The Bots of a `bots.watch` answer or a `bots.changed` notification, or `None`
/// when there is no list. An entry that is not a Bot is left out and the others
/// are kept, as the Web viewer does: serde needs a string `bot_id`, a name, a
/// description and a known status, and reads a bad `waiting_threads` as none.
fn parse_bots(value: &Value) -> Option<Vec<RemoteBotSummary>> {
    let entries = value.get("bots")?.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
            .collect(),
    )
}

/// What a computer that has never heard of a method answers: `invalid_params`,
/// "unknown method <name>".
fn is_unknown_method(error: &RpcError) -> bool {
    error.code == RemoteErrorCode::InvalidParams && error.message.starts_with("unknown method")
}

/// Whether `bots.changed` shows something in the Bot's chats that its last summary did not.
fn activity_changed(was: Option<&RemoteBotSummary>, now: &RemoteBotSummary) -> bool {
    was.is_none_or(|was| {
        was.last_activity_at != now.last_activity_at
            || was.status != now.status
            || was.waiting_threads != now.waiting_threads
    })
}

impl ClientInner {
    /// Whether the computer may be asked about Bots: it has not said it does not know them.
    fn can_ask_bots(&self) -> bool {
        !matches!(
            *self.bots.state.lock().expect("bots"),
            BotsState::Unsupported
        )
    }

    /// Whether a Bot request can go out now: the link is ready and the computer knows Bots.
    fn can_request_bots(&self) -> bool {
        self.io.connected() && self.can_ask_bots()
    }
}

impl RemoteClient {
    /// The computer's Bots: [`BotsState::Unknown`] until it has answered, and
    /// [`BotsState::Unsupported`] when it answered that it does not know Bots.
    pub fn bots(&self) -> BotsState {
        self.inner.bots.state.lock().expect("bots").clone()
    }

    /// The chats of a Bot, created on first use and kept. A screen must
    /// [`activate`](BotThreads::activate) it for it to list anything.
    pub fn bot_threads(&self, bot_id: &str) -> BotThreads {
        self.inner
            .bots
            .threads
            .lock()
            .expect("threads")
            .entry(bot_id.to_owned())
            .or_insert_with(|| BotThreads::new(bot_id, Arc::downgrade(&self.inner)))
            .clone()
    }

    /// The Bot a chat belongs to, if this viewer has been told: from a list of
    /// the Bot's chats, or by starting the chat.
    pub fn bot_of_thread(&self, session_id: &str) -> Option<String> {
        self.inner
            .bots
            .thread_bots
            .lock()
            .expect("thread bots")
            .get(session_id)
            .cloned()
    }

    /// Start a new chat of a Bot and return its session id. If the link drops
    /// with the request on it, the computer may have made the chat already: ask
    /// once more when the link is back (for up to 30 s), with the same
    /// `client_request_id`, which it answers with the first chat instead of a
    /// second. Any other failure, or no link in time, is returned as it came.
    pub async fn create_bot_thread(&self, bot_id: &str) -> Result<String, RpcError> {
        if !self.inner.can_ask_bots() {
            return Err(RpcError::new(
                RemoteErrorCode::UnknownSession,
                "this computer does not know Bots",
            ));
        }
        let params = json!(BotThreadCreateParams {
            bot_id: bot_id.to_owned(),
            client_request_id: random_id(),
        });
        let io = &self.inner.io;
        let created = match io
            .request(methods::BOT_THREAD_CREATE, Some(params.clone()), None)
            .await
        {
            Err(error) if error.code == RemoteErrorCode::Closed => {
                self.until_ready().await?;
                io.request(methods::BOT_THREAD_CREATE, Some(params), None)
                    .await?
            }
            other => other?,
        };
        let session_id = parse::<BotThreadCreateResult>(created)?.session_id;
        if session_id.is_empty() {
            return Err(RpcError::new(RemoteErrorCode::Internal, "no session id"));
        }
        self.inner
            .bots
            .thread_bots
            .lock()
            .expect("thread bots")
            .insert(session_id.clone(), bot_id.to_owned());
        Ok(session_id)
    }

    /// Resolve once the link is ready (now, if it is); fail with `closed` if it
    /// is not within [`RECONNECT_WAIT`].
    async fn until_ready(&self) -> Result<(), RpcError> {
        let not_connected = || RpcError::new(RemoteErrorCode::Closed, "not connected");
        let mut readied = self.inner.bots.readied.subscribe();
        tokio::time::timeout(RECONNECT_WAIT, async {
            while !self.inner.io.connected() {
                readied.changed().await.map_err(|_| not_connected())?;
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|_| Err(not_connected()))
    }

    /// The link is ready, again or for the first time: ask for the Bots, list
    /// again the chats a screen shows, and let a request that waited for the
    /// link go on.
    pub(super) fn bots_ready(&self) {
        self.watch_bots();
        let stores: Vec<BotThreads> = self
            .inner
            .bots
            .threads
            .lock()
            .expect("threads")
            .values()
            .cloned()
            .collect();
        for store in stores {
            store.connection_ready();
        }
        self.inner.bots.readied.send_modify(|ready| *ready += 1);
    }

    /// Ask for the Bots and to be told when they change. A computer from before
    /// Bots answers with the unknown-method error: it has none, and nothing more
    /// about Bots is asked of it until the link is ready again. Any other
    /// failure leaves the state as it was.
    fn watch_bots(&self) {
        let client = self.clone();
        tokio::spawn(async move {
            match client
                .inner
                .io
                .request(methods::BOTS_WATCH, Some(json!({})), None)
                .await
            {
                Ok(answer) => client.bots_changed(&answer),
                Err(error) if is_unknown_method(&error) => {
                    client.change_bots(BotsState::Unsupported);
                }
                Err(_) => {}
            }
        });
    }

    /// The Bots as a `bots.watch` answer or a `bots.changed` notification carries them.
    pub(super) fn bots_changed(&self, params: &Value) {
        if let Some(bots) = parse_bots(params) {
            self.set_bots(bots);
        }
    }

    /// The screens are gone: no Bot's chats are kept current any more.
    pub(super) fn bots_dispose(&self) {
        let stores: Vec<BotThreads> = self
            .inner
            .bots
            .threads
            .lock()
            .expect("threads")
            .values()
            .cloned()
            .collect();
        for store in stores {
            store.inner.core.lock().expect("threads").shown = 0;
        }
    }

    /// Replace the list of Bots; a Bot whose chats a screen shows has them
    /// listed again if its activity changed.
    fn set_bots(&self, bots: Vec<RemoteBotSummary>) {
        let bots: Vec<RemoteBotSummary> = bots.into_iter().map(clean_bot).collect();
        let before = self.change_bots(BotsState::Bots(bots.clone()));
        let stores: Vec<BotThreads> = self
            .inner
            .bots
            .threads
            .lock()
            .expect("threads")
            .values()
            .cloned()
            .collect();
        for store in stores {
            let Some(now) = bots.iter().find(|bot| bot.bot_id == store.inner.bot_id) else {
                continue;
            };
            let was = before.known().iter().find(|bot| bot.bot_id == now.bot_id);
            if activity_changed(was, now) {
                store.refresh();
            }
        }
    }

    /// Make `next` the state and tell the listeners; returns the state it replaced.
    fn change_bots(&self, next: BotsState) -> BotsState {
        let _changes = self.inner.bots.changes.lock().expect("changes");
        let before = std::mem::replace(
            &mut *self.inner.bots.state.lock().expect("bots"),
            next.clone(),
        );
        for listener in self.inner.hearers() {
            listener.on_bots(&next);
        }
        before
    }
}

// ---------------------------------------------------------------------------
// A Bot's chats
// ---------------------------------------------------------------------------

struct ThreadsCore {
    state: BotThreadsState,
    /// Screens that show the chats.
    shown: u32,
    /// A listing is on its way.
    fetching: bool,
    /// A change came in while a listing was on its way: that listing may be out of date.
    stale: bool,
}

struct ThreadsInner {
    bot_id: String,
    client: Weak<ClientInner>,
    core: Mutex<ThreadsCore>,
}

/// The chats of one Bot. The computer has no `bot.threads.watch`, so the list is
/// asked for again whenever something says it may have changed, and only while a
/// screen shows it ([`activate`](Self::activate)): when the screen opens, after
/// each reconnect, and when `bots.changed` reports other activity of the Bot.
/// Cheap to clone.
#[derive(Clone)]
pub struct BotThreads {
    inner: Arc<ThreadsInner>,
}

impl std::fmt::Debug for BotThreads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotThreads")
            .field("bot_id", &self.inner.bot_id)
            .finish_non_exhaustive()
    }
}

impl BotThreads {
    fn new(bot_id: &str, client: Weak<ClientInner>) -> Self {
        Self {
            inner: Arc::new(ThreadsInner {
                bot_id: bot_id.to_owned(),
                client,
                core: Mutex::new(ThreadsCore {
                    state: BotThreadsState::default(),
                    shown: 0,
                    fetching: false,
                    stale: false,
                }),
            }),
        }
    }

    /// The Bot.
    pub fn bot_id(&self) -> &str {
        &self.inner.bot_id
    }

    /// The chats held now.
    pub fn state(&self) -> BotThreadsState {
        self.inner.core.lock().expect("threads").state.clone()
    }

    /// A screen shows the Bot's chats: list them now, and keep them current
    /// until the screen calls [`deactivate`](Self::deactivate). Must be called
    /// inside a Tokio runtime.
    pub fn activate(&self) {
        let idle = {
            let mut core = self.inner.core.lock().expect("threads");
            core.shown += 1;
            !core.fetching
        };
        if idle {
            self.refresh();
        }
    }

    /// A screen stopped showing the chats.
    pub fn deactivate(&self) {
        let mut core = self.inner.core.lock().expect("threads");
        core.shown = core.shown.saturating_sub(1);
    }

    /// List the chats again, if a screen shows them. One listing is on its way
    /// at a time; asking while it is makes it be followed by exactly one more.
    pub fn refresh(&self) {
        let Some(client) = self.inner.client.upgrade() else {
            return;
        };
        let can_request = client.can_request_bots();
        {
            let mut core = self.inner.core.lock().expect("threads");
            if core.shown == 0 || !can_request {
                return;
            }
            if core.fetching {
                core.stale = true;
                return;
            }
            core.fetching = true;
        }
        let store = self.clone();
        tokio::spawn(async move { store.load().await });
    }

    /// The link is ready again.
    fn connection_ready(&self) {
        self.refresh();
    }

    async fn load(&self) {
        let Some(client) = self.inner.client.upgrade() else {
            return;
        };
        let params = json!(BotThreadsListParams {
            bot_id: self.inner.bot_id.clone(),
        });
        let listed = client
            .io
            .request(methods::BOT_THREADS_LIST, Some(params), None)
            .await
            .and_then(parse::<BotThreadsListResult>);
        match listed {
            Ok(listed) => {
                let threads: Vec<RemoteSessionSummary> =
                    listed.sessions.into_iter().map(clean_summary).collect();
                client.bots.thread_bots.lock().expect("thread bots").extend(
                    threads
                        .iter()
                        .map(|thread| (thread.session_id.clone(), self.inner.bot_id.clone())),
                );
                self.commit(&client, |state| {
                    state.threads = Some(threads);
                    state.error = None;
                });
            }
            // The link dropped: the next `connection_ready` asks again.
            Err(error) if error.code == RemoteErrorCode::Closed => {}
            Err(error) => {
                let failure = session_error(&error);
                self.commit(&client, |state| state.error = Some(failure));
            }
        }
        let again = {
            let mut core = self.inner.core.lock().expect("threads");
            core.fetching = false;
            std::mem::take(&mut core.stale)
        };
        if again {
            self.refresh();
        }
    }

    /// Change the state and tell the listeners.
    fn commit(&self, client: &ClientInner, change: impl FnOnce(&mut BotThreadsState)) {
        let state = {
            let mut core = self.inner.core.lock().expect("threads");
            change(&mut core.state);
            core.state.clone()
        };
        for listener in client.hearers() {
            listener.on_bot_threads(&self.inner.bot_id, &state);
        }
    }
}
