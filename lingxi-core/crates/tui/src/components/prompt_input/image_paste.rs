//! Bracketed-paste coalescing + image-on-paste detection.
//!
//! **Scope (parent spec §4 R8): detection + reference insertion only.** No
//! inline terminal image display, no byte reading / base64 / resize / API
//! wiring — those are M8. On an image paste we record `Attachment` metadata
//! and insert a `[Image #N]` placeholder; on a text paste we insert the text
//! verbatim as one block (multi-line paste does NOT submit per line).
//!
//! iocraft 0.8.3 surfaces no paste event (see `root.rs` notes), so paste is
//! detected by burst coalescing in the live key path; the pure classification
//! and ref-building logic lives here and is unit-tested directly.

use std::time::{Duration, Instant};

/// claude-code `formatImageRef(id)` → `[Image #id]` (history.ts:58-59).
#[must_use]
pub fn format_image_ref(id: usize) -> String {
    format!("[Image #{id}]")
}

/// Supported image extensions, lowercase, including the leading dot. Mirrors
/// claude-code `IMAGE_EXTENSION_REGEX = /\.(png|jpe?g|gif|webp)$/i`
/// (imagePaste.ts:270).
const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// Strip a single pair of matching outer single/double quotes
/// (claude-code `removeOuterQuotes`).
fn remove_outer_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// True when `text` is a path/filename ending in a supported image extension,
/// after trimming whitespace and stripping outer quotes (claude-code
/// `isImageFilePath`). Backslash-escape unescaping (`stripBackslashEscapes`)
/// is NOT ported (M8); a path with literal `\` simply fails to match (safe
/// degrade — never a false positive).
#[must_use]
pub fn is_image_path(text: &str) -> bool {
    let cleaned = remove_outer_quotes(text.trim()).trim();
    let lower = cleaned.to_ascii_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// What kind of attachment a paste produced. M7-10 only mints `Image`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentKind {
    /// An image detected on paste. M7-10 records the source only (no bytes).
    Image,
}

/// Metadata recorded for one detected attachment. The `[Image #id]` ref in the
/// prompt points back to this by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// 1-based id, matching the `[Image #id]` placeholder.
    pub id: usize,
    /// The kind (Image only in M7-10).
    pub kind: AttachmentKind,
    /// The source: the file path, or `"clipboard"` for an empty macOS paste.
    pub source: String,
}

/// Paste-related state on `AppState`. Holds the attachment registry and the
/// next id to mint. Defaults to id 1, empty registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteState {
    /// Next `[Image #N]` id to assign (1-based, increments per image).
    pub next_image_id: usize,
    /// Recorded attachments, in mint order.
    pub attachments: Vec<Attachment>,
}

impl Default for PasteState {
    fn default() -> Self {
        Self {
            next_image_id: 1,
            attachments: Vec::new(),
        }
    }
}

/// The result of processing one coalesced paste block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteOutcome {
    /// The text to insert at the cursor as ONE block (image lines swapped for
    /// `[Image #N]`, everything else verbatim).
    pub insertion: String,
    /// The updated paste state (appended attachments + bumped id).
    pub state: PasteState,
}

/// Split a paste block the way claude-code's `usePasteHandler` does: first on
/// spaces that precede an absolute path (` /` on unix, ` C:\` on windows),
/// then on newlines. Returns segments paired with the separator that FOLLOWED
/// each segment in the original, so the insertion can be rebuilt exactly.
fn split_paste_segments(block: &str) -> Vec<(String, &'static str)> {
    // Token = a run of text; sep = "\n", " ", or "" (last token).
    // We tokenize char-by-char so we can preserve newlines and the
    // path-boundary spaces as separators while leaving in-path spaces intact.
    let mut out: Vec<(String, &'static str)> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = block.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push((std::mem::take(&mut cur), "\n"));
            i += 1;
            continue;
        }
        if c == ' ' {
            // Split only when the space precedes an absolute path: ` /` or
            // ` X:\`. Otherwise the space stays inside the current token.
            let next = chars.get(i + 1).copied();
            let drive = matches!(next, Some(ch) if ch.is_ascii_alphabetic())
                && matches!(chars.get(i + 2).copied(), Some(':'))
                && matches!(chars.get(i + 3).copied(), Some('\\'));
            if next == Some('/') || drive {
                out.push((std::mem::take(&mut cur), " "));
                i += 1;
                continue;
            }
        }
        cur.push(c);
        i += 1;
    }
    out.push((cur, ""));
    out
}

