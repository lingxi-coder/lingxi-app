//! Detect the terminal environment to choose a `SwarmBackend`.
//!
//! Mirrors claude-code `src/utils/swarm/backends/detection.ts` + the
//! priority flow in `registry.ts:130-254`.

use std::process::Command;

/// Snapshot of the host terminal at startup.
#[allow(clippy::struct_excessive_bools)] // each flag answers an independent probe
#[derive(Debug, Clone)]
pub struct TerminalEnv {
    /// `$TMUX` env var is set (we're inside a tmux session).
    pub inside_tmux: bool,
    /// `$TERM_PROGRAM == "iTerm.app"`.
    pub iterm_app: bool,
    /// `which tmux` resolves AND `tmux -V` reports version >= 3.2.
    pub tmux_available: bool,
    /// `which osascript` resolves.
    pub osascript_available: bool,
}

/// Which concrete backend to construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Real `tmux` backend (inside or outside a user session).
    Tmux,
    /// iTerm2 via `osascript`.
    ITerm,
    /// No-pane fallback.
    InProcess,
}

/// Build a `TerminalEnv` from process state. Pure (no shelling out beyond
/// `which`); cheap enough to call at construction time.
#[must_use]
pub fn detect_terminal_env() -> TerminalEnv {
    let inside_tmux = std::env::var("TMUX").is_ok();
    let iterm_app = std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app");

    let tmux_available = which::which("tmux").is_ok() && tmux_version_ok();
    let osascript_available = which::which("osascript").is_ok();

    TerminalEnv {
        inside_tmux,
        iterm_app,
        tmux_available,
        osascript_available,
    }
}

/// Parse `tmux -V` output (e.g. `tmux 3.3a`) and return true iff major.minor
/// is at least 3.2. claude-code's `set-option -p` for per-pane border style
/// requires this minimum.
#[must_use]
pub fn tmux_version_ok() -> bool {
    let Ok(output) = Command::new("tmux").arg("-V").output() else {
        return false;
    };
    let s = String::from_utf8_lossy(&output.stdout);
    parse_tmux_version_at_least_3_2(&s)
}

/// Pure helper, exposed for unit-testing without shell-out.
#[must_use]
pub fn parse_tmux_version_at_least_3_2(version_output: &str) -> bool {
    // Expected formats: "tmux 3.2", "tmux 3.3a", "tmux 2.9", "tmux next-3.4".
    let s = version_output.trim();
    let Some(after_prefix) = s.strip_prefix("tmux ") else {
        return false;
    };
    // strip a "next-" prefix if present (some distro packages use it)
    let token = after_prefix.strip_prefix("next-").unwrap_or(after_prefix);
    // Split on the first non-digit / non-dot to drop trailing letters like "3.3a"
    let numeric: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = numeric.split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major > 3 || (major == 3 && minor >= 2)
}

/// Pick the right backend per the priority flow.
///
/// Order:
/// 1. Inside tmux + tmux available → `Tmux`.
/// 2. iTerm.app + osascript available → `ITerm`.
/// 3. Tmux available (external session) → `Tmux`.
/// 4. Otherwise → `InProcess`.
#[must_use]
pub fn pick_backend(env: &TerminalEnv) -> BackendChoice {
    if env.inside_tmux && env.tmux_available {
        return BackendChoice::Tmux;
    }
    if env.iterm_app && env.osascript_available {
        return BackendChoice::ITerm;
    }
    if env.tmux_available {
        return BackendChoice::Tmux;
    }
    BackendChoice::InProcess
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_3_2_is_ok() {
        assert!(parse_tmux_version_at_least_3_2("tmux 3.2"));
        assert!(parse_tmux_version_at_least_3_2("tmux 3.3a"));
        assert!(parse_tmux_version_at_least_3_2("tmux 4.0"));
    }

    #[test]
    fn version_below_3_2_rejected() {
        assert!(!parse_tmux_version_at_least_3_2("tmux 3.1c"));
        assert!(!parse_tmux_version_at_least_3_2("tmux 2.9"));
    }

    #[test]
    fn next_prefix_handled() {
        assert!(parse_tmux_version_at_least_3_2("tmux next-3.4"));
    }

    #[test]
    fn malformed_rejected() {
        assert!(!parse_tmux_version_at_least_3_2("garbage"));
        assert!(!parse_tmux_version_at_least_3_2(""));
    }
}
