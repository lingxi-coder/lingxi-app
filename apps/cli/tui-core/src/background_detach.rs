//! Private PTY control sequence used by an attached background TUI to release
//! its controller without terminating the child/session.

/// Per-child random token passed by the PTY supervisor.
pub const DETACH_TOKEN_ENV: &str = "LINGXI_BG_DETACH_TOKEN";

const PREFIX: &[u8] = b"\x1b]9;lingxi-detach:";
const SUFFIX: u8 = 0x07;

/// Build the private detach request stripped by the PTY supervisor.
#[must_use]
pub fn detach_request_sequence(token: &str) -> Vec<u8> {
    let mut sequence = Vec::with_capacity(PREFIX.len() + token.len() + 1);
    sequence.extend_from_slice(PREFIX);
    sequence.extend_from_slice(token.as_bytes());
    sequence.push(SUFFIX);
    sequence
}

/// Incremental filter: PTY reads may split a control sequence at any byte.
pub struct DetachRequestFilter {
    marker: Vec<u8>,
    pending: Vec<u8>,
}

impl DetachRequestFilter {
    /// Construct over the supervisor-generated child token.
    #[must_use]
    pub fn new(token: &str) -> Self {
        Self {
            marker: detach_request_sequence(token),
            pending: Vec::new(),
        }
    }

    /// Strip complete requests and return ordinary PTY output plus request count.
    pub fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, usize) {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        let mut requests = 0;
        while let Some(at) = find_bytes(&self.pending, &self.marker) {
            output.extend(self.pending.drain(..at));
            self.pending.drain(..self.marker.len());
            requests += 1;
        }
        let keep = longest_marker_prefix_suffix(&self.pending, &self.marker);
        let emit = self.pending.len().saturating_sub(keep);
        output.extend(self.pending.drain(..emit));
        (output, requests)
    }

    /// Flush non-control tail when the PTY closes.
    pub fn finish(mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

fn longest_marker_prefix_suffix(bytes: &[u8], marker: &[u8]) -> usize {
    let max = bytes.len().min(marker.len().saturating_sub(1));
    (1..=max)
        .rev()
        .find(|&length| bytes[bytes.len() - length..] == marker[..length])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_split_request_without_losing_neighboring_output() {
        let marker = detach_request_sequence("secret");
        let mut filter = DetachRequestFilter::new("secret");
        let split = marker.len() / 2;
        let (first, count) = filter.push(&[b"before".as_slice(), &marker[..split]].concat());
        assert_eq!(first, b"before");
        assert_eq!(count, 0);
        let (second, count) = filter.push(&[&marker[split..], b"after".as_slice()].concat());
        assert_eq!(second, b"after");
        assert_eq!(count, 1);
        assert!(filter.finish().is_empty());
    }

    #[test]
    fn wrong_token_is_forwarded_verbatim() {
        let wrong = detach_request_sequence("wrong");
        let mut filter = DetachRequestFilter::new("right");
        let (output, count) = filter.push(&wrong);
        assert_eq!(count, 0);
        let mut full = output;
        full.extend(filter.finish());
        assert_eq!(full, wrong);
    }
}
