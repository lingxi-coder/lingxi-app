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
  assert.deepEqual(validateClientCommand({ type: 'list_session_agents' }), { type: 'list_session_agents' });
  assert.deepEqual(
    validateClientCommand({ type: 'load_session_agent_transcript', agent_id: 'agent:11111111-2222-4333-8444-555555555555' }),
    { type: 'load_session_agent_transcript', agent_id: 'agent:11111111-2222-4333-8444-555555555555' },
  );
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
  assert.throws(() => validateClientCommand({ type: 'list_session_agents', extra: true }), /unsupported fields/);
  assert.throws(() => validateClientCommand({ type: 'load_session_agent_transcript', agent_id: '../outside' }), /invalid agent id/);
  assert.throws(
    () => validateClientCommand({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }, { type: 'status' }] }),
    /invalid listing selection/,
  );
});

test('the settings, permission, workspace, and MCP commands pass the runtime allowlist', () => {
  assert.deepEqual(
    validateClientCommand({ type: 'update_settings', destination: 'user', patch_json: '{"outputStyle":"terse"}' }),
    { type: 'update_settings', destination: 'user', patch_json: '{"outputStyle":"terse"}' },
  );
  assert.deepEqual(
    validateClientCommand({
      type: 'update_permission_rules',
      destination: 'project',
      behavior: 'allow',
      add: ['Bash(git status)'],
      remove: [],
    }),
    {
      type: 'update_permission_rules',
      destination: 'project',
      behavior: 'allow',
      add: ['Bash(git status)'],
      remove: [],
    },
  );
  assert.deepEqual(
    validateClientCommand({ type: 'set_default_permission_mode', destination: 'user', mode: 'acceptEdits' }),
    { type: 'set_default_permission_mode', destination: 'user', mode: 'acceptEdits' },
  );
  assert.deepEqual(
    validateClientCommand({
      type: 'update_workspace_directories',
      destination: 'local',
      add: ['/workspace/extra'],
      remove: [],
    }),
    {
      type: 'update_workspace_directories',
      destination: 'local',
      add: ['/workspace/extra'],
      remove: [],
    },
  );
  assert.deepEqual(
    validateClientCommand({
      type: 'upsert_mcp_server',
      scope: 'user',
      name: 'filesystem',
      config_json: '{"command":"npx","args":["-y","mcp-fs"]}',
    }),
    {
      type: 'upsert_mcp_server',
      scope: 'user',
      name: 'filesystem',
      config_json: '{"command":"npx","args":["-y","mcp-fs"]}',
    },
  );
  assert.deepEqual(
    validateClientCommand({ type: 'remove_mcp_server', scope: 'user', name: 'filesystem' }),
    { type: 'remove_mcp_server', scope: 'user', name: 'filesystem' },
  );
  assert.deepEqual(
    validateClientCommand({ type: 'refresh_listings', which: [{ type: 'settings' }, { type: 'mcp' }, { type: 'skills' }] }),
    { type: 'refresh_listings', which: [{ type: 'settings' }, { type: 'mcp' }, { type: 'skills' }] },
  );
  assert.throws(
    () => validateClientCommand({ type: 'update_settings', destination: 'user', patch_json: 'not json' }),
    /invalid settings patch/,
  );
  assert.throws(
    () => validateClientCommand({ type: 'update_settings', destination: 'user', patch_json: '[1,2]' }),
    /invalid settings patch/,
  );
  assert.throws(
    () => validateClientCommand({ type: 'update_settings', destination: 'nope', patch_json: '{}' }),
    /invalid settings destination/,
  );
  assert.throws(
    () => validateClientCommand({ type: 'upsert_mcp_server', scope: 'user', name: '', config_json: '{}' }),
    /invalid mcp server name/,
  );
  assert.throws(
    () => validateClientCommand({
      type: 'update_permission_rules',
      destination: 'user',
      behavior: 'maybe',
      add: [],
      remove: [],
    }),
    /invalid permission behavior/,
  );
  assert.throws(
    () => validateClientCommand({ type: 'remove_mcp_server', scope: 'nowhere', name: 'x' }),
    /invalid mcp scope/,
  );
});

