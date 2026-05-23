//! Real `tmux`-backed `SwarmBackend` for POSIX hosts.
//!
//! Mirrors claude-code `src/utils/swarm/backends/TmuxBackend.ts`. Behavioral
//! parity items locked in M2-05 plan:
//! - `SWARM_SESSION_NAME = "claude-swarm"`
//! - `SWARM_VIEW_WINDOW_NAME = "swarm-view"`
//! - socket name `claude-swarm-<pid>` (per `getSwarmSocketName()`)
//! - `PANE_SHELL_INIT_DELAY_MS = 200`
//! - global pane-creation lock to prevent concurrent `tmux split-window` races
//! - color map literal (see `agent_color_to_tmux`)
//! - requires tmux >= 3.2 (`set-option -p` is per-pane only in 3.2+)

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::detection;

/// Shell-init delay after pane creation, per `TmuxBackend.ts:33`.
pub const PANE_SHELL_INIT_DELAY_MS: u64 = 200;

/// Process-global pane-creation lock. Mirrors claude-code's `paneCreationLock`
/// promise chain — prevents two concurrent `tmux split-window` calls from
/// racing for the same target pane.
fn pane_creation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Constants mirrored verbatim from `claude-code/src/utils/swarm/constants.ts`.
pub struct SwarmConstants;

impl SwarmConstants {
    /// External tmux session name (claude-code: `SWARM_SESSION_NAME`).
    pub const SESSION_NAME: &'static str = "claude-swarm";
    /// External tmux window name (claude-code: `SWARM_VIEW_WINDOW_NAME`).
    pub const VIEW_WINDOW_NAME: &'static str = "swarm-view";
    /// Name of the `tmux` binary on PATH.
    pub const TMUX_COMMAND: &'static str = "tmux";

    /// Per-pid socket name so multiple Claude instances don't collide.
    #[must_use]
    pub fn socket_name_for_pid(pid: u32) -> String {
        format!("claude-swarm-{pid}")
    }

    /// Socket name for the running process (uses [`std::process::id`]).
    #[must_use]
    pub fn current_socket_name() -> String {
        Self::socket_name_for_pid(std::process::id())
    }
}

/// Agent colors we support (matches `AgentColorName` in claude-code's
/// `agentColorManager.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentColor {
    /// `red`
    Red,
    /// `blue`
    Blue,
    /// `green`
    Green,
    /// `yellow`
    Yellow,
    /// `cyan`
    Cyan,
    /// `magenta` (claude-code's "purple")
    Purple,
    /// `colour208` (256-palette orange)
    Orange,
    /// `colour205` (256-palette pink)
    Pink,
}

/// Color literal mapping — see `TmuxBackend.ts:60-69`.
#[must_use]
pub fn agent_color_to_tmux(c: AgentColor) -> &'static str {
    match c {
        AgentColor::Red => "red",
        AgentColor::Blue => "blue",
        AgentColor::Green => "green",
        AgentColor::Yellow => "yellow",
        AgentColor::Cyan => "cyan",
        AgentColor::Purple => "magenta",
        AgentColor::Orange => "colour208",
        AgentColor::Pink => "colour205",
    }
}

// --- argv builders (pure, unit-testable) ---

/// Build the argv passed to `tmux split-window`. `horizontal=true` selects
/// `-h` (left/right split), `false` selects `-v` (top/bottom). When
/// `size_pct` is set we add `-l <pct>` to fix the new pane's size. The
/// `-P -F #{pane_id}` suffix makes tmux print the new pane id on stdout.
#[must_use]
pub fn build_split_window_argv(
    target_pane: &str,
    horizontal: bool,
    size_pct: Option<&str>,
) -> Vec<String> {
    let mut a = vec![
        "split-window".to_string(),
        "-t".to_string(),
        target_pane.to_string(),
        (if horizontal { "-h" } else { "-v" }).to_string(),
    ];
    if let Some(pct) = size_pct {
        a.push("-l".to_string());
        a.push(pct.to_string());
    }
    a.push("-P".to_string());
    a.push("-F".to_string());
    a.push("#{pane_id}".to_string());
    a
}

/// Build the argv for `tmux select-pane -P bg=default,fg=<color>` — sets
/// the pane's runtime foreground/background color (claude-code applies this
/// after splitting; see `TmuxBackend.ts`).
#[must_use]
pub fn build_select_pane_color_argv(pane_id: &str, tmux_color: &str) -> Vec<String> {
    vec![
        "select-pane".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "-P".to_string(),
        format!("bg=default,fg={tmux_color}"),
    ]
}

/// Build the argv for `tmux set-option -p -t <pane> pane-border-style fg=<color>`
/// — paints the pane border. Requires tmux >= 3.2 because `-p` is per-pane-only
/// from 3.2 onward.
#[must_use]
pub fn build_set_pane_border_argv(pane_id: &str, tmux_color: &str) -> Vec<String> {
    vec![
        "set-option".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "pane-border-style".to_string(),
        format!("fg={tmux_color}"),
    ]
}

/// Build the argv for `tmux set-option -p -t <pane> pane-border-format <fmt>`
/// — sets the per-pane border title shown when `pane-border-status` is on.
#[must_use]
pub fn build_set_pane_border_format_argv(pane_id: &str, fmt: &str) -> Vec<String> {
    vec![
        "set-option".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "pane-border-format".to_string(),
        fmt.to_string(),
    ]
}

/// Build the argv for `tmux send-keys -t <pane> <cmd> Enter` — types a
/// command into the target pane and presses Enter.
#[must_use]
pub fn build_send_keys_argv(pane_id: &str, cmd: &str) -> Vec<String> {
    vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        cmd.to_string(),
        "Enter".to_string(),
    ]
}

