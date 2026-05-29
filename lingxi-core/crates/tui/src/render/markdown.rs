//! CommonMark → [`StyledLine`] rendering via `pulldown-cmark`.
//!
//! Pure function: `render(text, theme) -> Vec<StyledLine>`. No iocraft, no
//! terminal, no async — colors are [`StyleColor`] values mapped to iocraft
//! only at draw time (`StyleColor::to_iocraft`).
//!
//! Literal reference: `claude-code/src/utils/markdown.ts` `formatToken`.
//! Handled here: paragraphs, headings, bold (`Strong`), italic (`Emphasis`),
//! inline code (`Code` → `theme.inline_code`), links (`text (url)`),
//! ordered/unordered/nested lists, blockquote, fenced code (emits a
//! [`SpanKind::CodePlaceholder`] span — M7-02 highlights it). Best-effort on
//! partial / unclosed input; never panics.
//!
//! Per claude-code: strikethrough is intentionally NOT parsed (the model
//! uses `~` for "approximately"); HTML/definitions render to nothing.

// `CommonMark`, `pulldown-cmark`, `claude-code` etc. read better unquoted in
// the module prose; suppress the doc-markdown nudge crate-wide for this file.
#![allow(clippy::doc_markdown)]

use crate::render::{SpanStyle, StyleColor, StyledLine, StyledSpan};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Dim vertical bar prefixing blockquote lines. Matches claude-code's
/// `BLOCKQUOTE_BAR` (`src/constants/figures.ts`).
const BLOCKQUOTE_BAR: &str = "│";

/// Theme colors the markdown renderer needs. Kept minimal and decoupled
/// from iocraft so the renderer is a pure value function. Expand in M7-15.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownTheme {
    /// Inline-code (`codespan`) foreground — claude-code uses the
    /// `permission` theme color here.
    pub inline_code: StyleColor,
}

/// Mutable inline styling state threaded through the event walk.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
struct InlineState {
    bold: bool,
    italic: bool,
    underline: bool,
    code: bool,
}

impl InlineState {
    fn to_style(self, theme: MarkdownTheme) -> SpanStyle {
        SpanStyle {
            fg: if self.code {
                theme.inline_code
            } else {
                StyleColor::Default
            },
            bg: StyleColor::Default,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
        }
    }
}

/// Buffered fenced/indented code-block content awaiting M7-02 highlighting.
#[derive(Debug, Default)]
struct CodeBlockState {
    lang: Option<String>,
    text: String,
}

/// Render CommonMark `text` to styled lines using `theme`. Strikethrough is
/// disabled to match claude-code; tables and footnotes are enabled by
/// `pulldown-cmark` defaults but only paragraph/heading/list/quote/code are
/// styled here (others fall through as their inline text).
#[must_use]
pub fn render(text: &str, theme: &MarkdownTheme) -> Vec<StyledLine> {
    let options = Options::ENABLE_TABLES;
    let parser = Parser::new_ext(text, options);

    let mut builder = Builder::new(theme);
    for event in parser {
        builder.handle(event);
    }
    builder.finish()
}

/// Accumulates styled lines while walking markdown events. Inline content is
/// appended to `pending` (the current line being built); block boundaries
/// flush `pending` into `lines`.
struct Builder<'a> {
    theme: &'a MarkdownTheme,
    lines: Vec<StyledLine>,
    pending: Vec<StyledSpan>,
    inline: InlineState,
    link_url: Option<String>,
    /// Stack of list contexts (outer to inner). `Some(n)` = ordered list at
    /// next item number `n`; `None` = unordered.
    list_stack: Vec<Option<u64>>,
    /// Nesting depth of `Emphasis` spans. Italic is active while `> 0`. A
    /// counter (not a bool) so `End(Emphasis)` only clears the emphasis it
    /// owns and never the italic owned by an enclosing blockquote.
    emphasis_depth: u32,
    /// Nesting depth of blockquotes. While `> 0`, lines are prefixed with the
    /// bar and rendered italic. Independent of `emphasis_depth` so the two
    /// concerns can't stomp each other's italic.
    blockquote_depth: u32,
    /// When inside a fenced/indented code block: accumulates raw text and
    /// the language hint. `Some` between `Start(CodeBlock)`/`End(CodeBlock)`.
    code_block: Option<CodeBlockState>,
}

impl<'a> Builder<'a> {
    fn new(theme: &'a MarkdownTheme) -> Self {
        Builder {
            theme,
            lines: Vec::new(),
            pending: Vec::new(),
            inline: InlineState::default(),
            link_url: None,
            list_stack: Vec::new(),
            emphasis_depth: 0,
            blockquote_depth: 0,
            code_block: None,
        }
    }

