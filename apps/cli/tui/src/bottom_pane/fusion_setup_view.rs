//! `/fusion setup`: the four-step model-configuration wizard, backed by the
//! pure [`crate::fusion::setup`] reducer.
//!
//! Modeled on [`crate::bottom_pane::web_picker_view::WebPickerView`] — arrows
//! move, typing filters, `Esc` cancels — with two differences the shape of the
//! task demands: the panel step is MULTI-select (and ordered, because a preset
//! takes a prefix of the roster), and the flow only produces an effect on the
//! final review step, so a half-answered wizard writes nothing.

use std::any::Any;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, FusionSetupAction, ViewOutcome};
use crate::fusion::setup::{
    handle_fusion_setup_key, FusionSetupOutcome, FusionSetupSnapshot, FusionSetupState,
    FusionSetupStep, STEP_COUNT,
};
use crate::renderable::Renderable;

/// Rows shown before the list scrolls.
const VIEWPORT: usize = 10;

/// What the wizard says when the session has no models at all — the same
/// wording `/model`'s empty picker uses, so the two agree about the cause.
const EMPTY_MESSAGE: &str = "No models available. Configure a provider to enable /fusion.";

/// The `/fusion setup` wizard view.
pub struct FusionSetupView {
    state: FusionSetupState,
}

impl FusionSetupView {
    /// Open the wizard over `snapshot` (live catalog + current settings).
    #[must_use]
    pub fn new(snapshot: FusionSetupSnapshot) -> Self {
        Self {
            state: FusionSetupState::from_snapshot(snapshot),
        }
    }

    /// The wizard state, for tests and for the owner's assertions.
    #[must_use]
    pub fn state(&self) -> &FusionSetupState {
        &self.state
    }

    fn title(&self) -> String {
        let step = self.state.step;
        let label = match step {
            FusionSetupStep::Panels => "Panel models",
            FusionSetupStep::Analyst => "Analyst model",
            FusionSetupStep::Confirm => "Review",
        };
        format!(
            "Fusion setup · Step {}/{STEP_COUNT} · {label}",
            step.number()
        )
    }

