//! iTerm2 `SwarmBackend` using `AppleScript` via `osascript`.
//!
//! Deliberate divergence from claude-code: claude-code uses the `it2` Python
//! CLI (`src/utils/swarm/backends/ITermBackend.ts`). We use `AppleScript`
//! instead because it's built into `macOS` and avoids the Python-API-disabled
//! trap (`it2 --version` succeeds even when iTerm2's API toggle is off,
//! causing `it2 session split` to fail with no fallback). See M2-05 plan
//! "Deliberate divergences" §1.
//!
//! Color + title setters are intentionally no-ops to match claude-code's
//! `ITermBackend.ts:270-300` performance posture.

use async_trait::async_trait;
use protocol::AgentId;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::sync::Mutex;
use traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

/// Per-process pane-creation lock, matching the tmux backend's serialization.
fn pane_creation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// iTerm2 backend using `osascript` to drive `AppleScript`.
#[derive(Default)]
pub struct ITermSwarmBackend;

impl ITermSwarmBackend {
    /// Construct a new `iTerm` `AppleScript` backend.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run an `AppleScript` snippet through `osascript -e <script>`.
    async fn run_osascript(&self, script: &str) -> Result<String, SwarmError> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| SwarmError::Tmux(format!("spawn osascript failed: {e}")))?;
        if !out.status.success() {
            return Err(SwarmError::Tmux(format!(
                "osascript exited {}: stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Pure helper, exposed for unit testing — assemble the `AppleScript` for
    /// "open a new `iTerm` window, return its session id".
    #[must_use]
    pub fn build_new_window_script(default_command: &str) -> String {
        // AppleScript single-quote-safety: callers must pre-escape any single
        // quotes in `default_command`. The engine never lets agent-provided
        // strings reach this function, so we keep escape rules simple.
        format!(
            r#"tell application "iTerm"
    set newWindow to (create window with default profile)
    tell current session of newWindow
        write text "{default_command}"
        return id
    end tell
end tell"#
        )
    }

    /// Pure helper — assemble the `AppleScript` for "split current session".
    #[must_use]
    pub fn build_split_script(vertical: bool) -> String {
        let direction = if vertical {
            "vertically"
        } else {
            "horizontally"
        };
        format!(
            r#"tell application "iTerm"
    tell current session of current window
        set newSession to (split {direction} with default profile)
        return id of newSession
    end tell
end tell"#
        )
    }
}

#[async_trait]
impl SwarmBackend for ITermSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        // Open a fresh iTerm window to host the swarm.
        let id = self
            .run_osascript(&Self::build_new_window_script(":"))
            .await?;
        Ok(SwarmHandle { session_name: id })
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        let _guard = pane_creation_lock().lock().await;
        let vertical = matches!(position, PanePosition::Top | PanePosition::Bottom);
        let id = self
            .run_osascript(&Self::build_split_script(vertical))
            .await?;
        tracing::debug!("ITermSwarmBackend: created pane {}", id);
        Ok(PaneId { raw: id })
    }

    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError> {
        // Closing the host window is the user's choice; we don't force-close
        // (matching claude-code's iTerm backend posture).
        tracing::debug!(
            "ITermSwarmBackend::destroy_swarm: leaving iTerm window {} intact",
            handle.session_name
        );
        Ok(())
    }

    fn is_available(&self) -> bool {
        std::env::var("TERM_PROGRAM").as_deref() == Ok("iTerm.app")
            && which::which("osascript").is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_script_contains_tell_iterm() {
        let s = ITermSwarmBackend::build_new_window_script(":");
        assert!(s.contains("tell application \"iTerm\""));
        assert!(s.contains("create window with default profile"));
    }

    #[test]
    fn split_script_horizontal_vs_vertical() {
        let v = ITermSwarmBackend::build_split_script(true);
        let h = ITermSwarmBackend::build_split_script(false);
        assert!(v.contains("split vertically"));
        assert!(h.contains("split horizontally"));
    }
}
