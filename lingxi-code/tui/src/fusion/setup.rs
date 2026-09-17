//! The `/fusion setup` reducer: candidate rows, wizard state, and the key
//! handling — all pure, so the flow is testable without a terminal.
//!
//! Fusion runs three different kinds of call and each is configured separately
//! (see [`platform_api::FusionModelRole`]). The wizard walks them in order and
//! only then offers to save, because a half-written configuration is worse than
//! none: the engine reports every missing role at once, and an operator who
//! saved two of three would see the same "not configured" error they started
//! with.

use crossterm::event::KeyCode;
use platform_api::fusion_setup::FusionModelRoles;
use platform_api::{FusionModelChoice, FusionModelRole};

/// One model the wizard can offer, derived from the session's live catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionCandidate {
    /// The `(profile, model)` pair written into settings.
    pub choice: FusionModelChoice,
    /// Human label (`"Claude Opus 5"`).
    pub display: String,
    /// Provider header (`"Anthropic"`).
    pub provider_label: String,
    /// Whether this route can serve as the analyst — the model claims
    /// structured output AND its profile's codec can encode a `response_format`.
    pub analyst_capable: bool,
    /// Checked-in suggestion rank. Higher sorts first WITHIN a provider group;
    /// zero means "no opinion", not "bad". Suggestion only — nothing at run
    /// time reads it.
    pub suggested_rank: u16,
    /// Whether this is the model the session itself is talking to.
    pub is_session_model: bool,
}

impl FusionCandidate {
    /// `profile/model`, the spelling settings and errors use.
    #[must_use]
    pub fn route(&self) -> String {
        self.choice.route()
    }

    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let query = query.to_lowercase();
        self.display.to_lowercase().contains(&query)
            || self.provider_label.to_lowercase().contains(&query)
            || self.choice.model.to_lowercase().contains(&query)
            || self.choice.profile.to_lowercase().contains(&query)
    }
}

/// What the wizard opens with: the live catalog plus what is already configured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionSetupSnapshot {
    /// Every model the session can currently reach.
    pub candidates: Vec<FusionCandidate>,
    /// What `settings.json` says today.
    pub roles: FusionModelRoles,
    /// `fusion.enabled` today.
    pub enabled: bool,
    /// The largest roster the current settings allow (`fusion.maxPanel`).
    pub max_panel: u8,
}

/// The persisted half of the wizard's input: what `settings.json` says today.
/// Shared with the composition root (`Arc<Mutex<_>>`) so a save updates what the
/// NEXT `/fusion setup` opens on, exactly like the `/web` snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionSettingsSnapshot {
    /// The three configured roles.
    pub roles: FusionModelRoles,
    /// `fusion.enabled`.
    pub enabled: bool,
    /// `fusion.maxPanel` (2..=8). Zero means "use the documented default".
    pub max_panel: u8,
}

/// Build the wizard's candidate list from the session's live model rows.
///
/// Candidates come from the SESSION catalog rather than from the snapshot so a
/// provider connected mid-session shows up without a relaunch — the same reason
/// `/model` filters at open time rather than at capture time.
#[must_use]
pub fn candidates_from_rows(rows: &[crate::session::ModelRow]) -> Vec<FusionCandidate> {
    rows.iter()
        .filter_map(|row| {
            let profile = row.profile.as_deref()?;
            if profile.is_empty() || row.request_model.is_empty() {
                return None;
            }
            Some(FusionCandidate {
                choice: FusionModelChoice::new(profile, &row.request_model),
                display: row.display.clone(),
                provider_label: row.provider_label.clone(),
                analyst_capable: row.fusion_analyst_capable,
                // A suggestion only. Nothing at run time reads the hint table
                // any more; it exists to put plausible picks near the top.
                suggested_rank: llm_client::hints_for(profile, &row.request_model)
                    .map_or(0, |hints| hints.quality_rank),
                is_session_model: row.is_current,
            })
        })
        .collect()
}

/// Which question the wizard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionSetupStep {
    /// Pick the panel roster.
    Panels,
    /// Pick the analyst.
    Analyst,
    /// Pick the synthesizer.
    Synthesizer,
    /// Review and save.
    Confirm,
}

impl FusionSetupStep {
    /// The role this step configures, or `None` on the review step.
    #[must_use]
    pub const fn role(self) -> Option<FusionModelRole> {
        match self {
            Self::Panels => Some(FusionModelRole::Panels),
            Self::Analyst => Some(FusionModelRole::Analyst),
            Self::Synthesizer => Some(FusionModelRole::Synthesizer),
            Self::Confirm => None,
        }
    }

