//! The RTDB tunnel: a [`FragmentLink`] that carries fragments as AES-GCM-sealed
//! frames written under `.../up/f{seq}` (viewer to host) or `.../down/f{seq}`
//! (host to viewer) and deleted by the reader once processed. It is the fallback
//! when no direct WebRTC connection can be made.
//!
//! Liveness pings are not part of this type; they are RPC notifications.
//!
//! **Writing.** Fragments queue up and are packed greedily into frames of at most
//! 49136 plaintext bytes. A frame is written when it is full or 25 ms after its
//! first fragment, as `PUT {send_path}/f{seq}` with `seq` counting from 0, at most
//! four writes in flight. A failed write is retried with backoff and ends the link
//! (`tunnel_write`) when retries run out.
//!
//! **Reading.** Frames are decrypted on arrival, reordered by `seq` and delivered
//! in order; a frame below the expected `seq` (the repeat after an SSE reconnect)
//! is ignored. Processed frames are deleted with one PATCH every 500 ms or 32
//! frames. A missing frame that stays missing for 30 s ends the link
//! (`tunnel_gap`); a frame that fails authentication ends it (`tunnel_crypto`).
//!
//! **Closing.** `close(reason)` stops reading at once, flushes what is queued,
//! writes the close notice (one more frame, sealed like any other, whose plaintext
//! holds no fragments) and then deletes its own `send_path` subtree (best effort).
//! The notice is skipped when the link cannot write any more, and when the close
//! was itself the answer to the peer's notice. The reader closes with
//! `peer_closed` once it has processed every frame before the notice.
//! [`TunnelLink::when_closed`] resolves when the cleanup is done. A peer that
//! vanishes without a notice is caught by the connection-level pings or the gap
//! timeout.
//!
//! Reasons the link closes with by itself: `peer_closed`, `tunnel_crypto`,
//! `protocol_error`, `tunnel_gap`, `tunnel_overflow`, `tunnel_write`,
//! `tunnel_listen` and `tunnel_exhausted`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::rtdb::tree::split_path;
use crate::rtdb::{
    child_entries, ListenControl, ListenItem, Listener, Rtdb, RtdbError, RtdbEvent, RtdbEventKind,
};
use crate::signaling::{frame_key, parse_frame_key, MAX_FRAME_SEQ};
use crate::transport::{FragmentLink, Link, LinkClosed, LinkEvent};
use crate::tunnel_crypto::{
    open_frame, pack_fragments, seal_frame, unpack_fragments, TunnelDir, TunnelKey,
};
use crate::version::{
    FRAGMENT_MAX_BYTES, TUNNEL_DELETE_BATCH, TUNNEL_DELETE_INTERVAL, TUNNEL_FLUSH_DELAY,
    TUNNEL_FRAGMENT_MAX_BYTES, TUNNEL_FRAME_PLAINTEXT_MAX, TUNNEL_GAP_TIMEOUT,
    TUNNEL_MAX_INFLIGHT_WRITES, TUNNEL_WRITE_HIGH_WATER_BYTES,
};

/// Options of [`TunnelLink`].
pub struct TunnelLinkOptions {
    /// The database.
    pub rtdb: Arc<dyn Rtdb>,
    /// The connection id (salt of the keys, part of the AAD).
    pub conn_id: String,
    /// Where this side writes frames: `.../up` for the viewer, `.../down` for the host.
    pub send_path: String,
    /// Direction of the frames this side writes.
    pub send_dir: TunnelDir,
    /// `keys.up` for the viewer, `keys.down` for the host.
    pub send_key: TunnelKey,
    /// Where the peer's frames appear: `.../down` for the viewer, `.../up` for the host.
    pub recv_path: String,
    /// Direction of the frames the peer writes.
    pub recv_dir: TunnelDir,
    /// The key that opens the peer's frames.
    pub recv_key: TunnelKey,
    /// `true` (viewer): listen on `recv_path` here. `false` (host): the owner has
    /// one stream on `to_host/{host_id}` for every connection and feeds the frames
    /// in through [`TunnelLink::deliver_frame`].
    pub subscribe: bool,
}

