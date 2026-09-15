import type {
  AppEventDto,
  ClientCommand,
  ClientEvent,
  ListingKindDto,
  PluginCommandDto,
} from '@lingxi/bridge-client';

export type DesktopDisposition = 'exposed' | 'host_private' | 'degraded' | 'not_applicable';

export const CLIENT_COMMAND_DISPOSITIONS = {
  cron_manage: 'exposed',
  send_prompt: 'host_private',
  cancel: 'host_private',
  attach_turn: 'not_applicable',
  resume_turn: 'not_applicable',
  pause_turn: 'not_applicable',
  approve_permission: 'host_private',
  deny_permission: 'host_private',
  set_permission_mode: 'exposed',
  set_typescript_lsp_mode: 'not_applicable',
  approve_computer_access: 'host_private',
  deny_computer_access: 'host_private',
  answer_ask_user_question: 'host_private',
  cancel_ask_user_question: 'host_private',
  list_provider_credentials: 'host_private',
  set_provider_credential: 'host_private',
  delete_provider_credential: 'host_private',
  test_provider_connection: 'host_private',
  set_model: 'exposed',
  list_models: 'exposed',
  get_conversation_controls: 'exposed',
  set_reasoning_selection: 'exposed',
  set_fast_mode: 'exposed',
  run_slash_command: 'exposed',
  refresh_listings: 'exposed',
  list_session_agents: 'exposed',
  load_session_agent_transcript: 'exposed',
  new_session: 'host_private',
  resume_session: 'host_private',
  list_sessions: 'exposed',
  fork_session: 'not_applicable',
  login: 'exposed',
  logout: 'exposed',
  force_compact: 'exposed',
  clear_session: 'host_private',
  task_list: 'exposed',
  task_output: 'exposed',
  task_stop: 'exposed',
  task_message: 'exposed',
  resume_workflow: 'not_applicable',
  list_apps: 'not_applicable',
  get_app_details: 'not_applicable',
  create_app: 'not_applicable',
  start_app: 'not_applicable',
  stop_app: 'not_applicable',
  restart_app: 'not_applicable',
  execute_app_bridge_request: 'not_applicable',
  resolve_app_ui_request: 'not_applicable',
  resolve_app_capability_request: 'not_applicable',
  resolve_app_dependency_change_confirmation: 'not_applicable',
  resolve_app_profile_proposal: 'not_applicable',
  resolve_app_runtime_profile_selection: 'not_applicable',
  plugin_command: 'not_applicable',
  reset_app_permissions: 'not_applicable',
  list_app_sessions: 'not_applicable',
  list_app_checkpoints: 'not_applicable',
  restore_app_checkpoint: 'not_applicable',
  delete_app: 'not_applicable',
  request_exit: 'not_applicable',
  update_settings: 'exposed',
  update_permission_rules: 'exposed',
  set_default_permission_mode: 'exposed',
  update_workspace_directories: 'exposed',
  upsert_mcp_server: 'exposed',
  remove_mcp_server: 'exposed',
  skill_admin: 'exposed',
  mcp_admin: 'exposed',
  plugin_admin: 'exposed',
  hook_admin: 'exposed',
  audio_response: 'exposed',
} as const satisfies Record<ClientCommand['type'], DesktopDisposition>;

export const REFRESH_LISTING_DISPOSITIONS = {
  sessions: 'host_private',
  models: 'exposed',
  mcp: 'exposed',
  skills: 'exposed',
  hooks: 'exposed',
  agents: 'exposed',
  slash_commands: 'exposed',
  memory: 'not_applicable',
  status: 'exposed',
  settings: 'exposed',
  auth: 'exposed',
  doctor: 'exposed',
  tasks: 'host_private',
} as const satisfies Record<ListingKindDto['type'], DesktopDisposition>;

