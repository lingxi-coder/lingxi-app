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
  TaskRowDto,
} from '../src/protocol.js';
import {
  ALL_APP_EVENT_TYPES,
  ALL_CLIENT_COMMAND_TYPES,
  ALL_CLIENT_EVENT_TYPES,
  ALL_LOCAL_APP_PLUGIN_ERROR_CODES,
  ALL_MANAGED_LOCAL_APP_MCP_STATUS_TYPES,
  ALL_PLUGIN_COMMAND_TYPES,
  ALL_TASK_ROW_DTO_KEYS,
} from '../src/protocolCoverage.js';

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
 * `category` — read from each file's JSON CONTENT, never its filename:
 * several `event/` goldens share the `app_event` tag but are named after
 * their inner `AppEventDto` variant instead (e.g. `app_details_changed.json`
 * holds `{"type":"app_event","event":{"type":"app_details_changed",...}}`).
 */
function typeTagsOnDisk(category: string): Set<string> {
  const tags = new Set<string>();
  for (const file of listSnapshots(category)) {
    const { type } = loadSnapshot<{ type: string }>(category, file);
    tags.add(type);
  }
  return tags;
}

function nestedPluginCommandTypeTagsOnDisk(): Set<string> {
  const tags = new Set<string>();
  for (const file of listSnapshots('command')) {
    const snapshot = loadSnapshot<{ type: string; command?: { type?: string } }>('command', file);
    if (snapshot.type !== 'plugin_command') continue;
    assert.ok(snapshot.command && typeof snapshot.command.type === 'string', `command snapshot ${file} must carry command.type`);
    tags.add(snapshot.command.type);
  }
  return tags;
}

function nestedAppEventTypeTagsOnDisk(): Set<string> {
  const tags = new Set<string>();
  for (const file of listSnapshots('event')) {
    const snapshot = loadSnapshot<{ type: string; event?: { type?: string } }>('event', file);
    if (snapshot.type !== 'app_event') continue;
    assert.ok(snapshot.event && typeof snapshot.event.type === 'string', `event snapshot ${file} must carry event.type`);
    tags.add(snapshot.event.type);
  }
  return tags;
}

const APP_EVENT_TYPES_WITH_EXPLICIT_NON_SNAPSHOT_TESTS = [
  'app_created',
  'app_llm_activity_changed',
  'app_agent_event_posted',
  'app_background_task_changed',
  'app_bridge_stream_frame',
] as const;

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
// hand-maintained and `clients/shared/tsconfig.json` excludes `test/` from
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

// ── local_apps.rs validators (bare-string enums + kind/op-tagged DTOs) ────────

function validateAppWorkflowState(v: unknown): void {
  assert.ok(['draft', 'published_unverified', 'published_verified'].includes(v as string));
}

function validateAppRuntimeState(v: unknown): void {
  assert.ok(['stopped', 'starting', 'running', 'stopping', 'failed'].includes(v as string));
}

function validateAppCreateOrigin(v: unknown): void {
  assert.ok(['chat', 'library'].includes(v as string));
}

function validateAppSurface(v: unknown): void {
  assert.ok(['dom', 'canvas'].includes(v as string));
}

function validateAppCreateMode(v: unknown): void {
  assert.ok(['shell', 'scaffolded'].includes(v as string));
}

function validatePluginInventory(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    [
      'pluginId',
      'displayName',
      'source',
      'version',
      'bundleSha256',
      'state',
      'manifestDefaultEnabled',
      'counts',
    ],
    ['validationError'],
    'plugin inventory',
  );
  assert.ok(
    isString(o['pluginId']) &&
      isString(o['displayName']) &&
      isString(o['source']) &&
      isString(o['version']) &&
      isString(o['bundleSha256']) &&
      ['loaded', 'disabled'].includes(String(o['state'])) &&
      isBool(o['manifestDefaultEnabled']),
  );
  const counts = rec(o['counts']);
  exactObjectKeys(counts, ['skills', 'agents', 'workflows', 'templates'], [], 'plugin counts');
  assert.ok(
    isNumber(counts['skills']) &&
      isNumber(counts['agents']) &&
      isNumber(counts['workflows']) &&
      isNumber(counts['templates']),
  );
  if ('validationError' in o) assert.ok(isString(o['validationError']));
}

function validateVerificationSummary(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(o, ['status', 'summary'], ['code'], 'verification summary');
  assert.ok(
    ['pending', 'passed', 'failed', 'unverified', 'unavailable'].includes(String(o['status'])) &&
      isString(o['summary']),
  );
  if ('code' in o) assert.ok(isString(o['code']));
}

function validateGateStatus(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(o, ['gateId', 'label', 'status', 'available'], ['detail'], 'gate status');
  assert.ok(isString(o['gateId']) && isString(o['label']) && isBool(o['available']));
  validateVerificationSummary({ status: o['status'], summary: '', ...(o['detail'] === undefined ? {} : { code: '' }) });
  if ('detail' in o) assert.ok(isString(o['detail']));
}