/// Frames that may wait for a missing predecessor at once.
const MAX_BUFFERED_FRAMES: usize = 1024;
/// Writes are retried this many times in all, backing off from 200 ms.
const WRITE_ATTEMPTS: u32 = 5;
const WRITE_RETRY_BASE: Duration = Duration::from_millis(200);
/// `close()` waits at most this long for queued frames to reach RTDB.
const CLOSE_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// The reason a link closes with when the peer's close notice arrives.
const PEER_CLOSED: &str = "peer_closed";
/// A failing delete batch is given up on after this many consecutive failures.
const DELETE_FAILURES_MAX: u32 = 3;

struct OutFrame {
    seq: u64,
    fragments: Vec<Vec<u8>>,
    bytes: usize,
}

#[derive(Default)]
struct State {
    closed: bool,
    close_reason: Option<String>,
    // writer
    send_seq: u64,
    pending: Vec<Vec<u8>>,
    pending_bytes: usize,
    outbox: VecDeque<OutFrame>,
    in_flight: usize,
    unsent: usize,
    flush_timer: Option<JoinHandle<()>>,
    abort_writes: bool,
    // reader
    recv_next: u64,
    ready: BTreeMap<u64, Vec<Vec<u8>>>,
    gap_timer: Option<JoinHandle<()>>,
    to_delete: Vec<String>,
    delete_timer: Option<JoinHandle<()>>,
    deleting: bool,
    delete_failures: u32,
}

struct Inner {
    rtdb: Arc<dyn Rtdb>,
    conn_id: String,
    send_path: String,
    send_dir: TunnelDir,
    send_key: TunnelKey,
    recv_path: String,
    recv_dir: TunnelDir,
    recv_key: TunnelKey,
    events: mpsc::UnboundedSender<LinkEvent>,
    state: Mutex<State>,
    /// Signalled when the writer can take more, or the link closed.
    writable: Notify,
    /// Signalled when queued frames have been written (or given up on).
    drained: Notify,
    settled: watch::Sender<bool>,
    listen: OnceLock<Arc<dyn ListenControl>>,
}

/// One connection's tunnel endpoint. Cheap to clone.
#[derive(Clone)]
pub struct TunnelLink {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TunnelLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelLink")
            .field("conn_id", &self.inner.conn_id)
            .finish_non_exhaustive()
    }
}