    fn list_lines(&self) -> Vec<Line<'static>> {
        let visible = self.state.visible();
        if visible.is_empty() {
            let message = if self.state.snapshot.candidates.is_empty() {
                EMPTY_MESSAGE.to_string()
            } else if self.state.step == FusionSetupStep::Analyst && self.state.query.is_empty() {
                // Distinct from "your search matched nothing": every model this
                // session can reach is on a provider whose codec cannot carry a
                // JSON schema, so there is no analyst to pick at all.
                "No connected model can emit structured output, which the analyst \
                 requires. Connect a provider that can."
                    .to_string()
            } else {
                "No models match.".to_string()
            };
            return vec![Line::from(Span::styled(
                message,
                Style::default().add_modifier(Modifier::DIM),
            ))];
        }
        let start = self.state.selected.saturating_sub(VIEWPORT - 1);
        visible
            .iter()
            .enumerate()
            .skip(start)
            .take(VIEWPORT)
            .map(|(position, &index)| {
                let candidate = &self.state.snapshot.candidates[index];
                let highlighted = position == self.state.selected;
                let marker = match self.state.panel_position(&candidate.choice) {
                    Some(order) if self.state.step == FusionSetupStep::Panels => {
                        format!("{order} ")
                    }
                    _ if self.state.is_picked(&candidate.choice) => "● ".to_string(),
                    _ => "  ".to_string(),
                };
                let cursor = if highlighted { "❯" } else { " " };
                let session = if candidate.is_session_model {
                    " · current"
                } else {
                    ""
                };
                let style = if highlighted {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(
                    format!(
                        "{cursor} {marker}{:<28} {}{session}",
                        candidate.display, candidate.provider_label
                    ),
                    style,
                ))
            })
            .collect()
    }

    fn review_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let roster = self
            .state
            .panels
            .iter()
            .enumerate()
            .map(|(index, choice)| format!("  {}. {choice}", index + 1))
            .collect::<Vec<_>>();
        lines.push(Line::from("Panels"));
        lines.extend(roster.into_iter().map(Line::from));
        lines.push(Line::from(format!(
            "Analyst     {}",
            self.state
                .analyst
                .as_ref()
                .map_or_else(|| "—".to_string(), ToString::to_string)
        )));
        lines.push(Line::from(
            "Final synthesis uses the parent conversation model.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "[{}] Also enable Fusion for agents and workflows (fusion.enabled)",
            if self.state.enable { "x" } else { " " }
        )));
        lines.push(Line::from(Span::styled(
            "/fusion works either way; this switch only adds the Fusion agent and \
             parent-session synthesis.",
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines
    }

    fn footer(&self) -> String {
        match self.state.step {
            FusionSetupStep::Panels => {
                "type to filter · Space pick/unpick · Enter next · Esc cancel".to_string()
            }
            FusionSetupStep::Analyst => {
                "type to filter · Enter pick & next · ← back · Esc cancel".to_string()
            }
            FusionSetupStep::Confirm => "Enter save · e toggle · ← back · Esc cancel".to_string(),
        }
    }

    fn body(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(role) = self.state.step.role() {
            lines.push(Line::from(Span::styled(
                role.description().to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(Line::from(format!("Filter: {}", self.state.query)));
            lines.extend(self.list_lines());
        } else {
            lines.extend(self.review_lines());
        }
        if let Some(notice) = self.state.notice.as_ref() {
            lines.push(Line::from(Span::styled(
                notice.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(Span::styled(
            self.footer(),
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines
    }
}

impl Renderable for FusionSetupView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = self.body();
        let title = self.title();
        let content_width = lines
            .iter()
            .map(ratatui::text::Line::width)
            .max()
            .unwrap_or(0)
            .max(title.chars().count());
        let width = u16::try_from(content_width + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(40);
        let height = u16::try_from(lines.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.body().len() + 2).unwrap_or(u16::MAX)
    }
}

impl BottomPaneView for FusionSetupView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_fusion_setup_key(&mut self.state, key.code) {
            FusionSetupOutcome::Stay => ViewOutcome::Pending,
            FusionSetupOutcome::Cancel => ViewOutcome::Cancelled,
            FusionSetupOutcome::Save { roles, enable } => {
                ViewOutcome::RunFusionSetupAction(FusionSetupAction::Save { roles, enable })
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion::setup::FusionCandidate;
    use crossterm::event::{KeyCode, KeyModifiers};
    use lingxi_core::host::FusionModelChoice;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn candidate(profile: &str, model: &str, display: &str, judge: bool) -> FusionCandidate {
        FusionCandidate {
            choice: FusionModelChoice::new(profile, model),
            display: display.to_string(),
            provider_label: profile.to_string(),
            analyst_capable: judge,
            suggested_rank: 0,
            is_session_model: false,
        }
    }

    fn view() -> FusionSetupView {
        FusionSetupView::new(FusionSetupSnapshot {
            candidates: vec![
                candidate("anthropic", "claude-opus-5", "Claude Opus 5", true),
                candidate("openai", "gpt-5.6-sol", "GPT-5.6 Sol", true),
                candidate("google", "gemini-3-pro", "Gemini 3 Pro", false),
            ],
            max_panel: 8,
            ..FusionSetupSnapshot::default()
        })
    }

    fn render_text(view: &FusionSetupView) -> String {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        Renderable::render(view, area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_wizard_only_emits_an_effect_on_the_final_step() {
        let mut view = view();
        for code in [
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
        ] {
            assert!(
                matches!(view.handle_key(key(code)), ViewOutcome::Pending),
                "a half-answered wizard must write nothing; {code:?} produced an effect"
            );
        }
        let outcome = view.handle_key(key(KeyCode::Enter));
        let ViewOutcome::RunFusionSetupAction(FusionSetupAction::Save { roles, .. }) = outcome
        else {
            panic!("the review step must save");
        };
        assert!(roles.is_configured());
    }

    #[test]
    fn escape_cancels_without_an_effect() {
        let mut view = view();
        assert!(matches!(
            view.handle_key(key(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn the_panel_step_renders_the_roster_order_it_will_save() {
        let mut view = view();
        // Pick the SECOND row first, then the first: the badges must show the
        // pick order, not the list order, because a preset takes a prefix of
        // the roster.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Char(' ')));
        view.handle_key(key(KeyCode::Up));
        view.handle_key(key(KeyCode::Char(' ')));
        let text = render_text(&view);
        let sol = text.lines().find(|l| l.contains("GPT-5.6 Sol")).unwrap();
        let opus = text.lines().find(|l| l.contains("Claude Opus 5")).unwrap();
        assert!(sol.contains("1 GPT-5.6 Sol"), "{sol}");
        assert!(opus.contains("2 Claude Opus 5"), "{opus}");
    }

    #[test]
    fn the_analyst_step_explains_an_empty_list_rather_than_showing_nothing() {
        let mut view = FusionSetupView::new(FusionSetupSnapshot {
            candidates: vec![
                candidate("google", "gemini-3-pro", "Gemini 3 Pro", false),
                candidate("google", "gemini-3-flash", "Gemini 3 Flash", false),
            ],
            max_panel: 8,
            ..FusionSetupSnapshot::default()
        });
        view.handle_key(key(KeyCode::Char(' ')));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Char(' ')));
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.state().step, FusionSetupStep::Analyst);
        let text = render_text(&view);
        assert!(
            text.contains("structured output"),
            "an empty analyst list must say WHY, not just look empty:\n{text}"
        );
    }

    #[test]
    fn the_review_step_shows_every_role_and_the_enable_switch() {
        let mut view = view();
        for code in [
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
        ] {
            view.handle_key(key(code));
        }
        let text = render_text(&view);
        assert!(text.contains("Panels"), "{text}");
        assert!(text.contains("Analyst"), "{text}");
        assert!(text.contains("parent conversation model"), "{text}");
        assert!(text.contains("[ ] Also enable"), "{text}");
        view.handle_key(key(KeyCode::Char('e')));
        assert!(render_text(&view).contains("[x] Also enable"));
    }

    /// Same hazard `tasks_view::fusion_rendering_is_safe_for_narrow_buffers`
    /// covers: a width computation that underflows or a rect wider than the
    /// area panics inside ratatui rather than drawing something narrow.
    #[test]
    fn the_wizard_draws_without_panicking_at_every_realistic_width() {
        let mut view = view();
        for step in 0..4 {
            for width in [20_u16, 40, 60, 80, 120, 200] {
                for height in [6_u16, 12, 30] {
                    let area = Rect::new(0, 0, width, height);
                    let mut buf = Buffer::empty(area);
                    Renderable::render(&view, area, &mut buf);
                    assert!(view.desired_height(width) > 0);
                }
            }
            // Advance one step per outer pass so all four render shapes are
            // covered, including the review step's different body.
            view.handle_key(key(KeyCode::Char(' ')));
            view.handle_key(key(KeyCode::Down));
            view.handle_key(key(KeyCode::Char(' ')));
            view.handle_key(key(KeyCode::Enter));
            let _ = step;
        }
        assert_eq!(view.state().step, FusionSetupStep::Confirm);
    }

    /// A long roster must not run the list off the bottom of a short pane.
    #[test]
    fn a_roster_longer_than_the_viewport_scrolls_with_the_highlight() {
        let candidates: Vec<_> = (0..24)
            .map(|index| {
                candidate(
                    "openrouter",
                    &format!("m-{index}"),
                    &format!("Model {index}"),
                    true,
                )
            })
            .collect();
        let mut view = FusionSetupView::new(FusionSetupSnapshot {
            candidates,
            max_panel: 8,
            ..FusionSetupSnapshot::default()
        });
        for _ in 0..23 {
            view.handle_key(key(KeyCode::Down));
        }
        let text = render_text(&view);
        assert!(
            text.contains("Model 23"),
            "the highlight must stay visible:\n{text}"
        );
        assert!(
            !text.contains("Model 0 "),
            "the viewport must have scrolled past the top of a 24-row list"
        );
    }

    #[test]
    fn an_empty_catalog_names_the_cause_instead_of_rendering_a_blank_list() {
        let view = FusionSetupView::new(FusionSetupSnapshot {
            max_panel: 8,
            ..FusionSetupSnapshot::default()
        });
        assert!(render_text(&view).contains("No models available"));
    }
}
