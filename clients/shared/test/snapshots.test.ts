/**
 * Wire-compatibility test: load the F1-08 golden snapshots (the canonical JSON
 * the Rust serde produces) and prove every one parses into the TypeScript wire
 * types in `../src/protocol.ts`.
 *
 * The snapshots ARE the contract: if a tag or field name drifts in the TS
 * mirror, the corresponding structural assertion here fails. Each snapshot is
 * validated by a hand-written runtime guard that mirrors the exact TS type, so
 * the test fails loudly on any divergence rather than silently `as`-casting.
 *
 * Coverage is enforced: the test asserts EVERY snapshot file under each category
 * directory is checked, so a newly-added Rust variant whose snapshot lands here
 * forces a TS-type + guard update.
 */

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

import type {
  ClientCommand,
  ClientError,
  ClientEvent,
  MessageDto,
  PermissionRequest,
  PermissionResolved,
} from '../src/protocol.js';

const here = dirname(fileURLToPath(import.meta.url));
const SNAP_ROOT = join(
  here,
  '..',
  '..',
  '..',
  'lingxi-code',
  'client-protocol',
  'snapshots',
);

function loadSnapshot<T>(category: string, name: string): T {
  const raw = readFileSync(join(SNAP_ROOT, category, name), 'utf8');
  return JSON.parse(raw) as T;
}

function listSnapshots(category: string): string[] {
  return readdirSync(join(SNAP_ROOT, category))
    .filter((f) => f.endsWith('.json'))
    .sort();
}

// ── Generic structural helpers ───────────────────────────────────────────────

const isString = (v: unknown): v is string => typeof v === 'string';
const isNumber = (v: unknown): v is number => typeof v === 'number';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

function rec(v: unknown): Record<string, unknown> {
  assert.equal(typeof v, 'object');
  assert.notEqual(v, null);
  return v as Record<string, unknown>;
}

/** Assert an internally-tagged value carries exactly the expected `type` tag. */
function tag(v: unknown, expected: string): Record<string, unknown> {
  const o = rec(v);
  assert.equal(o['type'], expected, `expected type="${expected}", got "${String(o['type'])}"`);
  return o;
}

// ── Per-DTO validators (mirror the TS unions exactly) ─────────────────────────

function validatePromptMode(v: unknown): void {
  const o = rec(v);
  assert.ok(['normal', 'bash', 'memory', 'plan'].includes(o['type'] as string));
}

function validateListingKind(v: unknown): void {
  const o = rec(v);
  assert.ok(
    [
      'sessions',
      'models',
      'mcp',
      'hooks',
      'agents',
      'slash_commands',
      'memory',
      'status',
      'settings',
      'auth',
      'doctor',
      'tasks',
    ].includes(o['type'] as string),
  );
}

function validatePermissionResponse(v: unknown): void {
  const o = rec(v);
  assert.ok(['allow_once', 'allow_always', 'deny'].includes(o['type'] as string));
}

function validateTaskStatus(v: unknown): void {
  const o = rec(v);
  assert.ok(
    ['pending', 'running', 'completed', 'failed', 'cancelled'].includes(o['type'] as string),
  );
}

function validateMessageBlock(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'text':
      assert.ok(isString(o['text']));
      break;
    case 'thinking':
      assert.ok(isString(o['thinking']));
      if ('signature' in o) assert.ok(isString(o['signature']));
      break;
    case 'redacted_thinking':
      assert.ok(isString(o['data']));
      break;
    case 'compact_boundary':
      assert.ok(
        isNumber(o['messages_before']) &&
          isNumber(o['messages_after']) &&
          isString(o['summary']),
      );
      break;
    case 'tool_use':
      assert.ok(isString(o['id']) && isString(o['tool']) && isString(o['input_json']));
      break;
    case 'tool_result':
      assert.ok(
        isString(o['id']) &&
          isString(o['tool']) &&
          isString(o['result_json']) &&
          isBool(o['is_error']),
      );
      for (const opt of ['old_string', 'new_string', 'file_path']) {
        if (opt in o) assert.ok(isString(o[opt]));
      }
      break;
    default:
      assert.fail(`unknown MessageBlockDto type: ${String(o['type'])}`);
  }
}

function validateMessage(v: unknown): void {
  const m = v as MessageDto;
  const o = rec(m);
  assert.ok(isString(o['role']));
  assert.ok(Array.isArray(o['blocks']));
  for (const b of o['blocks'] as unknown[]) validateMessageBlock(b);
}

function validateCost(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isNumber(o['total_usd']) &&
      isNumber(o['input_tokens']) &&
      isNumber(o['output_tokens']) &&
      isNumber(o['api_calls']) &&
      isNumber(o['session_duration_secs']) &&
      isString(o['formatted']),
  );
}

function validatePermissionMode(v: unknown): void {
  assert.ok(
    ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'].includes(
      v as string,
    ),
  );
}

