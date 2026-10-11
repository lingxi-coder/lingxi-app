//! `/fusion setup` — the model-configuration wizard.
//!
//! [`setup`] is the pure reducer (no IO, no ratatui) behind
//! [`crate::bottom_pane::fusion_setup_view::FusionSetupView`]; [`persist`]
//! locates and merges `~/.lingxi/settings.json`. Split into focused modules,
//! for the same reason: the composition root performs the write off the render
//! loop and both halves stay unit-testable.

pub mod persist;
pub mod setup;
