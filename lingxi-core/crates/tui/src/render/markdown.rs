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
struct InlineState {
    bold: bool,
    italic: bool,
    underline: bool,
    code: bool,
}

impl InlineState {
    fn to_style(self, theme: &MarkdownTheme) -> SpanStyle {
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
    /// Active heading level, set between `Start(Heading)`/`End(Heading)`.
    heading: Option<HeadingLevel>,
    /// Stack of list contexts (outer to inner). `Some(n)` = ordered list at
    /// next item number `n`; `None` = unordered.
    list_stack: Vec<Option<u64>>,
    /// True while inside a blockquote (prefix lines with the bar + italic).
    in_blockquote: bool,
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
            heading: None,
            list_stack: Vec::new(),
            in_blockquote: false,
            code_block: None,
        }
    }

    /// Flush the in-progress line (if any) into `lines`.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut spans = std::mem::take(&mut self.pending);
        if self.in_blockquote {
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
        self.pending
            .push(StyledSpan::styled(text, self.inline.to_style(self.theme)));
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::Strong) => self.inline.bold = true,
            Event::End(TagEnd::Strong) => self.inline.bold = false,
            Event::Start(Tag::Emphasis) => self.inline.italic = true,
            Event::End(TagEnd::Emphasis) => self.inline.italic = false,
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
                self.heading = Some(level);
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
                self.heading = None;
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
                self.in_blockquote = true;
                self.inline.italic = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.in_blockquote = false;
                self.inline.italic = false;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let trimmed = info.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            // info-string may be "rust ignore" — take first word.
                            Some(trimmed.split_whitespace().next().unwrap().to_string())
                        }
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
                    self.lines.push(StyledLine {
                        spans: vec![StyledSpan::code_placeholder(cb.text, cb.lang.as_deref())],
                    });
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<StyledLine> {
        // Defensive: an unterminated fence (streaming) leaves `code_block`
        // set with no `End(CodeBlock)` event — emit its placeholder so the
        // partial code still renders rather than vanishing.
        if let Some(cb) = self.code_block.take() {
            self.lines.push(StyledLine {
                spans: vec![StyledSpan::code_placeholder(cb.text, cb.lang.as_deref())],
            });
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
        let code = lines[0].spans.iter().find(|s| s.text == "cargo test").unwrap();
        assert_eq!(code.style.fg, StyleColor::Named(crate::render::NamedColor::Magenta));
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
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "- one"));
        assert!(texts.iter().any(|t| t == "- two"));
    }

    #[test]
    fn ordered_list_marker() {
        let lines = render("1. first\n2. second", &theme());
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "1. first"));
        assert!(texts.iter().any(|t| t == "2. second"));
    }

    #[test]
    fn nested_list_indents() {
        let lines = render("- a\n  - b", &theme());
        let texts: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        assert!(texts.iter().any(|t| t == "- a"));
        assert!(texts.iter().any(|t| t == "  - b"));
    }

    #[test]
    fn blockquote_has_bar_prefix() {
        let lines = render("> quoted", &theme());
        let line = lines.iter().find(|l| l.plain_text().contains("quoted")).unwrap();
        assert!(line.plain_text().starts_with("│ "));
        let text_span = line.spans.iter().find(|s| s.text.contains("quoted")).unwrap();
        assert!(text_span.style.italic);
    }

    #[test]
    fn fenced_code_emits_placeholder_with_lang() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render(md, &theme());
        // exactly one placeholder span carrying the raw code + lang hint.
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .expect("a CodePlaceholder span");
        assert_eq!(ph.text, "fn main() {}\n");
        assert_eq!(
            ph.kind,
            crate::render::SpanKind::CodePlaceholder { lang: Some("rust".to_string()) }
        );
    }

    #[test]
    fn fenced_code_without_lang_has_none() {
        let md = "```\nplain code\n```";
        let lines = render(md, &theme());
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .unwrap();
        assert_eq!(ph.kind, crate::render::SpanKind::CodePlaceholder { lang: None });
    }

    #[test]
    fn fenced_code_is_not_styled_as_inline() {
        let md = "```js\nconst x = 1;\n```";
        let lines = render(md, &theme());
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }))
            .unwrap();
        // placeholder text is raw — no bold/italic leaked in.
        assert!(!ph.style.bold);
        assert!(!ph.style.italic);
    }

    #[test]
    fn unclosed_code_fence_does_not_panic_and_emits_placeholder() {
        // No closing ``` — streaming mid-block.
        let md = "intro\n```rust\nfn main() {";
        let lines = render(md, &theme());
        // intro paragraph present.
        assert!(lines.iter().any(|l| l.plain_text().contains("intro")));
        // the partial code still becomes a placeholder.
        let ph = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| matches!(s.kind, crate::render::SpanKind::CodePlaceholder { .. }));
        assert!(ph.is_some(), "unclosed fence should still emit a placeholder");
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

    #[test]
    fn snapshot_fenced_code_placeholder() {
        insta::assert_yaml_snapshot!(render("```rust\nfn main() {}\n```", &theme()));
    }

    #[test]
    fn snapshot_partial_unclosed_fence() {
        insta::assert_yaml_snapshot!(render("text\n```python\nprint(1)", &theme()));
    }
}
