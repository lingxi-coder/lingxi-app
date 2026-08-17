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
    ['pending', 'running', 'paused', 'completed', 'failed', 'cancelled'].includes(
      o['type'] as string,
    ),
  );
}

function validateTaskRow(v: unknown): void {
  const task = rec(v);
  assert.ok(
    isString(task['task_id']) &&
      isString(task['task_type']) &&
      isString(task['description']),
  );
  validateTaskStatus(task['status']);
  if ('can_resume' in task) assert.ok(isBool(task['can_resume']));
  if ('started_at_ms' in task) assert.ok(isNumber(task['started_at_ms']));
}

// ── tool_display.rs — the pre-derived render model ────────────────────────────

const TOOL_VERBS = [
  'update', 'create', 'read', 'search', 'shell', 'output',
  'kill', 'fetch', 'task', 'todo', 'skill', 'generic',
];

const TOOL_ICONS = [
  'read', 'search', 'list', 'edit', 'terminal', 'globe', 'workflow',
  'list_checks', 'sparkles', 'plug', 'output', 'stop', 'wrench',
];

const SYNTAX_CLASSES = [
  'plain', 'keyword', 'type_name', 'function', 'string_lit', 'number',
  'comment', 'punctuation', 'operator', 'variable', 'constant', 'attribute',
];

const HEADLINE_KINDS = [
  'added', 'removed', 'added_removed', 'lines_read', 'lines_read_partial',
  'files_found', 'files_found_truncated', 'lines_found', 'matches_found',
  'interrupted', 'no_content', 'failed', 'plain',
];

const PLAN_TASK_STATES = ['pending', 'in_progress', 'completed'];

function validateToolHeader(v: unknown): void {
  const o = rec(v);
  assert.ok(TOOL_VERBS.includes(o['verb'] as string), `unknown ToolVerbDto "${String(o['verb'])}"`);
  if ('icon' in o) {
    assert.ok(TOOL_ICONS.includes(o['icon'] as string), `unknown ToolIconDto "${String(o['icon'])}"`);
  }
  assert.ok(isString(o['label']) && isString(o['title']));
  for (const opt of ['primary', 'qualifier']) {
    if (opt in o) assert.ok(isString(o[opt]));
  }
  if ('count' in o) assert.ok(isNumber(o['count']));
  if ('sub_line' in o) {
    const sub = rec(o['sub_line']);
    assert.ok(isString(sub['prefix']) && isString(sub['text']));
  }
}

function validateSessionAgent(v: unknown): void {
  const a = rec(v);
  assert.ok(
    isString(a['agent_id']) &&
      isString(a['name']) &&
      isString(a['agent_type']) &&
      isString(a['status']),
  );
  if ('model' in a) assert.ok(isString(a['model']));
  if ('model_profile' in a) assert.ok(isString(a['model_profile']));
  if ('latest_activity' in a) assert.ok(isString(a['latest_activity']));
  if ('updated_at_ms' in a) assert.ok(isNumber(a['updated_at_ms']));
}

function validateStructuredDiff(v: unknown): void {
  const o = rec(v);
  for (const opt of ['file_path', 'language']) {
    if (opt in o) assert.ok(isString(o[opt]));
  }
  assert.ok(
    isNumber(o['gutter_width']) &&
      isNumber(o['additions']) &&
      isNumber(o['removals']) &&
      isNumber(o['truncated_rows']),
  );
  assert.ok(Array.isArray(o['rows']));
  for (const row of o['rows'] as unknown[]) {
    const r = rec(row);
    assert.ok(['add', 'remove', 'context'].includes(r['kind'] as string));
    assert.ok(isNumber(r['line_no']) && isNumber(r['hunk']));
    if ('word_diffed' in r) assert.ok(isBool(r['word_diffed']));
    assert.ok(Array.isArray(r['segments']));
    for (const segment of r['segments'] as unknown[]) {
      const s = rec(segment);
      assert.ok(isString(s['text']));
      assert.ok(
        SYNTAX_CLASSES.includes(s['class'] as string),
        `unknown SyntaxClassDto "${String(s['class'])}"`,
      );
      if ('rgb' in s) assert.ok(isNumber(s['rgb']));
      for (const flag of ['bold', 'italic', 'underline', 'emph']) {
        if (flag in s) assert.ok(isBool(s[flag]));
      }
    }
  }
}