function validateMcpToolSurface(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    ['name', 'inputSchemaJson', 'semanticFlowJson', 'permissionCeiling'],
    [
      'title',
      'description',
      'outputSchemaJson',
      'annotationsJson',
      'executionJson',
      'visibleMetaJson',
    ],
    'MCP tool surface',
  );
  assert.ok(
    isString(o['name']) &&
      isString(o['inputSchemaJson']) &&
      isString(o['semanticFlowJson']) &&
      isString(o['permissionCeiling']),
  );
  for (const key of [
    'title',
    'description',
    'outputSchemaJson',
    'annotationsJson',
    'executionJson',
    'visibleMetaJson',
  ]) {
    if (key in o) assert.ok(isString(o[key]));
  }
}

function validateRuntimeProfileOption(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    [
      'family',
      'revision',
      'contractSha256',
      'surface',
      'corePackages',
      'cacheStatus',
      'downloadStatus',
      'available',
    ],
    ['reason'],
    'runtime profile option',
  );
  assert.ok(
    ['react_dom', 'canvas_2d', 'three_3d', 'phaser_2d', 'babylon_3d'].includes(
      String(o['family']),
    ) &&
      isNumber(o['revision']) &&
      isString(o['contractSha256']) &&
      isString(o['cacheStatus']) &&
      isString(o['downloadStatus']) &&
      isBool(o['available']),
  );
  validateAppSurface(o['surface']);
  assert.ok(Array.isArray(o['corePackages']));
  for (const value of o['corePackages'] as unknown[]) {
    const pkg = rec(value);
    exactObjectKeys(pkg, ['name', 'version'], [], 'runtime profile package');
    assert.ok(isString(pkg['name']) && isString(pkg['version']));
  }
  if ('reason' in o) assert.ok(isString(o['reason']));
}

function validateCreateConfirmationRequest(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    ['requestId', 'appId', 'name', 'brief', 'selectedTemplate', 'runtimeProfile', 'reason'],
    ['rejected', 'initialTools', 'requiredGates'],
    'create confirmation request',
  );
  assert.ok(
    isString(o['requestId']) &&
      isString(o['appId']) &&
      isString(o['name']) &&
      isString(o['brief']) &&
      isString(o['reason']),
  );
  const selected = rec(o['selectedTemplate']);
  exactObjectKeys(selected, ['templateId', 'surface', 'summary'], [], 'selected template');
  assert.ok(isString(selected['templateId']) && isString(selected['summary']));
  validateAppSurface(selected['surface']);
  validateRuntimeProfileOption(o['runtimeProfile']);
  if ('rejected' in o) {
    assert.ok(Array.isArray(o['rejected']));
    for (const value of o['rejected'] as unknown[]) {
      const rejected = rec(value);
      exactObjectKeys(rejected, ['templateId', 'reason'], [], 'rejected template');
      assert.ok(isString(rejected['templateId']) && isString(rejected['reason']));
    }
  }
  if ('initialTools' in o) {
    assert.ok(Array.isArray(o['initialTools']));
    for (const tool of o['initialTools'] as unknown[]) validateMcpToolSurface(tool);
  }
  if ('requiredGates' in o) {
    assert.ok(Array.isArray(o['requiredGates']));
    for (const gate of o['requiredGates'] as unknown[]) validateGateStatus(gate);
  }
}

function validateProposalApprovalRequest(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    [
      'requestId',
      'appId',
      'workflowRunId',
      'summary',
      'proposalSha256',
      'approvalContractSha256',
      'toolSurfaceSha256',
    ],
    [
      'toolDiffs',
      'requiredFlowChanges',
      'excludedCapabilities',
      'pendingGates',
    ],
    'MCP proposal approval request',
  );
  assert.ok(
    isString(o['requestId']) &&
      isString(o['appId']) &&
      isString(o['workflowRunId']) &&
      isString(o['summary']) &&
      isString(o['proposalSha256']) &&
      isString(o['approvalContractSha256']) &&
      isString(o['toolSurfaceSha256']),
  );
  if ('toolDiffs' in o) {
    assert.ok(Array.isArray(o['toolDiffs']));
    for (const value of o['toolDiffs'] as unknown[]) {
      const diff = rec(value);
      exactObjectKeys(diff, ['kind', 'name'], ['before', 'after', 'changedFields'], 'MCP tool diff');
      assert.ok(['added', 'removed', 'changed'].includes(String(diff['kind'])) && isString(diff['name']));
      if ('before' in diff) validateMcpToolSurface(diff['before']);
      if ('after' in diff) validateMcpToolSurface(diff['after']);
      if ('changedFields' in diff) {
        assert.ok(Array.isArray(diff['changedFields']));
        const fields = [
          'name',
          'title',
          'description',
          'input_schema',
          'output_schema',
          'annotations',
          'execution',
          'visible_meta',
          'semantic_flow',
          'permission_ceiling',
        ];
        for (const field of diff['changedFields'] as unknown[]) assert.ok(fields.includes(String(field)));
      }
    }
  }
  for (const key of ['requiredFlowChanges', 'excludedCapabilities']) {
    if (key in o) {
      assert.ok(Array.isArray(o[key]));
      for (const value of o[key] as unknown[]) assert.ok(isString(value));
    }
  }
  if ('pendingGates' in o) {
    assert.ok(Array.isArray(o['pendingGates']));
    for (const gate of o['pendingGates'] as unknown[]) validateGateStatus(gate);
  }
}

