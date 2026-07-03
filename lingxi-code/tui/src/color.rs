//! `/color` — set the session accent color for the composer (plan Phase 8).
//!
//! The pure arg-parser is a 1:1 port of the iocraft backend's
//! `tui/src/commands/color.rs` (itself a behavioral port of claude-code
//! `commands/color/color.ts`): every display string below is byte-locked to
//! that surface. The parser maps the argument text onto a [`ColorCommand`];
//! [`crate::chat_widget::ChatWidget::cmd_color`] applies the effect (push the
//! `system` message, set/clear the pane accent). No I/O and no widget state
//! here — fully unit-testable.
//!
//! Presentation differs from iocraft by design: iocraft drew a full-width
//! colored rule above the prompt row; this backend tints the composer box
//! (border + prompt marker) via [`accent_color`] instead — "equivalent look"
//! parity, exact pixels are a non-goal (same posture as the M9 agent-color
//! design). The color is session-only on both backends.

use tui_core::render::{NamedColor, StyleColor};

/// Agent-color names accepted by `/color`, byte-locked to claude-code
/// `AGENT_COLORS` (`tools/AgentTool/agentColorManager.ts`).
pub const AGENT_COLORS: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];

/// Reset aliases — typing any of these clears the session color back to the
/// theme default. Byte-locked to claude-code `RESET_ALIASES`.
pub const RESET_ALIASES: [&str; 5] = ["default", "reset", "none", "gray", "grey"];

/// Outcome of parsing a `/color` argument — the display message plus the
/// effect the dispatcher applies. Each variant carries its byte-locked
/// `display` string so the tests can lock it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorCommand {
    /// Empty arg: list the available colors. No state change.
    List {
        /// The `system` message body (the available-colors line).
        display: String,
    },
    /// A reset alias: clear the session accent.
    Reset {
        /// The `system` message body.
        display: String,
    },
    /// A valid color name: set the session accent (lowercased).
    Set {
        /// The resolved (lowercased) color name to set.
        name: String,
        /// The `system` message body.
        display: String,
    },
    /// An invalid name: an error `system` message. No state change.
    Invalid {
        /// The error `system` message body.
        display: String,
    },
}

/// Render the `Available colors: …, default` suffix shared by the list +
/// invalid messages — claude-code `AGENT_COLORS.join(', ')` then `, default`.
fn available_colors_clause() -> String {
    format!("Available colors: {}, default", AGENT_COLORS.join(", "))
}

/// Parse a `/color` argument string into a [`ColorCommand`]. `args` is the
/// text AFTER the command word (may be empty / whitespace). Trim + lowercase
/// mirrors claude-code `args.trim().toLowerCase()`.
#[must_use]
pub fn parse_color_command(args: &str) -> ColorCommand {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return ColorCommand::List {
            display: format!("Please provide a color. {}", available_colors_clause()),
        };
    }

    let color_arg = trimmed.to_lowercase();

    if RESET_ALIASES.contains(&color_arg.as_str()) {
        return ColorCommand::Reset {
            display: "Session color reset to default".to_string(),
        };
    }

    if !AGENT_COLORS.contains(&color_arg.as_str()) {
        return ColorCommand::Invalid {
            display: format!(
                "Invalid color \"{color_arg}\". {}",
                available_colors_clause()
            ),
        };
    }

    ColorCommand::Set {
        display: format!("Session color set to: {color_arg}"),
        name: color_arg,
    }
}

/// Map a `/color` name to a render color — the iocraft backend's
/// `agent_color_from_name` values ("equivalent look" parity: ANSI hues use
/// named colors, the rest RGB; unknown names fall back to cyan).
#[must_use]
pub fn accent_color(name: &str) -> StyleColor {
    match name.to_ascii_lowercase().as_str() {
        "red" => StyleColor::Named(NamedColor::Red),
        "blue" => StyleColor::Named(NamedColor::Blue),
        "green" => StyleColor::Named(NamedColor::Green),
        "yellow" => StyleColor::Named(NamedColor::Yellow),
        "orange" => StyleColor::Rgb(255, 165, 0),
        "purple" => StyleColor::Rgb(160, 90, 220),
        "pink" => StyleColor::Rgb(255, 130, 180),
        // "cyan" and anything unknown → cyan (claude-code fallback).
        _ => StyleColor::Named(NamedColor::Cyan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_arg_lists_available_colors() {
        let out = parse_color_command("");
        assert_eq!(
            out,
            ColorCommand::List {
                display:
                    "Please provide a color. Available colors: red, blue, green, yellow, purple, orange, pink, cyan, default"
                        .to_string(),
            }
        );
        // Whitespace-only is also "empty".
        assert!(matches!(
            parse_color_command("   "),
            ColorCommand::List { .. }
        ));
    }

    #[test]
    fn reset_aliases_all_reset_to_default() {
        for alias in RESET_ALIASES {
            assert_eq!(
                parse_color_command(alias),
                ColorCommand::Reset {
                    display: "Session color reset to default".to_string(),
                },
                "alias {alias} should reset"
            );
        }
        // Case-insensitive + surrounding whitespace.
        assert_eq!(
            parse_color_command("  DEFAULT  "),
            ColorCommand::Reset {
                display: "Session color reset to default".to_string(),
            }
        );
    }

    #[test]
    fn every_listed_color_is_valid_and_lowercases() {
        for c in AGENT_COLORS {
            assert_eq!(
                parse_color_command(c),
                ColorCommand::Set {
                    name: c.to_string(),
                    display: format!("Session color set to: {c}"),
                },
                "color {c} should be valid"
            );
        }
        assert_eq!(
            parse_color_command("  ORANGE "),
            ColorCommand::Set {
                name: "orange".to_string(),
                display: "Session color set to: orange".to_string(),
            }
        );
    }

    #[test]
    fn invalid_color_reports_error_with_list() {
        assert_eq!(
            parse_color_command("chartreuse"),
            ColorCommand::Invalid {
                display:
                    "Invalid color \"chartreuse\". Available colors: red, blue, green, yellow, purple, orange, pink, cyan, default"
                        .to_string(),
            }
        );
        // `magenta`/`teal` exist in render palettes but are NOT in the
        // `/color` command surface → invalid (1:1 with the 8-name TS list).
        assert!(matches!(
            parse_color_command("magenta"),
            ColorCommand::Invalid { .. }
        ));
    }

    #[test]
    fn accent_color_maps_names_with_cyan_fallback() {
        assert_eq!(accent_color("red"), StyleColor::Named(NamedColor::Red));
        assert_eq!(accent_color("Orange"), StyleColor::Rgb(255, 165, 0));
        assert_eq!(accent_color("pink"), StyleColor::Rgb(255, 130, 180));
        assert_eq!(accent_color("cyan"), StyleColor::Named(NamedColor::Cyan));
        assert_eq!(
            accent_color("chartreuse"),
            StyleColor::Named(NamedColor::Cyan)
        );
    }
}
