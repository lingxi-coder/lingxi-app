/**
 * LingXi bridge wire-protocol types — a 1:1 TypeScript mirror of the Rust
 * serde contract (the SOURCE OF TRUTH).
 *
 * Mirrored from:
 *  - `client-protocol/src/commands.rs`   → {@link ClientCommand} + payload DTOs
 *  - `client-protocol/src/events.rs`     → {@link ClientEvent} + cost/outcome DTOs
 *  - `client-protocol/src/message.rs`    → {@link MessageDto} / {@link MessageBlockDto}
 *  - `client-protocol/src/permission.rs` → {@link PermissionRequest} + response DTOs
 *  - `client-protocol/src/listings.rs`   → the screen-listing row/payload DTOs
 *  - `client-protocol/src/local_apps.rs` → the local-apps DTOs ({@link AppRecordDto}, …)
 *  - `client-protocol/src/error.rs`      → {@link ClientError}
 *  - `client-protocol/src/version.rs`    → {@link CLIENT_PROTOCOL_VERSION}
 *  - `bridge/src/wire.rs`                → {@link Frame}, {@link ClientHello},
 *                                          {@link ServerHello}, {@link Capabilities},
 *                                          {@link BRIDGE_PROTOCOL_VERSION}
 *
 * Serde conventions encoded here (governing decision §0.1):
 *  - enums are INTERNALLY tagged on a `type` field, `snake_case` variant names;
 *  - the {@link Frame} envelope is ADJACENTLY tagged (`type` + `payload`);
 *  - optional fields are `skip_serializing_if = "Option::is_none"` on the Rust
 *    side, so they may be absent on the wire — modeled as `field?: T`.
 *
 * Tool payloads cross the wire as JSON **Strings** (`input_json`/`result_json`/
 * `tool_input_json`), never as parsed objects (decision §0.4); they are kept as
 * `string` here.
 */

// ─────────────────────────────────────────────────────────────────────────────
// Version constants (bridge/src/wire.rs, client-protocol/src/version.rs)
// ─────────────────────────────────────────────────────────────────────────────

/** Bridge wire-envelope (framing/handshake) version this SDK speaks. */
export const BRIDGE_PROTOCOL_VERSION = '0.2.0';

/** `client-protocol` DTO contract version this SDK speaks. */
export const CLIENT_PROTOCOL_VERSION = '1.0.0';

// ─────────────────────────────────────────────────────────────────────────────
// commands.rs
// ─────────────────────────────────────────────────────────────────────────────

/** Prompt-input mode for {@link SendPrompt} (commands.rs `PromptModeDto`). */
export type PromptModeDto =
  | { type: 'normal' }
  | { type: 'bash' }
  | { type: 'memory' }
  | { type: 'plan' };

/** Inline image attachment for {@link SendPrompt} (commands.rs `ImageRefDto`). */
export interface ImageRefDto {
  media_type: string;
  base64: string;
}

/** One declared answer choice for an interactive questionnaire. */
export interface AskOptionDto {
  label: string;
  description: string;
  preview?: string;
}

/** One question in an interactive `AskUserQuestion` request. */
export interface AskQuestionDto {
  question: string;
  header: string;
  options: AskOptionDto[];
  multi_select: boolean;
}

/** A connection-scoped interactive questionnaire emitted by the engine. */
export interface AskUserQuestionRequestDto {
  request_id: number;
  questions: AskQuestionDto[];
  timeout_secs?: number;
}

/** Reply for {@link RunSlashCommand} (commands.rs `CommandResultDto`). */
export interface CommandResultDto {
  display: string;
  injected?: string;
}

/** Which screen listing to (re)pull (commands.rs `ListingKindDto`). */
export type ListingKindDto =
  | { type: 'sessions' }
  | { type: 'models' }
  | { type: 'mcp' }
  | { type: 'hooks' }
  | { type: 'agents' }
  | { type: 'slash_commands' }
  | { type: 'memory' }
  | { type: 'status' }
  | { type: 'settings' }
  | { type: 'auth' }
  | { type: 'doctor' }
  | { type: 'tasks' };

