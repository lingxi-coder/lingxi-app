import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  assertCommandAllowedDuringTurn,
  isAllowedIpcSender,
  validateAskUserQuestionAnswers,
  validateBridgeLockfile,
  validateClientCommand,
  validateComputerAccessResponse,
  validatePermissionResponse,
  validatePrompt,
} from '../src/main/validation';

test('only the bounded Desktop command surface passes the runtime allowlist', () => {
  assert.deepEqual(validateClientCommand({ type: 'list_models' }), { type: 'list_models' });
  assert.deepEqual(validateClientCommand({ type: 'list_sessions', limit: 25 }), { type: 'list_sessions', limit: 25 });
  assert.deepEqual(validateClientCommand({ type: 'task_output', task_id: 'task-1', offset: 0 }), { type: 'task_output', task_id: 'task-1', offset: 0 });
  assert.deepEqual(validateClientCommand({ type: 'task_list', status_filter: { type: 'paused' } }), { type: 'task_list', status_filter: { type: 'paused' } });
  assert.deepEqual(validateClientCommand({ type: 'set_permission_mode', mode: 'auto' }), { type: 'set_permission_mode', mode: 'auto' });
  assert.deepEqual(validateClientCommand({ type: 'run_slash_command', raw: '/model opus' }), { type: 'run_slash_command', raw: '/model opus' });
  assert.deepEqual(
    validateClientCommand({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] }),
    { type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] },
  );
  assert.throws(() => validateClientCommand({ type: 'send_prompt', text: 'bypass' }), /not allowed/);
  assert.throws(() => validateClientCommand({ type: 'request_exit' }), /not allowed/);
  assert.throws(() => validateClientCommand({ type: 'list_sessions', limit: 201 }), /invalid limit/);
  assert.throws(() => validateClientCommand({ type: 'list_models', surprise: true }), /unsupported fields/);
  assert.throws(() => validateClientCommand({ type: 'refresh_listings', which: [{ type: 'hooks' }] }), /not allowed/);
  assert.throws(() => validateClientCommand({ type: 'set_permission_mode', mode: 'unsafe' }), /invalid permission mode/);
  assert.throws(() => validateClientCommand({ type: 'run_slash_command', raw: 'model opus' }), /invalid slash command/);
  assert.throws(() => validateClientCommand({ type: 'run_slash_command', raw: '/' }), /invalid slash command/);
  assert.throws(() => validateClientCommand({ type: 'task_list', status_filter: { type: ['paused'] } }), /invalid task status/);
  assert.throws(
    () => validateClientCommand({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }, { type: 'status' }] }),
    /invalid listing selection/,
  );
});

test('AskUserQuestion answers are bounded non-empty string maps', () => {
  assert.deepEqual(
    validateAskUserQuestionAnswers({ 'Choose a mode': 'Safe, Fast' }),
    { 'Choose a mode': 'Safe, Fast' },
  );
  assert.throws(() => validateAskUserQuestionAnswers({}), /invalid AskUserQuestion answers/);
  assert.throws(() => validateAskUserQuestionAnswers({ Question: '   ' }), /invalid AskUserQuestion answer/);
  assert.throws(
    () => validateAskUserQuestionAnswers(Object.fromEntries(
      Array.from({ length: 5 }, (_, index) => [`Question ${index}`, 'Answer']),
    )),
    /invalid AskUserQuestion answers/,
  );
});

test('session cwd is pinned to the active workspace', () => {
  assert.deepEqual(validateClientCommand({ type: 'new_session' }, '/workspace'), { type: 'new_session', cwd: '/workspace' });
  assert.throws(() => validateClientCommand({ type: 'new_session', cwd: '/other' }, '/workspace'), /must match/);
});

test('active turns reject model and session mutation but retain recovery commands', () => {
  for (const command of [
    { type: 'set_model', model: 'claude-sonnet' },
    { type: 'set_permission_mode', mode: 'acceptEdits' },
    { type: 'run_slash_command', raw: '/clear' },
    { type: 'new_session' },
    { type: 'resume_session', session_id: 'session-1' },
  ] as const) {
    assert.throws(
      () => assertCommandAllowedDuringTurn(validateClientCommand(command, '/workspace'), true),
      /cancel the active turn/,
    );
  }
  assert.doesNotThrow(() => assertCommandAllowedDuringTurn({ type: 'task_stop', task_id: 'task-1' }, true));
  assert.doesNotThrow(() => assertCommandAllowedDuringTurn({ type: 'list_models' }, false));
});