    /// Flush the in-progress line (if any) into `lines`.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut spans = std::mem::take(&mut self.pending);
        if self.blockquote_depth > 0 {
            let mut prefixed = vec![StyledSpan::styled(
                format!("{BLOCKQUOTE_BAR} "),
                SpanStyle {
                    fg: StyleColor::Named(crate::render::NamedColor::BrightBlack),
                    ..SpanStyle::default()
                },
            )];
            prefixed.append(&mut spans);
            spans = prefixed;
        }
        self.lines.push(StyledLine { spans });
    }

    /// Append a styled-text span to the current line using current inline
    /// state.
    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Effective italic = inline emphasis OR an enclosing blockquote (or an
        // H1 heading's `inline.italic`). Derived here so emphasis and
        // blockquote own independent state and neither clears the other's bit.
        let mut state = self.inline;
        state.italic = state.italic || self.emphasis_depth > 0 || self.blockquote_depth > 0;
        self.pending
            .push(StyledSpan::styled(text, state.to_style(*self.theme)));
    }

    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Strong) => self.inline.bold = true,
            Event::End(TagEnd::Strong) => self.inline.bold = false,
            Event::Start(Tag::Emphasis) => self.emphasis_depth += 1,
            Event::End(TagEnd::Emphasis) => {
                self.emphasis_depth = self.emphasis_depth.saturating_sub(1);
            }
            Event::Code(text) => {
                // inline code: force code color regardless of surrounding em.
                let prev = self.inline.code;
                self.inline.code = true;
                self.push_text(&text);
                self.inline.code = prev;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                // Remember the URL to append after the link text closes.
                self.link_url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = self.link_url.take() {
                    self.push_text(&format!(" ({url})"));
                }
            }
            Event::Text(text) => {
                if let Some(cb) = self.code_block.as_mut() {
                    cb.text.push_str(&text);
                } else {
                    self.push_text(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                self.flush();
            }
            Event::End(TagEnd::Paragraph) => {
                self.flush();
                self.lines.push(StyledLine::empty());
            }
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush();
                match level {
                    HeadingLevel::H1 => {
                        self.inline.bold = true;
                        self.inline.italic = true;
                        self.inline.underline = true;
                    }
                    _ => self.inline.bold = true,
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                self.flush();
                self.inline = InlineState::default();
                self.lines.push(StyledLine::empty());
            }
            Event::Start(Tag::List(first)) => {
                self.list_stack.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                self.list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                self.flush();
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
                self.pending
                    .push(StyledSpan::plain(format!("{indent}{marker}")));
            }
            Event::End(TagEnd::Item) => {
                self.flush();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                self.blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let trimmed = info.trim();
                        // info-string may be "rust ignore" — take first word.
                        // `next()` yields `None` for whitespace-only info,
                        // collapsing to no language hint.
                        trimmed.split_whitespace().next().map(ToString::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_block = Some(CodeBlockState {
                    lang,
                    text: String::new(),
                });
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(cb) = self.code_block.take() {
                    self.emit_code_block(&cb);
                }
            }
            _ => {}
        }
    }

    /// M7-02: route a finished fenced/indented code block through syntect.
    /// `cb.lang` is the fence info-string (e.g. `rust`); empty/unknown → plain
    /// fallback. Emits one [`StyledLine`] per code line. Empty body → nothing.
    fn emit_code_block(&mut self, cb: &CodeBlockState) {
        let lang = crate::render::syntax::detect_language(cb.lang.as_deref(), None);
        let highlighted =
            crate::render::syntax::highlight(&cb.text, lang.as_deref(), &crate::theme::TuiTheme);
        if highlighted.is_empty() {
            // Preserve the previous behavior of an empty block still yielding a
            // (now empty-text) line so spacing/structure is stable.
            self.lines.push(StyledLine {
                spans: vec![StyledSpan::plain(String::new())],
            });
        } else {
            self.lines.extend(highlighted);
        }
    }

    fn finish(mut self) -> Vec<StyledLine> {
        // Defensive: an unterminated fence (streaming) leaves `code_block`
        // set with no `End(CodeBlock)` event — highlight its partial body so
        // the code still renders rather than vanishing.
        if let Some(cb) = self.code_block.take() {
            self.emit_code_block(&cb);
        }
        self.flush();
        // Drop a trailing blank line for tidy output.
        if matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{SpanStyle, StyleColor};

    fn theme() -> MarkdownTheme {
        MarkdownTheme {
            inline_code: StyleColor::Named(crate::render::NamedColor::Magenta),
        }
    }

    #[test]
    fn plain_paragraph() {
        let lines = render("hello world", &theme());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain_text(), "hello world");
        assert_eq!(lines[0].spans[0].style, SpanStyle::default());
    }

    #[test]
    fn bold_text() {
        let lines = render("a **b** c", &theme());
        // "a " | "b"(bold) | " c"
        let bold_span = lines[0].spans.iter().find(|s| s.text == "b").unwrap();
        assert!(bold_span.style.bold);
    }

    #[test]
    fn italic_text() {
        let lines = render("a *b* c", &theme());
        let it = lines[0].spans.iter().find(|s| s.text == "b").unwrap();
        assert!(it.style.italic);
    }

    #[test]
    fn inline_code_uses_theme_color() {
        let lines = render("run `cargo test` now", &theme());
        let code = lines[0]
            .spans
            .iter()
            .find(|s| s.text == "cargo test")
            .unwrap();
        assert_eq!(
            code.style.fg,
            StyleColor::Named(crate::render::NamedColor::Magenta)
        );
    }

    #[test]
    fn link_renders_text_and_url() {
        let lines = render("see [docs](https://x.io)", &theme());
        let joined = lines[0].plain_text();
        assert!(joined.contains("docs"));
        assert!(joined.contains("https://x.io"));
    }

    #[test]
    fn h1_is_bold_italic_underline() {
        let lines = render("# Title", &theme());
        let span = &lines[0].spans[0];
        assert_eq!(span.text, "Title");
        assert!(span.style.bold);
        assert!(span.style.italic);
        assert!(span.style.underline);
    }

    #[test]
    fn h2_is_bold_only() {
        let lines = render("## Sub", &theme());
        let span = &lines[0].spans[0];
        assert!(span.style.bold);
        assert!(!span.style.italic);
        assert!(!span.style.underline);
    }

    #[test]
    fn unordered_list_marker() {
        let lines = render("- one\n- two", &theme());
        let texts: Vec<String> = lines.iter().map(StyledLine::plain_text).collect();
        assert!(texts.iter().any(|t| t == "- one"));
        assert!(texts.iter().any(|t| t == "- two"));
    }

    #[test]
    fn ordered_list_marker() {
        let lines = render("1. first\n2. second", &theme());
        let texts: Vec<String> = lines.iter().map(StyledLine::plain_text).collect();
        assert!(texts.iter().any(|t| t == "1. first"));
        assert!(texts.iter().any(|t| t == "2. second"));
    }

    #[test]
    fn nested_list_indents() {
        let lines = render("- a\n  - b", &theme());
        let texts: Vec<String> = lines.iter().map(StyledLine::plain_text).collect();
        assert!(texts.iter().any(|t| t == "- a"));
        assert!(texts.iter().any(|t| t == "  - b"));
    }

    #[test]
    fn blockquote_has_bar_prefix() {
        let lines = render("> quoted", &theme());
        let line = lines
            .iter()
            .find(|l| l.plain_text().contains("quoted"))
            .unwrap();
        assert!(line.plain_text().starts_with("│ "));
        let text_span = line
            .spans
            .iter()
            .find(|s| s.text.contains("quoted"))
            .unwrap();
        assert!(text_span.style.italic);
    }

    #[test]
    fn emphasis_inside_blockquote_keeps_blockquote_italic() {
        // `> before *em* after`: the blockquote makes the whole line italic.
        // The inner emphasis must not clear the blockquote's italic for the
        // text that follows it.
        let lines = render("> before *em* after", &theme());
        let line = lines
            .iter()
            .find(|l| l.plain_text().contains("after"))
            .unwrap();
        let em = line.spans.iter().find(|s| s.text == "em").unwrap();
        assert!(em.style.italic, "emphasis text must be italic");
        let after = line
            .spans
            .iter()
            .find(|s| s.text.contains("after"))
            .unwrap();
        assert!(
            after.style.italic,
            "text after emphasis must remain italic (blockquote intact)"
        );
        let before = line
            .spans
            .iter()
            .find(|s| s.text.contains("before"))
            .unwrap();
        assert!(before.style.italic, "text before emphasis must be italic");
    }

    // M7-02: fenced code is now routed through syntect at render time, so
    // these blocks emit syntax-highlighted StyledLines, NOT a placeholder
    // span. Parity (§0 Q3): assert STRUCTURE (the code text appears, known
    // langs colorize at least one span), not exact per-token colors.

    #[test]
    fn fenced_rust_block_is_syntax_highlighted() {
        let md = "Here:\n\n```rust\nfn main() {}\n```\n";
        let lines = render(md, &theme());
        // Find the code line "fn main() {}" among the rendered lines.
        let code_line = lines
            .iter()
            .find(|l| l.plain_text().contains("fn main"))
            .expect("code line rendered");
        assert!(
            code_line
                .spans
                .iter()
                .any(|s| s.style.fg != StyleColor::Default),
            "fenced rust is syntax-highlighted, not a plain placeholder"
        );
        // No CodePlaceholder span survives.
        assert!(
            !lines
                .iter()
                .flat_map(|l| &l.spans)
                .any(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. })),
            "placeholder is replaced by highlighted spans"
        );
    }

    #[test]
    fn fenced_unknown_lang_block_is_plain() {
        let md = "```klingon\nQapla'\n```\n";
        let lines = render(md, &theme());
        let code_line = lines
            .iter()
            .find(|l| l.plain_text().contains("Qapla"))
            .expect("code line rendered");
        assert!(
            code_line
                .spans
                .iter()
                .all(|s| s.style.fg == StyleColor::Default),
            "unknown-lang fence falls back to plain"
        );
    }

    #[test]
    fn fenced_no_lang_block_is_plain() {
        let md = "```\nplain code\n```";
        let lines = render(md, &theme());
        let code_line = lines
            .iter()
            .find(|l| l.plain_text().contains("plain code"))
            .expect("code line rendered");
        assert!(code_line
            .spans
            .iter()
            .all(|s| s.style.fg == StyleColor::Default));
    }

    #[test]
    fn unclosed_code_fence_does_not_panic_and_renders_partial() {
        // No closing ``` — streaming mid-block.
        let md = "intro\n```rust\nfn main() {";
        let lines = render(md, &theme());
        // intro paragraph present.
        assert!(lines.iter().any(|l| l.plain_text().contains("intro")));
        // the partial code still renders (now highlighted, not a placeholder).
        assert!(
            lines.iter().any(|l| l.plain_text().contains("fn main")),
            "unclosed fence still renders its partial body"
        );
    }

    #[test]
    fn unclosed_bold_does_not_panic() {
        let lines = render("text **still bold", &theme());
        assert!(lines.iter().any(|l| l.plain_text().contains("still bold")));
    }

    #[test]
    fn dangling_list_item_does_not_panic() {
        let _ = render("- a\n- ", &theme());
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert!(render("", &theme()).is_empty());
    }

    #[test]
    fn lone_special_chars_do_not_panic() {
        let _ = render("*_`#>[](", &theme());
        let _ = render("```", &theme());
    }

    #[test]
    fn snapshot_heading() {
        insta::assert_yaml_snapshot!(render("# Title\n\n## Sub", &theme()));
    }

    #[test]
    fn snapshot_bold_italic() {
        insta::assert_yaml_snapshot!(render("normal **bold** and *italic* mix", &theme()));
    }

    #[test]
    fn snapshot_nested_list() {
        insta::assert_yaml_snapshot!(render("- a\n  - b\n  - c\n- d", &theme()));
    }

    #[test]
    fn snapshot_ordered_list() {
        insta::assert_yaml_snapshot!(render("1. first\n2. second\n3. third", &theme()));
    }

    #[test]
    fn snapshot_blockquote() {
        insta::assert_yaml_snapshot!(render("> quoted line\n> second", &theme()));
    }

    #[test]
    fn snapshot_inline_code() {
        insta::assert_yaml_snapshot!(render("run `cargo test --workspace` now", &theme()));
    }

    #[test]
    fn snapshot_link() {
        insta::assert_yaml_snapshot!(render("see [the docs](https://example.io/guide)", &theme()));
    }

    /// Render fenced-code lines structurally: per span "[C]"/"[P]" (colored /
    /// plain) + the text. M7-02 routes fences through syntect, so concrete
    /// colors are NOT snapshotted (parity §0 Q3) — only which spans colorize.
    fn fence_structure(lines: &[StyledLine]) -> String {
        let mut out = String::new();
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            for s in &l.spans {
                let flag = if s.style.fg == StyleColor::Default {
                    'P'
                } else {
                    'C'
                };
                out.push('[');
                out.push(flag);
                out.push(']');
                out.push_str(&s.text);
            }
        }
        out
    }

    #[test]
    fn snapshot_fenced_code_highlighted() {
        // M7-02: ```rust fence is syntect-highlighted (structure, not color).
        insta::assert_snapshot!(fence_structure(&render(
            "```rust\nfn main() {}\n```",
            &theme()
        )));
    }

    #[test]
    fn snapshot_partial_unclosed_fence() {
        insta::assert_snapshot!(fence_structure(&render(
            "text\n```python\nprint(1)",
            &theme()
        )));
    }
}
