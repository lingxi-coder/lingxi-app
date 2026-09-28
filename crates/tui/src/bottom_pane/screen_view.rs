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
use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use tui_core::theme::ThemeName;

use platform_api::CostSnapshot;

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
    /// Whether this panel represents a focusable settings list. Other
    /// read-only screens retain the historical hidden-cursor behavior.
    focusable: bool,
    /// Focused line in the unscrolled body for accessibility cursor tracking.
    selected: u16,
    /// Height of the most recently rendered body viewport. A one-row default
    /// preserves deterministic navigation before the first render.
    viewport_height: Cell<u16>,
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
            focusable: false,
            selected: 0,
            viewport_height: Cell::new(1),
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
            focusable: false,
            selected: 0,
            viewport_height: Cell::new(1),
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

    /// The `/btw` side-question exchange. Keeping the last exchange in a
    /// read-only panel lets a bare `/btw` reopen it without re-running the
    /// model or adding anything to the main conversation history.
    #[must_use]
    pub fn side_question(question: &str, answer: &str) -> Self {
        let mut lines = vec![
            Line::from(Span::styled(
                "Question",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        lines.extend(question.lines().map(|line| Line::from(line.to_string())));
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Answer",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ]);
        lines.extend(answer.lines().map(|line| Line::from(line.to_string())));
        Self::new(
            "Side question",
            lines,
            "esc to close · ↑/↓ scroll · /btw to reopen",
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
        large_memory_warnings: &[String],
    ) -> Self {
        Self::new(
            "Status",
            status_lines(d, model, vim, verbose, theme, large_memory_warnings),
            "esc to close · ↑/↓ scroll",
        )
    }

    /// The `/config` screen: the session-scoped settings this backend owns
    /// plus the on-disk settings files (read-only; plan Phase 8 — no
    /// settings-write API reaches the TUI, so the view is honest about it).
    /// The agents-view rows appear only while `agent_view_enabled` (the
    /// oracle's `...$H()?[row]:[]` / `...H7e()?[row]:[]` spreads).
    #[must_use]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn settings(
        theme: ThemeName,
        vim: bool,
        verbose: bool,
        dynamic_workflows_enabled: bool,
        workflow_size_guideline: &str,
        agent_view_enabled: bool,
        left_arrow_opens_agents: bool,
        default_to_agents_view: bool,
        lingxi_home: &str,
        cwd: &str,
    ) -> Self {
        let mut view = Self::new(
            "Settings",
            settings_lines(
                theme,
                vim,
                verbose,
                dynamic_workflows_enabled,
                workflow_size_guideline,
                agent_view_enabled,
                left_arrow_opens_agents,
                default_to_agents_view,
                lingxi_home,
                cwd,
            ),
            "esc to close · ↑/↓ scroll",
        );
        view.focusable = true;
        view
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
        let mut usage_lines = vec![
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
        if !cost.loops.is_empty() {
            usage_lines.push(Line::from(""));
            usage_lines.extend(Self::loops_section_lines(&cost.loops));
        }
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

    /// Oracle `gl()`: Loops is a Usage *section*, hidden when there are no
    /// rows — never a third tab.
    fn loops_section_lines(rows: &[platform_api::LoopUsageRow]) -> Vec<Line<'static>> {
        const MAX_ROWS: usize = 10;
        let mut lines = vec![header("Loops")];
        lines.push(row("every / runs / tokens / last", ""));
        let shown = rows.len().min(MAX_ROWS);
        for loop_row in &rows[..shown] {
            let summary = format!(
                "{} · {} · {} · {}",
                loop_row.every, loop_row.runs, loop_row.tokens, loop_row.last_run
            );
            lines.push(row(&loop_row.prompt, &summary));
        }
        if rows.len() > MAX_ROWS {
            lines.push(Line::from(Span::styled(
                format!("… {} more", rows.len() - MAX_ROWS),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines
    }

    /// The `/context` screen — claude-code 2.1.205's "Visualize current context
    /// usage as a colored grid". Renders a `GRID_COLS`×`GRID_ROWS` square grid
    /// whose filled cells (`■`, colored) are the used share of the context
    /// window and whose empty cells (`□`, dim) are free space, plus the token
    /// counts and percentages.
    ///
    /// Category cells and the legend are projected from the same
    /// [`platform_api::ContextUsageSnapshot`] the headless `/context` command uses.
    #[must_use]
    pub fn context(model: &str, usage: &platform_api::ContextUsageSnapshot) -> Self {
        const GRID_COLS: usize = 20;
        const GRID_ROWS: usize = 5;
        const CELLS: usize = GRID_COLS * GRID_ROWS;

        let used = usage.live_context_tokens;
        let max = usage.max_context_tokens;
        let pct = context_percentage(used, max);
        let fallback;
        let breakdown = if usage.breakdown.is_empty() {
            fallback = vec![
                platform_api::ContextUsageCategory::new(
                    platform_api::ContextUsageCategoryKind::Messages,
                    used,
                ),
                platform_api::ContextUsageCategory::new(
                    platform_api::ContextUsageCategoryKind::FreeSpace,
                    max.saturating_sub(used),
                ),
            ];
            &fallback
        } else {
            &usage.breakdown
        };

        let mut lines: Vec<Line<'static>> = vec![
            Line::from(Span::styled(
                format!(
                    "Model: {model} · {} / {} tokens ({pct}%)",
                    fmt_tokens(used),
                    fmt_tokens(max)
                ),
                Style::default().add_modifier(Modifier::DIM),
            )),
            Line::from(""),
        ];
        for r in 0..GRID_ROWS {
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(GRID_COLS);
            for c in 0..GRID_COLS {
                let idx = r * GRID_COLS + c;
                let kind = category_for_cell(breakdown, idx, CELLS, max);
                let glyph = if kind == platform_api::ContextUsageCategoryKind::FreeSpace {
                    "□ "
                } else {
                    "■ "
                };
                spans.push(Span::styled(glyph, context_category_style(kind)));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
        for row in breakdown {
            let row_pct = context_percentage(row.tokens, max);
            let glyph = if row.kind == platform_api::ContextUsageCategoryKind::FreeSpace {
                "□ "
            } else {
                "■ "
            };
            lines.push(Line::from(vec![
                Span::styled(glyph, context_category_style(row.kind)),
                Span::raw(format!(
                    "{}: {} tokens ({row_pct}%)",
                    context_category_label(row.kind),
                    fmt_tokens(row.tokens)
                )),
            ]));
        }
        if let Some(warning) = usage.overflow_warning(
            std::env::var_os("DISABLE_COMPACT").is_some_and(|value| !value.is_empty()),
        ) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("⚠ {warning}"),
                Style::default().fg(ratatui::style::Color::Yellow),
            )));
        }
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

    fn reveal_selection(&mut self) {
        let height = self.viewport_height.get().max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll.saturating_add(height) {
            self.scroll = self.selected.saturating_add(1).saturating_sub(height);
        }
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
        self.viewport_height.set(inner.height.max(1));
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

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.focusable || self.lines.is_empty() {
            return None;
        }
        let inner = Block::new().borders(Borders::ALL).inner(area);
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let visible_row = self.selected.saturating_sub(self.scroll);
        Some((
            inner.x,
            inner
                .y
                .saturating_add(visible_row)
                .min(inner.bottom().saturating_sub(1)),
        ))
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
                if self.focusable {
                    self.selected = self.selected.saturating_sub(1);
                    self.reveal_selection();
                } else {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.focusable {
                    self.selected = self.selected.saturating_add(1).min(max);
                    self.reveal_selection();
                } else {
                    self.scroll = self.scroll.saturating_add(1).min(max);
                }
                ViewOutcome::Pending
            }
            KeyCode::PageUp => {
                if self.focusable {
                    self.selected = self.selected.saturating_sub(10);
                    self.reveal_selection();
                } else {
                    self.scroll = self.scroll.saturating_sub(10);
                }
                ViewOutcome::Pending
            }
            KeyCode::PageDown => {
                if self.focusable {
                    self.selected = self.selected.saturating_add(10).min(max);
                    self.reveal_selection();
                } else {
                    self.scroll = self.scroll.saturating_add(10).min(max);
                }
                ViewOutcome::Pending
            }
            KeyCode::Home => {
                self.scroll = 0;
                self.selected = 0;
                ViewOutcome::Pending
            }
            KeyCode::End => {
                if self.focusable {
                    self.selected = max;
                    self.reveal_selection();
                } else {
                    self.scroll = max;
                }
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

/// Compact token notation (`1.2k`, `3m`) — mirrors `command_api::builtins::context`'s
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

fn context_percentage(tokens: u64, max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    tokens
        .saturating_mul(100)
        .saturating_add(max / 2)
        .saturating_div(max)
        .min(100)
}

fn category_for_cell(
    breakdown: &[platform_api::ContextUsageCategory],
    cell: usize,
    cells: usize,
    max: u64,
) -> platform_api::ContextUsageCategoryKind {
    use platform_api::ContextUsageCategoryKind as Kind;
    if max == 0 || cells == 0 {
        return Kind::FreeSpace;
    }
    let cell = u64::try_from(cell).unwrap_or(u64::MAX);
    let cells = u64::try_from(cells).unwrap_or(u64::MAX);
    let target = cell.saturating_mul(max) / cells;
    let mut cumulative = 0u64;
    for row in breakdown {
        cumulative = cumulative.saturating_add(row.tokens);
        if target < cumulative {
            return row.kind;
        }
    }
    Kind::FreeSpace
}

fn context_category_label(kind: platform_api::ContextUsageCategoryKind) -> &'static str {
    use platform_api::ContextUsageCategoryKind as Kind;
    match kind {
        Kind::SystemPrompt => "System prompt",
        Kind::SystemTools => "System tools",
        Kind::McpTools => "MCP tools",
        Kind::MemoryFiles => "Memory files",
        Kind::Skills => "Skills",
        Kind::Messages => "Messages",
        Kind::AutocompactBuffer => "Autocompact buffer",
        Kind::FreeSpace => "Free space",
    }
}

fn context_category_style(kind: platform_api::ContextUsageCategoryKind) -> Style {
    use platform_api::ContextUsageCategoryKind as Kind;
    use ratatui::style::Color;
    match kind {
        Kind::SystemPrompt => Style::default().fg(Color::Magenta),
        Kind::SystemTools => Style::default().fg(Color::Blue),
        Kind::McpTools => Style::default().fg(Color::Yellow),
        Kind::MemoryFiles => Style::default().fg(Color::Green),
        Kind::Skills => Style::default().fg(Color::LightGreen),
        Kind::Messages => Style::default().fg(Color::Cyan),
        Kind::AutocompactBuffer => Style::default().fg(Color::DarkGray),
        Kind::FreeSpace => Style::default().add_modifier(Modifier::DIM),
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

/// `/status` body: session facts + editor toggles, plus any
/// `large_memory_warnings` appended as a trailing `Warnings` block.
///
/// The warning rows are claude-code `htf()` (2.1.220 binary offset 241152161),
/// which contributes `Large <path> will impact performance (<n> chars > <max>)`
/// to the `/status` body for each memory file over
/// [`memory::max_memory_character_count`]. LingXi's `/status` panel is an
/// intentional divergence with a fixed row set, so the warnings are appended
/// under their own header rather than woven into the oracle's row order.
fn status_lines(
    d: &DoctorInfo,
    model: Option<&ModelRow>,
    vim: bool,
    verbose: bool,
    theme: ThemeName,
    large_memory_warnings: &[String],
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
    let mut lines = vec![
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
    ];
    // APPEND-ONLY — an empty warning list leaves the panel byte-identical.
    // Oracle `Ava` (@241166200) renders `null` for an empty list and otherwise a
    // bold "System diagnostics" above the rows; the header text is the oracle's,
    // not a LingXi coinage. (`header` adds UNDERLINED on top of the oracle's
    // bold — that is this panel's own house style, applied to every section.)
    if !large_memory_warnings.is_empty() {
        lines.push(Line::from(""));
        lines.push(header("System diagnostics"));
        for warning in large_memory_warnings {
            // NOT `row("└", warning)`: `row` pads its KEY to `KEY_WIDTH`, which
            // buys value-column alignment only when there IS a key. A bare
            // "└" would spend 17 columns on nothing, and the paragraph this
            // renders into never soft-wraps (see `ScreenView::render`), so at
            // 80 columns that pushes `(52.3k chars > 40.0k)` — the whole point
            // of the warning — off the right edge with no way to scroll to it.
            lines.push(Line::from(vec![
                Span::styled("└ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(warning.clone()),
            ]));
        }
    }
    lines
}

/// `/config` body: session-scoped settings plus the on-disk settings files
/// (existence probed at open time, mirroring the capture-at-open contract).
#[allow(clippy::fn_params_excessive_bools)]
fn settings_lines(
    theme: ThemeName,
    vim: bool,
    verbose: bool,
    dynamic_workflows_enabled: bool,
    workflow_size_guideline: &str,
    agent_view_enabled: bool,
    left_arrow_opens_agents: bool,
    default_to_agents_view: bool,
    lingxi_home: &str,
    cwd: &str,
) -> Vec<Line<'static>> {
    let mut out = vec![
        header("Session settings"),
        row("└ Theme", theme.as_wire()),
        row("└ Vim mode", on_off(vim)),
        row("└ Verbose", on_off(verbose)),
        row("└ Dynamic workflows", on_off(dynamic_workflows_enabled)),
        row("└ Dynamic workflow size", workflow_size_guideline),
        row(
            "└ Dialog expiry",
            tui_core::theme_persist::load_dialog_expiry()
                .as_deref()
                .unwrap_or("default"),
        ),
        row(
            "└ Messages from your other sessions",
            tui_core::theme_persist::load_cross_session_inbound()
                .as_deref()
                .unwrap_or("default"),
        ),
    ];
    // parity 2.1.220 agents-view rows, oracle order (`defaultToAgentsView`
    // "Open agents view by default" first, then `leftArrowOpensAgents`
    // "${DW} opens agents"), hidden entirely while agent view is disabled.
    if agent_view_enabled {
        if telemetry::flag_bool("tengu_maple_sundial", false) {
            // 2.1.220 managedEnum: the server-side gate collapses the two
            // booleans into one read-only summary row. There is deliberately
            // no per-setting control in this shape.
            out.push(row(
                "└ Agents view",
                on_off(default_to_agents_view || left_arrow_opens_agents),
            ));
        } else {
            out.push(row(
                "└ Open agents view by default",
                on_off(default_to_agents_view),
            ));
            out.push(row("└ ← opens agents", on_off(left_arrow_opens_agents)));
        }
    }
    out.push(Line::from(""));
    out.push(header("Settings files"));
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
            provenance: platform_api::ModelProvenance::ProviderCatalogTier,
            is_current: true,
            supports_reasoning: true,
            supports_multimodal: false,
            details: Vec::new(),
            fusion_analyst_capable: false,
        };
        let lines = status_lines(&d, Some(&model), true, false, ThemeName::Dark, &[]);
        let text = text_of(&lines);
        assert!(text.contains("Session"), "{text}");
        assert!(text.contains("Opus (Anthropic)"), "{text}");
        assert!(text.contains("3 configured, 2 connected"), "{text}");
        assert!(text.contains("Vim mode"), "{text}");
        assert!(text.contains("dark"), "{text}");
        // No warnings ⇒ NO `Warnings` block at all (claude-code `htf()` returns
        // `[]`, and a `/status` section with no rows is never drawn).
        assert!(!text.contains("System diagnostics"), "{text}");
        // No current model degrades to "unknown", never panics.
        let no_model = status_lines(&d, None, false, true, ThemeName::LightAnsi, &[]);
        let text = text_of(&no_model);
        assert!(text.contains("unknown"), "{text}");
        assert!(text.contains("light-ansi"), "{text}");
    }

    #[test]
    fn status_screen_appends_large_memory_warnings_after_the_locked_rows() {
        // claude-code `htf()` (2.1.220 binary offset 241152161) contributes one
        // `Large … will impact performance (… chars > …)` row per oversized
        // memory file to the `/status` body. LingXi's `/status` panel is an
        // intentional divergence (a fixed row set), so the warnings are
        // APPENDED — the existing Session/Editor rows must not move or reword.
        let d = crate::session::DoctorInfo {
            cli_version: "lingxi-cli v0.12.0".to_string(),
            lingxi_home: "/home/u/.lingxi".to_string(),
            cwd: "/work".to_string(),
            mcp_configured: 0,
            mcp_connected: 0,
            truecolor: false,
            term_size: (80, 24),
            image_protocol: "none".to_string(),
        };
        let warnings = vec![memory::format_large_memory_file_status_row(
            "LINGXI.md",
            52_310,
            40_000,
        )];
        let baseline = status_lines(&d, None, false, false, ThemeName::Dark, &[]);
        let lines = status_lines(&d, None, false, false, ThemeName::Dark, &warnings);
        // Append-only: every baseline line is still present, in order, at the
        // same index. `starts_with` (not `lines[..baseline.len()]`) so a panel
        // that SHRANK fails as an assertion instead of an index panic.
        assert!(
            lines.starts_with(&baseline),
            "warnings must be appended, not woven into the locked rows"
        );
        assert_eq!(lines.len(), baseline.len() + 3, "blank + header + one row");
        let text = text_of(&lines);
        assert!(text.contains("System diagnostics"), "{text}");
        assert!(
            text.contains("Large LINGXI.md will impact performance (52.3k chars > 40.0k)"),
            "{text}"
        );
    }

    #[test]
    fn settings_screen_probes_files_and_declares_itself_read_only() {
        let _state = crate::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        telemetry::test_clear_flag("tengu_maple_sundial");
        let dir = std::env::temp_dir().join(format!("tui-rata-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        std::fs::write(dir.join("settings.json"), b"{}").expect("write settings");
        let home = dir.display().to_string();
        let lines = settings_lines(
            ThemeName::Dark,
            false,
            true,
            true,
            "medium",
            true,
            true,
            false,
            &home,
            "/no/such/project",
        );
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
        assert!(
            text.contains("Dynamic workflows") && text.contains("on"),
            "{text}"
        );
        // Agents-view rows (2.1.220), oracle order and labels.
        assert!(text.contains("Open agents view by default"), "{text}");
        assert!(text.contains("← opens agents"), "{text}");
    }

    #[test]
    fn context_screen_uses_shared_category_snapshot() {
        use platform_api::{ContextUsageCategory, ContextUsageCategoryKind as Kind};
        let view = ScreenView::context(
            "claude-opus-5",
            &platform_api::ContextUsageSnapshot {
                live_context_tokens: 100_000,
                max_context_tokens: 1_000_000,
                breakdown: vec![
                    ContextUsageCategory::new(Kind::SystemPrompt, 20_000),
                    ContextUsageCategory::new(Kind::SystemTools, 30_000),
                    ContextUsageCategory::new(Kind::Messages, 50_000),
                    ContextUsageCategory::new(Kind::AutocompactBuffer, 13_000),
                    ContextUsageCategory::new(Kind::FreeSpace, 887_000),
                ],
                ..Default::default()
            },
        );
        let text = view.body_text();
        assert!(text.contains("claude-opus-5 · 100k / 1m tokens (10%)"));
        assert!(text.contains("System prompt: 20k tokens (2%)"));
        assert!(text.contains("System tools: 30k tokens (3%)"));
        assert!(text.contains("Autocompact buffer: 13k tokens (1%)"));
        assert!(text.contains("Free space: 887k tokens (89%)"));
    }

    #[test]
    fn context_screen_warns_when_usage_exceeds_the_window() {
        let view = ScreenView::context(
            "claude-opus-5",
            &platform_api::ContextUsageSnapshot {
                live_context_tokens: 1_012_345,
                max_context_tokens: 1_000_000,
                ..Default::default()
            },
        );
        assert!(view.body_text().contains(
            "Context exceeds the 1m-token limit by 12.3k tokens \u{2014} run /compact or /clear to continue."
        ));
    }

    /// With agent view disabled the two agents-view rows disappear entirely,
    /// mirroring the oracle's `...$H()?[row]:[]` / `...H7e()?[row]:[]`.
    #[test]
    fn settings_screen_hides_agents_rows_when_agent_view_is_disabled() {
        let lines = settings_lines(
            ThemeName::Dark,
            false,
            true,
            false,
            "small",
            false,
            true,
            true,
            "/no/such/home",
            "/no/such/project",
        );
        let text = text_of(&lines);
        assert!(!text.contains("Open agents view by default"), "{text}");
        assert!(!text.contains("opens agents"), "{text}");
    }

    #[test]
    fn maple_sundial_collapses_agents_settings_to_managed_row() {
        // `tengu_maple_sundial` is process-global and `chat_widget`'s tests
        // read it; take the crate-wide lock so neither observes the other's.
        let _state = crate::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        telemetry::test_set_flag("tengu_maple_sundial", true);
        let lines = settings_lines(
            ThemeName::Dark,
            false,
            true,
            true,
            "large",
            true,
            false,
            true,
            "/no/such/home",
            "/no/such/project",
        );
        telemetry::test_clear_flag("tengu_maple_sundial");
        let text = text_of(&lines);
        assert!(
            text.contains("Agents view") && text.contains("on"),
            "{text}"
        );
        assert!(!text.contains("Open agents view by default"), "{text}");
        assert!(!text.contains("opens agents"), "{text}");
    }

    #[test]
    fn settings_cursor_tracks_focused_row_inside_scrolled_viewport() {
        let mut view = ScreenView::settings(
            ThemeName::Dark,
            false,
            false,
            false,
            "medium",
            true,
            true,
            false,
            "/no/such/home",
            "/no/such/project",
        );
        let area = Rect::new(3, 4, 50, 6);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        for _ in 0..8 {
            let _ = view.handle_key(press(KeyCode::Down));
        }
        let (x, y) = view.cursor_pos(area).expect("settings row owns cursor");
        assert!(x >= area.left() && x < area.right());
        assert!(y > area.top() && y < area.bottom());
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
        use platform_api::orchestrator::ModelUsageRow;
        let snap = CostSnapshot {
            total_usd: 0.1234,
            api_duration: std::time::Duration::from_millis(5_000),
            session_duration: std::time::Duration::from_secs(125),
            code_lines_added: 10,
            code_lines_removed: 1,
            by_model: vec![ModelUsageRow {
                model: "claude-opus-4-8".into(),
                provider: None,
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
    fn usage_tab_hides_loops_section_when_empty() {
        let snap = CostSnapshot::default();
        let s = ScreenView::usage(&snap, UsageTab::Usage);
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
        assert!(
            !text.contains("Loops"),
            "empty loops must hide the section, got {text}"
        );
    }

    #[test]
    fn usage_tab_shows_loops_section_when_present() {
        let snap = CostSnapshot {
            loops: vec![platform_api::LoopUsageRow {
                prompt: "check deploy".into(),
                every: "5m".into(),
                runs: 3,
                tokens: 1200,
                last_run: "2m ago".into(),
            }],
            ..CostSnapshot::default()
        };
        let s = ScreenView::usage(&snap, UsageTab::Usage);
        let area = Rect::new(0, 0, 80, 24);
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
        assert!(text.contains("Loops"), "{text}");
        assert!(text.contains("check deploy"), "{text}");
        assert!(text.contains("5m"), "{text}");
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
        // Parity: the help dialog renders no "For more help" docs footer.
        // `/powerup` is now a live registry command and is covered by the
        // advertised-command sweep above.
        assert!(!text.contains("For more help"));
    }
}
