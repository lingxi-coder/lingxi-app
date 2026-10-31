//! The Bots of a computer against a scripted desktop: the list and its
//! negotiation, a Bot's chats, starting a chat, and the Bot a prompt came from.

mod support;

use async_trait::async_trait;
use remote_control_client::connection::ConnectionListener;
use remote_control_client::messages::{RemoteBotStatus, RemoteSessionSummary};
use remote_control_client::rpc::{RemoteErrorCode, RpcError};
use remote_control_client::session::{
    BotThreadsState, BotsState, RemoteClient, RemoteClientListener, SessionController, SessionIo,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use support::{advance, settle};

// ---------------------------------------------------------------------------
// The scripted desktop
// ---------------------------------------------------------------------------

fn bot(id: &str, change: Value) -> Value {
    let mut bot = json!({
        "bot_id": id, "name": format!("Bot {id}"), "description": format!("{id} helps"),
        "status": "idle", "waiting_threads": 0,
    });
    bot.as_object_mut()
        .unwrap()
        .extend(change.as_object().unwrap().clone());
    bot
}

fn thread(id: &str, bot_id: &str, change: Value) -> Value {
    let mut thread = json!({
        "session_id": id, "title": format!("Chat {id}"), "project_label": format!("Bot {bot_id}"),
        "status": "idle", "viewers": 0, "bot_id": bot_id,
    });
    thread
        .as_object_mut()
        .unwrap()
        .extend(change.as_object().unwrap().clone());
    thread
}

fn is_bot_method(method: &str) -> bool {
    method.starts_with("bots.") || method.starts_with("bot.")
}

/// What the desktop answers; the tests change it between steps.
struct Host {
    /// `false`: a desktop from before Bots, whose answer to every Bot method is the unknown-method error.
    supported: bool,
    sessions: Vec<Value>,
    bots: Vec<Value>,
    threads: HashMap<String, Vec<Value>>,
    /// The error to answer a method with (Bot methods: only where `supported`).
    fail_with: HashMap<String, RpcError>,
    /// The answer to give a method instead of the usual one.
    answers: HashMap<String, Value>,
    /// The chats made so far, by `client_request_id`: a repeat is given the first one, as the real desktop does.
    created: HashMap<String, String>,
    /// The next this many `bot.thread.create` requests are done, and then the link goes down: no answer arrives.
    drop_creates: usize,
    /// The same, but the link is back before the failure is seen: no `ready` is to come.
    lose_creates: usize,
    /// The same for `bot.threads.list`.
    drop_listings: usize,
}

struct Desk {
    connected: AtomicBool,
    host: Mutex<Host>,
    requests: Mutex<Vec<(String, Value)>>,
    /// While set, `bot.threads.list` waits for it before it answers.
    hold_listings: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl Desk {
    fn answer(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let mut host = self.host.lock().unwrap();
        if is_bot_method(method) && !host.supported {
            return Err(RpcError::new(
                RemoteErrorCode::InvalidParams,
                format!("unknown method {method}"),
            ));
        }
        if let Some(error) = host.fail_with.get(method) {
            return Err(error.clone());
        }
        if let Some(answer) = host.answers.get(method) {
            return Ok(answer.clone());
        }
        match method {
            "sessions.list" | "sessions.watch" => Ok(json!({ "sessions": host.sessions })),
            "bots.list" | "bots.watch" => Ok(json!({ "bots": host.bots })),
            "bot.threads.list" => {
                let listed = host
                    .threads
                    .get(params["bot_id"].as_str().unwrap_or_default())
                    .cloned()
                    .unwrap_or_default();
                if host.drop_listings > 0 {
                    host.drop_listings -= 1;
                    self.connected.store(false, Ordering::SeqCst);
                    return Err(RpcError::new(RemoteErrorCode::Closed, "dropped"));
                }
                Ok(json!({ "sessions": listed }))
            }
            "bot.thread.create" => {
                let request_id = params["client_request_id"].as_str().unwrap().to_owned();
                let made = host.created.len();
                let session_id = host
                    .created
                    .entry(request_id)
                    .or_insert_with(|| format!("chat-{}", made + 1))
                    .clone();
                if host.drop_creates > 0 {
                    host.drop_creates -= 1;
                    self.connected.store(false, Ordering::SeqCst);
                    return Err(RpcError::new(RemoteErrorCode::Closed, "dropped"));
                }
                if host.lose_creates > 0 {
                    host.lose_creates -= 1;
                    return Err(RpcError::new(RemoteErrorCode::Closed, "dropped"));
                }
                Ok(json!({ "session_id": session_id }))
            }
            "session.attach" => Ok(json!({ "mode": "snapshot", "epoch": "e1" })),
            _ => Ok(json!({})),
        }
    }
}

#[async_trait]
impl SessionIo for Desk {
    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        _timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        let params = params.unwrap_or(Value::Null);
        self.requests
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        if !self.connected() {
            return Err(RpcError::new(RemoteErrorCode::Closed, "not connected"));
        }
        let hold = if method == "bot.threads.list" {
            self.hold_listings.lock().unwrap().clone()
        } else {
            None
        };
        if let Some(hold) = hold {
            hold.notified().await;
        }
        self.answer(method, &params)
    }

    async fn notify(&self, _method: &str, _params: Option<Value>) {}

    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Recorder {
    bots: Mutex<Vec<BotsState>>,
    threads: Mutex<Vec<(String, BotThreadsState)>>,
}

impl RemoteClientListener for Recorder {
    fn on_sessions(&self, _sessions: &[RemoteSessionSummary]) {}
    fn on_bots(&self, state: &BotsState) {
        self.bots.lock().unwrap().push(state.clone());
    }
    fn on_bot_threads(&self, bot_id: &str, state: &BotThreadsState) {
        self.threads
            .lock()
            .unwrap()
            .push((bot_id.to_owned(), state.clone()));
    }
}

struct Rig {
    desk: Arc<Desk>,
    client: RemoteClient,
    router: Arc<dyn ConnectionListener>,
    recorder: Arc<Recorder>,
}

impl Rig {
    fn new() -> Self {
        let desk = Arc::new(Desk {
            connected: AtomicBool::new(true),
            host: Mutex::new(Host {
                supported: true,
                sessions: Vec::new(),
                bots: Vec::new(),
                threads: HashMap::new(),
                fail_with: HashMap::new(),
                answers: HashMap::new(),
                created: HashMap::new(),
                drop_creates: 0,
                lose_creates: 0,
                drop_listings: 0,
            }),
            requests: Mutex::default(),
            hold_listings: Mutex::default(),
        });
        let client = RemoteClient::new(desk.clone());
        let recorder = Arc::new(Recorder::default());
        client.add_listener(recorder.clone());
        let router = client.connection_listener();
        Self {
            desk,
            client,
            router,
            recorder,
        }
    }

    fn host(&self) -> MutexGuard<'_, Host> {
        self.desk.host.lock().unwrap()
    }

    /// The params of the requests the desktop received for a method.
    fn calls(&self, method: &str) -> Vec<Value> {
        self.desk
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// Every Bot request received so far, in order.
    fn bot_calls(&self) -> Vec<String> {
        self.desk
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| is_bot_method(m))
            .map(|(m, _)| m.clone())
            .collect()
    }

    /// The link is ready (again), and what it sets going has arrived.
    async fn ready(&self) {
        self.router.on_ready();
        settle().await;
    }

    /// The desktop tells the Bots as they are now, as it does when one changes.
    async fn announce(&self) {
        let bots = self.host().bots.clone();
        self.router
            .on_notification("bots.changed", &json!({ "bots": bots }));
        settle().await;
    }

    fn bot_ids(&self) -> Vec<String> {
        self.client
            .bots()
            .known()
            .iter()
            .map(|bot| bot.bot_id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Negotiation
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_desktop_that_has_bots_is_asked_for_them_next_to_the_sessions_and_what_it_sends_is_cleaned(
) {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.sessions = vec![json!({
            "session_id": "s1", "title": "Fix", "project_label": "app", "status": "idle", "viewers": 0
        })];
        host.bots = vec![
            bot(
                "a",
                json!({
                    "name": "Re\u{202E}search\ner",
                    "description": "Reads\u{200B} the\u{7} papers",
                    "waiting_threads": "many",
                }),
            ),
            bot(
                "b",
                json!({
                    "name": "x".repeat(100), "description": "d".repeat(300),
                    "waiting_threads": 2, "last_activity_at": 12, "status": "requires_action",
                }),
            ),
            Value::Null,
            json!({ "name": "no id", "description": "", "status": "idle" }),
            bot("c", json!({ "bot_id": 7 })),
        ];
    }
    assert_eq!(rig.client.bots(), BotsState::Unknown);
    rig.ready().await;

    assert_eq!(rig.calls("bots.watch").len(), 1);
    assert_eq!(rig.calls("sessions.watch").len(), 1);
    assert_eq!(
        rig.bot_calls(),
        ["bots.watch"],
        "one question, and no chats asked for before a screen shows them"
    );
    let state = rig.client.bots();
    let [a, b] = state.known() else {
        panic!("an entry without a string id is not a Bot: {state:?}");
    };
    assert_eq!(a.name, "Research er");
    assert_eq!(a.description, "Reads the papers");
    assert_eq!(a.waiting_threads, 0, "a count that is none is none");
    assert_eq!(b.name.chars().count(), 60);
    assert!(b.name.ends_with('…'));
    assert_eq!(b.description.chars().count(), 160);
    assert_eq!((b.waiting_threads, b.last_activity_at), (2, Some(12)));
    assert_eq!(b.status, RemoteBotStatus::RequiresAction);
    assert_eq!(
        rig.client.sessions().unwrap().len(),
        1,
        "the sessions are what they were"
    );
    assert_eq!(rig.recorder.bots.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn bots_changed_replaces_the_list_in_the_order_the_desktop_sends_it() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({})), bot("b", json!({}))];
    rig.ready().await;
    assert_eq!(rig.bot_ids(), ["a", "b"]);

    rig.host().bots = vec![
        bot("c", json!({ "name": "Zed\u{202E}" })),
        bot("b", json!({ "status": "running" })),
    ];
    rig.announce().await;
    let state = rig.client.bots();
    assert_eq!(
        state
            .known()
            .iter()
            .map(|bot| (bot.bot_id.as_str(), bot.name.as_str(), bot.status))
            .collect::<Vec<_>>(),
        [
            ("c", "Zed", RemoteBotStatus::Idle),
            ("b", "Bot b", RemoteBotStatus::Running)
        ]
    );

    // Something that is not a list says nothing, and a list of nothing is not "no Bots here".
    for nonsense in [
        json!({ "bots": "many" }),
        json!({ "bots": null }),
        json!({}),
        Value::Null,
    ] {
        rig.router.on_notification("bots.changed", &nonsense);
    }
    assert_eq!(rig.bot_ids(), ["c", "b"]);
    rig.host().bots = Vec::new();
    rig.announce().await;
    assert_eq!(rig.client.bots(), BotsState::Bots(Vec::new()));

    // Every change was heard, in order, and nothing else.
    let heard = rig.recorder.bots.lock().unwrap().clone();
    assert_eq!(
        heard
            .iter()
            .map(|state| state.known().len())
            .collect::<Vec<_>>(),
        [2, 2, 0]
    );
}

#[tokio::test(start_paused = true)]
async fn a_desktop_from_before_bots_answers_unknown_method_and_nothing_more_is_asked() {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.supported = false;
        host.sessions = vec![json!({
            "session_id": "s1", "title": "Fix", "project_label": "app", "status": "idle", "viewers": 0
        })];
    }
    rig.ready().await;
    assert_eq!(rig.client.bots(), BotsState::Unsupported);
    assert!(rig.client.bots().known().is_empty());
    assert_eq!(
        rig.client
            .sessions()
            .unwrap()
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>(),
        ["s1"],
        "the sessions are what they were"
    );
    assert_eq!(*rig.recorder.bots.lock().unwrap(), [BotsState::Unsupported]);

    // Whatever a screen might do next, none of it reaches the desktop.
    let store = rig.client.bot_threads("a");
    store.activate();
    store.refresh();
    let error = rig.client.create_bot_thread("a").await.unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::UnknownSession);
    settle().await;
    assert_eq!(rig.bot_calls(), ["bots.watch"]);
    assert_eq!(store.state(), BotThreadsState::default());

    // Every link asks again, so a desktop that was updated, or rolled back, in between is met as it is.
    {
        let mut host = rig.host();
        host.supported = true;
        host.bots = vec![bot("a", json!({}))];
    }
    rig.ready().await;
    assert_eq!(rig.bot_ids(), ["a"]);
    assert_eq!(rig.calls("bots.watch").len(), 2);
    assert_eq!(
        rig.calls("bot.threads.list").len(),
        1,
        "the screen that was waiting for the Bots gets its chats"
    );

    rig.host().supported = false;
    rig.ready().await;
    assert_eq!(rig.client.bots(), BotsState::Unsupported);
    assert_eq!(rig.calls("bots.watch").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn only_the_unknown_method_answer_means_unsupported() {
    for (code, message) in [
        (RemoteErrorCode::Internal, "internal error"),
        (RemoteErrorCode::Timeout, "bots.watch timed out"),
        // `invalid_params` for another reason is not what a desktop that never heard of the method says.
        (RemoteErrorCode::InvalidParams, "bad params"),
        (RemoteErrorCode::Internal, "unknown method bots.watch"),
    ] {
        let rig = Rig::new();
        rig.host()
            .fail_with
            .insert("bots.watch".into(), RpcError::new(code, message));
        rig.ready().await;
        assert_eq!(rig.calls("bots.watch").len(), 1);
        assert_eq!(rig.client.bots(), BotsState::Unknown, "{code:?} {message}");
        assert!(rig.recorder.bots.lock().unwrap().is_empty());
    }

    // A failure after the list is known leaves the list as it was.
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;
    rig.host().fail_with.insert(
        "bots.watch".into(),
        RpcError::new(RemoteErrorCode::Closed, "gone"),
    );
    rig.ready().await;
    assert_eq!(rig.bot_ids(), ["a"]);
}

// ---------------------------------------------------------------------------
// A Bot's chats
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_bots_chats_are_listed_when_a_screen_shows_them_and_again_when_the_bot_or_the_link_changed(
) {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.bots = vec![
            bot("a", json!({ "last_activity_at": 1 })),
            bot("b", json!({})),
        ];
        host.threads.insert(
            "a".into(),
            vec![
                thread("t2", "a", json!({ "title": "Newer\u{202E} one" })),
                thread("t1", "a", json!({})),
            ],
        );
    }
    rig.ready().await;
    let listings = || rig.calls("bot.threads.list").len();
    let store = rig.client.bot_threads("a");
    assert_eq!(store.bot_id(), "a");
    assert_eq!(store.state(), BotThreadsState::default());

    // Nobody shows the chats yet: nothing is asked.
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 2 })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 0);

    store.activate();
    settle().await;
    assert_eq!(listings(), 1);
    assert_eq!(rig.calls("bot.threads.list")[0], json!({ "bot_id": "a" }));
    let state = rig.client.bot_threads("a").state();
    assert_eq!(
        state
            .threads
            .as_ref()
            .unwrap()
            .iter()
            .map(|chat| (chat.session_id.as_str(), chat.title.as_str()))
            .collect::<Vec<_>>(),
        [("t2", "Newer one"), ("t1", "Chat t1")],
        "in the desktop's order, and cleaned like any session"
    );
    assert_eq!(state.error, None);
    assert_eq!(state.threads.unwrap()[0].bot_id.as_deref(), Some("a"));
    assert_eq!(rig.client.bot_of_thread("t1").as_deref(), Some("a"));
    assert_eq!(rig.client.bot_of_thread("t2").as_deref(), Some("a"));
    assert_eq!(rig.client.bot_of_thread("nobody"), None);
    {
        let heard = rig.recorder.threads.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].0, "a");
        assert_eq!(heard[0].1, store.state());
    }

    // Changes that say nothing about the chats of this Bot.
    rig.announce().await;
    rig.host().bots = vec![
        bot(
            "a",
            json!({ "last_activity_at": 2, "name": "Renamed", "description": "does other things" }),
        ),
        bot("b", json!({})),
    ];
    rig.announce().await;
    rig.host().bots = vec![
        bot(
            "a",
            json!({ "last_activity_at": 2, "name": "Renamed", "description": "does other things" }),
        ),
        bot(
            "b",
            json!({ "last_activity_at": 90, "status": "running", "waiting_threads": 3 }),
        ),
    ];
    rig.announce().await;
    assert_eq!(
        listings(),
        1,
        "the same list, a new name, and another Bot's activity change nothing here"
    );

    // Each of the three things that do.
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 3 })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 2, "activity");
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 3, "status": "running" })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 3, "status");
    rig.host().bots = vec![
        bot(
            "a",
            json!({ "last_activity_at": 3, "status": "running", "waiting_threads": 1 }),
        ),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 4, "waiting chats");

    // A new link lists them once more; the answer about the Bots that follows it adds nothing.
    rig.ready().await;
    assert_eq!(listings(), 5);
    assert_eq!(rig.calls("bots.watch").len(), 2);

    // A screen that left is not served: nothing is asked for it any more.
    store.deactivate();
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 4 })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    rig.ready().await;
    assert_eq!(listings(), 5);

    // Two screens count: the chats stay current until the last one leaves.
    store.activate();
    store.activate();
    settle().await;
    assert_eq!(listings(), 6, "the second screen does not list them again");
    store.deactivate();
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 5 })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 7);
    store.deactivate();
    store.deactivate();
    rig.host().bots = vec![
        bot("a", json!({ "last_activity_at": 6 })),
        bot("b", json!({})),
    ];
    rig.announce().await;
    assert_eq!(listings(), 7);
}