test('an unlisted command that genuinely exists in the protocol is still rejected', () => {
  // `login` is a real ClientCommand variant (auth is CLI/TUI-only on desktop);
  // it must never be reachable through the desktop's bounded IPC surface.
  assert.throws(() => validateClientCommand({ type: 'login' }), /command is not allowed/);
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

// ---------------------------------------------------------------------------
// `audio_response` — the renderer's answer to `ClientEvent::AudioRequest`.
//
// Every one of these assertions guards an engine call that is PARKED on a
// deadline (5s for `is_recording`, 30s for start/stop, 180s for
// transcribe/synthesize — `audio_bridge.rs`'s `STATE_QUERY_DEADLINE` /
// `DEVICE_CONTROL_DEADLINE` / `CAPTURE_DEADLINE`). A response this gate
// rejects is a response the engine never sees, so a wrong bound here is not
// a cosmetic validation bug: it is a stall of exactly that length.
// ---------------------------------------------------------------------------

test('the audio response command passes the runtime allowlist in every shape the renderer produces', () => {
  assert.deepEqual(
    validateClientCommand({ type: 'audio_response', request_id: 7, result: { type: 'ok' } }),
    { type: 'audio_response', request_id: 7, result: { type: 'ok' } },
  );
  assert.deepEqual(
    validateClientCommand({ type: 'audio_response', request_id: 0, result: { type: 'recording_state', recording: false } }),
    { type: 'audio_response', request_id: 0, result: { type: 'recording_state', recording: false } },
  );
  assert.deepEqual(
    validateClientCommand({
      type: 'audio_response',
      request_id: 3,
      result: { type: 'recording', audio_base64: 'AAEC', mime_type: 'audio/webm;codecs=opus' },
    }),
    {
      type: 'audio_response',
      request_id: 3,
      result: { type: 'recording', audio_base64: 'AAEC', mime_type: 'audio/webm;codecs=opus' },
    },
  );
  assert.deepEqual(
    validateClientCommand({
      type: 'audio_response',
      request_id: 4,
      result: { type: 'failed', kind: 'permission_denied', message: 'microphone permission denied: NotAllowedError' },
    }),
    {
      type: 'audio_response',
      request_id: 4,
      result: { type: 'failed', kind: 'permission_denied', message: 'microphone permission denied: NotAllowedError' },
    },
  );
});

test('the empty-PCM played-in-place synthesis answer survives the gate', () => {
  // `synthesis.ts` answers a successful `speechSynthesis` playback with
  // `{ pcm_base64: '', sample_rate_hz: 0 }` — "already played in place", a
  // genuine success pinned on the Rust side by
  // `synthesize_treats_empty_pcm_as_played_in_place_not_a_failure`. Every
  // other string this file validates is rejected when empty, so this is the
  // one place that rule must NOT apply: rejecting it here would make every
  // desktop TTS call wait out the 180s `CAPTURE_DEADLINE` and then fail.
  assert.deepEqual(
    validateClientCommand({ type: 'audio_response', request_id: 9, result: { type: 'audio', pcm_base64: '', sample_rate_hz: 0 } }),
    { type: 'audio_response', request_id: 9, result: { type: 'audio', pcm_base64: '', sample_rate_hz: 0 } },
  );
});

test('every AudioErrorKindDto the wire declares survives the gate', () => {
  // The kinds exist so `permission_denied` / `unavailable` / `not_recording`
  // stay distinguishable end to end (`audio_bridge.rs`'s `voice_error` /
  // `stt_error` / `tts_error` branch on each one). A kind this gate drops
  // would silently collapse to a stalled request, not to `other`.
  for (const kind of [
    'permission_denied', 'no_speech', 'not_recording', 'unavailable',
    'busy', 'retriable', 'synthesis_failed', 'other',
  ]) {
    assert.deepEqual(
      validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'failed', kind, message: 'why' } }),
      { type: 'audio_response', request_id: 1, result: { type: 'failed', kind, message: 'why' } },
    );
  }
  assert.throws(
    () => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'failed', kind: 'invented', message: 'why' } }),
    /invalid audio error kind/,
  );
});

test('a transcript cannot be sent from this renderer at all', () => {
  // Desktop has no speech recognizer and cannot reach a provider
  // transcription API (the renderer holds no credential — `host.ts` forwards
  // it straight to the engine and keeps nothing). `Transcribe` is answered
  // `failed`/`unavailable`, never with text. Keeping `transcript` OFF this
  // gate means a fabricated transcript cannot leave the renderer even if
  // some future code tried to send one — the same bounded-surface discipline
  // that removed `new_session`/`resume_session` from the allowlist.
  assert.throws(
    () => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'transcript', text: 'invented words' } }),
    /invalid audio result/,
  );
});

test('malformed audio responses are rejected rather than forwarded', () => {
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: -1, result: { type: 'ok' } }), /invalid audio request id/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1.5, result: { type: 'ok' } }), /invalid audio request id/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', result: { type: 'ok' } }), /invalid audio request id/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'ok' }, extra: 1 }), /unsupported fields/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'ok', recording: true } }), /unsupported fields/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'recording_state', recording: 'yes' } }), /invalid audio recording state/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'recording', audio_base64: 'not base64!', mime_type: 'audio/webm' } }), /invalid audio base64/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'recording', audio_base64: 'AAEC', mime_type: '' } }), /invalid audio mime type/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'audio', pcm_base64: '', sample_rate_hz: -1 } }), /invalid audio sample rate/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: { type: 'failed', kind: 'other', message: '' } }), /invalid audio error message/);
  assert.throws(() => validateClientCommand({ type: 'audio_response', request_id: 1, result: 'ok' }), /invalid payload/);
});

test('an audio response is never blocked by an active turn', () => {
  // Audio requests are emitted BY a running turn (the `speech` tool asks the
  // client for its microphone mid-turn). If `audio_response` ever joined
  // `assertCommandAllowedDuringTurn`'s blocked set, every audio request in
  // the product would park until its deadline expired and then fail.
  assert.doesNotThrow(
    () => assertCommandAllowedDuringTurn({ type: 'audio_response', request_id: 1, result: { type: 'ok' } }, true),
  );
});
