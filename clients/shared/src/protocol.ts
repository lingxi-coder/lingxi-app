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
export const CLIENT_PROTOCOL_VERSION = '6.0.0';

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
  | { type: 'get_conversation_controls' }
  | { type: 'set_reasoning_selection'; selection: ReasoningSelectionDto }
  // ── Slash commands ──────────────────────────────────────────────────────────
  | { type: 'run_slash_command'; raw: string; turn_id?: number }
  // ── Listings ────────────────────────────────────────────────────────────────
  | { type: 'refresh_listings'; which: ListingKindDto[] }
  | { type: 'list_session_agents' }
  | { type: 'load_session_agent_transcript'; agent_id: string }
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
  | { type: 'resume_workflow'; task_id: string }
  // ── Local apps ──────────────────────────────────────────────────────────────
  | { type: 'list_apps' }
  | { type: 'get_app_details'; app_id: string }
  | {
      type: 'create_app';
      name: string;
      origin: AppCreateOriginDto;
      brief: string;
      /** Git-backed source versioning choice; the engine defaults to enabled when omitted. */
      git_enabled?: boolean;
      /** Provider-qualified model used by the app creation workflow. */
      workflow_model?: string;
      conversation_id?: string;
    }
  | { type: 'start_app'; app_id: string }
  | { type: 'stop_app'; app_id: string }
  | { type: 'restart_app'; app_id: string }
  | { type: 'execute_app_bridge_request'; request: AppBridgeRequestDto }
  | {
      type: 'resolve_app_ui_request';
      request_id: string;
      decision: AppAuthorizationDecisionDto;
      result_json?: string;
      error?: string;
    }
  | {
      type: 'resolve_app_capability_request';
      request_id: string;
      decision: AppAuthorizationDecisionDto;
    }
  | {
      type: 'resolve_app_profile_proposal';
      app_id: string;
      approval_token: string;
      approved: boolean;
    }
  | { type: 'reset_app_permissions'; app_id: string }
  | { type: 'list_app_sessions'; app_id: string; offset?: number; limit?: number }
  | { type: 'list_app_checkpoints'; app_id: string }
  | { type: 'restore_app_checkpoint'; app_id: string; checkpoint_id: string }
  | { type: 'delete_app'; app_id: string }
  // ── Lifecycle ───────────────────────────────────────────────────────────────
  | { type: 'request_exit' };

// ─────────────────────────────────────────────────────────────────────────────
// tool_display.rs — the pre-derived render model for one tool call
// ─────────────────────────────────────────────────────────────────────────────
//
// Derived ONCE in Rust and shipped to every client, so the terminal, iOS,
// Android, and this desktop app cannot drift apart on how a tool call reads.
//
// Two shape decisions matter when consuming these:
//
//  1. Diff rows ship PRE-SPLIT segments, never string offsets. Rust indexes
//     strings by UTF-8 byte and JS by UTF-16 code unit, so a `(start, end)`
//     pair would silently mis-slice any non-ASCII line — and this repo's own
//     sources are full of CJK. Concatenating a row's `segments[].text`
//     reproduces the line exactly.
//  2. `CodeSegmentDto.class` is a semantic token class; `rgb` is the terminal's
//     resolved color, baked against ONE dark theme. Use `class` with the
//     active palette — `rgb` on a light background is unreadable, and it
//     cannot follow a runtime theme toggle.

/** Stable, non-localized verb identity for a tool-call header. */
export type ToolVerbDto =
  | 'update' | 'create' | 'read' | 'search' | 'shell' | 'output'
  | 'kill' | 'fetch' | 'task' | 'todo' | 'skill' | 'generic';

/** Stable semantic icon identity for one tool-call header. */
export type ToolIconDto =
  | 'read' | 'search' | 'list' | 'edit' | 'terminal' | 'globe'
  | 'workflow' | 'list_checks' | 'sparkles' | 'plug' | 'output'
  | 'stop' | 'wrench';

/** A second header line with its own glyph, e.g. `$ cargo test`. */
export interface ToolSubLineDto {
  prefix: string;
  text: string;
}

