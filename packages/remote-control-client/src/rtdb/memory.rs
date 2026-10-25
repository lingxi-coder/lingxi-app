//! [`MemoryRtdb`]: an in-memory [`Rtdb`] that emits the same events as the real
//! streaming API, for tests of hosts and viewers without a network or an emulator.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use super::tree::{from_tree, get_node, set_node, split_path, to_tree, tree_equals, Node};
use super::{
    ListenControl, ListenItem, Listener, Rtdb, RtdbError, RtdbEvent, RtdbEventKind, RtdbListenError,
};
use crate::clock::{system_clock, Clock};

struct ListenerEntry {
    id: u64,
    segments: Vec<String>,
    sender: mpsc::UnboundedSender<ListenItem>,
    closed: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    root: Option<Node>,
    listeners: Vec<ListenerEntry>,
    next_listener: u64,
    fail_writes: VecDeque<RtdbError>,
    fail_reads: VecDeque<RtdbError>,
    write_log: Vec<WriteRecord>,
}

/// One write the database accepted, for assertions.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteRecord {
    /// `PUT`, `PATCH` or `DELETE`.
    pub method: &'static str,
    /// The path written.
    pub path: String,
}

/// Behaves like the Realtime Database for the calls the remote stack makes:
///
/// - `listen` first delivers a `put` at `/` with the current subtree (or `null`),
///   then one event per later change, in order.
/// - `put`/`remove` under a listener produce a `put` at the written relative
///   path; `patch` produces a `patch` at the patched path carrying the changed
///   children; a write above a listener produces a `put` at `/` with the new
///   subtree. A write that changes nothing produces no event.
/// - Server-timestamp placeholders resolve to the injected clock, `null` deletes,
///   and empty objects vanish.
pub struct MemoryRtdb {
    state: Arc<Mutex<State>>,
    clock: Arc<dyn Clock>,
    coerce_arrays: bool,
}

impl Default for MemoryRtdb {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MemoryRtdb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryRtdb").finish_non_exhaustive()
    }
}

impl MemoryRtdb {
    /// A database using the system clock for server timestamps.
    pub fn new() -> Self {
        Self::with_clock(system_clock())
    }

