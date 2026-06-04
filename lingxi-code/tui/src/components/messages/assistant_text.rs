//! `AssistantTextMessage` — primary assistant body rendered through the
//! markdown renderer with a `●` marker to its left.
//!
//! Literal locks (plan §T0):
//!   L4 dot marker = "● " (U+25CF + space) — claude-code parity. The marker is
//!   a SEPARATE cell to the LEFT of the body column (claude-code's `minWidth=2`
//!   `<Box>` sibling of the `<Markdown>` column), NOT a per-line body prefix.
//!
//! claude-code reference (`AssistantTextMessage.tsx` default case):
//!   `<Box flexDirection="row">`
//!     `{shouldShowDot && <Box minWidth={2}><Text color="text">●</Text></Box>}`
//!     `<Box flexDirection="column"><Markdown>{text}</Markdown></Box>`
//!   `</Box>`
//!
//! Color rules (1:1 with `src/utils/markdown.ts` `formatToken`):
//!   - marker color = theme `text` (white in dark), NOT the claude accent.
//!   - body carries NO blanket foreground — paragraph/heading/em/strong/list
//!     emit only bold/italic/underline with terminal-default fg; ONLY inline
//!     code (`codespan`) gets a color (`color('permission', theme)` —
//!     `markdown.ts:88-91`). This matches `render::markdown`'s `InlineState`,
//!     so the renderer is already 1:1; we only wire `inline_code` to
//!     `permission`.
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use iocraft::prelude::*;

use crate::render::markdown::{render_with_width as render_markdown_width, MarkdownTheme};
use crate::render::{StyleColor, StyledLine};
use crate::theme::Theme;

/// (A2) Fallback markdown width when the caller threads no terminal width
/// (`width == 0`). Matches `render::markdown`'s default so the body wraps
/// identically to the pure `render` path. Only table blocks consult width.
const DEFAULT_BODY_WIDTH: usize = 80;

/// Resolve the effective markdown width: a `0` prop (the [`Default`] for the
/// component) means "use the default" so existing callers behave unchanged.
fn effective_width(width: usize) -> usize {
    if width == 0 {
        DEFAULT_BODY_WIDTH
    } else {
        width
    }
}

/// Locked dot marker — "● " (U+25CF + ASCII space). Sits once to the LEFT of
/// the body column (claude-code `minWidth={2}` sibling box), NOT per line.
pub const MARKER: &str = "\u{25CF} ";

/// Continuation indent (2 spaces) matching the marker's display width, so the
/// string oracle's wrapped/continuation rows align under the body column.
const CONT_INDENT: &str = "  ";

/// Props for `AssistantTextMessage`.
#[derive(Default, Props)]
pub struct AssistantTextMessageProps {
    /// Assistant body — rendered as markdown (multi-line; streaming-safe).
    pub body: String,
    /// (A2) Available render width in display columns for table layout. `0`
    /// (the default) means use [`DEFAULT_BODY_WIDTH`]; the live scrollback
    /// threads the real viewport width so markdown tables fit the terminal.
    pub width: usize,
}

/// `MarkdownTheme` for the STYLED assistant body. Unlike the plain-text
/// oracles (which drop color and use `StyleColor::Default`), the styled path
/// preserves the inline-code color per claude-code `formatToken`
/// (`markdown.ts:88-91` — `codespan` → `color('permission', theme)`).
///
/// The dark `permission` palette is `rgb(177, 185, 249)`
/// (`theme.ts` `darkTheme`). `AssistantTextMessage` takes no theme prop today,
/// so the minimal change keeps the dark default; threading the live theme so
/// the picker recolors inline-code is a documented follow-up.
fn markdown_theme() -> MarkdownTheme {
    MarkdownTheme {
        // markdown.ts:88-91 — codespan foreground = permission color.
        inline_code: StyleColor::Rgb(177, 185, 249),
        code_theme: crate::theme::ThemeName::Dark,
    }
}