fn retryable(error: &RtdbError) -> bool {
    error.status == 0 || error.status == 429 || error.status >= 500
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("tunnel state")
    }

    // -----------------------------------------------------------------------
    // Closing
    // -----------------------------------------------------------------------

    fn close_locked(self: &Arc<Self>, st: &mut State, reason: &str) {
        if st.closed {
            return;
        }
        st.closed = true;
        st.close_reason = Some(reason.to_owned());
        if let Some(listen) = self.listen.get() {
            listen.close();
        }
        if let Some(timer) = st.gap_timer.take() {
            timer.abort();
        }
        st.ready.clear();
        self.seal_pending(st);
        if reason != PEER_CLOSED {
            self.queue_close_notice(st);
        }
        self.writable.notify_waiters();
        let _ = self.events.send(LinkEvent::Closed(reason.to_owned()));
        self.settled.send_replace(false);
        tokio::spawn(Arc::clone(self).finish_close());
    }

    /// After `close`: let queued frames land, drop received-frame bookkeeping,
    /// remove our subtree.
    async fn finish_close(self: Arc<Self>) {
        let deadline = Instant::now() + CLOSE_FLUSH_TIMEOUT;
        self.wait_until(&self.drained, Some(deadline), |st| {
            st.in_flight == 0 && (st.outbox.is_empty() || st.abort_writes)
        })
        .await;
        {
            let mut st = self.lock();
            if let Some(timer) = st.delete_timer.take() {
                timer.abort();
            }
        }
        Arc::clone(&self).flush_deletes().await;
        // Best effort: the host also deletes the whole connection node when it ends.
        let _ = self.rtdb.remove(&self.send_path).await;
        self.settled.send_replace(true);
    }

    /// Wait until `condition` holds (true) or the deadline passes (false).
    async fn wait_until(
        &self,
        notify: &Notify,
        deadline: Option<Instant>,
        condition: impl Fn(&State) -> bool,
    ) -> bool {
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if condition(&self.lock()) {
                return true;
            }
            match deadline {
                None => notified.await,
                Some(deadline) => {
                    tokio::select! {
                        () = &mut notified => {}
                        () = tokio::time::sleep_until(deadline) => return condition(&self.lock()),
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Outgoing frames
    // -----------------------------------------------------------------------

    /// Close the frame being filled and queue it for writing.
    fn seal_pending(self: &Arc<Self>, st: &mut State) {
        if let Some(timer) = st.flush_timer.take() {
            timer.abort();
        }
        if st.pending.is_empty() {
            return;
        }
        if st.send_seq > MAX_FRAME_SEQ {
            st.abort_writes = true;
            self.close_locked(st, "tunnel_exhausted");
            return;
        }
        let fragments = std::mem::take(&mut st.pending);
        st.pending_bytes = 0;
        let bytes = fragments.iter().map(Vec::len).sum();
        let seq = st.send_seq;
        st.send_seq += 1;
        st.outbox.push_back(OutFrame {
            seq,
            fragments,
            bytes,
        });
        self.pump(st);
    }

    /// Queue the close notice: a frame with no fragments, after everything written so far.
    fn queue_close_notice(self: &Arc<Self>, st: &mut State) {
        if st.send_seq > MAX_FRAME_SEQ {
            return;
        }
        let seq = st.send_seq;
        st.send_seq += 1;
        st.outbox.push_back(OutFrame {
            seq,
            fragments: Vec::new(),
            bytes: 0,
        });
        self.pump(st);
    }

    fn pump(self: &Arc<Self>, st: &mut State) {
        while !st.abort_writes && st.in_flight < TUNNEL_MAX_INFLIGHT_WRITES {
            let Some(frame) = st.outbox.pop_front() else {
                break;
            };
            st.in_flight += 1;
            tokio::spawn(Arc::clone(self).write_frame(frame));
        }
        self.check_drained(st);
    }

    fn check_drained(&self, st: &State) {
        if st.in_flight == 0 && (st.outbox.is_empty() || st.abort_writes) {
            self.drained.notify_waiters();
        }
    }

    async fn write_frame(self: Arc<Self>, frame: OutFrame) {
        if self.put_frame(&frame).await.is_err() {
            let mut st = self.lock();
            st.abort_writes = true;
            self.close_locked(&mut st, "tunnel_write");
        }
        let mut st = self.lock();
        st.in_flight -= 1;
        st.unsent = st.unsent.saturating_sub(frame.bytes);
        if !st.closed && st.unsent >= TUNNEL_WRITE_HIGH_WATER_BYTES {
            // still above the high-water mark: writers keep waiting
        } else {
            self.writable.notify_waiters();
        }
        self.pump(&mut st);
    }

    async fn put_frame(&self, frame: &OutFrame) -> Result<(), ()> {
        let plaintext = pack_fragments(&frame.fragments).map_err(drop)?;
        let stored = seal_frame(
            &self.send_key,
            &self.conn_id,
            self.send_dir,
            frame.seq,
            &plaintext,
        )
        .map_err(drop)?;
        let path = format!("{}/{}", self.send_path, frame_key(frame.seq).map_err(drop)?);
        let value = Value::String(stored);
        for attempt in 0..WRITE_ATTEMPTS {
            match self.rtdb.put(&path, &value).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if attempt + 1 >= WRITE_ATTEMPTS
                        || !retryable(&error)
                        || self.lock().abort_writes
                    {
                        return Err(());
                    }
                    tokio::time::sleep(WRITE_RETRY_BASE * 2u32.pow(attempt)).await;
                    if self.lock().abort_writes {
                        return Err(());
                    }
                }
            }
        }
        Err(())
    }

    // -----------------------------------------------------------------------
    // Incoming frames
    // -----------------------------------------------------------------------

    fn deliver_frame(self: &Arc<Self>, seq: u64, value: &Value) {
        let Value::String(stored) = value else {
            return;
        };
        let mut st = self.lock();
        if st.closed || seq < st.recv_next || seq > MAX_FRAME_SEQ || st.ready.contains_key(&seq) {
            return;
        }
        if st.ready.len() >= MAX_BUFFERED_FRAMES {
            self.close_locked(&mut st, "tunnel_overflow");
            return;
        }
        let opened = open_frame(&self.recv_key, &self.conn_id, self.recv_dir, seq, stored);
        let plaintext = match opened {
            Ok(plaintext) => plaintext,
            Err(_) => {
                self.close_locked(&mut st, "tunnel_crypto");
                return;
            }
        };
        let fragments = match unpack_fragments(&plaintext) {
            Ok(fragments) => fragments,
            Err(_) => {
                self.close_locked(&mut st, "protocol_error");
                return;
            }
        };
        st.ready.insert(seq, fragments);
        self.drain(&mut st);
    }

    fn deliver_key(self: &Arc<Self>, key: &str, value: &Value) {
        if let Some(seq) = parse_frame_key(key) {
            self.deliver_frame(seq, value);
        }
    }

    fn on_rtdb_event(self: &Arc<Self>, event: &RtdbEvent) {
        if self.lock().closed {
            return;
        }
        let path = split_path(&event.path);
        match path.as_slice() {
            [] => {
                for (key, value) in child_entries(&event.data) {
                    self.deliver_key(&key, value);
                }
            }
            [key] if event.kind == RtdbEventKind::Put => self.deliver_key(key, &event.data),
            _ => {}
        }
    }

    /// Deliver every frame that is now next in line.
    fn drain(self: &Arc<Self>, st: &mut State) {
        while !st.closed {
            let next = st.recv_next;
            let Some(fragments) = st.ready.remove(&next) else {
                break;
            };
            st.recv_next += 1;
            self.processed(st, next);
            if fragments.is_empty() {
                // A frame without fragments is the peer's close notice.
                self.close_locked(st, PEER_CLOSED);
                break;
            }
            for fragment in fragments {
                if st.closed {
                    break;
                }
                let _ = self.events.send(LinkEvent::Fragment(fragment));
            }
        }
        self.update_gap_timer(st);
    }

    /// The 30 s clock on a missing frame: armed when later frames are waiting for
    /// one that has not arrived, left running while more frames pile up behind it,
    /// and stopped as soon as it shows up. A second hole behind it gets a fresh
    /// 30 s, since the timer was stopped when the first one was filled.
    fn update_gap_timer(self: &Arc<Self>, st: &mut State) {
        let waiting = !st.ready.is_empty();
        let arrived = st.ready.contains_key(&st.recv_next);
        if !waiting || arrived {
            if let Some(timer) = st.gap_timer.take() {
                timer.abort();
            }
        } else if st.gap_timer.is_none() {
            let inner = Arc::clone(self);
            st.gap_timer = Some(tokio::spawn(async move {
                tokio::time::sleep(TUNNEL_GAP_TIMEOUT).await;
                let mut st = inner.lock();
                st.gap_timer = None;
                if !st.closed {
                    inner.close_locked(&mut st, "tunnel_gap");
                }
            }));
        }
    }

    // -----------------------------------------------------------------------
    // Deleting processed frames
    // -----------------------------------------------------------------------

    fn processed(self: &Arc<Self>, st: &mut State, seq: u64) {
        if let Ok(key) = frame_key(seq) {
            st.to_delete.push(key);
        }
        self.schedule_deletes(st);
    }

    fn schedule_deletes(self: &Arc<Self>, st: &mut State) {
        if st.to_delete.len() >= TUNNEL_DELETE_BATCH {
            tokio::spawn(Arc::clone(self).flush_deletes());
        } else if !st.to_delete.is_empty() && st.delete_timer.is_none() {
            let inner = Arc::clone(self);
            st.delete_timer = Some(tokio::spawn(async move {
                tokio::time::sleep(TUNNEL_DELETE_INTERVAL).await;
                inner.lock().delete_timer = None;
                inner.flush_deletes().await;
            }));
        }
    }

    async fn flush_deletes(self: Arc<Self>) {
        let keys = {
            let mut st = self.lock();
            if let Some(timer) = st.delete_timer.take() {
                timer.abort();
            }
            if st.deleting || st.to_delete.is_empty() {
                return;
            }
            st.deleting = true;
            let take = st.to_delete.len().min(256);
            st.to_delete.drain(..take).collect::<Vec<_>>()
        };
        let patch: Map<String, Value> = keys.iter().map(|k| (k.clone(), Value::Null)).collect();
        let outcome = self.rtdb.patch(&self.recv_path, &patch).await;
        let mut st = self.lock();
        st.deleting = false;
        match outcome {
            Ok(()) => st.delete_failures = 0,
            Err(_) => {
                // Deleting is housekeeping: try again with the next batch, then give up.
                st.delete_failures += 1;
                if st.delete_failures < DELETE_FAILURES_MAX {
                    st.to_delete.splice(0..0, keys);
                }
            }
        }
        if !st.closed {
            self.schedule_deletes(&mut st);
        }
    }
}

impl TunnelLink {
    /// Create the endpoint. Must be called inside a Tokio runtime. The receiver
    /// carries the peer's fragments and the final close.
    pub fn new(options: TunnelLinkOptions) -> (Self, mpsc::UnboundedReceiver<LinkEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (settled, _) = watch::channel(true);
        let inner = Arc::new(Inner {
            rtdb: options.rtdb,
            conn_id: options.conn_id,
            send_path: options.send_path,
            send_dir: options.send_dir,
            send_key: options.send_key,
            recv_path: options.recv_path,
            recv_dir: options.recv_dir,
            recv_key: options.recv_key,
            events,
            state: Mutex::new(State::default()),
            writable: Notify::new(),
            drained: Notify::new(),
            settled,
            listen: OnceLock::new(),
        });
        if options.subscribe {
            let listener = inner.rtdb.listen(&inner.recv_path);
            let _ = inner.listen.set(listener.control());
            tokio::spawn(listen_loop(Arc::clone(&inner), listener));
        }
        (Self { inner }, receiver)
    }

    /// The link and its events as a [`Link`], ready for [`RpcPeer::start`](crate::rpc::RpcPeer::start).
    pub fn into_link(self, events: mpsc::UnboundedReceiver<LinkEvent>) -> Link {
        Link {
            link: Arc::new(self),
            events,
        }
    }

    /// Hand in the value stored at `{recv_path}/f{seq}`. For links created with
    /// `subscribe: false`; harmless (and ignored) for deletions, duplicates and
    /// anything that is not a string.
    pub fn deliver_frame(&self, seq: u64, value: &Value) {
        self.inner.deliver_frame(seq, value);
    }

    /// Resolves once `close` has finished flushing and deleting; immediately if never closed.
    pub async fn when_closed(&self) {
        let mut receiver = self.inner.settled.subscribe();
        let _ = receiver.wait_for(|settled| *settled).await;
    }

    /// Why the link closed, if it has.
    pub fn close_reason(&self) -> Option<String> {
        self.inner.lock().close_reason.clone()
    }
}

async fn listen_loop(inner: Arc<Inner>, mut listener: Listener) {
    while let Some(item) = listener.recv().await {
        match item {
            ListenItem::Event(event) => inner.on_rtdb_event(&event),
            ListenItem::Error(_) => {
                let mut st = inner.lock();
                inner.close_locked(&mut st, "tunnel_listen");
                return;
            }
        }
    }
}

#[async_trait]
impl FragmentLink for TunnelLink {
    fn max_fragment_bytes(&self) -> usize {
        TUNNEL_FRAGMENT_MAX_BYTES
    }

    async fn send_fragment(&self, fragment: Vec<u8>) -> Result<(), LinkClosed> {
        let inner = &self.inner;
        if fragment.is_empty() || fragment.len() > FRAGMENT_MAX_BYTES {
            return Err(LinkClosed("fragment size out of range".into()));
        }
        let cost = 4 + fragment.len();
        loop {
            let notified = inner.writable.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = inner.lock();
                if st.closed {
                    return Err(LinkClosed(st.close_reason.clone().unwrap_or_default()));
                }
                if st.unsent < TUNNEL_WRITE_HIGH_WATER_BYTES {
                    if st.pending_bytes + cost > TUNNEL_FRAME_PLAINTEXT_MAX {
                        inner.seal_pending(&mut st);
                    }
                    st.unsent += fragment.len();
                    st.pending_bytes += cost;
                    st.pending.push(fragment);
                    if TUNNEL_FRAME_PLAINTEXT_MAX - st.pending_bytes < 5 {
                        // no room left for even a one-byte fragment
                        inner.seal_pending(&mut st);
                    } else if st.flush_timer.is_none() {
                        let flusher = Arc::clone(inner);
                        st.flush_timer = Some(tokio::spawn(async move {
                            tokio::time::sleep(TUNNEL_FLUSH_DELAY).await;
                            let mut st = flusher.lock();
                            st.flush_timer = None;
                            flusher.seal_pending(&mut st);
                        }));
                    }
                    return Ok(());
                }
            }
            notified.await;
        }
    }

    fn close(&self, reason: &str) {
        let mut st = self.inner.lock();
        self.inner.close_locked(&mut st, reason);
    }
}
