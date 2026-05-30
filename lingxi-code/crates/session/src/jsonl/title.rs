//! First-user-message title extraction — 1:1 port of
//! `claude-code/src/utils/sessionStorage.ts::enrichLogs::firstPrompt` derivation.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-08-resume.md` Task 0 step 3 for byte-locks.

use crate::jsonl::schema::JsonlMessage;

/// Maximum number of `char`s in the displayed title before truncation + ellipsis.
pub const TITLE_MAX_CHARS: usize = 50;

/// The single-codepoint Unicode ellipsis (U+2026) appended when truncating.
pub const TITLE_ELLIPSIS: char = '…';

/// Returns the session title — the first user message's text content, trimmed and
/// truncated to [`TITLE_MAX_CHARS`] chars with [`TITLE_ELLIPSIS`] appended if longer.
///
/// Algorithm (locked against `claude-code` `enrichLogs::firstPrompt`):
/// 1. Find first `m` where `m.message_type == "user"` AND `m.message.role == "user"`.
/// 2. Extract `content`:
///    - JSON string → use directly.
///    - JSON array → first element with `type == "text"` → use its `text` field.
///    - Anything else → empty string.
/// 3. `trim()` the result.
/// 4. If > [`TITLE_MAX_CHARS`] `char`s, truncate to that boundary and append [`TITLE_ELLIPSIS`].
#[must_use]
pub fn extract_title(messages: &[JsonlMessage]) -> String {
    for m in messages {
        if m.message_type != "user" {
            continue;
        }
        // `JsonlMessage::message` is `serde_json::Value` per M5-07 T1 step 5.
        let role = m.message.get("role").and_then(serde_json::Value::as_str);
        if role != Some("user") {
            continue;
        }
        let Some(content) = m.message.get("content") else {
            continue;
        };
        let raw = if let Some(s) = content.as_str() {
            s.to_string()
        } else if let Some(arr) = content.as_array() {
            let first_text = arr.iter().find_map(|block| {
                let ty = block.get("type").and_then(serde_json::Value::as_str)?;
                if ty == "text" {
                    block
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            });
            first_text.unwrap_or_default()
        } else {
            String::new()
        };
        return truncate_with_ellipsis(raw.trim());
    }
    String::new()
}

/// Truncate `s` to [`TITLE_MAX_CHARS`] `char`s, appending [`TITLE_ELLIPSIS`] if anything was cut.
#[must_use]
fn truncate_with_ellipsis(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= TITLE_MAX_CHARS {
        return s.to_string();
    }
    // Find the byte index of the (TITLE_MAX_CHARS)th char so we slice on a valid UTF-8 boundary.
    let cutoff_byte = s
        .char_indices()
        .nth(TITLE_MAX_CHARS)
        .map_or_else(|| s.len(), |(idx, _)| idx);
    let mut out = String::with_capacity(cutoff_byte + TITLE_ELLIPSIS.len_utf8());
    out.push_str(&s[..cutoff_byte]);
    out.push(TITLE_ELLIPSIS);
    out
}
