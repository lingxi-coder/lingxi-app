//! AWS event-stream binary framing infrastructure.
//!
//! Implements the AWS event-stream wire format used by Bedrock streaming:
//! `[u32 BE total_len][u32 BE headers_len][u32 BE prelude_crc][headers][payload][u32 BE message_crc]`
//!
//! The CRC is IEEE CRC32 (reflected, poly `0xEDB8_8320`) computed with a
//! hand-rolled lookup table — no external crate dependency.

use crate::LlmError;

// ── CRC32 (IEEE, reflected, poly `0xEDB8_8320`) ────────────────────────────────

/// Generate the 256-entry CRC32 lookup table at compile time.
///
/// Uses the standard IEEE reflected polynomial `0xEDB8_8320`.
const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation, reason = "i < 256 fits u32")]
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// IEEE CRC32 lookup table (256 entries, reflected poly `0xEDB8_8320`).
const CRC_TABLE: [u32; 256] = make_crc_table();

/// Compute IEEE CRC32 over `data`.
///
/// The result matches the standard CRC32 of the POSIX/zlib/gzip family.
/// Verified: `crc32(b"123456789") == 0xCBF43926`.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC_TABLE[idx];
    }
    !crc
}

// ── Wire-format types ────────────────────────────────────────────────────────

/// A decoded AWS event-stream message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventStreamMessage {
    /// Headers with `value_type == 7` (string).  Each entry is `(name, value)`.
    /// Headers of other types are parsed for offset advancement but not retained.
    pub headers: Vec<(String, String)>,
    /// Raw message payload (the bytes between the headers block and the
    /// trailing message CRC).
    pub payload: Vec<u8>,
}

// ── EventStreamSplitter ──────────────────────────────────────────────────────

/// Stateful buffer that reassembles AWS event-stream binary frames from an
/// arbitrary byte stream.
///
/// Feed chunks via [`Self::feed`]; call [`Self::finish`] at end-of-stream to
/// verify there is no partial frame left in the buffer.
///
/// # Wire format
///
/// ```text
/// ┌─────────────┬──────────────┬──────────────┬─────────┬─────────┬─────────────┐
/// │ total_len   │ headers_len  │ prelude_crc  │ headers │ payload │ message_crc │
/// │ (u32 BE)    │ (u32 BE)     │ (u32 BE)     │         │         │ (u32 BE)    │
/// └─────────────┴──────────────┴──────────────┴─────────┴─────────┴─────────────┘
/// ```
///
/// - `prelude_crc` is CRC32 of the first 8 bytes (`total_len` + `headers_len`).
/// - `message_crc` is CRC32 of everything before it
///   (prelude incl. `prelude_crc` + headers + payload).
/// - Minimum frame size: 4 + 4 + 4 + 4 = 16 bytes (no headers, no payload).
#[derive(Debug, Default)]
pub struct EventStreamSplitter {
    buf: Vec<u8>,
}

/// Minimum number of bytes needed to know a frame's total length.
const PRELUDE_BYTES: usize = 12; // total_len(4) + headers_len(4) + prelude_crc(4)
/// Fixed overhead added to the payload+headers region: prelude(12) + `message_crc`(4).
const FRAME_OVERHEAD: usize = 16;
/// Hard upper bound on a single frame (defense-in-depth vs hostile lengths;
/// real Bedrock frames are well under 1 MiB).
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

impl EventStreamSplitter {
    /// Feed a chunk of bytes and return every complete frame decoded so far.
    ///
    /// Partial frames are retained internally until subsequent [`Self::feed`]
    /// calls complete them.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::StreamInterrupted`] when:
    /// - The prelude CRC does not match the first 8 bytes.
    /// - The message CRC does not match the frame data preceding it.
    /// - A header name or string value is not valid UTF-8.
    /// - Header length arithmetic is inconsistent with `total_len`.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<EventStreamMessage>, LlmError> {
        self.buf.extend_from_slice(chunk);
        let mut messages = Vec::new();

