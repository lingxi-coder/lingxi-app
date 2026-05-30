//! Verifies the detection logic picks the right backend given env probes.

use lingxi_platform_posix::swarm::detection::{pick_backend, BackendChoice, TerminalEnv};

#[test]
fn inside_tmux_picks_tmux() {
    let env = TerminalEnv {
        inside_tmux: true,
        iterm_app: false,
        tmux_available: true,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::Tmux));
}

#[test]
fn iterm_with_osascript_no_tmux_picks_iterm() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: true,
        tmux_available: false,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::ITerm));
}

#[test]
fn outside_tmux_with_tmux_picks_tmux() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: false,
        tmux_available: true,
        osascript_available: false,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::Tmux));
}

#[test]
fn iterm_plus_tmux_prefers_iterm() {
    // claude-code registry.ts:173-200 — iTerm wins over external tmux when
    // it2/osascript is reachable.
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: true,
        tmux_available: true,
        osascript_available: true,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::ITerm));
}

#[test]
fn nothing_available_picks_inprocess() {
    let env = TerminalEnv {
        inside_tmux: false,
        iterm_app: false,
        tmux_available: false,
        osascript_available: false,
    };
    assert!(matches!(pick_backend(&env), BackendChoice::InProcess));
}