/**
 * The inbound command envelope a client sends to the engine
 * (commands.rs `ClientCommand`). Internally tagged on `type`, `snake_case`.
 *
 * `#[non_exhaustive]` on the Rust side ⇒ a future variant is additive; consumers
 * should treat the union as open-ended.
 */
export type ClientCommand =
  // ── Turn driving ──────────────────────────────────────────────────────────
  | {
      type: 'send_prompt';
      text: string;
      prompt_mode?: PromptModeDto;
      images: ImageRefDto[];
      turn_id?: number;
    }
  | { type: 'cancel'; turn_id?: number }
  // ── Permission resolution ───────────────────────────────────────────────────
  | { type: 'approve_permission'; request_id: number; response: PermissionResponseDto }
  | { type: 'deny_permission'; request_id: number }
  | { type: 'set_permission_mode'; mode: PermissionModeId }
  // ── `computer` tool request_access resolution ───────────────────────────────
  | { type: 'approve_computer_access'; request_id: number; response: ComputerAccessResponseDto }
  | { type: 'deny_computer_access'; request_id: number }
  // ── `AskUserQuestion` resolution ───────────────────────────────────────────
  | { type: 'answer_ask_user_question'; request_id: number; answers: Record<string, string> }
  | { type: 'cancel_ask_user_question'; request_id: number }
  // ── Provider credentials (authenticated local bridge only) ──────────────────
  | { type: 'list_provider_credentials'; operation_id: number; provider_ids: string[] }
  | { type: 'set_provider_credential'; operation_id: number; provider_id: string; credential: string }
  | { type: 'delete_provider_credential'; operation_id: number; provider_id: string }
  // ── Model ─────────────────────────────────────────────────────────────────
  | { type: 'set_model'; model: string }
  | { type: 'list_models' }
  // ── Slash commands ──────────────────────────────────────────────────────────
  | { type: 'run_slash_command'; raw: string }
  // ── Listings ────────────────────────────────────────────────────────────────
  | { type: 'refresh_listings'; which: ListingKindDto[] }
  // ── Session lifecycle (decision §0.5) ───────────────────────────────────────
  | { type: 'new_session'; cwd?: string; model?: string }
  | { type: 'resume_session'; session_id: string; cwd?: string }
  | { type: 'list_sessions'; limit?: number }
  // ── Auth + session control ────────────────────────────────────────────────
  | { type: 'login' }
  | { type: 'logout' }
  | { type: 'force_compact' }
  | { type: 'clear_session' }
  // ── Tasks ───────────────────────────────────────────────────────────────────
  | { type: 'task_list'; status_filter?: TaskStatusDto }
  | { type: 'task_output'; task_id: string; offset: number }
  | { type: 'task_stop'; task_id: string }
  // ── Local apps ──────────────────────────────────────────────────────────────
  | { type: 'list_apps' }
  | {
      type: 'create_app';
      name: string;
      template: AppTemplateKindDto;
      origin: AppCreateOriginDto;
      conversation_id?: string;
    }
  | { type: 'open_app_designer'; app_id: string }
  | {
      type: 'update_app_design_draft';
      app_id: string;
      expected_revision: number;
      patch: AppDesignPatchDto;
    }
  | {
      type: 'apply_agent_design_suggestion';
      app_id: string;
      suggestion_id: string;
      expected_revision: number;
    }
  | { type: 'confirm_app_design'; app_id: string; revision: number; interaction_id: string }
  | { type: 'cancel_app_design'; app_id: string }
  | { type: 'start_app'; app_id: string }
  | { type: 'stop_app'; app_id: string }
  | { type: 'restart_app'; app_id: string }
  | { type: 'confirm_app_preview'; app_id: string; revision: number; interaction_id: string }
  | { type: 'request_app_revision'; app_id: string; prompt: string }
  | { type: 'list_app_checkpoints'; app_id: string }
  | { type: 'restore_app_checkpoint'; app_id: string; checkpoint_id: string }
  | { type: 'delete_app'; app_id: string }
  // ── Lifecycle ───────────────────────────────────────────────────────────────
  | { type: 'request_exit' };

