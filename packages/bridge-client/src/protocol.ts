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
export const CLIENT_PROTOCOL_VERSION = '20.0.0';

/**
 * The largest single WebSocket frame the engine will read
 * (`MAX_INBOUND_FRAME_BYTES` in `bridge/src/mcp_endpoint.rs`).
 *
 * This is a HARD ceiling, not a validation bound: a `ClientCommand` is sent as
 * one unfragmented text frame ({@link BridgeClient} does `ws.send(JSON.stringify(frame))`,
 * and `ws` does not fragment), and a frame over this limit is not a rejected
 * command — tungstenite yields `Err(Capacity(MessageTooLong))`, `run_frame_pump`
 * breaks, and `BridgeConnection::close_connection` aborts the turn and drains
 * every broker. The user loses the whole session rather than the one operation.
 *
 * So every payload bound a client applies has to be derived from THIS number
 * rather than chosen next to it — `apps/electron/src/shared/audioResponse.ts`
 * derives its base64 bound here, and `audio-engine-bounds.test.ts` pins this
 * constant against the engine's own source so the two cannot drift apart
 * silently. A bound that merely looks generous is how a 24 MiB audio limit came
 * to sit above a 16 MiB transport.
 */
export const MAX_BRIDGE_FRAME_BYTES = 16 * 1024 * 1024;

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
  | { type: 'skills' }
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
 * A settings tier the user can WRITE to, as named on the wire (commands.rs
 * `WritableScopeDto`). Deliberately narrower than the engine's full `Scope`
 * (which also has `defaults`/`cli`/`managed`/`env`, and for MCP
 * `dynamic`/`enterprise`): those tiers cannot be user-written, so they are
 * omitted rather than accepted and rejected at runtime.
 *
 * This was two identical types — one for settings destinations, one for MCP
 * scopes — merged in protocol 16.0.0. A bare wire string.
 */
export type WritableScopeDto = 'user' | 'project' | 'local';

/**
 * The behavior bucket a permission rule belongs to
 * (`permissions.{allow,deny,ask}`), as named on the wire (commands.rs
 * `PermissionBehaviorDto`). A bare wire string.
 */
export type PermissionBehaviorDto = 'allow' | 'deny' | 'ask';

/**
 * Device AudioService operation identity. IDs are unique UUIDs, while the two
 * monotonic counters prevent stale callbacks from changing a newer lease.
 */
export interface AudioOperationIdDto {
  id: string;
  generation: number;
  service_epoch: number;
}

/** Trusted host context; model arguments never supply this identity. */
export type AudioOwnerDto =
  | { type: 'session'; session_id: string }
  | { type: 'local_app'; app_id: string; runtime_generation: number }
  | { type: 'ui'; instance_id: string }
  | { type: 'system'; instance_id: string };

export interface AudioInitiatorDto {
  agent_id?: string;
  tool_use_id?: string;
  request_id?: string;
}

/** A single immutable audio intent and its host-owned operation context. */
export interface AudioOperationRequestDto {
  identity: AudioOperationIdDto;
  owner: AudioOwnerDto;
  initiator?: AudioInitiatorDto;
  timeout_budget_ms?: number;
  max_payload_bytes: number;
  operation: AudioOperationDto;
}

/** Operation variants shared by engine requests and device-local services. */
export type AudioOperationDto =
  | { type: 'start_recording'; sample_rate_hz: number; format: string }
  | { type: 'stop_recording'; handle: string }
  | { type: 'listen'; language?: string }
  | { type: 'synthesize'; text: string; language?: string; rate?: number; voice?: string }
  | { type: 'speak'; text: string; language?: string; rate?: number; voice?: string }
  | { type: 'status'; handle?: string }
  | { type: 'end_owner' };

export type AudioErrorKindDto =
  | 'permission_denied' | 'busy' | 'cancelled' | 'timeout' | 'no_speech' | 'not_recording'
  | 'unavailable' | 'unsupported' | 'model_missing' | 'voice_missing' | 'invalid_request'
  | 'synthesis_failed' | 'native_failure' | 'media_too_large';

export interface AudioErrorDto {
  kind: AudioErrorKindDto;
  message: string;
}

export interface AudioStatusDto {
  recording: boolean;
  playing: boolean;
}

/** Successful service outcomes or one structured terminal failure. */
export type AudioOperationResultDto =
  | { type: 'recording_started'; handle: string }
  | { type: 'recording'; audio_base64: string; mime_type: string }
  | { type: 'transcript'; text: string; language?: string; confidence?: number }
  | { type: 'synthesized'; pcm_base64: string; sample_rate_hz: number }
  | { type: 'playback_completed'; duration_ms: number }
  | { type: 'status'; status: AudioStatusDto }
  | { type: 'owner_ended' }
  | { type: 'failed'; error: AudioErrorDto };

export type AudioOperationKindDto = 'record' | 'listen' | 'synthesize' | 'speak';

export type AudioReadinessStateDto = 'ready' | 'needs_permission' | 'busy' | 'missing_model' | 'unavailable';

export interface AudioOperationReadinessDto {
  operation: AudioOperationKindDto;
  state: AudioReadinessStateDto;
}

export interface AudioCapabilitySnapshotDto {
  service_epoch: number;
  support_revision: number;
  supported_operations: AudioOperationKindDto[];
  readiness: AudioOperationReadinessDto[];
  max_payload_bytes: number;
}

/**
 * The inbound command envelope a client sends to the engine
 * (commands.rs `ClientCommand`). Internally tagged on `type`, `snake_case`.
 *
 * `#[non_exhaustive]` on the Rust side ⇒ a future variant is additive; consumers
 * should treat the union as open-ended.
 */
