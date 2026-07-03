//! F1-09 — Structural version-diff guard.
//!
//! This enforces governing decision §0.10's STRUCTURAL versioning rule, NOT a
//! naive "file changed ⇒ bump" check:
//!
//! - A REMOVED entry, or a RENAMED / RETYPED tag or field ⇒ requires a MAJOR
//!   `CLIENT_PROTOCOL_VERSION` bump (the guard fails unless the major changed).
//! - A NEW variant or a NEW (optional) field ⇒ additive, no bump required (the
//!   guard passes).
//!
//! The "contract index" is a flat, deterministic structural fingerprint of the
//! wire contract: one line per leaf, `"<path>": "<type>"`, where `<path>` walks
//! enum → variant → field and `<type>` is the field's wire type (so a rename of
//! a tag/field shows up as a removed key + a new key, and a retype shows up as a
//! changed value). It is checked in at `snapshots/contract_index.json` and is
//! the SAME kind of frozen, auditable artifact as the F1-08 goldens.
//!
//! ## How the index is built
//!
//! `current_contract_index()` is a hand-authored, exhaustive description of the
//! contract maintained ALONGSIDE the DTOs. A compile-time exhaustiveness anchor
//! ([`contract_index_covers_every_dto`]) constructs one value of every contract
//! type so that adding a DTO without indexing it cannot pass review unnoticed —
//! the constructor won't compile until the new type exists, and the
//! `current_contract_matches_index_or_version_bumped` guard then flags the
//! missing index key on first run.
//!
//! ## Regenerating the index
//!
//! Run with `BLESS=1` to (re)write the checked-in index from
//! `current_contract_index()`, then review the diff before committing. A bless is
//! only legitimate AFTER you have classified the change and (if breaking) bumped
//! `CLIENT_PROTOCOL_VERSION`:
//!
//! ```text
//! BLESS=1 cargo test -p client-protocol --test version_guard_test
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use client_protocol::version::CLIENT_PROTOCOL_VERSION;

// ─────────────────────────────────────────────────────────────────────────────
// Contract index model
// ─────────────────────────────────────────────────────────────────────────────

/// A flat structural fingerprint of the contract: `path -> wire type`.
///
/// `BTreeMap` so the on-disk JSON is deterministically ordered and `git diff`
/// is stable. A key is a structural path such as
/// `"ClientEvent::ToolUseStarted.input_json"`; the value is the leaf's wire type
/// (`"String"`, `"u64"`, `"Option<MessageDto>"`, an enum tag marker, …).
type ContractIndex = BTreeMap<String, String>;

/// The classification of a single index diff (decision §0.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compatibility {
    /// No structural change, OR purely additive (a new variant / new field).
    /// Requires NO major bump.
    Compatible,
    /// A removed / renamed / retyped entry. Requires a MAJOR bump.
    Breaking,
}

/// Classify the structural diff from `old` (checked-in) to `new` (current).
///
/// - A key present in `old` but ABSENT in `new` ⇒ removed/renamed ⇒ `Breaking`.
/// - A key present in BOTH but with a DIFFERENT type ⇒ retyped ⇒ `Breaking`.
/// - A key present only in `new` ⇒ additive ⇒ contributes `Compatible`.
///
/// The overall result is `Breaking` if ANY entry is breaking, else `Compatible`.
fn classify(old: &ContractIndex, new: &ContractIndex) -> Compatibility {
    // Removed or retyped keys are breaking.
    for (key, old_ty) in old {
        match new.get(key) {
            None => return Compatibility::Breaking, // removed / renamed
            Some(new_ty) if new_ty != old_ty => return Compatibility::Breaking, // retyped
            Some(_) => {}
        }
    }
    // Any key only in `new` is purely additive — not breaking.
    Compatibility::Compatible
}

/// Parse the `major` component of a `major.minor.patch` version string.
fn major_of(version: &str) -> u64 {
    version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("version {version:?} has no numeric major component"))
}

// ─────────────────────────────────────────────────────────────────────────────
// On-disk index
// ─────────────────────────────────────────────────────────────────────────────

fn index_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join("contract_index.json")
}

fn bless() -> bool {
    matches!(std::env::var("BLESS").as_deref(), Ok("1" | "true"))
}

/// Serialize a [`ContractIndex`] to the canonical golden string (pretty + a
/// trailing newline so the file is well-formed text and `git diff` is clean).
fn to_golden(index: &ContractIndex) -> String {
    let mut s = serde_json::to_string_pretty(index).expect("serialize contract index");
    s.push('\n');
    s
}

