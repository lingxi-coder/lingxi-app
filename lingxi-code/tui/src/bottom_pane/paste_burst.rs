//! Paste-burst detection for terminals without bracketed paste — ported from
//! codex-rs/tui `bottom_pane/paste_burst.rs`.
//!
//! On some platforms (notably Windows), pastes arrive as a rapid stream of
//! `KeyCode::Char` / `KeyCode::Enter` key events rather than as a single
//! bracketed `Event::Paste`. In that mode the composer needs to:
//!
//! - treat Enter as a newline *inside the paste*, not "submit the message";
//! - avoid flicker from inserting a typed prefix and then reclassifying it as
//!   a paste once enough chars arrive;
//! - route the reassembled text through the normal paste pipeline (large-paste
//!   placeholder, image-path detection).
//!
//! `PasteBurst` is a pure state machine: it never mutates the composer. The
//! caller feeds it events and applies the returned decisions:
//!
//! - For each plain `KeyCode::Char`, call [`PasteBurst::on_plain_char`]
//!   (ASCII) or [`PasteBurst::on_plain_char_no_hold`] (non-ASCII/IME).
//! - On a UI tick, call [`PasteBurst::flush_if_due`]: [`FlushResult::Typed`]
//!   inserts the held char as normal typing; [`FlushResult::Paste`] routes the
//!   buffer through the paste pipeline.
//! - Before applying non-char/modified input, call
//!   [`PasteBurst::flush_before_modified_input`] then
//!   [`PasteBurst::clear_window_after_non_char`].
//! - On an explicit bracketed paste, call
//!   [`PasteBurst::clear_after_explicit_paste`].
//!
//! Timing model: `PASTE_BURST_CHAR_INTERVAL` bounds the gap between chars of
//! one burst (and how long the first char is held); once buffering is active,
//! `PASTE_BURST_ACTIVE_IDLE_TIMEOUT` is the quiet period before the buffer
//! flushes as a paste. `flush_if_due` compares with `>` (not `>=`), so ticks
//! should cross the threshold by ≥1ms (see [`PasteBurst::recommended_flush_delay`]).

use std::time::{Duration, Instant};

// Heuristic thresholds for detecting paste-like input bursts.
// Detect quickly to avoid showing a typed prefix before the paste is recognized.
const PASTE_BURST_MIN_CHARS: u16 = 3;
const PASTE_ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(120);

/// Maximum delay between consecutive chars to be considered part of a burst.
const PASTE_BURST_CHAR_INTERVAL: Duration = Duration::from_millis(8);

// Idle timeout before flushing buffered paste content. Slower paste bursts
// have been observed in Windows environments.
#[cfg(not(windows))]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(8);
#[cfg(windows)]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(60);

/// The burst state machine. See the module docs for the caller contract.
#[derive(Default)]
pub(crate) struct PasteBurst {
    last_plain_char_time: Option<Instant>,
    consecutive_plain_char_burst: u16,
    burst_window_until: Option<Instant>,
    buffer: String,
    active: bool,
    /// Hold the first fast char briefly to avoid rendering flicker.
    pending_first_char: Option<(char, Instant)>,
}

/// What the caller should do with the current plain char.
pub(crate) enum CharDecision {
    /// Start buffering and retroactively capture some already-inserted chars.
    BeginBuffer { retro_chars: u16 },
    /// We are currently buffering; append the current char into the buffer.
    BufferAppend,
    /// Do not insert/render this char yet; it is held while we wait to see
    /// if a paste-like burst follows.
    RetainFirstChar,
    /// Begin buffering using the previously held first char (already moved
    /// into the buffer); append the current char.
    BeginBufferFromPending,
}

/// The outcome of a tick flush.
pub(crate) enum FlushResult {
    /// A completed burst: route through the paste pipeline.
    Paste(String),
    /// A held first char with no burst following: insert as normal typing.
    Typed(char),
    /// Nothing due.
    None,
}

