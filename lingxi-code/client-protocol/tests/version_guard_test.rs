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

/// Whether a BREAKING contract change is legitimately covered by a version
/// bump: the CURRENT major must exceed the major blessed alongside the
/// CHECKED-IN index — never a literal pin. A literal (`current_major > 1`)
/// is only correct until the first bump ever lands: once
/// `CLIENT_PROTOCOL_VERSION` is permanently `2.x.x` or higher, a literal `> 1`
/// is permanently `true` and a SECOND breaking change with no bump at all
/// would sail through ungated. Pulled out as its own pure function so the
/// regression has a direct unit test below, independent of disk I/O.
fn major_was_bumped_past(current_major: u64, blessed_major: u64) -> bool {
    current_major > blessed_major
}

// ─────────────────────────────────────────────────────────────────────────────
// On-disk index
// ─────────────────────────────────────────────────────────────────────────────

fn index_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join("contract_index.json")
}

/// Sidecar recording the `CLIENT_PROTOCOL_VERSION` MAJOR that was current the
/// last time `contract_index.json` was blessed. LOAD-BEARING (finding,
/// post-Task-2 review): the breaking-change threshold must travel with the
/// version, not be a literal pin — `assert!(current_major > 1)` looks correct
/// the day it's written but is silently permanent once any bump ever lands: a
/// SECOND breaking change after `CLIENT_PROTOCOL_VERSION` is already `2.x.x`
/// would satisfy `2 > 1` with no bump at all. Comparing against the major
/// blessed alongside the CHECKED-IN index (this file), the same way the index
/// itself is a checked-in/current diff, means every future breaking change —
/// not just the first one — is gated.
fn blessed_major_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join("blessed_major.txt")
}

/// Read the major recorded at the last bless. `None` only for a repo that
/// predates this sidecar (never happens post-bootstrap: every bless from here
/// on writes it in lockstep with `contract_index.json`).
fn read_blessed_major() -> Option<u64> {
    let raw = fs::read_to_string(blessed_major_path()).ok()?;
    Some(raw.trim().parse().unwrap_or_else(|error| {
        panic!(
            "{} does not contain a valid integer major: {error}",
            blessed_major_path().display()
        )
    }))
}

