//! Full-page screens for `tui-rata` (the `/help` viewer and future pickers).
//!
//! A [`FullScreen`] is a scrollable, read-only page drawn over the whole frame:
//! a bold title, a body of pre-styled [`Line`]s, and a dim footer hint. It owns
//! the keyboard while open (scroll keys move the window; `Esc`/`q` close),
//! mirroring the modal contract of [`crate::overlay::Dialog`].
//!
//! `/help` content is ported from the iocraft `screens::help` (claude-code
//! `HelpV2`): a `Shortcuts` section and a `Slash commands` section. Unlike the
//! iocraft port it renders the DEFAULT chords statically (no live
//! `keybindings.json` resolution) — a documented scaffold simplification.

use crossterm::event::KeyCode;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::session::{DoctorInfo, InfoRow};

/// Column width the shortcut/command key is padded to (iocraft `help::KEY_WIDTH`).
const KEY_WIDTH: usize = 16;

/// Result of routing a key into a [`FullScreen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenOutcome {
    /// Stay open (scrolled or inert key).
    Stay,
    /// Close the screen (`Esc` / bare `q`).
    Close,
}

/// A scrollable, read-only full-page screen.
pub struct FullScreen {
    title: String,
    lines: Vec<Line<'static>>,
    footer: String,
    /// Lines scrolled down from the top (`0` = top).
    scroll: u16,
}

impl FullScreen {
    /// Build a screen from a title, pre-styled body lines, and a footer hint.
    #[must_use]
    pub fn new(title: impl Into<String>, lines: Vec<Line<'static>>, footer: impl Into<String>) -> Self {
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

    /// Route a key: scroll keys move the window, `Esc`/bare `q` close.
    pub fn on_key(&mut self, code: KeyCode) -> ScreenOutcome {
        let max = u16::try_from(self.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => ScreenOutcome::Close,
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ScreenOutcome::Stay
            }
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1).min(max);
                ScreenOutcome::Stay
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                ScreenOutcome::Stay
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10).min(max);
                ScreenOutcome::Stay
            }
            KeyCode::Home => {
                self.scroll = 0;
                ScreenOutcome::Stay
            }
            KeyCode::End => {
                self.scroll = max;
                ScreenOutcome::Stay
            }
            _ => ScreenOutcome::Stay,
        }
    }

    /// Draw the screen over the whole `frame`, clearing beneath it.
    pub fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
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
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(self.lines.clone()).scroll((self.scroll, 0)),
            inner,
        );
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

/// Shortcut rows `(chord, label)`, ported from iocraft `help::SHORTCUTS`
/// (default chords).
const SHORTCUTS: &[(&str, &str)] = &[
    ("!", "for bash mode"),
    ("/", "for commands"),
    ("@", "for file paths"),
    ("&", "for background"),
    ("/btw", "for side question"),
    ("ctrl + o", "for verbose output"),
    ("ctrl + t", "to toggle tasks"),
    ("ctrl + s", "to stash prompt"),
    ("ctrl + v", "to paste images"),
    ("meta + p", "to switch model"),
    ("meta + o", "to toggle fast mode"),
    ("double tap esc", "to clear input"),
    ("/keybindings", "to customize"),
];

/// Slash commands `(command, description)`, ported from iocraft
/// `help::SLASH_COMMANDS`.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show keyboard shortcuts and commands"),
    ("/clear", "Clear the conversation history"),
    ("/exit", "Exit LingXi"),
    // (M4 cc2.1.198) The /agents wizard was removed; surface the same
    // removed-wizard description the command registry carries.
    ("/agents", "(removed) Ask Claude to create/manage subagents, or edit .claude/agents/"),
    ("/mcp", "Show configured MCP servers"),
    ("/hooks", "Show configured hooks"),
    ("/model", "Set the active model"),
    ("/skills", "List available skills"),
    ("/stats", "Show usage statistics"),
    ("/doctor", "Diagnose the installation"),
    ("/memory", "Edit LINGXI.md memory files"),
    ("/theme", "Change the color theme"),
    ("/config", "Open settings"),
    ("/status", "Show the session status"),
    ("/tasks", "View background tasks"),
    ("/vim", "Toggle vim editing mode"),
    ("/export", "Export the transcript"),
    ("/copy", "Copy the last response"),
    ("/color", "Set the prompt accent color"),
];

fn doctor_lines(d: &DoctorInfo) -> Vec<Line<'static>> {
    let mcp = if d.mcp_configured == 0 {
        "none configured".to_string()
    } else if d.mcp_connected == 0 {
        format!("{} configured, not connected", d.mcp_configured)
    } else {
        format!("{} configured, {} connected", d.mcp_configured, d.mcp_connected)
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

fn help_lines() -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(SHORTCUTS.len() + SLASH_COMMANDS.len() + 4);
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
    for (cmd, desc) in SLASH_COMMANDS {
        out.push(row(cmd, desc));
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
    use super::*;

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
        let empty = FullScreen::from_rows("MCP servers", "MCP servers", &[], "none here");
        assert!(text_of(&empty.lines).contains("none here"));

        let rows = vec![crate::session::InfoRow::new(
            "filesystem",
            Some("stdio · connected".to_string()),
        )];
        let filled = FullScreen::from_rows("MCP servers", "MCP servers", &rows, "none here");
        let text = text_of(&filled.lines);
        assert!(text.contains("filesystem"));
        assert!(text.contains("stdio · connected"));
    }

    #[test]
    fn esc_and_q_close_other_keys_stay() {
        let mut s = FullScreen::help();
        assert_eq!(s.on_key(KeyCode::Enter), ScreenOutcome::Stay);
        assert_eq!(s.on_key(KeyCode::Esc), ScreenOutcome::Close);
        let mut s2 = FullScreen::help();
        assert_eq!(s2.on_key(KeyCode::Char('q')), ScreenOutcome::Close);
    }

    #[test]
    fn scroll_keys_move_and_clamp() {
        let mut s = FullScreen::help();
        assert_eq!(s.scroll(), 0);
        s.on_key(KeyCode::Up); // clamps at 0
        assert_eq!(s.scroll(), 0);
        s.on_key(KeyCode::Down);
        assert_eq!(s.scroll(), 1);
        s.on_key(KeyCode::End);
        let bottom = s.scroll();
        assert!(bottom > 1);
        s.on_key(KeyCode::Down); // clamps at bottom
        assert_eq!(s.scroll(), bottom);
        s.on_key(KeyCode::Home);
        assert_eq!(s.scroll(), 0);
    }

    #[test]
    fn help_body_has_both_sections_and_known_rows() {
        let lines = help_lines();
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Shortcuts"));
        assert!(text.contains("Slash commands"));
        assert!(text.contains("for bash mode"));
        assert!(text.contains("/skills"));
        assert!(text.contains("/color"));
    }
}