// ─────────────────────────────────────────────────────────────────────────────
// message.rs
// ─────────────────────────────────────────────────────────────────────────────

/** One block within a {@link MessageDto} (message.rs `MessageBlockDto`). */
export type MessageBlockDto =
  | { type: 'text'; text: string }
  | { type: 'thinking'; thinking: string; signature?: string }
  | { type: 'redacted_thinking'; data: string }
  | {
      type: 'compact_boundary';
      messages_before: number;
      messages_after: number;
      summary: string;
    }
  | { type: 'tool_use'; id: string; tool: string; input_json: string }
  | {
      type: 'tool_result';
      id: string;
      tool: string;
      result_json: string;
      is_error: boolean;
      old_string?: string;
      new_string?: string;
      file_path?: string;
    };

/** A complete conversation message (message.rs `MessageDto`). */
export interface MessageDto {
  role: string;
  blocks: MessageBlockDto[];
}

// ─────────────────────────────────────────────────────────────────────────────
// permission.rs
// ─────────────────────────────────────────────────────────────────────────────

/** What the user is being asked to approve (permission.rs `PermissionKindDto`). */
export type PermissionKindDto =
  | { type: 'tool_use_confirm'; tool_name: string; tool_input_json: string; default_allow: boolean }
  | { type: 'exit_plan_mode'; plan: string }
  | { type: 'bypass_permissions_mode' };

/** Worker identity carried on a {@link PermissionRequest} (permission.rs `WorkerInfoDto`). */
export interface WorkerInfoDto {
  name: string;
  color: string;
  team?: string;
}

/** Outbound permission request (permission.rs `PermissionRequest`). */
export interface PermissionRequest {
  request_id: number;
  kind: PermissionKindDto;
  worker?: WorkerInfoDto;
}

/** The user's decision for a permission request (permission.rs `PermissionResponseDto`). */
export type PermissionResponseDto =
  | { type: 'allow_once' }
  | { type: 'allow_always' }
  | { type: 'deny' };

/** Live session permission modes accepted by the engine. */
export type PermissionModeId =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'auto'
  | 'dontAsk'
  | 'bypassPermissions';

/** Inbound resolution of a {@link PermissionRequest} (permission.rs `PermissionResolved`). */
export interface PermissionResolved {
  request_id: number;
  response: PermissionResponseDto;
}

// ─────────────────────────────────────────────────────────────────────────────
// computer-access bridge — the `computer` tool's `request_access` prompt DTOs.
//
// Mirrors `tui-core/src/computer_access_bridge.rs` (`AccessTier`, `TccState`,
// `RequestedApp`, `ComputerAccessRequest`, `ComputerAccessResponse`) at the
// wire boundary the client-protocol crate adds alongside `permission.rs`.
// Chosen over reusing {@link PermissionKindDto} for the same reason as the
// Rust side: a title+message+options-list prompt can't express per-app
// checkboxes, a tier, three independent capability flags, or a TCC
// missing-permissions panel.
// ─────────────────────────────────────────────────────────────────────────────

/** One requested application, pre-checked in the app-allowlist panel (`RequestedAppDto`). */
export interface RequestedAppDto {
  label: string;
}

/**
 * The per-app capability level `request_access` can grant — mirrors
 * `computer_access_bridge::AccessTier::as_str()` exactly (`"read"` / `"click"` /
 * `"full"`).
 */
export type AccessTierDto = 'read' | 'click' | 'full';

/** Which macOS TCC permissions are missing (`TccStateDto`, mirrors `TccState`). */
export interface TccStateDto {
  accessibility: boolean;
  screen_recording: boolean;
}

/**
 * Outbound `computer` tool `request_access` prompt (`ComputerAccessRequestDto`).
 * When {@link tcc_state} is present, the client shows the TCC panel instead of
 * the app-allowlist panel (a required macOS permission is missing).
 */