// ── local_apps.rs validators (bare-string enums + kind/op-tagged DTOs) ────────

function validateAppWorkflowState(v: unknown): void {
  assert.ok(
    [
      'authoring_questionnaire',
      'questionnaire_failed',
      'collecting_spec',
      'planning',
      'plan_failed',
      'awaiting_spec_confirmation',
      'generating',
      'validating',
      'awaiting_preview_confirmation',
      'revising',
      'ready',
      'generation_failed',
      'validation_failed',
    ].includes(v as string),
  );
}

function validateAppRuntimeState(v: unknown): void {
  assert.ok(['stopped', 'starting', 'running', 'stopping', 'failed'].includes(v as string));
}

function validateAppCreateOrigin(v: unknown): void {
  assert.ok(['chat', 'library'].includes(v as string));
}

function validateAppErrorCode(v: unknown): void {
  assert.ok(
    [
      'not_found',
      'revision_conflict',
      'interaction_invalid',
      'workflow_state_invalid',
      'runtime_busy',
      'not_yet_available',
      'storage_corrupt',
      'invalid_request',
      'io',
      'llm_unavailable',
      'llm_output_rejected',
    ].includes(v as string),
  );
}

function validateAppCheckpointKind(v: unknown): void {
  assert.ok(
    [
      'scaffold_created',
      'generation_validated',
      'preview_approved',
      'user_approved',
      'pre_restore',
    ].includes(v as string),
  );
}

function validateDesignValue(v: unknown): void {
  const o = rec(v);
  switch (o['kind']) {
    case 'short_text':
    case 'long_text':
    case 'single_choice':
    case 'color':
      assert.ok(isString(o['value']));
      break;
    case 'multiple_choice':
    case 'screen_list':
    case 'feature_list':
    case 'domain_list':
      assert.ok(Array.isArray(o['value']));
      for (const item of o['value'] as unknown[]) assert.ok(isString(item));
      break;
    case 'data_field_list':
      assert.ok(Array.isArray(o['value']));
      for (const item of o['value'] as unknown[]) validateAppDataField(item);
      break;
    case 'boolean':
      assert.ok(isBool(o['value']));
      break;
    case 'density':
      assert.ok(['compact', 'comfortable'].includes(o['value'] as string));
      break;
    case 'deferred':
      break;
    default:
      assert.fail(`unknown DesignValueDto kind: ${String(o['kind'])}`);
  }
}

function validateDesignPatch(v: unknown): void {
  const o = rec(v);
  assert.ok(Array.isArray(o['ops']));
  for (const op of o['ops'] as unknown[]) {
    const p = rec(op);
    switch (p['op']) {
      case 'set':
        assert.ok(isString(p['field_id']));
        validateDesignValue(p['value']);
        break;
      case 'remove':
        assert.ok(isString(p['field_id']));
        break;
      default:
        assert.fail(`unknown AppDesignPatchOpDto op: ${String(p['op'])}`);
    }
  }
  if ('note' in o) assert.ok(isString(o['note']));
}

function validateAppDataField(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && isString(o['label']) && isBool(o['required']));
  assert.ok(
    [
      'text',
      'long_text',
      'integer',
      'decimal',
      'boolean',
      'date_time',
      'enum',
      'image_ref',
    ].includes(o['field_type'] as string),
  );
  assert.ok(Array.isArray(o['options']));
  for (const opt of o['options'] as unknown[]) assert.ok(isString(opt));
}

function validateAppDataCollection(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && isString(o['label']) && isBool(o['enabled_by_default']));
  assert.ok(Array.isArray(o['fields']));
  for (const f of o['fields'] as unknown[]) validateAppDataField(f);
}

function validateAppDesignField(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['id']) &&
      isString(o['label']) &&
      isBool(o['required']) &&
      isBool(o['allows_custom']) &&
      isBool(o['allows_defer']),
  );
  assert.ok(
    [
      'short_text',
      'long_text',
      'single_choice',
      'multiple_choice',
      'boolean',
      'color',
      'density',
      'screen_list',
      'feature_list',
      'data_field_list',
      'domain_list',
    ].includes(o['field_type'] as string),
  );
  if ('description' in o) assert.ok(isString(o['description']));
  if ('default_value' in o) validateDesignValue(o['default_value']);
  assert.ok(Array.isArray(o['options']));
  for (const opt of o['options'] as unknown[]) {
    const p = rec(opt);
    assert.ok(isString(p['value']) && isString(p['label']));
  }
}

function validateAppDesignStep(v: unknown): void {
  const s = rec(v);
  assert.ok(isString(s['id']) && isNumber(s['order']) && isString(s['title']));
  if ('description' in s) assert.ok(isString(s['description']));
  assert.ok(Array.isArray(s['fields']));
  for (const f of s['fields'] as unknown[]) validateAppDesignField(f);
}