impl PasteBurst {
    /// Recommended delay before a tick so a held fast keystroke flushes as
    /// normal typed input (crosses `PASTE_BURST_CHAR_INTERVAL` by 1ms).
    #[cfg(test)]
    pub fn recommended_flush_delay() -> Duration {
        PASTE_BURST_CHAR_INTERVAL + Duration::from_millis(1)
    }

    #[cfg(test)]
    pub(crate) fn recommended_active_flush_delay() -> Duration {
        PASTE_BURST_ACTIVE_IDLE_TIMEOUT + Duration::from_millis(1)
    }

    /// Entry point: decide how to treat a plain ASCII char with current timing.
    pub fn on_plain_char(&mut self, ch: char, now: Instant) -> CharDecision {
        self.note_plain_char(now);

        if self.active {
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return CharDecision::BufferAppend;
        }

        // If we already held a first char and receive a second fast char,
        // start buffering without retro-grabbing (we never rendered the first).
        if let Some((held, held_at)) = self.pending_first_char {
            if now.duration_since(held_at) <= PASTE_BURST_CHAR_INTERVAL {
                self.active = true;
                let _ = self.pending_first_char.take();
                self.buffer.push(held);
                self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
                return CharDecision::BeginBufferFromPending;
            }
        }

        if self.consecutive_plain_char_burst >= PASTE_BURST_MIN_CHARS {
            return CharDecision::BeginBuffer {
                retro_chars: self.consecutive_plain_char_burst.saturating_sub(1),
            };
        }

        // Save the first fast char very briefly to see if a burst follows.
        self.pending_first_char = Some((ch, now));
        CharDecision::RetainFirstChar
    }

    /// Like [`Self::on_plain_char`], but never holds the first char. Used for
    /// non-ASCII (IME) input where holding a char feels like dropped input.
    /// Only ever returns `BufferAppend` or `BeginBuffer`.
    pub fn on_plain_char_no_hold(&mut self, now: Instant) -> Option<CharDecision> {
        self.note_plain_char(now);

        if self.active {
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return Some(CharDecision::BufferAppend);
        }

        if self.consecutive_plain_char_burst >= PASTE_BURST_MIN_CHARS {
            return Some(CharDecision::BeginBuffer {
                retro_chars: self.consecutive_plain_char_burst.saturating_sub(1),
            });
        }

        None
    }

    fn note_plain_char(&mut self, now: Instant) {
        match self.last_plain_char_time {
            Some(prev) if now.duration_since(prev) <= PASTE_BURST_CHAR_INTERVAL => {
                self.consecutive_plain_char_burst =
                    self.consecutive_plain_char_burst.saturating_add(1);
            }
            _ => self.consecutive_plain_char_burst = 1,
        }
        self.last_plain_char_time = Some(now);
    }

    /// Flush any buffered burst (or held first char) if the inter-key timeout
    /// has elapsed.
    pub fn flush_if_due(&mut self, now: Instant) -> FlushResult {
        let timeout = if self.is_active_internal() {
            PASTE_BURST_ACTIVE_IDLE_TIMEOUT
        } else {
            PASTE_BURST_CHAR_INTERVAL
        };
        let timed_out = self
            .last_plain_char_time
            .is_some_and(|t| now.duration_since(t) > timeout);
        if timed_out && self.is_active_internal() {
            self.active = false;
            let out = std::mem::take(&mut self.buffer);
            FlushResult::Paste(out)
        } else if timed_out {
            // A single held fast char with no burst following: normal typing.
            if let Some((ch, _at)) = self.pending_first_char.take() {
                FlushResult::Typed(ch)
            } else {
                FlushResult::None
            }
        } else {
            FlushResult::None
        }
    }

    /// While bursting: accumulate a newline into the buffer instead of
    /// submitting. Returns true when appended (we are in a burst context).
    pub fn append_newline_if_active(&mut self, now: Instant) -> bool {
        if self.is_active() {
            self.buffer.push('\n');
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            true
        } else {
            false
        }
    }