/// Write the blessed major (used under `BLESS=1`, alongside `write_index`,
/// on EVERY bless — additive or breaking — so the sidecar always reflects
/// the major of whatever contract is currently checked in.
fn write_blessed_major(major: u64) {
    let path = blessed_major_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create snapshots dir");
    }
    fs::write(&path, format!("{major}\n")).expect("write blessed_major.txt");
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

    put("ClientEvent::SystemNotice", "system_notice");
    put("ClientEvent::SystemNotice.message", "String");
    put("ClientEvent::SystemNotice.is_error", "bool");

    put("ClientEvent::TextDelta", "text_delta");
    put("ClientEvent::TextDelta.text", "String");

    put("ClientEvent::ToolUseStarted", "tool_use_started");
    put("ClientEvent::ToolUseStarted.id", "String");
    put("ClientEvent::ToolUseStarted.tool", "String");
    put("ClientEvent::ToolUseStarted.input_json", "String");

    put("ClientEvent::ToolHeartbeat", "tool_heartbeat");
    put("ClientEvent::ToolHeartbeat.id", "String");
    put("ClientEvent::ToolHeartbeat.tool", "String");
    put("ClientEvent::ToolHeartbeat.elapsed_ms", "u64");

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

    put(
        "ClientEvent::PermissionModeChanged",
        "permission_mode_changed",
    );
    put("ClientEvent::PermissionModeChanged.mode", "String");

    put(
        "ClientEvent::ProviderCredentialStatus",
        "provider_credential_status",
    );
    put("ClientEvent::ProviderCredentialStatus.operation_id", "u64");
    put(
        "ClientEvent::ProviderCredentialStatus.configured_provider_ids",
        "Vec<String>",
    );
    put(
        "ClientEvent::ProviderCredentialStatus.unavailable_provider_ids",
        "Vec<String>",
    );
    put(
        "ClientEvent::ProviderCredentialStatus.storage_encrypted",
        "bool",
    );
    put(
        "ClientEvent::ProviderCredentialStatus.error",
        "Option<String>",
    );

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

    put("ClientEvent::CommandsChanged", "commands_changed");
    put(
        "ClientEvent::CommandsChanged.commands",
        "Vec<SlashCommandDto>",
    );

    put("ClientEvent::AppsChanged", "apps_changed");
    put("ClientEvent::AppsChanged.apps", "Vec<AppRecordDto>");

    put("ClientEvent::AppEvent", "app_event");
    put("ClientEvent::AppEvent.event", "AppEventDto");

    put(
        "ClientEvent::AppDesignerRequested",
        "app_designer_requested",
    );
    put("ClientEvent::AppDesignerRequested.app_id", "String");
    put("ClientEvent::AppDesignerRequested.interaction_id", "String");
    put("ClientEvent::AppDesignerRequested.revision", "u64");

    put(
        "ClientEvent::AppDesignDraftChanged",
        "app_design_draft_changed",
    );
    put("ClientEvent::AppDesignDraftChanged.app_id", "String");
    put("ClientEvent::AppDesignDraftChanged.revision", "u64");
    put(
        "ClientEvent::AppDesignDraftChanged.fields",
        "HashMap<String, DesignValueDto>",
    );

    put(
        "ClientEvent::AppDesignSuggestionAvailable",
        "app_design_suggestion_available",
    );
    put("ClientEvent::AppDesignSuggestionAvailable.app_id", "String");
    put(
        "ClientEvent::AppDesignSuggestionAvailable.suggestion_id",
        "String",
    );
    put(
        "ClientEvent::AppDesignSuggestionAvailable.based_on_revision",
        "u64",
    );
    put(
        "ClientEvent::AppDesignSuggestionAvailable.patch",
        "AppDesignPatchDto",
    );

    put("ClientEvent::AppDesignConflict", "app_design_conflict");
    put("ClientEvent::AppDesignConflict.app_id", "String");
    put("ClientEvent::AppDesignConflict.expected_revision", "u64");
    put("ClientEvent::AppDesignConflict.actual_revision", "u64");

    put("ClientEvent::AppWorkflowChanged", "app_workflow_changed");
    put("ClientEvent::AppWorkflowChanged.app_id", "String");
    put(
        "ClientEvent::AppWorkflowChanged.state",
        "AppWorkflowStateDto",
    );
    put("ClientEvent::AppWorkflowChanged.detail", "Option<String>");

    put(
        "ClientEvent::AppGenerationProgress",
        "app_generation_progress",
    );
    put("ClientEvent::AppGenerationProgress.app_id", "String");
    put("ClientEvent::AppGenerationProgress.stage", "String");
    put("ClientEvent::AppGenerationProgress.percent", "Option<u8>");
    put(
        "ClientEvent::AppGenerationProgress.detail",
        "Option<String>",
    );

    put("ClientEvent::AppRuntimeChanged", "app_runtime_changed");
    put("ClientEvent::AppRuntimeChanged.app_id", "String");
    put("ClientEvent::AppRuntimeChanged.state", "AppRuntimeStateDto");
    put(
        "ClientEvent::AppRuntimeChanged.details",
        "Option<AppRuntimeDetailsDto>",
    );
    put(
        "ClientEvent::AppRuntimeChanged.last_error",
        "Option<String>",
    );

    put("ClientEvent::AppPreviewReady", "app_preview_ready");
    put("ClientEvent::AppPreviewReady.app_id", "String");
    put("ClientEvent::AppPreviewReady.interaction_id", "String");
    put("ClientEvent::AppPreviewReady.revision", "u64");
    put("ClientEvent::AppPreviewReady.url", "Option<String>");

    put(
        "ClientEvent::AppCheckpointCreated",
        "app_checkpoint_created",
    );
    put("ClientEvent::AppCheckpointCreated.app_id", "String");
    put(
        "ClientEvent::AppCheckpointCreated.checkpoint",
        "AppCheckpointDto",
    );

    put("ClientEvent::AppOperationFailed", "app_operation_failed");
    put("ClientEvent::AppOperationFailed.app_id", "Option<String>");
    put("ClientEvent::AppOperationFailed.code", "AppErrorCodeDto");
    put("ClientEvent::AppOperationFailed.message", "String");

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

    put("ClientEvent::ApiRetry", "api_retry");
    put("ClientEvent::ApiRetry.message", "String");
    put("ClientEvent::ApiRetry.attempt", "u32");
    put("ClientEvent::ApiRetry.max_retries", "u32");
    put("ClientEvent::ApiRetry.delay_ms", "u64");

    // ── ErrorKindDto (events.rs) ──────────────────────────────────────────
    put("ErrorKindDto::Transport", "transport");
    put("ErrorKindDto::Protocol", "protocol");
    put("ErrorKindDto::Server", "server");
    put("ErrorKindDto::MaxTurns", "max_turns");
    put("ErrorKindDto::Rejected", "rejected");
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

    put(
        "ClientCommand::ApproveComputerAccess",
        "approve_computer_access",
    );
    put("ClientCommand::ApproveComputerAccess.request_id", "u64");
    put(
        "ClientCommand::ApproveComputerAccess.response",
        "ComputerAccessResponseDto",
    );

    put("ClientCommand::DenyComputerAccess", "deny_computer_access");
    put("ClientCommand::DenyComputerAccess.request_id", "u64");

    put("ClientCommand::SetPermissionMode", "set_permission_mode");
    put("ClientCommand::SetPermissionMode.mode", "String");

    put(
        "ClientCommand::ListProviderCredentials",
        "list_provider_credentials",
    );
    put("ClientCommand::ListProviderCredentials.operation_id", "u64");
    put(
        "ClientCommand::ListProviderCredentials.provider_ids",
        "Vec<String>",
    );
    put(
        "ClientCommand::SetProviderCredential",
        "set_provider_credential",
    );
    put("ClientCommand::SetProviderCredential.operation_id", "u64");
    put("ClientCommand::SetProviderCredential.provider_id", "String");
    put(
        "ClientCommand::SetProviderCredential.credential",
        "ProviderCredentialSecretDto",
    );
    put(
        "ClientCommand::DeleteProviderCredential",
        "delete_provider_credential",
    );
    put(
        "ClientCommand::DeleteProviderCredential.operation_id",
        "u64",
    );
    put(
        "ClientCommand::DeleteProviderCredential.provider_id",
        "String",
    );

    put("ProviderCredentialSecretDto.value", "String");

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

    put("ClientCommand::ListApps", "list_apps");

    put("ClientCommand::GetAppDetails", "get_app_details");
    put("ClientCommand::GetAppDetails.app_id", "String");

    put("ClientCommand::CreateApp", "create_app");
    put("ClientCommand::CreateApp.name", "String");
    put("ClientCommand::CreateApp.origin", "AppCreateOriginDto");
    put("ClientCommand::CreateApp.brief", "String");
    put("ClientCommand::CreateApp.conversation_id", "Option<String>");

    put("ClientCommand::UpdateAppBrief", "update_app_brief");
    put("ClientCommand::UpdateAppBrief.app_id", "String");
    put("ClientCommand::UpdateAppBrief.brief", "String");

    put(
        "ClientCommand::RetryAppQuestionnaire",
        "retry_app_questionnaire",
    );
    put("ClientCommand::RetryAppQuestionnaire.app_id", "String");

    put("ClientCommand::BeginAppPlanning", "begin_app_planning");
    put("ClientCommand::BeginAppPlanning.app_id", "String");

    put("ClientCommand::RetryAppPlan", "retry_app_plan");
    put("ClientCommand::RetryAppPlan.app_id", "String");

    put("ClientCommand::OpenAppDesigner", "open_app_designer");
    put("ClientCommand::OpenAppDesigner.app_id", "String");

    put(
        "ClientCommand::UpdateAppDesignDraft",
        "update_app_design_draft",
    );
    put("ClientCommand::UpdateAppDesignDraft.app_id", "String");
    put(
        "ClientCommand::UpdateAppDesignDraft.expected_revision",
        "u64",
    );
    put(
        "ClientCommand::UpdateAppDesignDraft.patch",
        "AppDesignPatchDto",
    );

    put(
        "ClientCommand::ApplyAgentDesignSuggestion",
        "apply_agent_design_suggestion",
    );
    put("ClientCommand::ApplyAgentDesignSuggestion.app_id", "String");
    put(
        "ClientCommand::ApplyAgentDesignSuggestion.suggestion_id",
        "String",
    );
    put(
        "ClientCommand::ApplyAgentDesignSuggestion.expected_revision",
        "u64",
    );

    put(
        "ClientCommand::RequestAppDesignSuggestion",
        "request_app_design_suggestion",
    );
    put("ClientCommand::RequestAppDesignSuggestion.app_id", "String");
    put(
        "ClientCommand::RequestAppDesignSuggestion.expected_revision",
        "u64",
    );
    put(
        "ClientCommand::RequestAppDesignSuggestion.prompt",
        "Option<String>",
    );

    put(
        "ClientCommand::DismissAppDesignSuggestion",
        "dismiss_app_design_suggestion",
    );
    put("ClientCommand::DismissAppDesignSuggestion.app_id", "String");
    put(
        "ClientCommand::DismissAppDesignSuggestion.suggestion_id",
        "String",
    );

    put("ClientCommand::ConfirmAppDesign", "confirm_app_design");
    put("ClientCommand::ConfirmAppDesign.app_id", "String");
    put("ClientCommand::ConfirmAppDesign.revision", "u64");
    put("ClientCommand::ConfirmAppDesign.interaction_id", "String");

    put("ClientCommand::CancelAppDesign", "cancel_app_design");
    put("ClientCommand::CancelAppDesign.app_id", "String");

    put("ClientCommand::StartApp", "start_app");
    put("ClientCommand::StartApp.app_id", "String");

    put("ClientCommand::StopApp", "stop_app");
    put("ClientCommand::StopApp.app_id", "String");

    put("ClientCommand::RestartApp", "restart_app");
    put("ClientCommand::RestartApp.app_id", "String");

    put("ClientCommand::ConfirmAppPreview", "confirm_app_preview");
    put("ClientCommand::ConfirmAppPreview.app_id", "String");
    put("ClientCommand::ConfirmAppPreview.revision", "u64");
    put("ClientCommand::ConfirmAppPreview.interaction_id", "String");

    put("ClientCommand::RequestAppRevision", "request_app_revision");
    put("ClientCommand::RequestAppRevision.app_id", "String");
    put("ClientCommand::RequestAppRevision.prompt", "String");

    put("ClientCommand::RetryAppGeneration", "retry_app_generation");
    put("ClientCommand::RetryAppGeneration.app_id", "String");
    put("ClientCommand::RetryAppGeneration.prompt", "Option<String>");

    put(
        "ClientCommand::ExecuteAppBridgeRequest",
        "execute_app_bridge_request",
    );
    put(
        "ClientCommand::ExecuteAppBridgeRequest.request",
        "AppBridgeRequestDto",
    );

    put(
        "ClientCommand::ResolveAppUiRequest",
        "resolve_app_ui_request",
    );
    put("ClientCommand::ResolveAppUiRequest.request_id", "String");
    put(
        "ClientCommand::ResolveAppUiRequest.decision",
        "AppAuthorizationDecisionDto",
    );
    put(
        "ClientCommand::ResolveAppUiRequest.result_json",
        "Option<String>",
    );
    put("ClientCommand::ResolveAppUiRequest.error", "Option<String>");

    put(
        "ClientCommand::ResolveAppCapabilityRequest",
        "resolve_app_capability_request",
    );
    put(
        "ClientCommand::ResolveAppCapabilityRequest.request_id",
        "String",
    );
    put(
        "ClientCommand::ResolveAppCapabilityRequest.decision",
        "AppAuthorizationDecisionDto",
    );

    put(
        "ClientCommand::ResetAppPermissions",
        "reset_app_permissions",
    );
    put("ClientCommand::ResetAppPermissions.app_id", "String");

    put("ClientCommand::ListAppCheckpoints", "list_app_checkpoints");
    put("ClientCommand::ListAppCheckpoints.app_id", "String");

    put(
        "ClientCommand::RestoreAppCheckpoint",
        "restore_app_checkpoint",
    );
    put("ClientCommand::RestoreAppCheckpoint.app_id", "String");
    put(
        "ClientCommand::RestoreAppCheckpoint.checkpoint_id",
        "String",
    );

    put("ClientCommand::DeleteApp", "delete_app");
    put("ClientCommand::DeleteApp.app_id", "String");

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

    put("MessageBlockDto::CompactBoundary", "compact_boundary");
    put("MessageBlockDto::CompactBoundary.messages_before", "u32");
    put("MessageBlockDto::CompactBoundary.messages_after", "u32");
    put("MessageBlockDto::CompactBoundary.summary", "String");

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

    // ── ComputerAccessRequestDto / ComputerAccessResponseDto
    //    (computer_access.rs) ────────────────────────────────────────────
    put("ComputerAccessRequestDto.request_id", "u64");
    put("ComputerAccessRequestDto.reason", "String");
    put("ComputerAccessRequestDto.apps", "Vec<RequestedAppDto>");
    put("ComputerAccessRequestDto.tier", "AccessTierDto");
    put("ComputerAccessRequestDto.clipboard_read", "bool");
    put("ComputerAccessRequestDto.clipboard_write", "bool");
    put("ComputerAccessRequestDto.system_key_combos", "bool");
    put("ComputerAccessRequestDto.tcc_state", "Option<TccStateDto>");

    put("RequestedAppDto.label", "String");

    put("AccessTierDto::Read", "read");
    put("AccessTierDto::Click", "click");
    put("AccessTierDto::Full", "full");

    put("TccStateDto.accessibility", "bool");
    put("TccStateDto.screen_recording", "bool");

    put("ComputerAccessResponseDto.granted_apps", "Vec<String>");
    put("ComputerAccessResponseDto.clipboard_read", "bool");
    put("ComputerAccessResponseDto.clipboard_write", "bool");
    put("ComputerAccessResponseDto.system_key_combos", "bool");

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

    // ── Local-apps DTOs (local_apps.rs) ───────────────────────────────────
    put(
        "AppWorkflowStateDto::AuthoringQuestionnaire",
        "authoring_questionnaire",
    );
    put(
        "AppWorkflowStateDto::QuestionnaireFailed",
        "questionnaire_failed",
    );
    put("AppWorkflowStateDto::CollectingSpec", "collecting_spec");
    put("AppWorkflowStateDto::Planning", "planning");
    put("AppWorkflowStateDto::PlanFailed", "plan_failed");
    put(
        "AppWorkflowStateDto::AwaitingSpecConfirmation",
        "awaiting_spec_confirmation",
    );
    put("AppWorkflowStateDto::Generating", "generating");
    put("AppWorkflowStateDto::Validating", "validating");
    put(
        "AppWorkflowStateDto::AwaitingPreviewConfirmation",
        "awaiting_preview_confirmation",
    );
    put("AppWorkflowStateDto::Revising", "revising");
    put("AppWorkflowStateDto::Ready", "ready");
    put("AppWorkflowStateDto::GenerationFailed", "generation_failed");
    put("AppWorkflowStateDto::ValidationFailed", "validation_failed");

    put("AppRuntimeStateDto::Stopped", "stopped");
    put("AppRuntimeStateDto::Starting", "starting");
    put("AppRuntimeStateDto::Running", "running");
    put("AppRuntimeStateDto::Stopping", "stopping");
    put("AppRuntimeStateDto::Failed", "failed");

    put("AppCreateOriginDto::Chat", "chat");
    put("AppCreateOriginDto::Library", "library");

    put("AppErrorCodeDto::NotFound", "not_found");
    put("AppErrorCodeDto::RevisionConflict", "revision_conflict");
    put("AppErrorCodeDto::InteractionInvalid", "interaction_invalid");
    put(
        "AppErrorCodeDto::WorkflowStateInvalid",
        "workflow_state_invalid",
    );
    put("AppErrorCodeDto::RuntimeBusy", "runtime_busy");
    put("AppErrorCodeDto::NotYetAvailable", "not_yet_available");
    put("AppErrorCodeDto::StorageCorrupt", "storage_corrupt");
    put("AppErrorCodeDto::InvalidRequest", "invalid_request");
    put("AppErrorCodeDto::Io", "io");
    put("AppErrorCodeDto::LlmUnavailable", "llm_unavailable");
    put("AppErrorCodeDto::LlmOutputRejected", "llm_output_rejected");

    put("AppCheckpointKindDto::ScaffoldCreated", "scaffold_created");
    put(
        "AppCheckpointKindDto::GenerationValidated",
        "generation_validated",
    );
    put("AppCheckpointKindDto::PreviewApproved", "preview_approved");
    put("AppCheckpointKindDto::UserApproved", "user_approved");
    put("AppCheckpointKindDto::PreRestore", "pre_restore");

    put("DensityLevelDto::Compact", "compact");
    put("DensityLevelDto::Comfortable", "comfortable");

    put("AppDataFieldTypeDto::Text", "text");
    put("AppDataFieldTypeDto::LongText", "long_text");
    put("AppDataFieldTypeDto::Integer", "integer");
    put("AppDataFieldTypeDto::Decimal", "decimal");
    put("AppDataFieldTypeDto::Boolean", "boolean");
    put("AppDataFieldTypeDto::DateTime", "date_time");
    put("AppDataFieldTypeDto::Enum", "enum");
    put("AppDataFieldTypeDto::ImageRef", "image_ref");

    put("AppDataFieldDto.id", "String");
    put("AppDataFieldDto.label", "String");
    put("AppDataFieldDto.field_type", "AppDataFieldTypeDto");
    put("AppDataFieldDto.required", "bool");
    put("AppDataFieldDto.options", "Vec<String>");

    put("AppDataCollectionDto.id", "String");
    put("AppDataCollectionDto.label", "String");
    put("AppDataCollectionDto.fields", "Vec<AppDataFieldDto>");
    put("AppDataCollectionDto.enabled_by_default", "bool");

    put("AppDesignFieldTypeDto::ShortText", "short_text");
    put("AppDesignFieldTypeDto::LongText", "long_text");
    put("AppDesignFieldTypeDto::SingleChoice", "single_choice");
    put("AppDesignFieldTypeDto::MultipleChoice", "multiple_choice");
    put("AppDesignFieldTypeDto::Boolean", "boolean");
    put("AppDesignFieldTypeDto::Color", "color");
    put("AppDesignFieldTypeDto::Density", "density");
    put("AppDesignFieldTypeDto::ScreenList", "screen_list");
    put("AppDesignFieldTypeDto::FeatureList", "feature_list");
    put("AppDesignFieldTypeDto::DataFieldList", "data_field_list");
    put("AppDesignFieldTypeDto::DomainList", "domain_list");

    put("AppDesignFieldOptionDto.value", "String");
    put("AppDesignFieldOptionDto.label", "String");

    put("AppDesignFieldDto.id", "String");
    put("AppDesignFieldDto.label", "String");
    put("AppDesignFieldDto.description", "Option<String>");
    put("AppDesignFieldDto.field_type", "AppDesignFieldTypeDto");
    put("AppDesignFieldDto.required", "bool");
    put("AppDesignFieldDto.allows_custom", "bool");
    put("AppDesignFieldDto.allows_defer", "bool");
    put("AppDesignFieldDto.default_value", "Option<DesignValueDto>");
    put("AppDesignFieldDto.options", "Vec<AppDesignFieldOptionDto>");

    put("AppDesignStepDto.id", "String");
    put("AppDesignStepDto.order", "u32");
    put("AppDesignStepDto.title", "String");
    put("AppDesignStepDto.description", "Option<String>");
    put("AppDesignStepDto.fields", "Vec<AppDesignFieldDto>");

    put("AppPlanDto.collections", "Vec<AppDataCollectionDto>");
    put("AppPlanDto.capabilities", "Vec<AppCapabilityKindDto>");
    put("AppPlanDto.domains", "Vec<String>");
    put("AppPlanDto.summary", "String");

    put("AppRecordDto.id", "String");
    put("AppRecordDto.name", "String");
    put("AppRecordDto.brief", "String");
    put("AppRecordDto.created_at_ms", "u64");
    put("AppRecordDto.updated_at_ms", "u64");
    put("AppRecordDto.workflow_state", "AppWorkflowStateDto");
    put("AppRecordDto.conversation_id", "Option<String>");
    put("AppRecordDto.workspace_rel", "String");

    put("DesignValueDto::ShortText", "short_text");
    put("DesignValueDto::ShortText.value", "String");
    put("DesignValueDto::LongText", "long_text");
    put("DesignValueDto::LongText.value", "String");
    put("DesignValueDto::SingleChoice", "single_choice");
    put("DesignValueDto::SingleChoice.value", "String");
    put("DesignValueDto::MultipleChoice", "multiple_choice");
    put("DesignValueDto::MultipleChoice.value", "Vec<String>");
    put("DesignValueDto::Boolean", "boolean");
    put("DesignValueDto::Boolean.value", "bool");
    put("DesignValueDto::Color", "color");
    put("DesignValueDto::Color.value", "String");
    put("DesignValueDto::Density", "density");
    put("DesignValueDto::Density.value", "DensityLevelDto");
    put("DesignValueDto::ScreenList", "screen_list");
    put("DesignValueDto::ScreenList.value", "Vec<String>");
    put("DesignValueDto::FeatureList", "feature_list");
    put("DesignValueDto::FeatureList.value", "Vec<String>");
    put("DesignValueDto::DataFieldList", "data_field_list");
    put(
        "DesignValueDto::DataFieldList.value",
        "Vec<AppDataFieldDto>",
    );
    put("DesignValueDto::DomainList", "domain_list");
    put("DesignValueDto::DomainList.value", "Vec<String>");
    put("DesignValueDto::Deferred", "deferred");

    put("AppDesignPatchOpDto::Set", "set");
    put("AppDesignPatchOpDto::Set.field_id", "String");
    put("AppDesignPatchOpDto::Set.value", "DesignValueDto");
    put("AppDesignPatchOpDto::Remove", "remove");
    put("AppDesignPatchOpDto::Remove.field_id", "String");

    put("AppDesignPatchDto.ops", "Vec<AppDesignPatchOpDto>");
    put("AppDesignPatchDto.note", "Option<String>");

    put("AppCheckpointDto.id", "String");
    put("AppCheckpointDto.label", "String");
    put("AppCheckpointDto.kind", "AppCheckpointKindDto");
    put("AppCheckpointDto.created_at_ms", "u64");

    put("AppRuntimeModeDto::StaticExport", "static_export");
    put("AppRuntimeModeDto::NextProduction", "next_production");

    put(
        "AppRuntimeSuspensionReasonDto::Backgrounded",
        "backgrounded",
    );
    put(
        "AppRuntimeSuspensionReasonDto::MemoryWarning",
        "memory_warning",
    );
    put(
        "AppRuntimeSuspensionReasonDto::RuntimeQuota",
        "runtime_quota",
    );
    put(
        "AppRuntimeSuspensionReasonDto::ProcessExited",
        "process_exited",
    );

    put("AppRuntimeRecoveryStateDto::NotNeeded", "not_needed");
    put("AppRuntimeRecoveryStateDto::Pending", "pending");
    put("AppRuntimeRecoveryStateDto::Recovering", "recovering");
    put("AppRuntimeRecoveryStateDto::Recovered", "recovered");
    put("AppRuntimeRecoveryStateDto::Failed", "failed");

    put("AppGenerationJobStateDto::Queued", "queued");
    put("AppGenerationJobStateDto::Scaffolding", "scaffolding");
    put("AppGenerationJobStateDto::Generating", "generating");
    put("AppGenerationJobStateDto::Validating", "validating");
    put("AppGenerationJobStateDto::Building", "building");
    put(
        "AppGenerationJobStateDto::StartingPreview",
        "starting_preview",
    );
    put(
        "AppGenerationJobStateDto::AwaitingApproval",
        "awaiting_approval",
    );
    put("AppGenerationJobStateDto::Succeeded", "succeeded");
    put("AppGenerationJobStateDto::Failed", "failed");
    put("AppGenerationJobStateDto::Cancelled", "cancelled");

    put("AppGenerationJobDto.id", "String");
    put("AppGenerationJobDto.app_id", "String");
    put("AppGenerationJobDto.revision", "u64");
    put("AppGenerationJobDto.continuation_seq", "u64");
    put("AppGenerationJobDto.state", "AppGenerationJobStateDto");
    put("AppGenerationJobDto.percent", "Option<u8>");
    put("AppGenerationJobDto.detail", "Option<String>");
    put("AppGenerationJobDto.log_rel", "Option<String>");
    put("AppGenerationJobDto.updated_at_ms", "u64");

    put("AppManifestDto.schema_version", "u32");
    put("AppManifestDto.app_id", "String");
    put("AppManifestDto.name", "String");
    put("AppManifestDto.design_revision", "u64");
    put("AppManifestDto.collections", "Vec<AppDataCollectionDto>");
    put("AppManifestDto.allowed_domains", "Vec<String>");
    put("AppManifestDto.capabilities", "Vec<AppCapabilityKindDto>");

    put("AppRuntimeDetailsDto.state", "AppRuntimeStateDto");
    put("AppRuntimeDetailsDto.mode", "Option<AppRuntimeModeDto>");
    put("AppRuntimeDetailsDto.loopback_url", "Option<String>");
    put(
        "AppRuntimeDetailsDto.suspension_reason",
        "Option<AppRuntimeSuspensionReasonDto>",
    );
    put(
        "AppRuntimeDetailsDto.recovery_state",
        "Option<AppRuntimeRecoveryStateDto>",
    );
    put("AppRuntimeDetailsDto.last_error", "Option<String>");

    put("AppDesignFieldValueDto.field_id", "String");
    put("AppDesignFieldValueDto.value", "DesignValueDto");

    put("AppDetailsDto.app", "AppRecordDto");
    put("AppDetailsDto.design_revision", "u64");
    put("AppDetailsDto.design_fields", "Vec<AppDesignFieldValueDto>");
    put("AppDetailsDto.questionnaire", "Vec<AppDesignStepDto>");
    put("AppDetailsDto.plan", "Option<AppPlanDto>");
    put("AppDetailsDto.manifest", "Option<AppManifestDto>");
    put("AppDetailsDto.runtime", "AppRuntimeDetailsDto");
    put(
        "AppDetailsDto.generation_job",
        "Option<AppGenerationJobDto>",
    );
    put("AppDetailsDto.checkpoints", "Vec<AppCheckpointDto>");

    put("AppBridgeOperationDto::QueryData", "query_data");
    put("AppBridgeOperationDto::MutateData", "mutate_data");
    put("AppBridgeOperationDto::NetworkRequest", "network_request");
    put("AppBridgeOperationDto::RuntimeStatus", "runtime_status");
    put("AppBridgeOperationDto::CapturePhoto", "capture_photo");
    put("AppBridgeOperationDto::PickImage", "pick_image");
    put("AppBridgeOperationDto::RecordAudioStart", "record_audio_start");
    put("AppBridgeOperationDto::RecordAudioStop", "record_audio_stop");
    put("AppBridgeOperationDto::GetLocation", "get_location");
    put("AppBridgeOperationDto::TranscribeSpeech", "transcribe_speech");
    put("AppBridgeOperationDto::PostNotification", "post_notification");
    put("AppBridgeOperationDto::LlmChat", "llm_chat");
    put("AppBridgeOperationDto::AgentPost", "agent_post");

    put("AppBridgeRequestDto.request_id", "String");
    put("AppBridgeRequestDto.app_id", "String");
    put("AppBridgeRequestDto.operation", "AppBridgeOperationDto");
    put("AppBridgeRequestDto.payload_json", "Option<String>");

    put("AppBridgeResponseDto.request_id", "String");
    put("AppBridgeResponseDto.app_id", "String");
    put("AppBridgeResponseDto.ok", "bool");
    put("AppBridgeResponseDto.result_json", "Option<String>");
    put("AppBridgeResponseDto.error", "Option<String>");
    put("AppBridgeResponseDto.error_code", "Option<String>");

    put("AppUiActionKindDto::Inspect", "inspect");
    put("AppUiActionKindDto::Click", "click");
    put("AppUiActionKindDto::Fill", "fill");
    put("AppUiActionKindDto::Select", "select");
    put("AppUiActionKindDto::Toggle", "toggle");
    put("AppUiActionKindDto::Scroll", "scroll");
    put("AppUiActionKindDto::Navigate", "navigate");
    put("AppUiActionKindDto::Back", "back");
    put("AppUiActionKindDto::Reload", "reload");

    put("AppUiTargetDto.element_id", "Option<String>");
    put("AppUiTargetDto.role", "Option<String>");
    put("AppUiTargetDto.name", "Option<String>");

    put("AppUiRequestDto.request_id", "String");
    put("AppUiRequestDto.app_id", "String");
    put("AppUiRequestDto.action", "AppUiActionKindDto");
    put("AppUiRequestDto.target", "Option<AppUiTargetDto>");
    put("AppUiRequestDto.value", "Option<String>");

    put("AppCapabilityKindDto::DataMutation", "data_mutation");
    put("AppCapabilityKindDto::UiControl", "ui_control");
    put("AppCapabilityKindDto::NetworkDomain", "network_domain");
    put(
        "AppCapabilityKindDto::RestoreCheckpoint",
        "restore_checkpoint",
    );
    put("AppCapabilityKindDto::Camera", "camera");
    put("AppCapabilityKindDto::PhotoLibrary", "photo_library");
    put("AppCapabilityKindDto::Microphone", "microphone");
    put("AppCapabilityKindDto::Location", "location");
    put("AppCapabilityKindDto::Notifications", "notifications");
    put("AppCapabilityKindDto::Llm", "llm");
    put("AppCapabilityKindDto::AgentNotify", "agent_notify");

    put("AppCapabilityRequestDto.request_id", "String");
    put("AppCapabilityRequestDto.app_id", "String");
    put("AppCapabilityRequestDto.capability", "AppCapabilityKindDto");
    put("AppCapabilityRequestDto.domain", "Option<String>");
    put("AppCapabilityRequestDto.reason", "String");

    put("AppAuthorizationDecisionDto::Deny", "deny");
    put("AppAuthorizationDecisionDto::AllowOnce", "allow_once");
    put("AppAuthorizationDecisionDto::AllowSession", "allow_session");
    put("AppAuthorizationDecisionDto::AllowAlways", "allow_always");

    put("AppEventDto::AppDetailsChanged", "app_details_changed");
    put("AppEventDto::AppDetailsChanged.details", "AppDetailsDto");
    put(
        "AppEventDto::AppQuestionnaireChanged",
        "app_questionnaire_changed",
    );
    put("AppEventDto::AppQuestionnaireChanged.app_id", "String");
    put("AppEventDto::AppQuestionnaireChanged.revision", "u64");
    put(
        "AppEventDto::AppQuestionnaireChanged.steps",
        "Vec<AppDesignStepDto>",
    );
    put("AppEventDto::AppPlanChanged", "app_plan_changed");
    put("AppEventDto::AppPlanChanged.app_id", "String");
    put("AppEventDto::AppPlanChanged.revision", "u64");
    put("AppEventDto::AppPlanChanged.plan", "Option<AppPlanDto>");
    put(
        "AppEventDto::AppGenerationJobChanged",
        "app_generation_job_changed",
    );
    put(
        "AppEventDto::AppGenerationJobChanged.job",
        "AppGenerationJobDto",
    );
    put("AppEventDto::AppBridgeResponse", "app_bridge_response");
    put(
        "AppEventDto::AppBridgeResponse.response",
        "AppBridgeResponseDto",
    );
    put("AppEventDto::AppUiRequest", "app_ui_request");
    put("AppEventDto::AppUiRequest.request", "AppUiRequestDto");
    put(
        "AppEventDto::AppCapabilityRequested",
        "app_capability_requested",
    );
    put(
        "AppEventDto::AppCapabilityRequested.request",
        "AppCapabilityRequestDto",
    );
    put(
        "AppEventDto::AppCheckpointsChanged",
        "app_checkpoints_changed",
    );
    put("AppEventDto::AppCheckpointsChanged.app_id", "String");
    put(
        "AppEventDto::AppCheckpointsChanged.checkpoints",
        "Vec<AppCheckpointDto>",
    );
    put(
        "AppEventDto::AppLlmActivityChanged",
        "app_llm_activity_changed",
    );
    put("AppEventDto::AppLlmActivityChanged.app_id", "String");
    put("AppEventDto::AppLlmActivityChanged.active", "bool");
    put(
        "AppEventDto::AppAgentEventPosted",
        "app_agent_event_posted",
    );
    put("AppEventDto::AppAgentEventPosted.app_id", "String");
    put("AppEventDto::AppAgentEventPosted.seq", "u64");
    put("AppEventDto::AppAgentEventPosted.topic", "String");
    put("AppEventDto::AppAgentEventPosted.created_at_ms", "u64");

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