/// Classify + rebuild a coalesced paste block into an insertion string and an
/// updated paste state. Image segments become `[Image #N]` and record an
/// `Attachment`; all other segments (and the separators) are preserved.
#[must_use]
pub fn process_paste(block: &str, mut state: PasteState) -> PasteOutcome {
    let mut insertion = String::with_capacity(block.len());
    for (seg, sep) in split_paste_segments(block) {
        if !seg.is_empty() && is_image_path(&seg) {
            let id = state.next_image_id;
            state.next_image_id += 1;
            state.attachments.push(Attachment {
                id,
                kind: AttachmentKind::Image,
                source: remove_outer_quotes(seg.trim()).trim().to_string(),
            });
            insertion.push_str(&format_image_ref(id));
        } else {
            insertion.push_str(&seg);
        }
        insertion.push_str(sep);
    }
    PasteOutcome { insertion, state }
}

/// Result of applying a coalesced block to the prompt buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteApply {
    /// New prompt text.
    pub prompt: String,
    /// New cursor byte-index (just past the inserted block).
    pub cursor: usize,
    /// Updated paste state.
    pub state: PasteState,
}

/// Run `process_paste` on `block`, splice the insertion into `prompt` at
/// `cursor` (a char boundary), and advance the cursor past it. The block is
/// inserted as ONE unit — embedded newlines do not trigger submit (the live
/// path only submits on a bare Enter that the coalescer has already flushed
/// past). `cursor` is clamped into `prompt` defensively.
#[must_use]
pub fn apply_paste_block(
    prompt: &str,
    cursor: usize,
    block: &str,
    state: PasteState,
) -> PasteApply {
    let PasteOutcome { insertion, state } = process_paste(block, state);
    let at = clamp_to_char_boundary(prompt, cursor);
    let mut out = String::with_capacity(prompt.len() + insertion.len());
    out.push_str(&prompt[..at]);
    out.push_str(&insertion);
    out.push_str(&prompt[at..]);
    let new_cursor = at + insertion.len();
    PasteApply {
        prompt: out,
        cursor: new_cursor,
        state,
    }
}

/// Clamp `cursor` down to the nearest char boundary at or below it
/// (mirrors `prompt_input::mod`'s private helper; duplicated here to keep
/// `apply_paste_block` self-contained and pure).
fn clamp_to_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut c = cursor;
    while c > 0 && !text.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// Max gap between consecutive chars to count as the same paste burst. Paste
/// chars arrive sub-millisecond apart; human typing is tens of ms apart. 50ms
/// matches claude-code's `CLIPBOARD_CHECK_DEBOUNCE_MS` / paste-completion feel.
pub const BURST_WINDOW: Duration = Duration::from_millis(50);

/// Coalesces a rapid burst of single-char `Key` events (iocraft has no paste
/// event — see `root.rs`) into one block. Pure over `(char, Instant)`.
#[derive(Debug, Default)]
pub struct PasteCoalescer {
    buf: String,
    last: Option<Instant>,
}

impl PasteCoalescer {
    /// New, empty coalescer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one printable char (newline included) arriving at `now`. If `now`
    /// is more than `BURST_WINDOW` after the previous char, the existing buffer
    /// is flushed first and returned; `ch` then starts a fresh buffer.
    /// Returns `Some(block)` only when a flush happened.
    #[must_use]
    pub fn push_char(&mut self, ch: char, now: Instant) -> Option<String> {
        let flushed = match self.last {
            Some(prev) if now.duration_since(prev) > BURST_WINDOW => self.take(),
            _ => None,
        };
        self.buf.push(ch);
        self.last = Some(now);
        flushed
    }

    /// Flush if the buffer has gone quiet (called from a periodic tick). Flushes
    /// when `now` is more than `BURST_WINDOW` past the last char.
    #[must_use]
    pub fn flush_if_idle(&mut self, now: Instant) -> Option<String> {
        match self.last {
            Some(prev) if now.duration_since(prev) > BURST_WINDOW => self.take(),
            _ => None,
        }
    }