const APP_CAPABILITY_KINDS = [
  'data_mutation',
  'ui_control',
  'network_domain',
  'restore_checkpoint',
  'camera',
  'photo_library',
  'microphone',
  'location',
  'notifications',
  'llm',
  'agent_notify',
];

function validateAppPlan(v: unknown): void {
  const o = rec(v);
  assert.ok(Array.isArray(o['collections']));
  for (const c of o['collections'] as unknown[]) validateAppDataCollection(c);
  assert.ok(Array.isArray(o['capabilities']));
  for (const cap of o['capabilities'] as unknown[]) {
    assert.ok(APP_CAPABILITY_KINDS.includes(cap as string));
  }
  assert.ok(Array.isArray(o['domains']));
  for (const d of o['domains'] as unknown[]) assert.ok(isString(d));
  assert.ok(isString(o['summary']));
}

function validateAppRuntimeDetails(v: unknown): void {
  const o = rec(v);
  validateAppRuntimeState(o['state']);
  if ('mode' in o) {
    assert.ok(['static_export', 'next_production'].includes(o['mode'] as string));
  }
  if ('loopback_url' in o) assert.ok(isString(o['loopback_url']));
  if ('suspension_reason' in o) {
    assert.ok(
      ['backgrounded', 'memory_warning', 'runtime_quota', 'process_exited'].includes(
        o['suspension_reason'] as string,
      ),
    );
  }
  if ('recovery_state' in o) {
    assert.ok(
      ['not_needed', 'pending', 'recovering', 'recovered', 'failed'].includes(
        o['recovery_state'] as string,
      ),
    );
  }
  if ('last_error' in o) assert.ok(isString(o['last_error']));
}

function validateAppGenerationJob(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['id']) &&
      isString(o['app_id']) &&
      isNumber(o['revision']) &&
      isNumber(o['continuation_seq']) &&
      isNumber(o['updated_at_ms']),
  );
  assert.ok(
    [
      'queued',
      'scaffolding',
      'generating',
      'validating',
      'building',
      'starting_preview',
      'awaiting_approval',
      'succeeded',
      'failed',
      'cancelled',
    ].includes(o['state'] as string),
  );
  if ('percent' in o) assert.ok(isNumber(o['percent']));
  if ('detail' in o) assert.ok(isString(o['detail']));
  if ('log_rel' in o) assert.ok(isString(o['log_rel']));
}

function validateAppManifest(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isNumber(o['schema_version']) &&
      isString(o['app_id']) &&
      isString(o['name']) &&
      isNumber(o['design_revision']),
  );
  assert.ok(Array.isArray(o['collections']));
  for (const c of o['collections'] as unknown[]) validateAppDataCollection(c);
  assert.ok(Array.isArray(o['allowed_domains']));
  for (const d of o['allowed_domains'] as unknown[]) assert.ok(isString(d));
  assert.ok(Array.isArray(o['capabilities']));
  for (const cap of o['capabilities'] as unknown[]) {
    assert.ok(APP_CAPABILITY_KINDS.includes(cap as string));
  }
}

function validateAppBridgeRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['request_id']) && isString(o['app_id']));
  assert.ok(
    [
      'query_data',
      'mutate_data',
      'network_request',
      'runtime_status',
      'capture_photo',
      'pick_image',
      'record_audio_start',
      'record_audio_stop',
      'get_location',
      'transcribe_speech',
      'post_notification',
      'llm_chat',
      'agent_post',
    ].includes(o['operation'] as string),
  );
  if ('payload_json' in o) assert.ok(isString(o['payload_json']));
}

function validateAppBridgeResponse(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['request_id']) && isString(o['app_id']) && isBool(o['ok']));
  if ('result_json' in o) assert.ok(isString(o['result_json']));
  if ('error' in o) assert.ok(isString(o['error']));
  if ('error_code' in o) assert.ok(isString(o['error_code']));
}

function validateAppUiRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['request_id']) && isString(o['app_id']));
  assert.ok(
    [
      'inspect',
      'click',
      'fill',
      'select',
      'toggle',
      'scroll',
      'navigate',
      'back',
      'reload',
    ].includes(o['action'] as string),
  );
  if ('target' in o) {
    const target = rec(o['target']);
    for (const key of ['element_id', 'role', 'name']) {
      if (key in target) assert.ok(isString(target[key]));
    }
  }
  if ('value' in o) assert.ok(isString(o['value']));
}

function validateAppCapabilityRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['request_id']) && isString(o['app_id']) && isString(o['reason']));
  assert.ok(APP_CAPABILITY_KINDS.includes(o['capability'] as string));
  if ('domain' in o) assert.ok(isString(o['domain']));
}

function validateAppAuthorizationDecision(v: unknown): void {
  assert.ok(['deny', 'allow_once', 'allow_session', 'allow_always'].includes(v as string));
}