function validateManagedMcpServer(v: unknown): void {
  const o = rec(v);
  exactObjectKeys(
    o,
    [
      'serverName',
      'appId',
      'appName',
      'enabled',
      'status',
      'settingsRevision',
      'enabledTools',
      'pinnedToCurrentConversation',
      'buildId',
      'catalogSha256',
      'toolSurfaceSha256',
      'toolCount',
      'authoringRevision',
      'publicationState',
      'mcpVerification',
      'uiVerification',
    ],
    ['tools', 'widget'],
    'managed MCP server',
  );
  assert.ok(
      isString(o['serverName']) &&
      isString(o['appId']) &&
      isString(o['appName']) &&
      isBool(o['enabled']) &&
      ['disabled', 'needs_setup', 'authoring', 'enabled', 'needs_revalidation', 'error'].includes(String(o['status'])) &&
      isNumber(o['settingsRevision']) &&
      Array.isArray(o['enabledTools']) &&
      (o['enabledTools'] as unknown[]).every(isString) &&
      isBool(o['pinnedToCurrentConversation']) &&
      isString(o['buildId']) &&
      isString(o['catalogSha256']) &&
      isString(o['toolSurfaceSha256']) &&
      isNumber(o['toolCount']) &&
      isNumber(o['authoringRevision']),
  );
  validateAppWorkflowState(o['publicationState']);
  validateVerificationSummary(o['mcpVerification']);
  validateVerificationSummary(o['uiVerification']);
  if ('widget' in o && o['widget'] !== null) {
    const widget = rec(o['widget']);
    exactObjectKeys(widget, ['resourceUri', 'mimeType', 'resourceSha256'], [], 'managed MCP widget');
    assert.ok(
      isString(widget['resourceUri']) &&
        isString(widget['mimeType']) &&
        isString(widget['resourceSha256']),
    );
  }
  if ('tools' in o) {
    assert.ok(Array.isArray(o['tools']));
    for (const tool of o['tools'] as unknown[]) validateMcpToolSurface(tool);
  }
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
      ['chat', 'code'].includes(String(o['mode'])) &&
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
  'background_schedule',
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
      isNumber(o['runtime_api_version']) &&
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
  }
  if ('surface' in o) {
    assert.ok(['dom', 'canvas'].includes(o['surface'] as string));
  }
  if ('runtime_profile' in o) {
    const profile = rec(o['runtime_profile']);
    assert.ok(
      ['react_dom', 'canvas_2d', 'three_3d', 'phaser_2d', 'babylon_3d'].includes(
        profile['family'] as string,
      ) &&
        isNumber(profile['revision']) &&
        isString(profile['contractSha256']),
    );
  }
  if ('dependency_snapshot' in o) {
    const snapshot = rec(o['dependency_snapshot']);
    for (const key of [
      'requestedSha256',
      'packageSha256',
      'lockfileSha256',
      'dependencyTreeSha256',
      'sbomSha256',
      'toolchainKey',
      'verifiedProfileContractSha256',
    ]) {
      assert.ok(isString(snapshot[key]), `invalid dependency snapshot ${key}`);
    }
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
      'background_schedule',
      'background_list',
      'background_status',
      'background_cancel',
      'background_retry',
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
      'capture_view',
      'pointer',
      'key',
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

function validateDependencyChangeConfirmationRequest(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['requestId']) &&
      isString(o['appId']) &&
      isString(o['reason']) &&
      isString(o['licenseRisk']) &&
      isString(o['sbomRisk']) &&
      isBool(o['lifecycleScriptsBlocked']) &&
      isBool(o['nativeAddonsBlocked']) &&
      isString(o['rollbackPolicy']),
  );
  assert.ok(Array.isArray(o['changes']));
  for (const rawChange of o['changes'] as unknown[]) {
    const change = rec(rawChange);
    assert.ok(
      ['add', 'update', 'remove'].includes(String(change['kind'])) &&
        isString(change['package']) &&
        isString(change['cacheStatus']) &&
        isString(change['downloadStatus']),
    );
    if ('version' in change) assert.ok(isString(change['version']));
  }
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
      isString(o['workspace_rel']) &&
      // REQUIRED, not `if ('scaffolded' in o)`: the Rust field carries no
      // serde default, so a record without it is not a record.
      isBool(o['scaffolded']),
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
  if ('runtime_profile_status' in o) {
    assert.ok(
      [
        'verified',
        'dependencies_dirty',
        'core_dependency_drift',
        'rebuild_required',
        'migration_available',
        'runtime_bundle_missing',
        'runtime_contract_corrupt',
      ].includes(o['runtime_profile_status'] as string),
    );
  }
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
    case 'app_created':
      validateAppRecord(o['record']);
      if ('request_id' in o) assert.ok(isString(o['request_id']));
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
    case 'app_dependency_change_confirmation_requested':
      validateDependencyChangeConfirmationRequest(o['request']);
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
    case 'app_background_task_changed':
      assert.ok(
        isString(o['app_id']) &&
          isString(o['task_id']) &&
          isString(o['status']) &&
          isBool(o['retryable']),
      );
      if ('result_json' in o) assert.ok(isString(o['result_json']));
      if ('error' in o) assert.ok(isString(o['error']));
      break;
    case 'plugin_status_changed': {
      const st = o['status'];
      const status = rec(st);
      const keys = Object.keys(status).sort().join(',');
      assert.equal(keys, 'manifest_default_enabled,plugin_id,state',
        `plugin status must carry exactly {plugin_id, state, manifest_default_enabled}, got {${keys}}`);
      assert.ok(isString(status['plugin_id']) && isString(status['state'])
        && isBool(status['manifest_default_enabled']));
      break;
    }
    case 'plugin_inventory_changed':
      exactObjectKeys(o, ['type', 'inventory'], [], 'plugin_inventory_changed event');
      validatePluginInventory(o['inventory']);
      break;
    case 'create_confirmation_requested':
      exactObjectKeys(o, ['type', 'request'], [], 'create_confirmation_requested event');
      validateCreateConfirmationRequest(o['request']);
      break;
    case 'mcp_proposal_approval_requested':
      exactObjectKeys(o, ['type', 'request'], [], 'mcp_proposal_approval_requested event');
      validateProposalApprovalRequest(o['request']);
      break;
    case 'managed_mcp_inventory_changed':
      exactObjectKeys(o, ['type', 'servers'], [], 'managed_mcp_inventory_changed event');
      assert.ok(Array.isArray(o['servers']));
      for (const server of o['servers'] as unknown[]) validateManagedMcpServer(server);
      break;
    case 'verification_summary_changed':
      exactObjectKeys(
        o,
        ['type', 'app_id', 'publication_state', 'mcp_verification', 'ui_verification'],
        [],
        'verification_summary_changed event',
      );
      assert.ok(isString(o['app_id']));
      validateAppWorkflowState(o['publication_state']);
      validateVerificationSummary(o['mcp_verification']);
      validateVerificationSummary(o['ui_verification']);
      break;
    case 'local_app_operation_failed':
      exactObjectKeys(
        o,
        ['type', 'code', 'message'],
        ['app_id', 'request_id'],
        'local_app_operation_failed event',
      );
      assert.ok(
        [
          'plugin_disabled',
          'builtin_bundle_unavailable',
          'template_unavailable',
          'proposal_invalid',
          'catalog_stale',
          'active_state_corrupt',
          'revision_conflict',
          'invalid_mcp_settings',
          'mcp_authoring_required',
          'repair_budget_exhausted',
          'exposure_capacity_reached',
        ].includes(String(o['code'])) && isString(o['message']),
      );
      if ('app_id' in o) assert.ok(isString(o['app_id']));
      if ('request_id' in o) assert.ok(isString(o['request_id']));
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

