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
import { join } from 'node:path';
import { test } from 'node:test';

import type {
  ClientCommand,
  ClientError,
  ClientEvent,
  MessageDto,
  PermissionRequest,
  PermissionResolved,
  TaskRowDto,
} from '../src/protocol.js';
import {
  ALL_CLIENT_COMMAND_TYPES,
  ALL_CLIENT_EVENT_TYPES,
  ALL_TASK_ROW_DTO_KEYS,
} from '../src/protocolCoverage.js';
import { validateClientEvent, validateServerHello } from '../src/validation.js';
import { runtimePath } from './runtimeSource.js';

const SNAP_ROOT = runtimePath('crates/client/snapshots');

function loadSnapshot<T>(category: string, name: string): T {
  const raw = readFileSync(join(SNAP_ROOT, category, name), 'utf8');
  return JSON.parse(raw) as T;
}

function listSnapshots(category: string): string[] {
  return readdirSync(join(SNAP_ROOT, category))
    .filter((f) => f.endsWith('.json'))
    .sort();
}

function assertSnapshotCoverage(category: string, files: string[], forbidden: string[] = []): void {
  assert.ok(files.length > 0, `expected ${category} snapshots to be non-empty`);
  for (const name of forbidden) {
    assert.ok(
      !files.includes(name),
      `${category} snapshot ${name} still exists after its protocol variant was removed`,
    );
  }
}

/**
 * The distinct `type` tag values actually carried by every golden under
 * `category` — read from each file's JSON CONTENT, never its filename.
 */
function typeTagsOnDisk(category: string): Set<string> {
  const tags = new Set<string>();
  for (const file of listSnapshots(category)) {
    const { type } = loadSnapshot<{ type: string }>(category, file);
    tags.add(type);
  }
  return tags;
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

function exactObjectKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = [],
  label = 'object',
): void {
  const keys = Object.keys(value).sort();
  const allowed = new Set([...required, ...optional]);
  for (const key of required) {
    assert.ok(key in value, `${label} is missing required key ${key}`);
  }
  for (const key of keys) {
    assert.ok(allowed.has(key), `${label} carries unknown key ${key}`);
  }
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
      'skills',
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

function validateSettingsDestination(v: unknown): void {
  assert.ok(['user', 'project', 'local'].includes(v as string), `unknown WritableScopeDto "${String(v)}"`);
}

function validateConfigurationAdminCommand(v: unknown): void {
  const command = rec(v);
  assert.ok(isString(command['action']));
  if ('operation_id' in command) assert.ok(isNumber(command['operation_id']));
  for (const key of ['target', 'scope', 'revision', 'payload_json']) {
    if (key in command) assert.ok(isString(command[key]));
  }
}

function validatePermissionBehavior(v: unknown): void {
  assert.ok(['allow', 'deny', 'ask'].includes(v as string), `unknown PermissionBehaviorDto "${String(v)}"`);
}

function validateMcpScope(v: unknown): void {
  assert.ok(['user', 'local', 'project'].includes(v as string), `unknown WritableScopeDto "${String(v)}"`);
}

function validateStringArray(v: unknown): void {
  assert.ok(Array.isArray(v));
  for (const item of v as unknown[]) assert.ok(isString(item));
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
  if ('error' in task) assert.ok(isString(task['error']));
  if ('stage' in task) assert.ok(isString(task['stage']));
  if ('awaiting_plan_approval' in task) assert.equal(typeof task['awaiting_plan_approval'], 'boolean');
}

// Compile-time contract check for finding B7#11: `TaskRowDto` (listings.rs)
// gained a `stage` field for F005 (`/fusion` progress). This TS mirror is
// hand-maintained and `packages/bridge-client/tsconfig.json` excludes `test/` from
// `npm run typecheck`, so nothing in THIS file can be the type-level gate —
// `tsx` (which `npm test` runs through) transpiles without type-checking, so
// a dropped field here would be a silent pass. The actual enforcing gate is
// `ALL_TASK_ROW_DTO_KEYS` in `src/protocolCoverage.ts`, which lives under
// `src/` specifically so it IS covered by `npm run typecheck`: dropping
// `stage` from `TaskRowDto` there fails with "Property 'stage' does not
// exist on type 'Record<keyof TaskRowDto, true>'". The assertion below is a
// runtime cross-check on that same table (imported, not restated), so a key
// added to one but not the other — including a future 8th field — goes red
// here too.
test('ALL_TASK_ROW_DTO_KEYS (src/protocolCoverage.ts) matches TaskRowDto exactly', () => {
  assert.deepEqual(Object.keys(ALL_TASK_ROW_DTO_KEYS).sort(), [
    'agent_id',
    'awaiting_plan_approval',
    'can_resume',
    'description',
    'effort',
    'error',
    'kind',
    'model',
    'stage',
    'started_at_ms',
    'status',
    'task_id',
    'task_type',
    'unread',
  ].sort());
});

// This test does NOT go red if `stage` is removed from the `TaskRowDto`
// interface — `tsx` transpiles without type-checking, so a missing property
// on an object literal is silently dropped, not rejected. It exists only to
// prove `validateTaskRow` accepts and correctly type-checks a string `stage`
// value at runtime; the type-level guarantee is the `ALL_TASK_ROW_DTO_KEYS`
// test above.
test('validateTaskRow accepts a string stage value', () => {
  const withStage: TaskRowDto = {
    task_id: 'f00000001',
    task_type: 'local_fusion',
    status: { type: 'running' },
    description: '/fusion compare two approaches',
    stage: 'Running panels 2/3',
  };
  validateTaskRow(withStage);
  assert.equal(withStage.stage, 'Running panels 2/3');
});

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

function validateSessionAgentMessageRow(v: unknown): void {
  const row = rec(v);
  assert.ok(Number.isSafeInteger(row['message_index']) && (row['message_index'] as number) >= 0);
  assert.ok(isString(row['message_uuid']));
  validateMessage(row['message']);
  if ('api_error_json' in row) {
    assert.ok(isString(row['api_error_json']));
    const parsed: unknown = JSON.parse(row['api_error_json'] as string);
    assert.ok(parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed));
  }
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

function validateTurnRecoveryState(v: unknown): void {
  const o = rec(v);
  assert.ok(
    ['running', 'waiting_for_user', 'paused_recoverable', 'completed', 'failed', 'cancelled'].includes(
      o['type'] as string,
    ),
    `unknown TurnRecoveryStateDto "${String(o['type'])}"`,
  );
}

function validatePermissionMode(v: unknown): void {
  assert.ok(
    ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'].includes(
      v as string,
    ),
  );
}