export interface CronRunDto {
  ownerPid?: number;
  claimGeneration?: number;
  manualOccurrenceAt?: number;
  id: string;
  taskId: string;
  scheduledAt: number;
  startedAt?: number;
  finishedAt?: number;
  status: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';
  model: string;
  reasoning: ReasoningSelectionDto;
  sessionId?: string;
  summary?: string;
  error?: string;
}
export interface CronAutomationDto {
  name?: string;
  version: number;
  status: 'active' | 'paused' | 'completed';
  statusReason?: string;
  model: string;
  reasoning: ReasoningSelectionDto;
  runMode: 'new_session' | 'selected_session' | 'task_session';
  targetSessionId?: string;
  ownedSessionId?: string;
  notificationPolicy: 'all' | 'failed' | 'none';
  runs?: CronRunDto[];
}

export interface CronRequestDto {
  action: 'list' | 'create' | 'update' | 'delete' | 'pause' | 'resume' | 'complete' | 'history' | 'prune_history';
  automation?: CronAutomationDto;
  id?: string;
  cron?: string;
  prompt?: string;
  recurring?: boolean;
  durable?: boolean;
  no_expiry?: boolean;
  expires_at?: number;
}

export interface CronJobDto {
  next_run_at?: number;
  automation?: CronAutomationDto;
  expires_at?: number;
  session_id?: string;
  id: string;
  cron: string;
  prompt: string;
  recurring: boolean;
  durable: boolean;
  permanent: boolean;
  created_at: number;
  last_fired_at?: number;
}

/** JSON value carried by the isolated Mod UI protocol boundary. */
export type UiJsonValue = null | boolean | number | string | UiJsonValue[] | { [key: string]: UiJsonValue };

export type NativeUiComponent =
  | 'AskUserQuestion'
  | 'UserMessage'
  | 'AssistantMessage'
  | 'ToolUse'
  | 'ToolResult'
  | 'ToolGroup'
  | 'ToolProgress'
  | 'CommandOutput'
  | 'Spinner'
  | 'TurnDuration'
  | 'InfoNotice'
  | 'SessionMode'
  | 'PromptHint'
  | 'AbovePrompt'
  | 'Pane';

export interface NativeUiViewportDto {
  columns: number;
  rows: number;
  isFullscreen?: boolean;
}

export interface NativeUiOnScreenDto {
  first: number;
  last: number;
  of: number;
}

export interface NativeUiKeyedRegionDto {
  plugin: string;
  key: string;
  top: number;
  bottom: number;
}

export interface NativeUiBenchRequestDto {
  seq: number;
  t0: number;
}

export interface NativeUiClientAddressDto {
  plugin: string;
  component: NativeUiComponent;
  instance_id: string;
  client: string;
  module: string;
}

export type NativeUiClientPressEventDto =
  | { type: 'press' }
  | { type: 'input'; kind: 'change' | 'submit'; value: string }
  | { type: 'select'; value: string };

export type NativeUiSurfaceDto = 'desktop' | 'mobile' | 'vscode';

/** Native parent-surface controls. These are distinct from Client `ui_client_press`. */
export type NativeUiParentControlRequest =
  | {
      subtype: 'ui_press';
      plugin: string;
      handle: number;
      key?: string;
      surface?: NativeUiSurfaceDto;
      href?: string;
      client_id?: string;
    }
  | {
      subtype: 'ui_input';
      plugin: string;
      handle: number;
      kind: 'change' | 'submit';
      value: string;
      key?: string;
      component?: NativeUiComponent;
      instance_id?: string;
      surface?: NativeUiSurfaceDto;
      client_id?: string;
    }
  | {
      subtype: 'ui_select';
      plugin: string;
      handle: number;
      value: string;
      key?: string;
      component?: NativeUiComponent;
      instance_id?: string;
      surface?: NativeUiSurfaceDto;
      client_id?: string;
    };

/** Native `ui_*` payloads. The transport wraps these JSON bodies in strings. */
export type NativeUiControlRequest =
  | {
      subtype: 'ui_render';
      surface: 'desktop' | 'mobile' | 'vscode';
      component: NativeUiComponent;
      instance_id: string;
      props: Record<string, UiJsonValue>;
      client_id?: string;
      viewport?: NativeUiViewportDto;
      on_screen?: NativeUiOnScreenDto | null;
      content_rows?: number;
      keyed?: NativeUiKeyedRegionDto[];
      bench?: NativeUiBenchRequestDto;
    }
  | { subtype: 'ui_client_module'; plugin: string }
  | (NativeUiClientAddressDto & {
      subtype: 'ui_client_press';
      element: string;
      event: NativeUiClientPressEventDto;
    })
  | (NativeUiClientAddressDto & { subtype: 'ui_message'; data: UiJsonValue })
  | (NativeUiClientAddressDto & {
      subtype: 'ui_client_fault';
      phase: 'load' | 'render' | 'run';
      reason: string;
    })
  | NativeUiParentControlRequest;

export interface NativeUiRenderResponseDto {
  tree: UiJsonValue;
  props: Record<string, UiJsonValue>;
  rewritten: boolean;
  hooked: boolean;
  client_modules?: Record<string, string>;
  bench?: Record<string, UiJsonValue>;
}

export interface NativeUiClientModuleResponseDto {
  plugin: string;
  hash: string;
  modules: Array<{ module: string; entry: string; component: string }>;
  runtime: string;
  limits: { nodes: number; depth: number; chars: number; values: number; dataDepth: number };
  files: Array<{ key: string; source: string }>;
}

export interface NativeUiClientPressResponseDto {
  handled: boolean;
  reached?: NativeUiClientPressReachedDto;
}

export interface NativeUiParentPressResponseDto {
  handled: boolean;
  element?: string;
}

export interface NativeUiParentInputResponseDto {
  handled: boolean;
  element?: string;
  value?: string;
}

