/**
 * Compile-time exhaustiveness closure over {@link ClientCommand} and
 * {@link ClientEvent}.
 *
 * `snapshots.test.ts`'s validators cast loaded golden JSON `as ClientCommand`
 * / `as ClientEvent` and then read fields off a `Record<string, unknown>` —
 * nothing there ties the validator's `switch` to the actual TS union, so a
 * Rust variant can gain a golden snapshot AND a validator `case` while the
 * union itself silently never learns the variant exists. That is exactly how
 * `attach_turn` / `resume_turn` / `pause_turn` (`ClientCommand`) and
 * `turn_recovery_state` / `turn_event_replay` (`ClientEvent`) drifted: fully
 * validated, fully snapshotted, absent from the union, and nothing caught it.
 *
 * A `Record<Union['type'], true>` closes the union<->literal direction at
 * COMPILE time in both senses:
 *  - a union member missing from the literal ⇒ "Property '<x>' is missing"
 *  - a literal key the union doesn't have ⇒ excess-property error on the key
 * The literal below can therefore only type-check when its key set is
 * EXACTLY `ClientCommand['type']` / `ClientEvent['type']`, no more, no less.
 *
 * This file lives under `src/` (unlike the test file) specifically so this
 * failure is a `npm run typecheck` failure, not merely a test-runner one —
 * `clients/shared/tsconfig.json` excludes `test/`, so a check placed only in
 * `snapshots.test.ts` would never be typechecked at all.
 *
 * `snapshots.test.ts` asserts at RUNTIME that each record's key set equals
 * the set of `type` values actually found in the on-disk golden snapshots,
 * closing the other direction: a golden whose `type` has no union member, or
 * a union member with no golden exercising it. (The comparison must read
 * each golden's `type` FIELD, not its filename: several `event/` goldens
 * share the `app_event` tag but are named after their inner `AppEventDto`
 * variant instead, e.g. `app_details_changed.json`.)
 */
import type {
  AppEventDto,
  ClientCommand,
  ClientEvent,
  LocalAppPluginErrorCodeDto,
  ManagedLocalAppMcpStatusDto,
  PluginCommandDto,
  TaskRowDto,
} from './protocol.js';

export const ALL_CLIENT_COMMAND_TYPES: Record<ClientCommand['type'], true> = {
  send_prompt: true,
  cancel: true,
  attach_turn: true,
  resume_turn: true,
  pause_turn: true,
  approve_permission: true,
  deny_permission: true,
  approve_computer_access: true,
  deny_computer_access: true,
  answer_ask_user_question: true,
  cancel_ask_user_question: true,
  set_permission_mode: true,
  set_typescript_lsp_mode: true,
  list_provider_credentials: true,
  set_provider_credential: true,
  delete_provider_credential: true,
  test_provider_connection: true,
  set_model: true,
  list_models: true,
  get_conversation_controls: true,
  set_reasoning_selection: true,
  set_fast_mode: true,
  run_slash_command: true,
  refresh_listings: true,
  list_session_agents: true,
  load_session_agent_transcript: true,
  new_session: true,
  resume_session: true,
  list_sessions: true,
  fork_session: true,
  login: true,
  logout: true,
  force_compact: true,
  clear_session: true,
  task_list: true,
  task_output: true,
  task_stop: true,
  resume_workflow: true,
  list_apps: true,
  get_app_details: true,
  create_app: true,
  start_app: true,
  stop_app: true,
  restart_app: true,
  execute_app_bridge_request: true,
  resolve_app_ui_request: true,
  resolve_app_capability_request: true,
  resolve_app_dependency_change_confirmation: true,
  resolve_app_profile_proposal: true,
  resolve_app_runtime_profile_selection: true,
  plugin_command: true,
  reset_app_permissions: true,
  list_app_sessions: true,
  list_app_checkpoints: true,
  restore_app_checkpoint: true,
  delete_app: true,
  request_exit: true,
  update_settings: true,
  update_permission_rules: true,
  set_default_permission_mode: true,
  update_workspace_directories: true,
  upsert_mcp_server: true,
  remove_mcp_server: true,
  skill_admin: true,
  mcp_admin: true,
  plugin_admin: true,
  hook_admin: true,
  audio_response: true,
};