function validateToolResultDisplay(v: unknown): void {
  const o = rec(v);
  if ('headline' in o) assert.ok(isString(o['headline']));
  if ('headline_kind' in o) {
    assert.ok(
      HEADLINE_KINDS.includes(o['headline_kind'] as string),
      `unknown HeadlineKindDto "${String(o['headline_kind'])}"`,
    );
  }
  if ('headline_args' in o) {
    assert.ok(Array.isArray(o['headline_args']));
    for (const arg of o['headline_args'] as unknown[]) assert.ok(isNumber(arg));
  }
  if ('diff' in o) validateStructuredDiff(o['diff']);
  if ('body' in o) assert.ok(isString(o['body']));
  assert.ok(isNumber(o['body_lines']));
  for (const flag of ['body_truncated', 'collapsed']) {
    if (flag in o) assert.ok(isBool(o[flag]));
  }
}

function validatePlanTask(v: unknown): void {
  const o = rec(v);
  if ('id' in o) assert.ok(isString(o['id']));
  assert.ok(isString(o['subject']));
  if ('active_form' in o) assert.ok(isString(o['active_form']));
  assert.ok(
    PLAN_TASK_STATES.includes(o['state'] as string),
    `unknown PlanTaskStateDto "${String(o['state'])}"`,
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
      if ('header' in o) validateToolHeader(o['header']);
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
      if ('display' in o) validateToolResultDisplay(o['display']);
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

function validateDisabledReason(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['code']));
  if ('message' in o) assert.ok(isString(o['message']));
}

function validateReasoningSelection(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'automatic':
    case 'disabled':
    case 'enabled':
      break;
    case 'level':
      assert.ok(isString(o['id']));
      break;
    case 'token_budget':
      assert.ok(isNumber(o['tokens']));
      break;
    default:
      assert.fail(`unknown ReasoningSelectionDto type "${String(o['type'])}"`);
  }
}

function validateConversationControls(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['qualified_model']));

  const permission = rec(o['permission']);
  validatePermissionMode(permission['requested']);
  validatePermissionMode(permission['effective']);
  assert.ok(Array.isArray(permission['options']));
  for (const option of permission['options'] as unknown[]) {
    const item = rec(option);
    validatePermissionMode(item['mode']);
    assert.ok(isBool(item['available']));
    if ('disabled_reason' in item) validateDisabledReason(item['disabled_reason']);
  }

  const reasoning = rec(o['reasoning']);
  validateReasoningSelection(reasoning['requested']);
  validateReasoningSelection(reasoning['effective']);

  const spec = rec(reasoning['spec']);
  assert.ok(Array.isArray(spec['options']));
  for (const option of spec['options'] as unknown[]) {
    const item = rec(option);
    validateReasoningSelection(item['selection']);
    assert.ok(isBool(item['persistable']));
  }
  if ('budget_range' in spec) {
    const range = rec(spec['budget_range']);
    assert.ok(isNumber(range['min_tokens']) && isNumber(range['max_tokens']));
  }
  validateReasoningSelection(spec['provider_default']);
  assert.ok(isBool(spec['forced_reasoning']) && isBool(spec['editable']));
  if ('disabled_reason' in spec) validateDisabledReason(spec['disabled_reason']);
}

// ── local_apps.rs validators (bare-string enums + kind/op-tagged DTOs) ────────