export interface ComputerAccessRequestDto {
  request_id: number;
  reason: string;
  apps: RequestedAppDto[];
  tier: AccessTierDto;
  clipboard_read: boolean;
  clipboard_write: boolean;
  system_key_combos: boolean;
  tcc_state?: TccStateDto;
}

/**
 * The user's resolution (`ComputerAccessResponseDto`). An empty `granted_apps`
 * with every flag `false` means "denied" — there is no separate boolean,
 * matching the Rust `Default` (also what Esc/deny sends).
 */
export interface ComputerAccessResponseDto {
  granted_apps: string[];
  clipboard_read: boolean;
  clipboard_write: boolean;
  system_key_combos: boolean;
}

// ─────────────────────────────────────────────────────────────────────────────
// listings.rs
// ─────────────────────────────────────────────────────────────────────────────

/** One resumable-session row (listings.rs `SessionRowDto`). */
export interface SessionRowDto {
  uuid: string;
  title: string;
  modified_rfc3339: string;
  message_count: number;
  path: string;
}

/** Connection status for an MCP server (listings.rs `McpStatusDto`). */
export type McpStatusDto =
  | { type: 'connected' }
  | { type: 'disconnected' }
  | { type: 'error'; reason: string };

/** One MCP server entry (listings.rs `McpServerDto`). */
export interface McpServerDto {
  name: string;
  status: McpStatusDto;
  transport: string;
}

/** One hook entry (listings.rs `HookDto`). */
export interface HookDto {
  name: string;
  event: string;
  matcher?: string;
  timeout_ms: number;
}

/** One subagent entry (listings.rs `AgentDto`). */
export interface AgentDto {
  name: string;
  description: string;
  tools_allowed: string[];
}

/** One slash-command catalog entry (listings.rs `SlashCommandDto`). */
export interface SlashCommandDto {
  name: string;
  description: string;
  source: string;
}

/** Memory tier (listings.rs `MemoryTierDto`). */
export type MemoryTierDto =
  | { type: 'session' }
  | { type: 'project' }
  | { type: 'team' }
  | { type: 'user' };

/** One LINGXI.md memory entry (listings.rs `MemoryEntryDto`). */
export interface MemoryEntryDto {
  path: string;
  tier: MemoryTierDto;
  body: string;
  age_days: number;
  size_bytes: number;
}

/** The `/status` panel snapshot (listings.rs `StatusSnapshotDto`). */
export interface StatusSnapshotDto {
  session_id: string;
  model: string;
  n_messages: number;
  total_cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  n_mcp_connected: number;
  n_mcp_total: number;
  n_hooks: number;
  n_agents: number;
  started_at: string;
  cwd: string;
  status_line?: string;
}

/** Auth state (listings.rs `AuthStateDto`). */
export type AuthStateDto =
  | { type: 'signed_out' }
  | { type: 'signed_in'; email: string; org_id: string };

/** Outcome of a single doctor check (listings.rs `CheckStatusDto`). */
export type CheckStatusDto = { type: 'pass' } | { type: 'warn' } | { type: 'fail' };

/** One `/doctor` check result (listings.rs `DoctorCheckDto`). */
export interface DoctorCheckDto {
  name: string;
  status: CheckStatusDto;
  detail?: string;
}

/** Pass/warn/fail tallies (listings.rs `DoctorSummaryDto`). */
export interface DoctorSummaryDto {
  passed: number;
  warnings: number;
  failed: number;
}

/** Aggregate diagnostic report (listings.rs `DoctorReportDto`). */
export interface DoctorReportDto {
  checks: DoctorCheckDto[];
  summary: DoctorSummaryDto;
}

/** Task status (listings.rs `TaskStatusDto`). */
export type TaskStatusDto =
  | { type: 'pending' }
  | { type: 'running' }
  | { type: 'completed' }
  | { type: 'failed' }
  | { type: 'cancelled' };

/** One task row (listings.rs `TaskRowDto`). */
export interface TaskRowDto {
  task_id: string;
  task_type: string;
  status: TaskStatusDto;
  description: string;
}