/// Regression for the post-Task-2 review finding: the breaking-change
/// threshold must travel with the version, not be a literal pin. A literal
/// `current_major > 1` is true forever once the major has EVER been bumped
/// past 1 — so a SECOND breaking change landing while `CLIENT_PROTOCOL_VERSION`
/// is already e.g. `2.0.0`, with no further bump, must still be REJECTED.
/// `major_was_bumped_past` compares against the major blessed alongside the
/// checked-in index instead, so it keeps gating every subsequent breaking
/// change, not just the first one.
#[test]
fn a_second_breaking_change_with_no_further_bump_is_still_rejected() {
    // The exact failure sequence the review described: a first breaking
    // change bumped 1 -> 2 (blessed_major becomes 2). A SECOND breaking
    // change lands with CLIENT_PROTOCOL_VERSION still at major 2 — a literal
    // `current_major > 1` would wrongly pass (`2 > 1`); the real check must
    // reject it (`2` does not exceed the blessed `2`).
    assert!(
        !major_was_bumped_past(2, 2),
        "a second breaking change must still be rejected when the major did not move again"
    );
    // The properly-bumped case (2 -> 3) must pass.
    assert!(
        major_was_bumped_past(3, 2),
        "a genuine further bump past the blessed major must be accepted"
    );
    // The original (first-ever) bump this guard was written for must still work.
    assert!(
        major_was_bumped_past(2, 1),
        "the original 1 -> 2 bump must still be accepted"
    );
    // A missing/no-op bump at the foundation pin must still be rejected.
    assert!(
        !major_was_bumped_past(1, 1),
        "no bump at all must be rejected"
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
    let current_major = major_of(CLIENT_PROTOCOL_VERSION);

    if bless() {
        write_index(&current);
        // Always in lockstep with the index, not just on a breaking bless:
        // the sidecar's job is to answer "what major was checked in", which
        // must stay true after an additive-only re-bless too.
        write_blessed_major(current_major);
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
            // We need a major bump. THE THRESHOLD TRAVELS WITH THE VERSION:
            // compare against the major that was blessed alongside the
            // CHECKED-IN index (`blessed_major_path()`), never a literal pin
            // — a literal (e.g. `> 1`) is correct only until the FIRST bump
            // ever lands, then stays true forever and stops gating anything
            // (finding: this guard shipped with exactly that bug once
            // `CLIENT_PROTOCOL_VERSION` first became `2.x.x`).
            let blessed_major = read_blessed_major().unwrap_or_else(|| {
                panic!(
                    "missing `{}` (the major recorded at the last bless); regenerate with \
                     `BLESS=1 cargo test -p client-protocol --test version_guard_test`",
                    blessed_major_path().display()
                )
            });
            assert!(
                major_was_bumped_past(current_major, blessed_major),
                "BREAKING contract change detected (a removed / renamed / retyped \
                 entry) but `CLIENT_PROTOCOL_VERSION`'s major ({current_major}, from \
                 {CLIENT_PROTOCOL_VERSION:?}) does not exceed the major blessed alongside \
                 the checked-in contract index ({blessed_major}, from `{}`). Per decision \
                 §0.10 a breaking change REQUIRES a major bump PAST the last blessed one. \
                 Bump the major in `client-protocol/src/version.rs`, then re-bless with \
                 `BLESS=1 cargo test -p client-protocol --test version_guard_test`.",
                blessed_major_path().display()
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
        ProviderCredentialSecretDto,
    };
    use client_protocol::computer_access::{
        AccessTierDto, ComputerAccessRequestDto, ComputerAccessResponseDto, RequestedAppDto,
        TccStateDto,
    };
    use client_protocol::error::ClientError;
    use client_protocol::events::{ClientEvent, CostDto, ErrorKindDto, TurnOutcomeDto};
    use client_protocol::listings::{
        AgentDto, AuthStateDto, CheckStatusDto, CoordinatorWorkerDto, DoctorCheckDto,
        DoctorReportDto, DoctorSummaryDto, HookDto, McpServerDto, McpStatusDto, MemoryEntryDto,
        MemoryTierDto, SessionRowDto, SlashCommandDto, StatusSnapshotDto, TaskRowDto,
        TaskStatusDto,
    };
    use client_protocol::local_apps::{
        AppAuthorizationDecisionDto, AppBridgeOperationDto, AppBridgeRequestDto,
        AppBridgeResponseDto, AppCapabilityKindDto, AppCapabilityRequestDto, AppCheckpointDto,
        AppCheckpointKindDto, AppCreateOriginDto, AppDataCollectionDto, AppDataFieldDto,
        AppDataFieldTypeDto, AppDesignFieldDto, AppDesignFieldOptionDto, AppDesignFieldTypeDto,
        AppDesignFieldValueDto, AppDesignPatchDto, AppDesignPatchOpDto, AppDesignStepDto,
        AppDetailsDto, AppErrorCodeDto, AppEventDto, AppGenerationJobDto, AppGenerationJobStateDto,
        AppManifestDto, AppPlanDto, AppRecordDto, AppRuntimeDetailsDto, AppRuntimeModeDto,
        AppRuntimeRecoveryStateDto, AppRuntimeStateDto, AppRuntimeSuspensionReasonDto,
        AppUiActionKindDto, AppUiRequestDto, AppUiTargetDto,
        AppWorkflowStateDto, DensityLevelDto, DesignValueDto,
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
        ClientEvent::SystemNotice {
            message: String::new(),
            is_error: false,
        },
        ClientEvent::ProviderCredentialStatus {
            operation_id: 0,
            configured_provider_ids: Vec::new(),
            unavailable_provider_ids: Vec::new(),
            storage_encrypted: false,
            error: None,
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
    let _cmd: Vec<ClientCommand> = vec![
        ClientCommand::ListProviderCredentials {
            operation_id: 0,
            provider_ids: Vec::new(),
        },
        ClientCommand::SetProviderCredential {
            operation_id: 0,
            provider_id: String::new(),
            credential: ProviderCredentialSecretDto::new(String::new()),
        },
        ClientCommand::DeleteProviderCredential {
            operation_id: 0,
            provider_id: String::new(),
        },
        ClientCommand::ListModels,
        ClientCommand::RequestExit,
        ClientCommand::ApproveComputerAccess {
            request_id: 0,
            response: ComputerAccessResponseDto::default(),
        },
        ClientCommand::DenyComputerAccess { request_id: 0 },
    ];
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
    let _computer_access = ComputerAccessRequestDto {
        request_id: 0,
        reason: String::new(),
        apps: vec![RequestedAppDto {
            label: String::new(),
        }],
        tier: AccessTierDto::Full,
        clipboard_read: false,
        clipboard_write: false,
        system_key_combos: false,
        tcc_state: Some(TccStateDto {
            accessibility: false,
            screen_recording: false,
        }),
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
    let _apps = (
        AppRecordDto {
            id: String::new(),
            name: String::new(),
            brief: String::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            workflow_state: AppWorkflowStateDto::CollectingSpec,
            conversation_id: None,
            workspace_rel: String::new(),
        },
        AppDesignPatchDto {
            ops: vec![
                AppDesignPatchOpDto::Set {
                    field_id: String::new(),
                    value: DesignValueDto::Density {
                        value: DensityLevelDto::Compact,
                    },
                },
                AppDesignPatchOpDto::Remove {
                    field_id: String::new(),
                },
            ],
            note: None,
        },
        AppCheckpointDto {
            id: String::new(),
            label: String::new(),
            kind: AppCheckpointKindDto::ScaffoldCreated,
            created_at_ms: 0,
        },
        AppRuntimeStateDto::Stopped,
        AppCreateOriginDto::Library,
        AppErrorCodeDto::NotYetAvailable,
        ClientEvent::AppOperationFailed {
            app_id: None,
            code: AppErrorCodeDto::NotFound,
            message: String::new(),
        },
        ClientCommand::ListApps,
    );

    // The local-app surface added after the initial freeze. Every field is
    // named explicitly (no `..Default::default()`, no positional construction)
    // so a RENAMED or REMOVED field breaks THIS compile — which is the whole
    // point of the anchor, and is what `_apps` above already did for the
    // original twelve DTOs.
    let _app_data_field = AppDataFieldDto {
        id: String::new(),
        label: String::new(),
        field_type: AppDataFieldTypeDto::Text,
        required: false,
        options: Vec::new(),
    };
    let _app_data_collection = AppDataCollectionDto {
        id: String::new(),
        label: String::new(),
        fields: Vec::new(),
        enabled_by_default: false,
    };
    let _app_design_field_option = AppDesignFieldOptionDto {
        value: String::new(),
        label: String::new(),
    };
    let _app_design_field = AppDesignFieldDto {
        id: String::new(),
        label: String::new(),
        description: None,
        field_type: AppDesignFieldTypeDto::ShortText,
        required: false,
        allows_custom: false,
        allows_defer: false,
        default_value: None,
        options: Vec::new(),
    };
    let _app_design_step = AppDesignStepDto {
        id: String::new(),
        order: 0,
        title: String::new(),
        description: None,
        fields: Vec::new(),
    };
    let _app_plan = AppPlanDto {
        collections: Vec::new(),
        capabilities: Vec::new(),
        domains: Vec::new(),
        summary: String::new(),
    };
    let app_generation_job = AppGenerationJobDto {
        id: String::new(),
        app_id: String::new(),
        revision: 0,
        continuation_seq: 0,
        state: AppGenerationJobStateDto::Queued,
        percent: None,
        detail: None,
        log_rel: None,
        updated_at_ms: 0,
    };
    let _app_manifest = AppManifestDto {
        schema_version: 0,
        app_id: String::new(),
        name: String::new(),
        design_revision: 0,
        collections: Vec::new(),
        allowed_domains: Vec::new(),
        capabilities: Vec::new(),
    };
    let _app_runtime_details = AppRuntimeDetailsDto {
        state: AppRuntimeStateDto::Stopped,
        mode: Some(AppRuntimeModeDto::StaticExport),
        loopback_url: None,
        suspension_reason: Some(AppRuntimeSuspensionReasonDto::Backgrounded),
        recovery_state: Some(AppRuntimeRecoveryStateDto::NotNeeded),
        last_error: None,
    };
    let _app_design_field_value = AppDesignFieldValueDto {
        field_id: String::new(),
        value: DesignValueDto::Boolean { value: false },
    };
    let app_details = AppDetailsDto {
        app: AppRecordDto {
            id: String::new(),
            name: String::new(),
            brief: String::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            workflow_state: AppWorkflowStateDto::CollectingSpec,
            conversation_id: None,
            workspace_rel: String::new(),
        },
        design_revision: 0,
        design_fields: Vec::new(),
        questionnaire: Vec::new(),
        plan: None,
        manifest: None,
        runtime: AppRuntimeDetailsDto {
            state: AppRuntimeStateDto::Stopped,
            mode: None,
            loopback_url: None,
            suspension_reason: None,
            recovery_state: None,
            last_error: None,
        },
        generation_job: None,
        checkpoints: Vec::new(),
    };
    let _app_bridge_request = AppBridgeRequestDto {
        request_id: String::new(),
        app_id: String::new(),
        operation: AppBridgeOperationDto::QueryData,
        payload_json: None,
    };
    let app_bridge_response = AppBridgeResponseDto {
        request_id: String::new(),
        app_id: String::new(),
        ok: false,
        result_json: None,
        error: None,
        error_code: None,
    };
    let _app_ui_target = AppUiTargetDto {
        element_id: None,
        role: None,
        name: None,
    };
    let app_ui_request = AppUiRequestDto {
        request_id: String::new(),
        app_id: String::new(),
        action: AppUiActionKindDto::Inspect,
        target: None,
        value: None,
    };
    let app_capability_request = AppCapabilityRequestDto {
        request_id: String::new(),
        app_id: String::new(),
        capability: AppCapabilityKindDto::DataMutation,
        domain: None,
        reason: String::new(),
    };
    let _app_authorization_decision = AppAuthorizationDecisionDto::AllowOnce;
    // One value per `AppEventDto` variant: the envelope is a single
    // `ClientEvent::AppEvent`, so nothing else forces these ten tags to
    // exist.
    let _app_events: Vec<AppEventDto> = vec![
        AppEventDto::AppDetailsChanged {
            details: app_details,
        },
        AppEventDto::AppQuestionnaireChanged {
            app_id: String::new(),
            revision: 0,
            steps: Vec::new(),
        },
        AppEventDto::AppPlanChanged {
            app_id: String::new(),
            revision: 0,
            plan: None,
        },
        AppEventDto::AppGenerationJobChanged {
            job: app_generation_job,
        },
        AppEventDto::AppBridgeResponse {
            response: app_bridge_response,
        },
        AppEventDto::AppUiRequest {
            request: app_ui_request,
        },
        AppEventDto::AppCapabilityRequested {
            request: app_capability_request,
        },
        AppEventDto::AppCheckpointsChanged {
            app_id: String::new(),
            checkpoints: Vec::new(),
        },
        AppEventDto::AppLlmActivityChanged {
            app_id: String::new(),
            active: false,
        },
        AppEventDto::AppAgentEventPosted {
            app_id: String::new(),
            seq: 0,
            topic: String::new(),
            created_at_ms: 0,
        },
    ];


    // Sanity: the index is non-empty and contains a known anchor key.
    let ix = current_contract_index();
    assert!(
        ix.contains_key("ClientEvent::TextDelta.text"),
        "the contract index must enumerate the contract leaves"
    );
}

/// `DesignValueDto::Deferred` carries no payload — unlike every other
/// `DesignValueDto` variant, its wire form is the bare tag alone
/// (`{ "kind": "deferred" }`, no `value` key). Pins that shape so a future
/// change cannot silently attach a payload to the "let the model decide"
/// sentinel.
#[test]
fn deferred_design_value_serialises_as_a_bare_tagged_variant() {
    let json = serde_json::to_value(client_protocol::local_apps::DesignValueDto::Deferred)
        .expect("serialise");
    assert_eq!(
        json,
        serde_json::json!({ "kind": "deferred" }),
        "Deferred carries no payload; the tag alone must round-trip"
    );
    let back: client_protocol::local_apps::DesignValueDto =
        serde_json::from_value(json).expect("deserialise");
    assert_eq!(back, client_protocol::local_apps::DesignValueDto::Deferred);
}