function validateAppWorkflowState(v: unknown): void {
  assert.ok(['draft', 'ready'].includes(v as string));
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

function validateAppSessionKind(v: unknown): void {
  assert.ok(['init', 'conversation'].includes(v as string));
}

function validateAppSessionRow(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['uuid']) &&
      isString(o['title']) &&
      isString(o['modified_rfc3339']) &&
      isNumber(o['message_count']),
  );
  validateAppSessionKind(o['kind']);
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
  if ('device_context' in o) {
    const context = rec(o['device_context']);
    assert.ok(['ios', 'android', 'desktop', 'unknown'].includes(context['os'] as string));
    assert.ok(
      ['iphone', 'ipad', 'phone', 'tablet', 'desktop', 'unknown'].includes(
        context['formFactor'] as string,
      ),
    );
    const viewport = rec(context['viewport']);
    assert.ok(isNumber(viewport['width']) && isNumber(viewport['height']));
    const safeArea = rec(context['safeArea']);
    for (const side of ['top', 'right', 'bottom', 'left']) assert.ok(isNumber(safeArea[side]));
    assert.ok(['light', 'dark', 'unknown'].includes(context['colorScheme'] as string));
    assert.ok(isBool(context['reducedMotion']));
    assert.ok(['touch', 'pointer', 'hybrid', 'unknown'].includes(context['inputMode'] as string));
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
      isBool(o['git_enabled']) &&
      isNumber(o['created_at_ms']) &&
      isNumber(o['updated_at_ms']) &&
      isString(o['workspace_rel']),
  );
  validateAppWorkflowState(o['workflow_state']);
  if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
  if ('init_session_id' in o) assert.ok(isString(o['init_session_id']));
}

function validateAppCheckpoint(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && isString(o['label']) && isNumber(o['created_at_ms']));
  validateAppCheckpointKind(o['kind']);
}

function validateAppDetails(v: unknown): void {
  const o = rec(v);
  validateAppRecord(o['app']);
  if ('manifest' in o) validateAppManifest(o['manifest']);
  validateAppRuntimeDetails(o['runtime']);
  assert.ok(Array.isArray(o['checkpoints']));
  for (const c of o['checkpoints'] as unknown[]) validateAppCheckpoint(c);
}

