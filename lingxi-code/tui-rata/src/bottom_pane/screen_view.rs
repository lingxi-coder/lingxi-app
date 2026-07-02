//! Read-only full-frame screens (`/help`, `/doctor`, `/mcp`, `/hooks`,
//! `/agents`), ported from the former `screens::FullScreen` into a stacked
//! [`BottomPaneView`] (plan Phase 4).
//!
//! A [`ScreenView`] is a scrollable, read-only page drawn over the whole
//! bottom viewport: a bold title, a body of pre-styled [`Line`]s, and a dim
//! footer hint. Pushed onto the view stack it owns the keyboard while open
//! (scroll keys move the window; `Esc`/`q` close) and suppresses the status
//! line ([`BottomPaneView::wants_status_line`] → `false`).
//!
//! `/help` keeps the iocraft `screens::help` (claude-code `HelpV2`) shape — a
//! `Shortcuts` section and a `Slash commands` section — but the shortcuts
//! list the chords THIS backend implements and the slash-command listing
//! derives from the single [`crate::command::BUILTIN`] registry (plan
//! Phase 8), so `/help` can never advertise a dead chord or command.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;
use crate::session::{DoctorInfo, InfoRow};

/// Column width the shortcut/command key is padded to (iocraft `help::KEY_WIDTH`).
const KEY_WIDTH: usize = 16;

/// A scrollable, read-only full-frame screen.
pub struct ScreenView {
    title: String,
    lines: Vec<Line<'static>>,
    footer: String,
    /// Lines scrolled down from the top (`0` = top).
    scroll: u16,
}