// ── Audio (events.rs `AudioOpDto`, commands.rs `AudioResultDto`/`AudioErrorKindDto`) ──

function validateAudioOp(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'start_recording':
      assert.ok(isNumber(o['sample_rate_hz']) && isString(o['format']));
      break;
    case 'stop_recording':
    case 'is_recording':
      break;
    case 'transcribe':
      if ('language' in o) assert.ok(isString(o['language']));
      break;
    case 'synthesize':
      assert.ok(isString(o['text']));
      if ('voice' in o) assert.ok(isString(o['voice']));
      break;
    default:
      assert.fail(`unknown AudioOpDto type: ${String(o['type'])}`);
  }
}

// The 8 kinds exist specifically so `SttError`/`VoiceError`/`TtsError` round-trip
// without losing distinctions (client-protocol/src/commands.rs `AudioErrorKindDto`).
function validateAudioErrorKind(v: unknown): void {
  assert.ok(
    [
      'permission_denied',
      'no_speech',
      'not_recording',
      'unavailable',
      'busy',
      'retriable',
      'synthesis_failed',
      'other',
    ].includes(v as string),
    `unknown AudioErrorKindDto "${String(v)}"`,
  );
}

function validateAudioResult(v: unknown): void {
  const o = rec(v);
  switch (o['type']) {
    case 'ok':
      break;
    case 'recording_state':
      assert.ok(isBool(o['recording']));
      break;
    case 'recording':
      assert.ok(isString(o['audio_base64']) && isString(o['mime_type']));
      break;
    case 'transcript':
      assert.ok(isString(o['text']));
      if ('language' in o) assert.ok(isString(o['language']));
      if ('confidence' in o) assert.ok(isNumber(o['confidence']));
      break;
    case 'audio':
      assert.ok(isString(o['pcm_base64']) && isNumber(o['sample_rate_hz']));
      break;
    case 'failed':
      validateAudioErrorKind(o['kind']);
      assert.ok(isString(o['message']));
      break;
    default:
      assert.fail(`unknown AudioResultDto type: ${String(o['type'])}`);
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
    case 'list_apps':
      break;
    case 'create_app':
      assert.ok(isString(o['name']) && isString(o['brief']));
      validateAppCreateOrigin(o['origin']);
      if ('git_enabled' in o) assert.ok(isBool(o['git_enabled']));
      if ('workflow_model' in o) assert.ok(isString(o['workflow_model']));
      if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
      if ('surface' in o) validateAppSurface(o['surface']);
      validateAppCreateMode(o['mode']);
      if ('request_id' in o) assert.ok(isString(o['request_id']));
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
    case 'resolve_app_dependency_change_confirmation':
      assert.ok(isString(o['request_id']) && isBool(o['approved']));
      break;
    case 'resolve_app_profile_proposal':
      assert.ok(
        isString(o['app_id']) &&
          isString(o['approval_token']) &&
          isBool(o['approved']),
      );
      break;
    case 'resolve_app_runtime_profile_selection':
      assert.ok(
        isString(o['request_id']) &&
          ['react_dom', 'canvas_2d', 'three_3d', 'phaser_2d', 'babylon_3d'].includes(o['selected_family'] as string),
      );
      break;
    case 'restore_app_checkpoint':
      assert.ok(isString(o['app_id']) && isString(o['checkpoint_id']));
      break;
    case 'plugin_command': {
      // Exact key sets. The Rust goldens contract "carries exactly
      // {plugin_id, enabled}" / "{plugin_id}" — a fourth key is a contract
      // change, not a detail, so assert the SET rather than the presence of
      // the fields we happen to expect.
      const c = o['command'];
      const cmd = rec(c);
      const keys = Object.keys(cmd).sort().join(',');
      switch (cmd['type']) {
        case 'set_enabled':
          assert.equal(
            keys,
            'enabled,plugin_id,type',
            `snapshot ${name}: set_enabled must carry exactly {plugin_id, enabled}, got {${keys}}`,
          );
          assert.ok(isString(cmd['plugin_id']) && isBool(cmd['enabled']));
          break;
        case 'get_status':
          assert.equal(
            keys,
            'plugin_id,type',
            `snapshot ${name}: get_status must carry exactly {plugin_id}, got {${keys}}`,
          );
          assert.ok(isString(cmd['plugin_id']));
          break;
        case 'get_inventory':
          assert.equal(
            keys,
            'plugin_id,type',
            `snapshot ${name}: get_inventory must carry exactly {plugin_id}, got {${keys}}`,
          );
          assert.ok(isString(cmd['plugin_id']));
          break;
        case 'resolve_create_confirmation':
          assert.equal(
            keys,
            'approved,request_id,type',
            `snapshot ${name}: resolve_create_confirmation must carry exactly {request_id, approved}, got {${keys}}`,
          );
          assert.ok(isString(cmd['request_id']) && isBool(cmd['approved']));
          break;
        case 'resolve_mcp_proposal_approval':
          assert.equal(
            keys,
            'approved,request_id,type',
            `snapshot ${name}: resolve_mcp_proposal_approval must carry exactly {request_id, approved}, got {${keys}}`,
          );
          assert.ok(isString(cmd['request_id']) && isBool(cmd['approved']));
          break;
        case 'get_managed_mcp_inventory':
          assert.equal(
            keys,
            'type',
            `snapshot ${name}: get_managed_mcp_inventory must carry no fields, got {${keys}}`,
          );
          break;
        case 'start_local_app_mcp_authoring':
          assert.equal(keys, 'app_id,type,user_goal');
          assert.ok(isString(cmd['app_id']) && isString(cmd['user_goal']));
          break;
        case 'set_local_app_mcp_enabled':
          assert.equal(keys, 'app_id,enabled,expected_revision,type');
          assert.ok(
            isString(cmd['app_id']) &&
              isBool(cmd['enabled']) &&
              isNumber(cmd['expected_revision']),
          );
          break;
        case 'set_local_app_mcp_tool_enabled':
          assert.equal(keys, 'app_id,enabled,expected_revision,tool_name,type');
          assert.ok(
            isString(cmd['app_id']) &&
              isBool(cmd['enabled']) &&
              isNumber(cmd['expected_revision']) &&
              isString(cmd['tool_name']),
          );
          break;
        case 'set_local_app_mcp_conversation_pinned':
          assert.equal(keys, 'app_id,conversation_id,pinned,type');
          assert.ok(
            isString(cmd['app_id']) &&
              isString(cmd['conversation_id']) &&
              isBool(cmd['pinned']),
          );
          break;
        default:
          assert.fail(`snapshot ${name}: unknown PluginCommandDto type "${String(cmd['type'])}"`);
      }
      break;
    }
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
    case 'audio_response':
      assert.ok(isNumber(o['request_id']));
      validateAudioResult(o['result']);
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
      if ('request_id' in o) assert.ok(isString(o['request_id']));
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
    case 'audio_request':
      assert.ok(isNumber(o['request_id']));
      validateAudioOp(o['op']);
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

test('workflow model metadata and paused task status pass the wire guards', () => {
  validateTaskStatus({ type: 'paused' });
  validateCommand('create_app.json', {
    type: 'create_app',
    name: 'Demo',
    origin: 'chat',
    brief: 'Demo app',
    workflow_model: 'deepseek/deepseek-flash',
    mode: 'scaffolded',
  });
  // The "+" button's shape: an empty shell, no surface, correlated by a
  // client-generated request id.
  validateCommand('create_app.json', {
    type: 'create_app',
    name: '',
    origin: 'library',
    brief: '',
    mode: 'shell',
    request_id: 'req-1',
  });
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
test('ALL_CLIENT_COMMAND_TYPES matches the `type` tags on disk exactly', () => {
  const onDisk = [...typeTagsOnDisk('command')].sort();
  const declared = Object.keys(ALL_CLIENT_COMMAND_TYPES).sort();
  assert.deepEqual(
    declared,
    onDisk,
    'ALL_CLIENT_COMMAND_TYPES (src/protocolCoverage.ts) must declare exactly the `type` tags carried by the command goldens — no more, no fewer',
  );
});

test('ALL_CLIENT_EVENT_TYPES matches the `type` tags on disk exactly', () => {
  const onDisk = [...typeTagsOnDisk('event')].sort();
  const declared = Object.keys(ALL_CLIENT_EVENT_TYPES).sort();
  assert.deepEqual(
    declared,
    onDisk,
    'ALL_CLIENT_EVENT_TYPES (src/protocolCoverage.ts) must declare exactly the `type` tags carried by the event goldens — no more, no fewer',
  );
});

test('ALL_PLUGIN_COMMAND_TYPES matches the nested plugin_command tags on disk exactly', () => {
  const onDisk = [...nestedPluginCommandTypeTagsOnDisk()].sort();
  const declared = Object.keys(ALL_PLUGIN_COMMAND_TYPES).sort();
  assert.deepEqual(
    declared,
    onDisk,
    'ALL_PLUGIN_COMMAND_TYPES (src/protocolCoverage.ts) must declare exactly the nested PluginCommandDto tags carried by the command goldens — no more, no fewer',
  );
});

test('ALL_APP_EVENT_TYPES matches the nested app_event tags on disk plus explicit non-snapshot coverage', () => {
  const onDisk = new Set([
    ...nestedAppEventTypeTagsOnDisk(),
    ...APP_EVENT_TYPES_WITH_EXPLICIT_NON_SNAPSHOT_TESTS,
  ]);
  const declared = Object.keys(ALL_APP_EVENT_TYPES).sort();
  assert.deepEqual(
    APP_EVENT_TYPES_WITH_EXPLICIT_NON_SNAPSHOT_TESTS,
    [
      'app_created',
      'app_llm_activity_changed',
      'app_agent_event_posted',
      'app_background_task_changed',
      'app_bridge_stream_frame',
    ],
    'keep the explicit non-snapshot coverage list intentional and audited',
  );
  assert.deepEqual(
    declared,
    [...onDisk].sort(),
    'ALL_APP_EVENT_TYPES (src/protocolCoverage.ts) must declare exactly the nested AppEventDto tags covered by snapshots plus explicit non-snapshot tests — no more, no fewer',
  );
});

test('local app status/code coverage tables stay exhaustive for the native control plane', () => {
  assert.deepEqual(
    Object.keys(ALL_MANAGED_LOCAL_APP_MCP_STATUS_TYPES).sort(),
    ['authoring', 'disabled', 'enabled', 'error', 'needs_revalidation', 'needs_setup'],
  );
  assert.deepEqual(
    Object.keys(ALL_LOCAL_APP_PLUGIN_ERROR_CODES).sort(),
    [
      'active_state_corrupt',
      'builtin_bundle_unavailable',
      'catalog_stale',
      'exposure_capacity_reached',
      'invalid_mcp_settings',
      'mcp_authoring_required',
      'plugin_disabled',
      'proposal_invalid',
      'repair_budget_exhausted',
      'revision_conflict',
      'template_unavailable',
    ],
  );
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

// The on-disk goldens only exercise `AudioOpDto::Transcribe` and
// `AudioResultDto::Transcript` — every other op/result shape (including all
// 8 `AudioErrorKindDto` kinds, which exist specifically so `SttError`/
// `VoiceError`/`TtsError` round-trip without losing distinctions) has no
// golden, so without this test those branches are dead code that always
// "passes".
test('audio_request covers every AudioOpDto variant', () => {
  validateEvent('audio_request(start_recording)', {
    type: 'audio_request',
    request_id: 1,
    op: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
  });
  validateEvent('audio_request(stop_recording)', {
    type: 'audio_request',
    request_id: 2,
    op: { type: 'stop_recording' },
  });
  validateEvent('audio_request(is_recording)', {
    type: 'audio_request',
    request_id: 3,
    op: { type: 'is_recording' },
  });
  validateEvent('audio_request(transcribe-no-language)', {
    type: 'audio_request',
    request_id: 4,
    op: { type: 'transcribe' },
  });
  validateEvent('audio_request(synthesize)', {
    type: 'audio_request',
    request_id: 5,
    op: { type: 'synthesize', text: 'hello', voice: 'default' },
  });
  validateEvent('audio_request(synthesize-no-voice)', {
    type: 'audio_request',
    request_id: 6,
    op: { type: 'synthesize', text: 'hello' },
  });

  // `request_id`/`op` are required on the envelope itself.
  assert.throws(
    () => validateEvent('audio_request(missing-request_id)', { type: 'audio_request', op: { type: 'is_recording' } }),
    'a missing `request_id` must be rejected',
  );
  assert.throws(
    () => validateEvent('audio_request(missing-op)', { type: 'audio_request', request_id: 1 }),
    'a missing `op` must be rejected',
  );

  // `start_recording` requires both `sample_rate_hz` and `format`.
  assert.throws(
    () =>
      validateEvent('audio_request(start_recording-no-rate)', {
        type: 'audio_request',
        request_id: 1,
        op: { type: 'start_recording', format: 'wav' },
      }),
    'a `start_recording` missing `sample_rate_hz` must be rejected',
  );
  assert.throws(
    () =>
      validateEvent('audio_request(start_recording-no-format)', {
        type: 'audio_request',
        request_id: 1,
        op: { type: 'start_recording', sample_rate_hz: 16_000 },
      }),
    'a `start_recording` missing `format` must be rejected',
  );

  // `synthesize` requires `text` (`voice` is optional).
  assert.throws(
    () =>
      validateEvent('audio_request(synthesize-no-text)', {
        type: 'audio_request',
        request_id: 1,
        op: { type: 'synthesize', voice: 'default' },
      }),
    'a `synthesize` missing `text` must be rejected',
  );
});

test('audio_response covers every AudioResultDto variant, including all 8 AudioErrorKindDto kinds', () => {
  validateCommand('audio_response(ok)', { type: 'audio_response', request_id: 1, result: { type: 'ok' } });
  validateCommand('audio_response(recording_state)', {
    type: 'audio_response',
    request_id: 2,
    result: { type: 'recording_state', recording: true },
  });
  validateCommand('audio_response(recording)', {
    type: 'audio_response',
    request_id: 3,
    result: { type: 'recording', audio_base64: 'YWJj', mime_type: 'audio/m4a' },
  });
  validateCommand('audio_response(audio)', {
    type: 'audio_response',
    request_id: 4,
    result: { type: 'audio', pcm_base64: 'YWJj', sample_rate_hz: 22_050 },
  });

  const AUDIO_ERROR_KINDS = [
    'permission_denied',
    'no_speech',
    'not_recording',
    'unavailable',
    'busy',
    'retriable',
    'synthesis_failed',
    'other',
  ];
  assert.equal(AUDIO_ERROR_KINDS.length, 8);
  for (const kind of AUDIO_ERROR_KINDS) {
    validateCommand(`audio_response(failed-${kind})`, {
      type: 'audio_response',
      request_id: 5,
      result: { type: 'failed', kind, message: `${kind} happened` },
    });
  }
  assert.throws(
    () =>
      validateCommand('audio_response(failed-unknown-kind)', {
        type: 'audio_response',
        request_id: 5,
        result: { type: 'failed', kind: 'bogus_kind', message: 'nope' },
      }),
    'an unknown AudioErrorKindDto must be rejected',
  );

  // `request_id`/`result` are required on the envelope itself.
  assert.throws(
    () =>
      validateCommand('audio_response(missing-request_id)', {
        type: 'audio_response',
        result: { type: 'ok' },
      }),
    'a missing `request_id` must be rejected',
  );
  assert.throws(
    () => validateCommand('audio_response(missing-result)', { type: 'audio_response', request_id: 1 }),
    'a missing `result` must be rejected',
  );

  // `recording_state` requires `recording`.
  assert.throws(
    () =>
      validateCommand('audio_response(recording_state-no-flag)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'recording_state' },
      }),
    'a `recording_state` missing `recording` must be rejected',
  );

  // `recording` requires both `audio_base64` and `mime_type`.
  assert.throws(
    () =>
      validateCommand('audio_response(recording-no-audio)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'recording', mime_type: 'audio/m4a' },
      }),
    'a `recording` missing `audio_base64` must be rejected',
  );
  assert.throws(
    () =>
      validateCommand('audio_response(recording-no-mime)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'recording', audio_base64: 'YWJj' },
      }),
    'a `recording` missing `mime_type` must be rejected',
  );

  // `audio` requires both `pcm_base64` and `sample_rate_hz`.
  assert.throws(
    () =>
      validateCommand('audio_response(audio-no-pcm)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'audio', sample_rate_hz: 22_050 },
      }),
    'an `audio` missing `pcm_base64` must be rejected',
  );
  assert.throws(
    () =>
      validateCommand('audio_response(audio-no-rate)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'audio', pcm_base64: 'YWJj' },
      }),
    'an `audio` missing `sample_rate_hz` must be rejected',
  );

  // `transcript` requires `text` (`language`/`confidence` are optional).
  assert.throws(
    () =>
      validateCommand('audio_response(transcript-no-text)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'transcript', language: 'en-US' },
      }),
    'a `transcript` missing `text` must be rejected',
  );

  // `failed` requires both `kind` and `message`.
  assert.throws(
    () =>
      validateCommand('audio_response(failed-no-kind)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'failed', message: 'nope' },
      }),
    'a `failed` missing `kind` must be rejected',
  );
  assert.throws(
    () =>
      validateCommand('audio_response(failed-no-message)', {
        type: 'audio_response',
        request_id: 1,
        result: { type: 'failed', kind: 'other' },
      }),
    'a `failed` missing `message` must be rejected',
  );
});