    /// Whether Enter should insert a newline (burst context) vs submit.
    pub fn newline_should_insert_instead_of_submit(&self, now: Instant) -> bool {
        let in_burst_window = self.burst_window_until.is_some_and(|until| now <= until);
        self.is_active() || in_burst_window
    }

    /// Begin buffering with retroactively grabbed text.
    fn begin_with_retro_grabbed(&mut self, grabbed: String, now: Instant) {
        if !grabbed.is_empty() {
            self.buffer.push_str(&grabbed);
        }
        self.active = true;
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
    }

    /// Append a char into the burst buffer.
    pub fn append_char_to_buffer(&mut self, ch: char, now: Instant) {
        self.buffer.push(ch);
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
    }

    /// Decide whether to begin buffering by retroactively capturing
    /// `retro_tail` — the already-inserted chars of the burst window,
    /// immediately before the cursor.
    ///
    /// Heuristic: a tail containing whitespace or ≥16 chars is paste-like
    /// (URLs, paths, multiline text) — short words are not, so ordinary fast
    /// typing never disappears into a buffer. Returns true when buffering
    /// began; the CALLER then removes the tail from the UI text (it is now
    /// in the burst buffer).
    pub fn decide_begin_buffer(&mut self, now: Instant, retro_tail: &str) -> bool {
        let looks_pastey =
            retro_tail.chars().any(char::is_whitespace) || retro_tail.chars().count() >= 16;
        if looks_pastey {
            self.begin_with_retro_grabbed(retro_tail.to_string(), now);
        }
        looks_pastey
    }

    /// Before applying modified/non-char input: flush the buffered burst
    /// immediately (returns the text to route through the paste path).
    pub fn flush_before_modified_input(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        self.active = false;
        let mut out = std::mem::take(&mut self.buffer);
        if let Some((ch, _at)) = self.pending_first_char.take() {
            out.push(ch);
        }
        Some(out)
    }

    /// Clear only the timing window and any pending first-char so subsequent
    /// typing does not join a previous burst. Callers must have flushed any
    /// buffer first (this clears the timestamp `flush_if_due` times out
    /// against).
    pub fn clear_window_after_non_char(&mut self) {
        self.consecutive_plain_char_burst = 0;
        self.last_plain_char_time = None;
        self.burst_window_until = None;
        self.active = false;
        self.pending_first_char = None;
    }

    /// Whether we are in any burst-related transient state (buffering, a
    /// non-empty buffer, or a held first char).
    pub fn is_active(&self) -> bool {
        self.is_active_internal() || self.pending_first_char.is_some()
    }

    fn is_active_internal(&self) -> bool {
        self.active || !self.buffer.is_empty()
    }

