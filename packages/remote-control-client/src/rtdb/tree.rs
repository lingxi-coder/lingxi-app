//! Immutable in-memory JSON trees with Realtime Database semantics, shared by
//! [`RtdbMirror`](super::RtdbMirror) and [`MemoryRtdb`](super::MemoryRtdb).
//!
//! RTDB never stores `null` or an empty object: writing `null` deletes, and a
//! parent left without children disappears. Arrays do not exist either; they are
//! objects keyed by index. A node is therefore a leaf (string, number, boolean)
//! or a non-empty map. Updates copy only the path they touch, so a "before" tree
//! stays valid and unchanged subtrees are shared.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Number, Value};

use crate::error::ProtocolError;

/// A leaf value, or a non-empty set of children.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A string leaf.
    Str(Arc<str>),
    /// A number leaf.
    Num(Number),
    /// A boolean leaf.
    Bool(bool),
    /// Children by key; never empty.
    Map(Arc<BTreeMap<String, Node>>),
}

/// RTDB itself caps nesting at 32 levels.
const MAX_DEPTH: usize = 64;

/// Split an RTDB path (`/a/b`, `a/b`, `/`) into segments.
pub fn split_path(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Build a tree from wire JSON. `null` and containers without any child become
/// `None`; arrays become index-keyed maps with `null` holes dropped.
pub fn to_tree(value: &Value) -> Result<Option<Node>, ProtocolError> {
    build(value, 0)
}

fn build(value: &Value, depth: usize) -> Result<Option<Node>, ProtocolError> {
    Ok(match value {
        Value::Null => None,
        Value::String(text) => Some(Node::Str(Arc::from(text.as_str()))),
        Value::Number(number) => Some(Node::Num(number.clone())),
        Value::Bool(flag) => Some(Node::Bool(*flag)),
        Value::Array(items) => {
            if depth >= MAX_DEPTH {
                return Err(ProtocolError::new("RTDB value is nested too deeply"));
            }
            let mut map = BTreeMap::new();
            for (index, child) in items.iter().enumerate() {
                if let Some(node) = build(child, depth + 1)? {
                    map.insert(index.to_string(), node);
                }
            }
            (!map.is_empty()).then(|| Node::Map(Arc::new(map)))
        }
        Value::Object(object) => {
            if depth >= MAX_DEPTH {
                return Err(ProtocolError::new("RTDB value is nested too deeply"));
            }
            let mut map = BTreeMap::new();
            for (key, child) in object {
                if let Some(node) = build(child, depth + 1)? {
                    map.insert(key.clone(), node);
                }
            }
            (!map.is_empty()).then(|| Node::Map(Arc::new(map)))
        }
    })
}

/// Plain JSON for a node (objects only, never arrays); `None` becomes `null`.
pub fn from_tree(node: Option<&Node>) -> Value {
    match node {
        None => Value::Null,
        Some(Node::Str(text)) => Value::String(text.to_string()),
        Some(Node::Num(number)) => Value::Number(number.clone()),
        Some(Node::Bool(flag)) => Value::Bool(*flag),
        Some(Node::Map(map)) => {
            let mut object = Map::new();
            for (key, child) in map.iter() {
                object.insert(key.clone(), from_tree(Some(child)));
            }
            Value::Object(object)
        }
    }
}

/// The node at `segments`, or `None`.
pub fn get_node<'a>(root: Option<&'a Node>, segments: &[String]) -> Option<&'a Node> {
    let mut node = root?;
    for segment in segments {
        match node {
            Node::Map(map) => node = map.get(segment)?,
            _ => return None,
        }
    }
    Some(node)
}

/// The tree after replacing the node at `segments` with `value` (`None` deletes
/// it). Writing below a leaf replaces the leaf, and containers left empty are
/// pruned, exactly as RTDB does.
pub fn set_node(root: Option<&Node>, segments: &[String], value: Option<Node>) -> Option<Node> {
    let Some((key, rest)) = segments.split_first() else {
        return value;
    };
    let mut map = match root {
        Some(Node::Map(existing)) => (**existing).clone(),
        _ => BTreeMap::new(),
    };
    let existing_child = match root {
        Some(Node::Map(existing)) => existing.get(key),
        _ => None,
    };
    match set_node(existing_child, rest, value) {
        Some(child) => {
            map.insert(key.clone(), child);
        }
        None => {
            map.remove(key);
        }
    }
    (!map.is_empty()).then(|| Node::Map(Arc::new(map)))
}

/// Deep equality of two optional nodes.
pub fn tree_equals(a: Option<&Node>, b: Option<&Node>) -> bool {
    a == b
}