// `settings_snapshot`'s four widened fields (files_json/active_json/locked/
// layers_json) have no golden that exercises them — the on-disk snapshot
// predates the widening. Without this test the four `if (...)` checks in the
// `settings_snapshot` case are dead code that always "passes".
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

test('background task app events mirror the Rust wire contract', () => {
  validateAppEvent({
    type: 'app_background_task_changed',
    app_id: 'abc12345',
    task_id: 'background-1',
    status: 'succeeded',
    result_json: '{"ok":true}',
    retryable: false,
  });
});

test('llm-activity and mailbox app events remain in the shared AppEventDto union', () => {
  validateAppEvent({
    type: 'app_llm_activity_changed',
    app_id: 'abc12345',
    active: true,
  });
  validateAppEvent({
    type: 'app_agent_event_posted',
    app_id: 'abc12345',
    seq: 7,
    topic: 'mailbox:new',
    created_at_ms: 1_750_000_000_000,
  });
});

// `app_created` has no golden snapshot, so nothing under `snapshots/event/`
// reaches its branch in `validateAppEvent` — without this test the guard for
// the create-flow correlation key is dead code that always "passes".
test('app_created carries a full record and an optional correlation key', () => {
  const record = {
    id: 'abc12345',
    name: 'Habits',
    brief: 'A daily habit tracker',
    git_enabled: true,
    created_at_ms: 1750000000000,
    updated_at_ms: 1750000000001,
    workflow_state: 'draft',
    workspace_rel: 'apps/abc12345/workspace',
    scaffolded: false,
  };

  // The "+" button's own creation: the key it sent comes back verbatim, and it
  // arrives through the real `app_event` envelope, not just the inner helper.
  validateEvent('app_event(app_created)', {
    type: 'app_event',
    event: { type: 'app_created', record, request_id: 'req-1' },
  });

  // An agent-driven create has no client request behind it — the key is
  // absent, and absent is legal.
  validateAppEvent({ type: 'app_created', record });

  // …but a PRESENT key must be a string. This is what stops the branch from
  // being vacuous: without the `isString` check the case below would pass.
  assert.throws(
    () => validateAppEvent({ type: 'app_created', record, request_id: 42 }),
    /request_id|falsy/,
    'a non-string correlation key must be rejected',
  );

  // `scaffolded` is REQUIRED on the record — a shell that omits it must not
  // slip through as a formed app.
  const { scaffolded: _dropped, ...withoutScaffolded } = record;
  assert.throws(
    () => validateAppEvent({ type: 'app_created', record: withoutScaffolded }),
    'a record without `scaffolded` must be rejected',
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
