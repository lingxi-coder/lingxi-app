//! `UserPromptMessage` — prompt text with head+tail truncation.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - MAX_DISPLAY_CHARS = 10_000, TRUNCATE_HEAD_CHARS = TRUNCATE_TAIL_CHARS = 2_500
//!   - truncated form: `{head}\n… +{hiddenLines} lines …\n{tail}`
//!     (U+2026 ellipsis, surrounding spaces)
//!   - SCOPE: plain path only; KAIROS/brief-layout defers to M8.
//!   source: claude-code/src/components/messages/UserPromptMessage.tsx
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

/// Hard cap on displayed prompt text.
pub const MAX_DISPLAY_CHARS: usize = 10_000;
/// Head slice kept on truncation.
pub const TRUNCATE_HEAD_CHARS: usize = 2_500;
/// Tail slice kept on truncation.
pub const TRUNCATE_TAIL_CHARS: usize = 2_500;

/// Props for [`UserPromptMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserPromptProps {
    /// Prompt body text.
    pub text: String,
}

/// Count occurrences of `\n` in the first `up_to` chars of `s`
/// (claude-code `countCharInString(s, '\n', up_to)`).
fn count_newlines_in_prefix(s: &str, up_to: usize) -> usize {
    s.chars().take(up_to).filter(|&c| c == '\n').count()
}

/// Pure string renderer with head+tail truncation. Operates on `char`
/// boundaries to stay UTF-8 safe. Empty text → empty string.
#[must_use]
pub fn render_prompt_to_string(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= MAX_DISPLAY_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(TRUNCATE_HEAD_CHARS).collect();
    let tail: String = text
        .chars()
        .skip(char_count.saturating_sub(TRUNCATE_TAIL_CHARS))
        .collect();
    let hidden_lines = count_newlines_in_prefix(text, TRUNCATE_HEAD_CHARS)
        .saturating_sub(tail.matches('\n').count());
    format!("{head}\n\u{2026} +{hidden_lines} lines \u{2026}\n{tail}")
}

/// iocraft component. Empty text → empty view.
#[component]
pub fn UserPromptMessage(props: &UserPromptProps) -> impl Into<AnyElement<'static>> {
    let content = render_prompt_to_string(&props.text);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: content, color: TuiTheme::USER.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_empty() {
        assert_eq!(render_prompt_to_string(""), "");
    }

    #[test]
    fn short_prompt_unchanged() {
        assert_eq!(render_prompt_to_string("hello"), "hello");
    }

    #[test]
    fn long_prompt_truncates_head_tail() {
        let body = "x".repeat(MAX_DISPLAY_CHARS + 100);
        let s = render_prompt_to_string(&body);
        assert!(s.contains("\u{2026} +"), "expected ellipsis marker");
        assert!(s.contains(" lines \u{2026}"));
        assert!(s.len() < body.len());
    }
}