function validateTypescriptLspMode(v: unknown): void {
  assert.ok(['auto', 'off', 'on'].includes(v as string));
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

// ── AudioService DTO guards ─────────────────────────────────────────────────

function validateAudioIdentity(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && Number.isSafeInteger(o['generation']) && Number.isSafeInteger(o['service_epoch']));
}

function validateAudioOperation(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'capture':
      exactObjectKeys(o, ['type', 'sample_rate_hz', 'format']);
      assert.ok(isNumber(o['sample_rate_hz']) && isString(o['format']));
      break;
    case 'start_recording':
      assert.ok(isNumber(o['sample_rate_hz']) && isString(o['format']));
      break;
    case 'play':
      exactObjectKeys(o, ['type', 'pcm_base64', 'sample_rate_hz']);
      assert.ok(isString(o['pcm_base64']) && isNumber(o['sample_rate_hz']));
      break;
    case 'stop_recording':
      assert.ok(isString(o['handle']));
      break;
    case 'listen':
      if ('language' in o) assert.ok(isString(o['language']));
      break;
    case 'synthesize':
    case 'speak':
      assert.ok(isString(o['text']));
      if ('language' in o) assert.ok(isString(o['language']));
      if ('rate' in o) assert.ok(isNumber(o['rate']));
      if ('voice' in o) assert.ok(isString(o['voice']));
      break;
    case 'status':
      if ('handle' in o) assert.ok(isString(o['handle']));
      break;
    case 'end_owner':
      break;
    default:
      assert.fail(`unknown AudioOperationDto type: ${String(o['type'])}`);
  }
}

function validateAudioErrorKind(v: unknown): void {
  assert.ok([
    'permission_denied', 'busy', 'cancelled', 'timeout', 'no_speech', 'not_recording',
    'unavailable', 'unsupported', 'model_missing', 'voice_missing', 'invalid_request',
    'synthesis_failed', 'native_failure', 'media_too_large',
  ].includes(String(v)), `unknown AudioErrorKindDto "${String(v)}"`);
}

function validateAudioResult(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'recording_started':
      assert.ok(isString(o['handle']));
      break;
    case 'recording':
      assert.ok(isString(o['audio_base64']) && isString(o['mime_type']));
      break;
    case 'transcript':
      assert.ok(isString(o['text']));
      if ('language' in o) assert.ok(isString(o['language']));
      if ('confidence' in o) assert.ok(isNumber(o['confidence']));
      break;
    case 'synthesized':
      assert.ok(isString(o['pcm_base64']) && isNumber(o['sample_rate_hz']));
      break;
    case 'playback_completed':
      assert.ok(isNumber(o['duration_ms']));
      break;
    case 'status': {
      const status = rec(o['status']);
      assert.ok(isBool(status['recording']) && isBool(status['playing']));
      break;
    }
    case 'owner_ended':
      break;
    case 'failed': {
      const error = rec(o['error']);
      validateAudioErrorKind(error['kind']);
      assert.ok(isString(error['message']));
      break;
    }
    default:
      assert.fail(`unknown AudioOperationResultDto type: ${String(o['type'])}`);
  }
}