#[tokio::test(start_paused = true)]
async fn a_bot_that_was_gone_and_is_back_has_its_chats_listed_again_and_one_that_is_gone_is_not_asked_about(
) {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.bots = vec![bot("a", json!({}))];
        host.threads
            .insert("a".into(), vec![thread("t1", "a", json!({}))]);
    }
    rig.ready().await;
    let store = rig.client.bot_threads("a");
    store.activate();
    settle().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 1);

    rig.host().bots = Vec::new();
    rig.announce().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 1);
    assert!(rig.client.bots().known().is_empty());

    rig.host().bots = vec![bot("a", json!({}))];
    rig.announce().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_change_that_arrives_while_the_chats_are_being_listed_gets_one_more_listing_not_one_each()
{
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.bots = vec![bot("a", json!({}))];
        host.threads
            .insert("a".into(), vec![thread("t1", "a", json!({}))]);
    }
    rig.ready().await;
    let hold = Arc::new(tokio::sync::Notify::new());
    *rig.desk.hold_listings.lock().unwrap() = Some(hold.clone());
    let store = rig.client.bot_threads("a");
    store.activate();
    settle().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 1);

    rig.host().bots = vec![bot("a", json!({ "last_activity_at": 1 }))];
    rig.announce().await;
    rig.host().bots = vec![bot("a", json!({ "last_activity_at": 2 }))];
    rig.announce().await;
    store.refresh();
    settle().await;
    assert_eq!(
        rig.calls("bot.threads.list").len(),
        1,
        "one listing at a time"
    );
    assert_eq!(store.state(), BotThreadsState::default());

    *rig.desk.hold_listings.lock().unwrap() = None;
    hold.notify_one();
    settle().await;
    assert_eq!(
        rig.calls("bot.threads.list").len(),
        2,
        "the listing the changes asked for, and no more"
    );
    advance(Duration::from_secs(5)).await;
    assert_eq!(rig.calls("bot.threads.list").len(), 2);
    assert_eq!(
        store
            .state()
            .threads
            .unwrap()
            .iter()
            .map(|chat| chat.session_id.as_str())
            .collect::<Vec<_>>(),
        ["t1"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_listing_that_fails_is_kept_and_does_not_stop_the_next_one_and_a_dropped_link_is_not_a_failure(
) {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.bots = vec![bot("a", json!({}))];
        host.threads
            .insert("a".into(), vec![thread("t1", "a", json!({}))]);
        host.fail_with.insert(
            "bot.threads.list".into(),
            RpcError::new(RemoteErrorCode::Internal, "the chats could not be listed"),
        );
    }
    rig.ready().await;
    let store = rig.client.bot_threads("a");
    store.activate();
    settle().await;
    let state = store.state();
    assert_eq!(state.error.as_ref().unwrap().code, "internal");
    assert_eq!(state.threads, None);

    rig.host().fail_with.clear();
    store.refresh();
    settle().await;
    let state = store.state();
    assert_eq!(state.error, None, "the next listing that works clears it");
    assert_eq!(state.threads.as_ref().unwrap().len(), 1);

    // A Bot that cannot be reached says so, and the chats held stay.
    for code in ["unknown_session", "not_remote_enabled"] {
        rig.host().fail_with.insert(
            "bot.threads.list".into(),
            RpcError::new(RemoteErrorCode::from_wire(code), "no"),
        );
        store.refresh();
        settle().await;
        let state = store.state();
        assert_eq!(state.error.as_ref().unwrap().code, code);
        assert_eq!(state.threads.as_ref().unwrap().len(), 1);
    }
    rig.host().fail_with.clear();
    store.refresh();
    settle().await;
    assert_eq!(store.state().error, None);

    // The link dies under a listing: that is no error to show, and the next ready asks again.
    let before = rig.calls("bot.threads.list").len();
    rig.host().drop_listings = 1;
    store.refresh();
    settle().await;
    assert_eq!(rig.calls("bot.threads.list").len(), before + 1);
    assert_eq!(store.state().error, None);
    assert!(!rig.desk.connected());
    store.refresh();
    settle().await;
    assert_eq!(
        rig.calls("bot.threads.list").len(),
        before + 1,
        "nothing is sent while there is no link"
    );
    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    assert_eq!(rig.calls("bot.threads.list").len(), before + 2);
    assert_eq!(store.state().error, None);
    assert_eq!(store.state().threads.unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_client_that_is_disposed_keeps_no_bots_chats_current() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;
    let store = rig.client.bot_threads("a");
    store.activate();
    settle().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 1);

    rig.client.dispose();
    rig.host().bots = vec![bot("a", json!({ "last_activity_at": 1 }))];
    rig.announce().await;
    rig.ready().await;
    store.refresh();
    settle().await;
    assert_eq!(rig.calls("bot.threads.list").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_screen_that_opens_while_there_is_no_link_gets_its_chats_when_the_link_is_ready() {
    let rig = Rig::new();
    {
        let mut host = rig.host();
        host.bots = vec![bot("a", json!({}))];
        host.threads
            .insert("a".into(), vec![thread("t1", "a", json!({}))]);
    }
    rig.ready().await;
    rig.desk.connected.store(false, Ordering::SeqCst);
    let store = rig.client.bot_threads("a");
    store.activate();
    settle().await;
    assert_eq!(
        rig.bot_calls(),
        ["bots.watch"],
        "nothing is sent while there is no link"
    );
    assert_eq!(store.state(), BotThreadsState::default());

    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    assert_eq!(
        rig.bot_calls(),
        ["bots.watch", "bots.watch", "bot.threads.list"],
        "one listing: the Bots that come with the link are the ones that were"
    );
    assert_eq!(store.state().threads.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Starting a chat
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_new_chat_is_asked_for_with_a_fresh_request_id_and_its_bot_is_remembered() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    let first = rig.client.create_bot_thread("a").await.unwrap();
    let second = rig.client.create_bot_thread("a").await.unwrap();
    let creates = rig.calls("bot.thread.create");
    assert_eq!(creates.len(), 2);
    for create in &creates {
        let mut keys: Vec<&String> = create.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["bot_id", "client_request_id"]);
        assert_eq!(create["bot_id"], "a");
        assert!(!create["client_request_id"].as_str().unwrap().is_empty());
    }
    assert_ne!(
        creates[0]["client_request_id"],
        creates[1]["client_request_id"]
    );
    assert_ne!(first, second);
    assert_eq!(rig.client.bot_of_thread(&first).as_deref(), Some("a"));
    assert_eq!(rig.client.bot_of_thread(&second).as_deref(), Some("a"));
}

#[tokio::test(start_paused = true)]
async fn a_link_that_drops_with_the_request_on_it_gets_one_more_request_with_the_same_id_and_the_same_chat(
) {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    rig.host().drop_creates = 1;
    let client = rig.client.clone();
    let creating = tokio::spawn(async move { client.create_bot_thread("a").await });
    settle().await;
    assert_eq!(rig.calls("bot.thread.create").len(), 1);
    assert!(!rig.desk.connected());
    assert!(!creating.is_finished(), "it waits for the link");

    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    assert_eq!(creating.await.unwrap().unwrap(), "chat-1");
    let creates = rig.calls("bot.thread.create");
    assert_eq!(creates.len(), 2);
    assert_eq!(
        creates[0]["client_request_id"],
        creates[1]["client_request_id"]
    );
    assert_eq!(rig.host().created.len(), 1, "the desktop made one chat");
    assert_eq!(rig.client.bot_of_thread("chat-1").as_deref(), Some("a"));
}

#[tokio::test(start_paused = true)]
async fn a_link_that_is_back_already_needs_no_wait() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    // The link dropped under the request and was up again before the failure was seen.
    rig.host().lose_creates = 1;
    assert_eq!(rig.client.create_bot_thread("a").await.unwrap(), "chat-1");
    let creates = rig.calls("bot.thread.create");
    assert_eq!(creates.len(), 2);
    assert_eq!(
        creates[0]["client_request_id"],
        creates[1]["client_request_id"]
    );
    assert_eq!(rig.host().created.len(), 1, "the desktop made one chat");
}

#[tokio::test(start_paused = true)]
async fn the_request_is_repeated_once_not_again_and_again() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    rig.host().drop_creates = usize::MAX; // every link it goes over dies under it
    let client = rig.client.clone();
    let creating = tokio::spawn(async move { client.create_bot_thread("a").await });
    settle().await;
    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    let error = creating.await.unwrap().unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Closed);
    assert_eq!(rig.calls("bot.thread.create").len(), 2);

    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    advance(Duration::from_secs(120)).await;
    assert_eq!(rig.calls("bot.thread.create").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_link_that_does_not_come_back_in_time_ends_the_wait_with_an_error_and_nothing_more_is_sent(
) {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    rig.host().drop_creates = 1;
    let client = rig.client.clone();
    let creating = tokio::spawn(async move { client.create_bot_thread("a").await });
    settle().await;
    advance(Duration::from_secs(29)).await;
    assert!(!creating.is_finished(), "it waits for the link");
    advance(Duration::from_secs(2)).await;
    let error = creating.await.unwrap().unwrap_err();
    assert_eq!(error.code, RemoteErrorCode::Closed);
    assert_eq!(rig.calls("bot.thread.create").len(), 1);

    // A link that comes back after that is no use to the request that gave up.
    rig.desk.connected.store(true, Ordering::SeqCst);
    rig.ready().await;
    assert_eq!(rig.calls("bot.thread.create").len(), 1);
    assert_eq!(rig.client.bot_of_thread("chat-1"), None);
}

#[tokio::test(start_paused = true)]
async fn what_the_desktop_refuses_surfaces_as_it_came_with_no_second_request() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;

    for code in [
        RemoteErrorCode::NotRemoteEnabled,
        RemoteErrorCode::UnknownSession,
        RemoteErrorCode::Internal,
        RemoteErrorCode::InvalidParams,
        RemoteErrorCode::Timeout,
    ] {
        rig.host()
            .fail_with
            .insert("bot.thread.create".into(), RpcError::new(code, "refused"));
        let before = rig.calls("bot.thread.create").len();
        let error = rig.client.create_bot_thread("a").await.unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(rig.calls("bot.thread.create").len(), before + 1, "{code:?}");
    }
    assert_eq!(rig.client.bot_of_thread("chat-1"), None);

    rig.host().fail_with.clear();
    assert_eq!(
        rig.client.create_bot_thread("a").await.unwrap(),
        "chat-1",
        "the next try works"
    );
}

#[tokio::test(start_paused = true)]
async fn a_chat_the_desktop_has_none_to_give_is_not_made_up() {
    let rig = Rig::new();
    rig.host().bots = vec![bot("a", json!({}))];
    rig.ready().await;
    for answer in [
        json!({ "session_id": "" }),
        json!({}),
        json!({ "session_id": 7 }),
    ] {
        rig.host()
            .answers
            .insert("bot.thread.create".into(), answer);
        let error = rig.client.create_bot_thread("a").await.unwrap_err();
        assert_eq!(error.code, RemoteErrorCode::Internal);
    }
    assert_eq!(rig.client.bot_of_thread(""), None);
}

// ---------------------------------------------------------------------------
// The Bot a prompt came from
// ---------------------------------------------------------------------------

fn prompt_record(seq: u64, turn: u64, origin: &str, from_bot_name: Option<Value>) -> Value {
    let mut record = json!({
        "type": "prompt", "turn_id": turn, "text": "do it", "images": [], "origin": origin,
    });
    if let Some(name) = from_bot_name {
        record["from_bot_name"] = name;
    }
    json!({ "seq": seq, "record": record })
}

fn hold_records(controller: &SessionController, records: Vec<Value>) {
    for part in [
        json!({ "phase": "begin", "session_id": "s1", "epoch": "e1", "first_message_index": 0 }),
        json!({ "phase": "records", "session_id": "s1", "records": records }),
        json!({ "phase": "end", "session_id": "s1", "cursor": { "epoch": "e1", "seq": 20 },
                "pending": {}, "turn": { "running": false } }),
    ] {
        controller.handle_snapshot(serde_json::from_value(part).unwrap());
    }
}

#[tokio::test(start_paused = true)]
async fn a_prompt_a_bot_handed_over_carries_its_name_when_it_gave_one() {
    let rig = Rig::new();
    let controller = rig.client.session("s1");
    hold_records(
        &controller,
        vec![
            prompt_record(1, 1, "bot", Some(json!("Researcher"))),
            prompt_record(2, 2, "bot", None),
            prompt_record(3, 3, "bot", Some(json!(""))),
            prompt_record(4, 4, "bot", Some(json!("\u{202E}\u{200B} \n"))),
            prompt_record(5, 5, "bot", Some(json!("N".repeat(100)))),
            prompt_record(6, 6, "bot", Some(json!("Al\u{202E}pha\nBeta"))),
            // Only a Bot's prompt has a Bot to name.
            prompt_record(7, 7, "desktop", Some(json!("Alpha"))),
            prompt_record(8, 8, "scheduled", Some(json!("Alpha"))),
            prompt_record(9, 9, "device", Some(json!("Alpha"))),
        ],
    );
    let name = |turn: u64| controller.prompt_bot_name(turn);
    assert_eq!(name(1).as_deref(), Some("Researcher"));
    assert_eq!(name(2), None, "no name");
    assert_eq!(name(3), None, "an empty name");
    assert_eq!(name(4), None, "nothing is left of the name");
    assert_eq!(name(5), Some(format!("{}…", "N".repeat(59))));
    assert_eq!(name(6).as_deref(), Some("Alpha Beta"));
    assert_eq!(name(7), None);
    assert_eq!(name(8), None);
    assert_eq!(name(9), None);
    assert_eq!(name(99), None, "a turn that has no prompt");

    // Through the client, by session; a session it does not hold has no prompts.
    assert_eq!(
        rig.client.prompt_bot_name("s1", 1).as_deref(),
        Some("Researcher")
    );
    assert_eq!(rig.client.prompt_bot_name("s1", 7), None);
    assert_eq!(rig.client.prompt_bot_name("other", 1), None);

    // A live prompt is looked up the same way.
    controller.handle_event(
        serde_json::from_value(prompt_record(21, 21, "bot", Some(json!("Writer")))).unwrap(),
    );
    assert_eq!(name(21).as_deref(), Some("Writer"));
}
