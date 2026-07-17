//! The permission-mode indicator rendered BELOW the composer — claude-code's
//! bottom-of-input mode line (`⏵⏵ accept edits on (shift+tab to cycle)`).
//!
//! Claude-code keys a `{label, symbol, color}` config off the active permission
//! mode (`tyl`/the mode-indicator array): `default` shows nothing (empty
//! symbol), the other modes show a colored `symbol label` followed by a dim
//! `(shift+tab to cycle)` hint. `default` mode renders no line at all, matching
//! claude-code (where the row is empty in manual mode).

use permission::PermissionMode;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// The 2-column indent shared with the key-hint footer (codex `FOOTER_INDENT_COLS`).
const INDENT_COLS: usize = 2;

/// The `(shift+tab to cycle)` hint appended after the mode label, dim.
const CYCLE_HINT: &str = " (shift+tab to cycle)";

/// Claude-code per-mode indicator config: `(symbol, label, accent color)`.
/// Returns `None` for modes that render no indicator (`Default`/`DontAsk`/
/// internal). Symbols: `⏵⏵` (U+23F5 ×2) for accept-edits/bypass/auto, `⏸`
/// (U+23F8) for plan. The theme color is resolved to a ratatui `Color` here so
/// callers never name the crate-private `StyleColor`.
fn indicator_config(
    mode: PermissionMode,
    theme: &tui_core::theme::Theme,
) -> Option<(&'static str, &'static str, Color)> {
    let (symbol, label, color) = match mode {
        // `{label:"accept edits on",symbol:"⏵⏵",color:"autoAccept"}` → success (green).
        PermissionMode::AcceptEdits => ("\u{23F5}\u{23F5}", "accept edits on", theme.success),
        // `{label:"plan mode on",symbol:"⏸",color:"planMode"}`.
        PermissionMode::Plan => ("\u{23F8}", "plan mode on", theme.plan_mode),
        // Bypass renders in red (`error`) — claude-code's danger accent for the
        // `⏵⏵ bypass permissions on` line.
        PermissionMode::BypassPermissions => {
            ("\u{23F5}\u{23F5}", "bypass permissions on", theme.error)
        }
        // `{label:"auto mode on",symbol:"⏵⏵",color:"warning"}`.
        PermissionMode::Auto => ("\u{23F5}\u{23F5}", "auto mode on", theme.warning),
        // `default` (and the internal `DontAsk`/`Bubble`) show no indicator.
        _ => return None,
    };
    Some((symbol, label, crate::style_adapter::to_ratatui(color)))
}

/// Rows the indicator occupies at `mode`: 1 for a mode with a config, else 0
/// (`Default` shows nothing, so the pane reclaims the row).
#[must_use]
pub(crate) fn indicator_height(mode: PermissionMode, theme: &tui_core::theme::Theme) -> u16 {
    u16::from(indicator_config(mode, theme).is_some())
}

/// The indicator line for `mode`, or `None` when the mode renders nothing.
/// Layout: `<indent><symbol> <label>` in the mode color, then `(shift+tab to
/// cycle)` in dim — matching claude-code's colored-label + dim-hint split.
#[must_use]
pub(crate) fn indicator_line(
    mode: PermissionMode,
    theme: &tui_core::theme::Theme,
) -> Option<Line<'static>> {
    let (symbol, label, accent) = indicator_config(mode, theme)?;
    let dim = crate::style_adapter::to_ratatui(theme.dim);
    Some(Line::from(vec![
        Span::raw(" ".repeat(INDENT_COLS)),
        Span::styled(format!("{symbol} {label}"), Style::default().fg(accent)),
        Span::styled(CYCLE_HINT, Style::default().fg(dim)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn default_mode_renders_nothing() {
        let theme = tui_core::theme::Theme::dark();
        assert!(indicator_line(PermissionMode::Default, &theme).is_none());
        assert_eq!(indicator_height(PermissionMode::Default, &theme), 0);
    }

    #[test]
    fn bypass_matches_claude_copy() {
        let theme = tui_core::theme::Theme::dark();
        let line = indicator_line(PermissionMode::BypassPermissions, &theme).expect("indicator");
        assert_eq!(
            text_of(&line),
            "  \u{23F5}\u{23F5} bypass permissions on (shift+tab to cycle)"
        );
        assert_eq!(indicator_height(PermissionMode::BypassPermissions, &theme), 1);
    }

    #[test]
    fn accept_edits_plan_auto_labels_and_symbols() {
        let theme = tui_core::theme::Theme::dark();
        assert_eq!(
            text_of(&indicator_line(PermissionMode::AcceptEdits, &theme).unwrap()),
            "  \u{23F5}\u{23F5} accept edits on (shift+tab to cycle)"
        );
        assert_eq!(
            text_of(&indicator_line(PermissionMode::Plan, &theme).unwrap()),
            "  \u{23F8} plan mode on (shift+tab to cycle)"
        );
        assert_eq!(
            text_of(&indicator_line(PermissionMode::Auto, &theme).unwrap()),
            "  \u{23F5}\u{23F5} auto mode on (shift+tab to cycle)"
        );
    }
}