    /// An explicit bracketed paste arrived: a real paste cannot affect the
    /// next Enter, so drop every burst state including the window.
    pub fn clear_after_explicit_paste(&mut self) {
        self.last_plain_char_time = None;
        self.consecutive_plain_char_burst = 0;
        self.burst_window_until = None;
        self.active = false;
        self.buffer.clear();
        self.pending_first_char = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_single_char_flushes_as_typed() {
        let mut pb = PasteBurst::default();
        let t0 = Instant::now();
        assert!(matches!(
            pb.on_plain_char('a', t0),
            CharDecision::RetainFirstChar
        ));
        // After the interval passes, the held char flushes as normal typing.
        let later = t0 + PasteBurst::recommended_flush_delay();
        assert!(matches!(pb.flush_if_due(later), FlushResult::Typed('a')));
        assert!(!pb.is_active());
    }

    #[test]
    fn fast_chars_become_one_paste() {
        let mut pb = PasteBurst::default();
        let t0 = Instant::now();
        let step = Duration::from_millis(1);
        assert!(matches!(
            pb.on_plain_char('h', t0),
            CharDecision::RetainFirstChar
        ));
        // Second fast char upgrades the held char into a buffer.
        assert!(matches!(
            pb.on_plain_char('e', t0 + step),
            CharDecision::BeginBufferFromPending
        ));
        pb.append_char_to_buffer('e', t0 + step);
        for (i, ch) in ['l', 'l', 'o'].into_iter().enumerate() {
            let at = t0 + step * (2 + u32::try_from(i).unwrap());
            assert!(matches!(
                pb.on_plain_char(ch, at),
                CharDecision::BufferAppend
            ));
            pb.append_char_to_buffer(ch, at);
        }
        // Enter mid-burst appends a newline instead of submitting.
        assert!(pb.append_newline_if_active(t0 + step * 5));
        // Quiet period → the whole burst flushes as ONE paste.
        let flush_at = t0 + step * 5 + PasteBurst::recommended_active_flush_delay();
        match pb.flush_if_due(flush_at) {
            FlushResult::Paste(text) => assert_eq!(text, "hello\n"),
            _ => panic!("expected paste flush"),
        }
    }

    #[test]
    fn retro_grab_reclassifies_fast_typed_prefix() {
        let mut pb = PasteBurst::default();
        let t0 = Instant::now();
        let step = Duration::from_millis(1);
        // 17 fast chars inserted normally (no-hold path): the retro window
        // grows past 16 chars, which classifies as paste-like even without
        // whitespace.
        let before = "abcdefghijklmnopq"; // 17 chars already in the composer
        let mut last = None;
        for i in 0..17u32 {
            last = pb.on_plain_char_no_hold(t0 + step * i);
        }
        let Some(CharDecision::BeginBuffer { retro_chars }) = last else {
            panic!("expected BeginBuffer");
        };
        let tail: String = before
            .chars()
            .skip(before.chars().count() - usize::from(retro_chars))
            .collect();
        assert!(
            pb.decide_begin_buffer(t0 + step * 16, &tail),
            ">=16-char retro tail is paste-like"
        );
        assert!(pb.is_active());
    }

    #[test]
    fn enter_suppress_window_outlives_the_buffer() {
        let mut pb = PasteBurst::default();
        let t0 = Instant::now();
        // A held char upgraded by a second fast char = an active burst (also
        // seeds last_plain_char_time, which the flush timeout counts from).
        let _ = pb.on_plain_char('x', t0);
        let _ = pb.on_plain_char('y', t0 + Duration::from_millis(1));
        pb.append_char_to_buffer('y', t0 + Duration::from_millis(1));
        let flush_at = t0 + Duration::from_millis(1) + PasteBurst::recommended_active_flush_delay();
        assert!(matches!(pb.flush_if_due(flush_at), FlushResult::Paste(_)));
        // Right after the flush, Enter still inserts a newline (trailing
        // pasted newline must not submit).
        assert!(pb.newline_should_insert_instead_of_submit(flush_at));
        // Once the window lapses, Enter submits again.
        assert!(!pb.newline_should_insert_instead_of_submit(
            flush_at + PASTE_ENTER_SUPPRESS_WINDOW + Duration::from_millis(1)
        ));
    }

    #[test]
    fn short_fast_word_is_not_reclassified() {
        let mut pb = PasteBurst::default();
        let t0 = Instant::now();
        let step = Duration::from_millis(1);
        for i in 0..4u32 {
            let _ = pb.on_plain_char_no_hold(t0 + step * i);
        }
        let Some(CharDecision::BeginBuffer { retro_chars }) =
            pb.on_plain_char_no_hold(t0 + step * 4)
        else {
            panic!("expected BeginBuffer");
        };
        // A short word with no whitespace stays as normal typing.
        let _ = retro_chars;
        assert!(!pb.decide_begin_buffer(t0 + step * 4, "word"));
        assert!(!pb.is_active());
    }
}