function validateAppRecord(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['id']) &&
      isString(o['name']) &&
      isString(o['brief']) &&
      isNumber(o['created_at_ms']) &&
      isNumber(o['updated_at_ms']) &&
      isString(o['workspace_rel']),
  );
  validateAppWorkflowState(o['workflow_state']);
  if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
}

function validateAppCheckpoint(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && isString(o['label']) && isNumber(o['created_at_ms']));
  validateAppCheckpointKind(o['kind']);
}

function validateAppDetails(v: unknown): void {
  const o = rec(v);
  validateAppRecord(o['app']);
  assert.ok(isNumber(o['design_revision']));
  assert.ok(Array.isArray(o['design_fields']));
  for (const pair of o['design_fields'] as unknown[]) {
    const p = rec(pair);
    assert.ok(isString(p['field_id']));
    validateDesignValue(p['value']);
  }
  assert.ok(Array.isArray(o['questionnaire']));
  for (const step of o['questionnaire'] as unknown[]) validateAppDesignStep(step);
  if ('plan' in o) validateAppPlan(o['plan']);
  if ('manifest' in o) validateAppManifest(o['manifest']);
  validateAppRuntimeDetails(o['runtime']);
  if ('generation_job' in o) validateAppGenerationJob(o['generation_job']);
  assert.ok(Array.isArray(o['checkpoints']));
  for (const c of o['checkpoints'] as unknown[]) validateAppCheckpoint(c);
}

function validateAppEvent(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'app_details_changed':
      validateAppDetails(o['details']);
      break;
    case 'app_questionnaire_changed':
      assert.ok(isString(o['app_id']) && isNumber(o['revision']));
      assert.ok(Array.isArray(o['steps']));
      for (const step of o['steps'] as unknown[]) validateAppDesignStep(step);
      break;
    case 'app_plan_changed':
      assert.ok(isString(o['app_id']) && isNumber(o['revision']));
      if ('plan' in o) validateAppPlan(o['plan']);
      break;
    case 'app_generation_job_changed':
      validateAppGenerationJob(o['job']);
      break;
    case 'app_bridge_response':
      validateAppBridgeResponse(o['response']);
      break;
    case 'app_ui_request':
      validateAppUiRequest(o['request']);
      break;
    case 'app_capability_requested':
      validateAppCapabilityRequest(o['request']);
      break;
    case 'app_checkpoints_changed':
      assert.ok(isString(o['app_id']) && Array.isArray(o['checkpoints']));
      for (const c of o['checkpoints'] as unknown[]) validateAppCheckpoint(c);
      break;
    case 'app_llm_activity_changed':
      assert.ok(isString(o['app_id']) && isBool(o['active']));
      break;
    case 'app_agent_event_posted':
      assert.ok(
        isString(o['app_id']) &&
          isNumber(o['seq']) &&
          isString(o['topic']) &&
          isNumber(o['created_at_ms']),
      );
      break;
    default:
      assert.fail(`unknown AppEventDto type: ${String(o['type'])}`);
  }
}

function validateAskUserQuestionRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(isNumber(o['request_id']));
  if ('timeout_secs' in o) assert.ok(isNumber(o['timeout_secs']));
  assert.ok(Array.isArray(o['questions']));
  for (const q of o['questions'] as unknown[]) {
    const question = rec(q);
    assert.ok(
      isString(question['question']) &&
        isString(question['header']) &&
        isBool(question['multi_select']),
    );
    assert.ok(Array.isArray(question['options']));
    for (const opt of question['options'] as unknown[]) {
      const option = rec(opt);
      assert.ok(isString(option['label']) && isString(option['description']));
      if ('preview' in option) assert.ok(isString(option['preview']));
    }
  }
}

function validateAttachment(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'nested_memory':
      assert.ok(isString(o['display_path']));
      break;
    default:
      assert.fail(`unknown AttachmentDto type: ${String(o['type'])}`);
  }
}

// ── ClientCommand ─────────────────────────────────────────────────────────────