function validateAppEvent(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'app_details_changed':
      validateAppDetails(o['details']);
      break;
    case 'app_record_changed':
      validateAppRecord(o['record']);
      break;
    case 'app_profile_proposal': {
      const proposal = rec(o['proposal']);
      assert.ok(
        isString(proposal['appId']) &&
          isString(proposal['approvalToken']) &&
          isNumber(proposal['baseRevision']) &&
          isNumber(proposal['currentRevision']) &&
          isString(proposal['instructions']) &&
          isString(proposal['reason']),
      );
      break;
    }
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
    case 'get_conversation_controls':
      break;
    case 'set_reasoning_selection':
      validateReasoningSelection(o['selection']);
      break;
    case 'run_slash_command':
      assert.ok(isString(o['raw']));
      if ('turn_id' in o) assert.ok(isNumber(o['turn_id']));
      break;
    case 'refresh_listings':
      assert.ok(Array.isArray(o['which']));
      for (const k of o['which'] as unknown[]) validateListingKind(k);
      break;
    case 'list_session_agents':
      break;
    case 'load_session_agent_transcript':
      assert.ok(isString(o['agent_id']));
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
    case 'resume_workflow':
      assert.ok(isString(o['task_id']));
      break;
    case 'list_apps':
      break;
    case 'create_app':
      assert.ok(isString(o['name']) && isString(o['brief']));
      validateAppCreateOrigin(o['origin']);
      if ('git_enabled' in o) assert.ok(isBool(o['git_enabled']));
      if ('workflow_model' in o) assert.ok(isString(o['workflow_model']));
      if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
      break;
    case 'start_app':
    case 'stop_app':
    case 'restart_app':
    case 'get_app_details':
    case 'reset_app_permissions':
    case 'list_app_checkpoints':
    case 'delete_app':
      assert.ok(isString(o['app_id']));
      break;
    case 'list_app_sessions':
      assert.ok(isString(o['app_id']));
      if ('offset' in o) assert.ok(isNumber(o['offset']));
      if ('limit' in o) assert.ok(isNumber(o['limit']));
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
    case 'resolve_app_profile_proposal':
      assert.ok(
        isString(o['app_id']) &&
          isString(o['approval_token']) &&
          isBool(o['approved']),
      );
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
    case 'permission_request_resolved':
      assert.ok(isNumber(o['request_id']));
      assert.ok(
        ['approved', 'denied', 'cancelled', 'expired'].includes(String(o['resolution'])),
      );
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
      if ('header' in o) validateToolHeader(o['header']);
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
      if ('display' in o) validateToolResultDisplay(o['display']);
      break;
    case 'plan_updated':
      assert.ok(Array.isArray(o['tasks']));
      for (const task of o['tasks'] as unknown[]) validatePlanTask(task);
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
    case 'session_agent_list':
      assert.ok(isString(o['session_id']) && Array.isArray(o['agents']));
      for (const a of o['agents'] as unknown[]) validateSessionAgent(a);
      break;
    case 'session_agent_transcript':
      assert.ok(
        isString(o['session_id']) &&
          isString(o['agent_id']) &&
          Array.isArray(o['messages']) &&
          isNumber(o['next_message_index']) &&
          isNumber(o['revision']),
      );
      for (const m of o['messages'] as unknown[]) validateMessage(m);
      break;
    case 'session_agent_updated':
      assert.ok(isString(o['session_id']));
      validateSessionAgent(o['agent']);
      break;
    case 'session_agent_message':
      assert.ok(
        isString(o['session_id']) &&
          isString(o['agent_id']) &&
          isNumber(o['message_index']),
      );
      validateMessage(o['message']);
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
    case 'conversation_controls_changed':
      validateConversationControls(o['controls']);
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
        if ('aliases' in r) assert.ok(Array.isArray(r['aliases']) && r['aliases'].every(isString));
        if ('argument_hint' in r) assert.ok(isString(r['argument_hint']));
        if ('menu_description' in r) assert.ok(isString(r['menu_description']));
        if ('hidden' in r) assert.ok(isBool(r['hidden']));
      }
      break;
    case 'slash_command_result':
      if ('turn_id' in o) assert.ok(isNumber(o['turn_id']));
      assert.ok(isString(o['display']));
      if ('is_error' in o) assert.ok(isBool(o['is_error']));
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
      validateTaskRow(o['task']);
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
      if ('origin_session_id' in o) assert.ok(isString(o['origin_session_id']));
      break;
    case 'workflow_resumed':
      assert.ok(
        isString(o['previous_task_id']) &&
          isString(o['run_id']),
      );
      validateTaskRow(o['task']);
      if ('origin_session_id' in o) assert.ok(isString(o['origin_session_id']));
      break;
    case 'apps_changed':
      assert.ok(Array.isArray(o['apps']));
      for (const app of o['apps'] as unknown[]) validateAppRecord(app);
      break;
    case 'app_event':
      validateAppEvent(o['event']);
      break;
    case 'app_workflow_changed':
      assert.ok(isString(o['app_id']));
      validateAppWorkflowState(o['state']);
      if ('detail' in o) assert.ok(isString(o['detail']));
      break;
    case 'app_runtime_changed':
      assert.ok(isString(o['app_id']));
      validateAppRuntimeState(o['state']);
      if ('details' in o) validateAppRuntimeDetails(o['details']);
      if ('last_error' in o) assert.ok(isString(o['last_error']));
      break;
    case 'app_sessions_changed':
      assert.ok(isString(o['app_id']));
      assert.ok(Array.isArray(o['sessions']));
      for (const row of o['sessions'] as unknown[]) validateAppSessionRow(row);
      if ('next_offset' in o) assert.ok(isNumber(o['next_offset']));
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
  if ('owner' in o) {
    const owner = rec(o['owner']);
    if ('session_id' in owner) assert.ok(isString(owner['session_id']));
    if ('turn_id' in owner) assert.ok(isNumber(owner['turn_id']));
    if ('worker_name' in owner) assert.ok(isString(owner['worker_name']));
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
  assert.equal(files.length, 47, `expected 47 command snapshots, found ${files.length}`);
  for (const file of files) {
    validateCommand(file, loadSnapshot('command', file));
  }
});

test('workflow model metadata and paused task status pass the wire guards', () => {
  validateTaskStatus({ type: 'paused' });
  validateCommand('create_app.json', {
    type: 'create_app',
    name: 'Demo',
    origin: 'chat',
    brief: 'Demo app',
    workflow_model: 'deepseek/deepseek-v4-flash',
  });
  validateSessionAgent({
    agent_id: 'design',
    name: 'design',
    agent_type: 'design',
    model: 'deepseek-v4-flash',
    model_profile: 'deepseek',
    status: 'running',
  });
});

test('every event snapshot parses as ClientEvent', () => {
  const files = listSnapshots('event');
  assert.equal(files.length, 62, `expected 62 event snapshots, found ${files.length}`);
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
