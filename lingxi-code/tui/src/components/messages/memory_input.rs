//! `UserMemoryInputMessage` — `# {input}` + a saving acknowledgement line.
//!
//! Literal locks (byte-for-byte from claude-code):
//!   - prefix glyph: `#` (claude-code `remember`). The lingxi `Theme` ports
//!     only the keys it renders and has no `remember` / `memoryBackgroundColor`
//!     fields; the glyph stays `TuiTheme::USER` (terminal default). Dedicated
//!     `remember` / memory-background `Theme` fields are re-deferred — TODO(M8).
//!   - saving line: claude-code samples ['Got it.', 'Good to know.', 'Noted.']
//!     at random; M7-05 PINS the first ("Got it.") for snapshot determinism
//!     (documented divergence — recorded in the M7-16 literal-lock catalog).
//!   source: claude-code/src/components/messages/UserMemoryInputMessage.tsx
#![allow(
    clippy::doc_markdown,
    clippy::doc_lazy_continuation,
    clippy::doc_link_with_quotes
)]

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::theme::TuiTheme;

/// Pinned saving-acknowledgement line (first of claude-code's sample set
/// `['Got it.', 'Good to know.', 'Noted.']`; pinned for snapshot determinism).
pub const SAVING_MESSAGE: &str = "Got it.";

/// Props for [`UserMemoryInputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserMemoryInputProps {
    /// Memory text (from `<user-memory-input>`).
    pub input: String,
}

/// Pure string renderer: `"# {input}\n{SAVING_MESSAGE}"`. Empty input → `""`.
#[must_use]
pub fn render_memory_to_string(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    format!("# {input}\n{SAVING_MESSAGE}")
}

/// iocraft component.
#[component]
pub fn UserMemoryInputMessage(props: &UserMemoryInputProps) -> impl Into<AnyElement<'static>> {
    let head = format!("# {}", props.input);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: head, color: TuiTheme::USER.to_iocraft())
            Text(content: SAVING_MESSAGE, color: TuiTheme::DIM.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_empty() {
        assert_eq!(render_memory_to_string(""), "");
    }

    #[test]
    fn memory_input_form() {
        assert_eq!(
            render_memory_to_string("prefer tabs"),
            "# prefer tabs\nGot it."
        );
    }
}
