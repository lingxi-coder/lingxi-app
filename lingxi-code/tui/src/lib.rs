//! `lingxi-tui` — iocraft-based fullscreen TUI for the `LingXi` CLI.
//!
//! M6-01 ships the crate skeleton: terminal guard, event loop merge,
//! placeholder root component, and the `run_tui_session` entry point.
//! Components (`StatusLine`, `PromptInput`, `Scrollback`, message
//! renderers, permission dialogs) land in M6-02..M6-05.
//!
//! M7-01 adds the `render` module: a full ANSI parser (16-color, 256-color,
//! truecolor; cursor/erase skipped) and a `CommonMark` markdown renderer
//! (`pulldown-cmark`), both producing the shared `render::StyledLine` model.
//! See plan `docs/superpowers/plans/2026-05-29-m7-01-ansi-markdown.md`.
//! See plan `docs/superpowers/plans/2026-05-28-m6-01-foundation.md`.

#![forbid(unsafe_code)]

pub mod app;
pub mod bash_runner;
pub mod commands;
pub mod components;
pub mod error;
pub mod events;
pub mod multiagent;
pub mod permission_bridge;
pub mod rate_limit_messages;
pub mod recent_models;
pub mod render;
pub mod replay;
pub mod root;
pub mod screens;
pub mod session;
pub mod startup_bypass;
pub mod startup_trust;
pub mod state;
pub mod streaming;
pub mod telemetry;
pub(crate) mod terminal;
pub(crate) mod theme_detect;
pub mod theme;
pub mod theme_persist;

pub use app::TuiApp;
pub use error::TuiError;
pub use events::orchestrator_bridge::{BridgeOutputStream, TurnEvent};
pub use events::{OrchestratorOutputEvent, TuiEvent};
pub use session::{run_tui_session, Runtime};

/// (T3) Whether to use iocraft's INLINE render loop instead of the fullscreen
/// alt-screen one. Opt-in via `LINGXI_TUI_INLINE` (non-empty, not `"0"`).
///
/// Fullscreen (the default) enters the alt screen + draws at absolute positions,
/// which Warp's non-standard alt-screen compositing mis-renders (ghosting).
/// Inline mode (`render_loop()` without `.fullscreen()`) draws into the terminal's
/// own scrollback (Ink / claude-code model) — no alt-screen, no absolute
/// positioning — at the cost of the app's bounded-viewport scrolling (the terminal
/// scrolls instead). EXPERIMENTAL: default OFF; cached once (env is fixed for the
/// process lifetime).
#[must_use]
pub(crate) fn inline_render_mode() -> bool {
    use std::sync::OnceLock;
    static INLINE: OnceLock<bool> = OnceLock::new();
    *INLINE.get_or_init(|| parse_inline_flag(std::env::var("LINGXI_TUI_INLINE").ok()))
}

/// Pure parse of the `LINGXI_TUI_INLINE` value: on iff present, non-empty, and
/// not `"0"`. Split out so it is unit-testable (the public getter caches via a
/// `OnceLock`, which a live env test can't reset).
#[must_use]
fn parse_inline_flag(value: Option<String>) -> bool {
    matches!(value, Some(s) if !s.is_empty() && s != "0")
}

#[cfg(test)]
mod inline_mode_tests {
    use super::parse_inline_flag;

    #[test]
    fn inline_flag_truthiness() {
        assert!(parse_inline_flag(Some("1".into())));
        assert!(parse_inline_flag(Some("true".into())));
        assert!(!parse_inline_flag(Some("0".into())));
        assert!(!parse_inline_flag(Some(String::new())));
        assert!(!parse_inline_flag(None));
    }
}

// Re-export points are filled in by later tasks; the stubs above keep the
// crate compiling task-by-task.