        loop {
            // Need at least the prelude to know the total frame length.
            if self.buf.len() < PRELUDE_BYTES {
                break;
            }

            let total_len = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
            let headers_len = u32::from_be_bytes(self.buf[4..8].try_into().unwrap()) as usize;

            // ── Prelude CRC ─────────────────────────────────────────────────
            // Validated BEFORE waiting for `total_len` bytes: a fabricated
            // length field fails here immediately instead of making the
            // splitter buffer gigabytes waiting for a frame that never ends.
            let expected_prelude_crc = u32::from_be_bytes(self.buf[8..12].try_into().unwrap());
            let actual_prelude_crc = crc32(&self.buf[0..8]);
            if actual_prelude_crc != expected_prelude_crc {
                return Err(LlmError::StreamInterrupted {
                    message: format!(
                        "event-stream prelude CRC mismatch: expected 0x{expected_prelude_crc:08X}, got 0x{actual_prelude_crc:08X}"
                    ),
                });
            }

            // Validate that total_len is geometrically consistent.
            if total_len < FRAME_OVERHEAD {
                return Err(LlmError::StreamInterrupted {
                    message: format!(
                        "event-stream frame total_len={total_len} is smaller than the minimum {FRAME_OVERHEAD}"
                    ),
                });
            }
            // Defense-in-depth bound even for CRC-valid frames: Bedrock frames
            // are well under 1 MiB; refuse anything claiming more than 8 MiB.
            if total_len > MAX_FRAME_BYTES {
                return Err(LlmError::StreamInterrupted {
                    message: format!(
                        "event-stream frame total_len={total_len} exceeds the {MAX_FRAME_BYTES}-byte limit"
                    ),
                });
            }

            // Not enough bytes yet for the full frame.
            if self.buf.len() < total_len {
                break;
            }

            // We have a full frame in `self.buf[0..total_len]`.
            let frame = &self.buf[..total_len];

            // ── Message CRC ─────────────────────────────────────────────────
            let expected_message_crc =
                u32::from_be_bytes(frame[total_len - 4..total_len].try_into().unwrap());
            let actual_message_crc = crc32(&frame[..total_len - 4]);
            if actual_message_crc != expected_message_crc {
                return Err(LlmError::StreamInterrupted {
                    message: format!(
                        "event-stream message CRC mismatch: expected 0x{expected_message_crc:08X}, got 0x{actual_message_crc:08X}"
                    ),
                });
            }

            // ── Headers ─────────────────────────────────────────────────────
            let headers_start = PRELUDE_BYTES;
            let headers_end = headers_start + headers_len;
            if headers_end > total_len - 4 {
                return Err(LlmError::StreamInterrupted {
                    message: format!(
                        "event-stream headers_len={headers_len} overflows frame total_len={total_len}"
                    ),
                });
            }
            let headers_bytes = &frame[headers_start..headers_end];
            let headers = decode_headers(headers_bytes)?;

            // ── Payload ─────────────────────────────────────────────────────
            let payload_start = headers_end;
            let payload_end = total_len - 4; // strip trailing message CRC
            let payload = frame[payload_start..payload_end].to_vec();

            messages.push(EventStreamMessage { headers, payload });

            // Consume the frame from the buffer without reallocating the tail.
            let remaining = self.buf.len() - total_len;
            self.buf.copy_within(total_len.., 0);
            self.buf.truncate(remaining);
        }

