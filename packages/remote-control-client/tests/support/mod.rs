//! Helpers shared by the integration tests.
#![allow(dead_code)]

pub mod host;
pub mod http;
pub mod x25519_ref;

use serde_json::Value;
use std::path::PathBuf;

/// Load one of the cross-language fixtures of the TypeScript reference.
pub fn fixture(name: &str) -> Value {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "bridge-client",
        "test",
        "fixtures",
        "remote",
        &format!("{name}.json"),
    ]
    .iter()
    .collect();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture is JSON")
}

pub fn hex_decode(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&remote_control_client::encoding::sha256(bytes))
}

use async_trait::async_trait;
use remote_control_client::rtdb::{ChangeKind, Listener, MemoryRtdb, Rtdb, RtdbError, RtdbMirror};
use remote_control_client::signaling::{parse_frame_key, to_host_path};
use remote_control_client::transport::LinkEvent;
use remote_control_client::tunnel::{TunnelLink, TunnelLinkOptions};
use remote_control_client::tunnel_crypto::{derive_tunnel_keys, TunnelDir, TunnelKeys};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Let every ready task run until the system is idle (without advancing paused time).
pub async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

/// Advance paused time in small steps, letting chained timers (retries, flushes) register in between.
pub async fn advance(total: Duration) {
    let step = if total > Duration::from_secs(5) {
        Duration::from_secs(1)
    } else {
        Duration::from_millis(10)
    };
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        tokio::time::advance(step).await;
        settle().await;
        elapsed += step;
    }
}

/// An RTDB that delays writes and records how many were in flight at once.
pub struct SlowRtdb {
    pub inner: Arc<MemoryRtdb>,
    pub delay: Duration,
    pub active: AtomicUsize,
    pub max_active: AtomicUsize,
}

