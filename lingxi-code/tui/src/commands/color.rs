//! `/color` — set the prompt-bar agent color for this session.
//!
//! 1:1 behavioral port of `claude-code/src/commands/color/color.ts` (an
//! `immediate: true` `local-jsx` command — NO React screen). The command:
//!
//! - empty arg            → a `system` message LISTING the available colors.
//! - a reset alias        → persist the `"default"` sentinel + CLEAR the
//!   session color, then a `Session color reset to default` `system` message.
//! - a valid color name   → persist + SET the session color, then a
//!   `Session color set to: <name>` `system` message.
//! - an invalid name      → an error `system` message naming the bad color.
//!
//! This module is the PURE arg-parser. It maps `args` onto a [`ColorCommand`]
//! outcome carrying the exact display string + the persistence/session-state
//! effect. The dispatch intercept (`app::dispatch`) applies the effect (push
//! the `SystemText`, set `AppState.session_agent_color`, raise
//! `pending_save_color`); the disk write happens in the `root::pump_save_color`
//! async pump. No I/O, no `AppState`, no `.await` here — fully unit-testable.

/// Agent-color names accepted by `/color`, byte-locked to claude-code
/// `AGENT_COLORS` (`tools/AgentTool/agentColorManager.ts`). NOTE: this is the
/// 8-name color-command surface — distinct from (and a subset of) the 10-hue
/// `multiagent::style::AgentColor` render palette, which adds `magenta`/`teal`
/// for sub-agent badges. `/color` validates + lists exactly these 8, matching
/// the TS command.
pub const AGENT_COLORS: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];

/// Reset aliases — typing any of these clears the session color back to the
/// theme default. Byte-locked to claude-code `RESET_ALIASES`.
pub const RESET_ALIASES: [&str; 5] = ["default", "reset", "none", "gray", "grey"];

/// The `"default"` sentinel persisted on reset. claude-code writes this
/// (not an empty string) so resume-time truthiness guards re-apply the reset
/// (`color.ts` comment at the reset branch).
pub const DEFAULT_SENTINEL: &str = "default";

/// Outcome of parsing a `/color` argument — the display message plus the
/// effect the dispatcher applies. Each variant carries its byte-locked
/// `display` string so the tests can lock it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorCommand {
    /// Empty arg: list the available colors. No state change, no persistence.
    List {
        /// The `system` message body (the available-colors line).
        display: String,
    },
    /// A reset alias: clear the session color + persist the `"default"`
    /// sentinel.
    Reset {
        /// The `system` message body.
        display: String,
    },
    /// A valid color name: set + persist it (lowercased).
    Set {
        /// The resolved (lowercased) color name to set + persist.
        name: String,
        /// The `system` message body.
        display: String,
    },
    /// An invalid name: an error `system` message. No state change, no
    /// persistence.
    Invalid {
        /// The error `system` message body.
        display: String,
    },
}

/// Render the `Available colors: …, default` suffix shared by the list +
/// invalid messages — `claude-code` `AGENT_COLORS.join(', ')` then `, default`.
fn available_colors_clause() -> String {
    format!("Available colors: {}, default", AGENT_COLORS.join(", "))
}

/// Parse a `/color` argument string into a [`ColorCommand`]. `args` is the text
/// AFTER the command word (may be empty / whitespace). Trim + lowercase
/// mirrors claude-code `args.trim().toLowerCase()`.
///
/// (The claude-code teammate guard — "Teammate colors are assigned by the team
/// leader" — is a no-op here: swarm/teammate context is not present on the TUI
/// surface. TODO: re-add the `isTeammate()` short-circuit when the teammate
/// context lands.)
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
    fn valid_color_sets_lowercased_name() {
        assert_eq!(
            parse_color_command("cyan"),
            ColorCommand::Set {
                name: "cyan".to_string(),
                display: "Session color set to: cyan".to_string(),
            }
        );
        // Uppercase + whitespace normalize to the lowercased name.
        assert_eq!(
            parse_color_command("  ORANGE "),
            ColorCommand::Set {
                name: "orange".to_string(),
                display: "Session color set to: orange".to_string(),
            }
        );
    }

    #[test]
    fn every_listed_color_is_valid() {
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
        // `magenta`/`teal` exist in the render palette but are NOT in the
        // `/color` command surface → invalid (1:1 with the 8-name TS list).
        assert!(matches!(
            parse_color_command("magenta"),
            ColorCommand::Invalid { .. }
        ));
        assert!(matches!(
            parse_color_command("teal"),
            ColorCommand::Invalid { .. }
        ));
    }
}
