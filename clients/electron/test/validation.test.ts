import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  assertCommandAllowedDuringTurn,
  isAllowedIpcSender,
  MAX_CLIPBOARD_TEXT_CHARS,
  validateAskUserQuestionAnswers,
  validateBridgeLockfile,
  validateClientCommand,
  validateClipboardText,
  validateComputerAccessResponse,
  validateImageRefs,
  validatePermissionResponse,
  validatePrompt,
} from '../src/main/validation';

test('clipboard writes accept only bounded plain text', () => {
  assert.equal(validateClipboardText(''), '');
  assert.equal(validateClipboardText('x'.repeat(MAX_CLIPBOARD_TEXT_CHARS)).length, MAX_CLIPBOARD_TEXT_CHARS);
  assert.throws(() => validateClipboardText('x'.repeat(MAX_CLIPBOARD_TEXT_CHARS + 1)), /invalid clipboard text/);
  assert.throws(() => validateClipboardText({ text: 'nope' }), /invalid clipboard text/);
});

test('only the bounded Desktop command surface passes the runtime allowlist', () => {
  assert.deepEqual(validateClientCommand({ type: 'list_models' }), { type: 'list_models' });
  assert.deepEqual(validateClientCommand({ type: 'list_sessions', limit: 25 }), { type: 'list_sessions', limit: 25 });
  assert.deepEqual(validateClientCommand({ type: 'task_output', task_id: 'task-1', offset: 0 }), { type: 'task_output', task_id: 'task-1', offset: 0 });
  assert.deepEqual(validateClientCommand({ type: 'task_list', status_filter: { type: 'paused' } }), { type: 'task_list', status_filter: { type: 'paused' } });
  assert.deepEqual(validateClientCommand({ type: 'set_permission_mode', mode: 'auto' }), { type: 'set_permission_mode', mode: 'auto' });
  assert.deepEqual(validateClientCommand({ type: 'get_conversation_controls' }), { type: 'get_conversation_controls' });
  assert.deepEqual(validateClientCommand({ type: 'set_reasoning_selection', selection: { type: 'level', id: 'high' } }), { type: 'set_reasoning_selection', selection: { type: 'level', id: 'high' } });
  assert.deepEqual(validateClientCommand({ type: 'set_fast_mode', enabled: true }), { type: 'set_fast_mode', enabled: true });
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
  assert.throws(() => validateClientCommand({ type: 'set_fast_mode', enabled: 'true' }), /invalid fast mode enabled flag/);
  assert.throws(() => validateClientCommand({ type: 'set_reasoning_selection', selection: { type: 'level', id: '' } }), /invalid reasoning level/);
  assert.throws(() => validateClientCommand({ type: 'set_reasoning_selection', selection: { type: 'token_budget', tokens: -1 } }), /invalid reasoning token budget/);
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

test('session lifecycle commands cannot cross the renderer command boundary', () => {
  assert.throws(() => validateClientCommand({ type: 'new_session' }, '/workspace'), /command is not allowed/);
  assert.throws(
    () => validateClientCommand({ type: 'resume_session', session_id: '11111111-2222-4333-8444-555555555555' }, '/workspace'),
    /command is not allowed/,
  );
});

test('active turns reject model and session mutation but retain recovery commands', () => {
  for (const command of [
    { type: 'set_model', model: 'claude-sonnet' },
    { type: 'set_permission_mode', mode: 'acceptEdits' },
    { type: 'set_reasoning_selection', selection: { type: 'level', id: 'high' } },
    { type: 'set_fast_mode', enabled: true },
    { type: 'run_slash_command', raw: '/clear' },
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

test('image prompt payloads require canonical, bounded image data', () => {
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]);
  const image = { media_type: 'image/png', base64: png.toString('base64') };
  assert.deepEqual(validateImageRefs([image]), [image]);
  assert.deepEqual(validateImageRefs(undefined), []);
  assert.throws(() => validateImageRefs([{ ...image, media_type: 'image/svg+xml' }]), /invalid image media type/);
  assert.throws(() => validateImageRefs([{ ...image, base64: `data:image/png;base64,${image.base64}` }]), /invalid image base64/);
  assert.throws(() => validateImageRefs([{ ...image, base64: `${image.base64.slice(0, -2)}xx` }]), /invalid image format|invalid image base64/);
  assert.throws(() => validateImageRefs([{ ...image, extra: true }]), /unsupported fields/);
  assert.throws(() => validateImageRefs(Array.from({ length: 5 }, () => image)), /invalid image attachments/);
  assert.throws(() => validatePrompt(''), /invalid prompt/);
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