export interface NativeUiParentSelectResponseDto {
  handled: boolean;
  element?: string;
  value?: string;
}

export interface NativeUiClientPressReachedDto {
  element: string;
  value?: string;
}

export interface NativeUiMessageResponseDto {
  handled: boolean;
  props?: UiJsonValue;
}

export interface NativeUiClientFaultResponseDto {
  handled: boolean;
}

export type NativeUiControlResponse =
  | NativeUiRenderResponseDto
  | NativeUiClientModuleResponseDto
  | NativeUiClientPressResponseDto
  | NativeUiParentPressResponseDto
  | NativeUiParentInputResponseDto
  | NativeUiParentSelectResponseDto
  | NativeUiMessageResponseDto
  | NativeUiClientFaultResponseDto
  | null;

export type NativeUiControlResponseFor<T extends NativeUiControlRequest> = T extends { subtype: 'ui_render' }
  ? NativeUiRenderResponseDto
  : T extends { subtype: 'ui_client_module' }
    ? NativeUiClientModuleResponseDto | null
    : T extends { subtype: 'ui_client_press' }
      ? NativeUiClientPressResponseDto
      : T extends { subtype: 'ui_press' }
        ? NativeUiParentPressResponseDto
        : T extends { subtype: 'ui_input' }
          ? NativeUiParentInputResponseDto
          : T extends { subtype: 'ui_select' }
            ? NativeUiParentSelectResponseDto
      : T extends { subtype: 'ui_message' }
        ? NativeUiMessageResponseDto
        : NativeUiClientFaultResponseDto;

/** Local Harness VM operation request; source code is never part of this DTO. */
export type UiClientOperation =
  | {
      type: 'mount'; surface: 'desktop'; component: NativeUiComponent; instance_id: string;
      plugin: string; client: string; module: string; render_revision: number; columns: number; rows: number;
    }
  | { type: 'render'; runtimeId: string; render_revision: number }
  | { type: 'unmount'; runtimeId: string; render_revision: number }
  | { type: 'setProps'; runtimeId: string; render_revision: number; props: Record<string, UiJsonValue> }
  | { type: 'resize'; runtimeId: string; render_revision: number; columns: number; rows: number }
  | { type: 'pointer'; runtimeId: string; render_revision: number; event: UiJsonValue }
  | { type: 'key'; runtimeId: string; render_revision: number; event: UiJsonValue }
  | { type: 'runHeld'; runtimeId: string; render_revision: number; event?: UiJsonValue; handle: number }
  | {
      type: 'draw_commit'; surface: 'desktop'; component: NativeUiComponent; instance_id: string;
      render_revision: number; clients: Array<{ plugin: string; key: string; module: string }>;
    }
  | { type: 'draw_unmount'; surface: 'desktop'; component: NativeUiComponent; instance_id: string; render_revision: number };

export interface UiClientFrameDto {
  runtimeId: string;
  renderRevision: number;
  /** Host-local per-runtime ordering for RPC and asynchronous frames. */
  frameSequence: number;
  tree: UiJsonValue;
  hasPointerListener: boolean;
  hasKeyListener: boolean;
}

/** Worker-side failure published asynchronously without a successful tree frame. */
export interface UiClientWorkerFaultSnapshotDto {
  renderRevision: number;
  fault: UiClientWorkerFaultResponseDto['fault'];
}

export type UiClientFrameEventPayloadDto = Omit<UiClientFrameDto, 'runtimeId'> | UiClientWorkerFaultSnapshotDto;

export interface UiClientHandledOperationResponseDto {
  handled: boolean;
  renderRevision: number;
}

/** A frame operation was safely ignored because its runtime/revision is stale. */
export interface UiClientOperationNoopResponseDto {
  handled: false;
  renderRevision: number;
}

/** A worker fault is a leaf result for this operation, not a second fault command. */
export interface UiClientWorkerFaultResponseDto {
  handled: false;
  renderRevision: number;
  runtimeId?: string;
  fault: {
    phase: 'load' | 'render' | 'run';
    reason: string;
    source: 'worker';
  };
}

export type UiClientFrameOperationResponseDto =
  | UiClientFrameDto
  | UiClientOperationNoopResponseDto
  | UiClientWorkerFaultResponseDto;

export type UiClientOperationResponse = UiClientFrameOperationResponseDto | UiClientHandledOperationResponseDto;

export type UiClientOperationResponseFor<T extends UiClientOperation> = T extends
  | { type: 'mount' | 'render' | 'setProps' | 'resize' | 'pointer' | 'key' | 'runHeld' }
  ? UiClientFrameOperationResponseDto
  : UiClientHandledOperationResponseDto;

/** Host-local runtime facts are separate from Native UI JSON. */
export interface UiControlMetadataDto {
  renderRevision?: number;
  clientRuntimeEpochs?: Record<string, number>;
  /** Opaque Native Client failure-state version for this parent render site. */
  clientStateToken?: string;
}

export interface UiControlCallResultDto<TResponse = NativeUiControlResponse> {
  response: TResponse;
  metadata?: UiControlMetadataDto;
}

export interface UiClientFrameEventDto {
  sessionId: string;
  runtimeId: string;
  frame: UiClientFrameEventPayloadDto;
}

export interface UiInvalidateEventDto {
  sessionId: string;
  instances?: Array<{ surface: 'desktop' | 'mobile' | 'vscode'; component: NativeUiComponent; instance_id: string }>;
  uuid: string;
}

