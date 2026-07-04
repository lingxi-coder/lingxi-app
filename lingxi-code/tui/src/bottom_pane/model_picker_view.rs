//! Scrollable model picker view for `/model`: a centered, bordered selection
//! list over the session's [`ModelRow`]s (ported from the former
//! `picker::ModelPicker`, plan Phases 4 + 11).
//!
//! Unlike the read-only [`crate::bottom_pane::screen_view::ScreenView`] it is
//! INTERACTIVE — arrow keys move a highlight (the viewport follows), `Enter`
//! confirms the model ([`ViewOutcome::SwitchModel`]), `Esc` cancels. The
//! currently active model is marked with a `●`. An empty model list renders
//! its explanation INSIDE the view (plan Phase 11 step 5 — previously an
//! app-side transcript message). Modeled on codex's `list_selection_view` +
//! the modal contract of [`crate::bottom_pane::dialog_view::DialogView`].

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewOutcome};
use crate::renderable::Renderable;
use crate::session::ModelRow;

/// Rows shown in the picker viewport before it scrolls.
const VIEWPORT: usize = 12;

/// What the picker shows when the session has no models (plan Phase 11
/// step 5: the empty state lives IN the view, not in the transcript). The
/// first sentence is the pre-Phase-11 app-side message, byte-preserved.
const EMPTY_MESSAGE: &str = "No models available. Configure a provider to enable /model.";

/// An interactive, scrollable, search-filterable model selection list.
pub struct ModelPickerView {
    /// The full grouped set (every eligible provider's rows). The search
    /// `query` filters this into [`Self::rows`]; a connected aggregator like
    /// OpenRouter contributes hundreds of models, so type-to-filter is what
    /// keeps the picker navigable.
    all_rows: Vec<ModelRow>,
    /// The currently visible rows — `all_rows` filtered by `query`.
    rows: Vec<ModelRow>,
    /// Case-insensitive search filter (matches display name, provider label,
    /// or wire id). Empty → every row shows.
    query: String,
    selected: usize,
    /// Top row index of the scroll window.
    offset: usize,
}

impl ModelPickerView {
    /// Build a picker over `rows`, starting the highlight on the current model
    /// (the row with `is_current`), or the first row when none is marked.
    #[must_use]
    pub fn new(rows: Vec<ModelRow>) -> Self {
        let all_rows = group_by_provider(rows);
        let selected = all_rows.iter().position(|r| r.is_current).unwrap_or(0);
        let offset = selected.saturating_sub(VIEWPORT - 1);
        Self {
            rows: all_rows.clone(),
            all_rows,
            query: String::new(),
            selected,
            offset,
        }
    }