    /// One-based step number, for the `Step n/4` header.
    #[must_use]
    pub const fn number(self) -> usize {
        match self {
            Self::Panels => 1,
            Self::Analyst => 2,
            Self::Synthesizer => 3,
            Self::Confirm => 4,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Panels => Self::Analyst,
            Self::Analyst => Self::Synthesizer,
            Self::Synthesizer | Self::Confirm => Self::Confirm,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Panels | Self::Analyst => Self::Panels,
            Self::Synthesizer => Self::Analyst,
            Self::Confirm => Self::Synthesizer,
        }
    }
}

/// Total wizard steps, for the `Step n/N` header.
pub const STEP_COUNT: usize = 4;

/// What the wizard asks its owner to do after one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusionSetupOutcome {
    /// Consumed; the wizard stays open.
    Stay,
    /// Save this configuration and close.
    Save {
        /// The three chosen roles.
        roles: FusionModelRoles,
        /// Whether to also set `fusion.enabled` (agents + workflows).
        enable: bool,
    },
    /// Closed without saving.
    Cancel,
}

/// The wizard's live state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionSetupState {
    /// The catalog and current settings this wizard opened over.
    pub snapshot: FusionSetupSnapshot,
    /// Current step.
    pub step: FusionSetupStep,
    /// Type-to-filter query for the current step's list.
    pub query: String,
    /// Highlight index INTO the current step's visible rows.
    pub selected: usize,
    /// Panel roster being built, in pick order.
    pub panels: Vec<FusionModelChoice>,
    /// Analyst pick.
    pub analyst: Option<FusionModelChoice>,
    /// Synthesizer pick.
    pub synthesizer: Option<FusionModelChoice>,
    /// Whether to set `fusion.enabled` on save.
    pub enable: bool,
    /// A message explaining why the last key did not advance the wizard.
    pub notice: Option<String>,
}

impl FusionSetupState {
    /// Open the wizard seeded from what is already configured, so re-running it
    /// to change one role does not make the operator retype the other two.
    #[must_use]
    pub fn from_snapshot(snapshot: FusionSetupSnapshot) -> Self {
        let panels = snapshot
            .roles
            .panels
            .iter()
            .filter(|choice| snapshot.candidates.iter().any(|c| &c.choice == *choice))
            .cloned()
            .collect();
        let keep = |choice: &Option<FusionModelChoice>| -> Option<FusionModelChoice> {
            choice
                .as_ref()
                .filter(|c| snapshot.candidates.iter().any(|row| &row.choice == *c))
                .cloned()
        };
        let analyst = keep(&snapshot.roles.analyst);
        let synthesizer = keep(&snapshot.roles.synthesizer);
        let enable = snapshot.enabled;
        let mut state = Self {
            snapshot,
            step: FusionSetupStep::Panels,
            query: String::new(),
            selected: 0,
            panels,
            analyst,
            synthesizer,
            enable,
            notice: None,
        };
        state.reset_highlight();
        state
    }

