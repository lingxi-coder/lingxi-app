//! The standalone `lingxi-cli agents` view — a minimal, usable port of the
//! claude-code 2.1.198 fleet view (agent list).
//!
//! (M7 cc2.1.198) The real view groups background sessions into four bands
//! (binary `NZo`/`MFc`, @222792125):
//!
//! ```text
//! review  → "Ready for review"
//! blocked → "Needs input"   — "Sessions that have a question or need your
//!                              decision land here"
//! working → "Working"       — "Sessions Claude is actively working on — they
//!                              keep running even if you close the terminal"
//! done    → "Completed"     — "Finished sessions wait here for you to review"
//! ```
//!
//! Status stability (2.1.196 fix): the band comes from the MERGED state
//! computed by `cli::agents_registry::merged_state`, where a terminal outcome
//! beats a stale `blocked` tempo — so a row can never flip Done ↔ Needs
//! input between refreshes. Rows whose text references a PR (`#123` or a
//! `/pull/123` URL — binary `Hon`, @223853400 region) show the PR reference.
//!
//! Pure-state-machine + thin-mount idiom (same contract as
//! `tui::startup_bypass` and the standalone full-screen view): every decision
//! lives in [`AgentsScreenState::on_key`] (fully unit-tested); the terminal
//! mount loop lives in the CLI (`apps/cli/src/commands/agents.rs`) and only
//! does IO.
//!
//! Deferred (M7 seams):
//! * the "review" band never populates until lingxi writes background jobs
//!   with PR/worktree completion metadata (M8);
//! * `/login` from this view (binary 2.1.198) needs a mountable auth dialog;
//! * dispatching NEW sessions from the composer row (binary `BZo`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// The four fleet-view bands, in the binary's display order (`NZo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// `review` — "Ready for review".
    Review,
    /// `blocked` — "Needs input".
    Blocked,
    /// `working` — "Working".
    Working,
    /// `done` — "Completed".
    Done,
}

impl Band {
    /// All bands in display order.
    pub const ALL: [Band; 4] = [Band::Review, Band::Blocked, Band::Working, Band::Done];

    /// Section label (binary `MFc`, byte-exact).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Band::Review => "Ready for review",
            Band::Blocked => "Needs input",
            Band::Working => "Working",
            Band::Done => "Completed",
        }
    }

    /// Section description (binary `MFc` sibling map, byte-exact; review's is
    /// empty).
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Band::Review => "",
            Band::Blocked => "Sessions that have a question or need your decision land here",
            Band::Working => {
                static DESCRIPTION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
                    format!("Sessions {} is actively working on \u{2014} they keep running even if you close the terminal", branding::PRODUCT_NAME)
                });
                DESCRIPTION.as_str()
            }
            Band::Done => "Finished sessions wait here for you to review",
        }
    }
}

/// Map a merged session state (`working`/`blocked`/`done`/`failed`/`stopped`)
/// to its band. Terminal outcomes (done/failed/stopped) all land in
/// Completed; unknown states are treated as working (live).
#[must_use]
pub fn band_for_state(state: &str) -> Band {
    match state {
        "blocked" => Band::Blocked,
        "done" | "failed" | "stopped" => Band::Done,
        _ => Band::Working,
    }
}