export type ClientCommand =
  | { type: 'cron_run_started'; run_id: string; session_id: string }
  | { type: 'scheduled_run_turn'; run_id: string; prompt: string; model: string; reasoning: ReasoningSelectionDto }
  | { type: 'cron_run_completed'; run_id: string; session_id?: string | null; summary?: string | null; error?: string | null }
  | { type: 'cron_manage'; request_id: string; request: CronRequestDto }
  | { type: 'ui_render'; request_id: string; request_json: string }
  | { type: 'ui_client_module'; request_id: string; plugin: string }
  | { type: 'ui_message'; request_id: string; request_json: string }
  | { type: 'ui_client_fault'; request_id: string; request_json: string }
  | { type: 'ui_client_press'; request_id: string; request_json: string }
  | { type: 'ui_press'; request_id: string; request_json: string }
  | { type: 'ui_input'; request_id: string; request_json: string }
  | { type: 'ui_select'; request_id: string; request_json: string }
  | { type: 'ui_client_operation'; request_id: string; operation_json: string }
  // ── Turn driving ──────────────────────────────────────────────────────────
  | {
      type: 'send_prompt';
      text: string;
      prompt_mode?: PromptModeDto;
      images: ImageRefDto[];
      turn_id?: number;
      /** Widget the user follows up on; the engine attaches its saved model state. */
      visualization_context?: VisualizationRefDto;
    }
  | { type: 'cancel'; turn_id?: number }
  /**
   * Reattach a mobile client to a durable turn on the connection's active
   * session; the engine emits the current recovery snapshot and replays
   * events whose sequence is greater than `after_sequence`
   * (commands.rs `ClientCommand::AttachTurn`).
   */
  | { type: 'attach_turn'; turn_id: number; after_sequence?: number }
  /** Resume a checkpointed durable turn when its recovery policy allows it (commands.rs `ClientCommand::ResumeTurn`). */
  | { type: 'resume_turn'; turn_id: number }
  /**
   * Persist a platform-lease expiration without converting it to cancel.
   * `reason` is a machine-readable platform reason such as
   * `background_time_expired` (commands.rs `ClientCommand::PauseTurn`).
   */
  | { type: 'pause_turn'; turn_id: number; reason: string }
  // ── Permission resolution ───────────────────────────────────────────────────
  | { type: 'approve_permission'; request_id: number; response: PermissionResponseDto }
  | { type: 'deny_permission'; request_id: number }
  | { type: 'set_permission_mode'; mode: PermissionModeId }
  | { type: 'set_typescript_lsp_mode'; mode: TypescriptLspModeId }
  // ── `computer` tool request_access resolution ───────────────────────────────
  | { type: 'approve_computer_access'; request_id: number; response: ComputerAccessResponseDto }
  | { type: 'deny_computer_access'; request_id: number }
  // ── `AskUserQuestion` resolution ───────────────────────────────────────────
  | { type: 'answer_ask_user_question'; request_id: number; answers: Record<string, string> }
  | { type: 'cancel_ask_user_question'; request_id: number }
  // ── Provider credentials (authenticated local bridge only) ──────────────────
  | { type: 'list_provider_credentials'; operation_id: number; provider_ids: string[]; preview_provider_ids?: string[] }
  | { type: 'set_provider_credential'; operation_id: number; provider_id: string; credential: string }
  | { type: 'delete_provider_credential'; operation_id: number; provider_id: string }
  | {
      type: 'test_provider_connection';
      operation_id: number;
      provider_id: string;
      api_base: string;
      model: string;
      credential_override?: string;
    }
  // ── Model ─────────────────────────────────────────────────────────────────
  | { type: 'set_model'; model: string }
  | { type: 'list_models' }
  | { type: 'get_conversation_controls' }
  | { type: 'set_reasoning_selection'; selection: ReasoningSelectionDto }
  | { type: 'set_fast_mode'; enabled: boolean }
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
  | { type: 'fork_session'; session_id: string; target_mode: SessionModeDto }
  // Explicit UI mount lifecycle; transport hello/close does not imply an attach.
  | { type: 'ui_attach'; surface: 'desktop' | 'mobile' | 'vscode'; client_id: string }
  | { type: 'ui_detach'; client_id: string }
  // ── Auth + session control ────────────────────────────────────────────────
  | { type: 'login' }
  | { type: 'logout' }
  | { type: 'force_compact' }
  | { type: 'clear_session' }
  // ── Tasks ───────────────────────────────────────────────────────────────────
  | { type: 'task_list'; status_filter?: TaskStatusDto; request_id?: string }
  | { type: 'task_output'; task_id: string; offset: number }
  | { type: 'task_stop'; task_id: string }
  | { type: 'task_message'; task_id: string; message: string }
  | { type: 'resume_workflow'; task_id: string }
  // ── Lifecycle ───────────────────────────────────────────────────────────────
  | { type: 'request_exit' }
  // ── Settings (persisted) ─────────────────────────────────────────────────────
  | { type: 'update_settings'; destination: WritableScopeDto; patch_json: string }
  // ── Permissions (persisted) ──────────────────────────────────────────────────
  | {
      type: 'update_permission_rules';
      destination: WritableScopeDto;
      behavior: PermissionBehaviorDto;
      add: string[];
      remove: string[];
    }
  | { type: 'set_default_permission_mode'; destination: WritableScopeDto; mode: string }
  | {
      type: 'update_workspace_directories';
      destination: WritableScopeDto;
      add: string[];
      remove: string[];
    }
  // ── MCP servers (persisted) ──────────────────────────────────────────────────
  | { type: 'upsert_mcp_server'; scope: WritableScopeDto; name: string; config_json: string }
  | { type: 'remove_mcp_server'; scope: WritableScopeDto; name: string }
  | { type: 'skill_admin'; command: SkillAdminCommandDto }
  | { type: 'mcp_admin'; command: McpAdminCommandDto }
  | { type: 'plugin_admin'; command: PluginAdminCommandDto }
  | { type: 'hook_admin'; command: HookAdminCommandDto }
  // ── AudioService (engine -> device operations and capability publication) ──
  | { type: 'audio_response'; identity: AudioOperationIdDto; result: AudioOperationResultDto }
  | { type: 'update_audio_capabilities'; capabilities: AudioCapabilitySnapshotDto };

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
    }
  /** An inline visualization placed by a reference line; no `reference` renders "unavailable". */
  | { type: 'visualization'; reference?: VisualizationRefDto };