// ─────────────────────────────────────────────────────────────────────────────
// local_apps.rs
//
// The fieldless enums ride as BARE wire strings (byte-identical to the
// local-apps core enums' canonical values — the `AccessTierDto` precedent);
// `DesignValueDto` is tagged on `kind` and `AppDesignPatchOpDto` on `op`, the
// discriminators the local-apps spec fixes.
// ─────────────────────────────────────────────────────────────────────────────

/** Scaffold template an app is designed from (local_apps.rs `AppTemplateKindDto`). */
export type AppTemplateKindDto =
  | 'dashboard'
  | 'crud_tracker'
  | 'content_showcase'
  | 'form_utility';

/** Designer/generation workflow state (local_apps.rs `AppWorkflowStateDto`). */
export type AppWorkflowStateDto =
  | 'collecting_spec'
  | 'awaiting_spec_confirmation'
  | 'generating'
  | 'validating'
  | 'awaiting_preview_confirmation'
  | 'revising'
  | 'ready'
  | 'generation_failed'
  | 'validation_failed';

/** Runtime (dev-server) state (local_apps.rs `AppRuntimeStateDto`). */
export type AppRuntimeStateDto = 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';

/** Where a `create_app` originated (local_apps.rs `AppCreateOriginDto`). */
export type AppCreateOriginDto = 'chat' | 'library';

/** Typed local-app failure code carried by `app_operation_failed` (local_apps.rs `AppErrorCodeDto`). */
export type AppErrorCodeDto =
  | 'not_found'
  | 'revision_conflict'
  | 'interaction_invalid'
  | 'workflow_state_invalid'
  | 'runtime_busy'
  | 'not_yet_available'
  | 'storage_corrupt'
  | 'invalid_request'
  | 'io';

/** Why a checkpoint was recorded (local_apps.rs `AppCheckpointKindDto`). */
export type AppCheckpointKindDto =
  | 'scaffold_created'
  | 'generation_validated'
  | 'preview_approved'
  | 'user_approved'
  | 'pre_restore';

/** Layout density for a `density` design value (local_apps.rs `DensityLevelDto`). */
export type DensityLevelDto = 'compact' | 'comfortable';

/** One draft field value, tagged by field kind (local_apps.rs `DesignValueDto`). */
export type DesignValueDto =
  | { kind: 'short_text'; value: string }
  | { kind: 'long_text'; value: string }
  | { kind: 'single_choice'; value: string }
  | { kind: 'multiple_choice'; value: string[] }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'color'; value: string }
  | { kind: 'density'; value: DensityLevelDto }
  | { kind: 'screen_list'; value: string[] }
  | { kind: 'feature_list'; value: string[] };

/** One patch operation against the draft field map (local_apps.rs `AppDesignPatchOpDto`). */
export type AppDesignPatchOpDto =
  | { op: 'set'; field_id: string; value: DesignValueDto }
  | { op: 'remove'; field_id: string };

/** An ordered batch of draft edits (local_apps.rs `AppDesignPatchDto`). */
export interface AppDesignPatchDto {
  ops: AppDesignPatchOpDto[];
  note?: string;
}

/** One local-app row (local_apps.rs `AppRecordDto`). */
export interface AppRecordDto {
  id: string;
  name: string;
  template: AppTemplateKindDto;
  created_at_ms: number;
  updated_at_ms: number;
  workflow_state: AppWorkflowStateDto;
  conversation_id?: string;
  workspace_rel: string;
}

/** One restorable app checkpoint (local_apps.rs `AppCheckpointDto`). */
export interface AppCheckpointDto {
  id: string;
  label: string;
  kind: AppCheckpointKindDto;
  created_at_ms: number;
}

// ─────────────────────────────────────────────────────────────────────────────
// events.rs
// ─────────────────────────────────────────────────────────────────────────────

/** Coarse error class carried by {@link ClientEvent} `error` (events.rs `ErrorKindDto`). */
export type ErrorKindDto =
  | { type: 'transport' }
  | { type: 'protocol' }
  | { type: 'server' }
  | { type: 'max_turns' }
  | { type: 'rejected' }
  | { type: 'internal' };

