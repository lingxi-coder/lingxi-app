//! `/model` picker (claude-code `model/` `ModelPicker`): select the main-loop
//! model. A highlight-only single-select list (the active model is marked
//! `(current)` and pre-highlighted on open).
//!
//! Unlike `theme.rs` — which live-previews and commits SYNCHRONOUSLY — switching
//! the model is an ASYNC write (`OrchestratorHandle::switch_model`) the sync key
//! path can't `.await`. So this reducer is pure highlight-only (mirroring
//! `agents.rs`): Enter yields [`ModelOutcome::Commit`] carrying the chosen model
//! id; the caller raises `AppState.pending_switch_model` and the async
//! `root::pump_switch_model` performs the write + refreshes the status line. Esc
//! cancels with no change (there is no live preview to restore).
//!
//! SCOPE: the picker switches the SESSION model via `switch_model`. claude-code's
//! `ModelPicker` also surfaces per-model descriptions / tiers + a
//! `CLAUDE_CODE_SUBAGENT_MODEL`-style env note; those are not modeled — the rows
//! are the wire model ids from `list_available_models()`.

/// Picker state: the available model ids, the active model (marked `(current)`),
/// and the highlight index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelScreenState {
    /// Available model ids (from `OrchestratorHandle::list_available_models`).
    pub models: Vec<String>,
    /// The active model on open (rendered with a `(current)` badge).
    pub current: String,
    /// Highlighted index into [`Self::models`].
    pub highlighted: usize,
}

impl ModelScreenState {
    /// Open the picker focused on the currently-active model (or row 0 when the
    /// active model is not in the list).
    #[must_use]
    pub fn new(models: Vec<String>, current: String) -> Self {
        let highlighted = models.iter().position(|m| *m == current).unwrap_or(0);
        Self {
            models,
            current,
            highlighted,
        }
    }
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutcome {
    /// Stay open (highlight moved / inert key).
    Stay,
    /// Enter — commit the carried model id (the caller raises the async switch).
    Commit(String),
    /// Esc — cancel with no change.
    Cancel,
}

/// Reduce a key (mirrors `handle_agents_key`; highlight-only, no live preview).
#[must_use]
pub fn handle_model_key(
    state: &mut ModelScreenState,
    key: crossterm::event::KeyCode,
) -> ModelOutcome {
    use crossterm::event::KeyCode;
    match key {
        KeyCode::Up | KeyCode::Char('k') => {
            state.highlighted = state.highlighted.saturating_sub(1);
            ModelOutcome::Stay
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.models.is_empty() {
                state.highlighted = (state.highlighted + 1).min(state.models.len() - 1);
            }
            ModelOutcome::Stay
        }
        KeyCode::Enter => match state.models.get(state.highlighted) {
            Some(m) => ModelOutcome::Commit(m.clone()),
            None => ModelOutcome::Stay,
        },
        KeyCode::Esc | KeyCode::Char('q') => ModelOutcome::Cancel,
        _ => ModelOutcome::Stay,
    }
}

/// Render the screen body (claude-code model picker).
#[must_use]
pub fn render_model_to_string(state: &ModelScreenState) -> String {
    let mut out = String::from("Select Model\n");
    if state.models.is_empty() {
        out.push_str("No models available.");
        return out;
    }
    for (i, m) in state.models.iter().enumerate() {
        let marker = if i == state.highlighted {
            "\u{276F} "
        } else {
            "  "
        };
        out.push_str(marker);
        out.push_str(m);
        if *m == state.current {
            out.push_str(" (current)");
        }
        out.push('\n');
    }
    out.push_str(
        "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn state() -> ModelScreenState {
        ModelScreenState::new(
            vec![
                "claude-opus-4-7".into(),
                "claude-sonnet-4-6".into(),
                "claude-haiku-4-5".into(),
            ],
            "claude-sonnet-4-6".into(),
        )
    }

    #[test]
    fn new_focuses_current_model() {
        assert_eq!(state().highlighted, 1);
        // Current not in list → row 0.
        let s = ModelScreenState::new(vec!["a".into(), "b".into()], "z".into());
        assert_eq!(s.highlighted, 0);
    }

    #[test]
    fn nav_clamps_both_ends() {
        let mut s = state();
        // Up from row 1 → 0, then clamp at 0.
        assert_eq!(handle_model_key(&mut s, KeyCode::Up), ModelOutcome::Stay);
        assert_eq!(s.highlighted, 0);
        assert_eq!(handle_model_key(&mut s, KeyCode::Up), ModelOutcome::Stay);
        assert_eq!(s.highlighted, 0);
        // Down to the last row, then clamp.
        let _ = handle_model_key(&mut s, KeyCode::Down);
        let _ = handle_model_key(&mut s, KeyCode::Down);
        assert_eq!(s.highlighted, 2);
        let _ = handle_model_key(&mut s, KeyCode::Down);
        assert_eq!(s.highlighted, 2);
    }

    #[test]
    fn enter_commits_highlighted_and_esc_cancels() {
        let mut s = state();
        // Highlight is row 1 (sonnet) on open.
        assert_eq!(
            handle_model_key(&mut s, KeyCode::Enter),
            ModelOutcome::Commit("claude-sonnet-4-6".to_string())
        );
        // Move + commit a different one.
        let _ = handle_model_key(&mut s, KeyCode::Up); // row 0
        assert_eq!(
            handle_model_key(&mut s, KeyCode::Enter),
            ModelOutcome::Commit("claude-opus-4-7".to_string())
        );
        assert_eq!(handle_model_key(&mut s, KeyCode::Esc), ModelOutcome::Cancel);
    }

    #[test]
    fn enter_on_empty_is_inert() {
        let mut s = ModelScreenState::default();
        assert_eq!(handle_model_key(&mut s, KeyCode::Enter), ModelOutcome::Stay);
        assert_eq!(handle_model_key(&mut s, KeyCode::Down), ModelOutcome::Stay);
        assert_eq!(s.highlighted, 0);
    }

    #[test]
    fn render_marks_highlight_and_current_badge() {
        let out = render_model_to_string(&state());
        assert!(out.starts_with("Select Model\n  claude-opus-4-7\n"));
        // The current (sonnet) is highlighted AND badged.
        assert!(out.contains("\u{276F} claude-sonnet-4-6 (current)\n"));
        assert!(out.contains("  claude-haiku-4-5\n"));
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn empty_shows_locked_state() {
        assert_eq!(
            render_model_to_string(&ModelScreenState::default()),
            "Select Model\nNo models available."
        );
    }
}