    /// Recompute the visible `rows` from `all_rows` against the current
    /// `query` (case-insensitive substring on display / provider / wire id),
    /// then re-anchor the highlight on the current model if it survives the
    /// filter, else the first match.
    fn apply_filter(&mut self) {
        let q = self.query.to_ascii_lowercase();
        self.rows = self
            .all_rows
            .iter()
            .filter(|r| {
                q.is_empty()
                    || r.display.to_ascii_lowercase().contains(&q)
                    || r.provider_label.to_ascii_lowercase().contains(&q)
                    || r.request_model.to_ascii_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        self.selected = self.rows.iter().position(|r| r.is_current).unwrap_or(0);
        self.offset = self.selected.saturating_sub(VIEWPORT - 1);
    }

    /// Number of dim provider header rows the render emits — one per distinct
    /// NON-EMPTY provider group (rows with an empty provider label render no
    /// header, matching [`Renderable::render`]).
    fn group_count(&self) -> usize {
        let mut n = 0usize;
        let mut prev: Option<&str> = None;
        for r in &self.rows {
            if prev != Some(r.provider_label.as_str()) {
                prev = Some(r.provider_label.as_str());
                if !r.provider_label.is_empty() {
                    n += 1;
                }
            }
        }
        n
    }

    /// Whether the picker has any rows to choose from.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The currently highlighted row index (exposed for tests).
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The rows currently shown (grouped by provider), for inspection/tests.
    #[must_use]
    pub(crate) fn rows(&self) -> &[ModelRow] {
        &self.rows
    }

    /// Keep the highlighted row inside the scroll window.
    fn follow(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + VIEWPORT {
            self.offset = self.selected + 1 - VIEWPORT;
        }
    }

    fn max_width(&self) -> usize {
        if self.rows.is_empty() {
            return EMPTY_MESSAGE.chars().count();
        }
        // The widest of a model row (` › ● {display}` = display + 4 gutter cols)
        // and a provider header (its label). Both must fit inside the modal.
        let widest_model = self
            .rows
            .iter()
            .map(|r| r.display.chars().count() + 4)
            .max()
            .unwrap_or(0);
        let widest_header = self
            .rows
            .iter()
            .map(|r| r.provider_label.chars().count())
            .max()
            .unwrap_or(0);
        widest_model.max(widest_header)
    }
}

/// Stable-group rows under their provider so same-provider models render
/// contiguously beneath one dim provider header. Provider order is
/// first-appearance (the catalog's routable-first ordering); within a provider
/// the incoming order is preserved (stable sort).
fn group_by_provider(mut rows: Vec<ModelRow>) -> Vec<ModelRow> {
    let mut order: Vec<String> = Vec::new();
    for r in &rows {
        if !order.iter().any(|l| l == &r.provider_label) {
            order.push(r.provider_label.clone());
        }
    }
    rows.sort_by_key(|r| {
        order
            .iter()
            .position(|l| l == &r.provider_label)
            .unwrap_or(usize::MAX)
    });
    // De-duplicate models that appear more than once under the same provider
    // header (a live/routable listing plus its catalog twin surface the same
    // `(provider, display)` — e.g. GLM-5.1 twice). Keep the first, but let a
    // later `is_current` row win so the active model stays highlighted.
    let mut out: Vec<ModelRow> = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(existing) = out
            .iter_mut()
            .find(|e| e.provider_label == r.provider_label && e.display == r.display)
        {
            if r.is_current && !existing.is_current {
                *existing = r;
            }
        } else {
            out.push(r);
        }
    }
    out
}

impl Renderable for ModelPickerView {
    /// Draw the picker centered over `area`, clearing the buffer beneath it.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let width = u16::try_from(self.max_width() + 6)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(24);
        let visible = self.rows.len().min(VIEWPORT);
        // A "Search:" filter row sits above the list whenever there are any
        // models (present iff the catalog is non-empty, independent of the
        // current filter). + group_count() for the dim provider header rows.
        let search_row = usize::from(!self.all_rows.is_empty());
        let height = u16::try_from(visible + self.group_count() + 4 + search_row)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Select model");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let end = (self.offset + VIEWPORT).min(self.rows.len());
        let mut lines: Vec<Line> = Vec::with_capacity(visible + self.group_count() + 3);
        if self.all_rows.is_empty() {
            lines.push(Line::from(EMPTY_MESSAGE));
        } else {
            lines.push(Line::from(format!("Search: {}", self.query)));
            if self.rows.is_empty() {
                lines.push(Line::from("No models match."));
            }
        }
        // Group the visible rows under a dim provider header: a header is
        // emitted whenever the provider changes from the previous rendered row
        // (including the first row of the window). The model rows below it show
        // only the model name (the provider is the header now).
        let mut prev_label: Option<&str> = None;
        for (i, row) in self.rows[self.offset..end].iter().enumerate() {
            let idx = self.offset + i;
            if prev_label != Some(row.provider_label.as_str()) {
                prev_label = Some(row.provider_label.as_str());
                if !row.provider_label.is_empty() {
                    lines.push(Line::from(Span::styled(
                        row.provider_label.clone(),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
            }
            let marker = if row.is_current { "● " } else { "  " };
            let caret = if idx == self.selected { "› " } else { "  " };
            let style = if idx == self.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            // Models are indented one column under their provider header.
            lines.push(Line::from(Span::styled(
                format!(" {caret}{marker}{}", row.display),
                style,
            )));
        }
        let hint = if self.all_rows.is_empty() {
            "Esc close"
        } else {
            "↑/↓ · type to filter · Esc"
        };
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }

    /// The bottom-viewport rows the picker claims: its visible model rows + one
    /// dim header per provider group + the "Search:" row + modal chrome. Empty
    /// catalogs keep the 4-row chrome, which fits the in-view empty message +
    /// hint (no search row).
    fn desired_height(&self, _width: u16) -> u16 {
        let search_row = usize::from(!self.all_rows.is_empty());
        u16::try_from(self.rows.len().min(12) + self.group_count() + search_row).unwrap_or(0) + 4
    }
}

impl BottomPaneView for ModelPickerView {
    /// Route a key: arrows move the highlight (viewport follows, clamped at
    /// the list edges — locked behavior), `Enter` confirms, `Esc` cancels.
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.follow();
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                self.follow();
                ViewOutcome::Pending
            }
            KeyCode::Enter => self
                .rows
                .get(self.selected)
                .map_or(ViewOutcome::Cancelled, |r| ViewOutcome::SwitchModel {
                    request_model: r.request_model.clone(),
                    profile: r.profile.clone(),
                }),
            KeyCode::Esc => ViewOutcome::Cancelled,
            // Type-to-filter: edit the search query and re-filter in place.
            KeyCode::Char(c) => {
                self.query.push(c);
                self.apply_filter();
                ViewOutcome::Pending
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.apply_filter();
                ViewOutcome::Pending
            }
            _ => ViewOutcome::Pending,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn rows() -> Vec<ModelRow> {
        vec![
            ModelRow {
                display: "Opus".into(),
                request_model: "claude-opus".into(),
                profile: Some("anthropic".into()),
                provider_label: "Anthropic".into(),
                is_current: false,
            },
            ModelRow {
                display: "Sonnet".into(),
                request_model: "claude-sonnet".into(),
                profile: Some("anthropic".into()),
                provider_label: "Anthropic".into(),
                is_current: true,
            },
        ]
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn starts_on_current_model() {
        let p = ModelPickerView::new(rows());
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_confirms_selected_request_model_and_profile() {
        let mut p = ModelPickerView::new(rows());
        p.handle_key(press(KeyCode::Up)); // move to Opus
        assert_eq!(p.selected(), 0);
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::SwitchModel { ref request_model, ref profile }
                if request_model == "claude-opus" && profile.as_deref() == Some("anthropic")
        ));
    }

    #[test]
    fn groups_by_provider_and_dedups_within_a_group() {
        // Two providers, one with a duplicate model (a live + catalog twin).
        let rows = vec![
            ModelRow {
                display: "GLM-5.1".into(),
                request_model: "glm-5.1".into(),
                profile: Some("glm-coding".into()),
                provider_label: "GLM (coding)".into(),
                is_current: false,
            },
            ModelRow {
                display: "GPT-5.5".into(),
                request_model: "gpt-5.5".into(),
                profile: Some("openai".into()),
                provider_label: "OpenAI".into(),
                is_current: true,
            },
            // Same (provider, display) as the first row — must be de-duped.
            ModelRow {
                display: "GLM-5.1".into(),
                request_model: "glm-5.1".into(),
                profile: Some("glm-coding".into()),
                provider_label: "GLM (coding)".into(),
                is_current: false,
            },
        ];
        let p = ModelPickerView::new(rows);
        // GLM-5.1 appears once; the two providers stay grouped (2 rows total).
        assert_eq!(p.rows.len(), 2, "the duplicate GLM-5.1 is removed");
        assert_eq!(p.group_count(), 2, "two provider headers");
        // The current GPT-5.5 remains selectable + highlighted.
        assert_eq!(p.rows[p.selected()].display, "GPT-5.5");
        let text = render_text(&p, Rect::new(0, 0, 60, 12));
        assert!(text.contains("GLM (coding)") && text.contains("OpenAI"), "{text}");
        assert_eq!(text.matches("GLM-5.1").count(), 1, "no duplicate row: {text}");
    }

    #[test]
    fn esc_cancels() {
        let mut p = ModelPickerView::new(rows());
        assert!(matches!(
            p.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn model_picker_navigation_clamps_at_edges_by_design() {
        // Plan Phase 12 decision: clamp-at-edges is the deliberate LingXi
        // navigation behavior (no wrap-around) across dialog/picker/completion.
        let mut p = ModelPickerView::new(rows());
        p.handle_key(press(KeyCode::Down)); // already last (index 1), clamps
        assert_eq!(p.selected(), 1);
        p.handle_key(press(KeyCode::Up));
        p.handle_key(press(KeyCode::Up)); // clamps at 0 — does NOT wrap
        assert_eq!(p.selected(), 0);
    }

    #[test]
    fn empty_picker_enter_cancels() {
        let mut p = ModelPickerView::new(Vec::new());
        assert!(p.is_empty());
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn empty_picker_renders_its_explanation_in_the_view() {
        // Plan Phase 11 step 5: the empty state is a user-facing message
        // INSIDE the picker, not an app-side transcript dump.
        let p = ModelPickerView::new(Vec::new());
        assert_eq!(p.desired_height(80), 4, "chrome fits message + hint");
        let area = Rect::new(0, 0, 80, 6);
        let text = render_text(&p, area);
        assert!(text.contains("Select model"), "{text}");
        assert!(
            text.contains("No models available. Configure a provider to enable /model."),
            "{text}"
        );
        assert!(text.contains("Esc close"), "empty-state hint: {text}");
        assert!(!text.contains("Enter switch"), "no switch hint: {text}");
    }

    fn many_rows(n: usize, current: Option<usize>) -> Vec<ModelRow> {
        (0..n)
            .map(|i| ModelRow {
                display: format!("model-{i:02}"),
                request_model: format!("model-{i:02}"),
                profile: None,
                provider_label: String::new(),
                is_current: Some(i) == current,
            })
            .collect()
    }

    fn render_text(p: &ModelPickerView, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn long_list_scrolls_to_keep_the_highlight_visible() {
        // 30 rows, 12 visible: walking to the end scrolls the window so the
        // highlighted row is always rendered; walking back scrolls it up.
        let mut p = ModelPickerView::new(many_rows(30, None));
        for _ in 0..29 {
            p.handle_key(press(KeyCode::Down));
        }
        assert_eq!(p.selected(), 29);
        let area = Rect::new(0, 0, 60, 20);
        let text = render_text(&p, area);
        assert!(text.contains("›   model-29"), "highlight visible: {text}");
        assert!(!text.contains("model-00"), "top rows scrolled out: {text}");
        for _ in 0..29 {
            p.handle_key(press(KeyCode::Up));
        }
        let text = render_text(&p, area);
        assert!(text.contains("›   model-00"), "{text}");
        assert!(
            !text.contains("model-29"),
            "bottom rows scrolled out: {text}"
        );
    }

    #[test]
    fn starts_scrolled_to_a_current_model_deep_in_a_long_list() {
        let p = ModelPickerView::new(many_rows(30, Some(25)));
        assert_eq!(p.selected(), 25);
        let text = render_text(&p, Rect::new(0, 0, 60, 20));
        assert!(
            text.contains("› ● model-25"),
            "current model highlighted and visible: {text}"
        );
    }

    #[test]
    fn wide_layout_centers_the_picker_within_the_area() {
        let p = ModelPickerView::new(rows());
        let area = Rect::new(0, 0, 120, 40);
        let text = render_text(&p, area);
        let title_row = text
            .lines()
            .find(|l| l.contains("Select model"))
            .expect("title row");
        let border = title_row.find('┌').expect("left border");
        assert!(
            border > 30,
            "modal centered in a 120-col frame (left border at {border})"
        );
        assert!(text.contains("› ● Sonnet"), "{text}");
    }

    #[test]
    fn narrow_layout_clips_rows_without_panicking() {
        let p = ModelPickerView::new(many_rows(30, Some(5)));
        let area = Rect::new(0, 0, 40, 12);
        let text = render_text(&p, area);
        assert!(text.contains("Select model"), "{text}");
        assert!(text.contains("› ● model-05"), "{text}");
        // Keys still work at narrow sizes.
        let mut p = p;
        assert!(matches!(
            p.handle_key(press(KeyCode::Enter)),
            ViewOutcome::SwitchModel { ref request_model, .. } if request_model == "model-05"
        ));
    }

    #[test]
    fn desired_height_is_rows_plus_headers_plus_chrome() {
        // 2 Anthropic models + 1 provider header + 1 Search row + 4 chrome = 8.
        assert_eq!(ModelPickerView::new(rows()).desired_height(80), 8);
        let many: Vec<ModelRow> = (0..30)
            .map(|i| ModelRow {
                display: format!("m{i}"),
                request_model: format!("m{i}"),
                profile: None,
                provider_label: String::new(),
                is_current: false,
            })
            .collect();
        // Caps at the 12-row scroll viewport + 1 Search row + 4 chrome.
        assert_eq!(ModelPickerView::new(many).desired_height(80), 17);
    }

    #[test]
    fn render_centers_list_with_current_marker_into_buffer() {
        let p = ModelPickerView::new(rows());
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);
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
        assert!(text.contains("Select model"), "{text}");
        // The search filter row sits above the list.
        assert!(text.contains("Search:"), "search row present: {text}");
        // Models group under a dim provider header; rows show just the model.
        assert!(text.contains("Anthropic"), "provider header: {text}");
        assert!(text.contains("Opus"), "{text}");
        // Sonnet is both current (●) and the starting highlight (›).
        assert!(text.contains("› ● Sonnet"), "{text}");
        // The footer hint advertises the filter affordance.
        assert!(text.contains("type to filter"), "{text}");
    }

    #[test]
    fn typing_filters_the_visible_models_and_backspace_restores_them() {
        let mut p = ModelPickerView::new(vec![
            ModelRow {
                display: "Claude Opus".into(),
                request_model: "claude-opus-4-8".into(),
                profile: Some("anthropic".into()),
                provider_label: "Anthropic".into(),
                is_current: true,
            },
            ModelRow {
                display: "GPT-4o".into(),
                request_model: "openai/gpt-4o".into(),
                profile: Some("openrouter".into()),
                provider_label: "OpenRouter".into(),
                is_current: false,
            },
            ModelRow {
                display: "Gemini Pro".into(),
                request_model: "google/gemini-pro".into(),
                profile: Some("openrouter".into()),
                provider_label: "OpenRouter".into(),
                is_current: false,
            },
        ]);
        assert_eq!(p.rows().len(), 3, "all rows before filtering");
        // Case-insensitive substring; matches the wire id too.
        for c in "gpt".chars() {
            assert!(matches!(
                p.handle_key(press(KeyCode::Char(c))),
                ViewOutcome::Pending
            ));
        }
        let filtered: Vec<&str> = p.rows().iter().map(|r| r.request_model.as_str()).collect();
        assert_eq!(filtered, vec!["openai/gpt-4o"], "only the GPT row matches 'gpt'");
        // Enter switches to the single filtered match.
        let outcome = p.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::SwitchModel { ref request_model, .. } if request_model == "openai/gpt-4o"
        ));
        // Backspacing the query restores the full list.
        for _ in 0..3 {
            p.handle_key(press(KeyCode::Backspace));
        }
        assert_eq!(p.rows().len(), 3, "clearing the query restores all rows");
    }
}