function validateAudioCapabilities(v: unknown): void {
  const o = rec(v);
  assert.ok(Number.isSafeInteger(o['service_epoch']) && (o['service_epoch'] as number) >= 0);
  assert.ok(Number.isSafeInteger(o['support_revision']) && (o['support_revision'] as number) >= 0);
  assert.ok(Number.isSafeInteger(o['max_payload_bytes']) && (o['max_payload_bytes'] as number) >= 0);
  assert.ok(Array.isArray(o['supported_operations']) && Array.isArray(o['readiness']));
  for (const operation of o['supported_operations']) assert.ok(['record', 'listen', 'synthesize', 'speak'].includes(String(operation)));
  for (const entry of o['readiness']) {
    const row = rec(entry);
    assert.ok(['record', 'listen', 'synthesize', 'speak'].includes(String(row['operation'])));
    assert.ok(['ready', 'needs_permission', 'busy', 'missing_model', 'unavailable'].includes(String(row['state'])));
  }
}

// ── ClientCommand ─────────────────────────────────────────────────────────────

function validateCronJob(v: unknown): void {
  const job = rec(v);
  for (const key of ['id', 'cron', 'prompt']) assert.ok(isString(job[key]));
  for (const key of ['recurring', 'durable', 'permanent']) assert.ok(isBool(job[key]));
  assert.ok(isNumber(job['created_at']));
  for (const key of ['last_fired_at', 'expires_at', 'next_run_at']) if (job[key] != null) assert.ok(isNumber(job[key]));
  if (job['automation'] != null) validateCronAutomation(job['automation']);
}
function validateCronAutomation(v: unknown): void {
  const config = rec(v);
  assert.equal(config['version'], 2);
  assert.ok(['active', 'paused', 'completed'].includes(String(config['status'])));
  assert.ok(['new_session', 'selected_session', 'task_session'].includes(String(config['runMode'])));
  assert.ok(['all', 'failed', 'none'].includes(String(config['notificationPolicy'])));
  assert.ok(isString(config['model'])); validateReasoningSelection(config['reasoning']);
  if (config['runs'] != null) {
    assert.ok(Array.isArray(config['runs']));
    for (const value of config['runs'] as unknown[]) {
      const run = rec(value);
      assert.ok(isString(run['id']) && isString(run['taskId']) && isNumber(run['scheduledAt']));
      assert.ok(['queued', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted'].includes(String(run['status'])));
      assert.ok(isString(run['model'])); validateReasoningSelection(run['reasoning']);
    }
  }
}

function validateCommand(name: string, v: unknown): void {
  const c = v as ClientCommand;
  const o = rec(c);
  switch (o['type']) {
    case 'ui_attach':
      assert.ok(['desktop', 'mobile', 'vscode'].includes(o['surface'] as string));
      assert.ok(isString(o['client_id']));
      break;
    case 'ui_detach':
      assert.ok(isString(o['client_id']));
      break;
    case 'ui_client_module':
      exactObjectKeys(o, ['type', 'request_id', 'plugin']);
      assert.ok(isString(o['request_id']) && isString(o['plugin']));
      break;
    case 'ui_render':
    case 'ui_message':
    case 'ui_client_fault':
    case 'ui_client_press':
    case 'ui_press':
    case 'ui_input':
    case 'ui_select':
      assert.ok(isString(o['request_id']) && isString(o['request_json']));
      rec(JSON.parse(o['request_json'] as string));
      break;
    case 'ui_client_operation':
      assert.ok(isString(o['request_id']) && isString(o['operation_json']));
      rec(JSON.parse(o['operation_json'] as string));
      break;
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
    case 'attach_turn':
      assert.ok(isNumber(o['turn_id']));
      if ('after_sequence' in o) assert.ok(isNumber(o['after_sequence']));
      break;
    case 'resume_turn':
      assert.ok(isNumber(o['turn_id']));
      break;
    case 'pause_turn':
      assert.ok(isNumber(o['turn_id']) && isString(o['reason']));
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
    case 'set_typescript_lsp_mode':
      validateTypescriptLspMode(o['mode']);
      break;
    case 'list_provider_credentials':
      assert.ok(isNumber(o['operation_id']) && Array.isArray(o['provider_ids']));
      for (const providerId of o['provider_ids'] as unknown[]) assert.ok(isString(providerId));
      if (o['preview_provider_ids'] !== undefined) {
        assert.ok(Array.isArray(o['preview_provider_ids']));
        for (const providerId of o['preview_provider_ids'] as unknown[]) assert.ok(isString(providerId));
      }
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
    case 'test_provider_connection':
      assert.ok(
        isNumber(o['operation_id']) &&
          isString(o['provider_id']) &&
          isString(o['api_base']) &&
          isString(o['model']),
      );
      if ('credential_override' in o) assert.ok(isString(o['credential_override']));
      break;
    case 'cron_run_started':
      assert.ok(isString(o['run_id']) && isString(o['session_id']));
      break;
    case 'scheduled_run_turn':
      assert.ok(isString(o['run_id']) && isString(o['prompt']) && isString(o['model']));
      validateReasoningSelection(o['reasoning']);
      break;
    case 'cron_run_completed':
      assert.ok(isString(o['run_id']));
      for (const key of ['session_id', 'summary', 'error']) if (o[key] != null) assert.ok(isString(o[key]));
      break;
    case 'cron_manage': {
      assert.ok(isString(o['request_id']));
      const request = o['request'] as Record<string, unknown>;
      assert.ok(['list', 'create', 'update', 'delete', 'pause', 'resume', 'complete', 'history', 'prune_history'].includes(request['action'] as string));
      if (request['automation'] != null) validateCronAutomation(request['automation']);
      for (const key of ['id', 'cron', 'prompt']) if (key in request) assert.ok(isString(request[key]));
      for (const key of ['recurring', 'durable']) if (key in request) assert.equal(typeof request[key], 'boolean');
      break;
    }
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
    case 'set_fast_mode':
      assert.ok(isBool(o['enabled']));
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
    case 'fork_session':
      assert.ok(
        isString(o['session_id']) &&
          ['chat', 'code'].includes(String(o['target_mode'])),
      );
      break;
    case 'login':
    case 'logout':
    case 'force_compact':
    case 'clear_session':
    case 'request_exit':
      break;
    case 'task_list':
      if ('request_id' in o) assert.ok(isString(o['request_id']));
      if ('status_filter' in o) validateTaskStatus(o['status_filter']);
      break;
    case 'task_output':
      assert.ok(isString(o['task_id']) && isNumber(o['offset']));
      break;
    case 'task_stop':
      assert.ok(isString(o['task_id']));
      break;
    case 'task_message':
      assert.ok(isString(o['task_id']) && isString(o['message']));
      break;
    case 'resume_workflow':
      assert.ok(isString(o['task_id']));
      break;
    case 'update_settings':
      validateSettingsDestination(o['destination']);
      assert.ok(isString(o['patch_json']));
      break;
    case 'update_permission_rules':
      validateSettingsDestination(o['destination']);
      validatePermissionBehavior(o['behavior']);
      validateStringArray(o['add']);
      validateStringArray(o['remove']);
      break;
    case 'set_default_permission_mode':
      validateSettingsDestination(o['destination']);
      assert.ok(isString(o['mode']));
      break;
    case 'update_workspace_directories':
      validateSettingsDestination(o['destination']);
      validateStringArray(o['add']);
      validateStringArray(o['remove']);
      break;
    case 'upsert_mcp_server':
      validateMcpScope(o['scope']);
      assert.ok(isString(o['name']) && isString(o['config_json']));
      break;
    case 'remove_mcp_server':
      validateMcpScope(o['scope']);
      assert.ok(isString(o['name']));
      break;
    case 'skill_admin':
    case 'mcp_admin':
    case 'plugin_admin':
    case 'hook_admin':
      validateConfigurationAdminCommand(o['command']);
      break;
    case 'get_audio_session_context':
    case 'stop_realtime_audio':
      exactObjectKeys(o, ['type']);
      break;
    case 'start_realtime_audio':
      exactObjectKeys(o, ['type', 'request_json']);
      assert.ok(isString(o['request_json']));
      break;
    case 'realtime_audio_input':
      exactObjectKeys(o, ['type', 'input_json']);
      assert.ok(isString(o['input_json']));
      break;
    case 'audio_response':
      validateAudioIdentity(o['identity']);
      validateAudioResult(o['result']);
      break;
    case 'update_audio_capabilities':
      validateAudioCapabilities(o['capabilities']);
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
    case 'query_model_change':
    case 'visualization_block':
    case 'assistant_block_start':
    case 'assistant_block_identity':
    case 'tombstone':
    case 'refusal_continuation':
    case 'user_transcript_row_identity':
    case 'assistant_transcript_row_uuids':
    case 'ui_control_result':
    case 'ui_invalidate':
      assert.equal(validateClientEvent(v).type, o['type']);
      break;
    case 'ui_client_frame':
      // The Rust DTO stores opaque JSON; these serde fixtures exercise that
      // envelope. ui-runtime.test.ts checks valid VM payloads and rejects
      // malformed revisions through the production validator.
      exactObjectKeys(o, ['type', 'runtime_id', 'frame_json']);
      assert.ok(isString(o['runtime_id']) && isString(o['frame_json']));
      rec(JSON.parse(o['frame_json'] as string));
      break;
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
    case 'ui_log':
      assert.ok(isString(o['plugin']) && isString(o['text']));
      break;
    case 'ui_toast':
      assert.ok(isString(o['plugin']) && isString(o['text']) && isNumber(o['timeout_ms']));
      break;
    case 'ui_status':
      assert.ok(isString(o['plugin']) && (o['text'] === null || isString(o['text'])));
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
    case 'turn_recovery_state': {
      const snapshot = rec(o['snapshot']);
      assert.ok(
        isString(snapshot['session_id']) &&
          isNumber(snapshot['turn_id']) &&
          isNumber(snapshot['first_sequence']) &&
          isNumber(snapshot['last_sequence']) &&
          isBool(snapshot['safe_to_resume']),
      );
      validateTurnRecoveryState(snapshot['state']);
      if ('reason' in snapshot) assert.ok(isString(snapshot['reason']));
      break;
    }
    case 'turn_event_replay':
      assert.ok(
        isString(o['session_id']) &&
          isNumber(o['turn_id']) &&
          isNumber(o['sequence']) &&
          isString(o['event_json']),
      );
      break;
    case 'turn_ended':
      assert.ok(['end_turn', 'max_turns', 'cancelled'].includes(rec(o['outcome'])['type'] as string));
      if ('stop_reason' in o) assert.ok(isString(o['stop_reason']));
      validateCost(o['cost']);
      break;
    case 'cost_update':
      validateCost(o);
      break;
    case 'compaction_status':
      assert.ok(isString(o['phase']));
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'compaction_completed':
      assert.ok(
        isNumber(o['messages_before']) &&
          isNumber(o['messages_after']) &&
          isNumber(o['bytes_saved']) &&
          isString(o['summary']),
      );
      break;
    case 'session_started':
      assert.ok(isString(o['session_id']) && ['chat', 'code'].includes(String(o['mode'])));
      break;
    case 'session_resumed':
      assert.ok(isString(o['session_id']) && ['chat', 'code'].includes(String(o['mode'])));
      // `messages` is REQUIRED (Vec<MessageDto>, no skip_serializing_if) — it
      // carries the full restored transcript (oldest-first); may be empty.
      assert.ok(Array.isArray(o['messages']));
      for (const m of o['messages'] as unknown[]) validateMessage(m);
      break;
    case 'session_forked':
      assert.ok(
        isString(o['source_session_id']) &&
          isString(o['session_id']) &&
          ['chat', 'code'].includes(String(o['mode'])),
      );
      break;
    case 'session_ended':
      break;
    case 'session_list':
      assert.ok(Array.isArray(o['sessions']));
      for (const s of o['sessions'] as unknown[]) {
        const r = rec(s);
        assert.ok(
          isString(r['uuid']) &&
            ['chat', 'code'].includes(String(r['mode'])) &&
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
      {
        const indexes = new Set<number>();
        const uuids = new Set<string>();
        for (const m of o['messages'] as unknown[]) {
          validateSessionAgentMessageRow(m);
          const row = rec(m);
          const index = row['message_index'] as number;
          const uuid = row['message_uuid'] as string;
          assert.ok(!indexes.has(index), `duplicate session-agent message index ${index}`);
          assert.ok(!uuids.has(uuid), `duplicate session-agent message UUID ${uuid}`);
          indexes.add(index);
          uuids.add(uuid);
        }
      }
      break;
    case 'session_agent_updated':
      assert.ok(isString(o['session_id']));
      validateSessionAgent(o['agent']);
      break;
    case 'session_agent_message':
      assert.ok(
        isString(o['session_id']) &&
          isString(o['agent_id']) &&
          Number.isSafeInteger(o['message_index']) &&
          (o['message_index'] as number) >= 0 &&
          isString(o['message_uuid']),
      );
      validateMessage(o['message']);
      if ('api_error_json' in o) {
        assert.ok(isString(o['api_error_json']));
        const parsed: unknown = JSON.parse(o['api_error_json'] as string);
        assert.ok(parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed));
      }
      break;
    case 'session_agent_tombstone':
      assert.ok(isString(o['session_id']) && isString(o['agent_id']) && isString(o['message_uuid']) && isBool(o['display_only']));
      break;
    case 'model_list':
      assert.ok(Array.isArray(o['models']) && isString(o['current']));
      if (o['details'] !== undefined) assert.ok(Array.isArray(o['details']));
      break;
    case 'provider_model_catalog':
      assert.ok(Array.isArray(o['providers']));
      for (const provider of o['providers'] as unknown[]) {
        const entry = rec(provider);
        assert.ok(
          isString(entry['provider_id']) &&
            isString(entry['provider_label']) &&
            Array.isArray(entry['models']),
        );
      }
      break;
    case 'model_changed':
      assert.ok(isString(o['model']));
      break;
    case 'permission_mode_changed':
      validatePermissionMode(o['mode']);
      break;
    case 'typescript_lsp_mode_changed':
      validateTypescriptLspMode(o['requested']);
      validateTypescriptLspMode(o['effective']);
      assert.ok(isBool(o['available']));
      break;
    case 'conversation_controls_changed':
      validateConversationControls(o['controls']);
      break;
    case 'fast_mode_changed':
      assert.ok(isBool(o['enabled']));
      break;
    case 'openai_oauth_updated': {
      const session = rec(o['session']);
      assert.ok(isString(session['access_token']) && isNumber(session['expires_at']) && isBool(session['fedramp']));
      if (session['refresh_token'] != null) assert.ok(isString(session['refresh_token']));
      if (session['account_id'] != null) assert.ok(isString(session['account_id']));
      break;
    }
    case 'provider_credential_status':
      assert.ok(
        isNumber(o['operation_id']) &&
          Array.isArray(o['configured_provider_ids']) &&
          isBool(o['storage_encrypted']),
      );
      for (const providerId of o['configured_provider_ids'] as unknown[]) assert.ok(isString(providerId));
      if ('credential_previews' in o) {
        const previews = rec(o['credential_previews']);
        for (const [providerId, preview] of Object.entries(previews)) {
          assert.ok(isString(providerId) && isString(preview));
          assert.match(preview as string, /^••••.{0,4}$/u);
        }
      }
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'provider_connection_tested':
      assert.ok(
        isNumber(o['operation_id']) &&
          isString(o['provider_id']) &&
          isBool(o['connected']) &&
          isBool(o['reachable']) &&
          isBool(o['authenticated']) &&
          isBool(o['model_available']) &&
          isNumber(o['latency_ms']) &&
          isString(o['message']) &&
          isBool(o['used_stored_credential']),
      );
      if ('http_status' in o) assert.ok(isNumber(o['http_status']));
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
    case 'skills':
      assert.ok(Array.isArray(o['skills']));
      for (const s of o['skills'] as unknown[]) {
        const r = rec(s);
        assert.ok(isString(r['name']) && isString(r['source_dir']));
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
      if ('files_json' in o) assert.ok(isString(o['files_json']));
      if ('active_json' in o) assert.ok(isString(o['active_json']));
      if ('locked' in o) validateStringArray(o['locked']);
      if ('layers_json' in o) assert.ok(isString(o['layers_json']));
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
    case 'task_list_complete':
      assert.ok(isString(o['request_id']));
      assert.ok(Number.isSafeInteger(o['active_count']) && (o['active_count'] as number) >= 0);
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'task_row': {
      validateTaskRow(o['task']);
      break;
    }
    case 'message_identity':
    case 'message_retracted':
      assert.ok(isString(o['message_id']));
      break;
    case 'loop_wakeup':
      // `companion` rides only on a collapsed streak, so it stays optional
      // while the counters are always present.
      assert.ok(isString(o['message']) && isNumber(o['streak']) && isNumber(o['since_ms']));
      if ('companion' in o) assert.ok(isString(o['companion']));
      break;
    case 'task_lifecycle':
      // The engine forwards the SDK record already serialized, so the wire
      // guarantee here is the envelope: exactly one JSON string payload.
      assert.ok(isString(o['event_json']));
      break;
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
    case 'audio_session_context':
      exactObjectKeys(o, ['type', 'session_id', 'profile_id', 'account_scope']);
      assert.ok(isString(o['session_id']) && isString(o['profile_id']) && isString(o['account_scope']));
      assert.deepEqual(validateClientEvent(v), v);
      break;
    case 'realtime_audio_event':
      exactObjectKeys(o, ['type', 'session_id', 'event_json']);
      assert.ok(isString(o['session_id']) && isString(o['event_json']));
      assert.deepEqual(validateClientEvent(v), v);
      break;
    case 'audio_request':
      assert.deepEqual(validateClientEvent(v), v);
      break;
    case 'audio_cancel':
      assert.deepEqual(validateClientEvent(v), v);
      break;
    case 'audio_capabilities_changed':
      validateAudioCapabilities(o['capabilities']);
      assert.deepEqual(validateClientEvent(v), v);
      break;
    case 'cron_run_bound':
    case 'scheduled_run_finished':
      assert.ok(isString(o['run_id']));
      for (const key of ['summary', 'error']) if (o[key] != null) assert.ok(isString(o[key]));
      break;
    case 'cron_run_requested':
      assert.ok(isString(o['run_id'])); validateCronJob(o['task']);
      break;
    case 'scheduled_task_fire':
      assert.ok(isString(o['message']));
      break;
    case 'cron_result':
      assert.ok(isString(o['request_id']));
      assert.ok(Array.isArray(o['jobs']));
      for (const job of o['jobs'] as Record<string, unknown>[]) {
        for (const key of ['id', 'cron', 'prompt']) assert.ok(isString(job[key]));
        for (const key of ['recurring', 'durable', 'permanent']) assert.equal(typeof job[key], 'boolean');
        assert.ok(isNumber(job['created_at']));
        if ('last_fired_at' in job) assert.ok(isNumber(job['last_fired_at']));
      }
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'configuration_operation':
      assert.ok(['skill', 'mcp', 'plugin', 'hook'].includes(o['domain'] as string));
      assert.ok(isNumber(o['operation_id']));
      assert.ok(['started', 'progress', 'succeeded', 'failed'].includes(o['status'] as string));
      assert.ok(['applied', 'restart_required', 'not_applicable'].includes(o['effect'] as string));
      if ('message' in o) assert.ok(isString(o['message']));
      if ('details_json' in o) assert.ok(isString(o['details_json']));
      break;
    case 'skill_catalog':
      assert.ok(isString(o['catalog_json']));
      break;
    case 'skill_document':
      assert.ok(isString(o['document_json']));
      break;
    case 'mcp_configuration_snapshot':
      assert.ok(isString(o['snapshot_json']));
      break;
    case 'plugin_catalog':
      assert.ok(isString(o['catalog_json']));
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
  assertSnapshotCoverage('command', files);
  for (const file of files) {
    validateCommand(file, loadSnapshot('command', file));
  }
});

test('current worker model metadata and paused task status pass while obsolete app creation is rejected', () => {
  validateTaskStatus({ type: 'paused' });
  assert.throws(() => validateCommand('obsolete_create_app.json', { type: 'create_app', name: 'Demo', mode: 'shell' }), /unknown ClientCommand type/);
  validateSessionAgent({
    agent_id: 'design',
    name: 'design',
    agent_type: 'design',
    model: 'deepseek-flash',
    model_profile: 'deepseek',
    status: 'running',
  });
});

test('every event snapshot parses as ClientEvent', () => {
  const files = listSnapshots('event');
  assertSnapshotCoverage('event', files);
  for (const file of files) {
    validateEvent(file, loadSnapshot('event', file));
  }
});

// The two tests above only prove every golden's `type` tag is HANDLED by the
// hand-written `switch` in validateCommand/validateEvent — they cast the
// loaded JSON `as ClientCommand`/`as ClientEvent` and never check that tag
// against the actual TS union. That is exactly how `attach_turn`/
// `resume_turn`/`pause_turn` and `turn_recovery_state`/`turn_event_replay`
// went missing from the unions while staying fully validated and
// snapshotted: nothing tied the switch to the union. `ALL_CLIENT_COMMAND_TYPES`
// / `ALL_CLIENT_EVENT_TYPES` (src/protocolCoverage.ts) close that at COMPILE
// time — a `Record<Union['type'], true>` can only type-check when its key set
// is exactly the union's `type` values. These two tests close the remaining
// runtime direction: the record's keys must also match what the goldens
// actually carry, so a golden with no union member (or a union member with no
// golden) fails HERE, by name, instead of staying silent.
test('ALL_CLIENT_COMMAND_TYPES matches the locked runtime exactly', () => {
  assert.deepEqual(Object.keys(ALL_CLIENT_COMMAND_TYPES).sort(), [...typeTagsOnDisk('command')].sort());
});

test('ALL_CLIENT_EVENT_TYPES matches the locked runtime exactly', () => {
  assert.deepEqual(Object.keys(ALL_CLIENT_EVENT_TYPES).sort(), [...typeTagsOnDisk('event')].sort());
});

// `turn_recovery_state`'s golden only exercises `paused_recoverable` — this
// test both proves `state` is checked against the real 6-kind
// TurnRecoveryStateDto (until now that field wasn't checked at all: the
// switch case validated every other field but never `state`'s tag) and that
// an unrecognized kind is rejected rather than silently accepted.
test('turn_recovery_state validates its `state` kind, not just the surrounding fields', () => {
  const base = {
    session_id: '11111111-1111-4111-8111-111111111111',
    turn_id: 1,
    first_sequence: 1,
    last_sequence: 7,
    safe_to_resume: true,
  };
  for (const kind of ['running', 'waiting_for_user', 'paused_recoverable', 'completed', 'failed', 'cancelled']) {
    validateEvent(`turn_recovery_state(${kind})`, {
      type: 'turn_recovery_state',
      snapshot: { ...base, state: { type: kind } },
    });
  }
  assert.throws(
    () =>
      validateEvent('turn_recovery_state(bogus-state)', {
        type: 'turn_recovery_state',
        snapshot: { ...base, state: { type: 'bogus_state' } },
      }),
    'an unknown TurnRecoveryStateDto kind must be rejected',
  );
});

// AudioService uses operation identity and structured result/error DTOs;
// these cases pin every variant used by the desktop and native bindings.
const AUDIO_IDENTITY = { id: '00000000-0000-4000-8000-000000000001', generation: 3, service_epoch: 2 };
const AUDIO_OWNER = { type: 'session', session_id: 'session-1' };
const AUDIO_REQUEST = (operation: unknown) => ({
  type: 'audio_request',
  request: {
    identity: AUDIO_IDENTITY,
    owner: AUDIO_OWNER,
    initiator: { agent_id: 'agent-1', tool_use_id: 'tool-1', request_id: 'request-1' },
    timeout_budget_ms: 5_000,
    max_payload_bytes: 1_000_000,
    operation,
  },
});

test('audio_request covers every AudioOperationDto variant and its trusted context', () => {
  const operations = [
    { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    { type: 'stop_recording', handle: 'recording-1' },
    { type: 'listen' },
    { type: 'listen', language: 'en-US' },
    { type: 'synthesize', text: 'hello' },
    { type: 'synthesize', text: 'hello', language: 'en-US', rate: 1.25, voice: 'system:voice-1' },
    { type: 'speak', text: 'hello', voice: 'system:voice-1' },
    { type: 'status' },
    { type: 'status', handle: 'recording-1' },
    { type: 'end_owner' },
  ];
  for (const [index, operation] of operations.entries()) {
    const event = AUDIO_REQUEST(operation);
    validateEvent(`audio_request(${String((operation as { type: string }).type)}-${index})`, event);
    assert.deepEqual(validateClientEvent(event), event);
  }

  assert.throws(() => validateClientEvent({ type: 'audio_request' }), /audio operation request/);
  assert.throws(() => validateClientEvent(AUDIO_REQUEST({ type: 'stop_recording' })), /audio recording handle/);
  assert.throws(() => validateClientEvent({
    ...AUDIO_REQUEST({ type: 'listen' }),
    request: { ...AUDIO_REQUEST({ type: 'listen' }).request, owner: { type: 'session', session_id: 's', agent_id: 'model-claimed' } },
  }), /audio owner/);
  assert.throws(() => validateClientEvent({
    ...AUDIO_REQUEST({ type: 'listen' }),
    request: { ...AUDIO_REQUEST({ type: 'listen' }).request, identity: { ...AUDIO_IDENTITY, generation: -1 } },
  }), /generation/);
});

test('audio cancel and capability updates preserve the shared snapshot contract', () => {
  const capabilities = {
    service_epoch: 2,
    support_revision: 7,
    supported_operations: ['record', 'listen', 'synthesize', 'speak'],
    readiness: [
      { operation: 'record', state: 'needs_permission' },
      { operation: 'listen', state: 'missing_model' },
      { operation: 'synthesize', state: 'ready' },
      { operation: 'speak', state: 'busy' },
    ],
    max_payload_bytes: 1_000_000,
  };
  const cancel = { type: 'audio_cancel', identity: AUDIO_IDENTITY };
  assert.deepEqual(validateClientEvent(cancel), cancel);
  const changed = { type: 'audio_capabilities_changed', capabilities };
  assert.deepEqual(validateClientEvent(changed), changed);
  validateCommand('update_audio_capabilities', { type: 'update_audio_capabilities', capabilities });
  validateAudioCapabilities(capabilities);
  assert.throws(() => validateAudioCapabilities({ ...capabilities, support_revision: -1 }));
});

test('audio_response covers every AudioOperationResultDto and structured error kind', () => {
  const identity = AUDIO_IDENTITY;
  const successes = [
    { type: 'recording_started', handle: 'recording-1' },
    { type: 'recording', audio_base64: 'YWJj', mime_type: 'audio/wav' },
    { type: 'transcript', text: 'hello', language: 'en-US', confidence: 0.9 },
    { type: 'synthesized', pcm_base64: 'AQIDBA==', sample_rate_hz: 24_000 },
    { type: 'playback_completed', duration_ms: 500 },
    { type: 'status', status: { recording: false, playing: true } },
    { type: 'owner_ended' },
  ];
  for (const [index, result] of successes.entries()) {
    validateCommand(`audio_response(success-${index})`, { type: 'audio_response', identity, result });
  }

  const kinds = [
    'permission_denied', 'busy', 'cancelled', 'timeout', 'no_speech', 'not_recording',
    'unavailable', 'unsupported', 'model_missing', 'voice_missing', 'invalid_request',
    'synthesis_failed', 'native_failure', 'media_too_large',
  ];
  for (const kind of kinds) {
    const result = { type: 'failed', error: { kind, message: `${kind} happened` } };
    validateCommand(`audio_response(failed-${kind})`, { type: 'audio_response', identity, result });
    validateAudioResult(result);
  }
  assert.throws(
    () => validateAudioResult({ type: 'failed', error: { kind: 'invented', message: 'nope' } }),
    /unknown AudioErrorKindDto/,
  );
  assert.throws(
    () => validateCommand('audio_response(missing-identity)', { type: 'audio_response', result: { type: 'owner_ended' } }),
  );
});

test('a widened settings_snapshot validates its four new optional fields, and rejects a malformed one', () => {
  validateEvent('settings_snapshot(widened)', {
    type: 'settings_snapshot',
    effective_json: '{"model":"claude-opus-4-7"}',
    provenance_json: '{"model":"user-settings"}',
    files_json: '[{"layer":"user","path":"/home/user/.lingxi/settings.json","exists":true,"parsed":true}]',
    active_json: '{"model":"claude-opus-4-7"}',
    locked: ['model'],
    layers_json: '{"user":{"model":"claude-opus-4-7"}}',
  });
  assert.throws(
    () => validateEvent('settings_snapshot(bad-locked)', {
      type: 'settings_snapshot',
      effective_json: '{}',
      provenance_json: '{}',
      locked: [42],
    }),
    'a non-string entry in `locked` must be rejected',
  );
  assert.throws(
    () => validateEvent('settings_snapshot(bad-files_json)', {
      type: 'settings_snapshot',
      effective_json: '{}',
      provenance_json: '{}',
      files_json: 123,
    }),
    'a non-string `files_json` must be rejected',
  );
  assert.throws(
    () => validateEvent('settings_snapshot(bad-layers_json)', {
      type: 'settings_snapshot',
      effective_json: '{}',
      provenance_json: '{}',
      layers_json: 123,
    }),
    'a non-string `layers_json` must be rejected',
  );
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

test('audio bridge guards reject missing fields and stale context/frame properties', () => {
  for (const type of ['start_realtime_audio', 'realtime_audio_input']) assert.throws(() => validateCommand('missing-audio-fields.json', { type }));
  assert.throws(() => validateCommand('stale-audio-context-command.json', { type: 'get_audio_session_context', model: 'guessed/profile' }));
  assert.throws(() => validateEvent('missing-audio-context.json', { type: 'audio_session_context', session_id: 's', profile_id: 'p' }));
  assert.throws(() => validateEvent('missing-realtime-event.json', { type: 'realtime_audio_event', session_id: 's' }));
  assert.throws(() => validateEvent('stale-realtime-event.json', { type: 'realtime_audio_event', session_id: 's', event_json: '{}', audio: 'stale' }));
});