impl ScreenView {
    /// Build a screen from a title, pre-styled body lines, and a footer hint.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        lines: Vec<Line<'static>>,
        footer: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            lines,
            footer: footer.into(),
            scroll: 0,
        }
    }

    /// The `/help` screen (shortcuts + slash commands).
    #[must_use]
    pub fn help() -> Self {
        Self::new(
            concat!("LingXi v", env!("CARGO_PKG_VERSION")),
            help_lines(),
            "esc to close · ↑/↓ scroll",
        )
    }

    /// The `/doctor` diagnostics screen, built from a captured [`DoctorInfo`].
    #[must_use]
    pub fn doctor(d: &DoctorInfo) -> Self {
        Self::new("Doctor", doctor_lines(d), "esc to close · ↑/↓ scroll")
    }

    /// A read-only listing screen (`/mcp`, `/hooks`, `/agents`): a section
    /// header, then one entry per [`InfoRow`] (bold title + optional dim
    /// detail), or a dim "none" line when the list is empty.
    #[must_use]
    pub fn from_rows(
        title: impl Into<String>,
        section: &str,
        rows: &[InfoRow],
        empty: &str,
    ) -> Self {
        let mut lines = vec![header(section), Line::from("")];
        if rows.is_empty() {
            lines.push(Line::from(Span::styled(
                empty.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for r in rows {
                lines.push(Line::from(Span::styled(
                    r.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                if let Some(detail) = &r.detail {
                    lines.push(Line::from(Span::styled(
                        format!("  {detail}"),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
            }
        }
        Self::new(title, lines, "esc to close · ↑/↓ scroll")
    }

    /// Current scroll offset (top line index). Exposed for tests.
    #[must_use]
    pub fn scroll(&self) -> u16 {
        self.scroll
    }
}

impl Renderable for ScreenView {
    /// Draw the screen over the whole `area`, clearing the buffer beneath it.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .title(Span::styled(
                self.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::from(Span::styled(
                self.footer.clone(),
                Style::default().add_modifier(Modifier::DIM),
            )));
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(self.lines.clone())
            .scroll((self.scroll, 0))
            .render(inner, buf);
    }

    /// Body lines + the border rows (the paragraph never soft-wraps, so the
    /// height is width-independent). The bottom viewport clamps this to its
    /// maximum, and the body scrolls within what it gets.
    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for ScreenView {
    /// Route a key: scroll keys move the window, `Esc`/bare `q` close.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        let max = u16::try_from(self.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1).min(max);
                ViewOutcome::Pending
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                ViewOutcome::Pending
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(max);
                ViewOutcome::Pending
            }
            KeyCode::Home => {
                self.scroll = 0;
                ViewOutcome::Pending
            }
            KeyCode::End => {
                self.scroll = max;
                ViewOutcome::Pending
            }
            _ => ViewOutcome::Pending,
        }
    }

    /// A full-frame screen owns the whole viewport: no status row, no
    /// composer beneath.
    fn wants_status_line(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    ))
}

fn row(key: &str, label: &str) -> Line<'static> {
    let pad = KEY_WIDTH.saturating_sub(key.chars().count());
    Line::from(vec![
        Span::styled(
            format!("{key}{}", " ".repeat(pad)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {label}")),
    ])
}

/// Shortcut rows `(chord, label)`: the chords this ratatui backend actually
/// implements (composer + pane routing). The former iocraft-ported list
/// advertised chords with no handler here (`!` bash mode, `ctrl+t` tasks,
/// `meta+p`, `/keybindings`, …) — plan Phase 8 replaces it with the honest
/// set so `/help` never advertises a dead chord.
const SHORTCUTS: &[(&str, &str)] = &[
    ("/", "for commands"),
    ("@", "for file paths"),
    ("enter", "to send"),
    ("alt + enter", "for a newline"),
    ("tab", "to complete"),
    ("↑/↓", "for history"),
    ("ctrl + o", "for verbose output"),
    ("ctrl + a / e", "for line start/end"),
    ("ctrl + w / u", "to delete word/line"),
    ("esc", "to interrupt or quit"),
    ("ctrl + c", "to cancel (twice quits)"),
];

fn doctor_lines(d: &DoctorInfo) -> Vec<Line<'static>> {
    let mcp = if d.mcp_configured == 0 {
        "none configured".to_string()
    } else if d.mcp_connected == 0 {
        format!("{} configured, not connected", d.mcp_configured)
    } else {
        format!(
            "{} configured, {} connected",
            d.mcp_configured, d.mcp_connected
        )
    };
    vec![
        header("Diagnostics"),
        row("└ Version", &d.cli_version),
        row("└ LingXi home", &d.lingxi_home),
        row("└ Working dir", &d.cwd),
        row("└ MCP servers", &mcp),
        Line::from(""),
        header("Terminal"),
        row("└ Truecolor", if d.truecolor { "yes" } else { "no" }),
        row(
            "└ Terminal size",
            &format!("{}x{}", d.term_size.0, d.term_size.1),
        ),
        row("└ Inline images", &d.image_protocol),
    ]
}

/// The `/help` body: a static shortcuts section plus the slash-command
/// listing DERIVED from the single [`crate::command::BUILTIN`] registry (plan
/// Phase 8) — advertised entries only, in registry order.
pub(crate) fn help_lines() -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(SHORTCUTS.len() + 24);
    out.push(Line::from(
        "Claude understands your codebase, makes edits with your permission, \
         and executes commands — right from your terminal.",
    ));
    out.push(Line::from(""));
    out.push(header("Shortcuts"));
    for (chord, label) in SHORTCUTS {
        out.push(row(chord, label));
    }
    out.push(Line::from(""));
    out.push(header("Slash commands"));
    for command in crate::command::advertised() {
        out.push(row(command.name, command.description));
    }
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "For more help: https://code.claude.com/docs/en/overview",
        Style::default().add_modifier(Modifier::DIM),
    )));
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn text_of(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn doctor_screen_renders_diagnostics() {
        let d = crate::session::DoctorInfo {
            cli_version: "lingxi-cli v0.12.0".to_string(),
            lingxi_home: "/home/u/.lingxi".to_string(),
            cwd: "/work".to_string(),
            mcp_configured: 2,
            mcp_connected: 0,
            truecolor: true,
            term_size: (120, 40),
            image_protocol: "kitty graphics".to_string(),
        };
        let lines = doctor_lines(&d);
        let text = text_of(&lines);
        assert!(text.contains("Diagnostics"));
        assert!(text.contains("Terminal"));
        assert!(text.contains("120x40"));
        assert!(text.contains("2 configured, not connected"));
    }

    #[test]
    fn from_rows_shows_empty_state_and_entries() {
        let empty = ScreenView::from_rows("MCP servers", "MCP servers", &[], "none here");
        assert!(text_of(&empty.lines).contains("none here"));

        let rows = vec![crate::session::InfoRow::new(
            "filesystem",
            Some("stdio · connected".to_string()),
        )];
        let filled = ScreenView::from_rows("MCP servers", "MCP servers", &rows, "none here");
        let text = text_of(&filled.lines);
        assert!(text.contains("filesystem"));
        assert!(text.contains("stdio · connected"));
    }

    #[test]
    fn esc_and_q_close_other_keys_stay() {
        let mut s = ScreenView::help();
        assert!(matches!(
            s.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            s.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
        let mut s2 = ScreenView::help();
        assert!(matches!(
            s2.handle_key(press(KeyCode::Char('q'))),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn scroll_keys_move_and_clamp() {
        let mut s = ScreenView::help();
        assert_eq!(s.scroll(), 0);
        s.handle_key(press(KeyCode::Up)); // clamps at 0
        assert_eq!(s.scroll(), 0);
        s.handle_key(press(KeyCode::Down));
        assert_eq!(s.scroll(), 1);
        s.handle_key(press(KeyCode::End));
        let bottom = s.scroll();
        assert!(bottom > 1);
        s.handle_key(press(KeyCode::Down)); // clamps at bottom
        assert_eq!(s.scroll(), bottom);
        s.handle_key(press(KeyCode::Home));
        assert_eq!(s.scroll(), 0);
    }

    #[test]
    fn full_frame_contract_suppresses_status_line() {
        let s = ScreenView::help();
        assert!(!s.wants_status_line());
        // Body + 2 border rows.
        assert_eq!(
            s.desired_height(80),
            u16::try_from(s.lines.len()).unwrap() + 2
        );
    }

    #[test]
    fn render_draws_title_body_and_footer_into_buffer() {
        let s = ScreenView::new(
            "My screen",
            vec![Line::from("body line one")],
            "esc to close",
        );
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        s.render(area, &mut buf);
        let text: String = (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("My screen"), "{text}");
        assert!(text.contains("body line one"), "{text}");
        assert!(text.contains("esc to close"), "{text}");
    }

    #[test]
    fn help_body_has_both_sections_and_derives_commands_from_the_registry() {
        let lines = help_lines();
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Shortcuts"));
        assert!(text.contains("Slash commands"));
        assert!(text.contains("for commands"));
        // Every advertised registry command is listed (the registry is the
        // single source — full sweep in command::tests).
        for command in crate::command::advertised() {
            assert!(text.contains(command.name), "{} missing", command.name);
        }
        // Dead chords from the iocraft help are gone.
        assert!(!text.contains("for bash mode"));
        assert!(!text.contains("/keybindings"));
    }
}