function validateCommand(name: string, v: unknown): void {
  const c = v as ClientCommand;
  const o = rec(c);
  switch (o['type']) {
    case 'send_prompt':
      assert.ok(isString(o['text']));
      assert.ok(Array.isArray(o['images']));
      for (const img of o['images'] as unknown[]) {
        const i = rec(img);
        assert.ok(isString(i['media_type']) && isString(i['base64']));
      }
      if ('prompt_mode' in o) validatePromptMode(o['prompt_mode']);
      if ('turn_id' in o) assert.ok(isNumber(o['turn_id']));
      break;
    case 'cancel':
      if ('turn_id' in o) assert.ok(isNumber(o['turn_id']));
      break;
    case 'approve_permission':
      assert.ok(isNumber(o['request_id']));
      validatePermissionResponse(o['response']);
      break;
    case 'deny_permission':
      assert.ok(isNumber(o['request_id']));
      break;
    case 'approve_computer_access':
      assert.ok(isNumber(o['request_id']));
      {
        const response = rec(o['response']);
        assert.ok(Array.isArray(response['granted_apps']));
        for (const label of response['granted_apps'] as unknown[]) assert.ok(isString(label));
        assert.ok(
          typeof response['clipboard_read'] === 'boolean' &&
            typeof response['clipboard_write'] === 'boolean' &&
            typeof response['system_key_combos'] === 'boolean',
        );
      }
      break;
    case 'deny_computer_access':
      assert.ok(isNumber(o['request_id']));
      break;
    case 'answer_ask_user_question':
      assert.ok(isNumber(o['request_id']));
      {
        const answers = rec(o['answers']);
        for (const [question, answer] of Object.entries(answers)) {
          assert.ok(isString(question) && isString(answer));
        }
      }
      break;
    case 'cancel_ask_user_question':
      assert.ok(isNumber(o['request_id']));
      break;
    case 'set_permission_mode':
      validatePermissionMode(o['mode']);
      break;
    case 'list_provider_credentials':
      assert.ok(isNumber(o['operation_id']) && Array.isArray(o['provider_ids']));
      for (const providerId of o['provider_ids'] as unknown[]) assert.ok(isString(providerId));
      break;
    case 'set_provider_credential':
      assert.ok(
        isNumber(o['operation_id']) &&
          isString(o['provider_id']) &&
          isString(o['credential']),
      );
      break;
    case 'delete_provider_credential':
      assert.ok(isNumber(o['operation_id']) && isString(o['provider_id']));
      break;
    case 'set_model':
      assert.ok(isString(o['model']));
      break;
    case 'list_models':
      break;
    case 'run_slash_command':
      assert.ok(isString(o['raw']));
      break;
    case 'refresh_listings':
      assert.ok(Array.isArray(o['which']));
      for (const k of o['which'] as unknown[]) validateListingKind(k);
      break;
    case 'new_session':
      if ('cwd' in o) assert.ok(isString(o['cwd']));
      if ('model' in o) assert.ok(isString(o['model']));
      break;
    case 'resume_session':
      assert.ok(isString(o['session_id']));
      if ('cwd' in o) assert.ok(isString(o['cwd']));
      break;
    case 'list_sessions':
      if ('limit' in o) assert.ok(isNumber(o['limit']));
      break;
    case 'login':
    case 'logout':
    case 'force_compact':
    case 'clear_session':
    case 'request_exit':
      break;
    case 'task_list':
      if ('status_filter' in o) validateTaskStatus(o['status_filter']);
      break;
    case 'task_output':
      assert.ok(isString(o['task_id']) && isNumber(o['offset']));
      break;
    case 'task_stop':
      assert.ok(isString(o['task_id']));
      break;
    case 'list_apps':
      break;
    case 'create_app':
      assert.ok(isString(o['name']) && isString(o['brief']));
      validateAppCreateOrigin(o['origin']);
      if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
      break;
    case 'update_app_brief':
      assert.ok(isString(o['app_id']) && isString(o['brief']));
      break;
    case 'open_app_designer':
    case 'cancel_app_design':
    case 'start_app':
    case 'stop_app':
    case 'restart_app':
    case 'get_app_details':
    case 'retry_app_generation':
    case 'retry_app_questionnaire':
    case 'begin_app_planning':
    case 'retry_app_plan':
    case 'reset_app_permissions':
    case 'list_app_checkpoints':
    case 'delete_app':
      assert.ok(isString(o['app_id']));
      break;
    case 'update_app_design_draft':
      assert.ok(isString(o['app_id']) && isNumber(o['expected_revision']));
      validateDesignPatch(o['patch']);
      break;
    case 'apply_agent_design_suggestion':
      assert.ok(
        isString(o['app_id']) &&
          isString(o['suggestion_id']) &&
          isNumber(o['expected_revision']),
      );
      break;
    case 'confirm_app_design':
    case 'confirm_app_preview':
      assert.ok(
        isString(o['app_id']) && isNumber(o['revision']) && isString(o['interaction_id']),
      );
      break;
    case 'request_app_revision':
      assert.ok(isString(o['app_id']) && isString(o['prompt']));
      break;
    case 'request_app_design_suggestion':
      assert.ok(isString(o['app_id']) && isNumber(o['expected_revision']));
      if ('prompt' in o) assert.ok(isString(o['prompt']));
      break;
    case 'dismiss_app_design_suggestion':
      assert.ok(isString(o['app_id']) && isString(o['suggestion_id']));
      break;
    case 'execute_app_bridge_request':
      validateAppBridgeRequest(o['request']);
      break;
    case 'resolve_app_ui_request':
      assert.ok(isString(o['request_id']));
      validateAppAuthorizationDecision(o['decision']);
      if ('result_json' in o) assert.ok(isString(o['result_json']));
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'resolve_app_capability_request':
      assert.ok(isString(o['request_id']));
      validateAppAuthorizationDecision(o['decision']);
      break;
    case 'restore_app_checkpoint':
      assert.ok(isString(o['app_id']) && isString(o['checkpoint_id']));
      break;
    default:
      assert.fail(`snapshot ${name}: unknown ClientCommand type "${String(o['type'])}"`);
  }
}

