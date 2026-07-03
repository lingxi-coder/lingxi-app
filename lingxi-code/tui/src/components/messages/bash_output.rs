//! `UserBashOutputMessage` — stdout/stderr rendered through the ANSI parser.
//!
//! Literal locks / behavior:
//!   - body = `<bash-stdout>` (inner `<persisted-output>` unwrapped) + `<bash-stderr>`
//!   - bash output carries SGR codes → parsed via the ANSI parser, NOT plain text
//!   source: claude-code/src/components/messages/UserBashOutputMessage.tsx
//!           + tools/BashTool/BashToolResultMessage.tsx
//!
//! Uses M7-01's full `render::ansi` parser (256/truecolor) and the same
//! `StyledSpan → iocraft Color` pipeline as `user_tool_result.rs` (via
//! `StyleColor::to_iocraft`). The string-form oracle joins span texts
//! (ANSI escape codes already stripped by the parser).
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::doc_markdown, clippy::doc_lazy_continuation)]

use crate::render_iocraft::StyleColorIocraftExt;
use iocraft::prelude::*;

use crate::render::ansi::parse_ansi;
use crate::render::{split_spans_into_line_rows, StyledSpan};

/// Props for [`UserBashOutputMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserBashOutputProps {
    /// Standard output (ANSI-coded).
    pub stdout: String,
    /// Standard error (ANSI-coded).
    pub stderr: String,
}

/// Join stdout + stderr (stderr after stdout, separated by a newline when both
/// non-empty) into a single body string.
#[must_use]
pub fn join_bash_output(stdout: &str, stderr: &str) -> String {
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(stderr);
    }
    body
}

/// Parse the joined stdout+stderr into a flat span list. Lines are rejoined
/// with `\n`-only spans between them (mirrors `user_tool_result.rs`'s
/// `render_user_tool_result_body_spans` flattening) so a single span pipeline
/// reproduces multi-line bash output.
#[must_use]
pub fn render_bash_output_spans(stdout: &str, stderr: &str) -> Vec<StyledSpan> {
    let body = join_bash_output(stdout, stderr);
    let lines = parse_ansi(&body);
    let mut spans: Vec<StyledSpan> = Vec::new();
    for (li, line) in lines.into_iter().enumerate() {
        if li > 0 {
            spans.push(StyledSpan::plain("\n"));
        }
        spans.extend(line.spans);
    }
    spans
}

/// iocraft component — one row per parsed line, each span a styled `Text`.
///
/// The flat span stream from [`render_bash_output_spans`] rejoins parsed lines
/// with `\n` delimiter spans. [`split_spans_into_line_rows`] recovers the
/// per-line groups; each group becomes its own `FlexDirection::Row` inside a
/// `FlexDirection::Column`, so N-line output renders on N visual rows (a flex
/// **Row** of every span — including the delimiters — collapsed them onto one).
/// An empty body yields an empty Column (no spurious blank row), matching the
/// prior behavior for empty output.
#[component]
pub fn UserBashOutputMessage(props: &UserBashOutputProps) -> impl Into<AnyElement<'static>> {
    let spans = render_bash_output_spans(&props.stdout, &props.stderr);
    let line_rows = split_spans_into_line_rows(spans);
    let rows: Vec<AnyElement<'static>> = line_rows
        .into_iter()
        .map(|line_spans| {
            let span_elements: Vec<AnyElement<'static>> = line_spans
                .into_iter()
                .map(|s| {
                    let color = s.style.fg.to_iocraft();
                    let weight = if s.style.bold {
                        Weight::Bold
                    } else {
                        Weight::Normal
                    };
                    element! { Text(content: s.text, color: color, weight: weight) }.into_any()
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
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_stdout_and_stderr() {
        let spans = render_bash_output_spans("out", "err");
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "out\nerr");
    }

    #[test]
    fn ansi_splits_into_multiple_spans() {
        // SGR 31 (red) "err" reset, then plain "ok".
        let spans = render_bash_output_spans("\x1b[31merr\x1b[0mok", "");
        assert!(spans.len() >= 2, "expected >=2 spans, got {}", spans.len());
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "errok");
    }
}