    /// The candidate indices this step shows, after role filtering and search.
    #[must_use]
    pub fn visible(&self) -> Vec<usize> {
        self.snapshot
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                // The analyst must be able to emit constrained JSON. Offering a
                // route that cannot is offering a pick whose only symptom is a
                // failed run AFTER every panel has spent, so those rows are not
                // shown at all rather than shown and then rejected.
                self.step != FusionSetupStep::Analyst || candidate.analyst_capable
            })
            .filter(|(_, candidate)| candidate.matches(&self.query))
            .map(|(index, _)| index)
            .collect()
    }

    /// The highlighted candidate, if the current step lists any.
    #[must_use]
    pub fn highlighted(&self) -> Option<&FusionCandidate> {
        let visible = self.visible();
        let index = *visible.get(self.selected)?;
        self.snapshot.candidates.get(index)
    }

    /// One-based position of `choice` in the roster, for the picker's order
    /// badge. `None` when it is not on the roster.
    #[must_use]
    pub fn panel_position(&self, choice: &FusionModelChoice) -> Option<usize> {
        self.panels
            .iter()
            .position(|picked| picked == choice)
            .map(|index| index + 1)
    }

    /// Whether `choice` is the current step's single pick.
    #[must_use]
    pub fn is_picked(&self, choice: &FusionModelChoice) -> bool {
        match self.step {
            FusionSetupStep::Panels => self.panel_position(choice).is_some(),
            FusionSetupStep::Analyst => self.analyst.as_ref() == Some(choice),
            FusionSetupStep::Synthesizer => self.synthesizer.as_ref() == Some(choice),
            FusionSetupStep::Confirm => false,
        }
    }

    /// The roles as they would be saved right now.
    #[must_use]
    pub fn roles(&self) -> FusionModelRoles {
        FusionModelRoles {
            panels: self.panels.clone(),
            analyst: self.analyst.clone(),
            synthesizer: self.synthesizer.clone(),
        }
    }

    /// Why the current step cannot be left yet, if it cannot.
    #[must_use]
    pub fn blocking_reason(&self) -> Option<String> {
        match self.step {
            FusionSetupStep::Panels => (self.panels.len() < 2).then(|| {
                "Pick at least 2 panel models — Fusion compares independent answers, \
                 so one model is not a panel."
                    .to_string()
            }),
            FusionSetupStep::Analyst => self
                .analyst
                .is_none()
                .then(|| "Pick the analyst that scores the panel reports.".to_string()),
            FusionSetupStep::Synthesizer => self
                .synthesizer
                .is_none()
                .then(|| "Pick the synthesizer that writes the final answer.".to_string()),
            FusionSetupStep::Confirm => None,
        }
    }

    fn reset_highlight(&mut self) {
        // Open each list on something useful: the first already-picked row,
        // else the session's own model, else the top of the list.
        let visible = self.visible();
        let position = visible
            .iter()
            .position(|&index| {
                self.snapshot
                    .candidates
                    .get(index)
                    .is_some_and(|candidate| self.is_picked(&candidate.choice))
            })
            .or_else(|| {
                visible.iter().position(|&index| {
                    self.snapshot
                        .candidates
                        .get(index)
                        .is_some_and(|candidate| candidate.is_session_model)
                })
            });
        self.selected = position.unwrap_or(0);
    }

    fn clamp(&mut self) {
        let len = self.visible().len();
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
    }

    fn toggle_highlighted(&mut self) {
        let Some(candidate) = self.highlighted().cloned() else {
            return;
        };
        self.notice = None;
        match self.step {
            FusionSetupStep::Panels => {
                if let Some(position) = self
                    .panels
                    .iter()
                    .position(|picked| *picked == candidate.choice)
                {
                    self.panels.remove(position);
                } else if self.panels.len() >= usize::from(self.snapshot.max_panel) {
                    self.notice = Some(format!(
                        "fusion.maxPanel is {} — remove one before adding another.",
                        self.snapshot.max_panel
                    ));
                } else {
                    self.panels.push(candidate.choice);
                }
            }
            FusionSetupStep::Analyst => self.analyst = Some(candidate.choice),
            FusionSetupStep::Synthesizer => self.synthesizer = Some(candidate.choice),
            FusionSetupStep::Confirm => {}
        }
    }

    fn advance(&mut self) -> Option<FusionSetupOutcome> {
        if let Some(reason) = self.blocking_reason() {
            self.notice = Some(reason);
            return None;
        }
        if self.step == FusionSetupStep::Confirm {
            return Some(FusionSetupOutcome::Save {
                roles: self.roles(),
                enable: self.enable,
            });
        }
        self.step = self.step.next();
        self.query.clear();
        self.notice = None;
        self.reset_highlight();
        None
    }

    fn retreat(&mut self) {
        if self.step == FusionSetupStep::Panels {
            return;
        }
        self.step = self.step.previous();
        self.query.clear();
        self.notice = None;
        self.reset_highlight();
    }
}