// ── ClientEvent ───────────────────────────────────────────────────────────────

function validateEvent(name: string, v: unknown): void {
  const e = v as ClientEvent;
  const o = rec(e);
  switch (o['type']) {
    case 'error':
      tag(o['kind'], (rec(o['kind'])['type']) as string);
      assert.ok(
        ['transport', 'protocol', 'server', 'max_turns', 'rejected', 'internal'].includes(
          rec(o['kind'])['type'] as string,
        ),
      );
      assert.ok(isString(o['message']));
      break;
    case 'system_notice':
      assert.ok(isString(o['message']) && isBool(o['is_error']));
      break;
    case 'ask_user_question':
      validateAskUserQuestionRequest(o['request']);
      break;
    case 'ask_user_question_resolved':
      assert.ok(isNumber(o['request_id']));
      break;
    case 'attachment':
      validateAttachment(o['attachment']);
      break;
    case 'commands_changed':
      assert.ok(Array.isArray(o['commands']));
      for (const cmd of o['commands'] as unknown[]) {
        const s = rec(cmd);
        assert.ok(isString(s['name']) && isString(s['description']) && isString(s['source']));
      }
      break;
    case 'text_delta':
      assert.ok(isString(o['text']));
      break;
    case 'tool_use_started':
      assert.ok(isString(o['id']) && isString(o['tool']) && isString(o['input_json']));
      break;
    case 'tool_heartbeat':
      assert.ok(isString(o['id']) && isString(o['tool']) && isNumber(o['elapsed_ms']));
      break;
    case 'tool_use_result':
      assert.ok(
        isString(o['id']) &&
          isString(o['tool']) &&
          isString(o['result_json']) &&
          isBool(o['is_error']),
      );
      break;
    case 'message_complete':
      if ('stop_reason' in o) assert.ok(isString(o['stop_reason']));
      if ('message' in o) validateMessage(o['message']);
      break;
    case 'turn_started':
      if ('turn_id' in o) assert.ok(isNumber(o['turn_id']));
      break;
    case 'turn_ended':
      assert.ok(['end_turn', 'max_turns', 'cancelled'].includes(rec(o['outcome'])['type'] as string));
      if ('stop_reason' in o) assert.ok(isString(o['stop_reason']));
      validateCost(o['cost']);
      break;
    case 'cost_update':
      validateCost(o);
      break;
    case 'compaction_completed':
      assert.ok(
        isNumber(o['messages_before']) &&
          isNumber(o['messages_after']) &&
          isNumber(o['bytes_saved']),
      );
      break;
    case 'session_started':
      assert.ok(isString(o['session_id']));
      break;
    case 'session_resumed':
      assert.ok(isString(o['session_id']));
      // `messages` is REQUIRED (Vec<MessageDto>, no skip_serializing_if) — it
      // carries the full restored transcript (oldest-first); may be empty.
      assert.ok(Array.isArray(o['messages']));
      for (const m of o['messages'] as unknown[]) validateMessage(m);
      break;
    case 'session_ended':
      break;
    case 'session_list':
      assert.ok(Array.isArray(o['sessions']));
      for (const s of o['sessions'] as unknown[]) {
        const r = rec(s);
        assert.ok(
          isString(r['uuid']) &&
            isString(r['title']) &&
            isString(r['modified_rfc3339']) &&
            isNumber(r['message_count']) &&
            isString(r['path']),
        );
      }
      break;
    case 'model_list':
      assert.ok(Array.isArray(o['models']) && isString(o['current']));
      break;
    case 'model_changed':
      assert.ok(isString(o['model']));
      break;
    case 'permission_mode_changed':
      validatePermissionMode(o['mode']);
      break;
    case 'provider_credential_status':
      assert.ok(
        isNumber(o['operation_id']) &&
          Array.isArray(o['configured_provider_ids']) &&
          isBool(o['storage_encrypted']),
      );
      for (const providerId of o['configured_provider_ids'] as unknown[]) assert.ok(isString(providerId));
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'mcp_servers':
      assert.ok(Array.isArray(o['servers']));
      for (const s of o['servers'] as unknown[]) {
        const r = rec(s);
        assert.ok(isString(r['name']) && isString(r['transport']));
        const status = rec(r['status']);
        assert.ok(['connected', 'disconnected', 'error'].includes(status['type'] as string));
        if (status['type'] === 'error') assert.ok(isString(status['reason']));
      }
      break;
    case 'hooks':
      assert.ok(Array.isArray(o['hooks']));
      for (const h of o['hooks'] as unknown[]) {
        const r = rec(h);
        assert.ok(isString(r['name']) && isString(r['event']) && isNumber(r['timeout_ms']));
        if ('matcher' in r) assert.ok(isString(r['matcher']));
      }
      break;
    case 'agents':
      assert.ok(Array.isArray(o['agents']));
      for (const a of o['agents'] as unknown[]) {
        const r = rec(a);
        assert.ok(isString(r['name']) && isString(r['description']) && Array.isArray(r['tools_allowed']));
      }
      break;
    case 'slash_command_catalog':
      assert.ok(Array.isArray(o['commands']));
      for (const cmd of o['commands'] as unknown[]) {
        const r = rec(cmd);
        assert.ok(isString(r['name']) && isString(r['description']) && isString(r['source']));
      }
      break;
    case 'memory_entries':
      assert.ok(Array.isArray(o['entries']));
      for (const m of o['entries'] as unknown[]) {
        const r = rec(m);
        assert.ok(
          isString(r['path']) &&
            isString(r['body']) &&
            isNumber(r['age_days']) &&
            isNumber(r['size_bytes']),
        );
        assert.ok(['session', 'project', 'team', 'user'].includes(rec(r['tier'])['type'] as string));
      }
      break;
    case 'status_snapshot': {
      const s = rec(o['snapshot']);
      assert.ok(
        isString(s['session_id']) &&
          isString(s['model']) &&
          isNumber(s['n_messages']) &&
          isNumber(s['total_cost_usd']) &&
          isNumber(s['input_tokens']) &&
          isNumber(s['output_tokens']) &&
          isNumber(s['n_mcp_connected']) &&
          isNumber(s['n_mcp_total']) &&
          isNumber(s['n_hooks']) &&
          isNumber(s['n_agents']) &&
          isString(s['started_at']) &&
          isString(s['cwd']),
      );
      if ('status_line' in s) assert.ok(isString(s['status_line']));
      break;
    }
    case 'settings_snapshot':
      assert.ok(isString(o['effective_json']) && isString(o['provenance_json']));
      break;
    case 'auth_state': {
      const st = rec(o['state']);
      assert.ok(['signed_out', 'signed_in'].includes(st['type'] as string));
      if (st['type'] === 'signed_in') assert.ok(isString(st['email']) && isString(st['org_id']));
      break;
    }
    case 'doctor_report': {
      const r = rec(o['report']);
      assert.ok(Array.isArray(r['checks']));
      for (const c of r['checks'] as unknown[]) {
        const ch = rec(c);
        assert.ok(isString(ch['name']));
        assert.ok(['pass', 'warn', 'fail'].includes(rec(ch['status'])['type'] as string));
        if ('detail' in ch) assert.ok(isString(ch['detail']));
      }
      const sum = rec(r['summary']);
      assert.ok(isNumber(sum['passed']) && isNumber(sum['warnings']) && isNumber(sum['failed']));
      break;
    }
    case 'task_row': {
      const t = rec(o['task']);
      assert.ok(isString(t['task_id']) && isString(t['task_type']) && isString(t['description']));
      validateTaskStatus(t['status']);
      break;
    }
    case 'task_output_chunk':
      assert.ok(
        isString(o['task_id']) &&
          isString(o['content']) &&
          isNumber(o['total_lines']) &&
          isBool(o['truncated']),
      );
      break;
    case 'task_status_changed':
      assert.ok(isString(o['task_id']));
      validateTaskStatus(o['status']);
      break;
    case 'apps_changed':
      assert.ok(Array.isArray(o['apps']));
      for (const app of o['apps'] as unknown[]) validateAppRecord(app);
      break;
    case 'app_event':
      validateAppEvent(o['event']);
      break;
    case 'app_designer_requested':
      assert.ok(
        isString(o['app_id']) && isString(o['interaction_id']) && isNumber(o['revision']),
      );
      break;
    case 'app_design_draft_changed': {
      assert.ok(isString(o['app_id']) && isNumber(o['revision']));
      const fields = rec(o['fields']);
      for (const value of Object.values(fields)) validateDesignValue(value);
      break;
    }
    case 'app_design_suggestion_available':
      assert.ok(
        isString(o['app_id']) &&
          isString(o['suggestion_id']) &&
          isNumber(o['based_on_revision']),
      );
      validateDesignPatch(o['patch']);
      break;
    case 'app_design_conflict':
      assert.ok(
        isString(o['app_id']) &&
          isNumber(o['expected_revision']) &&
          isNumber(o['actual_revision']),
      );
      break;
    case 'app_workflow_changed':
      assert.ok(isString(o['app_id']));
      validateAppWorkflowState(o['state']);
      if ('detail' in o) assert.ok(isString(o['detail']));
      break;
    case 'app_generation_progress':
      assert.ok(isString(o['app_id']) && isString(o['stage']));
      if ('percent' in o) assert.ok(isNumber(o['percent']));
      if ('detail' in o) assert.ok(isString(o['detail']));
      break;
    case 'app_runtime_changed':
      assert.ok(isString(o['app_id']));
      validateAppRuntimeState(o['state']);
      if ('details' in o) validateAppRuntimeDetails(o['details']);
      if ('last_error' in o) assert.ok(isString(o['last_error']));
      break;
    case 'app_preview_ready':
      assert.ok(
        isString(o['app_id']) && isString(o['interaction_id']) && isNumber(o['revision']),
      );
      if ('url' in o) assert.ok(isString(o['url']));
      break;
    case 'app_checkpoint_created':
      assert.ok(isString(o['app_id']));
      validateAppCheckpoint(o['checkpoint']);
      break;
    case 'app_operation_failed':
      if ('app_id' in o) assert.ok(isString(o['app_id']));
      validateAppErrorCode(o['code']);
      assert.ok(isString(o['message']));
      break;
    case 'coordinator_status':
      assert.ok(isNumber(o['active_workers']));
      if ('team' in o) assert.ok(isString(o['team']));
      break;
    case 'coordinator_worker': {
      const w = rec(o['worker']);
      assert.ok(
        isString(w['agent_id']) &&
          isString(w['name']) &&
          isString(w['agent_type']) &&
          isString(w['status']),
      );
      break;
    }
    case 'thinking_delta':
      assert.ok(isString(o['thinking']));
      if ('signature' in o) assert.ok(isString(o['signature']));
      break;
    case 'usage_update':
      assert.ok(
        isNumber(o['input_tokens']) &&
          isNumber(o['output_tokens']) &&
          isNumber(o['cache_read_tokens']) &&
          isNumber(o['cache_creation_tokens']),
      );
      break;
    case 'api_retry':
      assert.ok(
        isString(o['message']) &&
          isNumber(o['attempt']) &&
          isNumber(o['max_retries']) &&
          isNumber(o['delay_ms']),
      );
      break;
    default:
      assert.fail(`snapshot ${name}: unknown ClientEvent type "${String(o['type'])}"`);
  }
}

