//! Message fragmentation. An RPC message is one UTF-8 JSON text; it travels as
//! fragments `[kind][payload]` of at most 16 KiB each (one DataChannel message,
//! or one entry of a tunnel frame). kind `0` is a whole message, `1` means more
//! follows, `2` is the last piece.
//!
//! Fragments cut the UTF-8 bytes anywhere, even inside a multi-byte character;
//! decoding happens once, after reassembly.

use crate::error::ProtocolError;
use crate::version::{FRAGMENT_MAX_BYTES, MESSAGE_MAX_BYTES};

/// Kind byte of a whole message.
pub const KIND_WHOLE: u8 = 0;
/// Kind byte of a fragment with more to follow.
pub const KIND_MORE: u8 = 1;
/// Kind byte of the last fragment of a split message.
pub const KIND_LAST: u8 = 2;

/// Split `text` into fragments, each at most `max_fragment_bytes` (default
/// [`FRAGMENT_MAX_BYTES`]) including the kind byte. The empty message is one
/// fragment, `[0]`. Fails above [`MESSAGE_MAX_BYTES`].
pub fn encode_message(
    text: &str,
    max_fragment_bytes: usize,
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if !(2..=FRAGMENT_MAX_BYTES).contains(&max_fragment_bytes) {
        return Err(ProtocolError::new(
            "max_fragment_bytes must be from 2 to 16384",
        ));
    }
    let bytes = text.as_bytes();
    if bytes.len() > MESSAGE_MAX_BYTES {
        return Err(ProtocolError::new("message exceeds the size limit"));
    }
    let payload_max = max_fragment_bytes - 1;
    if bytes.len() <= payload_max {
        return Ok(vec![fragment(KIND_WHOLE, bytes)]);
    }
    let mut fragments = Vec::with_capacity(bytes.len() / payload_max + 1);
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + payload_max).min(bytes.len());
        let kind = if end == bytes.len() {
            KIND_LAST
        } else {
            KIND_MORE
        };
        fragments.push(fragment(kind, &bytes[offset..end]));
        offset = end;
    }
    Ok(fragments)
}

fn fragment(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// Stateful inverse of [`encode_message`]: push fragments in arrival order,
/// receive each completed message. Any violation returns [`ProtocolError`] (and
/// the connection should be closed with `protocol_error`): an empty or over-long
/// fragment, an unknown kind, a continuation without a start, a whole message in
/// the middle of a split one, a message above [`MESSAGE_MAX_BYTES`], or invalid UTF-8.
#[derive(Debug, Default)]
pub struct Reassembler {
    buffer: Vec<u8>,
    started: bool,
}

impl Reassembler {
    /// An empty reassembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one fragment; returns the message it completes, else `None`.
    pub fn push(&mut self, input: &[u8]) -> Result<Option<String>, ProtocolError> {
        let result = self.accept(input);
        if result.is_err() {
            self.reset();
        }
        result
    }

    /// Bytes of a partly received message.
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }

    /// Drop any partly received message.
    pub fn reset(&mut self) {
        self.buffer = Vec::new();
        self.started = false;
    }

    fn accept(&mut self, input: &[u8]) -> Result<Option<String>, ProtocolError> {
        let (&kind, payload) = input
            .split_first()
            .ok_or_else(|| ProtocolError::new("empty fragment"))?;
        if input.len() > FRAGMENT_MAX_BYTES {
            return Err(ProtocolError::new("fragment exceeds the size limit"));
        }
        match kind {
            KIND_WHOLE => {
                if self.started {
                    return Err(ProtocolError::new("whole message inside a split message"));
                }
                utf8(payload.to_vec()).map(Some)
            }
            KIND_MORE | KIND_LAST => {
                if kind == KIND_LAST && !self.started {
                    return Err(ProtocolError::new("continuation without a start"));
                }
                if self.buffer.len() + payload.len() > MESSAGE_MAX_BYTES {
                    return Err(ProtocolError::new("message exceeds the size limit"));
                }
                self.started = true;
                self.buffer.extend_from_slice(payload);
                if kind == KIND_MORE {
                    return Ok(None);
                }
                let bytes = std::mem::take(&mut self.buffer);
                self.started = false;
                utf8(bytes).map(Some)
            }
            _ => Err(ProtocolError::new("unknown fragment kind")),
        }
    }
}

fn utf8(bytes: Vec<u8>) -> Result<String, ProtocolError> {
    String::from_utf8(bytes).map_err(|_| ProtocolError::new("invalid UTF-8"))
}
