//! `impl OrchestratorHandle for ConversationOrchestrator`.
//!
//! M5-02 declared the trait; M5-10 lights up the implementation against the
//! real orchestrator state. The 5 original methods (`current_session_id`,
//! `clear_session`, `force_compact`, `snapshot_cost`, `switch_model`)
//! project a minimal coarse-grained view of orchestrator state. M5-10 adds
//! two new methods: `request_exit` and `open_memory_editor`.
//!
//! Behavioural notes:
//!
//! - `clear_session` wipes [`lingxi_core::SessionState::history`] and mints
//!   a fresh `SessionId`.
//! - `force_compact` is a thin shim — for M5-10 it reports the current
//!   history length as both `messages_before` and `messages_after` (no-op)
//!   since the orchestrator's compaction subsystem is not yet plumbed into
//!   the production type. M5-11/M5-12 will wire a real
//!   `CompactionOrchestrator`.
//! - `request_exit` flips an `AtomicBool` on the orchestrator. The REPL
//!   (M5-13) reads this between turns and breaks out of the loop.
//! - `open_memory_editor` ensures `<config>/claude/CLAUDE.md` exists, then
//!   spawns the user's `$EDITOR`. Falls back to `VISUAL`, then `vi`
//!   (Unix) / `notepad.exe` (Windows). Inherits `stdin`/`stdout`/`stderr`
//!   so TUI editors render correctly.

use crate::ConversationOrchestrator;
use async_trait::async_trait;
use lingxi_traits::{
    AgentInfo, CompactionSummary, CostSnapshot, DoctorReport, HandleError, HookInfo, McpServerInfo,
    MemoryEditorOutcome, OrchestratorHandle, StatusSnapshot,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tokio::process::Command;

#[async_trait]
impl OrchestratorHandle for ConversationOrchestrator {
    async fn current_session_id(&self) -> lingxi_protocol::SessionId {
        self.session.lock().await.session_id
    }

    async fn clear_session(&self) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.history.clear();
        s.session_id = lingxi_protocol::SessionId::new();
        // Reset the JSONL parent-uuid chain (M5-07) since we minted a new
        // session id; downstream appends should not chain to the prior
        // session's last entry.
        *self.last_jsonl_uuid.lock().await = None;
        Ok(())
    }

    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        // M6-08: dispatch to the cancelable inherent method with a fresh
        // (un-cancelled) token. The REPL/TUI can call
        // `force_compact_with_cancel` directly to provide a Ctrl-C token.
        self.force_compact_with_cancel(tokio_util::sync::CancellationToken::new())
            .await
    }

    async fn snapshot_cost(&self) -> CostSnapshot {
        // M6-06: delegate to the inherent helper that reads from the wired
        // CostTracker. Returns zero-valued snapshot if no tracker is wired
        // (library callers — production CLI always wires one via init.rs).
        self.snapshot_cost_real().await
    }

    async fn switch_model(&self, model: &str) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.model = model.to_string();
        Ok(())
    }

    async fn request_exit(&self) {
        self.should_exit.store(true, Ordering::SeqCst);
    }

    async fn current_should_exit(&self) -> bool {
        self.should_exit.load(Ordering::SeqCst)
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            HandleError::ActionFailed("config_dir unavailable on this platform".into())
        })?;
        let target = config_dir.join("claude").join("CLAUDE.md");
        spawn_editor_on(target, "").await
    }

    // M5-11 additions:

    async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
        // M6-07: read the wired McpRegistry (Task 7); falls back to
        // `vec![]` when no registry was attached so unit tests / library
        // callers remain unaffected.
        let Some(reg) = self.mcp_registry.as_ref() else {
            return Vec::new();
        };
        reg.snapshot().await
    }

    async fn list_hooks(&self) -> Vec<HookInfo> {
        // M6-07: read the wired HookRegistry (Task 7).
        let Some(reg) = self.hook_registry.as_ref() else {
            return Vec::new();
        };
        let g = reg.read().await;
        let mut out: Vec<HookInfo> = g
            .all_hooks()
            .into_iter()
            .map(|h| HookInfo {
                name: h.name.clone(),
                event: h.events.first().map_or("Unknown", event_str).to_string(),
                matcher: h.if_condition.as_ref().map(|c| c.pattern.clone()),
                timeout_ms: h.timeout.map_or(60_000_u64, |d| {
                    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
                }),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    async fn list_agents(&self) -> Vec<AgentInfo> {
        // M6-07: read the wired agent catalog (Task 7).
        let Some(cat) = self.agent_catalog.as_ref() else {
            return Vec::new();
        };
        let g = cat.read().await;
        let mut out: Vec<AgentInfo> = g
            .iter()
            .map(|a| AgentInfo {
                name: a.agent_type.clone(),
                description: a.when_to_use.clone(),
                tools_allowed: a.allowed_tools.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    async fn run_doctor_checks(&self) -> DoctorReport {
        // Use `dirs::config_dir()/claude` as the canonical config dir for
        // probes (matches what `/memory` and `/config` write to).
        let config_dir =
            dirs::config_dir().map_or_else(|| std::path::PathBuf::from("."), |d| d.join("claude"));
        crate::diagnostics::run_all(&config_dir).await
    }

    async fn get_status_snapshot(&self) -> StatusSnapshot {
        let s = self.session.lock().await;
        let cost = CostSnapshot {
            session_id: s.session_id,
            ..CostSnapshot::default()
        };
        StatusSnapshot {
            session_id: s.session_id.to_string(),
            model: s.model.clone(),
            n_messages: u32::try_from(s.history.len()).unwrap_or(u32::MAX),
            total_cost_usd: cost.total_usd,
            input_tokens: cost.input_tokens,
            output_tokens: cost.output_tokens,
            n_mcp_connected: 0,
            n_mcp_total: 0,
            n_hooks: 0,
            n_agents: 0,
            started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            cwd: self.cwd.clone(),
        }
    }

    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            HandleError::ActionFailed("config_dir unavailable on this platform".into())
        })?;
        let target = config_dir.join("claude").join("config.json");
        spawn_editor_on(target, "{}\n").await
    }

    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            HandleError::ActionFailed("config_dir unavailable on this platform".into())
        })?;
        let target = config_dir.join("claude").join("permissions.json");
        spawn_editor_on(target, "{}\n").await
    }

    async fn list_available_models(&self) -> Vec<String> {
        // Hardcoded list from M3-03's model catalog. The orchestrator does
        // NOT validate names against this list — `switch_model` accepts
        // arbitrary strings. The list is purely informational for the
        // `/model` (no-arg) display.
        vec![
            "claude-opus-4-7".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-haiku-4-5".to_string(),
        ]
    }

    async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<lingxi_traits::TurnOutcome, HandleError> {
        // Delegate to the inherent method on `ConversationOrchestrator`
        // (M6-03 T1). Disambiguate via fully-qualified call syntax since
        // the trait method has the same name.
        match crate::ConversationOrchestrator::run_turn_streaming_with_cancel(self, prompt, cancel)
            .await
        {
            Ok(crate::conversation::TurnOutcome::EndTurn) => {
                Ok(lingxi_traits::TurnOutcome::EndTurn)
            }
            Ok(crate::conversation::TurnOutcome::MaxTurns) => {
                Ok(lingxi_traits::TurnOutcome::MaxTurns)
            }
            Ok(crate::conversation::TurnOutcome::Cancelled) => {
                Ok(lingxi_traits::TurnOutcome::Cancelled)
            }
            Err(e) => Err(HandleError::ActionFailed(e.to_string())),
        }
    }
}