// ── permission.rs snapshots ───────────────────────────────────────────────────

function validatePermissionRequest(v: unknown): void {
  const p = v as PermissionRequest;
  const o = rec(p);
  assert.ok(isNumber(o['request_id']));
  const kind = rec(o['kind']);
  switch (kind['type']) {
    case 'tool_use_confirm':
      assert.ok(
        isString(kind['tool_name']) &&
          isString(kind['tool_input_json']) &&
          isBool(kind['default_allow']),
      );
      break;
    case 'exit_plan_mode':
      assert.ok(isString(kind['plan']));
      break;
    case 'bypass_permissions_mode':
      break;
    default:
      assert.fail(`unknown PermissionKindDto type "${String(kind['type'])}"`);
  }
  if ('worker' in o) {
    const w = rec(o['worker']);
    assert.ok(isString(w['name']) && isString(w['color']));
    if ('team' in w) assert.ok(isString(w['team']));
  }
}

function validatePermissionResolved(v: unknown): void {
  const r = v as PermissionResolved;
  const o = rec(r);
  assert.ok(isNumber(o['request_id']));
  validatePermissionResponse(o['response']);
}

// ── error.rs snapshots ────────────────────────────────────────────────────────

function validateError(v: unknown): void {
  const e = v as ClientError;
  const o = rec(e);
  assert.ok(['transport', 'protocol', 'rejected', 'not_found', 'internal'].includes(o['type'] as string));
  assert.ok(isString(o['message']));
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

test('every command snapshot parses as ClientCommand', () => {
  const files = listSnapshots('command');
  assert.equal(files.length, 54, `expected 54 command snapshots, found ${files.length}`);
  for (const file of files) {
    validateCommand(file, loadSnapshot('command', file));
  }
});

test('every event snapshot parses as ClientEvent', () => {
  const files = listSnapshots('event');
  assert.equal(files.length, 59, `expected 59 event snapshots, found ${files.length}`);
  for (const file of files) {
    validateEvent(file, loadSnapshot('event', file));
  }
});

test('message block_set snapshot parses as MessageDto', () => {
  validateMessage(loadSnapshot('message', 'block_set.json'));
});

test('permission snapshots parse as PermissionRequest / PermissionResolved', () => {
  for (const file of listSnapshots('permission')) {
    if (file === 'resolved.json') {
      validatePermissionResolved(loadSnapshot('permission', file));
    } else {
      validatePermissionRequest(loadSnapshot('permission', file));
    }
  }
});

test('every error snapshot parses as ClientError', () => {
  const files = listSnapshots('error');
  assert.equal(files.length, 5);
  for (const file of files) {
    validateError(loadSnapshot('error', file));
  }
});
