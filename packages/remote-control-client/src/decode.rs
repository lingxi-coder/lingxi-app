//! Decoding of `ClientEvent`s carried inside records. A host newer than this
//! client may send an event variant (or a variant shape) this client's pinned
//! `client-protocol` does not know; that must not fail the whole record, so it
//! becomes [`DecodedEvent::Unsupported`].

use client::protocol::events::ClientEvent;
use serde::{Serialize, Serializer};
use serde_json::Value;

/// A `ClientEvent` as received, or a placeholder for one this client cannot decode.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedEvent {
    /// A known event.
    Event(ClientEvent),
    /// An event this client's protocol version does not understand.
    Unsupported {
        /// The event's `type` tag (empty when it had none).
        event_type: String,
        /// The event as received.
        raw: Value,
    },
}

impl DecodedEvent {
    /// The known event, if any.
    pub fn known(&self) -> Option<&ClientEvent> {
        match self {
            Self::Event(event) => Some(event),
            Self::Unsupported { .. } => None,
        }
    }
}

impl Serialize for DecodedEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Event(event) => event.serialize(serializer),
            Self::Unsupported { raw, .. } => raw.serialize(serializer),
        }
    }
}

/// Decode a `ClientEvent` from JSON, turning anything undecodable into
/// [`DecodedEvent::Unsupported`] instead of an error.
pub fn decode_client_event(value: Value) -> DecodedEvent {
    match serde_json::from_value::<ClientEvent>(value.clone()) {
        Ok(event) => DecodedEvent::Event(event),
        Err(_) => DecodedEvent::Unsupported {
            event_type: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            raw: value,
        },
    }
}
