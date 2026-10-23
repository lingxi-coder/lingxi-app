//! [`RtdbMirror`]: the in-memory copy of a listened subtree that turns RTDB
//! stream events into "what changed" answers.
//!
//! A consumer such as the host dispatcher listens once on `to_host/{host_id}` and
//! asks, for each event, which `(conn_id, "offer")` or `(conn_id, "up", "f12")`
//! entries appeared. Because changes are computed by diffing the tree before and
//! after the event, a reconnect's repeated full-subtree `put` reports only what
//! is genuinely new or gone.

use std::collections::HashSet;

use serde_json::Value;

use super::tree::{from_tree, get_node, set_node, split_path, to_tree, tree_equals, Node};
use super::{RtdbEvent, RtdbEventKind};

/// How an entry differs before and after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The entry did not exist before.
    Added,
    /// The entry exists before and after with different content.
    Changed,
    /// The entry is gone.
    Removed,
}

/// One entry that differs before and after an event.
#[derive(Debug, Clone, PartialEq)]
pub struct RtdbChange {
    /// Segments below the mirrored root, e.g. `[conn_id, "up", "f3"]`.
    pub path: Vec<String>,
    /// How it changed.
    pub kind: ChangeKind,
    /// The new value as plain JSON; `null` when removed.
    pub value: Value,
    /// The old value as plain JSON; `null` when added.
    pub previous: Value,
}

/// What one [`RtdbMirror::apply`] did, queryable at any depth.
#[derive(Debug, Clone)]
pub struct RtdbChanges {
    before: Option<Node>,
    after: Option<Node>,
    touched: Vec<Vec<String>>,
}

impl RtdbChanges {
    /// `true` when the event left the tree unchanged.
    pub fn is_empty(&self) -> bool {
        tree_equals(self.before.as_ref(), self.after.as_ref())
    }

    /// The entries exactly `depth` segments below the root that differ before and
    /// after the event, each once, including entries removed along with a parent.
    /// Depth 1 on `to_host/{host_id}` is the connection ids; depth 2 is
    /// `offer`/`up`; depth 3 under `up` is the frames.
    pub fn at(&self, depth: usize) -> Vec<RtdbChange> {
        assert!(depth >= 1, "depth must be a positive integer");
        let mut seen: HashSet<Vec<String>> = HashSet::new();
        let mut candidates: Vec<Vec<String>> = Vec::new();
        let mut add = |path: Vec<String>, candidates: &mut Vec<Vec<String>>| {
            if seen.insert(path.clone()) {
                candidates.push(path);
            }
        };
        fn collect(
            node: Option<&Node>,
            prefix: Vec<String>,
            depth: usize,
            out: &mut Vec<Vec<String>>,
        ) {
            if prefix.len() == depth {
                out.push(prefix);
                return;
            }
            if let Some(Node::Map(map)) = node {
                for (key, child) in map.iter() {
                    let mut next = prefix.clone();
                    next.push(key.clone());
                    collect(Some(child), next, depth, out);
                }
            }
        }
        for touched in &self.touched {
            if touched.len() >= depth {
                add(touched[..depth].to_vec(), &mut candidates);
            } else {
                let mut found = Vec::new();
                collect(
                    get_node(self.before.as_ref(), touched),
                    touched.clone(),
                    depth,
                    &mut found,
                );
                collect(
                    get_node(self.after.as_ref(), touched),
                    touched.clone(),
                    depth,
                    &mut found,
                );
                for path in found {
                    add(path, &mut candidates);
                }
            }
        }
        let mut changes = Vec::new();
        for path in candidates {
            let previous = get_node(self.before.as_ref(), &path);
            let value = get_node(self.after.as_ref(), &path);
            if tree_equals(previous, value) {
                continue;
            }
            let kind = match (previous, value) {
                (None, _) => ChangeKind::Added,
                (_, None) => ChangeKind::Removed,
                _ => ChangeKind::Changed,
            };
            changes.push(RtdbChange {
                kind,
                value: from_tree(value),
                previous: from_tree(previous),
                path,
            });
        }
        changes
    }
}

/// Applies `put`/`patch` events (paths relative to the listened location) to an
/// in-memory tree. `null` deletes, a `patch` replaces the listed children and
/// keeps the rest, arrays are normalized to objects, and containers left empty
/// are pruned, mirroring RTDB.
#[derive(Debug, Default)]
pub struct RtdbMirror {
    root: Option<Node>,
}

impl RtdbMirror {
    /// An empty mirror.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one event and report what changed. A malformed event changes nothing.
    pub fn apply(&mut self, event: &RtdbEvent) -> RtdbChanges {
        let before = self.root.clone();
        let mut touched: Vec<Vec<String>> = Vec::new();
        let base = split_path(&event.path);
        let outcome: Result<Option<Option<Node>>, ()> = match event.kind {
            RtdbEventKind::Put => match to_tree(&event.data) {
                Ok(tree) => {
                    touched.push(base.clone());
                    Ok(Some(set_node(before.as_ref(), &base, tree)))
                }
                Err(_) => Err(()),
            },
            RtdbEventKind::Patch => match event.data.as_object() {
                None => Ok(None),
                Some(object) => {
                    let mut root = before.clone();
                    let mut failed = false;
                    for (key, value) in object {
                        let mut segments = base.clone();
                        segments.extend(split_path(key));
                        match to_tree(value) {
                            Ok(tree) => {
                                root = set_node(root.as_ref(), &segments, tree);
                                touched.push(segments);
                            }
                            Err(_) => {
                                failed = true;
                                break;
                            }
                        }
                    }
                    if failed {
                        Err(())
                    } else {
                        Ok(Some(root))
                    }
                }
            },
        };
        match outcome {
            Ok(Some(root)) => self.root = root,
            Ok(None) => {}
            Err(()) => {
                self.root = before.clone();
                touched.clear();
            }
        }
        RtdbChanges {
            before,
            after: self.root.clone(),
            touched,
        }
    }

    /// The value at `path` (use `"/"` for the root) as plain JSON; `null` when absent.
    pub fn get(&self, path: &str) -> Value {
        from_tree(get_node(self.root.as_ref(), &split_path(path)))
    }

    /// Forget everything, e.g. when resubscribing from scratch.
    pub fn clear(&mut self) {
        self.root = None;
    }
}
