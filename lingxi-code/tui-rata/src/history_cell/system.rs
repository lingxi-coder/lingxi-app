//! Per-variant history cells for the system-side messages: plain/level-aware
//! system text, API errors, and rate-limit notices.
//!
//! Split out of `message.rs` in the message-cells phase (plan Phase 9). The
//! styled-line renderers here are the single source for these variants: the
//! cells consume them through [`StyledCell`], and the legacy
//! [`crate::message::render_message`] dispatcher delegates to them so its
//! output stays line-identical to the pre-split renderer.

use tui_core::message::SystemLevel;
use tui_core::render::StyledLine;
use tui_core::theme::Theme;

use super::{colored_lines, StyledCell};

/// System text: dim grey, or error red when `is_error`.
pub(crate) fn system_text_lines(body: &str, is_error: bool, theme: &Theme) -> Vec<StyledLine> {
    let color = if is_error { theme.error } else { theme.dim };
    colored_lines(body, color)
}

/// Level-aware system text: info → dim, warning → warning, error → error.
pub(crate) fn system_text_rich_lines(
    body: &str,
    level: SystemLevel,
    theme: &Theme,
) -> Vec<StyledLine> {
    let color = match level {
        SystemLevel::Info => theme.dim,
        SystemLevel::Warning => theme.warning,
        SystemLevel::Error => theme.error,
    };
    colored_lines(body, color)
}

/// API error with retry counter: `API error: {error} (retry {n}/{max})`.
pub(crate) fn system_api_error_lines(
    error: &str,
    retry_attempt: u32,
    max_retries: u32,
    theme: &Theme,
) -> Vec<StyledLine> {
    colored_lines(
        &format!("API error: {error} (retry {retry_attempt}/{max_retries})"),
        theme.error,
    )
}

/// Rate-limit notice: error-colored text + optional dim upsell line.
pub(crate) fn rate_limit_lines(text: &str, upsell: Option<&str>, theme: &Theme) -> Vec<StyledLine> {
    let mut out = colored_lines(text, theme.error);
    if let Some(upsell) = upsell {
        out.extend(colored_lines(upsell, theme.dim));
    }
    out
}

/// [`RenderedMessage::SystemText`](tui_core::message::RenderedMessage::SystemText)
/// — dim (or error-red) system text.
#[derive(Debug)]
pub struct SystemTextCell {
    body: String,
    is_error: bool,
}

impl SystemTextCell {
    /// Wrap a system message body.
    #[must_use]
    pub fn new(body: String, is_error: bool) -> Self {
        Self { body, is_error }
    }

    /// The system message body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Whether this renders in the error color.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.is_error
    }
}

impl StyledCell for SystemTextCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        system_text_lines(&self.body, self.is_error, theme)
    }
}

/// [`RenderedMessage::SystemTextRich`](tui_core::message::RenderedMessage::SystemTextRich)
/// — severity-colored system text.
#[derive(Debug)]
pub struct SystemTextRichCell {
    body: String,
    level: SystemLevel,
}

impl SystemTextRichCell {
    /// Wrap a level-aware system message body.
    #[must_use]
    pub fn new(body: String, level: SystemLevel) -> Self {
        Self { body, level }
    }

    /// The severity level driving the color.
    #[must_use]
    pub fn level(&self) -> SystemLevel {
        self.level
    }
}

impl StyledCell for SystemTextRichCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        system_text_rich_lines(&self.body, self.level, theme)
    }
}

/// [`RenderedMessage::SystemApiError`](tui_core::message::RenderedMessage::SystemApiError)
/// — the error-red `API error: … (retry n/max)` line.
#[derive(Debug)]
pub struct SystemApiErrorCell {
    error: String,
    retry_attempt: u32,
    max_retries: u32,
}

impl SystemApiErrorCell {
    /// Wrap an API error + its retry counters.
    #[must_use]
    pub fn new(error: String, retry_attempt: u32, max_retries: u32) -> Self {
        Self {
            error,
            retry_attempt,
            max_retries,
        }
    }
}

