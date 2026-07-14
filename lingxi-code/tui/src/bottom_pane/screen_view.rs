//! Read-only full-frame screens (`/help`, `/doctor`, `/mcp`, `/hooks`,
//! `/agents`, `/skills`, `/memory`, `/status`, `/config`), ported from the
//! former `screens::FullScreen` into a stacked [`BottomPaneView`] (plan
//! Phase 4; completed by the Phase 8 command registry).
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

use tui_core::theme::ThemeName;

use traits::CostSnapshot;

use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;
use crate::session::{DoctorInfo, InfoRow, ModelRow};

/// Which tab the `/usage` screen opens on (claude-code
/// `defaultTab: n === "stats" ? "Stats" : "Usage"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageTab {
    /// The `/usage` and `/cost` entry points open on the Usage tab.
    Usage,
    /// The `/stats` alias opens on the Stats tab.
    Stats,
}

/// Column width the shortcut/command key is padded to (iocraft `help::KEY_WIDTH`).
const KEY_WIDTH: usize = 16;

/// A scrollable, read-only full-frame screen.
pub struct ScreenView {
    title: String,
    lines: Vec<Line<'static>>,
    footer: String,
    /// Lines scrolled down from the top (`0` = top).
    scroll: u16,
    /// Optional tab strip (`(label, body)` pairs). Empty for single-body
    /// screens; non-empty for the `/usage` Usage/Stats screen, where `Tab`
    /// (and `←`/`→`) cycle the active tab and swap [`Self::lines`] to its body.
    tabs: Vec<(String, Vec<Line<'static>>)>,
    /// Index into [`Self::tabs`] of the active tab (`0` when untabbed).
    active_tab: usize,
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
            tabs: Vec::new(),
            active_tab: 0,
        }
    }

    /// A tabbed screen: a title, a set of `(label, body)` tabs, and a footer.
    /// `default_tab` is the initially-active tab index (clamped). `Tab`/`←`/`→`
    /// cycle tabs. The rendered body is a tab strip line, a blank spacer, then
    /// the active tab's lines. Used by [`Self::usage`].
    #[must_use]
    pub fn tabbed(
        title: impl Into<String>,
        tabs: Vec<(String, Vec<Line<'static>>)>,
        default_tab: usize,
        footer: impl Into<String>,
    ) -> Self {
        let active_tab = default_tab.min(tabs.len().saturating_sub(1));
        let mut view = Self {
            title: title.into(),
            lines: Vec::new(),
            footer: footer.into(),
            scroll: 0,
            tabs,
            active_tab,
        };
        view.rebuild_tab_lines();
        view
    }

    /// Rebuild [`Self::lines`] for the active tab: a tab strip (active tab in
    /// bold + `‹ ›` guillemets, others dim), a blank spacer, then the tab body.
    /// No-op when untabbed.
    fn rebuild_tab_lines(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let mut strip: Vec<Span<'static>> = Vec::new();
        for (i, (label, _)) in self.tabs.iter().enumerate() {
            if i > 0 {
                strip.push(Span::raw("   "));
            }
            if i == self.active_tab {
                strip.push(Span::styled(
                    format!("‹{label}›"),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            } else {
                strip.push(Span::styled(
                    format!(" {label} "),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
        }
        let mut lines = vec![Line::from(strip), Line::from("")];
        lines.extend(self.tabs[self.active_tab].1.iter().cloned());
        self.lines = lines;
        self.scroll = 0;
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

    /// The `/status` screen: session facts from the startup snapshot plus the
    /// live editor toggles (plan Phase 8).
    #[must_use]
    pub fn status(
        d: &DoctorInfo,
        model: Option<&ModelRow>,
        vim: bool,
        verbose: bool,
        theme: ThemeName,
    ) -> Self {
        Self::new(
            "Status",
            status_lines(d, model, vim, verbose, theme),
            "esc to close · ↑/↓ scroll",
        )
    }

    /// The `/config` screen: the session-scoped settings this backend owns
    /// plus the on-disk settings files (read-only; plan Phase 8 — no
    /// settings-write API reaches the TUI, so the view is honest about it).
    #[must_use]
    pub fn settings(
        theme: ThemeName,
        vim: bool,
        verbose: bool,
        lingxi_home: &str,
        cwd: &str,
    ) -> Self {
        Self::new(
            "Settings",
            settings_lines(theme, vim, verbose, lingxi_home, cwd),
            "esc to close · ↑/↓ scroll",
        )
    }

    /// The `/usage` (aliases `/cost`, `/stats`) interactive screen — the
    /// claude-code 2.1.205 tabbed Usage/Stats dialog (`oje`), populated from
    /// the live [`CostSnapshot`]. `default_tab` picks the initially-shown tab
    /// (`"Stats"` for the `/stats` alias, `"Usage"` otherwise), mirroring the
    /// reference's `defaultTab: n === "stats" ? "Stats" : "Usage"`.
    ///
    /// - **Usage** tab: this session's token usage (input / output / cache) and
    ///   API-call count — the local data LingXi tracks. Claude-plan rate-limit
    ///   rows (the reference's "Current session" / "Current week" tiers) need a
    ///   Claude.ai subscription backend LingXi (multi-provider) does not have,
    ///   so that section is honestly absent rather than shown with placeholder
    ///   numbers.
    /// - **Stats** tab: the reference's byte-exact `i6e()` `/cost` block —
    ///   `Total cost:`, `Total duration (API):`, `Total duration (wall):`,
    ///   `Total code changes:`, and the `Usage by model:` breakdown — from
    ///   real accumulated figures, dimmed.
    #[must_use]
    pub fn usage(cost: &CostSnapshot, default_tab: UsageTab) -> Self {
        let usage_lines = vec![
            header("Session token usage"),
            row("└ Input", &cost.input_tokens.to_string()),
            row("└ Output", &cost.output_tokens.to_string()),
            row("└ Cache read", &cost.cache_read_tokens.to_string()),
            row("└ Cache write", &cost.cache_creation_tokens.to_string()),
            row("└ API calls", &cost.api_calls.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Plan usage limits require a Claude.ai subscription.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        let stats_lines: Vec<Line<'static>> = cost::render::cost_summary_from_snapshot(cost)
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().add_modifier(Modifier::DIM),
                ))
            })
            .collect();
        let tabs = vec![
            ("Usage".to_string(), usage_lines),
            ("Stats".to_string(), stats_lines),
        ];
        let default = match default_tab {
            UsageTab::Usage => 0,
            UsageTab::Stats => 1,
        };
        Self::tabbed("Usage", tabs, default, "tab to switch · esc to close")
    }

    /// The `/context` screen — claude-code 2.1.205's "Visualize current context
    /// usage as a colored grid". Renders a `GRID_COLS`×`GRID_ROWS` square grid
    /// whose filled cells (`■`, colored) are the used share of the context
    /// window and whose empty cells (`□`, dim) are free space, plus the token
    /// counts and percentages.
    ///
    /// The reference splits the used cells by category (system prompt, tools,
    /// MCP, memory, messages). LingXi's engine exposes only the total
    /// `(used, max)` — there is no per-category token accounting seam — so the
    /// used cells render as a single "Messages" band rather than the reference's
    /// multi-color category breakdown. Every number shown is real; the category
    /// split is honestly absent rather than fabricated.
    #[must_use]
    pub fn context(model: &str, used: u64, max: u64) -> Self {
        const GRID_COLS: usize = 20;
        const GRID_ROWS: usize = 5;
        const CELLS: usize = GRID_COLS * GRID_ROWS;

        let pct = if max == 0 { 0 } else { ((used * 100) / max).min(100) };
        let free = max.saturating_sub(used);
        let free_pct = 100u64.saturating_sub(pct);
        // Round the filled-cell count to the nearest cell.
        let filled = if max == 0 {
            0
        } else {
            usize::try_from((used * CELLS as u64 + max / 2) / max).unwrap_or(CELLS).min(CELLS)
        };

        let used_style = Style::default().fg(ratatui::style::Color::Cyan);
        let free_style = Style::default().add_modifier(Modifier::DIM);
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(Span::styled(
                format!("Model: {model}"),
                Style::default().add_modifier(Modifier::DIM),
            )),
            Line::from(""),
        ];
        for r in 0..GRID_ROWS {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(GRID_COLS);
            for c in 0..GRID_COLS {
                let idx = r * GRID_COLS + c;
                if idx < filled {
                    spans.push(Span::styled("■ ", used_style));
                } else {
                    spans.push(Span::styled("□ ", free_style));
                }
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("■ ", used_style),
            Span::raw(format!(
                "Messages: {} tokens ({pct}%)",
                fmt_tokens(used)
            )),
        ]));
        lines.push(Line::from(vec![
            Span::styled("□ ", free_style),
            Span::raw(format!(
                "Free space: {} tokens ({free_pct}%)",
                fmt_tokens(free)
            )),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Per-category breakdown (system prompt, tools, MCP) is not tracked yet.",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Self::new("Context Usage", lines, "esc to close")
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

    /// The body as plain text — spans joined per line, lines joined with
    /// `\n`. Exposed for tests.
    #[must_use]
    pub fn body_text(&self) -> String {
        self.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
            // Tab / ←/→ cycle the active tab on a tabbed screen (the `/usage`
            // Usage/Stats dialog). No-op (falls through to scroll for `←`/`→`,
            // which scroll nothing) on single-body screens.
            KeyCode::Tab | KeyCode::Right if !self.tabs.is_empty() => {
                self.active_tab = (self.active_tab + 1) % self.tabs.len();
                self.rebuild_tab_lines();
                ViewOutcome::Pending
            }
            KeyCode::BackTab | KeyCode::Left if !self.tabs.is_empty() => {
                self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                self.rebuild_tab_lines();
                ViewOutcome::Pending
            }
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

/// Compact token notation (`1.2k`, `3m`) — mirrors `command_core::context`'s
/// `format_tokens` (Intl `maximumFractionDigits: 1`, trailing `.0` dropped).
fn fmt_tokens(n: u64) -> String {
    const UNITS: [(u64, char); 4] = [
        (1_000_000_000_000, 't'),
        (1_000_000_000, 'b'),
        (1_000_000, 'm'),
        (1_000, 'k'),
    ];
    for &(threshold, suffix) in &UNITS {
        if n >= threshold {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let rounded = ((n as f64 / threshold as f64) * 10.0).round() / 10.0;
            let s = format!("{rounded:.1}");
            let s = s.strip_suffix(".0").unwrap_or(&s);
            return format!("{s}{suffix}");
        }
    }
    n.to_string()
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

/// The human MCP-server summary shared by `/doctor` and `/status`.
fn mcp_clause(configured: u32, connected: u32) -> String {
    if configured == 0 {
        "none configured".to_string()
    } else if connected == 0 {
        format!("{configured} configured, not connected")
    } else {
        format!("{configured} configured, {connected} connected")
    }
}

/// `"on"` / `"off"` for the editor-toggle rows.
fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn doctor_lines(d: &DoctorInfo) -> Vec<Line<'static>> {
    let mcp = mcp_clause(d.mcp_configured, d.mcp_connected);
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

/// `/status` body: session facts + editor toggles.
fn status_lines(
    d: &DoctorInfo,
    model: Option<&ModelRow>,
    vim: bool,
    verbose: bool,
    theme: ThemeName,
) -> Vec<Line<'static>> {
    let model_label = model.map_or_else(
        || "unknown".to_string(),
        |m| {
            if m.provider_label.is_empty() {
                m.display.clone()
            } else {
                format!("{} ({})", m.display, m.provider_label)
            }
        },
    );
    vec![
        header("Session"),
        row("└ Version", &d.cli_version),
        row("└ Model", &model_label),
        row("└ Working dir", &d.cwd),
        row("└ LingXi home", &d.lingxi_home),
        row(
            "└ MCP servers",
            &mcp_clause(d.mcp_configured, d.mcp_connected),
        ),
        Line::from(""),
        header("Editor"),
        row("└ Theme", theme.as_wire()),
        row("└ Vim mode", on_off(vim)),
        row("└ Verbose", on_off(verbose)),
    ]
}

/// `/config` body: session-scoped settings plus the on-disk settings files
/// (existence probed at open time, mirroring the capture-at-open contract).
fn settings_lines(
    theme: ThemeName,
    vim: bool,
    verbose: bool,
    lingxi_home: &str,
    cwd: &str,
) -> Vec<Line<'static>> {
    // parity 2.1.207 "Dynamic workflow size" (`workflowSizeGuideline`): the
    // persisted `/config` enum, shown read-only here (set via
    // `/config workflowSizeGuideline=…`). Absent ⇒ `unrestricted`.
    let workflow_size = tui_core::theme_persist::load_workflow_size_guideline()
        .unwrap_or_else(|| "unrestricted".to_string());
    let mut out = vec![
        header("Session settings"),
        row("└ Theme", theme.as_wire()),
        row("└ Vim mode", on_off(vim)),
        row("└ Verbose", on_off(verbose)),
        row("└ Dynamic workflow size", &workflow_size),
        Line::from(""),
        header("Settings files"),
    ];
    let files = [
        format!("{lingxi_home}/settings.json"),
        format!("{cwd}/.lingxi/settings.json"),
        format!("{cwd}/.lingxi/settings.local.json"),
    ];
    for file in files {
        let state = if std::path::Path::new(&file).is_file() {
            "present"
        } else {
            "absent"
        };
        out.push(row(&format!("└ {state}"), &file));
    }
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Read-only view — edit the files above to change persisted settings.",
        Style::default().add_modifier(Modifier::DIM),
    )));
    out
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
    // [PARITY] 2.1.206's help dialog (`rCo` header + `LBs` command tabs) ends
    // at the command list — it renders NO "For more help" docs footer, so the
    // port's previously-invented footer line is dropped. The
    // "New here? Run /powerup …" hint 2.1.206 shows here is intentionally NOT
    // added: `/powerup` (a Claude-Code interactive-tutorial `local-jsx`
    // command, `requires:{ink}`) is not ported, and advertising a command the
    // port does not have would be worse than omitting the hint.
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
    fn status_screen_renders_session_facts_and_editor_toggles() {
        let d = crate::session::DoctorInfo {
            cli_version: "lingxi-cli v0.12.0".to_string(),
            lingxi_home: "/home/u/.lingxi".to_string(),
            cwd: "/work".to_string(),
            mcp_configured: 3,
            mcp_connected: 2,
            truecolor: true,
            term_size: (80, 24),
            image_protocol: "none".to_string(),
        };
        let model = ModelRow {
            display: "Opus".into(),
            request_model: "claude-opus".into(),
            profile: None,
            provider_label: "Anthropic".into(),
            is_current: true,
            supports_reasoning: true,
        };
        let lines = status_lines(&d, Some(&model), true, false, ThemeName::Dark);
        let text = text_of(&lines);
        assert!(text.contains("Session"), "{text}");
        assert!(text.contains("Opus (Anthropic)"), "{text}");
        assert!(text.contains("3 configured, 2 connected"), "{text}");
        assert!(text.contains("Vim mode"), "{text}");
        assert!(text.contains("dark"), "{text}");
        // No current model degrades to "unknown", never panics.
        let no_model = status_lines(&d, None, false, true, ThemeName::LightAnsi);
        let text = text_of(&no_model);
        assert!(text.contains("unknown"), "{text}");
        assert!(text.contains("light-ansi"), "{text}");
    }

    #[test]
    fn settings_screen_probes_files_and_declares_itself_read_only() {
        let dir = std::env::temp_dir().join(format!("tui-rata-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        std::fs::write(dir.join("settings.json"), b"{}").expect("write settings");
        let home = dir.display().to_string();
        let lines = settings_lines(ThemeName::Dark, false, true, &home, "/no/such/project");
        std::fs::remove_dir_all(&dir).ok();
        let text = text_of(&lines);
        assert!(text.contains("Session settings"), "{text}");
        assert!(
            text.contains(&format!("present {home}/settings.json")) || text.contains("present"),
            "{text}"
        );
        assert!(text.contains("absent"), "{text}");
        assert!(text.contains("Read-only view"), "{text}");
        assert!(text.contains("Verbose") && text.contains("on"), "{text}");
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
    fn usage_stats_tab_renders_byte_exact_cost_block() {
        use traits::orchestrator::ModelUsageRow;
        let snap = CostSnapshot {
            total_usd: 0.1234,
            api_duration: std::time::Duration::from_millis(5_000),
            session_duration: std::time::Duration::from_secs(125),
            code_lines_added: 10,
            code_lines_removed: 1,
            by_model: vec![ModelUsageRow {
                model: "claude-opus-4-8".into(),
                total_nano_usd: 123_400_000,
                input_tokens: 5_000,
                output_tokens: 2_000,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            }],
            ..CostSnapshot::default()
        };
        let s = ScreenView::usage(&snap, UsageTab::Stats);
        let area = Rect::new(0, 0, 80, 20);
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
        assert!(text.contains("Total duration (API):"), "{text}");
        assert!(text.contains("Total code changes:"), "{text}");
        assert!(text.contains("Usage by model:"), "{text}");
        assert!(text.contains("claude-opus-4-8:"), "{text}");
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
        // Dead chords from the iocraft help are gone. (`/keybindings` was once
        // a dead chord here but is now a real registry-backed command, so it is
        // asserted present by the advertised-command sweep above.)
        assert!(!text.contains("for bash mode"));
        // Parity: 2.1.206's help dialog renders no "For more help" docs footer,
        // and does not advertise the unported `/powerup` command.
        assert!(!text.contains("For more help"));
        assert!(!text.contains("/powerup"));
    }
}
