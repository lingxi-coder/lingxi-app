//! Root iocraft component. M6-01 ships only a single `View` with a fixed
//! placeholder line. M6-02 expands this into the three-zone REPL screen
//! (`StatusLine` / `Scrollback` / `PromptInput`).
//!
//! Plan §Task 5 originally referenced iocraft 0.6's `<Box>` element; the
//! pinned `iocraft = "=0.8.3"` renames that to `<View>`. The structure
//! and props (`padding`, optional `flex_direction`) match the plan
//! exactly otherwise.

use iocraft::prelude::*;

/// Crate version string surfaced to the placeholder line.
///
/// Sourced from the lingxi-tui crate's own `CARGO_PKG_VERSION` so version
/// bumps automatically propagate. M6-09 bumps the crate to 0.7.0, at which
/// point the rendered placeholder reads `"lingxi-tui v0.7.0"`.
fn version_line() -> String {
    format!("lingxi-tui v{}", env!("CARGO_PKG_VERSION"))
}

/// Top-level TUI application state. M6-01 keeps this minimal: no fields
/// are needed to render the placeholder. M6-02 grows it into
/// `{ messages, prompt_text, streaming, ... }`.
#[derive(Debug, Default, Clone)]
pub struct TuiApp {
    /// Whether the user has requested quit. Wired by the event loop in
    /// `run_tui_session` once Ctrl-C / Ctrl-D classifies as `KeyAction::Quit`.
    pub should_quit: bool,
}

impl TuiApp {
    /// Construct a fresh, idle app state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the app as wanting to quit. Called by the event loop on
    /// `KeyAction::Quit` or when the cancel token trips.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Render the root frame. Returns an iocraft element tree owned for
    /// `'static`, which the session loop hands to iocraft's render driver.
    #[must_use]
    pub fn render(&self) -> AnyElement<'static> {
        let line = version_line();
        element! {
            View(padding: 1, flex_direction: FlexDirection::Column) {
                Text(content: line)
            }
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_idle_state() {
        let app = TuiApp::new();
        assert!(!app.should_quit);
    }

    #[test]
    fn request_quit_sets_flag() {
        let mut app = TuiApp::new();
        app.request_quit();
        assert!(app.should_quit);
    }

    #[test]
    fn version_line_includes_crate_version() {
        let v = version_line();
        assert!(v.starts_with("lingxi-tui v"));
        assert!(v.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn render_returns_an_element() {
        // We don't snapshot the full rendered ANSI here (that's
        // `tests/render_placeholder.rs`); we only assert the call
        // produces an `AnyElement<'static>` without panicking.
        let app = TuiApp::new();
        let _el = app.render();
    }
}
