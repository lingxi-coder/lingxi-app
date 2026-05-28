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
    CompactionSummary, CostSnapshot, HandleError, MemoryEditorOutcome, OrchestratorHandle,
};
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
        // M5-10: minimal stub. The orchestrator's compaction subsystem is
        // not yet wired into the production struct; M5-11/M5-12 will
        // promote `CompactionOrchestrator` into a field and dispatch here.
        // For now report the current history length unchanged so `/compact`
        // returns a stable success result that downstream callers can
        // render.
        let s = self.session.lock().await;
        let count = u32::try_from(s.history.len()).unwrap_or(u32::MAX);
        Ok(CompactionSummary {
            messages_before: count,
            messages_after: count,
            bytes_saved: 0,
        })
    }

    async fn snapshot_cost(&self) -> CostSnapshot {
        // M5-10: minimal stub. M5-12 will promote a `CostTracker` field
        // and read from it. For now return a zeroed snapshot keyed to the
        // current session id.
        CostSnapshot {
            session_id: self.session.lock().await.session_id,
            total_nano_usd: 0,
            total_tokens: 0,
        }
    }

    async fn switch_model(&self, model: &str) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.model = model.to_string();
        Ok(())
    }

    async fn request_exit(&self) {
        self.should_exit.store(true, Ordering::SeqCst);
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        // 1. Resolve target path: <config-dir>/claude/CLAUDE.md.
        let config_dir = dirs::config_dir().ok_or_else(|| {
            HandleError::ActionFailed("config_dir unavailable on this platform".into())
        })?;
        let target = config_dir.join("claude").join("CLAUDE.md");

        // 2. Ensure parent dir + file exist (touch with empty body if missing).
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| HandleError::ActionFailed(format!("mkdir {parent:?}: {e}")))?;
        }
        if !target.exists() {
            tokio::fs::write(&target, "")
                .await
                .map_err(|e| HandleError::ActionFailed(format!("touch {target:?}: {e}")))?;
        }

        // 3. Resolve editor: EDITOR → VISUAL → platform default.
        let editor = resolve_editor();

        // 4. Spawn + wait. Inherits stdin/stdout/stderr so the user can
        //    interact with their TUI editor.
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