/// Pure string oracle: flatten the markdown body to plain text, prefix the
/// first line with [`MARKER`] and indent continuation lines by 2 spaces (the
/// marker cell is `minWidth=2` in claude-code, so the body column starts 2
/// columns in). Used by the `render_entry_to_string` dispatcher and the
/// `measured_height` proxy so measurement counts the SAME markdown-FLATTENED
/// rows the component draws (a raw-body proxy over-counts dropped ``` fence
/// rows / trailing blanks once markdown is applied → scroll desync).
///
/// An empty flattened body still yields the marker (matches the prior
/// always-show-marker behavior; the dispatcher only emits this variant for
/// non-empty assistant text).
///
/// (A2) `width` is the markdown table layout width (0 → [`DEFAULT_BODY_WIDTH`]);
/// the measurement oracle MUST pass the SAME width the component uses so the
/// flattened-row count stays in lock-step with what is drawn. For bodies
/// without a markdown table this is width-independent (only tables consult it),
/// so the existing default-width callers are unaffected.
#[must_use]
pub fn render_assistant_text_to_string(body: &str, width: usize) -> String {
    let theme = markdown_theme();
    let flat = render_markdown_width(body, &theme, effective_width(width))
        .iter()
        .map(StyledLine::plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    if flat.is_empty() {
        return MARKER.to_string();
    }
    let mut out = String::new();
    for (i, line) in flat.lines().enumerate() {
        if i == 0 {
            out.push_str(MARKER);
        } else {
            out.push('\n');
            out.push_str(CONT_INDENT);
        }
        out.push_str(line);
    }
    out
}

/// iocraft component — a `Row` of [marker cell | markdown body column].
///
/// The marker is drawn ONCE (theme `text` color) to the left of the body
/// column, mirroring claude-code's `minWidth=2` sibling box. The body column
/// is one `Row` per [`StyledLine`] from `render::markdown`; each span becomes a
/// styled `Text` (per-span fg via [`StyleColor::to_iocraft`], plus bold /
/// italic / underline from the span's style). This reproduces `formatToken`:
/// bold/italic/underline per element, inline-code in the permission color,
/// everything else terminal-default. Streaming reuses this exact path (the
/// body just grows each frame; `render::markdown` is streaming-safe).
#[component]
pub fn AssistantTextMessage(props: &AssistantTextMessageProps) -> impl Into<AnyElement<'static>> {
    let lines = render_markdown_width(&props.body, &markdown_theme(), effective_width(props.width));
    let rows: Vec<AnyElement<'static>> = lines
        .into_iter()
        .map(|line| {
            let span_elements: Vec<AnyElement<'static>> = line
                .spans
                .into_iter()
                .map(|s| {
                    let color = s.style.fg.to_iocraft();
                    let weight = if s.style.bold {
                        Weight::Bold
                    } else {
                        Weight::Normal
                    };
                    let decoration = if s.style.underline {
                        TextDecoration::Underline
                    } else {
                        TextDecoration::None
                    };
                    element! {
                        Text(
                            content: s.text,
                            color: color,
                            weight: weight,
                            italic: s.style.italic,
                            decoration: decoration,
                        )
                    }
                    .into_any()
                })
                .collect();
            element! {
                View(flex_direction: FlexDirection::Row) {
                    #(span_elements)
                }
            }
            .into_any()
        })
        .collect();
    // Marker color = theme `text` (white in dark) — claude-code uses
    // `color="text"` for the dot, NOT the claude accent.
    let marker_color = Theme::dark().text;
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: MARKER, color: marker_color)
            View(flex_direction: FlexDirection::Column) {
                #(rows)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes() {
        // U+25CF = 0xE2 0x97 0x8F, then ASCII space.
        assert_eq!(MARKER.as_bytes(), &[0xE2, 0x97, 0x8F, 0x20]);
    }

    #[test]
    fn oracle_prefixes_first_line_with_marker() {
        let s = render_assistant_text_to_string("hello", 0);
        assert!(s.starts_with(MARKER), "got: {s:?}");
        assert_eq!(s, "\u{25CF} hello");
    }

    #[test]
    fn oracle_indents_continuation_lines() {
        // Two paragraphs flatten to "a", "", "b" (blank between paragraphs).
        let s = render_assistant_text_to_string("a\n\nb", 0);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], "\u{25CF} a");
        // Continuation lines are indented 2 to align under the body column.
        for cont in &lines[1..] {
            assert!(
                cont.is_empty() || cont.starts_with("  "),
                "continuation line not indented: {cont:?}"
            );
        }
    }

    #[test]
    fn oracle_flattens_bold_to_plain() {
        // Bold styling is dropped in the plain oracle; the literal text stays.
        let s = render_assistant_text_to_string("a **b** c", 0);
        assert_eq!(s, "\u{25CF} a b c");
    }

    #[test]
    fn oracle_flattens_list() {
        let s = render_assistant_text_to_string("- one\n- two", 0);
        assert!(s.contains("- one"), "got: {s:?}");
        assert!(s.contains("- two"), "got: {s:?}");
        assert!(s.starts_with(MARKER));
    }

    #[test]
    fn oracle_flattens_fenced_code_dropping_fences() {
        // The ``` fence lines are NOT part of the flattened output (markdown
        // renders the body, not the fence syntax).
        let s = render_assistant_text_to_string("intro\n```rust\nlet x = 1;\n```\noutro", 0);
        assert!(!s.contains("```"), "fences must be dropped: {s:?}");
        assert!(s.contains("let x = 1;"), "got: {s:?}");
        assert!(s.contains("intro"));
        assert!(s.contains("outro"));
    }

    #[test]
    fn empty_body_yields_marker() {
        assert_eq!(render_assistant_text_to_string("", 0), MARKER);
    }

    #[test]
    fn inline_code_uses_permission_color() {
        // The STYLED renderer (not the plain oracle) colors inline code with
        // the dark `permission` palette rgb(177,185,249) per markdown.ts:88-91.
        let lines = render_markdown_width("run `cargo test`", &markdown_theme(), DEFAULT_BODY_WIDTH);
        let code = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text == "cargo test")
            .expect("inline-code span present");
        assert_eq!(code.style.fg, StyleColor::Rgb(177, 185, 249));
    }
}