/** How a turn ended (events.rs `TurnOutcomeDto`). */
export type TurnOutcomeDto =
  | { type: 'end_turn' }
  | { type: 'max_turns' }
  | { type: 'cancelled' };

/** Cumulative cost snapshot carried by `turn_ended` (events.rs `CostDto`). */
export interface CostDto {
  total_usd: number;
  input_tokens: number;
  output_tokens: number;
  api_calls: number;
  session_duration_secs: number;
  formatted: string;
}

/**
 * Outbound events the engine streams to a client (events.rs `ClientEvent`).
 * Internally tagged on `type`, `snake_case`.
 *
 * `#[non_exhaustive]` on the Rust side ⇒ a future variant is additive.
 */
export type ClientEvent =
  // ── Error ─────────────────────────────────────────────────────────────────
  | { type: 'error'; kind: ErrorKindDto; message: string }
  | { type: 'system_notice'; message: string; is_error: boolean }
  | { type: 'ask_user_question'; request: AskUserQuestionRequestDto }
  | { type: 'ask_user_question_resolved'; request_id: number }
  // ── Live-turn streaming events ──────────────────────────────────────────────
  | { type: 'text_delta'; text: string }
  | { type: 'tool_use_started'; id: string; tool: string; input_json: string }
  | { type: 'tool_heartbeat'; id: string; tool: string; elapsed_ms: number }
  | { type: 'tool_use_result'; id: string; tool: string; result_json: string; is_error: boolean }
  | { type: 'message_complete'; stop_reason?: string; message?: MessageDto }
  | { type: 'turn_started'; turn_id?: number }
  | { type: 'turn_ended'; outcome: TurnOutcomeDto; stop_reason?: string; cost: CostDto }
  | {
      type: 'cost_update';
      total_usd: number;
      input_tokens: number;
      output_tokens: number;
      api_calls: number;
      session_duration_secs: number;
      formatted: string;
    }
  | {
      type: 'compaction_completed';
      messages_before: number;
      messages_after: number;
      bytes_saved: number;
    }
  // ── Session lifecycle ───────────────────────────────────────────────────────
  | { type: 'session_started'; session_id: string }
  | { type: 'session_ended' }
  | { type: 'session_resumed'; session_id: string; messages: MessageDto[] }
  | { type: 'session_list'; sessions: SessionRowDto[] }
  // ── Listing / screen events ─────────────────────────────────────────────────
  | { type: 'model_list'; models: string[]; current: string }
  | { type: 'model_changed'; model: string }
  | { type: 'permission_mode_changed'; mode: PermissionModeId }
  | {
      type: 'provider_credential_status';
      operation_id: number;
      configured_provider_ids: string[];
      unavailable_provider_ids?: string[];
      storage_encrypted: boolean;
      error?: string;
    }
  | { type: 'mcp_servers'; servers: McpServerDto[] }
  | { type: 'hooks'; hooks: HookDto[] }
  | { type: 'agents'; agents: AgentDto[] }
  | { type: 'slash_command_catalog'; commands: SlashCommandDto[] }
  | { type: 'memory_entries'; entries: MemoryEntryDto[] }
  | { type: 'status_snapshot'; snapshot: StatusSnapshotDto }
  | { type: 'settings_snapshot'; effective_json: string; provenance_json: string }
  | { type: 'auth_state'; state: AuthStateDto }
  | { type: 'doctor_report'; report: DoctorReportDto }
  | { type: 'task_row'; task: TaskRowDto }
  | {
      type: 'task_output_chunk';
      task_id: string;
      content: string;
      total_lines: number;
      truncated: boolean;
    }
  | { type: 'task_status_changed'; task_id: string; status: TaskStatusDto }
  | { type: 'commands_changed'; commands: SlashCommandDto[] }
  // ── Local apps ──────────────────────────────────────────────────────────────
  | { type: 'apps_changed'; apps: AppRecordDto[] }
  | { type: 'app_designer_requested'; app_id: string; interaction_id: string; revision: number }
  | {
      type: 'app_design_draft_changed';
      app_id: string;
      revision: number;
      fields: Record<string, DesignValueDto>;
    }
  | {
      type: 'app_design_suggestion_available';
      app_id: string;
      suggestion_id: string;
      based_on_revision: number;
      patch: AppDesignPatchDto;
    }
  | {
      type: 'app_design_conflict';
      app_id: string;
      expected_revision: number;
      actual_revision: number;
    }
  | { type: 'app_workflow_changed'; app_id: string; state: AppWorkflowStateDto; detail?: string }
  | {
      type: 'app_generation_progress';
      app_id: string;
      stage: string;
      percent?: number;
      detail?: string;
    }
  | { type: 'app_runtime_changed'; app_id: string; state: AppRuntimeStateDto; last_error?: string }
  | {
      type: 'app_preview_ready';
      app_id: string;
      interaction_id: string;
      revision: number;
      url?: string;
    }
  | { type: 'app_checkpoint_created'; app_id: string; checkpoint: AppCheckpointDto }
  | { type: 'app_operation_failed'; app_id?: string; code: AppErrorCodeDto; message: string }
  // ── Reserved / feed-deferred (round-trip only) ──────────────────────────────
  | { type: 'coordinator_status'; active_workers: number; team?: string }
  | {
      type: 'coordinator_worker';
      worker: { agent_id: string; name: string; agent_type: string; status: string };
    }
  | { type: 'thinking_delta'; thinking: string; signature?: string }
  | {
      type: 'usage_update';
      input_tokens: number;
      output_tokens: number;
      cache_read_tokens: number;
      cache_creation_tokens: number;
    }
  | {
      type: 'api_retry';
      message: string;
      attempt: number;
      max_retries: number;
      delay_ms: number;
    };

