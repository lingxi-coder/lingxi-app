//! First-user-message title extraction — 1:1 port of claude-code's
//! `extractFirstPrompt` + `getFirstMeaningfulUserMessageTextContent`
//! (`claude-code/src/utils/sessionStorage.ts:1725-1812`), with the
//! enriched-picker empty fallback `'(session)'`
//! (`sessionStorage.ts:5052-5054`).
//!
//! `extract_title` feeds the `/resume` picker rows (`loader.rs::list_recent_sessions`),
//! which is the structural equivalent of claude-code's `enrichLog` path: the
//! stored `firstPrompt` is capped at 200 chars (`sessionStorage.ts:1732`) and an
//! empty/absent prompt is shown as `'(session)'` (`sessionStorage.ts:5052-5054`).
//!
//! Sub-rules ported from `getFirstMeaningfulUserMessageTextContent`:
//!  - newline→space + trim + 200-char cap (`extractFirstPrompt`, 1728-1734);
//!  - skip `isMeta` user messages (1750) and `isCompactSummary` (1752) — both
//!    read from `JsonlMessage::extra`;
//!  - iterate ALL `text` blocks across messages, not just the first (1761-1809);
//!  - command-name handling: format custom-with-args as `<name> <args>` instead
//!    of emitting raw `<command-name>` XML (1775-1793);
//!  - bash-input → `! <cmd>` prefix (1797-1800);
//!  - `SKIP_FIRST_PROMPT_PATTERN` skip of leading lowercase XML tags /
//!    `[Request interrupted…]` markers (1804; pattern 125-126).
//!
//! KNOWN DIVERGENCE: `getFirstMeaningfulUserMessageTextContent` (1781) skips
//! BUILT-IN slash commands (e.g. `/model sonnet`) via `builtInCommandNames()`.
//! That registry lives in the `command-api` crate, which the `session` crate
//! does not depend on (adding the dep is out of scope here). We therefore treat
//! every `<command-name>` block as a CUSTOM command: keep it only when it has
//! `<command-args>`, otherwise skip. This matches claude-code for custom
//! commands; a built-in command that carries args would surface here where
//! claude-code would skip it. See the parity report for the follow-up.

use crate::jsonl::schema::JsonlMessage;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

/// Maximum number of `char`s stored for the title before truncation + ellipsis.
///
/// claude-code stores a "reasonably long" version (200 chars) and re-truncates
/// at display time by terminal width (`sessionStorage.ts:1730-1734`).
pub const TITLE_MAX_CHARS: usize = 200;

/// The single-codepoint Unicode ellipsis (U+2026) appended when truncating.
pub const TITLE_ELLIPSIS: char = '…';

/// Fallback title when no meaningful user prompt can be extracted —
/// claude-code's `enrichLog` shows `'(session)'` (`sessionStorage.ts:5052-5054`).
pub const EMPTY_TITLE_FALLBACK: &str = "(session)";

/// `SKIP_FIRST_PROMPT_PATTERN` (`sessionStorage.ts:125-126`): leading lowercase
/// XML-like tag (IDE context, hook output, task notifications, …) or a synthetic
/// `[Request interrupted by user…]` marker.
static SKIP_FIRST_PROMPT_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:\s*<[a-z][\w-]*[\s>]|\[Request interrupted by user[^\]]*\])")
        .expect("SKIP_FIRST_PROMPT_PATTERN is a valid regex")
});

/// `extractTag(text, 'command-name')` (`constants/xml.ts:2`).
static COMMAND_NAME_RE: Lazy<Regex> = Lazy::new(|| tag_regex("command-name"));
/// `extractTag(text, 'command-args')` (`constants/xml.ts:4`).
static COMMAND_ARGS_RE: Lazy<Regex> = Lazy::new(|| tag_regex("command-args"));
/// `extractTag(text, 'bash-input')` (`constants/xml.ts:8`).
static BASH_INPUT_RE: Lazy<Regex> = Lazy::new(|| tag_regex("bash-input"));

/// Build the `extractTag` regex for a fixed tag name: opening tag with optional
/// attributes, non-greedy content, closing tag — case-insensitive
/// (`messages.ts:633-687`). Same-tag nesting (the depth check in the TS) does not
/// occur for the command/bash tags, so the leftmost match is faithful.
fn tag_regex(tag: &str) -> Regex {
    Regex::new(&format!(r"(?i)<{tag}(?:\s+[^>]*?)?>([\s\S]*?)</{tag}>"))
        .expect("tag_regex pattern is valid")
}

