//! Orchestrator-side [`TodoReminderTaskProvider`] backed by the file-backed V2
//! [`tool_task::todo_store::TodoStore`].
//!
//! Finding #73: the V2 (`task_reminder`) variant of the per-turn todo reminder
//! lists the existing tasks (`B4p` reading `p9(KF())`). This adapter supplies
//! that list to [`ConversationOrchestrator::todo_reminder_message`] by resolving
//! the active task-list id and reading the store, mirroring the binary's `KF()`
//! precedence (`utils/tasks.ts`):
//!
//! 1. `CLAUDE_CODE_TASK_LIST_ID` env (explicit override);
//! 2. in-process teammate `teamName` — N/A at the orchestrator seam (no tool
//!    `ToolUseContext` here), so skipped;
//! 3. `CLAUDE_CODE_TEAM_NAME` env (`zp()`, process-based teammate);
//! 4. leader team name ([`traits::team_registry::leader_team_name`]);
//! 5. session id fallback — see RESIDUAL below.
//!
//! This is the SAME resolution the V2 `Task*` tools use ([`tool_task`]
//! `resolve_task_list_id`) for levels 1/3/4, so the reminder reads the SAME
//! on-disk task dir whenever a team / env list-id is active (the case where V2
//! task management is actually in use).
//!
//! RESIDUAL: the orchestrator owns its `SessionState` internally
//! (`SessionState::empty(SessionId::new(), …)`), and the composition root that
//! constructs this provider does not see that session id. So this adapter does
//! NOT implement the level-5 session-id fallback for a standalone session with
//! no env/team list-id; in that case it reads the default-named store (empty
//! unless `CLAUDE_CODE_TASK_LIST_ID` was set), so the reminder renders with its
//! base text only. V1 (`todo_reminder`, the default-when-tasks-disabled path)
//! needs no provider and is unaffected.

use async_trait::async_trait;

use crate::prompt::todo_reminder::{TaskReminderItem, TodoReminderTaskProvider};

/// [`TodoReminderTaskProvider`] that reads the file-backed V2 task store for the
/// active list each turn.
pub struct TodoStoreReminderTasks;

impl TodoStoreReminderTasks {
    /// Construct the store-backed provider. Stateless — it resolves the active
    /// list id at call time so a mid-session `TeamCreate` / env change is
    /// honored.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Resolve the active task-list id via the `KF()` precedence (levels 1/3/4;
    /// see module docs). Returns `None` when no list id is resolvable at the
    /// orchestrator seam (the level-5 session-id fallback is not available
    /// here), in which case the reminder lists no tasks.
    fn resolve_list_id() -> Option<String> {
        // 1. Explicit env override.
        if let Some(explicit) = std::env::var_os("CLAUDE_CODE_TASK_LIST_ID") {
            if !explicit.is_empty() {
                return Some(explicit.to_string_lossy().into_owned());
            }
        }
        // 3. `CLAUDE_CODE_TEAM_NAME` env (process-based teammate; TS `getTeamName()`).
        if let Some(team) = std::env::var_os("CLAUDE_CODE_TEAM_NAME") {
            if !team.is_empty() {
                return Some(team.to_string_lossy().into_owned());
            }
        }
        // 4. Leader team name (set by `TeamCreate`).
        if let Some(team) = traits::team_registry::leader_team_name().filter(|t| !t.is_empty()) {
            return Some(team);
        }
        None
    }
}

impl Default for TodoStoreReminderTasks {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TodoReminderTaskProvider for TodoStoreReminderTasks {
    async fn task_items(&self) -> Vec<TaskReminderItem> {
        let Some(list_id) = Self::resolve_list_id() else {
            return Vec::new();
        };
        let store = tool_task::todo_store::TodoStore::for_list(&list_id);
        store
            .list()
            .await
            .into_iter()
            .map(|t| TaskReminderItem {
                id: t.id,
                status: t.status,
                subject: t.subject,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_list_id_yields_empty() {
        // With no env/team list id, the provider lists nothing (the reminder
        // renders base-text-only). Guarded against env interference by checking
        // the unset path directly through the resolver.
        let prev = std::env::var_os("CLAUDE_CODE_TASK_LIST_ID");
        let prev_team = std::env::var_os("CLAUDE_CODE_TEAM_NAME");
        std::env::remove_var("CLAUDE_CODE_TASK_LIST_ID");
        std::env::remove_var("CLAUDE_CODE_TEAM_NAME");
        // (leader_team_name is None in a fresh test process)
        assert!(TodoStoreReminderTasks::resolve_list_id().is_none());
        let items = TodoStoreReminderTasks::new().task_items().await;
        assert!(items.is_empty());
        if let Some(v) = prev {
            std::env::set_var("CLAUDE_CODE_TASK_LIST_ID", v);
        }
        if let Some(v) = prev_team {
            std::env::set_var("CLAUDE_CODE_TEAM_NAME", v);
        }
    }
}