        Ok(messages)
    }

    /// Assert that the internal buffer is empty at end-of-stream.
    ///
    /// A non-empty buffer means the stream ended mid-frame (truncated or
    /// corrupted).
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::StreamInterrupted`] when there are unconsumed bytes.
    pub fn finish(&self) -> Result<(), LlmError> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(LlmError::StreamInterrupted {
                message: format!(
                    "event-stream ended with {} unconsumed bytes (truncated frame)",
                    self.buf.len()
                ),
            })
        }
    }
}

// ── Header decoding ──────────────────────────────────────────────────────────

/// Value-type byte constants per the AWS event-stream spec.
const VALUE_TYPE_BOOL_TRUE: u8 = 0;
const VALUE_TYPE_BOOL_FALSE: u8 = 1;
const VALUE_TYPE_I8: u8 = 2;
const VALUE_TYPE_I16: u8 = 3;
const VALUE_TYPE_I32: u8 = 4;
const VALUE_TYPE_I64: u8 = 5;
const VALUE_TYPE_BYTES: u8 = 6;
const VALUE_TYPE_STRING: u8 = 7;
const VALUE_TYPE_TIMESTAMP: u8 = 8;
const VALUE_TYPE_UUID: u8 = 9;

/// Decode the headers block of a frame.
///
/// Header encoding: `[u8 name_len][name bytes][u8 value_type][value bytes…]`
///
/// Only `value_type == 7` (string, `[u16 BE len][bytes]`) is retained in the
/// output; all other types are parsed for byte-advancement only.
fn decode_headers(mut data: &[u8]) -> Result<Vec<(String, String)>, LlmError> {
    let mut headers = Vec::new();

    while !data.is_empty() {
        // ── Name ────────────────────────────────────────────────────────────
        let name_len = read_u8(&mut data)?;
        let name_bytes = read_exact(&mut data, name_len as usize, "header name")?;
        let name =
            String::from_utf8(name_bytes.to_vec()).map_err(|e| LlmError::StreamInterrupted {
                message: format!("event-stream header name is not UTF-8: {e}"),
            })?;

        // ── Value type ───────────────────────────────────────────────────────
        let value_type = read_u8(&mut data)?;

        match value_type {
            VALUE_TYPE_BOOL_TRUE | VALUE_TYPE_BOOL_FALSE => {
                // No payload bytes.
            }
            VALUE_TYPE_I8 => {
                read_exact(&mut data, 1, "i8 header value")?;
            }
            VALUE_TYPE_I16 => {
                read_exact(&mut data, 2, "i16 header value")?;
            }
            VALUE_TYPE_I32 => {
                read_exact(&mut data, 4, "i32 header value")?;
            }
            VALUE_TYPE_I64 => {
                read_exact(&mut data, 8, "i64 header value")?;
            }
            VALUE_TYPE_BYTES => {
                let len = read_u16_be(&mut data, "bytes header length")?;
                read_exact(&mut data, len as usize, "bytes header value")?;
            }
            VALUE_TYPE_STRING => {
                let len = read_u16_be(&mut data, "string header length")?;
                let value_bytes = read_exact(&mut data, len as usize, "string header value")?;
                let value = String::from_utf8(value_bytes.to_vec()).map_err(|e| {
                    LlmError::StreamInterrupted {
                        message: format!(
                            "event-stream string header '{name}' value is not UTF-8: {e}"
                        ),
                    }
                })?;
                headers.push((name, value));
                // Already consumed; skip to next iteration.
                continue;
            }
            VALUE_TYPE_TIMESTAMP => {
                read_exact(&mut data, 8, "timestamp header value")?;
            }
            VALUE_TYPE_UUID => {
                read_exact(&mut data, 16, "uuid header value")?;
            }
            other => {
                return Err(LlmError::StreamInterrupted {
                    message: format!("event-stream unknown header value_type={other}"),
                });
            }
        }

        // Non-string type: name is parsed but not retained.
        let _ = name;
    }

    Ok(headers)
}

// ── Byte-slice reading helpers ───────────────────────────────────────────────

fn read_u8(data: &mut &[u8]) -> Result<u8, LlmError> {
    if data.is_empty() {
        return Err(LlmError::StreamInterrupted {
            message: "event-stream header truncated reading u8".to_string(),
        });
    }
    let val = data[0];
    *data = &data[1..];
    Ok(val)
}

fn read_u16_be(data: &mut &[u8], ctx: &str) -> Result<u16, LlmError> {
    let bytes = read_exact(data, 2, ctx)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_exact<'a>(data: &mut &'a [u8], n: usize, ctx: &str) -> Result<&'a [u8], LlmError> {
    if data.len() < n {
        return Err(LlmError::StreamInterrupted {
            message: format!(
                "event-stream header block truncated reading {ctx}: need {n}, have {}",
                data.len()
            ),
        });
    }
    let out = &data[..n];
    *data = &data[n..];
    Ok(out)
}

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Build a single AWS event-stream binary frame from raw `headers_bytes` and
/// `payload`.
///
/// `headers_bytes` must already be in the encoded wire format
/// (`[u8 name_len][name][u8 value_type][...]`).  Use [`encode_string_header`]
/// to build individual header entries.
///
/// CRCs are computed with the same [`crc32`] function used by the parser so
/// round-trip tests work correctly. For an independent CRC verification use the
/// `crc32_pin` unit test.
#[cfg(test)]
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "test fixture builder; sizes are tiny"
)]
pub fn build_frame(headers_bytes: &[u8], payload: &[u8]) -> Vec<u8> {
    let headers_len = headers_bytes.len() as u32;
    let payload_len = payload.len();
    let total_len = (FRAME_OVERHEAD + headers_bytes.len() + payload_len) as u32;

    let mut frame = Vec::with_capacity(total_len as usize);

    // Prelude: total_len + headers_len
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.extend_from_slice(&headers_len.to_be_bytes());

    // Prelude CRC (over first 8 bytes)
    let prelude_crc = crc32(&frame[0..8]);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());

    // Headers + payload
    frame.extend_from_slice(headers_bytes);
    frame.extend_from_slice(payload);

    // Message CRC (over everything so far)
    let message_crc = crc32(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());

    frame
}

/// Encode a single string header (`value_type == 7`) into wire bytes.
#[cfg(test)]
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "test fixture builder; sizes are tiny"
)]
pub fn encode_string_header(name: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let name_bytes = name.as_bytes();
    out.push(name_bytes.len() as u8);
    out.extend_from_slice(name_bytes);
    out.push(VALUE_TYPE_STRING); // type 7
    let value_bytes = value.as_bytes();
    out.extend_from_slice(&(value_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(value_bytes);
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── CRC32 pin ───────────────────────────────────────────────────────────

    /// CRC32 of the ASCII string "123456789" must equal 0xCBF43926.
    ///
    /// This is the well-known "check value" for IEEE CRC32 and serves as an
    /// independent pin that the table and algorithm are correct before any
    /// framing tests run.
    #[test]
    fn crc32_pin_123456789() {
        assert_eq!(
            crc32(b"123456789"),
            0xCBF4_3926,
            "IEEE CRC32 of '123456789' must be 0xCBF43926"
        );
    }

    /// Additional sanity: CRC32 of empty input is 0x00000000.
    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    // ── Helper: fully hand-computed fixture ─────────────────────────────────

    /// Build a minimal frame (no headers, 4-byte payload `[1, 2, 3, 4]`) and
    /// verify both CRCs by re-computing them inline, checking that the
    /// `build_frame` helper and the parser are consistent.
    ///
    /// Byte layout:
    ///   offset 0..4  : `total_len` = 20 (16 overhead + 4 payload)
    ///   offset 4..8  : `headers_len` = 0
    ///   offset 8..12 : `prelude_crc` = crc32(bytes\[0..8\])
    ///   offset 12..16: payload \[1, 2, 3, 4\]
    ///   offset 16..20: `message_crc` = crc32(bytes\[0..16\])
    #[test]
    fn hand_computed_empty_headers_tiny_payload() {
        let payload = &[1u8, 2, 3, 4];
        let frame = build_frame(&[], payload);

        // total_len must be 16 (overhead) + 4 (payload) = 20
        assert_eq!(frame.len(), 20);

        let total_len = u32::from_be_bytes(frame[0..4].try_into().unwrap());
        let headers_len = u32::from_be_bytes(frame[4..8].try_into().unwrap());
        assert_eq!(total_len, 20);
        assert_eq!(headers_len, 0);

        // Prelude CRC
        let stored_prelude_crc = u32::from_be_bytes(frame[8..12].try_into().unwrap());
        let computed_prelude_crc = crc32(&frame[0..8]);
        assert_eq!(
            stored_prelude_crc, computed_prelude_crc,
            "prelude CRC mismatch in hand fixture"
        );

        // Message CRC
        let stored_message_crc = u32::from_be_bytes(frame[16..20].try_into().unwrap());
        let computed_message_crc = crc32(&frame[0..16]);
        assert_eq!(
            stored_message_crc, computed_message_crc,
            "message CRC mismatch in hand fixture"
        );

        // Parser must decode it correctly.
        let mut splitter = EventStreamSplitter::default();
        let msgs = splitter.feed(&frame).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].headers, Vec::<(String, String)>::new());
        assert_eq!(msgs[0].payload, payload);
        splitter.finish().unwrap();
    }

    // ── Single frame ────────────────────────────────────────────────────────

    #[test]
    fn single_frame_no_headers() {
        let payload = b"hello world";
        let frame = build_frame(&[], payload);

        let mut splitter = EventStreamSplitter::default();
        let msgs = splitter.feed(&frame).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].headers.is_empty());
        assert_eq!(msgs[0].payload, payload);
        splitter.finish().unwrap();
    }

    #[test]
    fn single_frame_with_string_header() {
        let header_bytes = encode_string_header(":message-type", "event");
        let payload = b"{\"type\":\"message_start\"}";
        let frame = build_frame(&header_bytes, payload);

        let mut splitter = EventStreamSplitter::default();
        let msgs = splitter.feed(&frame).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0].headers,
            vec![(":message-type".to_string(), "event".to_string())]
        );
        assert_eq!(msgs[0].payload, payload);
    }

    // ── Two frames in one chunk ─────────────────────────────────────────────

    #[test]
    fn two_frames_one_chunk() {
        let frame1 = build_frame(&[], b"first");
        let frame2 = build_frame(
            &encode_string_header(":event-type", "content_block_delta"),
            b"second",
        );

        let mut both = frame1.clone();
        both.extend_from_slice(&frame2);

        let mut splitter = EventStreamSplitter::default();
        let msgs = splitter.feed(&both).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].payload, b"first");
        assert_eq!(msgs[1].payload, b"second");
        splitter.finish().unwrap();
    }

    // ── Frame split across chunks ───────────────────────────────────────────

    /// Split the byte stream INSIDE the prelude (after 2 bytes), verifying the
    /// splitter buffers correctly.
    #[test]
    fn frame_split_inside_prelude() {
        let frame = build_frame(&[], b"payload");
        assert!(frame.len() > 4, "frame must be longer than split point");

        let mut splitter = EventStreamSplitter::default();

        // Feed just 2 bytes — not enough even for the prelude.
        let msgs = splitter.feed(&frame[..2]).unwrap();
        assert!(msgs.is_empty(), "no complete frame yet");

        // Feed the rest.
        let msgs = splitter.feed(&frame[2..]).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload, b"payload");
        splitter.finish().unwrap();
    }

    /// Split the byte stream inside the payload region.
    #[test]
    fn frame_split_inside_payload() {
        let frame = build_frame(&[], b"longer payload bytes here");

        let midpoint = frame.len() / 2;
        let mut splitter = EventStreamSplitter::default();

        let msgs = splitter.feed(&frame[..midpoint]).unwrap();
        assert!(msgs.is_empty(), "first half is incomplete");

        let msgs = splitter.feed(&frame[midpoint..]).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload, b"longer payload bytes here");
        splitter.finish().unwrap();
    }

    // ── CRC error cases ─────────────────────────────────────────────────────

    #[test]
    fn bad_prelude_crc_returns_error() {
        let mut frame = build_frame(&[], b"data");

        // Corrupt the prelude CRC (bytes 8..12).
        frame[9] ^= 0xFF;

        let mut splitter = EventStreamSplitter::default();
        let err = splitter.feed(&frame).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("prelude CRC"),
            "error must mention prelude CRC; got: {msg}"
        );
    }

    #[test]
    fn bad_message_crc_returns_error() {
        let mut frame = build_frame(&[], b"data");
        let len = frame.len();

        // Corrupt the message CRC (last 4 bytes).
        frame[len - 2] ^= 0xFF;

        let mut splitter = EventStreamSplitter::default();
        let err = splitter.feed(&frame).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("message CRC"),
            "error must mention message CRC; got: {msg}"
        );
    }

    // ── Trailing garbage on finish ──────────────────────────────────────────

    #[test]
    fn trailing_garbage_on_finish_returns_error() {
        let mut splitter = EventStreamSplitter::default();
        // Feed a complete frame...
        let frame = build_frame(&[], b"ok");
        splitter.feed(&frame).unwrap();
        // ...then inject garbage that doesn't form a complete frame.
        splitter.feed(b"\x00\x00").unwrap();

        let err = splitter.finish().unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unconsumed bytes"),
            "error must mention unconsumed bytes; got: {msg}"
        );
    }

    // ── Hostile-length defense ──────────────────────────────────────────────

    /// A fabricated huge `total_len` with a WRONG prelude CRC fails immediately
    /// (the CRC is validated before waiting for `total_len` bytes).
    #[test]
    fn fabricated_total_len_fails_on_prelude_crc_immediately() {
        let mut splitter = EventStreamSplitter::default();
        let mut hostile = Vec::new();
        hostile.extend_from_slice(&u32::MAX.to_be_bytes()); // total_len = 4 GiB
        hostile.extend_from_slice(&0u32.to_be_bytes()); // headers_len
        hostile.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes()); // bogus prelude CRC

        let err = splitter.feed(&hostile).unwrap_err();
        assert!(
            format!("{err}").contains("prelude CRC"),
            "must fail on prelude CRC before buffering; got: {err}"
        );
    }

    /// A CRC-VALID frame claiming more than `MAX_FRAME_BYTES` is refused
    /// (defense-in-depth: never buffer multi-gigabyte frames).
    #[test]
    fn crc_valid_oversize_frame_is_refused() {
        let mut splitter = EventStreamSplitter::default();
        let total_len = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
        let mut hostile = Vec::new();
        hostile.extend_from_slice(&total_len.to_be_bytes());
        hostile.extend_from_slice(&0u32.to_be_bytes());
        let crc = crc32(&hostile[0..8]); // valid prelude CRC
        hostile.extend_from_slice(&crc.to_be_bytes());

        let err = splitter.feed(&hostile).unwrap_err();
        assert!(
            format!("{err}").contains("exceeds"),
            "must refuse oversize frames; got: {err}"
        );
    }

    // ── Non-string header types skipped correctly ───────────────────────────

    #[test]
    fn non_string_header_types_skipped() {
        // Build a headers block manually: one bool-true (type 0) + one string (type 7).
        let mut headers_bytes = Vec::new();

        // Bool true header ":flag" (type 0, no payload bytes)
        let flag_name = b":flag";
        headers_bytes.push(u8::try_from(flag_name.len()).unwrap());
        headers_bytes.extend_from_slice(flag_name);
        headers_bytes.push(VALUE_TYPE_BOOL_TRUE);

        // String header ":message-type" = "event"
        headers_bytes.extend_from_slice(&encode_string_header(":message-type", "event"));

        let frame = build_frame(&headers_bytes, b"body");
        let mut splitter = EventStreamSplitter::default();
        let msgs = splitter.feed(&frame).unwrap();
        assert_eq!(msgs.len(), 1);
        // Only the string header is retained.
        assert_eq!(
            msgs[0].headers,
            vec![(":message-type".to_string(), "event".to_string())]
        );
    }
}