    /// A database that resolves server timestamps with `clock`.
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            clock,
            coerce_arrays: false,
        }
    }

    /// Hand out objects whose keys are integers as arrays, the way RTDB does (every
    /// key a non-negative integer and the largest below twice the number of keys),
    /// holes as `null`. Off by default; turn it on to prove a consumer copes with
    /// array-shaped data.
    pub fn coerce_arrays(mut self, on: bool) -> Self {
        self.coerce_arrays = on;
        self
    }

    /// Number of open listeners (for leak assertions).
    pub fn listener_count(&self) -> usize {
        let mut state = self.state.lock().expect("state");
        state.listeners.retain(|l| !l.closed.load(Ordering::SeqCst));
        state.listeners.len()
    }

    /// The current value at `path` (default: everything) as plain JSON; `null` when absent.
    pub fn read(&self, path: &str) -> Value {
        let state = self.state.lock().expect("state");
        self.view(get_node(state.root.as_ref(), &split_path(path)))
    }

    /// Make the next write fail with this error (queued; each write consumes one).
    pub fn fail_next_write(&self, error: RtdbError) {
        self.state
            .lock()
            .expect("state")
            .fail_writes
            .push_back(error);
    }

    /// Make the next read fail with this error.
    pub fn fail_next_read(&self, error: RtdbError) {
        self.state
            .lock()
            .expect("state")
            .fail_reads
            .push_back(error);
    }

    /// Every write accepted so far, oldest first.
    pub fn writes(&self) -> Vec<WriteRecord> {
        self.state.lock().expect("state").write_log.clone()
    }

    /// Tell every listener the stream ended for good (as the server does with `cancel`).
    pub fn cancel_listeners(&self) {
        let mut state = self.state.lock().expect("state");
        for listener in state.listeners.drain(..) {
            let _ = listener
                .sender
                .send(ListenItem::Error(RtdbListenError::Cancel));
            listener.closed.store(true, Ordering::SeqCst);
        }
    }

    fn view(&self, node: Option<&Node>) -> Value {
        let json = from_tree(node);
        if self.coerce_arrays {
            coerce(json)
        } else {
            json
        }
    }

    /// Replace server-timestamp placeholders with the clock's time, on a copy.
    fn resolve(&self, value: &Value) -> Result<Value, RtdbError> {
        match value {
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|child| self.resolve(child))
                    .collect::<Result<_, _>>()?,
            )),
            Value::Object(object) => {
                if object.contains_key(".sv") {
                    if object.len() == 1
                        && object.get(".sv") == Some(&Value::String("timestamp".into()))
                    {
                        return Ok(Value::from(self.clock.now_ms()));
                    }
                    return Err(RtdbError::new(400, "unsupported server value"));
                }
                let mut out = Map::new();
                for (key, child) in object {
                    out.insert(key.clone(), self.resolve(child)?);
                }
                Ok(Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    fn take_write_failure(&self) -> Option<RtdbError> {
        self.state.lock().expect("state").fail_writes.pop_front()
    }

    fn notify(
        &self,
        state: &State,
        written: &[String],
        patch: Option<&Map<String, Value>>,
        before: Option<&Node>,
    ) {
        for listener in &state.listeners {
            if listener.closed.load(Ordering::SeqCst) {
                continue;
            }
            let own = &listener.segments;
            let below = written.len() >= own.len() && own.iter().zip(written).all(|(a, b)| a == b);
            if below {
                let relative = &written[own.len()..];
                let path = format!("/{}", relative.join("/"));
                match patch {
                    None => {
                        let previous = get_node(before, written);
                        let current = get_node(state.root.as_ref(), written);
                        if !tree_equals(previous, current) {
                            send(
                                listener,
                                RtdbEvent {
                                    kind: RtdbEventKind::Put,
                                    path,
                                    data: self.view(current),
                                },
                            );
                        }
                    }
                    Some(patch) => {
                        let mut changed = Map::new();
                        for key in patch.keys() {
                            let mut child = written.to_vec();
                            child.extend(split_path(key));
                            let previous = get_node(before, &child);
                            let current = get_node(state.root.as_ref(), &child);
                            if !tree_equals(previous, current) {
                                changed.insert(key.clone(), self.view(current));
                            }
                        }
                        if !changed.is_empty() {
                            send(
                                listener,
                                RtdbEvent {
                                    kind: RtdbEventKind::Patch,
                                    path,
                                    data: Value::Object(changed),
                                },
                            );
                        }
                    }
                }
            } else if own.len() > written.len() && written.iter().zip(own).all(|(a, b)| a == b) {
                let previous = get_node(before, own);
                let current = get_node(state.root.as_ref(), own);
                if !tree_equals(previous, current) {
                    send(
                        listener,
                        RtdbEvent {
                            kind: RtdbEventKind::Put,
                            path: "/".into(),
                            data: self.view(current),
                        },
                    );
                }
            }
        }
    }
}

fn send(listener: &ListenerEntry, event: RtdbEvent) {
    let _ = listener.sender.send(ListenItem::Event(event));
}

fn coerce(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let entries: Vec<(String, Value)> =
                object.into_iter().map(|(k, v)| (k, coerce(v))).collect();
            let is_index = |key: &str| {
                key == "0"
                    || (!key.starts_with('0')
                        && !key.is_empty()
                        && key.bytes().all(|b| b.is_ascii_digit()))
            };
            if !entries.is_empty() && entries.iter().all(|(k, _)| is_index(k)) {
                let max = entries
                    .iter()
                    .filter_map(|(k, _)| k.parse::<usize>().ok())
                    .max()
                    .unwrap_or(0);
                if max < 2 * entries.len() {
                    let mut array = vec![Value::Null; max + 1];
                    for (key, child) in entries {
                        array[key.parse::<usize>().expect("index")] = child;
                    }
                    return Value::Array(array);
                }
            }
            Value::Object(entries.into_iter().collect())
        }
        other => other,
    }
}