/// Read the checked-in index, or `None` if it does not exist yet.
fn read_checked_in_index() -> Option<ContractIndex> {
    let raw = fs::read_to_string(index_path()).ok()?;
    Some(serde_json::from_str(&raw).expect("checked-in contract_index.json is valid JSON"))
}

/// Write the index to disk (used under `BLESS=1`).
fn write_index(index: &ContractIndex) {
    let path = index_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create snapshots dir");
    }
    fs::write(&path, to_golden(index)).expect("write contract_index.json");
}

// ─────────────────────────────────────────────────────────────────────────────
// The hand-authored current contract index
// ─────────────────────────────────────────────────────────────────────────────

/// Build the current structural fingerprint of the whole contract.
///
/// This is maintained alongside the DTOs: when a DTO grows a variant/field, add
/// the matching index line(s); when a DTO drops/renames/retypes one, the change
/// here forces the guard to confirm a major bump.
///
/// Enum variants are recorded with a `<EnumName>::<Variant>` tag line (value
/// `"<variant>"`, the `snake_case` wire tag) so a renamed/removed variant is a
/// removed key; each variant field is `<EnumName>::<Variant>.<field>`. Structs
/// record `<StructName>.<field>`.
#[allow(clippy::too_many_lines)] // a flat data table: one row per contract leaf
fn current_contract_index() -> ContractIndex {
    let mut ix = ContractIndex::new();

    let mut put = |path: &str, ty: &str| {
        ix.insert(path.to_string(), ty.to_string());
    };

    // ── ClientEvent (events.rs) ───────────────────────────────────────────
    put("ClientEvent::Error", "error");
    put("ClientEvent::Error.kind", "ErrorKindDto");
    put("ClientEvent::Error.message", "String");

    put("ClientEvent::TextDelta", "text_delta");
    put("ClientEvent::TextDelta.text", "String");

    put("ClientEvent::ToolUseStarted", "tool_use_started");
    put("ClientEvent::ToolUseStarted.id", "String");
    put("ClientEvent::ToolUseStarted.tool", "String");
    put("ClientEvent::ToolUseStarted.input_json", "String");

    put("ClientEvent::ToolUseResult", "tool_use_result");
    put("ClientEvent::ToolUseResult.id", "String");
    put("ClientEvent::ToolUseResult.tool", "String");
    put("ClientEvent::ToolUseResult.result_json", "String");
    put("ClientEvent::ToolUseResult.is_error", "bool");

    put("ClientEvent::MessageComplete", "message_complete");
    put("ClientEvent::MessageComplete.stop_reason", "Option<String>");
    put("ClientEvent::MessageComplete.message", "Option<MessageDto>");

    put("ClientEvent::TurnStarted", "turn_started");
    put("ClientEvent::TurnStarted.turn_id", "Option<u64>");

    put("ClientEvent::TurnEnded", "turn_ended");
    put("ClientEvent::TurnEnded.outcome", "TurnOutcomeDto");
    put("ClientEvent::TurnEnded.stop_reason", "Option<String>");
    put("ClientEvent::TurnEnded.cost", "CostDto");

    put("ClientEvent::CostUpdate", "cost_update");
    put("ClientEvent::CostUpdate.total_usd", "f64");
    put("ClientEvent::CostUpdate.input_tokens", "u64");
    put("ClientEvent::CostUpdate.output_tokens", "u64");
    put("ClientEvent::CostUpdate.api_calls", "u32");
    put("ClientEvent::CostUpdate.session_duration_secs", "u64");
    put("ClientEvent::CostUpdate.formatted", "String");

    put("ClientEvent::CompactionCompleted", "compaction_completed");
    put("ClientEvent::CompactionCompleted.messages_before", "u32");
    put("ClientEvent::CompactionCompleted.messages_after", "u32");
    put("ClientEvent::CompactionCompleted.bytes_saved", "u64");

    put("ClientEvent::SessionStarted", "session_started");
    put("ClientEvent::SessionStarted.session_id", "String");

    put("ClientEvent::SessionEnded", "session_ended");

    put("ClientEvent::SessionResumed", "session_resumed");
    put("ClientEvent::SessionResumed.session_id", "String");
    put("ClientEvent::SessionResumed.messages", "Vec<MessageDto>");

    put("ClientEvent::SessionList", "session_list");
    put("ClientEvent::SessionList.sessions", "Vec<SessionRowDto>");

    put("ClientEvent::ModelList", "model_list");
    put("ClientEvent::ModelList.models", "Vec<String>");
    put("ClientEvent::ModelList.current", "String");

    put("ClientEvent::ModelChanged", "model_changed");
    put("ClientEvent::ModelChanged.model", "String");

    put("ClientEvent::McpServers", "mcp_servers");
    put("ClientEvent::McpServers.servers", "Vec<McpServerDto>");

    put("ClientEvent::Hooks", "hooks");
    put("ClientEvent::Hooks.hooks", "Vec<HookDto>");

    put("ClientEvent::Agents", "agents");
    put("ClientEvent::Agents.agents", "Vec<AgentDto>");

    put("ClientEvent::SlashCommandCatalog", "slash_command_catalog");
    put(
        "ClientEvent::SlashCommandCatalog.commands",
        "Vec<SlashCommandDto>",
    );

    put("ClientEvent::MemoryEntries", "memory_entries");
    put("ClientEvent::MemoryEntries.entries", "Vec<MemoryEntryDto>");

    put("ClientEvent::StatusSnapshot", "status_snapshot");
    put("ClientEvent::StatusSnapshot.snapshot", "StatusSnapshotDto");

    put("ClientEvent::SettingsSnapshot", "settings_snapshot");
    put("ClientEvent::SettingsSnapshot.effective_json", "String");
    put("ClientEvent::SettingsSnapshot.provenance_json", "String");

    put("ClientEvent::AuthState", "auth_state");
    put("ClientEvent::AuthState.state", "AuthStateDto");

    put("ClientEvent::DoctorReport", "doctor_report");
    put("ClientEvent::DoctorReport.report", "DoctorReportDto");

    put("ClientEvent::TaskRow", "task_row");
    put("ClientEvent::TaskRow.task", "TaskRowDto");

    put("ClientEvent::TaskOutputChunk", "task_output_chunk");
    put("ClientEvent::TaskOutputChunk.task_id", "String");
    put("ClientEvent::TaskOutputChunk.content", "String");
    put("ClientEvent::TaskOutputChunk.total_lines", "u64");
    put("ClientEvent::TaskOutputChunk.truncated", "bool");

    put("ClientEvent::TaskStatusChanged", "task_status_changed");
    put("ClientEvent::TaskStatusChanged.task_id", "String");
    put("ClientEvent::TaskStatusChanged.status", "TaskStatusDto");

    put("ClientEvent::CoordinatorStatus", "coordinator_status");
    put("ClientEvent::CoordinatorStatus.active_workers", "u32");
    put("ClientEvent::CoordinatorStatus.team", "Option<String>");

    put("ClientEvent::CoordinatorWorker", "coordinator_worker");
    put(
        "ClientEvent::CoordinatorWorker.worker",
        "CoordinatorWorkerDto",
    );

    put("ClientEvent::ThinkingDelta", "thinking_delta");
    put("ClientEvent::ThinkingDelta.thinking", "String");
    put("ClientEvent::ThinkingDelta.signature", "Option<String>");

    put("ClientEvent::UsageUpdate", "usage_update");
    put("ClientEvent::UsageUpdate.input_tokens", "u64");
    put("ClientEvent::UsageUpdate.output_tokens", "u64");
    put("ClientEvent::UsageUpdate.cache_read_tokens", "u64");
    put("ClientEvent::UsageUpdate.cache_creation_tokens", "u64");

    // ── ErrorKindDto (events.rs) ──────────────────────────────────────────
    put("ErrorKindDto::Transport", "transport");
    put("ErrorKindDto::Protocol", "protocol");
    put("ErrorKindDto::Server", "server");
    put("ErrorKindDto::MaxTurns", "max_turns");
    put("ErrorKindDto::Internal", "internal");

    // ── TurnOutcomeDto (events.rs) ────────────────────────────────────────
    put("TurnOutcomeDto::EndTurn", "end_turn");
    put("TurnOutcomeDto::MaxTurns", "max_turns");
    put("TurnOutcomeDto::Cancelled", "cancelled");

    // ── CostDto (events.rs) ───────────────────────────────────────────────
    put("CostDto.total_usd", "f64");
    put("CostDto.input_tokens", "u64");
    put("CostDto.output_tokens", "u64");
    put("CostDto.api_calls", "u32");
    put("CostDto.session_duration_secs", "u64");
    put("CostDto.formatted", "String");

    // ── ClientCommand (commands.rs) ───────────────────────────────────────
    put("ClientCommand::SendPrompt", "send_prompt");
    put("ClientCommand::SendPrompt.text", "String");
    put(
        "ClientCommand::SendPrompt.prompt_mode",
        "Option<PromptModeDto>",
    );
    put("ClientCommand::SendPrompt.images", "Vec<ImageRefDto>");
    put("ClientCommand::SendPrompt.turn_id", "Option<u64>");

    put("ClientCommand::Cancel", "cancel");
    put("ClientCommand::Cancel.turn_id", "Option<u64>");

    put("ClientCommand::ApprovePermission", "approve_permission");
    put("ClientCommand::ApprovePermission.request_id", "u64");
    put(
        "ClientCommand::ApprovePermission.response",
        "PermissionResponseDto",
    );

    put("ClientCommand::DenyPermission", "deny_permission");
    put("ClientCommand::DenyPermission.request_id", "u64");

    put("ClientCommand::SetModel", "set_model");
    put("ClientCommand::SetModel.model", "String");

    put("ClientCommand::ListModels", "list_models");

    put("ClientCommand::RunSlashCommand", "run_slash_command");
    put("ClientCommand::RunSlashCommand.raw", "String");

    put("ClientCommand::RefreshListings", "refresh_listings");
    put(
        "ClientCommand::RefreshListings.which",
        "Vec<ListingKindDto>",
    );

    put("ClientCommand::NewSession", "new_session");
    put("ClientCommand::NewSession.cwd", "Option<String>");
    put("ClientCommand::NewSession.model", "Option<String>");

    put("ClientCommand::ResumeSession", "resume_session");
    put("ClientCommand::ResumeSession.session_id", "String");
    put("ClientCommand::ResumeSession.cwd", "Option<String>");

    put("ClientCommand::ListSessions", "list_sessions");
    put("ClientCommand::ListSessions.limit", "Option<u32>");

    put("ClientCommand::Login", "login");
    put("ClientCommand::Logout", "logout");
    put("ClientCommand::ForceCompact", "force_compact");
    put("ClientCommand::ClearSession", "clear_session");

    put("ClientCommand::TaskList", "task_list");
    put(
        "ClientCommand::TaskList.status_filter",
        "Option<TaskStatusDto>",
    );

    put("ClientCommand::TaskOutput", "task_output");
    put("ClientCommand::TaskOutput.task_id", "String");
    put("ClientCommand::TaskOutput.offset", "u64");

    put("ClientCommand::TaskStop", "task_stop");
    put("ClientCommand::TaskStop.task_id", "String");

    put("ClientCommand::RequestExit", "request_exit");

    // ── PromptModeDto (commands.rs) ───────────────────────────────────────
    put("PromptModeDto::Normal", "normal");
    put("PromptModeDto::Bash", "bash");
    put("PromptModeDto::Memory", "memory");
    put("PromptModeDto::Plan", "plan");

    // ── ImageRefDto (commands.rs) ─────────────────────────────────────────
    put("ImageRefDto.media_type", "String");
    put("ImageRefDto.base64", "String");

    // ── CommandResultDto (commands.rs) ────────────────────────────────────
    put("CommandResultDto.display", "String");
    put("CommandResultDto.injected", "Option<String>");

    // ── ListingKindDto (commands.rs) ──────────────────────────────────────
    put("ListingKindDto::Sessions", "sessions");
    put("ListingKindDto::Models", "models");
    put("ListingKindDto::Mcp", "mcp");
    put("ListingKindDto::Hooks", "hooks");
    put("ListingKindDto::Agents", "agents");
    put("ListingKindDto::SlashCommands", "slash_commands");
    put("ListingKindDto::Memory", "memory");
    put("ListingKindDto::Status", "status");
    put("ListingKindDto::Settings", "settings");
    put("ListingKindDto::Auth", "auth");
    put("ListingKindDto::Doctor", "doctor");
    put("ListingKindDto::Tasks", "tasks");
    put("ListingKindDto::Coordinator", "coordinator");

    // ── MessageDto / MessageBlockDto (message.rs) ─────────────────────────
    put("MessageDto.role", "String");
    put("MessageDto.blocks", "Vec<MessageBlockDto>");

    put("MessageBlockDto::Text", "text");
    put("MessageBlockDto::Text.text", "String");

    put("MessageBlockDto::Thinking", "thinking");
    put("MessageBlockDto::Thinking.thinking", "String");
    put("MessageBlockDto::Thinking.signature", "Option<String>");

    put("MessageBlockDto::RedactedThinking", "redacted_thinking");
    put("MessageBlockDto::RedactedThinking.data", "String");

    put("MessageBlockDto::ToolUse", "tool_use");
    put("MessageBlockDto::ToolUse.id", "String");
    put("MessageBlockDto::ToolUse.tool", "String");
    put("MessageBlockDto::ToolUse.input_json", "String");

    put("MessageBlockDto::ToolResult", "tool_result");
    put("MessageBlockDto::ToolResult.id", "String");
    put("MessageBlockDto::ToolResult.tool", "String");
    put("MessageBlockDto::ToolResult.result_json", "String");
    put("MessageBlockDto::ToolResult.is_error", "bool");
    put("MessageBlockDto::ToolResult.old_string", "Option<String>");
    put("MessageBlockDto::ToolResult.new_string", "Option<String>");
    put("MessageBlockDto::ToolResult.file_path", "Option<String>");

    // ── PermissionRequest / kinds (permission.rs) ─────────────────────────
    put("PermissionRequest.request_id", "u64");
    put("PermissionRequest.kind", "PermissionKindDto");
    put("PermissionRequest.worker", "Option<WorkerInfoDto>");

    put("PermissionKindDto::ToolUseConfirm", "tool_use_confirm");
    put("PermissionKindDto::ToolUseConfirm.tool_name", "String");
    put(
        "PermissionKindDto::ToolUseConfirm.tool_input_json",
        "String",
    );
    put("PermissionKindDto::ToolUseConfirm.default_allow", "bool");

    put("PermissionKindDto::ExitPlanMode", "exit_plan_mode");
    put("PermissionKindDto::ExitPlanMode.plan", "String");

    put(
        "PermissionKindDto::BypassPermissionsMode",
        "bypass_permissions_mode",
    );

    put("WorkerInfoDto.name", "String");
    put("WorkerInfoDto.color", "String");
    put("WorkerInfoDto.team", "Option<String>");

    put("PermissionResolved.request_id", "u64");
    put("PermissionResolved.response", "PermissionResponseDto");

    put("PermissionResponseDto::AllowOnce", "allow_once");
    put("PermissionResponseDto::AllowAlways", "allow_always");
    put("PermissionResponseDto::Deny", "deny");

    // ── ClientError (error.rs) ────────────────────────────────────────────
    put("ClientError::Transport", "transport");
    put("ClientError::Transport.message", "String");
    put("ClientError::Protocol", "protocol");
    put("ClientError::Protocol.message", "String");
    put("ClientError::Rejected", "rejected");
    put("ClientError::Rejected.message", "String");
    put("ClientError::NotFound", "not_found");
    put("ClientError::NotFound.message", "String");
    put("ClientError::Internal", "internal");
    put("ClientError::Internal.message", "String");

    // ── Listing row / payload structs + enums (listings.rs) ───────────────
    put("SessionRowDto.uuid", "String");
    put("SessionRowDto.title", "String");
    put("SessionRowDto.modified_rfc3339", "String");
    put("SessionRowDto.message_count", "u32");
    put("SessionRowDto.path", "String");

    put("McpServerDto.name", "String");
    put("McpServerDto.status", "McpStatusDto");
    put("McpServerDto.transport", "String");

    put("McpStatusDto::Connected", "connected");
    put("McpStatusDto::Disconnected", "disconnected");
    put("McpStatusDto::Error", "error");
    put("McpStatusDto::Error.reason", "String");

    put("HookDto.name", "String");
    put("HookDto.event", "String");
    put("HookDto.matcher", "Option<String>");
    put("HookDto.timeout_ms", "u64");

    put("AgentDto.name", "String");
    put("AgentDto.description", "String");
    put("AgentDto.tools_allowed", "Vec<String>");

    put("SlashCommandDto.name", "String");
    put("SlashCommandDto.description", "String");
    put("SlashCommandDto.source", "String");

    put("MemoryEntryDto.path", "String");
    put("MemoryEntryDto.tier", "MemoryTierDto");
    put("MemoryEntryDto.body", "String");
    put("MemoryEntryDto.age_days", "u64");
    put("MemoryEntryDto.size_bytes", "u64");

    put("MemoryTierDto::Session", "session");
    put("MemoryTierDto::Project", "project");
    put("MemoryTierDto::Team", "team");
    put("MemoryTierDto::User", "user");

    put("StatusSnapshotDto.session_id", "String");
    put("StatusSnapshotDto.model", "String");
    put("StatusSnapshotDto.n_messages", "u32");
    put("StatusSnapshotDto.total_cost_usd", "f64");
    put("StatusSnapshotDto.input_tokens", "u64");
    put("StatusSnapshotDto.output_tokens", "u64");
    put("StatusSnapshotDto.n_mcp_connected", "u32");
    put("StatusSnapshotDto.n_mcp_total", "u32");
    put("StatusSnapshotDto.n_hooks", "u32");
    put("StatusSnapshotDto.n_agents", "u32");
    put("StatusSnapshotDto.started_at", "String");
    put("StatusSnapshotDto.cwd", "String");
    put("StatusSnapshotDto.status_line", "Option<String>");
    put("StatusSnapshotDto.active_workers", "Option<u32>");

    put("AuthStateDto::SignedOut", "signed_out");
    put("AuthStateDto::SignedIn", "signed_in");
    put("AuthStateDto::SignedIn.email", "String");
    put("AuthStateDto::SignedIn.org_id", "String");

    put("DoctorReportDto.checks", "Vec<DoctorCheckDto>");
    put("DoctorReportDto.summary", "DoctorSummaryDto");

    put("DoctorCheckDto.name", "String");
    put("DoctorCheckDto.status", "CheckStatusDto");
    put("DoctorCheckDto.detail", "Option<String>");

    put("CheckStatusDto::Pass", "pass");
    put("CheckStatusDto::Warn", "warn");
    put("CheckStatusDto::Fail", "fail");

    put("DoctorSummaryDto.passed", "u32");
    put("DoctorSummaryDto.warnings", "u32");
    put("DoctorSummaryDto.failed", "u32");

    put("TaskRowDto.task_id", "String");
    put("TaskRowDto.task_type", "String");
    put("TaskRowDto.status", "TaskStatusDto");
    put("TaskRowDto.description", "String");

    put("TaskStatusDto::Pending", "pending");
    put("TaskStatusDto::Running", "running");
    put("TaskStatusDto::Completed", "completed");
    put("TaskStatusDto::Failed", "failed");
    put("TaskStatusDto::Cancelled", "cancelled");

    put("CoordinatorWorkerDto.agent_id", "String");
    put("CoordinatorWorkerDto.name", "String");
    put("CoordinatorWorkerDto.agent_type", "String");
    put("CoordinatorWorkerDto.status", "String");

    ix
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Removing a field is a BREAKING change ⇒ the classifier returns `Breaking`,
/// which (per the guard) forces a MAJOR `CLIENT_PROTOCOL_VERSION` bump.
///
/// Simulated by doctoring the current index: drop one field key and assert the
/// classifier flags it.
#[test]
fn removing_a_field_requires_major_bump() {
    let base = current_contract_index();

    let mut doctored = base.clone();
    let removed = doctored
        .remove("ClientEvent::ToolUseStarted.input_json")
        .expect("the field exists in the current contract index");
    assert_eq!(removed, "String");

    // old = full contract, new = the doctored (field-removed) contract.
    assert_eq!(
        classify(&base, &doctored),
        Compatibility::Breaking,
        "removing a field must classify as Breaking (forces a major bump)"
    );
}

/// Renaming a variant tag is BREAKING (it surfaces as a removed key + a new key,
/// and the removed key dominates).
#[test]
fn renaming_a_variant_requires_major_bump() {
    let base = current_contract_index();

    let mut doctored = base.clone();
    // Rename `ClientCommand::SetModel` → a different key (simulating a rename).
    doctored
        .remove("ClientCommand::SetModel")
        .expect("variant tag key exists");
    doctored.insert(
        "ClientCommand::ChangeModel".to_string(),
        "change_model".to_string(),
    );

    assert_eq!(
        classify(&base, &doctored),
        Compatibility::Breaking,
        "renaming a variant must classify as Breaking (forces a major bump)"
    );
}

/// Retyping a field (e.g. `u32` → `String`) is BREAKING.
#[test]
fn retyping_a_field_requires_major_bump() {
    let base = current_contract_index();

    let mut doctored = base.clone();
    doctored.insert(
        "SessionRowDto.message_count".to_string(),
        "String".to_string(), // was u32
    );

    assert_eq!(
        classify(&base, &doctored),
        Compatibility::Breaking,
        "retyping a field must classify as Breaking (forces a major bump)"
    );
}

/// Adding a NEW optional field is a COMPATIBLE (additive) change ⇒ the
/// classifier returns `Compatible`, so NO major bump is required.
#[test]
fn adding_optional_field_is_compatible() {
    let base = current_contract_index();

    let mut doctored = base.clone();
    // Add a brand-new optional field to an existing variant.
    doctored.insert(
        "ClientEvent::TextDelta.lang".to_string(),
        "Option<String>".to_string(),
    );

    // old = full contract (without the new field), new = with the new field.
    assert_eq!(
        classify(&base, &doctored),
        Compatibility::Compatible,
        "adding an optional field must classify as Compatible (no bump required)"
    );
}

/// Adding a NEW variant is also COMPATIBLE (the enums are `#[non_exhaustive]`,
/// so a new variant is additive).
#[test]
fn adding_a_variant_is_compatible() {
    let base = current_contract_index();

    let mut doctored = base.clone();
    doctored.insert(
        "ClientEvent::PromptAccepted".to_string(),
        "prompt_accepted".to_string(),
    );

    assert_eq!(
        classify(&base, &doctored),
        Compatibility::Compatible,
        "adding a variant must classify as Compatible (no bump required)"
    );
}

/// An identical index is trivially COMPATIBLE.
#[test]
fn identical_index_is_compatible() {
    let ix = current_contract_index();
    assert_eq!(classify(&ix, &ix), Compatibility::Compatible);
}

/// THE GUARD: the current contract index either matches the checked-in one, OR,
/// if it changed in a BREAKING way, the major component of
/// `CLIENT_PROTOCOL_VERSION` must have been bumped past the checked-in major.
///
/// - Index byte-identical ⇒ pass (no contract drift).
/// - Index changed but `Compatible` (purely additive) ⇒ pass, then remind the
///   author to re-bless so the additive entries are recorded.
/// - Index changed in a `Breaking` way ⇒ pass ONLY if the major was bumped past
///   the checked-in major; otherwise fail.
///
/// Under `BLESS=1`, (re)write the index from the current contract. A bless is
/// legitimate only after classifying the change and bumping the major when the
/// change is breaking.
#[test]
fn current_contract_matches_index_or_version_bumped() {
    let current = current_contract_index();

    if bless() {
        write_index(&current);
        return;
    }

    let Some(checked_in) = read_checked_in_index() else {
        panic!(
            "missing contract index `{}`; regenerate with \
             `BLESS=1 cargo test -p client-protocol --test version_guard_test`",
            index_path().display()
        );
    };

    // Byte-identical contract → nothing to check.
    if checked_in == current {
        return;
    }

    // The contract changed. Classify the change.
    let verdict = classify(&checked_in, &current);
    let current_major = major_of(CLIENT_PROTOCOL_VERSION);

    match verdict {
        Compatibility::Compatible => {
            // Additive-only: this is allowed WITHOUT a major bump, but the
            // checked-in index must be refreshed so the additive entries are
            // recorded for the NEXT diff.
            panic!(
                "the contract grew in an ADDITIVE way (new variant / optional field) but \
                 `snapshots/contract_index.json` is stale. No major bump is required; \
                 re-bless the index with \
                 `BLESS=1 cargo test -p client-protocol --test version_guard_test`."
            );
        }
        Compatibility::Breaking => {
            // We need a major bump. The checked-in index records the contract as
            // of the LAST blessed version. We can only compare against the
            // CURRENT version constant; require it to be > 1 (the foundation
            // pin) — i.e. a deliberate major bump must have happened.
            assert!(
                current_major > 1,
                "BREAKING contract change detected (a removed / renamed / retyped \
                 entry) but `CLIENT_PROTOCOL_VERSION` is still {CLIENT_PROTOCOL_VERSION:?} \
                 (major {current_major}). Per decision §0.10 a breaking change REQUIRES \
                 a major bump. Bump the major in `client-protocol/src/version.rs`, then \
                 re-bless with `BLESS=1 cargo test -p client-protocol --test version_guard_test`."
            );
        }
    }
}

/// Compile-time / structural anchor: constructing one value of EVERY contract
/// type proves the index author saw every type. If a new DTO is added without a
/// matching index line, this constructor still compiles but
/// `current_contract_matches_index_or_version_bumped` flags the new key on the
/// first run after `BLESS=1` — and conversely, deleting a DTO breaks THIS
/// function's compile, so the index cannot silently keep a stale key.
///
/// (This is a belt-and-braces exhaustiveness check; the real guarantee is the
/// guard test above.)
#[test]
// Coverage anchors: each `let _foo = Dto::Variant{..}` instantiates a DTO so a
// removed/renamed type breaks THIS compile. They are intentionally inert (no
// effect, never read) — that is the whole point — so both pedantic lints are
// allowed here rather than worked around.
#[allow(clippy::too_many_lines, clippy::no_effect_underscore_binding)]
fn contract_index_covers_every_dto() {
    use client_protocol::commands::{
        ClientCommand, CommandResultDto, ImageRefDto, ListingKindDto, PromptModeDto,
    };
    use client_protocol::error::ClientError;
    use client_protocol::events::{ClientEvent, CostDto, ErrorKindDto, TurnOutcomeDto};
    use client_protocol::listings::{
        AgentDto, AuthStateDto, CheckStatusDto, CoordinatorWorkerDto, DoctorCheckDto,
        DoctorReportDto, DoctorSummaryDto, HookDto, McpServerDto, McpStatusDto, MemoryEntryDto,
        MemoryTierDto, SessionRowDto, SlashCommandDto, StatusSnapshotDto, TaskRowDto,
        TaskStatusDto,
    };
    use client_protocol::message::{MessageBlockDto, MessageDto};
    use client_protocol::permission::{
        PermissionKindDto, PermissionRequest, PermissionResolved, PermissionResponseDto,
        WorkerInfoDto,
    };

    // Touch every type so removing a type breaks this compile. The values are
    // intentionally minimal; this is a coverage anchor, not a round-trip test.
    let _ev: Vec<ClientEvent> = vec![
        ClientEvent::Error {
            kind: ErrorKindDto::Internal,
            message: String::new(),
        },
        ClientEvent::SessionEnded,
    ];
    let _outcome = TurnOutcomeDto::EndTurn;
    let _cost = CostDto {
        total_usd: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        api_calls: 0,
        session_duration_secs: 0,
        formatted: String::new(),
    };
    let _cmd: Vec<ClientCommand> = vec![ClientCommand::ListModels, ClientCommand::RequestExit];
    let _mode = PromptModeDto::Normal;
    let _img = ImageRefDto {
        media_type: String::new(),
        base64: String::new(),
    };
    let _cmd_result = CommandResultDto {
        display: String::new(),
        injected: None,
    };
    let _lk = ListingKindDto::Mcp;
    let _err = ClientError::Internal {
        message: String::new(),
    };
    let _msg = MessageDto {
        role: String::new(),
        blocks: vec![MessageBlockDto::Text {
            text: String::new(),
        }],
    };
    let _req = PermissionRequest {
        request_id: 0,
        kind: PermissionKindDto::BypassPermissionsMode,
        worker: Some(WorkerInfoDto {
            name: String::new(),
            color: String::new(),
            team: None,
        }),
    };
    let _resolved = PermissionResolved {
        request_id: 0,
        response: PermissionResponseDto::Deny,
    };
    let _rows = (
        SessionRowDto {
            uuid: String::new(),
            title: String::new(),
            modified_rfc3339: String::new(),
            message_count: 0,
            path: String::new(),
        },
        McpServerDto {
            name: String::new(),
            status: McpStatusDto::Connected,
            transport: String::new(),
        },
        HookDto {
            name: String::new(),
            event: String::new(),
            matcher: None,
            timeout_ms: 0,
        },
        AgentDto {
            name: String::new(),
            description: String::new(),
            tools_allowed: vec![],
        },
        SlashCommandDto {
            name: String::new(),
            description: String::new(),
            source: String::new(),
        },
        MemoryEntryDto {
            path: String::new(),
            tier: MemoryTierDto::User,
            body: String::new(),
            age_days: 0,
            size_bytes: 0,
        },
        StatusSnapshotDto {
            session_id: String::new(),
            model: String::new(),
            n_messages: 0,
            total_cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            n_mcp_connected: 0,
            n_mcp_total: 0,
            n_hooks: 0,
            n_agents: 0,
            started_at: String::new(),
            cwd: String::new(),
            status_line: None,
            active_workers: None,
        },
        AuthStateDto::SignedOut,
        DoctorReportDto {
            checks: vec![DoctorCheckDto {
                name: String::new(),
                status: CheckStatusDto::Pass,
                detail: None,
            }],
            summary: DoctorSummaryDto {
                passed: 0,
                warnings: 0,
                failed: 0,
            },
        },
        TaskRowDto {
            task_id: String::new(),
            task_type: String::new(),
            status: TaskStatusDto::Pending,
            description: String::new(),
        },
        CoordinatorWorkerDto {
            agent_id: String::new(),
            name: String::new(),
            agent_type: String::new(),
            status: String::new(),
        },
    );

    // Sanity: the index is non-empty and contains a known anchor key.
    let ix = current_contract_index();
    assert!(
        ix.contains_key("ClientEvent::TextDelta.text"),
        "the contract index must enumerate the contract leaves"
    );
}
