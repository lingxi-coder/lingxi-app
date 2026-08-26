//! Incremental SSE frame splitting for byte-stream transports.
//!
//! Hosts whose HTTP layer already parses SSE can map each event's data
//! payload to a [`RawStreamFrame`] directly and skip this module.

use crate::RawStreamFrame;

/// Splits an SSE byte stream into one [`RawStreamFrame`] per event.
///
/// A frame carries the event's joined `data:` payload without the field
/// prefix — exactly what the provider stream decoders consume. Comment lines
/// and `event:`/`id:`/`retry:` fields are ignored; events with no data lines
/// produce no frame.
#[derive(Debug, Default)]
pub struct SseFrameSplitter {
    buffer: Vec<u8>,
}

impl SseFrameSplitter {
    /// Feed bytes and return frames for every event this chunk completed.
    ///
    /// Partial events are buffered until a blank line (`\n\n` or
    /// `\r\n\r\n`, mixed endings included) terminates them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<RawStreamFrame> {
        self.buffer.extend_from_slice(bytes);

        let mut frames = Vec::new();
        while let Some((content_len, consumed)) = find_event_boundary(&self.buffer) {
            if let Some(frame) = parse_event(&self.buffer[..content_len]) {
                frames.push(frame);
            }
            let remaining = self.buffer.len() - consumed;
            self.buffer.copy_within(consumed.., 0);
            self.buffer.truncate(remaining);
        }
        frames
    }

    /// Flush a trailing unterminated event at end of stream.
    pub fn finish(&mut self) -> Option<RawStreamFrame> {
        let event = std::mem::take(&mut self.buffer);
        parse_event(&event)
    }
}

/// Find the first blank line; returns `(content_len, total_consumed)`.
fn find_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    for (index, byte) in buffer.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if buffer.get(index + 1) == Some(&b'\n') {
            return Some((index + 1, index + 2));
        }
        if buffer.get(index + 1) == Some(&b'\r') && buffer.get(index + 2) == Some(&b'\n') {
            return Some((index + 1, index + 3));
        }
    }
    None
}

fn parse_event(event: &[u8]) -> Option<RawStreamFrame> {
    let mut data = Vec::new();
    let mut has_data = false;

    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(mut value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        if has_data {
            data.push(b'\n');
        }
        data.extend_from_slice(value);
        has_data = true;
    }

    has_data.then(|| RawStreamFrame::new(data))
}