test('prompt and permission payloads are bounded and exact', () => {
  assert.equal(validatePrompt('hello'), 'hello');
  assert.throws(() => validatePrompt(''), /invalid prompt/);
  assert.throws(() => validatePrompt(' \n\t '), /invalid prompt/);
  assert.deepEqual(validatePermissionResponse(undefined), { type: 'allow_once' });
  assert.deepEqual(validatePermissionResponse({ type: 'deny' }), { type: 'deny' });
  assert.throws(() => validatePermissionResponse({ type: 'allow_once', extra: true }), /unsupported fields/);
});

test('computer access responses are bounded and exact', () => {
  assert.deepEqual(
    validateComputerAccessResponse({
      granted_apps: ['Slack'],
      clipboard_read: false,
      clipboard_write: false,
      system_key_combos: false,
    }),
    { granted_apps: ['Slack'], clipboard_read: false, clipboard_write: false, system_key_combos: false },
  );
  assert.deepEqual(
    validateComputerAccessResponse({
      granted_apps: [],
      clipboard_read: false,
      clipboard_write: false,
      system_key_combos: false,
    }),
    { granted_apps: [], clipboard_read: false, clipboard_write: false, system_key_combos: false },
  );
  assert.throws(
    () => validateComputerAccessResponse({ granted_apps: ['Slack'], clipboard_read: false, clipboard_write: false }),
    /invalid computer access response/,
  );
  assert.throws(
    () => validateComputerAccessResponse({
      granted_apps: ['Slack'],
      clipboard_read: false,
      clipboard_write: false,
      system_key_combos: false,
      extra: true,
    }),
    /unsupported fields/,
  );
  assert.throws(
    () => validateComputerAccessResponse({ granted_apps: 'Slack', clipboard_read: false, clipboard_write: false, system_key_combos: false }),
    /invalid computer access response/,
  );
  assert.throws(
    () => validateComputerAccessResponse({
      granted_apps: Array.from({ length: 65 }, (_, i) => `app-${i}`),
      clipboard_read: false,
      clipboard_write: false,
      system_key_combos: false,
    }),
    /invalid computer access response/,
  );
});

test('bridge discovery accepts only the spawned child identity and complete private body', () => {
  const body = {
    pid: 42,
    workspaceFolders: ['/workspace'],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef',
  };
  assert.doesNotThrow(() => validateBridgeLockfile(body, 42, '/workspace'));
  assert.throws(() => validateBridgeLockfile({ ...body, pid: 43 }, 42, '/workspace'), /pid mismatch/);
  assert.throws(() => validateBridgeLockfile({ ...body, workspaceFolders: ['/other'] }, 42, '/workspace'), /workspace mismatch/);
  assert.throws(() => validateBridgeLockfile({ ...body, ideName: 'LingXi' }, 42, '/workspace'), /identity mismatch/);
  assert.throws(() => validateBridgeLockfile({ ...body, authToken: 'short' }, 42, '/workspace'), /auth token/);
  const { pid: _pid, ...missingPid } = body;
  assert.throws(() => validateBridgeLockfile(missingPid, 42, '/workspace'), /missing pid/);
});

test('IPC sender validation requires an allowlisted webContents, top frame, and origin', () => {
  const allowedIds = new Set([7]);
  const allowedOrigins = new Set(['https://localhost:5173', 'file://']);
  assert.equal(isAllowedIpcSender({ senderId: 7, frameId: 2, topFrameId: 2, url: 'https://localhost:5173/app' }, allowedIds, allowedOrigins), true);
  assert.equal(isAllowedIpcSender({ senderId: 7, frameId: 3, topFrameId: 2, url: 'https://localhost:5173/app' }, allowedIds, allowedOrigins), false);
  assert.equal(isAllowedIpcSender({ senderId: 8, frameId: 2, topFrameId: 2, url: 'https://localhost:5173/app' }, allowedIds, allowedOrigins), false);
  assert.equal(isAllowedIpcSender({ senderId: 7, frameId: 2, topFrameId: 2, url: 'https://evil.test/' }, allowedIds, allowedOrigins), false);
});
