//! The `/fusion setup` reducer: candidate rows, wizard state, and the key
//! handling — all pure, so the flow is testable without a terminal.
//!
//! Configure the panel roster and analyst together. The parent conversation
//! synthesizes the panel and analyst results with its current model.

use crossterm::event::KeyCode;
use lingxi_core::host::fusion_setup::FusionModelRoles;
use lingxi_core::host::{FusionModelChoice, FusionModelRole, FusionStrategyRef};
use lingxi_core::types::scope::WritableScope;

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
    /// False for a saved panel whose provider/model is currently unavailable.
    /// Such rows may be removed but cannot be added to a roster.
    pub available: bool,
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

/// A persisted named strategy offered by the setup picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionSetupEntry {
    /// Stable identity and immutable settings scope.
    pub reference: FusionStrategyRef,
    /// User-assigned strategy name.
    pub name: String,
    /// Panel roster and analyst.
    pub roles: FusionModelRoles,
    /// Maximum panel count configured for this strategy.
    pub max_panel: u8,
    /// Revision required by optimistic save validation.
    pub revision: String,
    /// Whether this is its scope's default strategy.
    pub is_default: bool,
}

/// One writable settings scope captured when the wizard opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionSetupScope {
    /// User, project, or local settings.
    pub scope: WritableScope,
    /// Revision required when creating a strategy in this scope.
    pub revision: String,
    /// Scope-local Fusion enable switch. `None` inherits other settings layers.
    pub enabled: Option<bool>,
    /// A configuration diagnostic that prevents writes to this scope.
    pub error: Option<String>,
}

/// Human-readable spelling of a writable Fusion scope.
#[must_use]
pub const fn scope_label(scope: WritableScope) -> &'static str {
    match scope {
        WritableScope::User => "user",
        WritableScope::Project => "project",
        WritableScope::Local => "local",
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
    /// Named strategies, kept distinct across scopes.
    pub entries: Vec<FusionSetupEntry>,
    /// Available writable scopes and their current revisions.
    pub scopes: Vec<FusionSetupScope>,
}

/// The persisted half of the wizard's input: what `settings.json` says today.
/// Shared with the composition root (`Arc<Mutex<_>>`) so a save updates what the
/// NEXT `/fusion setup` opens on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionSettingsSnapshot {
    /// The panel and analyst roles.
    pub roles: FusionModelRoles,
    /// `fusion.enabled`.
    pub enabled: bool,
    /// `fusion.maxPanel` (2..=8). Zero means "use the documented default".
    pub max_panel: u8,
    /// Named strategies, kept distinct across scopes.
    pub entries: Vec<FusionSetupEntry>,
    /// Available writable scopes and their current revisions.
    pub scopes: Vec<FusionSetupScope>,
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
                suggested_rank: llm_runtime::hints_for(profile, &row.request_model)
                    .map_or(0, |hints| hints.quality_rank),
                is_session_model: row.is_current,
                available: true,
            })
        })
        .collect()
}

/// Which question the wizard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionSetupStep {
    /// Select an existing strategy or create one.
    Strategy,
    /// Set its name and, for new strategies, its scope.
    Details,
    /// Pick the panel roster.
    Panels,
    /// Pick the analyst.
    Analyst,
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
            Self::Strategy | Self::Details | Self::Confirm => None,
        }
    }

    /// One-based step number, for the `Step n/5` header.
    #[must_use]
    pub const fn number(self) -> usize {
        match self {
            Self::Strategy => 1,
            Self::Details => 2,
            Self::Panels => 3,
            Self::Analyst => 4,
            Self::Confirm => 5,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Strategy => Self::Details,
            Self::Details => Self::Panels,
            Self::Panels => Self::Analyst,
            Self::Analyst => Self::Confirm,
            Self::Confirm => Self::Confirm,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Strategy | Self::Details => Self::Strategy,
            Self::Panels => Self::Details,
            Self::Analyst => Self::Panels,
            Self::Confirm => Self::Analyst,
        }
    }
}

/// Total wizard steps, for the `Step n/N` header.
pub const STEP_COUNT: usize = 5;

