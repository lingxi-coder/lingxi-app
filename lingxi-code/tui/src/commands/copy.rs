//! `/copy [N]` — copy the most recent assistant text to the system clipboard.
//!
//! 1:1 behavioral port of the no-arg/`N` fast path of
//! `claude-code/src/commands/copy/copy.tsx` (a `local-jsx` command). The TS
//! command:
//!
//! - `collectRecentAssistantTexts(messages)` walks the transcript NEWEST-FIRST,
//!   collecting text from assistant messages that actually said something
//!   (skipping tool-use-only turns and API errors), capped at `MAX_LOOKBACK`
//!   (20). Index 0 = latest, 1 = second-to-latest, …
//! - `/copy` (no arg) copies index 0; `/copy N` copies index `N - 1`.
//! - empty transcript  → `No assistant message to copy`.
//! - a non-integer / `< 1` arg → the `Usage: /copy [N] …` error.
//! - `N` past the end  → `Only K assistant message(s) available to copy`.
//! - success           → the text is copied + a
//!   `Copied to clipboard (C characters, L lines)` confirmation.
//!
//! This module is the PURE arg-parser. It maps the transcript + `args` onto a
//! [`CopyCommand`] outcome carrying the exact display string + (on success) the
//! text to write to the clipboard. The dispatch intercept (`app::dispatch`)
//! applies the effect (push the `SystemText`, raise `pending_copy_clipboard`);
//! the actual clipboard write happens in the `root::pump_copy_clipboard` async
//! pump. No I/O, no `AppState` mutation, no `.await` here — fully unit-testable.
//!
//! ## Spec divergences (noted per the batch brief)
//!
//! - **Code-block selector dialog deferred.** claude-code renders a `<CopyPicker>`
//!   when the chosen response contains code blocks and `copyFullResponse` is
//!   false; this batch ships only the plain full-response copy (bucket-(b) work
//!   for the dialog). We therefore always copy the FULL response text — i.e. the
//!   `config.copyFullResponse === true` branch of the TS `call`.
//! - **`also written to <file>` fallback dropped.** claude-code ALSO writes the
//!   text to `$TMPDIR/claude/response.md` and appends `Also written to <path>`
//!   to the confirmation. That temp-file path is a best-effort fallback for
//!   terminals without OSC-52; the Rust pump shells out to the native clipboard
//!   utility directly (the claude-code "native safety net" path), so the file
//!   fallback is omitted and the confirmation is the bare
//!   `Copied to clipboard (…)` line.
//! - **char count uses Unicode scalar values**, not JS UTF-16 code units. The
//!   line count (`'\n'` occurrences + 1) is byte-faithful; the character count
//!   differs only for astral-plane text and is cosmetic.

use crate::state::RenderedMessage;

/// Newest-first lookback cap — byte-locked to claude-code `MAX_LOOKBACK`.
pub const MAX_LOOKBACK: usize = 20;

/// Walk `messages` NEWEST-FIRST, returning text from assistant messages that
/// actually said something. Index 0 = latest, 1 = second-to-latest, … Caps at
/// [`MAX_LOOKBACK`].
///
/// 1:1 with claude-code `collectRecentAssistantTexts`: that walk keeps
/// `msg.type === 'assistant' && !msg.isApiErrorMessage` turns whose content has
/// non-empty extracted text. In this TUI's `RenderedMessage` model an assistant
/// turn that "said something" is exactly a [`RenderedMessage::AssistantText`]
/// with a non-empty body — tool-use-only turns render as
/// [`RenderedMessage::AssistantToolUse`] (no `AssistantText`) and API errors as
/// [`RenderedMessage::SystemApiError`] / error `SystemText`, so both are
/// naturally excluded by selecting only `AssistantText` bodies.
#[must_use]
pub fn collect_recent_assistant_texts(messages: &[RenderedMessage]) -> Vec<String> {
    let mut texts = Vec::new();
    for msg in messages.iter().rev() {
        if texts.len() >= MAX_LOOKBACK {
            break;
        }
        if let RenderedMessage::AssistantText { body, .. } = msg {
            if !body.is_empty() {
                texts.push(body.clone());
            }
        }
    }
    texts
}

