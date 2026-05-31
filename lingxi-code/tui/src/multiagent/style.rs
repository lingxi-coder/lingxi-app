//! Multi-agent presentation styling primitives. (M9-02)
//!
//! Pure maps from multi-agent state onto render styling:
//! - `task_status_icon` / `task_status_color` — claude-code `taskStatusUtils`
//!   parity over the byte-locked task status wire strings.
//! - `AgentColor` + `agent_color` — per-agent color (equivalent-look parity;
//!   exact RGB is a non-goal per the M9 design).

use crate::theme::Theme;
use iocraft::Color;

/// Icon for a task status (claude-code `getTaskStatusIcon`, base mapping —
/// state-flag variants land with richer task state in M9-04+). Unknown
/// statuses fall back to the bullet, matching claude-code's `default`.
#[must_use]
pub fn task_status_icon(status: &str) -> char {
    match status {
        "completed" => '✔',         // figures.tick
        "failed" | "killed" => '✖', // figures.cross
        "running" => '▶',           // figures.play
        _ => '●',                   // figures.bullet (pending + unknown)
    }
}

/// Theme color for a task status (claude-code `getTaskStatusColor`, base
/// mapping). `background`/inactive maps onto the theme's `dim` color.
#[must_use]
pub fn task_status_color(status: &str, theme: &Theme) -> Color {
    match status {
        "completed" => theme.success,
        "failed" => theme.error,
        "killed" => theme.warning,
        _ => theme.dim, // running, pending, unknown → background/inactive
    }
}

/// TUI-local mirror of `agent::display::AgentColor` (10 colors). The feed
/// boundary (M9-06) translates the engine enum into this presentation copy,
/// keeping `tui` free of an `agent`-crate dependency (same posture as M9-01's
/// `WorkerRow` using plain strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentColor {
    /// Cyan.
    Cyan,
    /// Magenta.
    Magenta,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Red.
    Red,
    /// Orange.
    Orange,
    /// Purple.
    Purple,
    /// Pink.
    Pink,
    /// Teal.
    Teal,
}

/// Map an [`AgentColor`] to an iocraft render color. Parity is "equivalent
/// look", not byte-identical RGB (M9 design §1 non-goal): the 6 ANSI hues use
/// named colors; the other 4 use `Rgb`.
#[must_use]
pub fn agent_color(c: AgentColor) -> Color {
    match c {
        AgentColor::Cyan => Color::Cyan,
        AgentColor::Magenta => Color::Magenta,
        AgentColor::Yellow => Color::Yellow,
        AgentColor::Green => Color::Green,
        AgentColor::Blue => Color::Blue,
        AgentColor::Red => Color::Red,
        AgentColor::Orange => Color::Rgb {
            r: 255,
            g: 165,
            b: 0,
        },
        AgentColor::Purple => Color::Rgb {
            r: 160,
            g: 90,
            b: 220,
        },
        AgentColor::Pink => Color::Rgb {
            r: 255,
            g: 130,
            b: 180,
        },
        AgentColor::Teal => Color::Rgb {
            r: 0,
            g: 160,
            b: 160,
        },
    }
}

/// Map a claude-code agent **color name** (e.g. `"cyan"`, `"orange"`) to a
/// render [`Color`]. Case-insensitive. Unknown / empty names fall back to
/// cyan — claude-code's `cyan_FOR_SUBAGENTS_ONLY` default (`toInkColor`).
#[must_use]
pub fn agent_color_from_name(name: &str) -> Color {
    let c = match name.to_ascii_lowercase().as_str() {
        "magenta" => AgentColor::Magenta,
        "yellow" => AgentColor::Yellow,
        "green" => AgentColor::Green,
        "blue" => AgentColor::Blue,
        "red" => AgentColor::Red,
        "orange" => AgentColor::Orange,
        "purple" => AgentColor::Purple,
        "pink" => AgentColor::Pink,
        "teal" => AgentColor::Teal,
        // "cyan" and anything unknown → cyan.
        _ => AgentColor::Cyan,
    };
    agent_color(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_icons_match_claude_code_figures() {
        assert_eq!(task_status_icon("completed"), '✔');
        assert_eq!(task_status_icon("failed"), '✖');
        assert_eq!(task_status_icon("killed"), '✖');
        assert_eq!(task_status_icon("running"), '▶');
        assert_eq!(task_status_icon("pending"), '●');
        assert_eq!(task_status_icon("totally-unknown"), '●');
    }

    #[test]
    fn status_colors_map_to_theme_semantics() {
        let t = Theme::dark();
        assert_eq!(task_status_color("completed", &t), t.success);
        assert_eq!(task_status_color("failed", &t), t.error);
        assert_eq!(task_status_color("killed", &t), t.warning);
        assert_eq!(task_status_color("running", &t), t.dim);
        assert_eq!(task_status_color("pending", &t), t.dim);
        assert_eq!(task_status_color("unknown", &t), t.dim);
    }

    #[test]
    fn agent_color_from_name_maps_known_and_falls_back_to_cyan() {
        // Known names resolve to the same Color as the enum path.
        assert_eq!(
            agent_color_from_name("magenta"),
            agent_color(AgentColor::Magenta)
        );
        assert_eq!(
            agent_color_from_name("Orange"),
            agent_color(AgentColor::Orange)
        );
        assert_eq!(agent_color_from_name("teal"), agent_color(AgentColor::Teal));
        // Unknown / empty → cyan fallback (claude-code cyan_FOR_SUBAGENTS_ONLY).
        assert_eq!(
            agent_color_from_name("chartreuse"),
            agent_color(AgentColor::Cyan)
        );
        assert_eq!(agent_color_from_name(""), agent_color(AgentColor::Cyan));
    }

    #[test]
    fn agent_colors_cover_all_ten_and_are_distinct() {
        let all = [
            AgentColor::Cyan,
            AgentColor::Magenta,
            AgentColor::Yellow,
            AgentColor::Green,
            AgentColor::Blue,
            AgentColor::Red,
            AgentColor::Orange,
            AgentColor::Purple,
            AgentColor::Pink,
            AgentColor::Teal,
        ];
        assert_eq!(all.len(), 10);
        // No two agents share a hue (Color: PartialEq via the theme derive).
        let mapped: Vec<Color> = all.iter().map(|c| agent_color(*c)).collect();
        for i in 0..mapped.len() {
            for j in (i + 1)..mapped.len() {
                assert_ne!(mapped[i], mapped[j], "agent colors {i} and {j} collide");
            }
        }
        // Spot-check a named + an rgb mapping.
        assert_eq!(agent_color(AgentColor::Cyan), Color::Cyan);
        assert_eq!(
            agent_color(AgentColor::Orange),
            Color::Rgb {
                r: 255,
                g: 165,
                b: 0
            }
        );
    }
}