/// Stable string label for a `HookEventType`, used by [`list_hooks`] to
/// populate [`lingxi_traits::HookInfo::event`]. Avoids `Debug` derive
/// drift — the locked names are part of the M6-07 surface and the
/// claude-code parity. (M6-07)
fn event_str(et: &lingxi_hooks::events::HookEventType) -> &'static str {
    use lingxi_hooks::events::HookEventType as E;
    match et {
        E::PreToolUse => "PreToolUse",
        E::PostToolUse => "PostToolUse",
        E::PostToolUseFailure => "PostToolUseFailure",
        E::SessionStart => "SessionStart",
        E::SessionEnd => "SessionEnd",
        E::Setup => "Setup",
        E::UserPromptSubmit => "UserPromptSubmit",
        E::Stop => "Stop",
        E::StopFailure => "StopFailure",
        E::SubagentStart => "SubagentStart",
        E::SubagentStop => "SubagentStop",
        E::PreCompact => "PreCompact",
        E::PostCompact => "PostCompact",
        E::PermissionRequest => "PermissionRequest",
        E::PermissionDenied => "PermissionDenied",
        E::TeammateIdle => "TeammateIdle",
        E::TaskCreated => "TaskCreated",
        E::TaskCompleted => "TaskCompleted",
        E::Elicitation => "Elicitation",
        E::ElicitationResult => "ElicitationResult",
        E::ConfigChange => "ConfigChange",
        E::WorktreeCreate => "WorktreeCreate",
        E::WorktreeRemove => "WorktreeRemove",
        E::InstructionsLoaded => "InstructionsLoaded",
        E::CwdChanged => "CwdChanged",
        E::FileChanged => "FileChanged",
        E::Notification => "Notification",
    }
}

/// Touch + spawn an editor on `target`. If the target does not yet exist,
/// it is created with `default_body` as its initial content.
async fn spawn_editor_on(
    target: PathBuf,
    default_body: &str,
) -> Result<MemoryEditorOutcome, HandleError> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| HandleError::ActionFailed(format!("mkdir {parent:?}: {e}")))?;
    }
    if !target.exists() {
        tokio::fs::write(&target, default_body)
            .await
            .map_err(|e| HandleError::ActionFailed(format!("touch {target:?}: {e}")))?;
    }
    let editor = resolve_editor();
    let status = Command::new(&editor)
        .arg(&target)
        .status()
        .await
        .map_err(|e| HandleError::ActionFailed(format!("spawn {editor}: {e}")))?;
    Ok(MemoryEditorOutcome {
        edited_path: target,
        exit_code: status.code().unwrap_or(-1),
    })
}

#[cfg(unix)]
fn default_editor() -> String {
    "vi".to_string()
}
#[cfg(windows)]
fn default_editor() -> String {
    "notepad.exe".to_string()
}
#[cfg(not(any(unix, windows)))]
fn default_editor() -> String {
    "vi".to_string()
}

fn resolve_editor() -> String {
    use std::env;
    if let Some(v) = env::var_os("EDITOR") {
        if !v.is_empty() {
            return v.to_string_lossy().into_owned();
        }
    }
    if let Some(v) = env::var_os("VISUAL") {
        if !v.is_empty() {
            return v.to_string_lossy().into_owned();
        }
    }
    default_editor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_returns_non_empty() {
        // Smoke test: helper must always return *some* non-empty editor.
        let s = resolve_editor();
        assert!(!s.is_empty(), "resolve_editor returned empty string");
    }

    #[cfg(unix)]
    #[test]
    fn default_editor_unix_is_vi() {
        assert_eq!(default_editor(), "vi");
    }

    #[cfg(windows)]
    #[test]
    fn default_editor_windows_is_notepad() {
        assert_eq!(default_editor(), "notepad.exe");
    }
}