/** One published revision of an inline visualization (message.rs `VisualizationRefDto`). */
export interface VisualizationRefDto {
  id: string;
  revision: number;
}

/** The widget a user message continued from (message.rs `VisualizationContextDto`). */
export interface VisualizationContextDto {
  id: string;
  revision: number;
  title: string;
}

/** Progress of a live visualization slot (message.rs `VisualizationBlockStatusDto`). */
export type VisualizationBlockStatusDto = 'pending' | 'ready' | 'unavailable' | 'discarded';

/** A complete conversation message (message.rs `MessageDto`). */
export interface LoopWakeupDto {
  message: string;
  companion?: string | null;
  streak: number;
  since_ms: number;
}

export interface MessageDto {
  loop_wakeup?: LoopWakeupDto | null;
  role: string;
  blocks: MessageBlockDto[];
  /** User-attached images projected as stable renderable URLs. */
  images?: MessageImageDto[];
  /** The widget this user message followed up on, shown as an attachment chip. */
  visualization_context?: VisualizationContextDto | null;
}

export interface MessageImageDto {
  media_type: string;
  url: string;
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
  /** True when the client must not offer or persist an always-allow rule. */
  suppress_always_allow_rule?: boolean;
  /** Engine-computed Auto action; clients must not infer eligibility. */
  auto_mode_prompt?: AutoModePromptDto;
}

export type AutoModePromptDto = 'workflow_bash' | 'exit_plan_mode';

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
  | { type: 'allow_auto' }
  | { type: 'deny' };

/** Live session permission modes accepted by the engine. */
export type PermissionModeId =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'auto'
  | 'dontAsk'
  | 'bypassPermissions';

/** Global activation policy for the fixed, built-in TypeScript 7 LSP. */
export type TypescriptLspModeId = 'auto' | 'off' | 'on';

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

export type ModelBillingModeDto = 'per_token' | 'subscription' | 'free' | 'unknown';

export interface ModelPricingTierDto {
  context_threshold_tokens: number;
  input_per_million?: number;
  output_per_million?: number;
  cache_read_per_million?: number;
  cache_write_per_million?: number;
  reasoning_per_million?: number;
}

export interface ModelPricingDto {
  billing_mode: ModelBillingModeDto;
  input_per_million?: number;
  output_per_million?: number;
  cache_read_per_million?: number;
  cache_write_per_million?: number;
  reasoning_per_million?: number;
  tiers: ModelPricingTierDto[];
  source?: string;
}

export interface ModelCapabilitiesDto {
  streaming: boolean;
  tools: boolean;
  vision: boolean;
  documents: boolean;
  reasoning: boolean;
  structured_output: boolean;
}

export interface ModelDetailsDto {
  reference: string;
  provider_id: string;
  provider_label: string;
  display_name: string;
  model_id: string;
  description?: string;
  family?: string;
  status?: string;
  release_date?: string;
  last_updated?: string;
  knowledge_cutoff?: string;
  input_modalities: string[];
  output_modalities: string[];
  context_window_tokens?: number;
  max_input_tokens?: number;
  max_output_tokens?: number;
  open_weights?: boolean;
  attachments?: boolean;
  temperature_control?: boolean;
  pricing?: ModelPricingDto;
  capabilities: ModelCapabilitiesDto;
  reasoning: ReasoningControlSpecDto;
  supports_fast_mode?: boolean;
  /**
   * 该路由能否担任 Fusion 的 analyst：模型自称支持结构化输出，**并且**它所属
   * profile 的 wire codec 真的能编码 `response_format`。严格强于
   * `capabilities.structured_output`（那只是模型自身的属性）——Gemini 路由会
   * 声明该能力然后在编码时硬失败，而那时每个 Fusion panel 已经花完钱了。
   * 让用户指定 analyst 的设置界面必须按这个字段过滤，不能按能力位。
   */
  fusion_analyst_capable?: boolean;
}