/// Returns the session title.
///
/// 1. Selects the first *meaningful* user-message text via
///    [`first_meaningful_user_text`] (port of
///    `getFirstMeaningfulUserMessageTextContent`).
/// 2. Post-processes it (`extractFirstPrompt`): newline→space, trim, 200-char cap.
/// 3. Falls back to [`EMPTY_TITLE_FALLBACK`] (`'(session)'`) when nothing
///    meaningful is found OR the processed text is empty.
#[must_use]
pub fn extract_title(messages: &[JsonlMessage]) -> String {
    let title = match first_meaningful_user_text(messages) {
        Some(text) => post_process(&text),
        None => String::new(),
    };
    if title.is_empty() {
        EMPTY_TITLE_FALLBACK.to_string()
    } else {
        title
    }
}

/// Port of `getFirstMeaningfulUserMessageTextContent`
/// (`sessionStorage.ts:1746-1812`). Returns the raw selected text (pre
/// newline-collapse / cap) or `None`.
fn first_meaningful_user_text(messages: &[JsonlMessage]) -> Option<String> {
    for m in messages {
        // `if (msg.type !== 'user' || msg.isMeta) continue` (1750).
        if m.message_type != "user" || extra_flag(m, "isMeta") {
            continue;
        }
        // `if ('isCompactSummary' in msg && msg.isCompactSummary) continue` (1752).
        if extra_flag(m, "isCompactSummary") {
            continue;
        }

        // `const content = msg.message?.content; if (!content) continue` (1754).
        let Some(content) = m.message.get("content") else {
            continue;
        };

        // Collect ALL text blocks (string → [content]; array → each text block).
        for text in collect_texts(content) {
            // `if (!textContent) continue` (1773): empty strings are skipped.
            if text.is_empty() {
                continue;
            }

            // Command-name handling (1775-1793). Without the built-in registry we
            // treat every command as custom: keep only if it carries args.
            if let Some(command_name) = extract_tag(&text, &COMMAND_NAME_RE) {
                let args = extract_tag(&text, &COMMAND_ARGS_RE)
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty());
                match args {
                    Some(args) => return Some(format!("{command_name} {args}")),
                    None => continue,
                }
            }

            // Bash input → `! <cmd>` (1797-1800), checked before the XML skip.
            if let Some(bash) = extract_tag(&text, &BASH_INPUT_RE) {
                return Some(format!("! {bash}"));
            }

            // Skip leading-XML / interrupt markers (1804).
            if SKIP_FIRST_PROMPT_PATTERN.is_match(&text) {
                continue;
            }

            return Some(text);
        }
    }
    None
}

/// `extractFirstPrompt` post-processing (`sessionStorage.ts:1728-1734`):
/// `result.replace(/\n/g,' ').trim()`, then cap at 200 chars with `'…'`.
fn post_process(text: &str) -> String {
    let collapsed = text.replace('\n', " ");
    truncate_with_ellipsis(collapsed.trim())
}

/// Normalize a SIDE-MAP title (`custom-title` / `ai-title` / `summary`) with the
/// exact same rule [`extract_title`] applies to a first-user-message prompt:
/// newline → space, `trim`, then the [`TITLE_MAX_CHARS`]-char cap +
/// [`TITLE_ELLIPSIS`]. The resume picker resolves
/// custom-title > ai-title > summary > first-prompt
/// ([`crate::jsonl::loader::collect_dir`]) and runs the winning side-map title
/// through this so every `SessionMetadata::title`, regardless of source, shares
/// one truncation/ellipsis contract (the display surfaces then re-truncate by
/// terminal width). Mirrors claude-code's `getLogDisplayTitle`
/// (`utils/log.ts:30`), which renders `customTitle || summary || firstPrompt`
/// through one `.trim()`-and-cap pipeline; the stored side-map titles are NOT
/// run through the `<command-name>`/`bash-input`/skip-XML transforms (those are
/// first-prompt-only, `getFirstMeaningfulUserMessageTextContent`).
#[must_use]
pub(crate) fn truncate_title(text: &str) -> String {
    post_process(text)
}