struct MemoryControl {
    state: Arc<Mutex<State>>,
    id: u64,
    closed: Arc<AtomicBool>,
    path: Vec<String>,
    view: Arc<dyn Fn(Option<&Node>) -> Value + Send + Sync>,
}

impl ListenControl for MemoryControl {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut state = self.state.lock().expect("state");
        state.listeners.retain(|l| l.id != self.id);
    }

    fn reconnect(&self) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        // What a dropped-and-reopened stream looks like: the whole subtree again.
        let state = self.state.lock().expect("state");
        if let Some(listener) = state.listeners.iter().find(|l| l.id == self.id) {
            let data = (self.view)(get_node(state.root.as_ref(), &self.path));
            send(
                listener,
                RtdbEvent {
                    kind: RtdbEventKind::Put,
                    path: "/".into(),
                    data,
                },
            );
        }
    }
}

#[async_trait]
impl Rtdb for MemoryRtdb {
    async fn get(&self, path: &str) -> Result<Value, RtdbError> {
        if let Some(error) = self.state.lock().expect("state").fail_reads.pop_front() {
            return Err(error);
        }
        Ok(self.read(path))
    }

    async fn put(&self, path: &str, value: &Value) -> Result<(), RtdbError> {
        if let Some(error) = self.take_write_failure() {
            return Err(error);
        }
        let segments = split_path(path);
        let resolved = self.resolve(value)?;
        let tree = to_tree(&resolved).map_err(|e| RtdbError::new(400, e.0))?;
        let mut state = self.state.lock().expect("state");
        let before = state.root.clone();
        state.root = set_node(before.as_ref(), &segments, tree);
        state.write_log.push(WriteRecord {
            method: if value.is_null() { "DELETE" } else { "PUT" },
            path: path.to_owned(),
        });
        self.notify(&state, &segments, None, before.as_ref());
        Ok(())
    }

    async fn patch(&self, path: &str, value: &Map<String, Value>) -> Result<(), RtdbError> {
        if let Some(error) = self.take_write_failure() {
            return Err(error);
        }
        let segments = split_path(path);
        let resolved = match self.resolve(&Value::Object(value.clone()))? {
            Value::Object(object) => object,
            _ => unreachable!("an object resolves to an object"),
        };
        let mut state = self.state.lock().expect("state");
        let before = state.root.clone();
        let mut root = before.clone();
        for (key, child) in &resolved {
            let mut target = segments.clone();
            target.extend(split_path(key));
            let tree = to_tree(child).map_err(|e| RtdbError::new(400, e.0))?;
            root = set_node(root.as_ref(), &target, tree);
        }
        state.root = root;
        state.write_log.push(WriteRecord {
            method: "PATCH",
            path: path.to_owned(),
        });
        self.notify(&state, &segments, Some(&resolved), before.as_ref());
        Ok(())
    }

    async fn remove(&self, path: &str) -> Result<(), RtdbError> {
        self.put(path, &Value::Null).await
    }

    fn listen(&self, path: &str) -> Listener {
        let (sender, receiver) = mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        let segments = split_path(path);
        let mut state = self.state.lock().expect("state");
        let id = state.next_listener;
        state.next_listener += 1;
        let entry = ListenerEntry {
            id,
            segments: segments.clone(),
            sender,
            closed: Arc::clone(&closed),
        };
        send(
            &entry,
            RtdbEvent {
                kind: RtdbEventKind::Put,
                path: "/".into(),
                data: self.view(get_node(state.root.as_ref(), &segments)),
            },
        );
        state.listeners.push(entry);
        let coerce_arrays = self.coerce_arrays;
        let control = MemoryControl {
            state: Arc::clone(&self.state),
            id,
            closed,
            path: segments,
            view: Arc::new(move |node| {
                let json = from_tree(node);
                if coerce_arrays {
                    coerce(json)
                } else {
                    json
                }
            }),
        };
        Listener::new(receiver, Arc::new(control))
    }
}