    /// Flush immediately (e.g. a non-printable key arrived, or before submit).
    #[must_use]
    pub fn flush_now(&mut self) -> Option<String> {
        self.take()
    }

    /// True when there is at least one buffered char awaiting flush.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        !self.buf.is_empty()
    }

    /// True when a char/newline arriving at `now` would CONTINUE the current
    /// burst (there is a pending buffer and `now` is within `BURST_WINDOW` of
    /// the last char). The live key path uses this to decide whether an `Enter`
    /// is a *pasted* newline (continues the burst → buffer it) or a *deliberate*
    /// submit (no pending burst, or the gap exceeded the window → flush + act).
    /// This is the single guarantee that a lone Enter still submits promptly
    /// while a multi-line paste's embedded Enters never submit per line.
    #[must_use]
    pub fn would_continue_burst(&self, now: Instant) -> bool {
        match self.last {
            Some(prev) => now.duration_since(prev) <= BURST_WINDOW,
            None => false,
        }
    }

    fn take(&mut self) -> Option<String> {
        self.last = None;
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ref_format_matches_claude_code() {
        assert_eq!(format_image_ref(1), "[Image #1]");
        assert_eq!(format_image_ref(42), "[Image #42]");
    }

    #[test]
    fn is_image_path_matches_supported_extensions() {
        assert!(is_image_path("/tmp/shot.png"));
        assert!(is_image_path("/tmp/a.JPG")); // case-insensitive
        assert!(is_image_path("photo.jpeg"));
        assert!(is_image_path("anim.gif"));
        assert!(is_image_path("logo.webp"));
        assert!(is_image_path("'/Users/me/My Pic.png'")); // outer quotes stripped
        assert!(is_image_path("  /tmp/trailing.png  ")); // trimmed
        assert!(!is_image_path("/tmp/notes.txt"));
        assert!(!is_image_path("just some pasted text"));
        assert!(!is_image_path("/tmp/archive.png.zip")); // ext must be trailing
    }

    fn fresh() -> PasteState {
        PasteState::default()
    }

    #[test]
    fn plain_multiline_text_inserts_verbatim_no_attachments() {
        let st = fresh();
        let out = process_paste("line one\nline two\nline three", st);
        assert_eq!(out.insertion, "line one\nline two\nline three");
        assert!(out.state.attachments.is_empty());
        assert_eq!(out.state.next_image_id, 1);
    }

    #[test]
    fn single_image_path_becomes_ref_and_records_attachment() {
        let st = fresh();
        let out = process_paste("/tmp/screenshot.png", st);
        assert_eq!(out.insertion, "[Image #1]");
        assert_eq!(out.state.attachments.len(), 1);
        assert_eq!(out.state.attachments[0].id, 1);
        assert_eq!(out.state.attachments[0].kind, AttachmentKind::Image);
        assert_eq!(out.state.attachments[0].source, "/tmp/screenshot.png");
        assert_eq!(out.state.next_image_id, 2);
    }

    #[test]
    fn counter_increments_across_multiple_images() {
        // newline-separated image paths → two refs, ids 1 and 2.
        let out = process_paste("/a/one.png\n/b/two.jpg", fresh());
        assert_eq!(out.insertion, "[Image #1]\n[Image #2]");
        assert_eq!(out.state.attachments.len(), 2);
        assert_eq!(out.state.next_image_id, 3);
        // a subsequent paste continues from where the counter left off.
        let out2 = process_paste("/c/three.gif", out.state);
        assert_eq!(out2.insertion, "[Image #3]");
        assert_eq!(out2.state.next_image_id, 4);
    }

    #[test]
    fn mixed_image_and_text_lines_keep_text_inline() {
        let out = process_paste("see this:\n/tmp/pic.png\nthanks", fresh());
        assert_eq!(out.insertion, "see this:\n[Image #1]\nthanks");
        assert_eq!(out.state.attachments.len(), 1);
    }

    #[test]
    fn space_separated_finder_paths_split_on_absolute_path_boundary() {
        // Finder drag pastes space-separated absolute paths.
        let out = process_paste("/tmp/a.png /tmp/b.png", fresh());
        assert_eq!(out.insertion, "[Image #1] [Image #2]");
        assert_eq!(out.state.attachments.len(), 2);
    }

    #[test]
    fn apply_block_inserts_text_at_cursor_and_advances() {
        let st = fresh();
        // prompt "ab|cd" (cursor at byte 2), paste "XY".
        let r = apply_paste_block("abcd", 2, "XY", st);
        assert_eq!(r.prompt, "abXYcd");
        assert_eq!(r.cursor, 4);
        assert!(r.state.attachments.is_empty());
    }

    #[test]
    fn apply_block_inserts_image_ref_and_records_attachment() {
        let st = fresh();
        let r = apply_paste_block("", 0, "/tmp/shot.png", st);
        assert_eq!(r.prompt, "[Image #1]");
        assert_eq!(r.cursor, "[Image #1]".len());
        assert_eq!(r.state.attachments.len(), 1);
        assert_eq!(r.state.attachments[0].source, "/tmp/shot.png");
    }

    #[test]
    fn apply_block_multiline_is_a_single_insertion() {
        // The whole multi-line block lands at once — no submit happens here.
        let r = apply_paste_block("> ", 2, "first\nsecond", fresh());
        assert_eq!(r.prompt, "> first\nsecond");
        assert_eq!(r.cursor, "> first\nsecond".len());
    }

    #[test]
    fn rapid_chars_buffer_then_flush_as_one_block() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        // Three chars 1ms apart → one burst.
        assert_eq!(c.push_char('a', base), None);
        assert_eq!(c.push_char('b', base + Duration::from_millis(1)), None);
        assert_eq!(c.push_char('\n', base + Duration::from_millis(2)), None);
        assert_eq!(c.push_char('c', base + Duration::from_millis(3)), None);
        // A quiet tick beyond the window flushes the whole block.
        assert_eq!(
            c.flush_if_idle(base + Duration::from_millis(60)),
            Some("ab\nc".to_string())
        );
        // Buffer is now empty.
        assert_eq!(c.flush_if_idle(base + Duration::from_millis(120)), None);
    }

    #[test]
    fn slow_typing_is_not_coalesced() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        // First char buffers; the SECOND char arrives after the window, so the
        // first char flushes as a 1-char "block" and the second starts anew.
        assert_eq!(c.push_char('x', base), None);
        let flushed = c.push_char('y', base + Duration::from_millis(80));
        assert_eq!(flushed, Some("x".to_string()));
        // 'y' is now the lone buffered char; an idle tick flushes it.
        assert_eq!(
            c.flush_if_idle(base + Duration::from_millis(200)),
            Some("y".to_string())
        );
    }

    #[test]
    fn non_printable_key_flushes_buffer() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        let _ = c.push_char('h', base);
        let _ = c.push_char('i', base + Duration::from_millis(1));
        // A non-printable key (e.g. Left arrow) forces an immediate flush.
        assert_eq!(c.flush_now(), Some("hi".to_string()));
        assert_eq!(c.flush_now(), None);
    }

    #[test]
    fn would_continue_burst_distinguishes_pasted_newline_from_submit() {
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        // Empty buffer → a lone Enter is a deliberate submit, NOT a burst.
        assert!(!c.would_continue_burst(base));
        // After a char arrives, an Enter within the window continues the burst
        // (a pasted newline) ...
        let _ = c.push_char('a', base);
        assert!(c.would_continue_burst(base + Duration::from_millis(2)));
        // ... but an Enter after the window is a deliberate submit.
        assert!(!c.would_continue_burst(base + Duration::from_millis(80)));
    }

    #[test]
    fn pasted_newline_is_buffered_not_submitted() {
        // Models the live path: chars + embedded newline all within the window
        // are one block; a multi-line paste therefore never submits per line.
        let base = Instant::now();
        let mut c = PasteCoalescer::new();
        assert_eq!(c.push_char('a', base), None);
        // The embedded Enter "continues the burst", so the live path pushes
        // '\n' instead of submitting.
        assert!(c.would_continue_burst(base + Duration::from_millis(1)));
        assert_eq!(c.push_char('\n', base + Duration::from_millis(1)), None);
        assert_eq!(c.push_char('b', base + Duration::from_millis(2)), None);
        // The whole block flushes as one unit on the idle tick.
        assert_eq!(
            c.flush_if_idle(base + Duration::from_millis(80)),
            Some("a\nb".to_string())
        );
    }
}