/// What the wizard asks its owner to do after one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusionSetupOutcome {
    /// Consumed; the wizard stays open.
    Stay,
    /// Save this configuration and close.
    Save {
        /// Stable strategy identity.
        reference: FusionStrategyRef,
        /// Name to persist in the selected scope.
        name: String,
        /// Scope revision captured when setup opened.
        expected_revision: String,
        /// Whether this strategy should be the scope default.
        make_default: bool,
        /// The chosen panel and analyst roles.
        roles: FusionModelRoles,
        /// Scope-local switch to write; `None` leaves inherited settings intact.
        enable: Option<bool>,
    },
    /// Closed without saving.
    Cancel,
}

/// Unsaved edits retained when the operator returns to the strategy picker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FusionSetupDraft {
    reference: FusionStrategyRef,
    name: String,
    expected_revision: String,
    make_default: bool,
    is_new: bool,
    panels: Vec<FusionModelChoice>,
    analyst: Option<FusionModelChoice>,
    enable: bool,
    enable_override: Option<bool>,
    max_panel: u8,
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
    /// Whether to set `fusion.enabled` on save.
    pub enable: bool,
    /// Explicit scope-local switch, retained separately from the displayed value.
    pub enable_override: Option<bool>,
    /// Stable strategy being created or edited.
    pub reference: FusionStrategyRef,
    /// Editable name, unique within the selected scope.
    pub name: String,
    /// Expected settings revision for save.
    pub expected_revision: String,
    /// Whether save should make this strategy the scope default.
    pub make_default: bool,
    /// Whether the scope can still be changed before first save.
    pub is_new: bool,
    /// A message explaining why the last key did not advance the wizard.
    pub notice: Option<String>,
    drafts: Vec<FusionSetupDraft>,
}