export interface ProviderModelCatalogEntryDto {
  provider_id: string;
  provider_label: string;
  models: ModelDetailsDto[];
  /**
   * Vendor this entry is one connection of; absent when it stands alone.
   * Settings UIs collapse entries sharing a group under one provider heading.
   * Model pickers must NOT: two connections can differ in billing, so the
   * choice has to stay visible.
   */
  group?: string;
  /** This connection's id within `group` (`cn`, `intl`, `coding`, …). */
  connection_id?: string;
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

/** Which capability profile a mobile session runs under (listings.rs `SessionModeDto`). */
export type SessionModeDto = 'chat' | 'code';

/** One resumable-session row (listings.rs `SessionRowDto`). */
export interface SessionRowDto {
  uuid: string;
  title: string;
  modified_rfc3339: string;
  message_count: number;
  mode: SessionModeDto;
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

/** One UUID-addressable persisted row in a session-agent transcript. */
export interface SessionAgentMessageRowDto {
  message_index: number;
  message_uuid: string;
  message: MessageDto;
  /** Complete serialized Native synthetic API-error row, kept losslessly. */
  api_error_json?: string;
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

export type ConfigurationOperationStatusDto = 'started' | 'progress' | 'succeeded' | 'failed';

export type ConfigurationEffectDto = 'applied' | 'restart_required' | 'not_applicable';

export type ConfigurationDomainDto = 'skill' | 'mcp' | 'plugin' | 'hook';

export interface ConfigurationOperationDto {
  operation_id: number;
  domain: ConfigurationDomainDto;
  status: ConfigurationOperationStatusDto;
  effect: ConfigurationEffectDto;
  message?: string;
  details_json?: string;
}

export interface ConfigurationAdminCommandDto {
  action: string;
  operation_id?: number;
  target?: string;
  scope?: string;
  revision?: string;
  payload_json?: string;
}

export type SkillAdminCommandDto = ConfigurationAdminCommandDto;
export type McpAdminCommandDto = ConfigurationAdminCommandDto;
export type PluginAdminCommandDto = ConfigurationAdminCommandDto;
export type HookAdminCommandDto = ConfigurationAdminCommandDto;

/** One discovered skill entry (listings.rs `SkillDto`). */
export interface SkillDto {
  /** Skill display name (matches its directory name, not frontmatter). */
  name: string;
  /** The skill's own directory on disk, as a display string. */
  source_dir: string;
}

/** One hook entry (listings.rs `HookDto`). */
export interface HookDto {
  name: string;
  event: string;
  matcher?: string;
  timeout_ms: number;
  hook_type?: string;
  source?: string;
  content?: string;
  status_message?: string;
  blocking?: boolean;
  async?: boolean;
  async_rewake?: boolean;
  async_timeout_ms?: number;
  priority?: number;
  if_condition?: string;
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
  /** The teammate is waiting for its leader's plan decision. */
  awaiting_plan_approval?: boolean;
  task_id: string;
  /** Persistent runner identity for local_agent tasks; never the task creator. */
  agent_id?: string;
  task_type: string;
  status: TaskStatusDto;
  description: string;
  can_resume?: boolean;
  started_at_ms?: number;
  /** Terminal failure reason for a `failed` row, when the handler reported one. */
  error?: string;
  /** `local_fusion` only (F005): the run's current progress-stage label. */
  stage?: string;
  /** Additive shell specialization, e.g. a command event monitor. */
  kind?: string;
  /** Agent completion has not yet been delivered or consumed. */
  unread?: boolean;
  /** Concrete model used by an agent task. */
  model?: string;
  /** String effort label; numeric budgets are omitted. */
  effort?: string;
}

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

/**
 * Durable execution state for a mobile turn. Backgrounding itself never
 * changes this state; only execution, a recovery gate, or an explicit cancel
 * does (events.rs `TurnRecoveryStateDto`). Internally tagged on `type`,
 * `snake_case`. `#[non_exhaustive]` on the Rust side ⇒ a future state is
 * additive.
 */
export type TurnRecoveryStateDto =
  | { type: 'running' }
  | { type: 'waiting_for_user' }
  | { type: 'paused_recoverable' }
  | { type: 'completed' }
  | { type: 'failed' }
  | { type: 'cancelled' };

/**
 * Snapshot clients use to decide whether a durable turn can be reattached or
 * needs explicit user intervention (events.rs `TurnRecoverySnapshotDto`).
 * Carried by {@link ClientEvent} `turn_recovery_state`.
 */
export interface TurnRecoverySnapshotDto {
  session_id: string;
  turn_id: number;
  state: TurnRecoveryStateDto;
  first_sequence: number;
  last_sequence: number;
  safe_to_resume: boolean;
  reason?: string;
}

/** Cumulative cost snapshot carried by `turn_ended` (events.rs `CostDto`). */
export interface CostDto {
  total_usd: number;
  input_tokens: number;
  output_tokens: number;
  api_calls: number;
  session_duration_secs: number;
  formatted: string;
}

/** Provider message facts retained on an accepted server-fallback tombstone. */
export interface ServerFallbackProviderMessageDto {
  id?: string;
  model?: string;
  stop_reason?: string;
  stop_details_json?: string;
  usage_json?: string;
  /** Serialized array of the complete provider content blocks. */
  content_json: string;
}

/** Complete durable-row facts forwarded when accepted server fallback removes a row. */
export interface ServerFallbackTombstoneMessageDto {
  uuid: string;
  /** Native outer row kind, serialized on the wire as `type`. */
  type: string;
  timestamp: string;
  request_id?: string;
  request_ref_json?: string;
  message: ServerFallbackProviderMessageDto;
  is_api_error_message?: boolean;
  supersedes_uuids?: string[];
}

export type RefusalContinuationPhaseDto = 'begin';
export type RefusalContinuationJoinDto = 'exact';

/**
/**
 * Outbound events the engine streams to a client (events.rs `ClientEvent`).
 * Internally tagged on `type`, `snake_case`.
 *
 * `#[non_exhaustive]` on the Rust side ⇒ a future variant is additive.
 */
export type ClientEvent =
  | { type: 'scheduled_run_finished'; run_id: string; summary?: string | null; error?: string | null }
  | { type: 'cron_run_bound'; run_id: string; error?: string | null }
  | { type: 'cron_run_requested'; run_id: string; task: CronJobDto }
  | { type: 'cron_result'; request_id: string; jobs: CronJobDto[]; error?: string }
  | { type: 'ui_control_result'; request_id: string; response_json?: string; metadata_json?: string; error?: string }
  | { type: 'ui_client_frame'; runtime_id: string; frame_json: string }
  | { type: 'ui_invalidate'; instances_json?: string; uuid: string; session_id: string }
  /** A visualization slot in the live assistant text (events.rs `VisualizationBlock`). */
  | { type: 'visualization_block'; status: VisualizationBlockStatusDto; reference?: VisualizationRefDto }
  // ── Error ─────────────────────────────────────────────────────────────────
  | { type: 'error'; kind: ErrorKindDto; message: string }
  | { type: 'message_identity'; message_id: string }
  | { type: 'message_retracted'; message_id: string }
  /** Native route receipt for an accepted server-fallback hop. */
  | { type: 'query_model_change'; to_model: string }
  /** Host/client key for deltas until the completed text block has a durable UUID. */
  | { type: 'assistant_block_start'; block_key: number }
  /** Maps a transient block key to the persisted JSONL row UUID. */
  | { type: 'assistant_block_identity'; block_key: number; message_uuid: string }
  /** Complete row facts for a row removed by accepted server fallback. */
  | { type: 'tombstone'; message: ServerFallbackTombstoneMessageDto; display_only: boolean }
  /** Native refusal-text continuation; `display_salvage_text` is a host display policy. */
  | {
      type: 'refusal_continuation';
      phase: RefusalContinuationPhaseDto;
      salvage_text: string;
      join: RefusalContinuationJoinDto;
      replaces_uuids: string[];
      display_salvage_text: boolean;
    }
  /** TUI user-row token resolved to the UUID from successful transcript persistence. */
  | { type: 'user_transcript_row_identity'; row_token: string; uuid: string }
  /** Persisted text-row UUIDs, grouped by an internal assistant response identity. */
  | { type: 'assistant_transcript_row_uuids'; message_id: string; uuids: Array<string | null> }
  | { type: 'system_notice'; message: string; is_error: boolean }
  | { type: 'ui_log'; plugin: string; text: string }
  | { type: 'ui_toast'; plugin: string; text: string; timeout_ms: number }
  | { type: 'ui_status'; plugin: string; text: string | null }
  | { type: 'scheduled_task_fire'; message: string }
  | {
      type: 'loop_wakeup';
      /** The resume line, already rendered host-side. */
      message: string;
      /** The companion meta line, present only when `streak > 0`. */
      companion?: string;
      /** Consecutive quiet ticks before this wakeup; 0 for an ordinary one. */
      streak: number;
      /** When the streak began, epoch ms; 0 when there is none. */
      since_ms: number;
    }
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
  /**
   * Authoritative durable state for one mobile turn. Emitted on attach,
   * resume, recovery gating, and every terminal transition
   * (events.rs `ClientEvent::TurnRecoveryState`).
   */
  | { type: 'turn_recovery_state'; snapshot: TurnRecoverySnapshotDto }
  /**
   * Sequenced retained copy of a turn event, emitted beside live delivery and
   * replayed after `attach_turn`. `event_json` is the original serialized
   * {@link ClientEvent}, kept as a string to avoid a recursive shape
   * (events.rs `ClientEvent::TurnEventReplay`).
   */
  | { type: 'turn_event_replay'; session_id: string; turn_id: number; sequence: number; event_json: string }
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
      /** Empty or absent when connected to an older engine. */
      summary?: string;
    }
  // ── Session lifecycle ───────────────────────────────────────────────────────
  | { type: 'session_started'; session_id: string; mode: SessionModeDto }
  | { type: 'session_ended' }
  | { type: 'session_resumed'; session_id: string; mode: SessionModeDto; messages: MessageDto[] }
  | { type: 'session_forked'; source_session_id: string; session_id: string; mode: SessionModeDto }
  | { type: 'session_list'; sessions: SessionRowDto[] }
  | { type: 'session_agent_list'; session_id: string; agents: SessionAgentSummaryDto[] }
  | {
      type: 'session_agent_transcript';
      session_id: string;
      agent_id: string;
      messages: SessionAgentMessageRowDto[];
      next_message_index: number;
      revision: number;
    }
  | { type: 'session_agent_updated'; session_id: string; agent: SessionAgentSummaryDto }
  | {
      type: 'session_agent_message';
      session_id: string;
      agent_id: string;
      message_index: number;
      message_uuid: string;
      message: MessageDto;
      api_error_json?: string;
    }
  | { type: 'session_agent_tombstone'; session_id: string; agent_id: string; message_uuid: string; display_only: boolean }
  // ── Listing / screen events ─────────────────────────────────────────────────
  | { type: 'model_list'; models: string[]; current: string; details?: ModelDetailsDto[] }
  | { type: 'provider_model_catalog'; providers: ProviderModelCatalogEntryDto[] }
  | { type: 'compaction_status'; phase: string; error?: string }
  | { type: 'model_changed'; model: string }
  | { type: 'permission_mode_changed'; mode: PermissionModeId }
  | {
      type: 'typescript_lsp_mode_changed';
      requested: TypescriptLspModeId;
      effective: TypescriptLspModeId;
      available: boolean;
    }
  | { type: 'conversation_controls_changed'; controls: ConversationControlsDto }
  | { type: 'fast_mode_changed'; enabled: boolean }
  | {
      type: 'openai_oauth_updated';
      session: { access_token: string; refresh_token?: string; expires_at: number; account_id?: string; fedramp: boolean };
    }
  | {
      type: 'provider_credential_status';
      operation_id: number;
      configured_provider_ids: string[];
      unavailable_provider_ids?: string[];
      storage_encrypted: boolean;
      credential_previews?: Record<string, string>;
      error?: string;
    }
  | {
      type: 'provider_connection_tested';
      operation_id: number;
      provider_id: string;
      connected: boolean;
      reachable: boolean;
      authenticated: boolean;
      model_available: boolean;
      http_status?: number;
      latency_ms: number;
      message: string;
      used_stored_credential: boolean;
    }
  | {
      type: 'configuration_operation';
      domain: ConfigurationDomainDto;
      operation_id: number;
      status: ConfigurationOperationStatusDto;
      effect: ConfigurationEffectDto;
      message?: string;
      details_json?: string;
    }
  | { type: 'skill_catalog'; catalog_json: string }
  | { type: 'skill_document'; document_json: string }
  | { type: 'mcp_configuration_snapshot'; snapshot_json: string }
  | { type: 'plugin_catalog'; catalog_json: string }
  | { type: 'mcp_servers'; servers: McpServerDto[] }
  | { type: 'skills'; skills: SkillDto[] }
  | { type: 'hooks'; hooks: HookDto[] }
  | { type: 'agents'; agents: AgentDto[] }
  | { type: 'slash_command_catalog'; commands: SlashCommandDto[] }
  | { type: 'slash_command_result'; turn_id?: number; display: string; is_error?: boolean }
  | { type: 'memory_entries'; entries: MemoryEntryDto[] }
  | { type: 'status_snapshot'; snapshot: StatusSnapshotDto }
  | {
      type: 'settings_snapshot';
      effective_json: string;
      provenance_json: string;
      /**
       * `[{layer, path, exists, parsed, parse_error?}]` — the on-disk state
       * of every settings file layer, so the UI can show which file backs a
       * layer and whether it parsed. `parsed` reports JSON validity only; it
       * says nothing about OS write permission.
       */
      files_json?: string;
      /**
       * `{key: value}` — the FILE-LAYER values as read at session start. NOT
       * the session's live configuration: no `cli`/`managed`/`env` overlay is
       * applied, so this can differ from `effective_json` both because of an
       * on-disk edit not yet picked up and because `effective_json` carries
       * the managed overlay that this field does not.
       */
      active_json?: string;
      /** Top-level keys the managed layer locks; editable elsewhere is refused. */
      locked?: string[];
      /**
       * `{layer: {key: value}}` — each FILE layer's OWN raw settings map,
       * unmerged. `effective_json` is a cross-layer merge and `active_json`
       * is the file-layer merge without the managed overlay; neither can
       * stand in for "what does layer L's file itself say", which a layered
       * editor needs before writing back to one layer: `update_settings`
       * replaces a key WHOLESALE in one layer's file, so pre-merging a write
       * against `effective_json` (which can carry another layer's entries
       * for an object-valued key like `providers`) would silently fork that
       * other layer's data into whichever layer gets saved.
       */
      layers_json?: string;
      /**
       * The keys in `effective_json` whose value is a CROSS-LAYER union
       * rather than any one layer's value. The engine deep-merges or
       * concat-dedups a specific set of keys (`hooks`, `permissions`,
       * `providers`, `enabledPlugins`, `trustedDirectories`, … — its
       * `settings::schema::MERGE_STRATEGIES` table), so once more than one
       * layer contributes, the effective value belongs to no single layer and
       * `provenance_json` names only the highest-priority CONTRIBUTOR. The UI
       * must therefore not draw a single-layer provenance badge for a key
       * listed here — it says the value is merged across layers instead.
       *
       * Only keys the merge actually unioned appear: a deep-merge key whose
       * entries the winning layer entirely redefines is absent, because there
       * the winning layer's badge is honest.
       */
      merged_keys?: string[];
    }
  | { type: 'auth_state'; state: AuthStateDto }
  | { type: 'doctor_report'; report: DoctorReportDto }
  | { type: 'task_row'; task: TaskRowDto }
  | { type: 'task_list_complete'; request_id: string; active_count: number; error?: string }
  /** SDK task lifecycle receipt (`ClientEvent::TaskLifecycle`). The payload is the
   *  already-serialized `system` / `task_*` SDK record, forwarded verbatim. */
  | { type: 'task_lifecycle'; event_json: string }
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
      /** Failure reason accompanying a `failed` transition, when reported. */
      error?: string;
    }
  | {
      type: 'workflow_resumed';
      previous_task_id: string;
      task: TaskRowDto;
      run_id: string;
      origin_session_id?: string;
    }
  | { type: 'commands_changed'; commands: SlashCommandDto[] }
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
      /** Complete restored counters, including zeros; absent for live deltas. */
      is_snapshot?: boolean;
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
    }
  // ── AudioService (engine -> device operations/cancellation/capabilities) ──
  | { type: 'audio_request'; request: AudioOperationRequestDto }
  | { type: 'audio_cancel'; identity: AudioOperationIdDto }
  | { type: 'audio_capabilities_changed'; capabilities: AudioCapabilitySnapshotDto };

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
  /** Device audio is unknown until the client publishes its initial snapshot. */
  audio?: AudioCapabilitySnapshotDto;
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