impl StyledCell for SystemApiErrorCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        system_api_error_lines(&self.error, self.retry_attempt, self.max_retries, theme)
    }
}

/// [`RenderedMessage::RateLimit`](tui_core::message::RenderedMessage::RateLimit)
/// — the rate-limit notice + optional upsell.
#[derive(Debug)]
pub struct RateLimitCell {
    text: String,
    upsell: Option<String>,
}

impl RateLimitCell {
    /// Wrap a rate-limit notice.
    #[must_use]
    pub fn new(text: String, upsell: Option<String>) -> Self {
        Self { text, upsell }
    }
}

impl StyledCell for RateLimitCell {
    fn styled_lines(&self, _width: usize, theme: &Theme, _verbose: bool) -> Vec<StyledLine> {
        rate_limit_lines(&self.text, self.upsell.as_deref(), theme)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HistoryCell, RenderMode};
    use super::*;

    fn plain(cell: &dyn HistoryCell) -> Vec<String> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn first_fg(cell: &dyn HistoryCell) -> Option<ratatui::style::Color> {
        cell.display_lines(80, &Theme::dark(), RenderMode::default())[0].spans[0]
            .style
            .fg
    }

    fn rata(color: tui_core::render::StyleColor) -> ratatui::style::Color {
        crate::style_adapter::to_ratatui(color)
    }

    #[test]
    fn system_text_cell_is_dim_and_error_variant_is_red() {
        let info = SystemTextCell::new("ready".to_string(), false);
        assert_eq!(plain(&info), vec!["ready".to_string()]);
        assert_eq!(first_fg(&info), Some(rata(Theme::dark().dim)));
        assert!(!info.is_error());
        assert_eq!(info.body(), "ready");

        let error = SystemTextCell::new("disk on fire".to_string(), true);
        assert_eq!(plain(&error), vec!["disk on fire".to_string()]);
        assert_eq!(first_fg(&error), Some(rata(Theme::dark().error)));
        assert!(error.is_error());
    }

    #[test]
    fn system_text_rich_cell_maps_every_level_to_its_color() {
        let theme = Theme::dark();
        for (level, color) in [
            (SystemLevel::Info, theme.dim),
            (SystemLevel::Warning, theme.warning),
            (SystemLevel::Error, theme.error),
        ] {
            let cell = SystemTextRichCell::new("careful".to_string(), level);
            assert_eq!(plain(&cell), vec!["careful".to_string()], "{level:?}");
            assert_eq!(first_fg(&cell), Some(rata(color)), "{level:?}");
            assert_eq!(cell.level(), level);
        }
    }

    #[test]
    fn system_api_error_cell_renders_retry_counter_in_error_color() {
        let cell = SystemApiErrorCell::new("boom".to_string(), 2, 5);
        assert_eq!(
            plain(&cell),
            vec!["API error: boom (retry 2/5)".to_string()]
        );
        assert_eq!(first_fg(&cell), Some(rata(Theme::dark().error)));
    }

    #[test]
    fn rate_limit_cell_renders_error_text_plus_dim_upsell() {
        let cell = RateLimitCell::new("Rate limited".to_string(), Some("Upgrade".to_string()));
        assert_eq!(
            plain(&cell),
            vec!["Rate limited".to_string(), "Upgrade".to_string()]
        );
        let styled = cell.display_lines(80, &Theme::dark(), RenderMode::default());
        assert_eq!(styled[0].spans[0].style.fg, Some(rata(Theme::dark().error)));
        assert_eq!(styled[1].spans[0].style.fg, Some(rata(Theme::dark().dim)));

        // Without an upsell it is a single line.
        let bare = RateLimitCell::new("Rate limited".to_string(), None);
        assert_eq!(plain(&bare), vec!["Rate limited".to_string()]);
    }
}