/** The parameterized tool-call header — `Update(src/host.rs)`. */
export interface ToolHeaderDto {
  verb: ToolVerbDto;
  icon?: ToolIconDto;
  /** English label. Localizing clients key off `verb` instead. */
  label: string;
  primary?: string;
  qualifier?: string;
  count?: number;
  sub_line?: ToolSubLineDto;
  /** Pre-composed `label(primary)qualifier`. */
  title: string;
}

/** Theme-independent semantic class of one code run. */
export type SyntaxClassDto =
  | 'plain' | 'keyword' | 'type_name' | 'function' | 'string_lit' | 'number'
  | 'comment' | 'punctuation' | 'operator' | 'variable' | 'constant' | 'attribute';

/** Add / remove / context classification of a diff row. */
export type DiffLineKindDto = 'add' | 'remove' | 'context';

/** One pre-split run of a diff row's text. */
export interface CodeSegmentDto {
  text: string;
  class: SyntaxClassDto;
  /** Terminal-resolved foreground packed `0x00RRGGBB`. Prefer `class`. */
  rgb?: number;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  /** A changed word of a word-diffed pair — stronger emphasis background. */
  emph?: boolean;
}

/** One diff row: gutter metadata plus its content runs. */
export interface DiffRowDto {
  kind: DiffLineKindDto;
  /** New-file line number for add/context; old-file for remove. */
  line_no: number;
  /** 0-based hunk index; a change between rows is where `⋯` belongs. */
  hunk: number;
  word_diffed?: boolean;
  segments: CodeSegmentDto[];
}

/** A complete structured diff. */
export interface StructuredDiffDto {
  file_path?: string;
  language?: string;
  /** Gutter width across ALL hunks, so it does not jitter between them. */
  gutter_width: number;
  additions: number;
  removals: number;
  /** Rows dropped by the wire cap; `0` when complete. */
  truncated_rows: number;
  rows: DiffRowDto[];
}

/** What a result headline says, for clients that localize. */
export type HeadlineKindDto =
  | 'added' | 'removed' | 'added_removed' | 'lines_read' | 'lines_read_partial'
  | 'files_found' | 'files_found_truncated' | 'lines_found' | 'matches_found'
  | 'interrupted' | 'no_content' | 'failed' | 'plain';

/** Everything needed to render one completed call's `⎿` block. */
export interface ToolResultDisplayDto {
  /** English headline. Absent when there is nothing to say (TodoWrite). */
  headline?: string;
  headline_kind?: HeadlineKindDto;
  /** Numeric slots for `headline_kind`, in the order it documents. */
  headline_args?: number[];
  diff?: StructuredDiffDto;
  /** Plain-text body for the expanded view, clamped to the wire caps. */
  body?: string;
  /** Line count BEFORE clamping — drives "show N more lines". */
  body_lines: number;
  /** `body` was clamped; the full text remains in `result_json`. */
  body_truncated?: boolean;
  /** The body exceeds the inline budget — render it collapsed. */
  collapsed?: boolean;
}

/** Lifecycle state of one plan task. */
export type PlanTaskStateDto = 'pending' | 'in_progress' | 'completed';

/** One item of the model-managed working plan. */
export interface PlanTaskDto {
  /** Stable V2 task id. TodoWrite V1 items have none. */
  id?: string;
  subject: string;
  /** Present-continuous label, for the status line — not the list row. */
  active_form?: string;
  state: PlanTaskStateDto;
}

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
  | {
      type: 'tool_use';
      id: string;
      tool: string;
      input_json: string;
      header?: ToolHeaderDto;
    }
  | {
      type: 'tool_result';
      id: string;
      tool: string;
      result_json: string;
      is_error: boolean;
      /** Legacy raw diff pair; `display.diff` supersedes it. */
      old_string?: string;
      new_string?: string;
      file_path?: string;
      display?: ToolResultDisplayDto;
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
  owner?: PermissionOwnerDto;
}