export const CLIENT_EVENT_DISPOSITIONS = {
  cron_result: 'exposed',
  error: 'exposed',
  message_identity: 'exposed',
  message_retracted: 'exposed',
  system_notice: 'exposed',
  loop_wakeup: 'exposed',
  ask_user_question: 'exposed',
  ask_user_question_resolved: 'exposed',
  permission_request_resolved: 'exposed',
  text_delta: 'exposed',
  tool_use_started: 'exposed',
  tool_heartbeat: 'exposed',
  tool_use_result: 'exposed',
  plan_updated: 'exposed',
  message_complete: 'exposed',
  turn_started: 'exposed',
  turn_ended: 'exposed',
  turn_recovery_state: 'degraded',
  turn_event_replay: 'host_private',
  cost_update: 'exposed',
  compaction_completed: 'exposed',
  compaction_status: 'exposed',
  session_started: 'exposed',
  session_ended: 'exposed',
  session_resumed: 'exposed',
  session_forked: 'not_applicable',
  session_list: 'exposed',
  session_agent_list: 'exposed',
  session_agent_transcript: 'exposed',
  session_agent_updated: 'exposed',
  session_agent_message: 'exposed',
  model_list: 'exposed',
  provider_model_catalog: 'exposed',
  model_changed: 'exposed',
  permission_mode_changed: 'exposed',
  typescript_lsp_mode_changed: 'not_applicable',
  conversation_controls_changed: 'exposed',
  fast_mode_changed: 'exposed',
  provider_credential_status: 'host_private',
  provider_connection_tested: 'host_private',
  configuration_operation: 'exposed',
  skill_catalog: 'exposed',
  skill_document: 'exposed',
  mcp_configuration_snapshot: 'exposed',
  plugin_catalog: 'exposed',
  mcp_servers: 'exposed',
  skills: 'exposed',
  hooks: 'exposed',
  agents: 'exposed',
  slash_command_catalog: 'exposed',
  slash_command_result: 'exposed',
  // An additive SDK task-lifecycle receipt carrying opaque `event_json`.
  // Desktop neither reads it nor forwards it: nothing outside this table
  // references the name, and the ClientEvent switch in `main/bridge.ts`
  // ends in `default: break;`, so an unhandled event is dropped rather
  // than relayed to the renderer.
  task_lifecycle: 'not_applicable',
  memory_entries: 'not_applicable',
  status_snapshot: 'exposed',
  settings_snapshot: 'exposed',
  auth_state: 'exposed',
  doctor_report: 'exposed',
  task_row: 'exposed',
  task_output_chunk: 'exposed',
  task_status_changed: 'exposed',
  workflow_resumed: 'degraded',
  commands_changed: 'exposed',
  apps_changed: 'not_applicable',
  app_event: 'not_applicable',
  app_workflow_changed: 'not_applicable',
  app_runtime_changed: 'not_applicable',
  app_sessions_changed: 'not_applicable',
  app_checkpoint_created: 'not_applicable',
  app_operation_failed: 'not_applicable',
  coordinator_status: 'exposed',
  coordinator_worker: 'exposed',
  attachment: 'exposed',
  thinking_delta: 'exposed',
  usage_update: 'exposed',
  api_retry: 'exposed',
  audio_request: 'exposed',
} as const satisfies Record<ClientEvent['type'], DesktopDisposition>;

export const PLUGIN_COMMAND_DISPOSITIONS = {
  set_enabled: 'not_applicable',
  get_status: 'not_applicable',
  get_inventory: 'not_applicable',
  resolve_create_confirmation: 'not_applicable',
  resolve_mcp_proposal_approval: 'not_applicable',
  start_local_app_mcp_authoring: 'not_applicable',
  set_local_app_mcp_enabled: 'not_applicable',
  set_local_app_mcp_tool_enabled: 'not_applicable',
  set_local_app_mcp_conversation_pinned: 'not_applicable',
  get_managed_mcp_inventory: 'not_applicable',
} as const satisfies Record<PluginCommandDto['type'], DesktopDisposition>;