export const ALL_CLIENT_EVENT_TYPES: Record<ClientEvent['type'], true> = {
  error: true,
  system_notice: true,
  ask_user_question: true,
  ask_user_question_resolved: true,
  permission_request_resolved: true,
  text_delta: true,
  tool_use_started: true,
  tool_heartbeat: true,
  tool_use_result: true,
  plan_updated: true,
  message_complete: true,
  turn_started: true,
  turn_ended: true,
  turn_recovery_state: true,
  turn_event_replay: true,
  cost_update: true,
  compaction_completed: true,
  compaction_status: true,
  session_started: true,
  session_ended: true,
  session_resumed: true,
  session_forked: true,
  session_list: true,
  session_agent_list: true,
  session_agent_transcript: true,
  session_agent_updated: true,
  session_agent_message: true,
  model_list: true,
  provider_model_catalog: true,
  model_changed: true,
  permission_mode_changed: true,
  typescript_lsp_mode_changed: true,
  conversation_controls_changed: true,
  fast_mode_changed: true,
  provider_credential_status: true,
  provider_connection_tested: true,
  configuration_operation: true,
  skill_catalog: true,
  skill_document: true,
  mcp_configuration_snapshot: true,
  plugin_catalog: true,
  mcp_servers: true,
  skills: true,
  hooks: true,
  agents: true,
  slash_command_catalog: true,
  slash_command_result: true,
  memory_entries: true,
  status_snapshot: true,
  settings_snapshot: true,
  auth_state: true,
  doctor_report: true,
  task_row: true,
  task_output_chunk: true,
  task_status_changed: true,
  workflow_resumed: true,
  commands_changed: true,
  apps_changed: true,
  app_event: true,
  app_workflow_changed: true,
  app_runtime_changed: true,
  app_sessions_changed: true,
  app_checkpoint_created: true,
  app_operation_failed: true,
  coordinator_status: true,
  coordinator_worker: true,
  attachment: true,
  thinking_delta: true,
  usage_update: true,
  api_retry: true,
  audio_request: true,
};

export const ALL_PLUGIN_COMMAND_TYPES: Record<PluginCommandDto['type'], true> = {
  set_enabled: true,
  get_status: true,
  get_inventory: true,
  resolve_create_confirmation: true,
  resolve_mcp_proposal_approval: true,
  start_local_app_mcp_authoring: true,
  set_local_app_mcp_enabled: true,
  set_local_app_mcp_tool_enabled: true,
  set_local_app_mcp_conversation_pinned: true,
  get_managed_mcp_inventory: true,
};

export const ALL_APP_EVENT_TYPES: Record<AppEventDto['type'], true> = {
  app_details_changed: true,
  app_created: true,
  app_record_changed: true,
  app_profile_proposal: true,
  app_bridge_response: true,
  app_ui_request: true,
  app_capability_requested: true,
  app_dependency_change_confirmation_requested: true,
  app_checkpoints_changed: true,
  app_llm_activity_changed: true,
  app_agent_event_posted: true,
  app_background_task_changed: true,
  app_bridge_stream_frame: true,
  plugin_status_changed: true,
  plugin_inventory_changed: true,
  create_confirmation_requested: true,
  mcp_proposal_approval_requested: true,
  managed_mcp_inventory_changed: true,
  verification_summary_changed: true,
  local_app_operation_failed: true,
};

export const ALL_MANAGED_LOCAL_APP_MCP_STATUS_TYPES: Record<ManagedLocalAppMcpStatusDto, true> = {
  disabled: true,
  needs_setup: true,
  authoring: true,
  enabled: true,
  needs_revalidation: true,
  error: true,
};

/**
 * The same closure as above, over `TaskRowDto`'s FIELD names rather than a
 * union's `type` tags — `listings.rs`'s `TaskRowDto` grew two fields on two
 * branches at once (`error`, the terminal failure reason, and `stage`, F005's
 * `/fusion` progress label), and nothing tied this
 * hand-maintained interface to the Rust struct it mirrors: dropping a field
 * here is a silent `npm test` pass (`tsx` transpiles without type-checking)
 * with no runtime symptom until a consumer tries to read the missing member.
 * A literal missing a `TaskRowDto` key fails with "Property '<x>' is
 * missing"; a literal key `TaskRowDto` doesn't have fails on the excess key.
 */
export const ALL_TASK_ROW_DTO_KEYS: Record<keyof TaskRowDto, true> = {
  task_id: true,
  task_type: true,
  status: true,
  description: true,
  can_resume: true,
  started_at_ms: true,
  error: true,
  stage: true,
};

export const ALL_LOCAL_APP_PLUGIN_ERROR_CODES: Record<LocalAppPluginErrorCodeDto, true> = {
  plugin_disabled: true,
  builtin_bundle_unavailable: true,
  template_unavailable: true,
  proposal_invalid: true,
  catalog_stale: true,
  active_state_corrupt: true,
  revision_conflict: true,
  invalid_mcp_settings: true,
  mcp_authoring_required: true,
  repair_budget_exhausted: true,
  exposure_capacity_reached: true,
};