// ─────────────────────────────────────────────────────────────────────────────
// Inline visualization host (bridge `visualization` request method)
// ─────────────────────────────────────────────────────────────────────────────

/** Host theme handed to a mount; token names are the upstream CSS variables. */
export interface VisualizationThemeDto {
  dark: boolean;
  tokens: Record<string, string>;
}

/** Parameters of the bridge `visualization` request, tagged by `op`. */
export type VisualizationRequest =
  | {
      op: 'mount';
      session_id: string;
      id: string;
      revision: number;
      theme: VisualizationThemeDto;
      locale: string;
      expanded: boolean;
    }
  | { op: 'serve'; path: string }
  | {
      op: 'write_state';
      token: string;
      generation: number;
      base_version: number;
      model_content: string;
      private_content: string;
    }
  | { op: 'unmount'; token: string }
  | { op: 'unmount_session'; session_id: string }
  | { op: 'list'; session_id: string }
  | { op: 'notices' };

/** A granted mount; `null` from the bridge means "unavailable". */
export interface VisualizationMountDto {
  token: string;
  generation: number;
  doc_url: string;
  title: string;
}

/** One scheme-handler response; the body is base64. */
export interface VisualizationServeDto {
  status: number;
  headers: Array<[string, string]>;
  body_base64: string;
}

/** Outcome of a compare-and-swap state write. */
export interface VisualizationStateWriteDto {
  saved: boolean;
  version: number;
  reason?: string;
  /** On `conflict`, the winning `{version, modelContent, privateContent}`. */
  current_state?: unknown;
}

/** One stored revision of a conversation. */
export interface VisualizationRevisionDto {
  id: string;
  revision: number;
  title: string;
  created_at_ms: number;
}
