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

function validateAppTemplateKind(v: unknown): void {
  assert.ok(
    ['dashboard', 'crud_tracker', 'content_showcase', 'form_utility'].includes(v as string),
  );
}

function validateAppWorkflowState(v: unknown): void {
  assert.ok(
    [
      'collecting_spec',
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
      assert.ok(Array.isArray(o['value']));
      for (const item of o['value'] as unknown[]) assert.ok(isString(item));
      break;
    case 'boolean':
      assert.ok(isBool(o['value']));
      break;
    case 'density':
      assert.ok(['compact', 'comfortable'].includes(o['value'] as string));
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

function validateAppRecord(v: unknown): void {
  const o = rec(v);
  assert.ok(
    isString(o['id']) &&
      isString(o['name']) &&
      isNumber(o['created_at_ms']) &&
      isNumber(o['updated_at_ms']) &&
      isString(o['workspace_rel']),
  );
  validateAppTemplateKind(o['template']);
  validateAppWorkflowState(o['workflow_state']);
  if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
}

function validateAppCheckpoint(v: unknown): void {
  const o = rec(v);
  assert.ok(isString(o['id']) && isString(o['label']) && isNumber(o['created_at_ms']));
  validateAppCheckpointKind(o['kind']);
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
      assert.ok(isString(o['name']));
      validateAppTemplateKind(o['template']);
      validateAppCreateOrigin(o['origin']);
      if ('conversation_id' in o) assert.ok(isString(o['conversation_id']));
      break;
    case 'open_app_designer':
    case 'cancel_app_design':
    case 'start_app':
    case 'stop_app':
    case 'restart_app':
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
  assert.equal(files.length, 40, `expected 40 command snapshots, found ${files.length}`);
  for (const file of files) {
    validateCommand(file, loadSnapshot('command', file));
  }
});

test('every event snapshot parses as ClientEvent', () => {
  const files = listSnapshots('event');
  assert.equal(files.length, 47, `expected 47 event snapshots, found ${files.length}`);
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