/// Route one key into the wizard.
///
/// `Enter` on a list step picks the highlighted row when nothing is picked yet
/// and otherwise advances — so the common path (arrow to a model, Enter) works
/// without learning that Space is the toggle.
pub fn handle_fusion_setup_key(state: &mut FusionSetupState, code: KeyCode) -> FusionSetupOutcome {
    match code {
        KeyCode::Esc => return FusionSetupOutcome::Cancel,
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Down => {
            let len = state.visible().len();
            if len > 0 {
                state.selected = (state.selected + 1).min(len - 1);
            }
        }
        KeyCode::Left | KeyCode::BackTab => state.retreat(),
        KeyCode::Char(' ') if state.step != FusionSetupStep::Confirm => {
            state.toggle_highlighted();
        }
        KeyCode::Char('e' | 'E') if state.step == FusionSetupStep::Confirm => {
            state.enable = !state.enable;
        }
        KeyCode::Enter => {
            if state.step != FusionSetupStep::Confirm && state.blocking_reason().is_some() {
                state.toggle_highlighted();
            }
            if let Some(outcome) = state.advance() {
                return outcome;
            }
        }
        KeyCode::Backspace => {
            state.query.pop();
            state.clamp();
        }
        KeyCode::Char(c) if state.step != FusionSetupStep::Confirm => {
            state.query.push(c);
            state.selected = 0;
        }
        _ => {}
    }
    FusionSetupOutcome::Stay
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn snapshot() -> FusionSetupSnapshot {
        FusionSetupSnapshot {
            candidates: vec![
                candidate("anthropic", "claude-opus-5", "Claude Opus 5", true),
                candidate("openai", "gpt-5.6-sol", "GPT-5.6 Sol", true),
                candidate("google", "gemini-3-pro", "Gemini 3 Pro", false),
            ],
            roles: FusionModelRoles::default(),
            enabled: false,
            max_panel: 8,
        }
    }

    fn state() -> FusionSetupState {
        FusionSetupState::from_snapshot(snapshot())
    }

    fn press(state: &mut FusionSetupState, keys: &[KeyCode]) -> FusionSetupOutcome {
        let mut last = FusionSetupOutcome::Stay;
        for key in keys {
            last = handle_fusion_setup_key(state, *key);
            if last != FusionSetupOutcome::Stay {
                return last;
            }
        }
        last
    }

    #[test]
    fn the_happy_path_saves_all_three_roles() {
        let mut state = state();
        let outcome = press(
            &mut state,
            &[
                // Panels: pick rows 1 and 2.
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Enter,
                // Analyst: the first judge-capable row.
                KeyCode::Enter,
                // Synthesizer.
                KeyCode::Enter,
                // Confirm.
                KeyCode::Enter,
            ],
        );
        let FusionSetupOutcome::Save { roles, enable } = outcome else {
            panic!("expected a save, got {outcome:?}");
        };
        assert_eq!(
            roles.panels,
            vec![
                FusionModelChoice::new("anthropic", "claude-opus-5"),
                FusionModelChoice::new("openai", "gpt-5.6-sol"),
            ]
        );
        assert!(roles.analyst.is_some());
        assert!(roles.synthesizer.is_some());
        assert!(roles.is_configured());
        assert!(!enable, "the switch defaults to what settings already said");
    }

    /// A one-model roster is the exact shape the engine reports as missing, so
    /// the wizard must not let it through as a "finished" configuration.
    #[test]
    fn one_panel_cannot_advance_and_says_why() {
        let mut state = state();
        assert_eq!(
            press(&mut state, &[KeyCode::Char(' '), KeyCode::Enter]),
            FusionSetupOutcome::Stay
        );
        assert_eq!(state.step, FusionSetupStep::Panels);
        assert!(state.notice.as_deref().unwrap().contains("at least 2"));
    }

    /// Offering a route that cannot encode a `response_format` would be
    /// offering a pick whose only symptom is a failed run after every panel has
    /// already spent.
    #[test]
    fn the_analyst_step_hides_routes_that_cannot_emit_constrained_json() {
        let mut state = state();
        press(
            &mut state,
            &[
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Enter,
            ],
        );
        assert_eq!(state.step, FusionSetupStep::Analyst);
        let visible: Vec<String> = state
            .visible()
            .into_iter()
            .map(|index| state.snapshot.candidates[index].route())
            .collect();
        assert_eq!(
            visible,
            vec![
                "anthropic/claude-opus-5".to_string(),
                "openai/gpt-5.6-sol".to_string()
            ],
            "the non-judge-capable route must not be offered"
        );
        // …and it stays offerable as a panel or synthesizer, where the
        // constraint does not apply.
        state.step = FusionSetupStep::Synthesizer;
        assert_eq!(state.visible().len(), 3);
    }

    #[test]
    fn a_second_space_on_a_picked_panel_removes_it_and_renumbers() {
        let mut state = state();
        press(
            &mut state,
            &[
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
            ],
        );
        assert_eq!(state.panels.len(), 3);
        assert_eq!(
            state.panel_position(&FusionModelChoice::new("google", "gemini-3-pro")),
            Some(3)
        );
        // Remove the FIRST pick; the rest keep their relative order.
        press(&mut state, &[KeyCode::Up, KeyCode::Up, KeyCode::Char(' ')]);
        assert_eq!(
            state.panels,
            vec![
                FusionModelChoice::new("openai", "gpt-5.6-sol"),
                FusionModelChoice::new("google", "gemini-3-pro"),
            ]
        );
        assert_eq!(
            state.panel_position(&FusionModelChoice::new("google", "gemini-3-pro")),
            Some(2),
            "roster order is what a preset takes a prefix of, so it must renumber"
        );
    }

    #[test]
    fn the_roster_cannot_grow_past_the_configured_panel_cap() {
        let mut snapshot = snapshot();
        snapshot.max_panel = 2;
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(
            &mut state,
            &[
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
            ],
        );
        assert_eq!(state.panels.len(), 2);
        assert!(state
            .notice
            .as_deref()
            .unwrap()
            .contains("fusion.maxPanel is 2"));
    }

    #[test]
    fn reopening_seeds_from_what_is_already_configured() {
        let mut snapshot = snapshot();
        snapshot.roles = FusionModelRoles {
            panels: vec![
                FusionModelChoice::new("openai", "gpt-5.6-sol"),
                FusionModelChoice::new("google", "gemini-3-pro"),
            ],
            analyst: Some(FusionModelChoice::new("anthropic", "claude-opus-5")),
            synthesizer: Some(FusionModelChoice::new("openai", "gpt-5.6-sol")),
        };
        snapshot.enabled = true;
        let state = FusionSetupState::from_snapshot(snapshot);
        assert_eq!(state.panels.len(), 2);
        assert_eq!(
            state.analyst,
            Some(FusionModelChoice::new("anthropic", "claude-opus-5"))
        );
        assert!(
            state.enable,
            "the switch seeds from settings, not from `false`"
        );
    }

    /// A configured model the current machine cannot reach (provider not
    /// connected, or the id was renamed upstream) must not silently come back
    /// on save — it would be written out again and fail the next run.
    #[test]
    fn a_configured_model_missing_from_the_catalog_is_dropped_on_open() {
        let mut snapshot = snapshot();
        snapshot.roles = FusionModelRoles {
            panels: vec![
                FusionModelChoice::new("openai", "gpt-5.6-sol"),
                FusionModelChoice::new("retired", "gone-2"),
            ],
            analyst: Some(FusionModelChoice::new("retired", "gone-2")),
            synthesizer: None,
        };
        let state = FusionSetupState::from_snapshot(snapshot);
        assert_eq!(
            state.panels,
            vec![FusionModelChoice::new("openai", "gpt-5.6-sol")]
        );
        assert_eq!(state.analyst, None);
    }

    #[test]
    fn typing_filters_and_backspace_restores() {
        let mut state = state();
        press(&mut state, &[KeyCode::Char('g'), KeyCode::Char('e')]);
        assert_eq!(state.visible().len(), 1);
        assert_eq!(state.highlighted().unwrap().display, "Gemini 3 Pro");
        press(&mut state, &[KeyCode::Backspace, KeyCode::Backspace]);
        assert_eq!(state.visible().len(), 3);
    }

    #[test]
    fn left_goes_back_a_step_without_losing_earlier_picks() {
        let mut state = state();
        press(
            &mut state,
            &[
                KeyCode::Char(' '),
                KeyCode::Down,
                KeyCode::Char(' '),
                KeyCode::Enter,
                KeyCode::Left,
            ],
        );
        assert_eq!(state.step, FusionSetupStep::Panels);
        assert_eq!(state.panels.len(), 2);
    }

    #[test]
    fn the_enable_switch_toggles_only_on_the_review_step() {
        let mut state = state();
        press(&mut state, &[KeyCode::Char('e')]);
        assert!(!state.enable, "`e` is a search character on a list step");
        assert_eq!(state.query, "e");
        state.query.clear();
        state.step = FusionSetupStep::Confirm;
        press(&mut state, &[KeyCode::Char('e')]);
        assert!(state.enable);
    }

    #[test]
    fn escape_cancels_from_any_step() {
        for step in [
            FusionSetupStep::Panels,
            FusionSetupStep::Analyst,
            FusionSetupStep::Synthesizer,
            FusionSetupStep::Confirm,
        ] {
            let mut state = state();
            state.step = step;
            assert_eq!(
                handle_fusion_setup_key(&mut state, KeyCode::Esc),
                FusionSetupOutcome::Cancel
            );
        }
    }

    #[test]
    fn an_empty_catalog_blocks_with_an_explanation_rather_than_saving_nothing() {
        let mut state = FusionSetupState::from_snapshot(FusionSetupSnapshot {
            candidates: Vec::new(),
            max_panel: 8,
            ..FusionSetupSnapshot::default()
        });
        assert_eq!(
            press(&mut state, &[KeyCode::Enter, KeyCode::Enter]),
            FusionSetupOutcome::Stay
        );
        assert_eq!(state.step, FusionSetupStep::Panels);
        assert!(state.notice.is_some());
    }
}