/// Collect text from message content. String → one element; array → the `text`
/// of every `type: "text"` block (1761-1770); anything else → empty.
fn collect_texts(content: &Value) -> Vec<String> {
    if let Some(s) = content.as_str() {
        vec![s.to_string()]
    } else if let Some(arr) = content.as_array() {
        arr.iter()
            .filter_map(|block| {
                let ty = block.get("type").and_then(Value::as_str)?;
                if ty == "text" {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// `extractTag` (`messages.ts:633-687`): returns the inner content of the first
/// `<tag …>…</tag>` match, or `None` when absent or empty (TS returns content
/// only when truthy).
fn extract_tag(text: &str, re: &Regex) -> Option<String> {
    let inner = re.captures(text)?.get(1)?.as_str();
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// Read a boolean outer field (e.g. `isMeta`, `isCompactSummary`) from
/// `JsonlMessage::extra`; missing / non-bool ⇒ `false`.
fn extra_flag(m: &JsonlMessage, key: &str) -> bool {
    m.extra.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Truncate `s` to [`TITLE_MAX_CHARS`] `char`s, trimming the slice and appending
/// [`TITLE_ELLIPSIS`] when anything was cut — `slice(0,200).trim() + '…'`
/// (`sessionStorage.ts:1733`).
#[must_use]
fn truncate_with_ellipsis(s: &str) -> String {
    if s.chars().count() <= TITLE_MAX_CHARS {
        return s.to_string();
    }
    // Byte index of the (TITLE_MAX_CHARS)th char → slice on a valid UTF-8 boundary.
    let cutoff_byte = s
        .char_indices()
        .nth(TITLE_MAX_CHARS)
        .map_or_else(|| s.len(), |(idx, _)| idx);
    let mut out = s[..cutoff_byte].trim().to_string();
    out.push(TITLE_ELLIPSIS);
    out
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a `user` JSONL message with the given inner `content`.
    fn user(content: Value) -> JsonlMessage {
        serde_json::from_value(json!({
            "type": "user",
            "uuid": "11111111-1111-1111-1111-111111111111",
            "parentUuid": null,
            "sessionId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp",
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": content}
        }))
        .expect("valid user message")
    }

    /// Build a `user` message with extra outer fields merged in (e.g. isMeta).
    fn user_with(content: Value, extra: Value) -> JsonlMessage {
        let mut base = json!({
            "type": "user",
            "uuid": "11111111-1111-1111-1111-111111111111",
            "parentUuid": null,
            "sessionId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp",
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": content}
        });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        serde_json::from_value(base).expect("valid user message")
    }

    #[test]
    fn short_string_is_returned_verbatim() {
        assert_eq!(extract_title(&[user(json!("hello world"))]), "hello world");
    }

    #[test]
    fn newline_is_replaced_with_space() {
        assert_eq!(
            extract_title(&[user(json!("line1\nline2\nline3"))]),
            "line1 line2 line3"
        );
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(extract_title(&[user(json!("   hi   "))]), "hi");
    }

    #[test]
    fn cap_is_200_not_50() {
        // 120 chars must survive verbatim (old 50-char cap would have truncated).
        let s = "a".repeat(120);
        assert_eq!(extract_title(&[user(json!(s.clone()))]), s);
    }

    #[test]
    fn exactly_200_chars_no_ellipsis() {
        let s = "a".repeat(200);
        let title = extract_title(&[user(json!(s.clone()))]);
        assert_eq!(title, s);
        assert!(!title.ends_with('…'));
    }

    #[test]
    fn over_200_chars_is_truncated_with_ellipsis() {
        let s = "a".repeat(250);
        let title = extract_title(&[user(json!(s))]);
        assert_eq!(title.chars().count(), 201); // 200 'a' + '…'
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().take(200).collect::<String>(), "a".repeat(200));
    }

    #[test]
    fn cap_counts_chars_not_bytes() {
        let s = "中".repeat(260); // 3 bytes each
        let title = extract_title(&[user(json!(s))]);
        assert_eq!(title.chars().count(), 201); // 200 '中' + '…'
        assert!(title.ends_with('…'));
    }

    #[test]
    fn empty_messages_fall_back_to_session() {
        let messages: Vec<JsonlMessage> = vec![];
        assert_eq!(extract_title(&messages), "(session)");
    }

    #[test]
    fn no_user_message_falls_back_to_session() {
        let mut m = user(json!("hi"));
        m.message_type = "assistant".to_string();
        assert_eq!(extract_title(&[m]), "(session)");
    }

    #[test]
    fn empty_content_falls_back_to_session() {
        assert_eq!(extract_title(&[user(json!(""))]), "(session)");
    }

    #[test]
    fn image_only_array_falls_back_to_session() {
        let m = user(json!([
            {"type": "image", "source": {"type": "base64", "data": "xxxx"}}
        ]));
        assert_eq!(extract_title(&[m]), "(session)");
    }

    #[test]
    fn array_uses_first_text_block() {
        let m = user(json!([
            {"type": "text", "text": "first"},
            {"type": "text", "text": "second"}
        ]));
        assert_eq!(extract_title(&[m]), "first");
    }

    #[test]
    fn skips_assistant_to_find_user() {
        let mut assistant = user(json!("I am the model"));
        assistant.message_type = "assistant".to_string();
        let real = user(json!("real prompt"));
        assert_eq!(extract_title(&[assistant, real]), "real prompt");
    }

    #[test]
    fn is_meta_user_message_is_skipped() {
        let meta = user_with(json!("meta noise"), json!({"isMeta": true}));
        let real = user(json!("actual question"));
        assert_eq!(extract_title(&[meta, real]), "actual question");
    }

    #[test]
    fn is_compact_summary_is_skipped() {
        let summary = user_with(json!("compacted history"), json!({"isCompactSummary": true}));
        let real = user(json!("the real prompt"));
        assert_eq!(extract_title(&[summary, real]), "the real prompt");
    }

    #[test]
    fn custom_command_with_args_is_formatted() {
        let m = user(json!(
            "<command-name>/review</command-name><command-args>reticulate splines</command-args>"
        ));
        assert_eq!(extract_title(&[m]), "/review reticulate splines");
    }

    #[test]
    fn command_without_args_is_skipped() {
        // No <command-args> ⇒ treated as a content-free command ⇒ skipped ⇒ fallback.
        let m = user(json!("<command-name>/clear</command-name>"));
        assert_eq!(extract_title(&[m]), "(session)");
    }

    #[test]
    fn command_without_args_falls_through_to_next_text_block() {
        let m = user(json!([
            {"type": "text", "text": "<command-name>/clear</command-name>"},
            {"type": "text", "text": "follow-up prompt"}
        ]));
        assert_eq!(extract_title(&[m]), "follow-up prompt");
    }

    #[test]
    fn bash_input_gets_bang_prefix() {
        let m = user(json!("<bash-input>ls -la</bash-input>"));
        assert_eq!(extract_title(&[m]), "! ls -la");
    }

    #[test]
    fn skip_pattern_drops_leading_xml_tag() {
        let m = user(json!([
            {"type": "text", "text": "<ide_selection>foo.rs:1-2</ide_selection>"},
            {"type": "text", "text": "what does this do?"}
        ]));
        assert_eq!(extract_title(&[m]), "what does this do?");
    }

    #[test]
    fn skip_pattern_drops_interrupt_marker() {
        let m = user(json!("[Request interrupted by user for tool use]"));
        assert_eq!(extract_title(&[m]), "(session)");
    }

    // ---- truncate_title: side-map titles share extract_title's cap rule -----

    #[test]
    fn truncate_title_short_is_verbatim() {
        assert_eq!(truncate_title("Refactor the parser"), "Refactor the parser");
    }

    #[test]
    fn truncate_title_collapses_newlines_and_trims() {
        // Same `extractFirstPrompt` post-processing extract_title applies.
        assert_eq!(truncate_title("  line1\nline2  "), "line1 line2");
    }

    #[test]
    fn truncate_title_caps_at_200_chars_with_ellipsis() {
        let title = truncate_title(&"a".repeat(250));
        assert_eq!(title.chars().count(), 201); // 200 + '…'
        assert!(title.ends_with('…'));
        // Identical contract to extract_title's first-prompt truncation.
        assert_eq!(title, extract_title(&[user(json!("a".repeat(250)))]));
    }
}