impl SlowRtdb {
    pub fn new(inner: Arc<MemoryRtdb>, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner,
            delay,
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Rtdb for SlowRtdb {
    async fn get(&self, path: &str) -> Result<Json, RtdbError> {
        self.inner.get(path).await
    }
    async fn put(&self, path: &str, value: &Json) -> Result<(), RtdbError> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        let result = self.inner.put(path, value).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
    async fn patch(&self, path: &str, value: &Map<String, Json>) -> Result<(), RtdbError> {
        self.inner.patch(path, value).await
    }
    async fn remove(&self, path: &str) -> Result<(), RtdbError> {
        self.inner.remove(path).await
    }
    fn listen(&self, path: &str) -> Listener {
        self.inner.listen(path)
    }
}

/// The host-side dispatcher of the tunnel: one listener on `to_host/{host}` whose
/// `up` frames are handed to the link of their connection.
pub struct HostInbox {
    links: Arc<Mutex<HashMap<String, TunnelLink>>>,
    task: tokio::task::JoinHandle<()>,
}

impl HostInbox {
    pub fn start(rtdb: &dyn Rtdb, uid: &str, host_id: &str) -> Self {
        let links: Arc<Mutex<HashMap<String, TunnelLink>>> = Arc::default();
        let mut listener = rtdb.listen(&to_host_path(uid, host_id).unwrap());
        let shared = Arc::clone(&links);
        let task = tokio::spawn(async move {
            let mut mirror = RtdbMirror::new();
            while let Some(item) = listener.recv().await {
                let remote_control_client::rtdb::ListenItem::Event(event) = item else {
                    return;
                };
                for change in mirror.apply(&event).at(3) {
                    let [conn, side, key] = change.path.as_slice() else {
                        continue;
                    };
                    if side != "up" || change.kind == ChangeKind::Removed {
                        continue;
                    }
                    if let (Some(seq), Some(link)) =
                        (parse_frame_key(key), shared.lock().unwrap().get(conn))
                    {
                        link.deliver_frame(seq, &change.value);
                    }
                }
            }
        });
        Self { links, task }
    }

    pub fn register(&self, conn_id: &str, link: TunnelLink) {
        self.links.lock().unwrap().insert(conn_id.to_owned(), link);
    }
}

impl Drop for HostInbox {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct TunnelPair {
    pub viewer: TunnelLink,
    pub viewer_events: mpsc::UnboundedReceiver<LinkEvent>,
    pub host: TunnelLink,
    pub host_events: mpsc::UnboundedReceiver<LinkEvent>,
    pub inbox: HostInbox,
    pub up: String,
    pub down: String,
}

pub const UID: &str = "user-1";
pub const HOST: &str = "nssiE2PAvtL12AM03ivuG-";

/// A viewer and a host tunnel endpoint over `rtdb`, keyed by a random shared secret.
pub fn tunnel_pair(rtdb: Arc<dyn Rtdb>, conn_id: &str) -> TunnelPair {
    tunnel_pair_for(rtdb, UID, HOST, conn_id)
}

/// Like [`tunnel_pair`] for any user and host.
pub fn tunnel_pair_for(rtdb: Arc<dyn Rtdb>, uid: &str, host_id: &str, conn_id: &str) -> TunnelPair {
    use remote_control_client::signaling::{down_path, up_path};
    let shared = [7u8; 32];
    let make_keys = || -> TunnelKeys { derive_tunnel_keys(&shared, conn_id).unwrap() };
    let up = up_path(uid, host_id, conn_id).unwrap();
    let down = down_path(uid, host_id, conn_id).unwrap();
    let (vk, hk) = (make_keys(), make_keys());
    let (viewer, viewer_events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: Arc::clone(&rtdb),
        conn_id: conn_id.into(),
        send_path: up.clone(),
        send_dir: TunnelDir::Up,
        send_key: vk.up,
        recv_path: down.clone(),
        recv_dir: TunnelDir::Down,
        recv_key: vk.down,
        subscribe: true,
    });
    let (host, host_events) = TunnelLink::new(TunnelLinkOptions {
        rtdb: Arc::clone(&rtdb),
        conn_id: conn_id.into(),
        send_path: down.clone(),
        send_dir: TunnelDir::Down,
        send_key: hk.down,
        recv_path: up.clone(),
        recv_dir: TunnelDir::Up,
        recv_key: hk.up,
        subscribe: false,
    });
    let inbox = HostInbox::start(rtdb.as_ref(), uid, host_id);
    inbox.register(conn_id, host.clone());
    TunnelPair {
        viewer,
        viewer_events,
        host,
        host_events,
        inbox,
        up,
        down,
    }
}

/// Drain whatever events are queued right now.
pub fn drain(events: &mut mpsc::UnboundedReceiver<LinkEvent>) -> Vec<LinkEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

/// One end of an in-memory fragment link; the other end receives what this one sends.
pub struct MemEnd {
    to_peer: mpsc::UnboundedSender<LinkEvent>,
    to_self: mpsc::UnboundedSender<LinkEvent>,
    closed: std::sync::atomic::AtomicBool,
    pub sent: Arc<Mutex<Vec<Vec<u8>>>>,
    pub max_fragment: usize,
}

#[async_trait]
impl remote_control_client::transport::FragmentLink for MemEnd {
    fn max_fragment_bytes(&self) -> usize {
        self.max_fragment
    }
    async fn send_fragment(
        &self,
        fragment: Vec<u8>,
    ) -> Result<(), remote_control_client::transport::LinkClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(remote_control_client::transport::LinkClosed(
                "closed".into(),
            ));
        }
        self.sent.lock().unwrap().push(fragment.clone());
        let _ = self.to_peer.send(LinkEvent::Fragment(fragment));
        Ok(())
    }
    fn close(&self, reason: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.to_self.send(LinkEvent::Closed(reason.to_owned()));
        let _ = self
            .to_peer
            .send(LinkEvent::Closed("peer_closed".to_owned()));
    }
}

/// Two connected links with small fragments (to exercise reassembly).
pub fn link_pair(
    max_fragment: usize,
) -> (
    remote_control_client::transport::Link,
    remote_control_client::transport::Link,
) {
    let (a, b, _, _) = link_pair_logged(max_fragment);
    (a, b)
}

pub type SentLog = Arc<Mutex<Vec<Vec<u8>>>>;

/// Like [`link_pair`], also returning what each end sent.
pub fn link_pair_logged(
    max_fragment: usize,
) -> (
    remote_control_client::transport::Link,
    remote_control_client::transport::Link,
    SentLog,
    SentLog,
) {
    let (a_events_tx, a_events) = mpsc::unbounded_channel();
    let (b_events_tx, b_events) = mpsc::unbounded_channel();
    let mk = |to_peer: mpsc::UnboundedSender<LinkEvent>,
              to_self: mpsc::UnboundedSender<LinkEvent>| {
        Arc::new(MemEnd {
            to_peer,
            to_self,
            closed: std::sync::atomic::AtomicBool::new(false),
            sent: Arc::default(),
            max_fragment,
        })
    };
    let a = mk(b_events_tx.clone(), a_events_tx.clone());
    let b = mk(a_events_tx, b_events_tx);
    let (log_a, log_b) = (Arc::clone(&a.sent), Arc::clone(&b.sent));
    (
        remote_control_client::transport::Link {
            link: a,
            events: a_events,
        },
        remote_control_client::transport::Link {
            link: b,
            events: b_events,
        },
        log_a,
        log_b,
    )
}