// ─────────────────────────────────────────────────────────────────────────────
// error.rs
// ─────────────────────────────────────────────────────────────────────────────

/** A flat, FFI-ready call-result error (error.rs `ClientError`). */
export type ClientError =
  | { type: 'transport'; message: string }
  | { type: 'protocol'; message: string }
  | { type: 'rejected'; message: string }
  | { type: 'not_found'; message: string }
  | { type: 'internal'; message: string };

// ─────────────────────────────────────────────────────────────────────────────
// wire.rs — handshake + framing envelope
// ─────────────────────────────────────────────────────────────────────────────

/** Capability flags advertised in the handshake (wire.rs `Capabilities`). */
export interface Capabilities {
  supports_streaming: boolean;
  supports_tools: boolean;
  supports_skills: boolean;
  supports_commands: boolean;
  client_protocol_version: string;
}

/** Client → server opening handshake (wire.rs `ClientHello`). */
export interface ClientHello {
  protocol_version: string;
  client_name: string;
  capabilities: Capabilities;
}

/** Server → client handshake reply (wire.rs `ServerHello`). */
export interface ServerHello {
  protocol_version: string;
  server_name: string;
  capabilities: Capabilities;
}

/** Wire-level error inside a {@link BridgeResponse} (wire.rs `BridgeWireError`). */
export interface BridgeWireError {
  code: number;
  message: string;
}

/**
 * A framed JSON-RPC-style request from client to server (wire.rs `BridgeRequest`).
 * `params` carries a {@link ClientCommand} (or a {@link ClientHello} when
 * `method === "hello"`).
 */
export interface BridgeRequest {
  id: number;
  method: string;
  params: unknown;
}

/** A framed reply to a {@link BridgeRequest} (wire.rs `BridgeResponse`). */
export interface BridgeResponse {
  id: number;
  result?: unknown | null;
  error?: BridgeWireError | null;
}

/**
 * The post-handshake frame carried over the WebSocket text channel
 * (wire.rs `Frame`). ADJACENTLY tagged on `type` + `payload` (`snake_case`).
 */
export type Frame =
  | { type: 'request'; payload: BridgeRequest }
  | { type: 'response'; payload: BridgeResponse }
  | { type: 'event'; payload: ClientEvent }
  | { type: 'permission_request'; payload: PermissionRequest }
  | { type: 'computer_access_request'; payload: ComputerAccessRequestDto };