/// Extract a PR number from free text (binary `Hon`): scan whitespace tokens;
/// a token that is exactly `#<digits>` or contains `/pull/<digits>` (not
/// followed by another digit — token-greedy digits satisfy this) yields the
/// number.
#[must_use]
pub fn extract_pr_number(text: &str) -> Option<u64> {
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix('#') {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(n) = rest.parse() {
                    return Some(n);
                }
            }
        }
        if let Some(idx) = token.find("/pull/") {
            let digits: String = token[idx + "/pull/".len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                if let Ok(n) = digits.parse() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// One selectable row of the agents view.
#[derive(Debug, Clone)]
pub struct AgentRow {
    /// Session UUID used for attach.
    pub session_id: String,
    /// Display name (already sanitized).
    pub name: String,
    /// Merged state string (`working`/`blocked`/`done`/`failed`/`stopped`,
    /// or a live status for interactive rows).
    pub state: String,
    /// `background` or `interactive`.
    pub kind: String,
    /// Row cwd (dimmed detail).
    pub cwd: String,
    /// PR referenced by the row's result/detail text, if any.
    pub pr: Option<u64>,
}

impl AgentRow {
    /// The band this row lands in.
    #[must_use]
    pub fn band(&self) -> Band {
        band_for_state(&self.state)
    }
}

/// Terminal outcome of routing a key into the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsOutcome {
    /// Stay open (moved selection or inert key).
    Stay,
    /// Attach to the selected session (open its transcript / resume it).
    Attach(String),
    /// Open the shared provider login flow (`/connect`) and then return here.
    Login,
    /// Close the view (`q` / `Esc` / `Ctrl-C`).
    Exit,
    /// Delete the session (kill its live worker, then remove its job state) —
    /// the second press of the two-press Ctrl-X confirm. Carries the armed
    /// row's session id.
    Delete(String),
    /// Stop every running agent + background job at once — the `Ctrl+X Ctrl+K`
    /// chord.
    StopAll,
}

/// The footer hint for the stop-all chord (binary `NZo`/FleetView, byte-exact).
pub const STOP_ALL_HINT: &str =
    "Ctrl+X Ctrl+K stops all running agents and background work at once.";

/// A delete-confirm armed by the first Ctrl-X, awaiting either the second
/// Ctrl-X (confirm delete) or the `Ctrl+X Ctrl+K` chord (stop all).
#[derive(Debug, Clone)]
struct PendingDelete {
    /// The session id the confirm is armed on (the selection at arm time).
    session_id: String,
    /// Whether the armed row is already stopped (terminal `Completed` band) —
    /// selects the confirm hint text.
    stopped: bool,
}

/// The agents-view state machine: rows grouped by band, one selection cursor
/// over the selectable rows.
#[derive(Debug, Clone)]
pub struct AgentsScreenState {
    /// Rows in display order (band-major, insertion order within a band).
    rows: Vec<AgentRow>,
    /// Selected index into `rows` (`None` when empty).
    selected: Option<usize>,
    /// A two-press Ctrl-X delete armed on a row (also the `Ctrl+X` prefix of
    /// the `Ctrl+X Ctrl+K` stop-all chord).
    pending: Option<PendingDelete>,
}

impl AgentsScreenState {
    /// Build from unordered rows: rows are regrouped band-major in the
    /// binary's band order; selection starts on the first row.
    #[must_use]
    pub fn new(mut rows: Vec<AgentRow>) -> Self {
        rows.sort_by_key(AgentRow::band);
        let selected = if rows.is_empty() { None } else { Some(0) };
        Self {
            rows,
            selected,
            pending: None,
        }
    }

    /// Replace the rows (registry refresh after an attach returns), keeping
    /// the cursor on the same session when it still exists.
    pub fn reload(&mut self, rows: Vec<AgentRow>) {
        let keep = self
            .selected
            .and_then(|i| self.rows.get(i))
            .map(|r| r.session_id.clone());
        *self = Self::new(rows);
        if let Some(sid) = keep {
            if let Some(idx) = self.rows.iter().position(|r| r.session_id == sid) {
                self.selected = Some(idx);
            }
        }
    }

    /// The rows in display order.
    #[must_use]
    pub fn rows(&self) -> &[AgentRow] {
        &self.rows
    }

    /// Selected row index (display order).
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The delete-confirm hint shown while a Ctrl-X is armed (binary strings,
    /// byte-exact): a running row → `ctrl+x again to delete`; an already-stopped
    /// row → `stopped · ctrl+x again to delete`. `None` when not armed.
    #[must_use]
    pub fn pending_hint(&self) -> Option<&'static str> {
        self.pending.as_ref().map(|p| {
            if p.stopped {
                "stopped \u{b7} ctrl+x again to delete"
            } else {
                "ctrl+x again to delete"
            }
        })
    }

    /// First Ctrl-X arms a delete-confirm on the selected row; a second Ctrl-X
    /// (while armed) confirms and returns [`AgentsOutcome::Delete`].
    fn on_ctrl_x(&mut self) -> AgentsOutcome {
        if let Some(pending) = self.pending.take() {
            return AgentsOutcome::Delete(pending.session_id);
        }
        if let Some(row) = self.selected.and_then(|i| self.rows.get(i)) {
            if !row.session_id.is_empty() {
                self.pending = Some(PendingDelete {
                    session_id: row.session_id.clone(),
                    stopped: row.band() == Band::Done,
                });
            }
        }
        AgentsOutcome::Stay
    }

    /// Route one key.
    pub fn on_key(&mut self, key: KeyEvent) -> AgentsOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Char('c') && ctrl {
            return AgentsOutcome::Exit;
        }
        // Two-press Ctrl-X delete + the `Ctrl+X Ctrl+K` stop-all chord. These
        // are handled BEFORE the plain-key arms so a Ctrl-modified `x`/`k`
        // never falls through to the `k` navigation binding.
        if ctrl && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')) {
            return self.on_ctrl_x();
        }
        if ctrl && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K')) {
            // Ctrl-K only fires the stop-all chord when Ctrl-X armed it first;
            // otherwise it is inert (never a plain-`k` navigation).
            if self.pending.take().is_some() {
                return AgentsOutcome::StopAll;
            }
            return AgentsOutcome::Stay;
        }
        // Any other key cancels a pending delete-confirm, then is processed
        // normally (so navigation still moves the cursor).
        self.pending = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => AgentsOutcome::Exit,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(i) = self.selected {
                    self.selected = Some(i.saturating_sub(1));
                }
                AgentsOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(i) = self.selected {
                    self.selected = Some((i + 1).min(self.rows.len().saturating_sub(1)));
                }
                AgentsOutcome::Stay
            }
            KeyCode::Enter => match self.selected.and_then(|i| self.rows.get(i)) {
                Some(row) if !row.session_id.is_empty() => {
                    AgentsOutcome::Attach(row.session_id.clone())
                }
                _ => AgentsOutcome::Stay,
            },
            KeyCode::Char('l') | KeyCode::Char('L') => AgentsOutcome::Login,
            _ => AgentsOutcome::Stay,
        }
    }

    /// Render the display lines: per non-empty band a bold section header
    /// (+ dim description), then its rows (`> ` cursor, name, dim state
    /// label, PR reference when present). Pure — unit-testable without a
    /// terminal.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        if self.rows.is_empty() {
            out.push(Line::from(Span::styled(
                "No background agents running".to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )));
            return out;
        }
        for band in Band::ALL {
            let members: Vec<(usize, &AgentRow)> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.band() == band)
                .collect();
            if members.is_empty() {
                continue;
            }
            out.push(Line::from(Span::styled(
                band.label().to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if !band.description().is_empty() {
                out.push(Line::from(Span::styled(
                    band.description().to_string(),
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            for (idx, row) in members {
                let marker = if self.selected == Some(idx) {
                    "> "
                } else {
                    "  "
                };
                let mut spans = vec![
                    Span::raw(marker.to_string()),
                    Span::styled(
                        row.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  [{}]", row.state),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ];
                if let Some(pr) = row.pr {
                    spans.push(Span::styled(
                        format!("  PR #{pr}"),
                        Style::default().fg(Color::Cyan),
                    ));
                }
                out.push(Line::from(spans));
                out.push(Line::from(Span::styled(
                    format!("    {}", row.cwd),
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            out.push(Line::from(""));
        }
        out
    }

    /// Draw the view over the whole frame (same modal contract as
    /// the standalone view render).
    pub fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        // Footer: while a Ctrl-X delete is armed, show the byte-exact confirm
        // hint; otherwise the base key legend (incl. the delete + stop-all
        // chords).
        let footer = self.pending_hint().map_or_else(
            || "enter attach · l connect · ↑/↓ select · ctrl+x delete · q quit".to_string(),
            std::string::ToString::to_string,
        );
        let block = Block::new()
            .borders(Borders::ALL)
            .title(Span::styled(
                "Agents",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::from(Span::styled(
                footer,
                Style::default().add_modifier(Modifier::DIM),
            )))
            .title_bottom(Line::from(Span::styled(
                STOP_ALL_HINT,
                Style::default().add_modifier(Modifier::DIM),
            )));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(self.lines()), inner);
    }
}

/// Drive the view on a live terminal until a terminal outcome (attach or
/// exit): paint, read one event, route keys (press/repeat only), repeat.
/// Terminal IO only — every decision lives in the unit-tested
/// [`AgentsScreenState::on_key`]. The caller owns terminal setup/restore
/// (so it can suspend for an attach and remount).
///
/// # Errors
/// Propagates the first draw/read IO error.
pub fn run_view_loop(
    state: &mut AgentsScreenState,
    terminal: &mut crate::AgentsTerminal,
) -> std::io::Result<AgentsOutcome> {
    run_view_loop_with_tick(state, terminal, None, &mut |_| {})
}

/// [`run_view_loop`] with a periodic refresh tick (M8 cc2.1.198): every
/// `tick` (when `Some`), `on_tick` runs with the state so the caller can
/// reload the registry rows and diff bands for the `agent_needs_input` /
/// `agent_completed` notifications (the binary FleetView re-renders on its
/// jobs poll and runs the `$1f` diff hook `hFc` per render, @222750113).
/// `tick == None` degrades to the pure blocking-read loop.
///
/// # Errors
/// Propagates the first draw/poll/read IO error.
pub fn run_view_loop_with_tick(
    state: &mut AgentsScreenState,
    terminal: &mut crate::AgentsTerminal,
    tick: Option<std::time::Duration>,
    on_tick: &mut dyn FnMut(&mut AgentsScreenState),
) -> std::io::Result<AgentsOutcome> {
    use crossterm::event::{Event, KeyEventKind};
    loop {
        terminal.draw(|f| state.render(f))?;
        if let Some(interval) = tick {
            if !crossterm::event::poll(interval)? {
                on_tick(state);
                continue;
            }
        }
        match crossterm::event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match state.on_key(key) {
                AgentsOutcome::Stay => {}
                outcome => return Ok(outcome),
            },
            // Resize/focus/paste: repaint on the next iteration.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sid: &str, name: &str, state: &str) -> AgentRow {
        AgentRow {
            session_id: sid.to_string(),
            name: name.to_string(),
            state: state.to_string(),
            kind: "background".to_string(),
            cwd: "/p".to_string(),
            pr: None,
        }
    }

    #[test]
    fn band_labels_match_binary_bytes() {
        // Binary MFc (@222792125): review/blocked/working/done labels.
        assert_eq!(Band::Review.label(), "Ready for review");
        assert_eq!(Band::Blocked.label(), "Needs input");
        assert_eq!(Band::Working.label(), "Working");
        assert_eq!(Band::Done.label(), "Completed");
        assert_eq!(
            Band::Blocked.description(),
            "Sessions that have a question or need your decision land here"
        );
        assert_eq!(
            Band::Working.description(),
            format!("Sessions {} is actively working on \u{2014} they keep running even if you close the terminal", branding::PRODUCT_NAME)
        );
        assert_eq!(
            Band::Done.description(),
            "Finished sessions wait here for you to review"
        );
    }

    #[test]
    fn bands_are_stable_terminal_states_stay_completed() {
        // 2.1.196 fix downstream: merged terminal states land in Completed
        // and can never render under "Needs input".
        assert_eq!(band_for_state("done"), Band::Done);
        assert_eq!(band_for_state("failed"), Band::Done);
        assert_eq!(band_for_state("stopped"), Band::Done);
        assert_eq!(band_for_state("blocked"), Band::Blocked);
        assert_eq!(band_for_state("working"), Band::Working);
    }

    #[test]
    fn pr_reference_extracted_from_result_text() {
        // Binary Hon: `#123` token or a `/pull/123` URL.
        assert_eq!(extract_pr_number("opened PR #482 for review"), Some(482));
        assert_eq!(
            extract_pr_number("see https://github.com/o/r/pull/77 please"),
            Some(77)
        );
        assert_eq!(extract_pr_number("issue #ab and pull things"), None);
        assert_eq!(extract_pr_number("no refs here"), None);
    }

    #[test]
    fn rows_group_band_major_in_band_order() {
        let s = AgentsScreenState::new(vec![
            row("a", "done-job", "done"),
            row("b", "live-job", "working"),
            row("c", "asks", "blocked"),
        ]);
        let bands: Vec<Band> = s.rows().iter().map(AgentRow::band).collect();
        assert_eq!(bands, [Band::Blocked, Band::Working, Band::Done]);
    }

    #[test]
    fn navigation_and_attach() {
        let mut s = AgentsScreenState::new(vec![
            row("a", "asks", "blocked"),
            row("b", "runs", "working"),
        ]);
        assert_eq!(s.selected(), Some(0));
        assert_eq!(s.on_key(key(KeyCode::Down)), AgentsOutcome::Stay);
        assert_eq!(s.selected(), Some(1));
        // Clamped at the end.
        assert_eq!(s.on_key(key(KeyCode::Down)), AgentsOutcome::Stay);
        assert_eq!(s.selected(), Some(1));
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            AgentsOutcome::Attach("b".to_string())
        );
        assert_eq!(s.on_key(key(KeyCode::Up)), AgentsOutcome::Stay);
        assert_eq!(
            s.on_key(key(KeyCode::Enter)),
            AgentsOutcome::Attach("a".to_string())
        );
    }

    #[test]
    fn login_key_routes_without_moving_selection() {
        let mut s =
            AgentsScreenState::new(vec![row("a", "one", "working"), row("b", "two", "working")]);
        let _ = s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected(), Some(1));
        assert_eq!(s.on_key(key(KeyCode::Char('l'))), AgentsOutcome::Login);
        assert_eq!(s.selected(), Some(1));
    }

    #[test]
    fn quit_keys_exit() {
        let mut s = AgentsScreenState::new(vec![row("a", "x", "working")]);
        assert_eq!(s.on_key(key(KeyCode::Char('q'))), AgentsOutcome::Exit);
        assert_eq!(s.on_key(key(KeyCode::Esc)), AgentsOutcome::Exit);
        assert_eq!(
            s.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            AgentsOutcome::Exit
        );
    }

    #[test]
    fn empty_view_shows_binary_empty_line_and_enter_is_inert() {
        let mut s = AgentsScreenState::new(vec![]);
        assert_eq!(s.selected(), None);
        assert_eq!(s.on_key(key(KeyCode::Enter)), AgentsOutcome::Stay);
        let lines = s.lines();
        assert_eq!(lines.len(), 1);
        // Binary empty-list line (@220704315), byte-exact.
        assert_eq!(
            lines[0].spans[0].content.as_ref(),
            "No background agents running"
        );
    }

    #[test]
    fn reload_keeps_selection_on_same_session() {
        let mut s =
            AgentsScreenState::new(vec![row("a", "one", "working"), row("b", "two", "working")]);
        let _ = s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected(), Some(1)); // on "b"
        s.reload(vec![
            row("c", "new", "blocked"),
            row("b", "two", "working"),
            row("a", "one", "working"),
        ]);
        let idx = s.selected().unwrap();
        assert_eq!(s.rows()[idx].session_id, "b");
    }

    #[test]
    fn lines_show_pr_reference_on_row() {
        let mut r = row("a", "fixed the bug", "done");
        r.pr = extract_pr_number("merged https://github.com/o/r/pull/9182");
        let s = AgentsScreenState::new(vec![r]);
        let text: String = s
            .lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.clone()))
            .collect();
        assert!(text.contains("PR #9182"), "PR ref missing: {text}");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_x_two_press_arms_then_confirms_delete() {
        let mut s = AgentsScreenState::new(vec![row("sid-a", "runs", "working")]);
        // First Ctrl-X arms the confirm on the selected running row.
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert_eq!(s.pending_hint(), Some("ctrl+x again to delete"));
        // Second Ctrl-X confirms the delete of the armed session.
        assert_eq!(
            s.on_key(ctrl(KeyCode::Char('x'))),
            AgentsOutcome::Delete("sid-a".to_string())
        );
        // Disarmed after confirm.
        assert_eq!(s.pending_hint(), None);
    }

    #[test]
    fn ctrl_x_on_stopped_row_shows_stopped_hint() {
        // A terminal (Completed band) row arms with the `stopped · …` hint.
        let mut s = AgentsScreenState::new(vec![row("sid-done", "finished", "done")]);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert_eq!(
            s.pending_hint(),
            Some("stopped \u{b7} ctrl+x again to delete")
        );
    }

    #[test]
    fn ctrl_x_then_ctrl_k_stops_all() {
        // The `Ctrl+X Ctrl+K` chord: Ctrl-X arms, Ctrl-K fires stop-all.
        let mut s = AgentsScreenState::new(vec![row("sid-a", "runs", "working")]);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('k'))), AgentsOutcome::StopAll);
        assert_eq!(s.pending_hint(), None);
    }

    #[test]
    fn ctrl_k_alone_is_inert_not_navigation() {
        // Ctrl-K without a preceding Ctrl-X is a no-op — and must NOT move the
        // cursor like a plain `k`.
        let mut s =
            AgentsScreenState::new(vec![row("a", "one", "working"), row("b", "two", "working")]);
        let _ = s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected(), Some(1));
        assert_eq!(s.on_key(ctrl(KeyCode::Char('k'))), AgentsOutcome::Stay);
        assert_eq!(s.selected(), Some(1), "Ctrl-K did not navigate");
        assert_eq!(s.pending_hint(), None);
    }

    #[test]
    fn navigation_cancels_pending_delete_confirm() {
        let mut s =
            AgentsScreenState::new(vec![row("a", "one", "working"), row("b", "two", "working")]);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert!(s.pending_hint().is_some());
        // Moving the selection disarms; the move still happens.
        assert_eq!(s.on_key(key(KeyCode::Down)), AgentsOutcome::Stay);
        assert_eq!(s.pending_hint(), None);
        assert_eq!(s.selected(), Some(1));
        // A single Ctrl-X now only RE-ARMS (does not immediately delete).
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert_eq!(s.pending_hint(), Some("ctrl+x again to delete"));
    }

    #[test]
    fn ctrl_x_on_empty_view_is_inert() {
        let mut s = AgentsScreenState::new(vec![]);
        assert_eq!(s.on_key(ctrl(KeyCode::Char('x'))), AgentsOutcome::Stay);
        assert_eq!(s.pending_hint(), None);
    }

    #[test]
    fn stop_all_hint_is_byte_exact() {
        // Binary FleetView footer (@195140238), byte-exact.
        assert_eq!(
            STOP_ALL_HINT,
            "Ctrl+X Ctrl+K stops all running agents and background work at once."
        );
    }
}