/** Immutable owner scope for a parked permission request. */
export interface PermissionOwnerDto {
  session_id?: string;
  turn_id?: number;
  worker_name?: string;
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

/** Authoritative engine-side terminal state for a permission request. */
export type PermissionResolutionDto = 'approved' | 'denied' | 'cancelled' | 'expired';

/** One disabled/unavailable reason emitted by the engine. */
export interface ControlDisabledReasonDto {
  code: string;
  message?: string;
}

/** One provider-neutral reasoning selection. */
export type ReasoningSelectionDto =
  | { type: 'automatic' }
  | { type: 'disabled' }
  | { type: 'enabled' }
  | { type: 'level'; id: string }
  | { type: 'token_budget'; tokens: number };

/** One selectable reasoning option surfaced by the engine. */
export interface ReasoningOptionDto {
  selection: ReasoningSelectionDto;
  persistable: boolean;
}

/** Official budget bounds for token-budget reasoning models. */
export interface ReasoningBudgetRangeDto {
  min_tokens: number;
  max_tokens: number;
}

/** Capability description for the active model's reasoning controls. */
export interface ReasoningControlSpecDto {
  options: ReasoningOptionDto[];
  budget_range?: ReasoningBudgetRangeDto;
  provider_default: ReasoningSelectionDto;
  forced_reasoning: boolean;
  editable: boolean;
  disabled_reason?: ControlDisabledReasonDto;
}

/** Authoritative state for the active conversation's reasoning controls. */
export interface ReasoningControlStateDto {
  requested: ReasoningSelectionDto;
  effective: ReasoningSelectionDto;
  spec: ReasoningControlSpecDto;
}

/** Availability metadata for one permission mode. */
export interface PermissionModeOptionDto {
  mode: PermissionModeId;
  available: boolean;
  disabled_reason?: ControlDisabledReasonDto;
}

/** Authoritative state for the active conversation's permission controls. */
export interface PermissionControlStateDto {
  requested: PermissionModeId;
  effective: PermissionModeId;
  options: PermissionModeOptionDto[];
}

/** Full conversation-controls snapshot authored by the engine. */
export interface ConversationControlsDto {
  qualified_model: string;
  permission: PermissionControlStateDto;
  reasoning: ReasoningControlStateDto;
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

/** One live agent instance attached to the active session. */
export interface SessionAgentSummaryDto {
  agent_id: string;
  name: string;
  agent_type: string;
  model?: string;
  model_profile?: string;
  status: string;
  latest_activity?: string;
  updated_at_ms?: number;
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
  aliases?: string[];
  argument_hint?: string;
  menu_description?: string;
  hidden?: boolean;
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
  | { type: 'paused' }
  | { type: 'completed' }
  | { type: 'failed' }
  | { type: 'cancelled' };

/** One task row (listings.rs `TaskRowDto`). */
export interface TaskRowDto {
  task_id: string;
  task_type: string;
  status: TaskStatusDto;
  description: string;
  can_resume?: boolean;
  started_at_ms?: number;
}

// ─────────────────────────────────────────────────────────────────────────────
// local_apps.rs
//
// The fieldless enums ride as BARE wire strings (byte-identical to the
// local-apps core enums' canonical values — the `AccessTierDto` precedent).
// ─────────────────────────────────────────────────────────────────────────────

/** Workflow state of an app (local_apps.rs `AppWorkflowStateDto`). */
export type AppWorkflowStateDto = 'draft' | 'ready';

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
  | 'io'
  | 'llm_unavailable'
  | 'llm_output_rejected';

/** Whether a catalog row is the app's pinned init session (local_apps.rs `AppSessionKindDto`). */
export type AppSessionKindDto = 'init' | 'conversation';

/**
 * One row of an app's workspace-scoped session catalog (local_apps.rs
 * `AppSessionRowDto`) — field-for-field the shared {@link SessionRowDto} shape
 * plus the init marker; no file paths cross the wire.
 */
export interface AppSessionRowDto {
  /** Bare session uuid (the resume key). */
  uuid: string;
  title: string;
  modified_rfc3339: string;
  message_count: number;
  kind: AppSessionKindDto;
}

/** Why a checkpoint was recorded (local_apps.rs `AppCheckpointKindDto`). */
export type AppCheckpointKindDto =
  | 'scaffold_created'
  | 'generation_validated'
  | 'preview_approved'
  | 'user_approved'
  | 'pre_restore';

/** Field type in an app-owned data collection (local_apps.rs `AppDataFieldTypeDto`). */
export type AppDataFieldTypeDto =
  | 'text'
  | 'long_text'
  | 'integer'
  | 'decimal'
  | 'boolean'
  | 'date_time'
  | 'enum'
  | 'image_ref';

/** One field in a structured app data collection (local_apps.rs `AppDataFieldDto`). */
export interface AppDataFieldDto {
  id: string;
  label: string;
  field_type: AppDataFieldTypeDto;
  required: boolean;
  /** Choices for an `enum` field; empty for every other type. */
  options: string[];
}

/** A collection exposed through the native data API (local_apps.rs `AppDataCollectionDto`). */
export interface AppDataCollectionDto {
  id: string;
  label: string;
  fields: AppDataFieldDto[];
  enabled_by_default: boolean;
}

/** One local-app row (local_apps.rs `AppRecordDto`). */
export interface AppRecordDto {
  id: string;
  name: string;
  /** One-line description the user gave at creation time. */
  brief: string;
  /** Whether Git controls this app's source checkpoints and restores. */
  git_enabled: boolean;
  created_at_ms: number;
  updated_at_ms: number;
  workflow_state: AppWorkflowStateDto;
  conversation_id?: string;
  /** The app's pinned "init" session (bare uuid) — listed first in its catalog. */
  init_session_id?: string;
  workspace_rel: string;
}

/** One restorable app checkpoint (local_apps.rs `AppCheckpointDto`). */
export interface AppCheckpointDto {
  id: string;
  label: string;
  kind: AppCheckpointKindDto;
  created_at_ms: number;
}

/** How an approved build is served on the device (local_apps.rs `AppRuntimeModeDto`). */
export type AppRuntimeModeDto = 'static_export' | 'next_production';

/** Why a runtime stopped outside a user stop (local_apps.rs `AppRuntimeSuspensionReasonDto`). */
export type AppRuntimeSuspensionReasonDto =
  | 'backgrounded'
  | 'memory_warning'
  | 'runtime_quota'
  | 'process_exited';

/** Foreground recovery state of a suspended runtime (local_apps.rs `AppRuntimeRecoveryStateDto`). */
export type AppRuntimeRecoveryStateDto =
  | 'not_needed'
  | 'pending'
  | 'recovering'
  | 'recovered'
  | 'failed';

/** Generated application manifest (local_apps.rs `AppManifestDto`). */
export interface AppManifestDto {
  schema_version: number;
  /** Runtime API major; legacy manifests are readable but not mountable in v2. */
  runtime_api_version?: number;
  app_id: string;
  name: string;
  design_revision: number;
  collections: AppDataCollectionDto[];
  allowed_domains: string[];
  /** Capabilities the confirmed plan declared; empty for pre-capability manifests. */
  capabilities: AppCapabilityKindDto[];
  /** Native target the app was generated for; host-derived, absent when unknown. */
  device_context?: DeviceContextDto;
}

/**
 * The stable native target pair only. Viewport, safe area, color scheme,
 * reduced motion and input mode are live values the generated page reads from
 * `window.lingxi.v2.deviceContext`, so they are deliberately not persisted here.
 */
export interface DeviceContextDto {
  os: 'ios' | 'android' | 'desktop' | 'unknown' | string;
  formFactor: 'iphone' | 'ipad' | 'phone' | 'tablet' | 'desktop' | 'unknown' | string;
}

/** Runtime snapshot inside an app detail response (local_apps.rs `AppRuntimeDetailsDto`). */
export interface AppRuntimeDetailsDto {
  state: AppRuntimeStateDto;
  mode?: AppRuntimeModeDto;
  loopback_url?: string;
  suspension_reason?: AppRuntimeSuspensionReasonDto;
  recovery_state?: AppRuntimeRecoveryStateDto;
  last_error?: string;
}

/** Full application detail snapshot (local_apps.rs `AppDetailsDto`). */
export interface AppDetailsDto {
  app: AppRecordDto;
  manifest?: AppManifestDto;
  runtime: AppRuntimeDetailsDto;
  checkpoints: AppCheckpointDto[];
}

/** Legacy operation names retained as the v2 native bridge's low-level mapping. */
export type AppBridgeOperationDto =
  | 'query_data'
  | 'mutate_data'
  | 'network_request'
  | 'runtime_status'
  | 'capture_photo'
  | 'pick_image'
  | 'record_audio_start'
  | 'record_audio_stop'
  | 'get_location'
  | 'transcribe_speech'
  | 'post_notification'
  | 'clipboard_get_text'
  | 'clipboard_set_text'
  | 'share'
  | 'synthesize_speech'
  | 'file_read'
  | 'file_write'
  | 'device_status'
  | 'haptics'
  | 'deep_link'
  | 'llm_chat'
  | 'llm_stream'
  | 'agent_post'
  | 'agent_session_create'
  | 'agent_session_list'
  | 'agent_session_resume'
  | 'agent_session_close'
  | 'agent_send'
  | 'agent_stream'
  | 'agent_cancel'
  | 'agent_profile_propose_update'
  | 'background_schedule'
  | 'background_list'
  | 'background_status'
  | 'background_cancel'
  | 'background_retry'
  | 'calendar_list_events'
  | 'contacts_search'
  | 'media_get';

/** One host-bound, data-only bridge request (local_apps.rs `AppBridgeRequestDto`). */
export interface AppBridgeRequestDto {
  request_id: string;
  app_id: string;
  operation: AppBridgeOperationDto;
  payload_json?: string;
}

/** Result of a bridge request (local_apps.rs `AppBridgeResponseDto`). */
export interface AppBridgeResponseDto {
  request_id: string;
  app_id: string;
  ok: boolean;
  result_json?: string;
  error?: string;
  /** Stable machine-readable failure code (`capability_not_declared`, …). */
  error_code?: string;
}

/** Local App Runtime OS v2 version (client-protocol `AppRuntimeApiVersionDto`). */
export interface AppRuntimeApiVersionDto {
  major: number;
  minor: number;
  patch: number;
}

/** The v2 runtime is a direct cutover; v1 pages are incompatible. */
export const LOCAL_APP_RUNTIME_API_VERSION: AppRuntimeApiVersionDto = {
  major: 2,
  minor: 0,
  patch: 0,
};

export type AppInvocationOriginDto =
  | 'page_foreground'
  | 'conversation_agent'
  | 'app_runtime_headless'
  | 'system_scheduler';

export interface AppInvocationFrameDto {
  appId: string;
  capability: string;
  inputHash: string;
}

/** Host-created attribution metadata for a privileged v2 call. */
export interface AppInvocationContextDto {
  appId: string;
  appInstanceId: string;
  requestId: string;
  turnId?: string;
  origin: AppInvocationOriginDto;
  grantEpoch: number;
  capabilityInstance?: string;
  callChain?: AppInvocationFrameDto[];
}

/** Generic v2 operation addressed by a registry id (`llm.stream`, etc.). */
export interface AppBridgeV2RequestDto {
  apiVersion: AppRuntimeApiVersionDto;
  context: AppInvocationContextDto;
  operation: string;
  payloadJson?: string;
  stream?: boolean;
}

export interface AppBridgeV2ResponseDto {
  requestId: string;
  appId: string;
  ok: boolean;
  resultJson?: string;
  error?: string;
  errorCode?: string;
  streamId?: string;
}

export type AppBridgeStreamFrameDto =
  | { type: 'started'; appId: string; requestId: string; streamId: string }
  | {
      type: 'data';
      appId: string;
      requestId: string;
      streamId: string;
      seq: number;
      dataJson: string;
    }
  | {
      type: 'completed';
      appId: string;
      requestId: string;
      streamId: string;
      seq: number;
    }
  | {
      type: 'error';
      appId: string;
      requestId: string;
      streamId: string;
      seq: number;
      code: string;
      message: string;
    }
  | {
      type: 'cancelled';
      appId: string;
      requestId: string;
      streamId: string;
      seq: number;
      reason: string;
    };

export type AppAgentSessionStatusDto = 'active' | 'paused' | 'closed';

export interface AppAgentBudgetDto {
  maxTokens: number;
  maxWallMs: number;
  maxTurns: number;
  maxBridgeCalls: number;
  maxMcpCalls: number;
  maxRecursionDepth: number;
}

export interface AppAgentSessionDto {
  sessionId: string;
  appId: string;
  appInstanceId: string;
  status: AppAgentSessionStatusDto;
  promptProfileRevision: number;
  budget: AppAgentBudgetDto;
  turnCount: number;
  outputTokensUsed: number;
  bridgeCallsUsed: number;
  mcpCallsUsed: number;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface AppAgentProfileDto {
  appId: string;
  revision: number;
  instructions: string;
  updatedAtMs: number;
}

export interface AppAgentProfileProposalDto {
  appId: string;
  approvalToken: string;
  baseRevision: number;
  currentRevision: number;
  instructions: string;
  reason: string;
}

/** Allow-listed UI operations; arbitrary script is absent (local_apps.rs `AppUiActionKindDto`). */
export type AppUiActionKindDto =
  | 'inspect'
  | 'click'
  | 'fill'
  | 'select'
  | 'toggle'
  | 'scroll'
  | 'navigate'
  | 'back'
  | 'reload'
  | 'capture_view'
  | 'pointer'
  | 'key';

/** A structured target resolved by the `WebView` host (local_apps.rs `AppUiTargetDto`). */
export interface AppUiTargetDto {
  element_id?: string;
  role?: string;
  name?: string;
}

/** One permission-gated UI automation request (local_apps.rs `AppUiRequestDto`). */
export interface AppUiRequestDto {
  request_id: string;
  app_id: string;
  action: AppUiActionKindDto;
  target?: AppUiTargetDto;
  value?: string;
}

/** Native capability whose first use needs a decision (local_apps.rs `AppCapabilityKindDto`). */
export type AppCapabilityKindDto =
  | 'data_mutation'
  | 'ui_control'
  | 'network_domain'
  | 'restore_checkpoint'
  | 'camera'
  | 'photo_library'
  | 'microphone'
  | 'location'
  | 'notifications'
  | 'files'
  | 'files_read'
  | 'files_write'
  | 'clipboard'
  | 'share'
  | 'text_to_speech'
  | 'device_status'
  | 'haptics'
  | 'deep_link'
  | 'llm'
  | 'agent_notify'
  | 'background_schedule'
  | 'calendar'
  | 'contacts'
  | 'media';

/** A capability approval request surfaced by the host (local_apps.rs `AppCapabilityRequestDto`). */
export interface AppCapabilityRequestDto {
  request_id: string;
  app_id: string;
  capability: AppCapabilityKindDto;
  domain?: string;
  reason: string;
}

/** User decision for data/UI/capability requests (local_apps.rs `AppAuthorizationDecisionDto`). */
export type AppAuthorizationDecisionDto =
  | 'deny'
  | 'allow_once'
  | 'allow_session'
  | 'allow_always';

/**
 * The local-app payload carried by the single {@link ClientEvent} `app_event`
 * envelope (local_apps.rs `AppEventDto`) — one envelope keeps the generated
 * mobile enum metadata bounded. Internally tagged on `type`, `snake_case`.
 */
export type AppEventDto =
  | { type: 'app_details_changed'; details: AppDetailsDto }
  | { type: 'app_record_changed'; record: AppRecordDto }
  | { type: 'app_profile_proposal'; proposal: AppAgentProfileProposalDto }
  | { type: 'app_bridge_response'; response: AppBridgeResponseDto }
  | { type: 'app_ui_request'; request: AppUiRequestDto }
  | { type: 'app_capability_requested'; request: AppCapabilityRequestDto }
  | { type: 'app_checkpoints_changed'; app_id: string; checkpoints: AppCheckpointDto[] }
  /** An app-initiated `llm.chat` started/finished; drives the "calling AI" indicator. */
  | { type: 'app_llm_activity_changed'; app_id: string; active: boolean }
  /** An app posted a mailbox event via `agent.post`; carries no body — badge only. */
  | { type: 'app_agent_event_posted'; app_id: string; seq: number; topic: string; created_at_ms: number }
  | {
      type: 'app_background_task_changed';
      app_id: string;
      task_id: string;
      status: string;
      result_json?: string;
      error?: string;
      retryable: boolean;
    }
  | { type: 'app_bridge_stream_frame'; frame: AppBridgeStreamFrameDto; frameJson: string };

// ─────────────────────────────────────────────────────────────────────────────
// events.rs
// ─────────────────────────────────────────────────────────────────────────────

/**
 * A user-visible attachment surfaced during a turn (events.rs `AttachmentDto`).
 * Internally tagged on `type`; `#[non_exhaustive]` on the Rust side ⇒ a future
 * attachment kind is additive.
 */
export type AttachmentDto = { type: 'nested_memory'; display_path: string };

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
  | {
      type: 'permission_request_resolved';
      request_id: number;
      resolution: PermissionResolutionDto;
    }
  // ── Live-turn streaming events ──────────────────────────────────────────────
  | { type: 'text_delta'; text: string }
  | {
      type: 'tool_use_started';
      id: string;
      tool: string;
      input_json: string;
      header?: ToolHeaderDto;
    }
  | { type: 'tool_heartbeat'; id: string; tool: string; elapsed_ms: number }
  | {
      type: 'tool_use_result';
      id: string;
      tool: string;
      result_json: string;
      is_error: boolean;
      display?: ToolResultDisplayDto;
    }
  | { type: 'plan_updated'; tasks: PlanTaskDto[] }
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
  | { type: 'session_agent_list'; session_id: string; agents: SessionAgentSummaryDto[] }
  | {
      type: 'session_agent_transcript';
      session_id: string;
      agent_id: string;
      messages: MessageDto[];
      next_message_index: number;
      revision: number;
    }
  | { type: 'session_agent_updated'; session_id: string; agent: SessionAgentSummaryDto }
  | {
      type: 'session_agent_message';
      session_id: string;
      agent_id: string;
      message_index: number;
      message: MessageDto;
    }
  // ── Listing / screen events ─────────────────────────────────────────────────
  | { type: 'model_list'; models: string[]; current: string }
  | { type: 'model_changed'; model: string }
  | { type: 'permission_mode_changed'; mode: PermissionModeId }
  | { type: 'conversation_controls_changed'; controls: ConversationControlsDto }
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
  | { type: 'slash_command_result'; turn_id?: number; display: string; is_error?: boolean }
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
  | {
      type: 'task_status_changed';
      task_id: string;
      status: TaskStatusDto;
      origin_session_id?: string;
    }
  | {
      type: 'workflow_resumed';
      previous_task_id: string;
      task: TaskRowDto;
      run_id: string;
      origin_session_id?: string;
    }
  | { type: 'commands_changed'; commands: SlashCommandDto[] }
  // ── Local apps ──────────────────────────────────────────────────────────────
  | { type: 'apps_changed'; apps: AppRecordDto[] }
  | { type: 'app_event'; event: AppEventDto }
  | { type: 'app_workflow_changed'; app_id: string; state: AppWorkflowStateDto; detail?: string }
  | {
      type: 'app_runtime_changed';
      app_id: string;
      state: AppRuntimeStateDto;
      details?: AppRuntimeDetailsDto;
      last_error?: string;
    }
  | {
      type: 'app_sessions_changed';
      app_id: string;
      sessions: AppSessionRowDto[];
      next_offset?: number;
    }
  | { type: 'app_checkpoint_created'; app_id: string; checkpoint: AppCheckpointDto }
  | { type: 'app_operation_failed'; app_id?: string; code: AppErrorCodeDto; message: string }
  // ── Reserved / feed-deferred (round-trip only) ──────────────────────────────
  | { type: 'coordinator_status'; active_workers: number; team?: string }
  | {
      type: 'coordinator_worker';
      worker: { agent_id: string; name: string; agent_type: string; status: string };
    }
  | { type: 'attachment'; attachment: AttachmentDto }
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