export const APP_EVENT_DISPOSITIONS = {
  app_details_changed: 'not_applicable',
  app_created: 'not_applicable',
  app_record_changed: 'not_applicable',
  app_profile_proposal: 'not_applicable',
  app_bridge_response: 'not_applicable',
  app_ui_request: 'not_applicable',
  app_capability_requested: 'not_applicable',
  app_dependency_change_confirmation_requested: 'not_applicable',
  app_checkpoints_changed: 'not_applicable',
  app_llm_activity_changed: 'not_applicable',
  app_agent_event_posted: 'not_applicable',
  app_background_task_changed: 'not_applicable',
  app_bridge_stream_frame: 'not_applicable',
  plugin_status_changed: 'not_applicable',
  plugin_inventory_changed: 'not_applicable',
  create_confirmation_requested: 'not_applicable',
  mcp_proposal_approval_requested: 'not_applicable',
  managed_mcp_inventory_changed: 'not_applicable',
  verification_summary_changed: 'not_applicable',
  local_app_operation_failed: 'not_applicable',
} as const satisfies Record<AppEventDto['type'], DesktopDisposition>;

/**
 * The runtime-checkable source of truth for the bounded, Desktop-facing
 * command surface the main process accepts from the renderer. Both the
 * compile-time gate (`AllowedClientCommand` below) and the runtime gate
 * (`src/main/validation.ts`'s `ALLOWED_COMMANDS`) derive from this one array
 * — neither restates it — so the two gates cannot independently drift the
 * way they already had: this array used to list `new_session` and
 * `resume_session` as part of the allowed surface while the runtime gate had
 * never accepted either. That was latent only because the renderer creates
 * and resumes sessions through the dedicated `CH_SESSION_NEW` /
 * `CH_SESSION_OPEN` IPC channels, never through `command()` — verified by
 * grepping `src/renderer/` for both wire names before removing them here.
 * `validation.test.ts`'s exclusion test for both is the record of why they
 * stay out.
 */
export const ALLOWED_CLIENT_COMMAND_TYPES = [
  'cron_manage',
  'set_model',
  'list_models',
  'list_sessions',
  'login',
  'logout',
  'force_compact',
  'task_list',
  'task_output',
  'task_stop',
  'task_message',
  'list_session_agents',
  'load_session_agent_transcript',
  'set_permission_mode',
  'run_slash_command',
  'get_conversation_controls',
  'set_reasoning_selection',
  'set_fast_mode',
  'update_settings',
  'update_permission_rules',
  'set_default_permission_mode',
  'update_workspace_directories',
  'upsert_mcp_server',
  'remove_mcp_server',
  'skill_admin',
  'mcp_admin',
  'plugin_admin',
  'hook_admin',
  // The renderer's answer to `ClientEvent::AudioRequest` — the client side of
  // `audio_bridge.rs`'s `AudioBridge`. Unlike every other entry here it is
  // never sent because a user clicked something: the engine PARKS a call on a
  // deadline waiting for it (5s / 30s / 180s per op), so leaving it off this
  // array is not "one fewer feature", it is every microphone and
  // text-to-speech call in the product stalling and then failing.
  'audio_response',
] as const;

/**
 * The listing kinds `refresh_listings` may request — its own bounded
 * sub-surface, shared the same way as `ALLOWED_CLIENT_COMMAND_TYPES` above.
 */
export const ALLOWED_REFRESH_LISTING_KINDS = [
  'auth',
  'status',
  'doctor',
  'slash_commands',
  'settings',
  'mcp',
  'skills',
  'hooks',
  'agents',
] as const;

/**
 * The bounded, Desktop-facing command surface the main process accepts from
 * the renderer. Both the preload bridge (`src/preload/index.ts`) and the
 * renderer's ambient types (`src/renderer/bridge/lingxi.d.ts`) import this
 * single definition — it used to be declared twice and the two copies had
 * already drifted (preload was missing three command types the renderer
 * copy had).
 */
export type AllowedClientCommand =
  | Extract<ClientCommand, { type: (typeof ALLOWED_CLIENT_COMMAND_TYPES)[number] }>
  | {
      type: 'refresh_listings';
      which: Array<{ type: (typeof ALLOWED_REFRESH_LISTING_KINDS)[number] }>;
    };