// --- real backend impl ---

/// POSIX `SwarmBackend` using `tmux` shell-out.
#[derive(Default)]
pub struct TmuxBackend {
    /// Optional per-pid socket name; defaults to `current_socket_name()`.
    socket_name: Option<String>,
}

impl TmuxBackend {
    /// Construct a new backend that derives its socket name from the
    /// running process pid.
    #[must_use]
    pub fn new() -> Self {
        Self { socket_name: None }
    }

    fn socket(&self) -> String {
        self.socket_name
            .clone()
            .unwrap_or_else(SwarmConstants::current_socket_name)
    }

    /// Internal helper: shell out to `tmux` with the requested argv.
    async fn run_tmux(&self, in_swarm_socket: bool, args: &[String]) -> Result<String, SwarmError> {
        let mut cmd = Command::new(SwarmConstants::TMUX_COMMAND);
        if in_swarm_socket {
            cmd.arg("-L").arg(self.socket());
        }
        cmd.args(args);
        let out = cmd
            .output()
            .await
            .map_err(|e| SwarmError::Tmux(format!("spawn tmux failed: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::Tmux(format!(
                "tmux exited {}: stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Synchronous "are we inside a tmux session" probe — claude-code only
    /// reads `$TMUX`, never shells out (see `detection.ts:35-37`).
    #[must_use]
    pub fn is_running_inside() -> bool {
        std::env::var("TMUX").is_ok()
    }
}

#[async_trait]
impl SwarmBackend for TmuxBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        let inside = TmuxBackend::is_running_inside();
        if inside {
            // Inside tmux: we don't create a new session — we'll split the
            // current window when teammates are added. Return the user's
            // current session name as the handle (best-effort).
            let session_name = std::env::var("TMUX")
                .ok()
                .and_then(|_| {
                    // `tmux display-message -p '#S'` returns current session.
                    std::process::Command::new(SwarmConstants::TMUX_COMMAND)
                        .args(["display-message", "-p", "#S"])
                        .output()
                        .ok()
                })
                .map_or_else(
                    || "user-session".to_string(),
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
                );
            return Ok(SwarmHandle { session_name });
        }

        // Outside tmux: create the external claude-swarm session on a per-pid socket.
        // `tmux -L <socket> new-session -d -s claude-swarm -n swarm-view`
        let args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            SwarmConstants::SESSION_NAME.to_string(),
            "-n".to_string(),
            SwarmConstants::VIEW_WINDOW_NAME.to_string(),
        ];
        self.run_tmux(true, &args).await?;
        Ok(SwarmHandle {
            session_name: SwarmConstants::SESSION_NAME.to_string(),
        })
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        // Serialize pane creation per claude-code paneCreationLock.
        let _guard = pane_creation_lock().lock().await;

        let inside = TmuxBackend::is_running_inside();
        let horizontal = matches!(position, PanePosition::Left | PanePosition::Right);

        // Target: inside-tmux uses the leader's pane id (from `$TMUX_PANE`),
        // outside-tmux uses the swarm session's first pane.
        let target_pane = if inside {
            std::env::var("TMUX_PANE")
                .map_err(|_| SwarmError::Tmux("TMUX_PANE not set inside tmux".into()))?
        } else {
            // Look up the first pane in the external swarm session.
            let listing = self
                .run_tmux(
                    true,
                    &[
                        "list-panes".to_string(),
                        "-t".to_string(),
                        format!(
                            "{}:{}",
                            SwarmConstants::SESSION_NAME,
                            SwarmConstants::VIEW_WINDOW_NAME
                        ),
                        "-F".to_string(),
                        "#{pane_id}".to_string(),
                    ],
                )
                .await?;
            listing
                .lines()
                .next()
                .ok_or_else(|| SwarmError::Tmux("no panes in swarm session".into()))?
                .to_string()
        };

        // For the first inside-tmux teammate, claude-code uses 70% width split.
        let split_argv = build_split_window_argv(&target_pane, horizontal, Some("70%"));
        let new_pane = self.run_tmux(!inside, &split_argv).await?;

        // Apply default color (red as a placeholder; production callers pass
        // the agent's color via a separate API the engine layers on top).
        let tmux_color = agent_color_to_tmux(AgentColor::Red);
        let color_argv = build_select_pane_color_argv(&new_pane, tmux_color);
        self.run_tmux(!inside, &color_argv).await?;

        let border_argv = build_set_pane_border_argv(&new_pane, tmux_color);
        self.run_tmux(!inside, &border_argv).await?;

        // Wait for shell init.
        tokio::time::sleep(Duration::from_millis(PANE_SHELL_INIT_DELAY_MS)).await;

        Ok(PaneId { raw: new_pane })
    }

    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError> {
        let inside = TmuxBackend::is_running_inside();
        if inside {
            // Inside tmux: leave the user's session intact; the engine will
            // kill individual panes via break-pane during teammate teardown.
            tracing::debug!(
                "TmuxBackend::destroy_swarm: inside-tmux mode is a no-op for session {}",
                handle.session_name
            );
            return Ok(());
        }
        // Outside tmux: kill the entire claude-swarm session on our socket.
        let args = vec![
            "kill-session".to_string(),
            "-t".to_string(),
            handle.session_name,
        ];
        self.run_tmux(true, &args).await?;
        Ok(())
    }

    fn is_available(&self) -> bool {
        which::which("tmux").is_ok() && detection::tmux_version_ok()
    }
}
