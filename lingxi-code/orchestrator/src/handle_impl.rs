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
//! - `clear_session` wipes [`engine::SessionState::history`] and mints
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
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tokio::process::Command;
use traits::{
    AgentInfo, CompactionSummary, CostSnapshot, DoctorReport, HandleError, HookInfo, McpServerInfo,
    MemoryEditorOutcome, OrchestratorHandle, StatusSnapshot,
};

#[async_trait]
impl OrchestratorHandle for ConversationOrchestrator {
    async fn current_session_id(&self) -> protocol::SessionId {
        self.session.lock().await.session_id
    }

    async fn clear_session(&self) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.history.clear();
        s.session_id = protocol::SessionId::new();
        // Reset the JSONL parent-uuid chain (M5-07) since we minted a new
        // session id; downstream appends should not chain to the prior
        // session's last entry.
        *self.last_jsonl_uuid.lock().await = None;
        Ok(())
    }

    /// Adopt a replayed session IN PLACE — the symmetric twin of
    /// [`Self::clear_session`]. Where `clear_session` wipes the history and
    /// MINTS a fresh `SessionId`, `resume_session` ADOPTS the named on-disk
    /// session: it swaps in the replayed `history`, adopts the named
    /// `session_id` (so the running orchestrator IS the resumed session), and
    /// seeds the JSONL parent-uuid chain to `last_jsonl_uuid` so any future
    /// append chains via `parent_uuid` off the resumed tail (the same field
    /// `with_resume` overrides at construction time, here applied to a live
    /// orchestrator).
    ///
    /// `s.model` is intentionally LEFT UNCHANGED — resume keeps the live model
    /// the connection is running. (`build_state_from_jsonl` uses
    /// `DEFAULT_MODEL` only for the throwaway `SessionState` the host loads +
    /// discards; the live model is the source of truth.)
    async fn resume_session(
        &self,
        session_id: protocol::SessionId,
        history: Vec<protocol::ConversationMessage>,
        last_jsonl_uuid: Option<String>,
    ) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.history = history;
        // Adopt the NAMED id (clear_session mints a fresh one; resume does NOT).
        s.session_id = session_id;
        drop(s);
        // Seed the parent-uuid chain so any future append chains off the
        // resumed tail (matching the M5-07 writer's chain semantics).
        *self.last_jsonl_uuid.lock().await = last_jsonl_uuid;
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

    async fn switch_model(&self, model: &str, profile: Option<&str>) -> Result<(), HandleError> {
        let mut s = self.session.lock().await;
        s.model = model.to_string();
        s.model_profile = profile.map(str::to_string);
        Ok(())
    }

    async fn request_exit(&self) {
        self.should_exit.store(true, Ordering::SeqCst);
    }

    async fn current_should_exit(&self) -> bool {
        self.should_exit.load(Ordering::SeqCst)
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        // `/memory` edits the USER-tier CLAUDE.md — the SAME file the system-prompt
        // hierarchy loads (memory::claude_md::user_config_dir): `$CLAUDE_CONFIG_DIR`
        // when set, else `~/.claude/CLAUDE.md`. (Previously this targeted
        // `dirs::config_dir()/claude/CLAUDE.md` — a different, never-loaded path that
        // also ignored `$CLAUDE_CONFIG_DIR`.)
        let home = dirs::home_dir().ok_or_else(|| {
            HandleError::ActionFailed("home dir unavailable on this platform".into())
        })?;
        let target = memory::claude_md::user_config_dir(&home).join("CLAUDE.md");
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
            // This handle is not coordinator-wired; the count is 0 (T21).
            active_workers: 0,
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

    /// Model names shown by the no-arg `/model` display. Surfaces the real
    /// configured profiles + aliases via the API-client seam
    /// (`ProviderApiAdapter` → `ModelRouter::available_models`, emitting
    /// `provider/model` ids and `@aliases`). Falls back to the static example
    /// list when no routing client is wired (library / test callers, or the
    /// no-streaming stub). `switch_model` still accepts any string; actual
    /// availability depends on the profile's API key (see `docs/LLM_PROVIDERS.md`).
    async fn list_available_models(&self) -> Vec<String> {
        let models = self.api.available_models();
        if !models.is_empty() {
            return models;
        }
        vec![
            "claude-opus-4-7".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-haiku-4-5".to_string(),
            "openai/gpt-4o".to_string(),
            "openai/gpt-4o-mini".to_string(),
            "gemini/gemini-2.0-flash".to_string(),
        ]
    }

    /// Richer catalog listing for the grouped `/model` picker. Delegates to the
    /// api client's [`OrchestratorApiClient::list_model_listings`], which the
    /// production `ProviderApiAdapter` sources from the llm-client catalog.
    async fn list_model_listings(&self) -> Vec<traits::orchestrator::ModelListing> {
        self.api.list_model_listings()
    }

    /// Return the most recently observed provider rate-limit header snapshot.
    ///
    /// Delegates to [`OrchestratorApiClient::last_rate_limit_info`] on the
    /// `api` field.  `ProviderApiAdapter` overrides the default (None) to
    /// return the cached 2xx header snapshot from `last_rate_limit`.
    ///
    /// TUI note: this surface is available for polling (e.g. from a ticker).
    /// Wiring it into `RenderedMessage::RateLimit` requires a new protocol
    /// event or a dedicated status-poll channel — both outside this task's
    /// scope (frozen protocol guard).  See the trait doc for details.
    async fn last_rate_limit_info(&self) -> Option<traits::RateLimitSnapshot> {
        self.api.last_rate_limit_info()
    }

    async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::TurnOutcome, HandleError> {
        // Delegate to the inherent method on `ConversationOrchestrator`
        // (M6-03 T1). Disambiguate via fully-qualified call syntax since
        // the trait method has the same name.
        match crate::ConversationOrchestrator::run_turn_streaming_with_cancel(self, prompt, cancel)
            .await
        {
            Ok(crate::conversation::TurnOutcome::EndTurn) => Ok(traits::TurnOutcome::EndTurn),
            Ok(crate::conversation::TurnOutcome::MaxTurns) => Ok(traits::TurnOutcome::MaxTurns),
            Ok(crate::conversation::TurnOutcome::Cancelled) => Ok(traits::TurnOutcome::Cancelled),
            Err(e) => Err(HandleError::ActionFailed(e.to_string())),
        }
    }

    async fn run_turn_streaming_with_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::TurnOutcome, HandleError> {
        // Delegate to the inherent image-aware streaming entry point.
        match crate::ConversationOrchestrator::run_turn_streaming_with_cancel_images(
            self,
            prompt,
            image_paths,
            cancel,
        )
        .await
        {
            Ok(crate::conversation::TurnOutcome::EndTurn) => Ok(traits::TurnOutcome::EndTurn),
            Ok(crate::conversation::TurnOutcome::MaxTurns) => Ok(traits::TurnOutcome::MaxTurns),
            Ok(crate::conversation::TurnOutcome::Cancelled) => Ok(traits::TurnOutcome::Cancelled),
            Err(e) => Err(HandleError::ActionFailed(e.to_string())),
        }
    }

    // engine-data-commands additions:

    async fn conversation_transcript(&self) -> Vec<protocol::ConversationMessage> {
        // Clone of the live, ordered session history. Backs `/export`
        // (transcript → file) and underpins `/summary` + `/diff`.
        self.session.lock().await.history.clone()
    }

    async fn files_in_context(&self) -> Vec<PathBuf> {
        // Read the orchestrator-owned read-file-state cache (TS
        // `context.readFileState`), populated by the dispatch loop on each
        // successful Read/Edit/Write/MultiEdit/NotebookEdit
        // (`turn_loop::record_read_file_state`). Keys are absolutized,
        // lexically-normalized paths in insertion order — 1:1 with TS
        // `cacheKeys(context.readFileState)` (`Array.from(cache.keys())`),
        // which `/files` renders via `relative(getCwd(), f)`. An empty cache
        // still renders the locked "No files in context" branch.
        self.read_file_state.lock().await.clone()
    }

    async fn context_window_usage(&self) -> (u64, u64) {
        // `used_tokens` is the session's cumulative input+output token count
        // (engine::SessionState::usage). `max_tokens` is the active model's
        // context budget; LingXi locks the 200k Claude window (matching
        // cost::budget). Falls back to (0, 0) when nothing has been counted.
        let s = self.session.lock().await;
        let usage = &s.usage.0;
        let used = usage.input_tokens.saturating_add(usage.output_tokens);
        (used, CONTEXT_WINDOW_MAX_TOKENS)
    }

    async fn list_resumable_sessions(&self) -> Vec<(String, String)> {
        // Enumerate the on-disk JSONL session store the resume path reads
        // from: `<claude_home>/projects/<project_dir_name(cwd)>/<uuid>.jsonl`.
        // `claude_home` follows the `~/.claude` convention the CLI wires.
        // Each entry maps to `(session_id, label)`; label is the id (the
        // first-prompt label + interactive picker are deferred). Newest-first
        // by mtime. Returns an empty Vec when the store is absent.
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let cwd = self.cwd.to_string_lossy();
        let project_dir = home
            .join(".claude")
            .join("projects")
            .join(session::project_dir_name(&cwd));
        let Ok(entries) = std::fs::read_dir(&project_dir) else {
            return Vec::new();
        };
        let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            found.push((mtime, stem.to_string()));
        }
        // Newest-first by modification time.
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found
            .into_iter()
            .map(|(_, id)| (id.clone(), id))
            .collect()
    }
}

/// LingXi-locked context-window budget used by [`context_window_usage`].
/// Matches the 200k-token Claude window referenced in `cost::budget`. The
/// rich per-model routing budget is deferred (it would require a new struct
/// on the frozen trait surface). (engine-data-commands)
const CONTEXT_WINDOW_MAX_TOKENS: u64 = 200_000;

/// Stable string label for a `HookEventType`, used by [`list_hooks`] to
/// populate [`traits::HookInfo::event`]. Avoids `Debug` derive
/// drift — the locked names are part of the M6-07 surface and the
/// claude-code parity. (M6-07)
fn event_str(et: &hooks::events::HookEventType) -> &'static str {
    use hooks::events::HookEventType as E;
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