/// Outcome of a `/copy [N]` command — the display message plus (on success) the
/// text to write to the clipboard. Each variant carries its byte-locked
/// `display` so the tests can lock it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyCommand {
    /// Copy succeeds: `text` goes to the clipboard, `display` is the confirmation.
    Copy {
        /// The assistant text to write to the system clipboard.
        text: String,
        /// The `Copied to clipboard (C characters, L lines)` confirmation.
        display: String,
    },
    /// Nothing to copy / bad arg / out-of-range — `display` is the error body.
    Error {
        /// The error message body (rendered as an error `SystemText`).
        display: String,
    },
}

/// Build the `Copied to clipboard (C characters, L lines)` confirmation.
///
/// 1:1 with claude-code `copyOrWriteToFile`: `lineCount = countCharInString(text,
/// '\n') + 1`, `charCount = text.length`. (We drop the `Also written to <path>`
/// suffix — see the module-level divergence note.)
fn copied_confirmation(text: &str) -> String {
    let char_count = text.chars().count();
    let line_count = text.matches('\n').count() + 1;
    format!("Copied to clipboard ({char_count} characters, {line_count} lines)")
}

/// Parse a `/copy [N]` command against the transcript `messages`.
///
/// `args` is the text AFTER the `/copy` command word (may be empty / whitespace).
/// 1:1 with the no-arg/`N` fast path of claude-code's `call` (the
/// `config.copyFullResponse` / no-code-blocks branch).
#[must_use]
pub fn parse_copy_command(messages: &[RenderedMessage], args: &str) -> CopyCommand {
    let texts = collect_recent_assistant_texts(messages);
    if texts.is_empty() {
        return CopyCommand::Error {
            display: "No assistant message to copy".to_string(),
        };
    }

    // `/copy N` reaches back N-1 messages (1 = latest, 2 = second-to-latest, …).
    let mut age: usize = 0;
    let arg = args.trim();
    if !arg.is_empty() {
        // claude-code uses JS `Number(arg)` then `Number.isInteger(n) && n >= 1`.
        // A leading-`+`/exponent/float `arg` is rejected by `parse::<usize>()`,
        // matching the integer-only guard. `0` and negatives are also rejected.
        match arg.parse::<usize>() {
            Ok(n) if n >= 1 => {
                if n > texts.len() {
                    let noun = if texts.len() == 1 { "message" } else { "messages" };
                    return CopyCommand::Error {
                        display: format!(
                            "Only {} assistant {noun} available to copy",
                            texts.len()
                        ),
                    };
                }
                age = n - 1;
            }
            _ => {
                return CopyCommand::Error {
                    display: format!(
                        "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: {arg}"
                    ),
                };
            }
        }
    }

    let text = texts[age].clone();
    let display = copied_confirmation(&text);
    CopyCommand::Copy { text, display }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(body: &str) -> RenderedMessage {
        RenderedMessage::AssistantText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    fn tool_use() -> RenderedMessage {
        RenderedMessage::AssistantToolUse {
            id: protocol::ToolUseId::new(),
            tool: "Read".to_string(),
            input: serde_json::json!({}),
        }
    }

    fn api_error() -> RenderedMessage {
        RenderedMessage::SystemApiError {
            error: "boom".to_string(),
            retry_attempt: 1,
            retry_in_seconds: 1,
            max_retries: 3,
            truncated: false,
        }
    }

    #[test]
    fn collect_orders_newest_first() {
        let msgs = vec![assistant("first"), assistant("second"), assistant("third")];
        let texts = collect_recent_assistant_texts(&msgs);
        assert_eq!(texts, vec!["third", "second", "first"]);
    }

    #[test]
    fn collect_skips_tool_use_and_api_errors_and_non_assistant() {
        let msgs = vec![
            user("hi"),
            assistant("said something"),
            tool_use(),
            api_error(),
            user("more"),
        ];
        // Only the single AssistantText survives the newest-first walk.
        let texts = collect_recent_assistant_texts(&msgs);
        assert_eq!(texts, vec!["said something"]);
    }

    #[test]
    fn collect_skips_empty_assistant_text() {
        let msgs = vec![assistant(""), assistant("real")];
        let texts = collect_recent_assistant_texts(&msgs);
        assert_eq!(texts, vec!["real"]);
    }

    #[test]
    fn collect_caps_at_max_lookback() {
        // 25 assistant turns → only the newest 20 are collected.
        let msgs: Vec<RenderedMessage> = (0..25).map(|i| assistant(&format!("m{i}"))).collect();
        let texts = collect_recent_assistant_texts(&msgs);
        assert_eq!(texts.len(), MAX_LOOKBACK);
        // Newest first: m24 down to m5.
        assert_eq!(texts.first().map(String::as_str), Some("m24"));
        assert_eq!(texts.last().map(String::as_str), Some("m5"));
    }

    #[test]
    fn empty_transcript_returns_nothing_to_copy() {
        let out = parse_copy_command(&[], "");
        assert_eq!(
            out,
            CopyCommand::Error {
                display: "No assistant message to copy".to_string()
            }
        );
    }

    #[test]
    fn no_arg_copies_latest() {
        let msgs = vec![assistant("older"), assistant("newest")];
        let out = parse_copy_command(&msgs, "");
        match out {
            CopyCommand::Copy { text, display } => {
                assert_eq!(text, "newest");
                assert_eq!(display, "Copied to clipboard (6 characters, 1 lines)");
            }
            CopyCommand::Error { display } => panic!("expected Copy, got Error({display:?})"),
        }
    }

    #[test]
    fn arg_n_selects_nth_latest() {
        let msgs = vec![assistant("third"), assistant("second"), assistant("first")];
        // /copy 2 → second-to-latest = "second".
        let out = parse_copy_command(&msgs, " 2");
        match out {
            CopyCommand::Copy { text, .. } => assert_eq!(text, "second"),
            CopyCommand::Error { display } => panic!("expected Copy, got Error({display:?})"),
        }
        // /copy 1 → latest = "first".
        match parse_copy_command(&msgs, "1") {
            CopyCommand::Copy { text, .. } => assert_eq!(text, "first"),
            CopyCommand::Error { display } => panic!("expected Copy, got Error({display:?})"),
        }
    }

    #[test]
    fn arg_out_of_range_singular_and_plural() {
        // One message → singular "message".
        let one = vec![assistant("only")];
        assert_eq!(
            parse_copy_command(&one, "2"),
            CopyCommand::Error {
                display: "Only 1 assistant message available to copy".to_string()
            }
        );
        // Two messages → plural "messages".
        let two = vec![assistant("a"), assistant("b")];
        assert_eq!(
            parse_copy_command(&two, "5"),
            CopyCommand::Error {
                display: "Only 2 assistant messages available to copy".to_string()
            }
        );
    }

    #[test]
    fn arg_non_integer_and_zero_return_usage() {
        let msgs = vec![assistant("x")];
        assert_eq!(
            parse_copy_command(&msgs, "abc"),
            CopyCommand::Error {
                display: "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: abc"
                    .to_string()
            }
        );
        assert_eq!(
            parse_copy_command(&msgs, "0"),
            CopyCommand::Error {
                display: "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: 0".to_string()
            }
        );
        assert_eq!(
            parse_copy_command(&msgs, "-1"),
            CopyCommand::Error {
                display: "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: -1"
                    .to_string()
            }
        );
        // A float is not an integer → rejected (matches Number.isInteger guard).
        assert_eq!(
            parse_copy_command(&msgs, "1.5"),
            CopyCommand::Error {
                display: "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: 1.5"
                    .to_string()
            }
        );
    }

    #[test]
    fn confirmation_counts_lines_and_chars() {
        let msgs = vec![assistant("line1\nline2\nline3")];
        match parse_copy_command(&msgs, "") {
            CopyCommand::Copy { display, .. } => {
                // 17 chars, 3 lines (2 newlines + 1).
                assert_eq!(display, "Copied to clipboard (17 characters, 3 lines)");
            }
            CopyCommand::Error { display } => panic!("expected Copy, got Error({display:?})"),
        }
    }
}