impl FusionSetupState {
    /// Open the wizard with the current named strategies and model catalog.
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
                .filter(|c| {
                    snapshot
                        .candidates
                        .iter()
                        .any(|row| &row.choice == *c && row.analyst_capable)
                })
                .cloned()
        };
        let analyst = keep(&snapshot.roles.analyst);
        let enable = snapshot.enabled;
        let strategy_picker = !snapshot.entries.is_empty() || !snapshot.scopes.is_empty();
        let mut state = Self {
            snapshot,
            step: if strategy_picker {
                FusionSetupStep::Strategy
            } else {
                FusionSetupStep::Panels
            },
            query: String::new(),
            selected: 0,
            panels,
            analyst,
            enable,
            enable_override: None,
            reference: FusionStrategyRef {
                scope: WritableScope::User,
                id: uuid::Uuid::new_v4().to_string(),
            },
            name: "Fusion".into(),
            expected_revision: String::new(),
            make_default: false,
            is_new: true,
            notice: None,
            drafts: Vec::new(),
        };
        state.reset_highlight();
        state
    }

    /// The candidate indices this step shows, after role filtering and search.
    #[must_use]
    pub fn visible(&self) -> Vec<usize> {
        if self.step == FusionSetupStep::Strategy {
            let query = self.query.to_lowercase();
            return self
                .snapshot
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    entry.name.to_lowercase().contains(&query)
                        || scope_label(entry.reference.scope).contains(&query)
                })
                .map(|(index, _)| index)
                .chain(std::iter::once(self.snapshot.entries.len()))
                .collect();
        }
        if matches!(
            self.step,
            FusionSetupStep::Details | FusionSetupStep::Confirm
        ) {
            return Vec::new();
        }
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
        self.step.role()?;
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
            FusionSetupStep::Strategy | FusionSetupStep::Details | FusionSetupStep::Confirm => {
                false
            }
        }
    }

    /// The roles as they would be saved right now.
    #[must_use]
    pub fn roles(&self) -> FusionModelRoles {
        FusionModelRoles {
            panels: self.panels.clone(),
            analyst: self.analyst.clone(),
        }
    }

    /// Why the current step cannot be left yet, if it cannot.
    #[must_use]
    pub fn blocking_reason(&self) -> Option<String> {
        match self.step {
            FusionSetupStep::Strategy => None,
            FusionSetupStep::Details => {
                if let Some(error) = self
                    .snapshot
                    .scopes
                    .iter()
                    .find(|scope| scope.scope == self.reference.scope)
                    .and_then(|scope| scope.error.clone())
                {
                    return Some(error);
                }
                let name = self.name.trim();
                if name.is_empty() {
                    return Some("Give this strategy a name.".into());
                }
                self.snapshot
                    .entries
                    .iter()
                    .any(|entry| {
                        entry.reference.scope == self.reference.scope
                            && entry.reference.id != self.reference.id
                            && entry.name == name
                    })
                    .then(|| "A strategy with this name already exists in this scope.".into())
            }
            FusionSetupStep::Panels => (self.panels.len() < 2).then(|| {
                "Pick at least 2 panel models — Fusion compares independent answers, \
                 so one model is not a panel."
                    .to_string()
            }),
            FusionSetupStep::Analyst => self
                .analyst
                .is_none()
                .then(|| "Pick the analyst that scores the panel reports.".to_string()),
            FusionSetupStep::Confirm => None,
        }
    }

    fn reset_highlight(&mut self) {
        if self.step.role().is_none() {
            self.selected = 0;
            return;
        }
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
                } else if !candidate.available {
                    self.notice = Some("Connect this provider before adding this model.".into());
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
            FusionSetupStep::Strategy | FusionSetupStep::Details | FusionSetupStep::Confirm => {}
        }
    }

    fn select_strategy(&mut self) {
        self.snapshot
            .candidates
            .retain(|candidate| candidate.available);
        let Some(index) = self.visible().get(self.selected).copied() else {
            return;
        };
        let selected = self
            .snapshot
            .entries
            .get(index)
            .map(|entry| &entry.reference);
        if let Some(draft) = self
            .drafts
            .iter()
            .find(|draft| {
                selected.map_or(draft.is_new, |reference| {
                    !draft.is_new && &draft.reference == reference
                })
            })
            .cloned()
        {
            self.reference = draft.reference;
            self.name = draft.name;
            self.expected_revision = draft.expected_revision;
            self.make_default = draft.make_default;
            self.is_new = draft.is_new;
            self.panels = draft.panels;
            self.analyst = draft.analyst;
            self.enable = draft.enable;
            self.enable_override = draft.enable_override;
            self.snapshot.max_panel = draft.max_panel;
            self.add_saved_panel_candidates();
            return;
        }
        if let Some(entry) = self.snapshot.entries.get(index).cloned() {
            self.snapshot.max_panel = entry.max_panel.clamp(2, 8);
            self.reference = entry.reference;
            self.name = entry.name;
            self.expected_revision = entry.revision;
            self.make_default = entry.is_default;
            self.is_new = false;
            // A disconnected provider must not silently delete saved roster
            // entries when the operator only edits the strategy's name.
            self.panels = entry.roles.panels;
            self.analyst = entry.roles.analyst.filter(|choice| {
                self.snapshot
                    .candidates
                    .iter()
                    .any(|c| &c.choice == choice && c.analyst_capable)
            });
        } else {
            self.snapshot.max_panel = 8;
            self.reference = FusionStrategyRef {
                scope: self
                    .snapshot
                    .scopes
                    .first()
                    .map_or(WritableScope::User, |scope| scope.scope),
                id: uuid::Uuid::new_v4().to_string(),
            };
            self.name.clear();
            self.panels.clear();
            self.analyst = None;
            self.make_default = false;
            self.is_new = true;
        }
        self.add_saved_panel_candidates();
        self.sync_scope();
    }

    fn add_saved_panel_candidates(&mut self) {
        for choice in &self.panels {
            if !self
                .snapshot
                .candidates
                .iter()
                .any(|candidate| &candidate.choice == choice)
            {
                self.snapshot.candidates.push(FusionCandidate {
                    choice: choice.clone(),
                    display: choice.model.clone(),
                    provider_label: format!("{} · unavailable", choice.profile),
                    analyst_capable: false,
                    suggested_rank: 0,
                    is_session_model: false,
                    available: false,
                });
            }
        }
    }

    fn retain_draft(&mut self) {
        self.drafts.retain(|draft| {
            if self.is_new {
                !draft.is_new
            } else {
                draft.is_new || draft.reference != self.reference
            }
        });
        self.drafts.push(FusionSetupDraft {
            reference: self.reference.clone(),
            name: self.name.clone(),
            expected_revision: self.expected_revision.clone(),
            make_default: self.make_default,
            is_new: self.is_new,
            panels: self.panels.clone(),
            analyst: self.analyst.clone(),
            enable: self.enable,
            enable_override: self.enable_override,
            max_panel: self.snapshot.max_panel,
        });
    }

    fn sync_scope(&mut self) {
        if let Some(scope) = self
            .snapshot
            .scopes
            .iter()
            .find(|scope| scope.scope == self.reference.scope)
        {
            self.enable = scope.enabled.unwrap_or(self.snapshot.enabled);
            self.enable_override = scope.enabled;
            self.expected_revision = scope.revision.clone();
        }
    }

    fn cycle_scope(&mut self) {
        if !self.is_new {
            return;
        }
        let scopes: Vec<_> = self
            .snapshot
            .scopes
            .iter()
            .map(|scope| scope.scope)
            .collect();
        if scopes.is_empty() {
            return;
        }
        let index = scopes
            .iter()
            .position(|scope| *scope == self.reference.scope)
            .unwrap_or(0);
        self.reference.scope = scopes[(index + 1) % scopes.len()];
        self.sync_scope();
        self.notice = None;
    }

    fn advance(&mut self) -> Option<FusionSetupOutcome> {
        if self.step == FusionSetupStep::Strategy {
            self.select_strategy();
        }
        if let Some(reason) = self.blocking_reason() {
            self.notice = Some(reason);
            return None;
        }
        if self.step == FusionSetupStep::Confirm {
            return Some(FusionSetupOutcome::Save {
                reference: self.reference.clone(),
                name: self.name.trim().to_string(),
                expected_revision: self.expected_revision.clone(),
                make_default: self.make_default,
                roles: self.roles(),
                enable: self.enable_override,
            });
        }
        self.step = self.step.next();
        self.query.clear();
        self.notice = None;
        self.reset_highlight();
        None
    }

    fn retreat(&mut self) {
        if self.step == FusionSetupStep::Panels
            && self.snapshot.entries.is_empty()
            && self.snapshot.scopes.is_empty()
        {
            return;
        }
        if self.step == FusionSetupStep::Details {
            self.retain_draft();
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
    if state.step == FusionSetupStep::Details {
        match code {
            KeyCode::Esc => return FusionSetupOutcome::Cancel,
            KeyCode::Tab => state.cycle_scope(),
            KeyCode::Left | KeyCode::BackTab => state.retreat(),
            KeyCode::Backspace => {
                state.name.pop();
                state.notice = None;
            }
            KeyCode::Char(c) => {
                state.name.push(c);
                state.notice = None;
            }
            KeyCode::Enter => {
                state.advance();
            }
            _ => {}
        }
        return FusionSetupOutcome::Stay;
    }
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
        KeyCode::Char(' ') if state.step.role().is_some() => {
            state.toggle_highlighted();
        }
        KeyCode::Char('d' | 'D') if state.step == FusionSetupStep::Confirm => {
            state.make_default = !state.make_default;
        }
        KeyCode::Char('e' | 'E') if state.step == FusionSetupStep::Confirm => {
            state.enable = !state.enable;
            state.enable_override = Some(state.enable);
        }
        KeyCode::Enter => {
            if state.step == FusionSetupStep::Analyst
                || (state.step == FusionSetupStep::Panels && state.blocking_reason().is_some())
            {
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
            available: true,
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
            ..FusionSetupSnapshot::default()
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

    fn named_snapshot() -> FusionSetupSnapshot {
        let mut snapshot = snapshot();
        snapshot.scopes = [
            WritableScope::User,
            WritableScope::Project,
            WritableScope::Local,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, scope)| FusionSetupScope {
            scope,
            revision: format!("revision-{index}"),
            enabled: Some(scope == WritableScope::Project),
            error: None,
        })
        .collect();
        snapshot.entries = vec![FusionSetupEntry {
            reference: FusionStrategyRef {
                scope: WritableScope::User,
                id: "review".into(),
            },
            name: "Review".into(),
            roles: FusionModelRoles {
                panels: snapshot.candidates[..2]
                    .iter()
                    .map(|candidate| candidate.choice.clone())
                    .collect(),
                analyst: Some(snapshot.candidates[0].choice.clone()),
            },
            revision: "revision-0".into(),
            max_panel: 8,
            is_default: true,
        }];
        snapshot
    }

    #[test]
    fn editing_a_strategy_keeps_its_identity_scope_and_revision() {
        let mut state = FusionSetupState::from_snapshot(named_snapshot());
        assert_eq!(state.step, FusionSetupStep::Strategy);
        press(&mut state, &[KeyCode::Enter]);
        assert_eq!(state.step, FusionSetupStep::Details);
        assert!(!state.is_new);
        press(
            &mut state,
            &[KeyCode::Tab, KeyCode::Char('!'), KeyCode::Enter],
        );
        assert_eq!(state.reference.scope, WritableScope::User);
        assert_eq!(state.panels.len(), 2);
        let result = press(
            &mut state,
            &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter],
        );
        let FusionSetupOutcome::Save {
            reference,
            name,
            expected_revision,
            make_default,
            ..
        } = result
        else {
            panic!("expected save");
        };
        assert_eq!(reference.id, "review");
        assert_eq!(name, "Review!");
        assert_eq!(expected_revision, "revision-0");
        assert!(make_default);
    }

    #[test]
    fn new_strategy_gets_a_new_id_and_the_chosen_scopes_revision() {
        let mut state = FusionSetupState::from_snapshot(named_snapshot());
        press(&mut state, &[KeyCode::Down, KeyCode::Enter]);
        assert!(state.is_new);
        assert!(state.name.is_empty());
        assert_ne!(state.reference.id, "review");
        assert!(state.panels.is_empty());
        press(&mut state, &[KeyCode::Enter]);
        assert_eq!(state.step, FusionSetupStep::Details);
        assert!(state.notice.as_deref().unwrap().contains("name"));
        for character in "Review".chars() {
            press(&mut state, &[KeyCode::Char(character)]);
        }
        press(&mut state, &[KeyCode::Enter]);
        assert!(state.notice.as_deref().unwrap().contains("already exists"));
        press(&mut state, &[KeyCode::Tab, KeyCode::Enter]);
        assert_eq!(
            state.step,
            FusionSetupStep::Panels,
            "the same name is valid in another scope"
        );
        assert_eq!(state.reference.scope, WritableScope::Project);
        assert_eq!(state.expected_revision, "revision-1");
        assert!(state.enable);
    }

    #[test]
    fn editing_an_inherited_scope_does_not_write_the_global_enable_switch() {
        let mut snapshot = named_snapshot();
        snapshot.enabled = true;
        snapshot.scopes[0].enabled = None;
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(
            &mut state,
            &[KeyCode::Enter, KeyCode::Char('!'), KeyCode::Enter],
        );
        assert!(
            state.enable,
            "the checkbox displays the inherited effective value"
        );
        let result = press(
            &mut state,
            &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter],
        );
        let FusionSetupOutcome::Save { enable, .. } = result else {
            panic!("expected save");
        };
        assert_eq!(
            enable, None,
            "a rename must not pin or disable inherited settings"
        );
    }

    #[test]
    fn an_explicit_enable_toggle_is_saved_even_for_an_inherited_scope() {
        let mut snapshot = named_snapshot();
        snapshot.enabled = true;
        snapshot.scopes[0].enabled = None;
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(
            &mut state,
            &[
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Enter,
            ],
        );
        let result = press(&mut state, &[KeyCode::Char('e'), KeyCode::Enter]);
        let FusionSetupOutcome::Save { enable, .. } = result else {
            panic!("expected save");
        };
        assert_eq!(enable, Some(false));
    }

    #[test]
    fn editing_keeps_unavailable_saved_panels_and_allows_explicit_removal() {
        let mut snapshot = named_snapshot();
        let unavailable = FusionModelChoice::new("offline", "saved-model");
        snapshot.entries[0]
            .roles
            .panels
            .insert(1, unavailable.clone());
        let saved_panels = snapshot.entries[0].roles.panels.clone();
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(
            &mut state,
            &[KeyCode::Enter, KeyCode::Char('!'), KeyCode::Enter],
        );
        assert_eq!(state.panels, saved_panels);
        let result = press(
            &mut state,
            &[KeyCode::Enter, KeyCode::Enter, KeyCode::Enter],
        );
        let FusionSetupOutcome::Save { roles, .. } = result else {
            panic!("expected save");
        };
        assert_eq!(
            roles.panels, saved_panels,
            "rename must preserve roster and order"
        );

        state.step = FusionSetupStep::Panels;
        state.selected = state
            .visible()
            .iter()
            .position(|&index| state.snapshot.candidates[index].choice == unavailable)
            .unwrap();
        press(&mut state, &[KeyCode::Char(' ')]);
        assert!(!state.panels.contains(&unavailable));
        press(&mut state, &[KeyCode::Char(' ')]);
        assert!(
            !state.panels.contains(&unavailable),
            "disconnected models are removal-only"
        );
        assert!(state.notice.as_deref().unwrap().contains("Connect"));

        // Leaving an edit must not offer this saved-only route on a new strategy.
        state.step = FusionSetupStep::Strategy;
        state.selected = 1;
        press(&mut state, &[KeyCode::Enter]);
        assert!(state
            .snapshot
            .candidates
            .iter()
            .all(|candidate| candidate.available));
    }

    #[test]
    fn selected_strategy_supplies_its_panel_cap_and_new_strategy_uses_engine_maximum() {
        let mut snapshot = named_snapshot();
        snapshot.entries[0].max_panel = 2;
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(&mut state, &[KeyCode::Enter]);
        assert_eq!(state.snapshot.max_panel, 2);
        press(&mut state, &[KeyCode::Left, KeyCode::Down, KeyCode::Enter]);
        assert!(state.is_new);
        assert_eq!(state.snapshot.max_panel, 8);
    }

    #[test]
    fn switching_strategies_retains_unsaved_roles_name_and_new_strategy_scope() {
        let mut state = FusionSetupState::from_snapshot(named_snapshot());
        press(&mut state, &[KeyCode::Enter, KeyCode::Char('!')]);
        state.panels.reverse();
        state.analyst = Some(state.snapshot.candidates[1].choice.clone());
        state.enable = true;
        let edited_roles = state.roles();
        press(&mut state, &[KeyCode::Left, KeyCode::Down, KeyCode::Enter]);
        press(&mut state, &[KeyCode::Char('N'), KeyCode::Tab]);
        let new_reference = state.reference.clone();
        press(&mut state, &[KeyCode::Left, KeyCode::Enter]);
        assert_eq!(state.name, "Review!");
        assert_eq!(state.roles(), edited_roles);
        assert!(state.enable);
        press(&mut state, &[KeyCode::Left, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(state.name, "N");
        assert_eq!(state.reference, new_reference);
        assert_eq!(state.reference.scope, WritableScope::Project);
    }

    #[test]
    fn analyst_enter_selects_the_highlighted_model_even_with_an_existing_pick() {
        let mut state = state();
        state.analyst = Some(state.snapshot.candidates[0].choice.clone());
        state.step = FusionSetupStep::Analyst;
        press(&mut state, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(state.step, FusionSetupStep::Confirm);
        assert_eq!(
            state.analyst,
            Some(state.snapshot.candidates[1].choice.clone())
        );
    }

    #[test]
    fn opening_drops_an_analyst_that_no_longer_supports_structured_output() {
        let mut snapshot = snapshot();
        snapshot.roles.analyst = Some(snapshot.candidates[2].choice.clone());
        let state = FusionSetupState::from_snapshot(snapshot);
        assert!(state.analyst.is_none());
    }

    #[test]
    fn invalid_scope_configuration_blocks_without_rewriting_it() {
        let mut snapshot = named_snapshot();
        snapshot.scopes[0].error =
            Some("Legacy Fusion configuration must be replaced explicitly.".into());
        let mut state = FusionSetupState::from_snapshot(snapshot);
        press(&mut state, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(state.step, FusionSetupStep::Details);
        assert!(state.notice.as_deref().unwrap().contains("Legacy"));
    }

    #[test]
    fn default_toggle_is_only_active_on_review() {
        let mut state = FusionSetupState::from_snapshot(named_snapshot());
        press(&mut state, &[KeyCode::Char('d')]);
        assert_eq!(state.query, "d");
        assert!(!state.make_default);
        state.step = FusionSetupStep::Confirm;
        press(&mut state, &[KeyCode::Char('d')]);
        assert!(state.make_default);
        press(&mut state, &[KeyCode::Char('d')]);
        assert!(!state.make_default);
    }

    #[test]
    fn the_happy_path_saves_panels_and_analyst() {
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
                // Confirm.
                KeyCode::Enter,
            ],
        );
        let FusionSetupOutcome::Save { roles, enable, .. } = outcome else {
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
        assert!(roles.is_configured());
        assert!(
            enable.is_none(),
            "untouched enable settings must be inherited"
        );
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
        // …and it stays offerable as a panel, where the
        // constraint does not apply.
        state.step = FusionSetupStep::Panels;
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
