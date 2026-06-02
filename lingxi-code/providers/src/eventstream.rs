//! Minimal pure-Rust decoder for the AWS event-stream wire format
//! (`application/vnd.amazon.eventstream`), used by Bedrock's
//! `invoke-with-response-stream`.
//!
//! We hand-roll this instead of depending on `aws-smithy-eventstream`, which
//! floors `aws-smithy-types` at a version requiring rustc 1.88 (this workspace
//! is pinned to Rust 1.82). The format is small and stable:
//!
//! ```text
//! [total_len: u32 BE][headers_len: u32 BE][prelude_crc: u32 BE]
//! [headers: headers_len bytes]
//! [payload: total_len - headers_len - 16 bytes]
//! [message_crc: u32 BE]
//! ```
//!
//! CRCs are not validated — the bytes arrive over TLS, which already guarantees
//! integrity, and a corrupt frame surfaces as a JSON/decode error downstream.
//! Only string-valued headers are retained (the ones Bedrock uses:
//! `:message-type`, `:event-type`, `:content-type`); other header value types
//! are parsed only enough to skip them.

/// A decoded event-stream frame: its string headers and raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// String-valued headers (name → value), e.g. `:event-type` → `chunk`.
    pub headers: Vec<(String, String)>,
    /// The frame payload (for a `chunk` event, a `{"bytes":"<base64>"}` JSON).
    pub payload: Vec<u8>,
}

impl Frame {
    /// First string header matching `name`, if present.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Framing error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

const PRELUDE_LEN: usize = 12; // total_len(4) + headers_len(4) + prelude_crc(4)
const MESSAGE_CRC_LEN: usize = 4;

/// Incremental decoder: feed bytes via [`Self::extend`], pull complete frames
/// via [`Self::next_frame`].
#[derive(Default)]
pub struct EventStreamDecoder {
    buf: Vec<u8>,
}

impl EventStreamDecoder {
    /// New, empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append received bytes to the internal buffer.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Try to decode and consume one complete frame.
    ///
    /// Returns `Ok(None)` when more bytes are needed.
    ///
    /// # Errors
    /// Returns [`DecodeError`] on malformed framing (bad prelude lengths or
    /// truncated headers).
    pub fn next_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.buf.len() < PRELUDE_LEN {
            return Ok(None);
        }
        let total_len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        let headers_len =
            u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]) as usize;
        if total_len < PRELUDE_LEN + MESSAGE_CRC_LEN
            || headers_len > total_len - PRELUDE_LEN - MESSAGE_CRC_LEN
        {
            return Err(DecodeError(format!(
                "bad prelude: total_len={total_len} headers_len={headers_len}"
            )));
        }
        if self.buf.len() < total_len {
            return Ok(None); // wait for the rest of the message
        }
        let headers_start = PRELUDE_LEN;
        let headers_end = PRELUDE_LEN + headers_len;
        let payload_end = total_len - MESSAGE_CRC_LEN;
        let headers = parse_headers(&self.buf[headers_start..headers_end])?;
        let payload = self.buf[headers_end..payload_end].to_vec();
        self.buf.drain(0..total_len);
        Ok(Some(Frame { headers, payload }))
    }
}

fn need(b: &[u8], n: usize) -> Result<(), DecodeError> {
    if b.len() < n {
        Err(DecodeError(format!("truncated header (need {n}, have {})", b.len())))
    } else {
        Ok(())
    }
}

/// Parse the headers section, retaining only string-valued headers.
fn parse_headers(mut b: &[u8]) -> Result<Vec<(String, String)>, DecodeError> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let name_len = b[0] as usize;
        b = &b[1..];
        need(b, name_len)?;
        let name = String::from_utf8_lossy(&b[..name_len]).into_owned();
        b = &b[name_len..];
        need(b, 1)?;
        let vtype = b[0];
        b = &b[1..];
        match vtype {
            0 | 1 => {}                              // bool true/false — no value bytes
            2 => b = skip(b, 1)?,                     // byte
            3 => b = skip(b, 2)?,                     // short
            4 => b = skip(b, 4)?,                     // int32
            5 => b = skip(b, 8)?,                     // int64
            6 | 7 => {
                // byte-array (6) / string (7): u16 length-prefixed.
                need(b, 2)?;
                let len = u16::from_be_bytes([b[0], b[1]]) as usize;
                b = &b[2..];
                need(b, len)?;
                if vtype == 7 {
                    out.push((name, String::from_utf8_lossy(&b[..len]).into_owned()));
                }
                b = &b[len..];
            }
            8 => b = skip(b, 8)?,                     // timestamp (int64 millis)
            9 => b = skip(b, 16)?,                    // uuid
            other => return Err(DecodeError(format!("unknown header value type {other}"))),
        }
    }
    Ok(out)
}

fn skip(b: &[u8], n: usize) -> Result<&[u8], DecodeError> {
    need(b, n)?;
    Ok(&b[n..])
}

/// Encode a frame with only string headers (test helper; CRCs written as 0,
/// which the decoder ignores). `pub(crate)` + `cfg(test)` so other modules'
/// tests can build fixtures.
#[cfg(test)]
pub(crate) fn encode_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut hbytes = Vec::new();
    for (name, val) in headers {
        hbytes.push(u8::try_from(name.len()).unwrap());
        hbytes.extend_from_slice(name.as_bytes());
        hbytes.push(7u8); // string
        hbytes.extend_from_slice(&u16::try_from(val.len()).unwrap().to_be_bytes());
        hbytes.extend_from_slice(val.as_bytes());
    }
    let total = PRELUDE_LEN + hbytes.len() + payload.len() + MESSAGE_CRC_LEN;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&u32::try_from(total).unwrap().to_be_bytes());
    out.extend_from_slice(&u32::try_from(hbytes.len()).unwrap().to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // prelude crc (ignored)
    out.extend_from_slice(&hbytes);
    out.extend_from_slice(payload);
    out.extend_from_slice(&0u32.to_be_bytes()); // message crc (ignored)
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_single_frame_with_string_headers() {
        let bytes = encode_frame(
            &[(":message-type", "event"), (":event-type", "chunk")],
            b"{\"bytes\":\"abc\"}",
        );
        let mut dec = EventStreamDecoder::new();
        dec.extend(&bytes);
        let frame = dec.next_frame().unwrap().expect("complete frame");
        assert_eq!(frame.header(":event-type"), Some("chunk"));
        assert_eq!(frame.header(":message-type"), Some("event"));
        assert_eq!(frame.payload, b"{\"bytes\":\"abc\"}".to_vec());
        // buffer drained → no more frames
        assert_eq!(dec.next_frame().unwrap(), None);
    }

    #[test]
    fn reassembles_across_partial_feeds() {
        let f1 = encode_frame(&[(":event-type", "chunk")], b"p1");
        let f2 = encode_frame(&[(":event-type", "chunk")], b"p2");
        let mut all = f1.clone();
        all.extend_from_slice(&f2);
        let mut dec = EventStreamDecoder::new();
        // feed one byte at a time; frames appear only when complete
        let mut frames = Vec::new();
        for byte in all {
            dec.extend(&[byte]);
            while let Some(fr) = dec.next_frame().unwrap() {
                frames.push(fr.payload);
            }
        }
        assert_eq!(frames, vec![b"p1".to_vec(), b"p2".to_vec()]);
    }

    #[test]
    fn rejects_bad_prelude() {
        let mut dec = EventStreamDecoder::new();
        // total_len=8 (< 16) is invalid
        dec.extend(&[0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(dec.next_frame().is_err());
    }
}
