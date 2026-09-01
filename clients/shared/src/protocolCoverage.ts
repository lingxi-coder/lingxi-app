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
import type { ClientCommand } from './protocol.js';
import type { ClientEvent } from './protocol.js';

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
  list_provider_credentials: true,
  set_provider_credential: true,
  delete_provider_credential: true,
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
  resolve_app_runtime_profile_selection: true,
  resolve_app_dependency_change_confirmation: true,
  resolve_app_profile_proposal: true,
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
  session_started: true,
  session_ended: true,
  session_resumed: true,
  session_list: true,
  session_agent_list: true,
  session_agent_transcript: true,
  session_agent_updated: true,
  session_agent_message: true,
  model_list: true,
  model_changed: true,
  permission_mode_changed: true,
  conversation_controls_changed: true,
  fast_mode_changed: true,
  provider_credential_status: true,
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
