//! `SessionColorBanner` — the standalone-agent `/color` banner rule.
//!
//! 1:1 port of the standalone-agent sub-case of claude-code
//! `useSwarmBanner` (`useSwarmBanner.ts:124-132` + `toThemeColor:148-155`)
//! and its `PromptInput.tsx:2250-2267` banner render. When a session color
//! is set via `/color <name>` (`AppState::session_agent_color`), a full-width
//! colored rule line is drawn directly ABOVE the prompt row, tinted by the
//! agent color.
//!
//! FORCED DIVERGENCE (data the Rust TUI has): claude-code's standalone branch
//! is `text: standaloneName ?? '', bgColor: toThemeColor(color)`. The Rust TUI
//! has no `/rename` name surface yet, so `text` is always empty here — which
//! is exactly the `'─'.repeat(columns)` text-empty branch of
//! `PromptInput.tsx:2259` (a plain colored rule, not the `─[ name ]──` inset
//! form). Still 1:1 for the data available. Color parity is "equivalent look"
//! per the M9 design (a non-goal to match RGB byte-for-byte), so the tint runs
//! through [`agent_color_from_name`], which mirrors `toThemeColor` over the 8
//! `/color` names with a cyan fallback.
//!
//! DEFERRED (1:1 gap, not a bug): claude-code brackets the prompt with the rule
//! ABOVE (`PromptInput.tsx:2260`) AND a second colored rule BELOW it (`:2267`),
//! plus a `borderBottom` box (`:2268`). The Rust `PromptInput` renders no bottom
//! border in either branch today (the `:2268` box is also unported), so this
//! draws only the top rule — internally consistent with the existing port. The
//! bottom rule (`:2267`) lands when the prompt's bottom border is ported.

use iocraft::prelude::*;

use crate::multiagent::style::agent_color_from_name;

/// The box-drawing rule glyph (U+2500 `─`), matching claude-code's `'─'.repeat`.
const RULE: char = '─';

/// Build the full-width rule line for the standalone-color banner: a run of
/// `─` (U+2500) `width` cells wide. Mirrors `PromptInput.tsx:2259`
/// (`'─'.repeat(columns)`) — the text-empty branch the Rust TUI hits. Pure
/// (returns a `String`) so the glyph + width contract is unit-lockable without
/// an iocraft runtime. The width floor is 1 (a zero-width terminal still draws
/// a single `─`). NOTE: the reference feeds the SAME `columns` to the rule and
/// the input body; a degenerate `width == 0` here would draw a 1-cell rule while
/// the prompt body falls back to 80 — unreachable in practice (terminal width is
/// never 0), so the two floors are left independent.
///
/// `color_name` is accepted for call-site symmetry with the component (the
/// banner exists only because a color is set), but the rule text itself is
/// color-independent — the tint is applied by [`SessionColorBanner`].
#[must_use]
pub fn render_session_color_banner(color_name: &str, width: usize) -> String {
    let _ = color_name; // tint is applied by the component, not the text.
    RULE.to_string().repeat(width.max(1))
}

/// Props for [`SessionColorBanner`].
#[derive(Default, Props)]
pub struct SessionColorBannerProps {
    /// Session agent-color name (claude-code `standaloneAgentContext.color`).
    /// Mapped to a render color via [`agent_color_from_name`] (cyan fallback).
    pub color_name: String,
    /// Total terminal column width — the rule spans the full width.
    pub width: usize,
}

/// Render the standalone-color banner rule: a full-width `─` line tinted by the
/// session agent color. 1:1 with the text-empty branch of
/// `PromptInput.tsx:2250-2267` (`<Text color={swarmBanner.bgColor}>{'─'.repeat(columns)}</Text>`).
#[component]
pub fn SessionColorBanner(props: &SessionColorBannerProps) -> impl Into<AnyElement<'static>> {
    let content = render_session_color_banner(&props.color_name, props.width);
    let color = agent_color_from_name(&props.color_name);
    element! {
        View(flex_direction: FlexDirection::Row) {
            Text(content: content, color: color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::style::{agent_color, AgentColor};

    #[test]
    fn banner_is_full_width_rule() {
        let line = render_session_color_banner("cyan", 10);
        assert_eq!(line, "──────────");
        // The rule is exactly `width` box-drawing chars wide.
        assert_eq!(line.chars().count(), 10);
    }

    #[test]
    fn banner_width_floor_is_one() {
        // Mirrors `width.max(1)`: a zero-width terminal still draws one `─`.
        let line = render_session_color_banner("cyan", 0);
        assert_eq!(line, "─");
        assert_eq!(line.chars().count(), 1);
    }

    #[test]
    fn banner_uses_box_drawing_dash() {
        // Lock the glyph against claude-code's `'─'.repeat`: every char is
        // U+2500 (`─`), not an ASCII hyphen or em-dash.
        let line = render_session_color_banner("cyan", 5);
        assert!(line.chars().all(|c| c == '\u{2500}'), "got: {line:?}");
    }

    #[test]
    fn banner_color_maps_each_color_name() {
        // Re-lock the name→color mapping at the banner boundary for all 8
        // `/color` names. Reuses the existing style.rs helper, so this guards
        // the wiring, not the map.
        let cases = [
            ("red", AgentColor::Red),
            ("blue", AgentColor::Blue),
            ("green", AgentColor::Green),
            ("yellow", AgentColor::Yellow),
            ("purple", AgentColor::Purple),
            ("orange", AgentColor::Orange),
            ("pink", AgentColor::Pink),
            ("cyan", AgentColor::Cyan),
        ];
        for (name, expected) in cases {
            assert_eq!(
                agent_color_from_name(name),
                agent_color(expected),
                "color name {name:?} did not map to the expected agent color"
            );
        }
    }

    #[test]
    fn banner_unknown_name_falls_back_to_cyan() {
        // Guards the `toThemeColor` fallback semantics at the sink: an unknown
        // name resolves to cyan (claude-code `cyan_FOR_SUBAGENTS_ONLY`).
        assert_eq!(
            agent_color_from_name("chartreuse"),
            agent_color(AgentColor::Cyan)
        );
    }
}
