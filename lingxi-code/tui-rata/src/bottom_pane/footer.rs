//! The key-hint footer rendered BELOW the composer — codex `bottom_pane/footer.rs`
//! (render layer): pure formatting of `FooterProps` into a dim, 2-column-indented
//! line. Mode selection stays in `BottomPane` (codex keeps it in ChatComposer).

#![allow(dead_code)] // consumed by Task 5

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Codex `FOOTER_INDENT_COLS`.
const FOOTER_INDENT_COLS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FooterMode {
    Idle,
    IdleVerbose,
    CompletionActive,
    CtrlCReminder,
}

pub(crate) struct FooterProps {
    pub mode: FooterMode,
    pub vim_label: Option<String>,
    pub cost: Option<String>,
}

/// Rows the footer occupies (codex `footer_height`; single-line today).
pub(crate) fn footer_height(_props: &FooterProps) -> u16 {
    1
}

/// The footer line: hint copy is byte-identical to the pre-move status row.
pub(crate) fn footer_line(props: &FooterProps, theme: &tui_core::theme::Theme) -> Line<'static> {
    let dim = crate::style_adapter::to_ratatui(theme.dim);
    if props.mode == FooterMode::CtrlCReminder {
        let claude = crate::style_adapter::to_ratatui(theme.claude);
        return Line::from(Span::styled("Press Ctrl-C again to exit", Style::default().fg(claude)));
    }
    let base = match props.mode {
        FooterMode::CompletionActive => "↑/↓: pick  ·  Tab: complete  ·  Esc: dismiss  ·  Enter: run",
        FooterMode::IdleVerbose => "Enter: send  ·  Ctrl-O: collapse  ·  ↑/↓: history  ·  Esc: quit",
        // CtrlCReminder is unreachable here (early return above); listed only
        // to keep the match exhaustive without a wildcard.
        FooterMode::Idle | FooterMode::CtrlCReminder => {
            "Enter: send  ·  Alt+Enter: newline  ·  Ctrl-O: verbose  ·  Esc: quit"
        }
    };
    let text = match &props.vim_label {
        Some(label) => format!("[{label}]  {base}"),
        None => base.to_string(),
    };
    let mut spans = vec![Span::styled(text, Style::default().fg(dim))];
    if let Some(cost) = &props.cost {
        spans.push(Span::styled(format!("  ·  {cost}"), Style::default().fg(dim)));
    }
    Line::from(spans)
}

/// Render the footer with the codex 2-column indent (codex `render_footer_line`).
pub(crate) fn render_footer(area: Rect, buf: &mut Buffer, props: &FooterProps, theme: &tui_core::theme::Theme) {
    let indent = " ".repeat(FOOTER_INDENT_COLS);
    let mut line = footer_line(props, theme);
    line.spans.insert(0, Span::raw(indent));
    Paragraph::new(line).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_idle_line_keeps_the_exact_hint_copy_with_cost() {
        let props = FooterProps {
            mode: FooterMode::Idle,
            vim_label: None,
            cost: Some("$0.0123".into()),
        };
        let line = footer_line(&props, &tui_core::theme::Theme::dark());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Enter: send  ·  Alt+Enter: newline  ·  Ctrl-O: verbose  ·  Esc: quit  ·  $0.0123");
    }

    #[test]
    fn footer_renders_with_two_column_indent() {
        let props = FooterProps { mode: FooterMode::Idle, vim_label: None, cost: None };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_footer(area, &mut buf, &props, &tui_core::theme::Theme::dark());
        let row: String = (0..80).map(|x| buf.cell(ratatui::layout::Position::new(x, 0)).unwrap().symbol().to_string()).collect();
        assert!(row.starts_with("  Enter: send"), "2-col FOOTER_INDENT_COLS prefix: {row:?}");
    }

    #[test]
    fn footer_ctrl_c_reminder_replaces_hints() {
        let props = FooterProps { mode: FooterMode::CtrlCReminder, vim_label: None, cost: Some("$1".into()) };
        let line = footer_line(&props, &tui_core::theme::Theme::dark());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Press Ctrl-C again to exit"); // transient reminder stays clean (no cost suffix)
    }
}
