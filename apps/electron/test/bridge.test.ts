import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { BridgeClient } from '@lingxi/bridge-client';
import type { ClientEvent } from '@lingxi/bridge-client';

import { SessionRuntime } from '../src/main/bridge.js';
import {
  discoverExternalReusableBridge,
  discoverReusableBridge,
  processCommandOwnsSession,
} from '../src/main/bridgeDiscovery.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { CH_EVENT, CH_MOD_UI_FRAME, CH_MOD_UI_INVALIDATE } from '../src/main/bridgeIpc.js';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';

function audioContextEventClient(): EventEmitter & { sendCommand(command: unknown): void } {
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
  client.sendCommand = assertAudioContextCommand;
  return client;
}

function assertAudioContextCommand(value: unknown): void {
  const command = value as { type: string; request_id: string };
  assert.deepEqual(Object.keys(command).sort(), ['request_id', 'type']);
  assert.equal(command.type, 'get_audio_session_context');
  assert.match(command.request_id, /^[0-9a-f-]{36}$/);
}

function temporaryDirectory(): string {
  return mkdtempSync(join(tmpdir(), 'lingxi-electron-bridge-test-'));
}

function deferred<T = void>(): { promise: Promise<T>; resolve(value?: T): void; reject(error: unknown): void } {
  let resolvePromise!: (value: T) => void;
  let rejectPromise!: (error: unknown) => void;
  const promise = new Promise<T>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: (value?: T) => resolvePromise(value as T), reject: rejectPromise };
}

function fakeWebContents(sent: Array<{ channel: string; payload: unknown }> = []): EventEmitter & {
  isDestroyed(): boolean;
  send(channel: string, payload: unknown): void;
} {
  const webContents = new EventEmitter() as EventEmitter & {
    isDestroyed(): boolean;
    send(channel: string, payload: unknown): void;
  };
  webContents.isDestroyed = () => false;
  webContents.send = (channel, payload) => sent.push({ channel, payload });
  return webContents;
}

test('SessionRuntimeManager owns one destroyed listener for all session runtimes', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents();
  const first = { projectPath: '/workspace', sessionId: '11111111-2222-4333-8444-555555555555' };
  const second = { projectPath: '/workspace', sessionId: '22222222-3333-4444-8555-666666666666' };

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  await manager.ensure(first, false);
  await manager.ensure(second, false);

  assert.equal(webContents.listenerCount('destroyed'), 1);
  webContents.emit('destroyed');
  assert.equal(webContents.listenerCount('destroyed'), 0);
  assert.equal((manager.get(first.sessionId) as any).targets.size, 0);
  assert.equal((manager.get(second.sessionId) as any).targets.size, 0);

  await manager.dispose();
});

test('disposing SessionRuntimeManager removes its centralized destroyed listener', async () => {
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents();

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  assert.equal(webContents.listenerCount('destroyed'), 1);
  await manager.dispose();
  assert.equal(webContents.listenerCount('destroyed'), 0);
});

test('disposing a session runtime sends an explicit removal state to the renderer', async () => {
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  const webContents = fakeWebContents(sent);
  const ref = { projectPath: '/workspace', sessionId: '33333333-4444-4555-8666-777777777777' };

  manager.registerWindow(webContents as any, 'app://desktop/index.html');
  await manager.ensure(ref, false);
  await manager.closeSession(ref);

  assert.deepEqual(sent.at(-1), {
    channel: 'lingxi:connectionStateChanged',
    payload: {
      sessionId: ref.sessionId,
      event: { status: 'disconnected', reason: 'session runtime disposed' },
    },
  });

  await manager.dispose();
});

test('newSession keeps the generated runtime/session id and does not send a second new_session command', async () => {
  const commands: unknown[] = [];
  let launchRef: { projectPath: string; sessionId: string } | undefined;
  const originalStart = SessionRuntime.prototype.start;
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => {
      launchRef = ref;
      return { workspace: '/workspace', sessionId: ref.sessionId, trusted: true };
    },
  });
  SessionRuntime.prototype.start = async function () {
    const launch = await (this as any).opts.launchConfig();
    assert.equal(launch.sessionId, this.sessionId);
  };
  try {
    const created = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' });
    assert.deepEqual(created, launchRef);
    assert.equal(commands.length, 0);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('newSession reuses one unsent draft per project until the first prompt crosses the bridge', async () => {
  const projectPath = '/workspace-draft';
  const commits: string[] = [];
  let prompts = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
    (this as any).draftModeRestorationRequired = false; // This fixture substitutes a ready engine.
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onFirstPromptSent: (ref) => { commits.push(ref.sessionId); },
  });
  try {
    const first = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    const second = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    assert.equal(second.sessionId, first.sessionId);

    const runtime = manager.require(first) as SessionRuntime & { client: { sendPrompt: () => void } | null };
    (runtime as any).client = { sendPrompt: () => { prompts += 1; } };
    runtime.sendPrompt('hello');
    runtime.sendPrompt('hello again');

    assert.equal(prompts, 2);
    assert.deepEqual(commits, [first.sessionId]);

    const third = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    const fourth = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    assert.notEqual(third.sessionId, first.sessionId);
    assert.equal(fourth.sessionId, third.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('newSession deduplicates concurrent draft creation and closeSession clears the draft slot', async () => {
  const projectPath = '/workspace-race';
  const gate = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  let starts = 0;
  SessionRuntime.prototype.start = async function () {
    starts += 1;
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
    await gate.promise;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const first = manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    const second = manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    gate.resolve();
    const [created, duplicate] = await Promise.all([first, second]);
    assert.equal(starts, 1);
    assert.equal(duplicate.sessionId, created.sessionId);

    await manager.closeSession(created);
    const next = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    assert.notEqual(next.sessionId, created.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('first-prompt commit failures keep the draft recoverable and retry without making sendPrompt fail', async () => {
  const projectPath = '/workspace-commit-error';
  const diagnostics = new DiagnosticBuffer();
  let commitAttempts = 0;
  const originalStart = SessionRuntime.prototype.start;
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = projectPath;
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
    (this as any).draftModeRestorationRequired = false; // This fixture substitutes a ready engine.
  };
  const manager = new SessionRuntimeManager({
    diagnostics,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    onFirstPromptSent: () => {
      commitAttempts += 1;
      if (commitAttempts === 1) throw new Error('settings disk is read-only');
    },
  });
  try {
    const first = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    const runtime = manager.require(first) as SessionRuntime & { client: { sendPrompt: () => void } | null };
    (runtime as any).client = { sendPrompt: () => undefined };

    assert.doesNotThrow(() => runtime.sendPrompt('hello'));
    assert.match(diagnostics.snapshot().at(-1)?.message ?? '', /failed to commit draft session/);

    const recoverable = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    assert.equal(recoverable.sessionId, first.sessionId);

    assert.doesNotThrow(() => runtime.sendPrompt('retry commit'));
    assert.equal(commitAttempts, 2);
    const next = await manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath });
    assert.notEqual(next.sessionId, first.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('session runtime manager launches a restored session with its catalog model hint', async () => {
  const ref = {
    projectPath: '/workspace',
    mode: 'code' as const,
    sessionId: '11111111-2222-4333-8444-555555555556',
  };
  const launchHints: Array<string | undefined> = [];
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    await (this as any).opts.launchConfig();
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    launchConfig: (_candidate, modelHint) => {
      launchHints.push(modelHint);
      return { workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true, model: modelHint };
    },
  });
  try {
    await manager.openSession(ref, false, 'openrouter/cohere/north-mini-code:free');
    assert.deepEqual(launchHints, ['openrouter/cohere/north-mini-code:free']);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('session runtime replay resets at resume and reconstructs later transcript events', () => {
  const sessionId = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  const client = audioContextEventClient();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).generation = 1;
  (runtime as any).wireClient(client, 1);

  client.emit('event', { type: 'session_started', session_id: sessionId });
  client.emit('event', { type: 'system_notice', level: 'info', message: 'old base' });
  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', { type: 'turn_started', turn_id: 7 });
  client.emit('event', { type: 'text_delta', text: 'hello' });

  const first = runtime.replaySnapshot();
  assert.deepEqual(first.map((entry) => entry.event.type), ['session_resumed', 'turn_started', 'text_delta']);
  assert.deepEqual(first.map((entry) => entry.sequence), [3, 4, 5]);
  (first[2]!.event as { text: string }).text = 'mutated';
  assert.equal((runtime.replaySnapshot()[2]!.event as { text: string }).text, 'hello');
});

test('SideChat events reach renderer without entering the parent transcript replay', () => {
  const sessionId = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
  client.sendCommand = () => { throw new Error('unexpected command'); };
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const webContents = fakeWebContents(sent);
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    envelopeEvents: true,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).generation = 1;
  runtime.registerWindow(webContents as any, 'app://desktop/index.html');
  (runtime as any).wireClient(client, 1);

  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', { type: 'text_delta', text: 'public transcript event' });
  const deliveredBeforeSideChat = sent.filter(({ channel }) => channel === CH_EVENT).length;

  const sideChatEvents: ClientEvent[] = [
    { type: 'side_chat_created', request_id: 'create-1', side_id: 'side-1' },
    { type: 'side_chat_list', side_ids: ['side-1'] },
    { type: 'side_chat_snapshot', side_id: 'side-1', events_json: ['{"type":"text_delta","text":"private child text"}'] },
    { type: 'side_chat_event', side_id: 'side-1', event_json: '{"type":"text_delta","text":"private child text"}' },
    {
      type: 'side_chat_permission',
      side_id: 'side-1',
      request: {
        request_id: 7,
        kind: { type: 'tool_use_confirm', tool_name: 'Read', tool_input_json: '{}', default_allow: true },
      },
    },
    { type: 'side_chat_submitted', side_id: 'side-1', request_id: 'submit-1' },
    { type: 'side_chat_ended', side_id: 'side-1' },
    { type: 'side_chat_error', request_id: 'submit-1', side_id: 'side-1', message: 'private child failure' },
  ];
  for (const event of sideChatEvents) client.emit('event', event);

  const beforeParentContinuation = sent
    .filter(({ channel }) => channel === CH_EVENT)
    .map(({ payload }) => payload as { sequence: number; event: ClientEvent });
  assert.equal(beforeParentContinuation.length, deliveredBeforeSideChat + sideChatEvents.length);
  assert.deepEqual(
    beforeParentContinuation.slice(deliveredBeforeSideChat).map(({ event }) => event),
    sideChatEvents,
    'the renderer SideChat controller receives child snapshots, events, and permission requests',
  );
  assert.deepEqual(
    runtime.replaySnapshot().map(({ event }) => event.type),
    ['session_resumed', 'text_delta'],
    'the SideChat controller receives live events; parent transcript replay remains session-only',
  );
  assert.equal(runtime.turnActive, false, 'child lifecycle events do not start or end the parent turn');

  client.emit('event', { type: 'text_delta', text: 'public event after SideChat' });
  const envelopes = sent
    .filter(({ channel }) => channel === CH_EVENT)
    .map(({ payload }) => payload as { sequence: number; event: { type: string } });
  assert.deepEqual(envelopes.map(({ event }) => event.type), [
    'session_resumed', 'text_delta', ...sideChatEvents.map((event) => event.type), 'text_delta',
  ], 'parent transcript and SideChat controller events share the renderer bridge');
  assert.deepEqual(envelopes.map(({ sequence }) => sequence),
    Array.from({ length: envelopes.length }, (_, index) => index + 1));
  assert.deepEqual(runtime.replaySnapshot().map(({ sequence }) => sequence),
    [1, 2, envelopes.at(-1)!.sequence],
    'SideChat event sequences do not enter the parent transcript replay buffer');

  const parent = runtime.replaySnapshot().reduce(
    (state, { event }) => reduceEvent(state, event),
    emptyConversation(),
  );
  assert.equal(
    parent.items.find((item) => item.type === 'narration' && item.role === 'assistant')?.text,
    'public transcript eventpublic event after SideChat',
    'opaque child transcript payloads and permission requests never alter the parent conversation',
  );
});

test('resumed usage and cumulative status reach the renderer and survive replay without an active turn', () => {
  const sessionId = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  const client = audioContextEventClient();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  const delivered: Array<{ channel: string; payload: any }> = [];
  (runtime as any).broadcast = (channel: string, payload: unknown) => delivered.push({ channel, payload });
  (runtime as any).wireClient(client, 0);
  const usage = { type: 'usage_update', is_snapshot: true, input_tokens: 9000, output_tokens: 1000, cache_read_tokens: 65000, cache_creation_tokens: 4000 };
  const status = { type: 'status_snapshot', snapshot: { session_id: sessionId, input_tokens: 120000, output_tokens: 10000, total_cost_usd: 0.5 } };

  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', usage);
  client.emit('event', status);
  client.emit('event', { type: 'model_changed', model: 'claude-opus-4-6' });
  assert.equal(runtime.turnActive, false);
  assert.ok(delivered.some(({ payload }) => payload.type === 'usage_update'));
  assert.ok(delivered.some(({ payload }) => payload.type === 'status_snapshot'));
  assert.deepEqual(runtime.replaySnapshot().map(({ event }) => event), [
    { type: 'session_resumed', session_id: sessionId, messages: [] }, usage, status,
  ]);

  client.emit('event', { ...status, snapshot: { ...status.snapshot, input_tokens: 130000 } });
  assert.equal(
    runtime.replaySnapshot().filter(({ event }) => event.type === 'status_snapshot').length,
    1,
    'status snapshots are coalesced instead of accumulating in replay',
  );
  assert.equal(
    (runtime.replaySnapshot().at(-1)?.event as { snapshot: { input_tokens: number } }).snapshot.input_tokens,
    130000,
  );

  client.emit('event', { ...usage, is_snapshot: undefined, input_tokens: 999999 });
  assert.equal(runtime.replaySnapshot().filter(({ event }) => event.type === 'usage_update').length, 1,
    'late unowned usage cannot overwrite restored context');

  client.emit('event', { type: 'compaction_status', phase: 'complete' });
  const reloaded = runtime.replaySnapshot().reduce((state, { event }) => reduceEvent(state, event), emptyConversation());
  assert.equal(reloaded.usage, null, 'renderer reload replays compaction invalidation after recovered usage');
  assert.ok(runtime.replaySnapshot().some(({ event }) => event.type === 'compaction_status'));
});

test('Mod pinned statuses replay only the latest value per plugin', () => {
  const sessionId = '12121212-3434-4567-8899-bbbbbbbbbbbb';
  const client = audioContextEventClient();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).broadcast = () => {};
  (runtime as any).wireClient(client, 0);
  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', { type: 'ui_status', plugin: 'one', text: 'First' });
  client.emit('event', { type: 'ui_status', plugin: 'two', text: 'Second' });
  client.emit('event', { type: 'ui_status', plugin: 'one', text: null });
  const statuses = runtime.replaySnapshot().filter(({ event }) => event.type === 'ui_status');
  assert.deepEqual(statuses.map(({ event }) => event), [
    { type: 'ui_status', plugin: 'two', text: 'Second' },
    { type: 'ui_status', plugin: 'one', text: null },
  ]);
  const restored = runtime.replaySnapshot().reduce((state, { event }) => reduceEvent(state, event), emptyConversation());
  assert.deepEqual(restored.modStatuses, [{ plugin: 'two', text: 'Second' }]);
});

test('server fallback block and tombstone events are turn-owned and replay in source order', () => {
  const sessionId = '12121212-3434-4567-8899-cccccccccccc';
  const client = audioContextEventClient();
  const runtime = new SessionRuntime({
    sessionId,
    projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
  });
  (runtime as any).broadcast = () => {};
  (runtime as any).wireClient(client, 0);
  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', { type: 'turn_started', turn_id: 3 });
  client.emit('event', { type: 'query_model_change', to_model: 'claude-sonnet-4' });
  client.emit('event', { type: 'assistant_block_start', block_key: 91 });
  client.emit('event', { type: 'text_delta', text: 'discarded' });
  client.emit('event', { type: 'assistant_block_identity', block_key: 91, message_uuid: 'old-row' });
  client.emit('event', {
    type: 'tombstone', display_only: true,
    message: {
      uuid: 'old-row', type: 'assistant', timestamp: '2026-10-03T12:00:00.000Z',
      message: { content_json: '[{"type":"text","text":"discarded"}]' },
    },
  });
  client.emit('event', {
    type: 'refusal_continuation', phase: 'begin', salvage_text: 'retained', join: 'exact',
    replaces_uuids: [], display_salvage_text: true,
  });
  client.emit('event', { type: 'assistant_block_start', block_key: 92 });
  client.emit('event', { type: 'text_delta', text: ' answer' });
  client.emit('event', { type: 'assistant_block_identity', block_key: 92, message_uuid: 'new-row' });

  const events = runtime.replaySnapshot().map(({ event }) => event);
  assert.deepEqual(events.slice(2).map((event) => event.type), [
    'query_model_change', 'assistant_block_start', 'text_delta', 'assistant_block_identity',
    'tombstone', 'refusal_continuation', 'assistant_block_start', 'text_delta', 'assistant_block_identity',
  ]);
  const restored = events.reduce((state, event) => reduceEvent(state, event), emptyConversation());
  const narration = restored.items.filter((item) => item.type === 'narration');
  assert.equal(narration.length, 1);
  assert.equal(narration[0]?.text, 'retained answer');
  assert.equal(narration[0]?.transcriptUuid, 'new-row');
  assert.equal(narration[0]?.servedModel, 'claude-sonnet-4');
});
test('restored snapshots survive interleaved events but late live usage is rejected', () => {
  const sessionId = '12121212-3434-4567-8899-aaaaaaaaaaaa';
  const client = audioContextEventClient();
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  (runtime as any).wireClient(client, 0);
  const usage = { type: 'usage_update', input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 };
  client.emit('event', { type: 'session_resumed', session_id: sessionId, messages: [] });
  client.emit('event', usage);
  client.emit('event', { type: 'model_changed', model: 'claude-opus-4-6' });
  client.emit('event', { ...usage, is_snapshot: true });
  client.emit('event', { ...usage, is_snapshot: undefined, input_tokens: 999999 });
  const events = runtime.replaySnapshot().map(({ event }) => event);
  assert.deepEqual(events.filter((event) => event.type === 'usage_update'), [{ ...usage, is_snapshot: true }]);
  const recovered = events.reduce((state, event) => reduceEvent(state, event), emptyConversation());
  assert.deepEqual(recovered.usage, { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 });
  (runtime as any).generation = 1;
  client.emit('event', { ...usage, is_snapshot: true, input_tokens: 999999 });
  assert.equal(runtime.replaySnapshot().filter(({ event }) => event.type === 'usage_update').length, 1,
    'obsolete connections cannot send snapshots');
});

test('failed historical resume removes only the newly-created runtime', async () => {
  const sessionId = '22222222-3333-4444-8555-666666666666';
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {
    throw new Error('transcript is corrupt');
  };
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    await assert.rejects(manager.openSession({ projectPath: '/workspace', sessionId, mode: 'code' }), /transcript is corrupt/);
    assert.equal(manager.get(sessionId), undefined);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('opening another session leaves the running session alive', async () => {
  const runningRef = { projectPath: '/workspace-a', mode: 'code' as const, sessionId: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee' };
  const nextRef = { projectPath: '/workspace-b', mode: 'code' as const, sessionId: 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff' };
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const running = await manager.ensure(runningRef, true);
    (running as any).activeTurn = true;

    const opened = await manager.openSession(nextRef);

    assert.equal(manager.size, 2);
    assert.equal(manager.get(runningRef.sessionId), running);
    assert.equal(running.turnActive, true);
    assert.equal(manager.get(nextRef.sessionId), opened);
    assert.equal(opened.connectionState.status, 'connected');
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('reusable bridge discovery matches the exact session and workspace', () => {
  const bridgeRoot = temporaryDirectory();
  const launchDir = join(bridgeRoot, 'launch-existing');
  const workspace = '/workspace';
  const sessionId = '99999999-aaaa-4bbb-8ccc-dddddddddddd';
  mkdirSync(launchDir, { mode: 0o700 });
  const lockfilePath = join(launchDir, '43123.lock');
  writeFileSync(lockfilePath, JSON.stringify({
    pid: process.pid,
    workspaceFolders: [workspace],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef',
  }), { mode: 0o600 });

  try {
    const command = `/Applications/LingXi Code.app/Contents/Resources/bin/bridge-server --cwd ${workspace} --session-id ${sessionId} --trusted-workspace`;
    assert.deepEqual(
      discoverReusableBridge(
        bridgeRoot,
        { projectPath: workspace, sessionId },
        () => command,
      ),
      { launchDir, lockfilePath, pid: process.pid, ownedProcess: true },
    );
    assert.equal(discoverReusableBridge(
      bridgeRoot,
      { projectPath: '/different', sessionId },
      () => command,
    ), undefined);
    assert.equal(discoverReusableBridge(
      bridgeRoot,
      { projectPath: workspace, sessionId: '88888888-aaaa-4bbb-8ccc-dddddddddddd' },
      () => command,
    ), undefined);
  } finally {
    rmSync(bridgeRoot, { recursive: true, force: true });
  }
});

test('reusable bridge discovery never inspects the command of a dead lockfile pid', () => {
  const bridgeRoot = temporaryDirectory();
  const launchDir = join(bridgeRoot, 'launch-crashed');
  const workspace = '/workspace';
  const sessionId = '99999999-aaaa-4bbb-8ccc-dddddddddddd';
  mkdirSync(launchDir, { mode: 0o700 });
  writeFileSync(join(launchDir, '43126.lock'), JSON.stringify({
    pid: process.pid,
    workspaceFolders: [workspace],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef',
  }), { mode: 0o600 });
  try {
    let inspected = 0;
    const command = `/bridge-server --cwd ${workspace} --session-id ${sessionId}`;
    assert.equal(discoverReusableBridge(
      bridgeRoot,
      { projectPath: workspace, sessionId },
      () => { inspected += 1; return command; },
      () => false,
    ), undefined);
    assert.equal(inspected, 0);
  } finally {
    rmSync(bridgeRoot, { recursive: true, force: true });
  }
});

test('external bridge discovery accepts only the exact process workspace and session', () => {
  const launchDir = temporaryDirectory();
  const ref = {
    projectPath: '/workspace with spaces',
    sessionId: '66666666-aaaa-4bbb-8ccc-dddddddddddd',
  };
  const lockfilePath = join(launchDir, '43125.lock');
  writeFileSync(lockfilePath, JSON.stringify({
    pid: process.pid,
    workspaceFolders: [ref.projectPath],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef',
  }), { mode: 0o600 });

  try {
    const command = `/bridge-server --cwd ${ref.projectPath} --bridge-dir ${launchDir} --session-id ${ref.sessionId} --trusted-workspace`;
    assert.deepEqual(
      discoverExternalReusableBridge(ref, [{ pid: process.pid, ppid: 42, command }]),
      { launchDir, lockfilePath, pid: process.pid, ownedProcess: false },
    );
    assert.equal(discoverExternalReusableBridge(
      { ...ref, projectPath: '/different' },
      [{ pid: process.pid, ppid: 42, command }],
    ), undefined);
    assert.deepEqual(
      discoverExternalReusableBridge(ref, [{ pid: process.pid, ppid: 1, command }]),
      { launchDir, lockfilePath, pid: process.pid, ownedProcess: true },
      'an orphaned bridge is safe for the current Desktop to replace if adoption is incompatible',
    );
  } finally {
    rmSync(launchDir, { recursive: true, force: true });
  }
});

test('process command matching does not accept a session id prefix or another flag value', () => {
  const sessionId = '99999999-aaaa-4bbb-8ccc-dddddddddddd';
  assert.equal(processCommandOwnsSession(
    `/bridge-server --session-id ${sessionId} --trusted-workspace`,
    sessionId,
  ), true);
  assert.equal(processCommandOwnsSession(
    `/bridge-server --session-id ${sessionId}0 --trusted-workspace`,
    sessionId,
  ), false);
  assert.equal(processCommandOwnsSession(
    `/bridge-server --label ${sessionId} --trusted-workspace`,
    sessionId,
  ), false);
});

test('opening a session adopts its detached bridge instead of spawning a duplicate UUID', async () => {
  const bridgeRoot = temporaryDirectory();
  const launchDir = temporaryDirectory();
  const ref = {
    projectPath: '/workspace',
    mode: 'code' as const,
    sessionId: '77777777-aaaa-4bbb-8ccc-dddddddddddd',
  };
  writeFileSync(join(launchDir, '43124.lock'), JSON.stringify({
    pid: process.pid,
    workspaceFolders: [ref.projectPath],
    ideName: 'LingXi-Bridge',
    transport: 'ws',
    runningInWindows: false,
    authToken: '0123456789abcdef0123456789abcdef',
  }), { mode: 0o600 });
  const originalConnect = BridgeClient.prototype.connect;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  const startupCommands: unknown[] = [];
  let launchCalls = 0;
  let telemetryRevocations = 0;
  BridgeClient.prototype.connect = async function () {
    this.sendCommand = (command) => { startupCommands.push(command); };
    this.disableTelemetry = async () => { telemetryRevocations += 1; };
    return {
      server_name: 'lingxi-bridge-server/0.9.0',
      protocol_version: '0.2.0',
      capabilities: { client_protocol_version: '11.0.0' },
    } as any;
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    bridgeRoot,
    listProcessCommands: () => [{
      pid: process.pid,
      ppid: 42,
      command: `/bridge-server --cwd ${ref.projectPath} --bridge-dir ${launchDir} --session-id ${ref.sessionId}`,
    }],
    launchConfig: () => {
      launchCalls += 1;
      throw new Error('adoption must not assemble another process');
    },
  });

  try {
    const runtime = await manager.openSession(ref);
    assert.equal(startupCommands.length, 2);
    assertAudioContextCommand(startupCommands[0]);
    assert.deepEqual(startupCommands[1], { type: 'get_context_manifest' });
    assert.equal(launchCalls, 0);
    assert.equal(telemetryRevocations, 1, 'a newly adopted process honors default-off device policy');
    assert.equal(runtime.connectionState.status, 'connected');
    assert.strictEqual(manager.get(ref.sessionId), runtime);
    (runtime as any).stopAdoptedBridge = async () => {
      throw new Error('an externally owned bridge must not be terminated');
    };
  } finally {
    BridgeClient.prototype.connect = originalConnect;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
    assert.equal(existsSync(launchDir), true, 'external launch state remains owned by its original host');
    rmSync(bridgeRoot, { recursive: true, force: true });
    rmSync(launchDir, { recursive: true, force: true });
  }
});

test('idle session runtimes use a bounded least-recently-used cache', async () => {
  const refs = [
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '10000000-0000-4000-8000-000000000001' },
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '10000000-0000-4000-8000-000000000002' },
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '10000000-0000-4000-8000-000000000003' },
  ];
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { (this as any).state = { status: 'connected' }; };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    maxCachedRuntimes: 2,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    await manager.openSession(refs[0]!);
    await manager.openSession(refs[1]!);
    await manager.openSession(refs[2]!);

    assert.equal(manager.size, 2);
    assert.equal(manager.get(refs[0]!.sessionId), undefined);
    assert.ok(manager.get(refs[1]!.sessionId));
    assert.ok(manager.get(refs[2]!.sessionId));
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('background-running and active sessions are pinned above the idle cache limit', async () => {
  const refs = [
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '20000000-0000-4000-8000-000000000001' },
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '20000000-0000-4000-8000-000000000002' },
    { projectPath: '/workspace', mode: 'code' as const, sessionId: '20000000-0000-4000-8000-000000000003' },
  ];
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { (this as any).state = { status: 'connected' }; };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    maxCachedRuntimes: 2,
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const background = await manager.openSession(refs[0]!);
    (background as any).activeTurn = true;
    await manager.openSession(refs[1]!);
    await manager.openSession(refs[2]!);

    assert.equal(manager.size, 2);
    assert.strictEqual(manager.get(refs[0]!.sessionId), background);
    assert.equal(manager.get(refs[1]!.sessionId), undefined);
    assert.ok(manager.get(refs[2]!.sessionId));

    const secondBackground = manager.get(refs[2]!.sessionId)!;
    (secondBackground as any).activeTurn = true;
    await manager.openSession(refs[1]!);
    assert.equal(manager.size, 3, 'protected background sessions may temporarily exceed the idle cache limit');

    (background as any).activeTurn = false;
    (background as any).notifyActivityChanged();
    await Promise.resolve();
    assert.equal(manager.size, 2, 'the cache trims again as soon as a background turn becomes idle');
    assert.strictEqual(manager.get(refs[2]!.sessionId), secondBackground, 'a still-running background session stays cached');
    assert.ok(manager.get(refs[1]!.sessionId), 'the active session stays cached');
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('openSession deduplicates concurrent opens by UUID and rejects a different project', async () => {
  const ref = { mode: 'code' as const, projectPath: '/workspace-a', sessionId: 'cccccccc-dddd-4eee-8fff-000000000000' };
  const resumeModel = 'openrouter/cohere/north-mini-code:free';
  const gate = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  let starts = 0;
  let resumes = 0;
  SessionRuntime.prototype.start = async function () {
    starts += 1;
    (this as any).state = { status: 'connected' };
    await gate.promise;
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () {
    resumes += 1;
  };
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }),
    launchConfig: (candidate) => ({ workspace: candidate.projectPath, sessionId: candidate.sessionId, trusted: true }),
  });
  try {
    const first = manager.openSession(ref, false, resumeModel);
    const second = manager.openSession(ref, false, 'deepseek/deepseek-flash');
    assert.strictEqual(first, second);
    assert.throws(
      () => manager.openSession(
        { ...ref, projectPath: '/workspace-b' },
        false,
        'anthropic/claude-sonnet-5',
      ),
      /owned by a different project/,
    );
    assert.equal(
      (manager as any).sessionModelHints.get(ref.sessionId),
      resumeModel,
      'deduplicated or rejected opens cannot replace the model selected by the owning open',
    );
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(starts, 1);
    assert.equal(resumes, 0);
    gate.resolve();
    const [firstRuntime, secondRuntime] = await Promise.all([first, second]);
    assert.strictEqual(firstRuntime, secondRuntime);
    assert.equal(resumes, 1);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('opening a validated empty session keeps its UUID runtime without sending resume_session', async () => {
  const sessionId = 'eeeeeeee-ffff-4000-8111-222222222222';
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  let resumes = 0;
  SessionRuntime.prototype.start = async function () {
    (this as any).state = { status: 'connected' };
  };
  SessionRuntime.prototype.resumeOwnedSession = async function () { resumes += 1; };
  const manager = new SessionRuntimeManager({
    launchConfig: (ref) => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
  });
  try {
    const runtime = await manager.openSession({ projectPath: '/workspace', sessionId, mode: 'code' }, true);
    assert.equal(runtime.sessionId, sessionId);
    assert.equal(resumes, 0);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('closeProject removes runtimes from routing before asynchronous disposal', async () => {
  const projectPath = '/workspace-closing';
  const ref = { projectPath, sessionId: 'dddddddd-eeee-4fff-8000-111111111111' };
  const gate = deferred<void>();
  const originalDispose = SessionRuntime.prototype.dispose;
  SessionRuntime.prototype.dispose = async function () {
    await gate.promise;
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (candidate) => ({ workspace: candidate.projectPath, sessionId: candidate.sessionId, trusted: true }),
  });
  try {
    const runtime = await manager.ensure(ref, false);
    (runtime as any).activeTurn = true;
    await assert.rejects(manager.closeProject(projectPath), /cancel active turns/);
    assert.strictEqual(manager.get(ref.sessionId), runtime);
    (runtime as any).activeTurn = false;
    const closing = manager.closeProject(projectPath);
    assert.equal(manager.get(ref.sessionId), undefined);
    assert.equal(manager.isProjectClosing(projectPath), true);
    assert.throws(() => manager.require(ref), /not open/);
    await assert.rejects(manager.newSession(projectPath, undefined, { mode: 'code', projectId: projectPath }), /project is closing/);
    assert.throws(() => manager.openSession({ projectPath, sessionId: 'eeeeeeee-ffff-4000-8111-222222222222' }), /project is closing/);
    assert.equal(runtime.projectPath, projectPath);
    gate.resolve();
    await closing;
    assert.equal(manager.isProjectClosing(projectPath), false);
  } finally {
    SessionRuntime.prototype.dispose = originalDispose;
    await manager.dispose();
  }
});

test('restart exposes an explicit restarting state and structured connection diagnostics', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new SessionRuntime({
    diagnostics,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;
  (manager as any).startInternal = async () => {
    (manager as any).setState({ status: 'connected' });
  };

  await manager.restart();

  assert.deepEqual(states, ['restarting', 'connected']);
  const events = diagnostics.snapshot().map((entry) => JSON.parse(entry.message));
  assert.deepEqual(
    events.filter((entry) => entry.event === 'connection_state').map((entry) => entry.state.status),
    ['restarting', 'connected'],
  );
});

test('stop disconnects the active project and returns the bridge to idle', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;

  await manager.stop();

  assert.deepEqual(states, ['idle']);
  assert.deepEqual(manager.connectionState, { status: 'idle' });
});

test('restart surfaces launch failures instead of remaining stuck in restarting', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new SessionRuntime({
    diagnostics,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const states: string[] = [];
  (manager as any).registerIpc = () => undefined;
  (manager as any).broadcast = (_channel: string, payload: { status?: string }) => {
    if (payload?.status) states.push(payload.status);
  };
  (manager as any).stopBridge = async () => undefined;
  (manager as any).startInternal = async () => {
    throw new Error('macOS login keychain is locked or access is denied (deepseek)');
  };

  await assert.rejects(manager.restart(), /login keychain is locked/);

  assert.deepEqual(states, ['restarting', 'error']);
  assert.deepEqual(manager.connectionState, {
    status: 'error',
    message: 'macOS login keychain is locked or access is denied (deepseek)',
  });
});

test('lockfile polling ignores invalid candidates until a valid private lockfile appears', async () => {
  const diagnostics = new DiagnosticBuffer();
  const workspace = temporaryDirectory();
  const launchDir = temporaryDirectory();
  const lockfilePath = join(launchDir, 'bridge.lock');
  const manager = new SessionRuntime({
    diagnostics,
    lockfileTimeoutMs: 500,
    launchConfig: () => ({ workspace, trusted: true }),
  });
  (manager as any).generation = 1;
  (manager as any).child = { pid: process.pid };

  writeFileSync(lockfilePath, '{"pid":', { mode: 0o600 });
  chmodSync(lockfilePath, 0o600);
  setTimeout(() => {
    writeFileSync(lockfilePath, JSON.stringify({
      pid: process.pid,
      workspaceFolders: [workspace],
      ideName: 'LingXi-Bridge',
      transport: 'ws',
      runningInWindows: false,
      authToken: '0123456789abcdef0123456789abcdef',
    }), { mode: 0o600 });
    chmodSync(lockfilePath, 0o600);
  }, 75);

  try {
    const resolved = await (manager as any).waitForLockfile(launchDir, { workspace, trusted: true }, 1);
    assert.equal(resolved, lockfilePath);
    const ignored = diagnostics.snapshot().map((entry) => JSON.parse(entry.message))
      .find((entry) => entry.event === 'lockfile_ignored');
    assert.equal(ignored?.file, lockfilePath);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
    rmSync(launchDir, { recursive: true, force: true });
  }
});

test('privileged bridge access re-checks current workspace trust before use', () => {
  let trusted = true;
  const client = { sendCommand: () => undefined };
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted }),
  });
  (manager as any).client = client;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).activeWorkspaceTrusted = true;
  (manager as any).state = { status: 'connected' };

  assert.equal((manager as any).requireClient(), client);
  trusted = false;
  assert.throws(() => (manager as any).requireClient(), /workspace trust is required/);
});

test('computer access requests broadcast to renderers and are tracked as pending', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  const request = {
    request_id: 5,
    reason: 'automate chat',
    apps: [{ label: 'Slack' }],
    tier: 'full',
    clipboard_read: false,
    clipboard_write: false,
    system_key_combos: false,
  };
  handlers.get('computerAccess')!(request);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:computerAccess', payload: request }]);
  assert.ok((manager as any).pendingComputerAccessIds.has(5));

  // stopBridge (restart/disconnect) clears pending computer access ids, just
  // like it clears pending permission ids.
  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingComputerAccessIds.size, 0);
});

test('AskUserQuestion events are tracked and cleared across disconnect', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: unknown }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 7,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!(event);

  assert.deepEqual(broadcasts, [{ channel: 'lingxi:event', payload: event }]);
  assert.ok((manager as any).pendingAskUserQuestionIds.has(7));
  assert.deepEqual((manager as any).pendingAskUserQuestionRequests.get(7)?.request, event.request);
  assert.deepEqual(manager.pendingAskUserQuestions, [event.request]);

  (manager as any).child = null;
  await (manager as any).stopBridge();
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionRequests.size, 0);
});

test('AskUserQuestion resolved events clear replay state before a renderer reload', () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 11,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
      timeout_secs: 60,
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 11,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(11), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(11), false);
});

test('AskUserQuestion broker resolution clears replay state without a renderer answer', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).broadcast = () => undefined;
  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!({
    type: 'ask_user_question',
    request: {
      request_id: 12,
      timeout_secs: 0,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [
          { label: 'Safe', description: 'Keep safeguards enabled' },
          { label: 'Fast', description: 'Move quicker' },
        ],
        multi_select: false,
      }],
    },
  });
  handlers.get('event')!({
    type: 'ask_user_question_resolved',
    request_id: 12,
  });

  assert.equal((manager as any).pendingAskUserQuestionIds.has(12), false);
  assert.equal((manager as any).pendingAskUserQuestionRequests.has(12), false);
});

test('permission resolutions clear every renderer and pending host state', async () => {
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    sessionId: '44444444-5555-4666-8777-888888888888',
    projectPath: '/workspace',
    envelopeEvents: true,
  } as any);
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const client = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return client; },
  };
  const first: Array<{ channel: string; payload: unknown }> = [];
  const second: Array<{ channel: string; payload: unknown }> = [];
  const firstWindow = fakeWebContents(first);
  const secondWindow = fakeWebContents(second);

  (runtime as any).client = client;
  (runtime as any).wireClient(client, 0);
  (runtime as any).activeTurn = true;
  runtime.registerWindow(firstWindow as any, 'app://desktop/index.html');
  await handlers.get('permission')!({ request_id: 17, kind: { type: 'exit_plan_mode', plan: '# plan' } });

  // A renderer that joins while the request is parked must receive the same
  // request; permission frames are not part of the sequenced event replay.
  runtime.registerWindow(secondWindow as any, 'app://desktop/index.html');
  assert.equal(first.filter((entry) => entry.channel === 'lingxi:permission').length, 1);
  assert.equal(second.filter((entry) => entry.channel === 'lingxi:permission').length, 1);
  assert.equal(runtime.pendingInteractions, 1);

  handlers.get('event')!({
    type: 'permission_request_resolved',
    request_id: 17,
    resolution: 'expired',
  });

  assert.equal(runtime.pendingInteractions, 0);
  assert.equal(first.filter((entry) => entry.channel === 'lingxi:event').length, 1);
  assert.equal(second.filter((entry) => entry.channel === 'lingxi:event').length, 1);

  // A delayed duplicate frame must not resurrect the terminal request.
  await handlers.get('permission')!({ request_id: 17, kind: { type: 'exit_plan_mode', plan: '# stale' } });
  assert.equal(runtime.pendingInteractions, 0);
});

test('registerWindow replays pending AskUserQuestion requests to a reloaded renderer', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const webContents = {
    send: (channel: string, payload: unknown) => sent.push({ channel, payload }),
    once: (_event: string, _handler: () => void) => undefined,
    isDestroyed: () => false,
  };
  const event = {
    type: 'ask_user_question',
    request: {
      request_id: 13,
      questions: [{
        question: 'Choose a mode',
        header: 'Mode',
        options: [{ label: 'Safe', description: 'Keep safeguards enabled' }],
        multi_select: false,
      }],
    },
  };

  (manager as any).client = fakeClient;
  (manager as any).wireClient(fakeClient, 0);
  (manager as any).activeTurn = true;
  handlers.get('event')!(event);

  manager.registerWindow(webContents as any, 'app://desktop/index.html');

  assert.deepEqual(sent, [{ channel: 'lingxi:event', payload: event }]);
});

test('old bridge generations cannot repopulate turn or permission state', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: unknown[] = [];
  (manager as any).broadcast = (_channel: string, payload: unknown) => broadcasts.push(payload);
  (manager as any).generation = 2;
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const staleClient = {
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return staleClient; },
  };

  (manager as any).wireClient(staleClient, 1);
  handlers.get('event')!({ type: 'turn_started', turn_id: 9 });
  await handlers.get('permission')!({ request_id: 9, kind: { type: 'exit_plan_mode' } });

  assert.equal(manager.turnActive, false);
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.deepEqual(broadcasts, []);
});

test('turn terminal owns release and rejects late interactive or tool events', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: any }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).client = fakeClient;
  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({ type: 'turn_started', turn_id: 7 });
  await handlers.get('permission')!({ request_id: 4, kind: { type: 'exit_plan_mode' } });
  handlers.get('event')!({ type: 'error', kind: { type: 'internal' }, message: 'non-terminal command error' });
  assert.equal(manager.turnActive, true, 'generic errors cannot release a running turn');
  assert.ok((manager as any).pendingPermissionIds.has(4));

  handlers.get('event')!({
    type: 'turn_ended',
    outcome: { type: 'cancelled' },
    cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '' },
  });
  assert.equal(manager.turnActive, false);
  assert.equal((manager as any).pendingPermissionIds.size, 0);

  await handlers.get('permission')!({ request_id: 5, kind: { type: 'exit_plan_mode' } });
  handlers.get('event')!({ type: 'tool_heartbeat', id: 'late', tool: 'WebSearch', elapsed_ms: 99_000 });
  handlers.get('event')!({
    type: 'ask_user_question',
    request: { request_id: 6, questions: [] },
  });

  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal(
    broadcasts.some((entry) => entry.payload?.type === 'tool_heartbeat' && entry.payload?.id === 'late'),
    false,
  );
});

test('cancelling turn rejects late interactions but keeps turn events flowing', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const broadcasts: Array<{ channel: string; payload: any }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).client = fakeClient;
  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({ type: 'turn_started', turn_id: 8 });
  (manager as any).cancellingTurn = true;
  await handlers.get('permission')!({ request_id: 8, kind: { type: 'exit_plan_mode' } });
  handlers.get('computerAccess')!({ request_id: 9, reason: 'late', apps: [] });
  handlers.get('event')!({
    type: 'ask_user_question',
    request: { request_id: 10, questions: [] },
  });
  handlers.get('event')!({ type: 'tool_heartbeat', id: 'owned', tool: 'Bash', elapsed_ms: 17_000 });

  assert.equal(manager.turnActive, true, 'cancellation does not release the turn slot');
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.equal((manager as any).pendingComputerAccessIds.size, 0);
  assert.equal((manager as any).pendingAskUserQuestionIds.size, 0);
  assert.equal(
    broadcasts.some((entry) => entry.payload?.type === 'tool_heartbeat' && entry.payload?.id === 'owned'),
    true,
    'Block-tool heartbeats remain visible while stopping',
  );
});

test('an engine-initiated turn re-arms the host and its permission prompts reach the user', async () => {
  const diagnostics = new DiagnosticBuffer();
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    diagnostics,
  });
  const broadcasts: Array<{ channel: string; payload: any }> = [];
  (manager as any).broadcast = (channel: string, payload: unknown) => broadcasts.push({ channel, payload });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };

  (manager as any).client = fakeClient;
  (manager as any).wireClient(fakeClient, 0);
  handlers.get('event')!({ type: 'turn_started', turn_id: 3 });
  handlers.get('event')!({
    type: 'turn_ended',
    outcome: { type: 'end_turn' },
    cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '' },
  });
  assert.equal(manager.turnActive, false, 'the user turn released the slot');

  // Nothing the CLIENT did starts what comes next: the engine drains a
  // background-task notification (or the leftover queue) into a follow-up turn
  // of its own. `turn_started` is the ONLY thing that can re-arm the host, so
  // the engine has to send one — see `driver.rs`
  // `a_queued_turn_announces_itself_before_it_streams` and the
  // `AdapterOutputStream::emit_turn_started` impl, which did not exist and made
  // every such turn invisible here.
  const beforeReArm = diagnostics.snapshot().length;
  await handlers.get('permission')!({ request_id: 21, kind: { type: 'exit_plan_mode' } });
  assert.equal((manager as any).pendingPermissionIds.size, 0, 'no turn owns this request yet');
  assert.match(
    diagnostics.snapshot().slice(beforeReArm).map((entry) => entry.message).join('\n'),
    /dropped permission request 21/,
    'dropping one is a five-minute stall at the engine gate, never a silent no-op',
  );

  handlers.get('event')!({ type: 'turn_started', turn_id: undefined });
  assert.equal(manager.turnActive, true, 'the engine-initiated turn owns the slot');

  await handlers.get('permission')!({ request_id: 22, kind: { type: 'exit_plan_mode' } });
  handlers.get('event')!({ type: 'tool_use_started', id: 'rewake', tool: 'Bash', input_json: '{}',
    header: { verb: 'shell', label: 'Running shell command', title: 'Running shell command' } });

  assert.ok((manager as any).pendingPermissionIds.has(22), 'its permission prompts reach the user');
  assert.equal(
    broadcasts.some((entry) => entry.payload?.type === 'tool_use_started' && entry.payload?.id === 'rewake'),
    true,
    'and its transcript events reach the renderer',
  );
});

test('prompt submission owns the pre-turn_started cancellation window', async () => {
  const calls: Array<{ type: string; value?: unknown }> = [];
  const manager = new SessionRuntime({
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const fakeClient = {
    requestPermissionScope: async (id: number) => ({ request_id: id, background_owned: false }),
    sendPrompt: (text: string, opts?: { images?: unknown[] }) => calls.push({ type: 'prompt', value: { text, images: opts?.images ?? [] } }),
    cancel: (turnId?: number) => calls.push({ type: 'cancel', value: turnId }),
    on: (event: string, handler: (...args: unknown[]) => void) => { handlers.set(event, handler); return fakeClient; },
  };
  (manager as any).client = fakeClient;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).client = fakeClient;
  (manager as any).wireClient(fakeClient, 0);

  (manager as any).sendPrompt('hello');
  assert.equal(manager.turnActive, true);
  (manager as any).cancelTurn(undefined);
  assert.equal((manager as any).cancellingTurn, true);

  handlers.get('event')!({ type: 'turn_started', turn_id: 91 });
  await handlers.get('permission')!({ request_id: 91, kind: { type: 'exit_plan_mode' } });

  assert.equal((manager as any).cancellingTurn, true, 'turn_started must not undo an accepted cancel');
  assert.equal((manager as any).pendingPermissionIds.size, 0);
  assert.deepEqual(calls, [
    { type: 'prompt', value: { text: 'hello', images: [] } },
    { type: 'cancel', value: undefined },
  ]);
});

test('prompt submission forwards validated image attachments to the bridge client', () => {
  const calls: Array<{ text: string; images?: unknown[] }> = [];
  const manager = new SessionRuntime({
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
  });
  const fakeClient = {
    sendPrompt: (text: string, opts?: { images?: unknown[] }) => calls.push({ text, images: opts?.images }),
  };
  (manager as any).client = fakeClient;
  (manager as any).activeWorkspace = '/workspace';
  (manager as any).sendPrompt('describe this', [{
    media_type: 'image/png',
    base64: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]).toString('base64'),
  }]);
  assert.deepEqual(calls, [{
    text: 'describe this',
    images: [{
      media_type: 'image/png',
      base64: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]).toString('base64'),
    }],
  }]);
});

test('clean child exit clears the SIGKILL timer so the process group is not signalled twice', async () => {
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    stopTimeoutMs: 40,
  });
  const child = new EventEmitter() as EventEmitter & {
    exitCode: number | null;
    signalCode: NodeJS.Signals | null;
    pid: number;
  };
  child.exitCode = null;
  child.signalCode = null;
  child.pid = 4242;

  const signals: NodeJS.Signals[] = [];
  (manager as any).child = child;
  (manager as any).signalChildTree = (_child: unknown, signal: NodeJS.Signals) => {
    signals.push(signal);
  };

  const stopping = (manager as any).stopBridge();
  setTimeout(() => child.emit('exit', 0, null), 10);
  await stopping;
  await new Promise((resolve) => setTimeout(resolve, 80));

  assert.deepEqual(signals, ['SIGINT']);
});

// ── Bypass Permissions acceptance gate (security #34) ──────────────────────

function trustedManager(confirm?: () => Promise<boolean>): {
  manager: BridgeManager;
  commands: Array<Record<string, unknown>>;
} {
  const commands: Array<Record<string, unknown>> = [];
  const manager = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    accessState: () => ({ workspace: '/workspace', trusted: true }),
    ...(confirm ? { confirmBypassPermissions: confirm } : {}),
  });
  (manager as any).client = {
    sendCommand: (command: Record<string, unknown>) => commands.push(command),
  };
  (manager as any).activeWorkspace = '/workspace';
  return { manager, commands };
}

test('bypassPermissions is refused when the acceptance dialog is declined', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return false; // user picked Cancel
  });
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(confirmCalls, 1, 'the acceptance confirmer is consulted');
  assert.equal(commands.length, 0, 'a declined bypass never reaches the engine');
});

test('bypassPermissions reaches the engine only after acceptance', async () => {
  const { manager, commands } = trustedManager(async () => true); // user accepted
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' });
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['type'], 'set_permission_mode');
  assert.equal(commands[0]!['mode'], 'bypassPermissions');
});

test('bypassPermissions is refused when no confirmer is wired (never one-click)', async () => {
  const { manager, commands } = trustedManager(); // no confirmBypassPermissions
  await assert.rejects(
    (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'bypassPermissions' }),
    /not accepted/,
  );
  assert.equal(commands.length, 0);
});

test('non-bypass permission modes reach the engine during an active turn', async () => {
  let confirmCalls = 0;
  const { manager, commands } = trustedManager(async () => {
    confirmCalls += 1;
    return true;
  });
  (manager as any).activeTurn = true;
  await (manager as any).dispatchCommand({ type: 'set_permission_mode', mode: 'acceptEdits' });
  assert.equal(confirmCalls, 0, 'only bypassPermissions consults the confirmer');
  assert.equal(commands.length, 1);
  assert.equal(commands[0]!['mode'], 'acceptEdits');
});

// ── Settings/permission/MCP command surface reaches the engine ─────────────

test('the settings and MCP commands reach the engine through dispatchCommand', async () => {
  const { manager, commands } = trustedManager();
  await (manager as any).dispatchCommand({ type: 'update_settings', destination: 'user', patch_json: '{"outputStyle":"terse"}' });
  await (manager as any).dispatchCommand({
    type: 'upsert_mcp_server',
    scope: 'user',
    name: 'filesystem',
    config_json: '{"command":"npx"}',
  });
  assert.deepEqual(commands.map((command) => command['type']), ['update_settings', 'upsert_mcp_server']);
});

test('a real protocol command outside the desktop surface never reaches the engine', async () => {
  const { manager, commands } = trustedManager();
  await assert.rejects((manager as any).dispatchCommand({ type: 'clear_session' }), /command is not allowed/);
  assert.equal(commands.length, 0, 'a rejected command must never be forwarded');
});

// ---------------------------------------------------------------------------
// An engine request that must be answered EXACTLY ONCE cannot be broadcast.
//
test('engine audio dispatch is handled exactly once in main and never sent to a renderer', async () => {
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const calls: unknown[] = [];
  const commands: unknown[] = [];
  const service = {
    getCapabilities: () => ({
      service_epoch: 3,
      support_revision: 1,
      supported_operations: ['record'],
      readiness: [],
      max_payload_bytes: 1_000_000,
    }),
    initializeCapabilities: async () => {},
    executeAudioRequest: async (request: unknown) => {
      calls.push(request);
      return { type: 'recording_started', handle: 'recording-1' };
    },
    cancelAudioRequest: async (identity: unknown) => { calls.push({ cancel: identity }); },
    endAudioOwner: async () => {},
    onEvent: () => () => {},
  };
  const runtime = new SessionRuntime({
    launchConfig: { workspace: '/workspace', sessionId: '44444444-5555-4666-8777-888888888888', trusted: true },
    sessionId: '44444444-5555-4666-8777-888888888888',
    projectPath: '/workspace',
    envelopeEvents: true,
    audioService: service,
  } as any);
  const renderer = fakeWebContents(sent);
  runtime.registerWindow(renderer as any, 'app://desktop/index.html');
  const client = new EventEmitter() as any;
  client.sendCommand = (command: unknown) => { commands.push(command); };
  client.close = () => undefined;
  (runtime as any).client = client;
  (runtime as any).wireClient(client, (runtime as any).generation);
  const request = {
    identity: { id: 'audio-1', generation: 1, service_epoch: 3 },
    owner: { type: 'session', session_id: '44444444-5555-4666-8777-888888888888' },
    max_payload_bytes: 1_000_000,
    operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
  };
  client.emit('event', { type: 'audio_request', request });
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(calls, [request]);
  assert.deepEqual(commands, [{ type: 'audio_response', identity: request.identity, result: { type: 'recording_started', handle: 'recording-1' } }]);
  assert.deepEqual(sent, [], 'audio requests must not be forwarded to renderer webContents');

  const identity = { id: 'audio-1', generation: 1, service_epoch: 3 };
  client.emit('event', { type: 'audio_cancel', identity });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(calls[1], { cancel: identity });
  await runtime.dispose();
});

test('unexpected bridge close cancels pending audio and ends only its accepted owner', async () => {
  const serviceResult = deferred<any>();
  const calls: unknown[] = [];
  const commands: unknown[] = [];
  const runtime = new SessionRuntime({
    launchConfig: { workspace: '/workspace', sessionId: '55555555-6666-4777-8888-999999999999', trusted: true },
    sessionId: '55555555-6666-4777-8888-999999999999',
    projectPath: '/workspace',
    audioService: {
      getCapabilities: () => ({ service_epoch: 3, support_revision: 1, supported_operations: ['record'], readiness: [], max_payload_bytes: 1_000_000 }),
      initializeCapabilities: async () => {},
      executeAudioRequest: async (request: unknown) => { calls.push({ execute: request }); return serviceResult.promise; },
      cancelAudioRequest: async (identity: unknown) => { calls.push({ cancel: identity }); },
      endAudioOwner: async (owner: unknown) => { calls.push({ end: owner }); },
      onEvent: () => () => {},
    },
  } as any);
  const client = new EventEmitter() as any;
  client.sendCommand = (command: unknown) => commands.push(command);
  client.close = () => undefined;
  (runtime as any).client = client;
  (runtime as any).generation = 4;
  (runtime as any).wireClient(client, 4);
  const request = {
    identity: { id: 'close-pending', generation: 1, service_epoch: 3 },
    owner: { type: 'system', instance_id: 'desktop-companion' },
    max_payload_bytes: 1_000_000,
    operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
  };

  client.emit('event', { type: 'audio_request', request });
  await new Promise((resolve) => setImmediate(resolve));
  client.emit('close', 1006, 'unexpected process disconnect');
  serviceResult.resolve({ type: 'recording_started', handle: 'late-start' });
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));

  assert.ok(calls.some((call: any) => call.cancel?.id === request.identity.id));
  assert.ok(calls.some((call: any) => call.end?.type === 'system' && call.end.instance_id === 'desktop-companion'));
  assert.deepEqual(commands, [], 'a disconnected bridge must not receive a late success response');
  await runtime.dispose();
});

test('disposing a runtime during delayed start cancels its identity before rolling back the late lease', async () => {
  const serviceResult = deferred<any>();
  const calls: unknown[] = [];
  const runtime = new SessionRuntime({
    launchConfig: { workspace: '/workspace', sessionId: '66666666-7777-4888-8999-aaaaaaaaaaaa', trusted: true },
    sessionId: '66666666-7777-4888-8999-aaaaaaaaaaaa',
    projectPath: '/workspace',
    audioService: {
      getCapabilities: () => ({ service_epoch: 3, support_revision: 1, supported_operations: ['record'], readiness: [], max_payload_bytes: 1_000_000 }),
      initializeCapabilities: async () => {},
      executeAudioRequest: async () => serviceResult.promise,
      cancelAudioRequest: async (identity: unknown) => { calls.push({ cancel: identity }); },
      endAudioOwner: async (owner: unknown) => { calls.push({ end: owner }); },
      onEvent: () => () => {},
    },
  } as any);
  const commands: unknown[] = [];
  const client = new EventEmitter() as any;
  client.sendCommand = (command: unknown) => commands.push(command);
  client.close = () => undefined;
  (runtime as any).client = client;
  (runtime as any).generation = 12;
  (runtime as any).wireClient(client, 12);
  const request = {
    identity: { id: 'dispose-pending', generation: 1, service_epoch: 3 },
    owner: { type: 'session', session_id: '66666666-7777-4888-8999-aaaaaaaaaaaa' },
    max_payload_bytes: 1_000_000,
    operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
  };
  client.emit('event', { type: 'audio_request', request });
  await new Promise((resolve) => setImmediate(resolve));
  const disposing = runtime.dispose();
  await disposing;
  serviceResult.resolve({ type: 'recording_started', handle: 'late-start' });
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));

  assert.ok(calls.some((call: any) => call.cancel?.id === request.identity.id));
  assert.ok(calls.some((call: any) => call.end?.type === 'session' && call.end.session_id === request.owner.session_id));
  assert.deepEqual(commands, []);
});

test('foreground open activates a session already opening for archive preflight', async () => {
  const ref = { mode: 'code' as const, projectPath: '/workspace', sessionId: 'cccccccc-dddd-4eee-8fff-000000000001' };
  const gate = deferred<void>();
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { await gate.promise; (this as any).state = { status: 'connected' }; };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }), launchConfig: (r) => ({ workspace: r.projectPath, sessionId: r.sessionId, trusted: true }) });
  try {
    const background = manager.openSession(ref, false, undefined, false);
    const foreground = manager.openSession(ref);
    gate.resolve();
    assert.strictEqual(await background, await foreground);
    assert.equal((manager as any).activeSessionId, ref.sessionId);
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('background session leases survive cache pressure and trim after archive preflight', async () => {
  const refs = [1, 2, 3].map((n) => ({ projectPath: '/workspace', mode: 'code' as const, sessionId: `cccccccc-dddd-4eee-8fff-00000000000${n}` }));
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { (this as any).state = { status: 'connected' }; };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }), maxCachedRuntimes: 1, launchConfig: (r) => ({ workspace: r.projectPath, sessionId: r.sessionId, trusted: true }) });
  try {
    await manager.openSession(refs[0]!);
    for (const ref of refs.slice(1)) {
      await manager.withBackgroundSession(ref, false, undefined, async (runtime) => {
        (manager as any).trimCache();
        assert.strictEqual(manager.get(ref.sessionId), runtime, 'operation must retain its runtime');
        assert.equal(manager.size, 2, 'active chat and preflight remain live');
      });
      assert.equal(manager.size, 1, 'cancelled preflight cannot leak a child process');
      assert.ok(manager.get(refs[0]!.sessionId));
    }
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
    await manager.dispose();
  }
});

test('successful scheduled creation commits the draft owner, while rejection keeps it reusable', async () => {
  const originalStart = SessionRuntime.prototype.start;
  const commits: Array<{ sessionId: string; active: boolean }> = [];
  SessionRuntime.prototype.start = async function () {
    (this as any).activeWorkspace = '/workspace';
    (this as any).activeWorkspaceTrusted = true;
    (this as any).state = { status: 'connected' };
    (this as any).draftModeRestorationRequired = false; // This fixture substitutes a ready engine.
  };
  const manager = new SessionRuntimeManager({
    launchConfig: (r) => ({ workspace: r.projectPath, sessionId: r.sessionId, trusted: true }),
    onFirstPromptSent: (ref, active) => { commits.push({ sessionId: ref.sessionId, active }); },
  });
  try {
    const ref = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' });
    const runtime = manager.require(ref);
    const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
    client.sendCommand = () => {};
    (runtime as any).client = client;
    (runtime as any).wireClient(client, 0);
    const create = (id: string) => runtime.dispatchCommand({ type: 'cron_manage', request_id: id, request: { action: 'create', cron: '0 9 * * *', prompt: 'Check updates' } });
    const rejected = create('rejected');
    assert.throws(() => runtime.beginArchive(), /pending interactions/);
    client.emit('event', { type: 'cron_result', request_id: 'rejected', jobs: [], error: 'Save failed' });
    await assert.rejects(rejected, /Save failed/);
    assert.deepEqual(commits, []);
    assert.equal((await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' })).sessionId, ref.sessionId);
    const accepted = create('accepted');
    client.emit('event', { type: 'cron_result', request_id: 'accepted', jobs: [] });
    await accepted;
    assert.deepEqual(commits, [{ sessionId: ref.sessionId, active: true }]);
    const next = await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' });
    assert.notEqual(next.sessionId, ref.sessionId);
    const nextRuntime = manager.require(next);
    const nextClient = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
    nextClient.sendCommand = () => {};
    (nextRuntime as any).client = nextClient;
    (nextRuntime as any).wireClient(nextClient, 0);
    const backgroundCreate = nextRuntime.dispatchCommand({ type: 'cron_manage', request_id: 'background', request: { action: 'create', cron: '0 9 * * *', prompt: 'Check updates' } });
    await manager.openSession(ref);
    nextClient.emit('event', { type: 'cron_result', request_id: 'background', jobs: [] });
    await backgroundCreate;
    assert.deepEqual(commits, [{ sessionId: ref.sessionId, active: true }, { sessionId: next.sessionId, active: false }]);
    assert.equal((manager as any).activeSessionId, ref.sessionId, 'background completion must not replace the active selection');
    assert.notEqual((await manager.newSession('/workspace', undefined, { mode: 'code', projectId: '/workspace' })).sessionId, next.sessionId, 'background success still commits its draft');
  } finally {
    SessionRuntime.prototype.start = originalStart;
    await manager.dispose();
  }
});

test('custom provider IDs follow the latest engine settings snapshot', () => {
  const manager = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const client = audioContextEventClient();
  (manager as any).wireClient(client, 0);
  client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providers: {
    'my-provider': { type: 'openai' }, 'Invalid ID': {}, null_profile: null,
  } }), provenance_json: '{}' });
  assert.deepEqual(manager.customProviderIds, ['my-provider']);
  client.emit('event', { type: 'settings_snapshot', effective_json: 'invalid json', provenance_json: '{}' });
  assert.deepEqual(manager.customProviderIds, []);
  client.emit('event', { type: 'settings_snapshot', effective_json: '{}', provenance_json: '{}' });
  assert.deepEqual(manager.customProviderIds, []);
});

test('account configuration captures effective region and profiles together after settings are ready', async () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
  const profiles = { custom: { enabled: false, provider_type: 'openai', base_url: 'https://configured.example/v1' } };
  let requests = 0;
  client.sendCommand = command => {
    assert.deepEqual(command, { type: 'refresh_listings', which: [{ type: 'settings' }] });
    requests++;
    queueMicrotask(() => client.emit('event', { type: 'settings_snapshot',
      effective_json: JSON.stringify({ providers: profiles, providerRegion: 'china_mainland' }), provenance_json: '{}' }));
  };
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).client = client;
  (runtime as any).wireClient(client, 0);
  const china = await runtime.providerAccountConfiguration();
  assert.deepEqual(china, { profilesJson: JSON.stringify(profiles), providerRegion: 'china_mainland' });
  assert.equal(requests, 1);
  client.emit('event', { type: 'settings_snapshot', effective_json: '{}', provenance_json: '{}' });
  assert.deepEqual(await runtime.providerAccountConfiguration(), { profilesJson: '{}', providerRegion: 'international' });
  client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providers: null, providerRegion: 'china_mainland' }), provenance_json: '{}' });
  assert.deepEqual(await runtime.providerAccountConfiguration(), { profilesJson: '{}', providerRegion: 'china_mainland' });
  assert.deepEqual(china, { profilesJson: JSON.stringify(profiles), providerRegion: 'china_mainland' });
  assert.equal(requests, 1);
});

test('account configuration rejects an unknown effective region instead of choosing another catalog', async () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const client = audioContextEventClient();
  (runtime as any).wireClient(client, 0);
  for (const providerRegion of ['unknown-region', null, 1]) {
    client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providerRegion }), provenance_json: '{}' });
    await assert.rejects(runtime.providerAccountConfiguration(), /Invalid provider region/);
  }
});

for (const interruption of ['cancel', 'restart'] as const) {
  test(`${interruption} interrupts a model switch and its waiting prompt`, async () => {
    const key = deferred<string>();
    const calls: string[] = [];
    const runtime = new SessionRuntime({
      launchConfig: () => ({ workspace: '/workspace', trusted: true }),
      resolveProviderAccounts: async () => undefined,
    });
    const client = new EventEmitter() as EventEmitter & { sendCommand(command: any): void; sendPrompt(): void; cancel(): void };
    (runtime as any).activeWorkspace = '/workspace';
    (runtime as any).activeWorkspaceTrusted = true;
    (runtime as any).client = client;
    (runtime as any).wireClient(client, 0);
    client.sendCommand = (command) => { calls.push(command.type); };
    client.sendPrompt = () => { calls.push('prompt'); };
    client.cancel = () => undefined;
    runtime.refreshProviderAccounts = async () => { await key.promise; };
    client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providers: { next: { models: ['model'] } } }), provenance_json: '{}' });
    const switching = runtime.dispatchCommand({ type: 'set_model', model: 'next/model' });
    const prompt = runtime.sendPrompt('hello');
    const settled = Promise.allSettled([switching, prompt]);
    assert.equal(runtime.turnActive, true);
    if (interruption === 'cancel') runtime.cancelTurn(undefined);
    else if (interruption === 'restart') {
      (runtime as any).startInternal = async () => undefined;
      await runtime.restart();
    }
    key.resolve('new-key');
    const results = await settled;
    assert.deepEqual(results.map((result) => result.status), ['rejected', 'rejected']);
    assert.equal(calls.includes('set_model'), false);
    assert.equal(calls.includes('prompt'), false);
    assert.equal(runtime.turnActive, false);
  });
}

test('engine warnings and errors retain severity and their originating session', () => {
  const diagnostics = new DiagnosticBuffer();
  const runtime = new SessionRuntime({ sessionId: '11111111-2222-4333-8444-555555555555',
    projectPath: '/workspace', launchConfig: () => ({ workspace: '/workspace', trusted: true }), diagnostics });
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  (runtime as any).captureLogs({ stdout, stderr }, ['private-value']);
  (runtime as any).generation = 1;
  stdout.write('\u001b[31mERROR\u001b[0m Fusion failed private-value\n');
  stdout.write('\u001b[33m WARN\u001b[0m terminal persistence failed\n');
  const entries = diagnostics.snapshot();
  assert.deepEqual(entries.map(entry => entry.level), ['error', 'warn']);
  const event = JSON.parse(entries[0]!.message);
  assert.equal(event.generation, 0);
  assert.equal(event.sessionId, '11111111-2222-4333-8444-555555555555');
  assert.doesNotMatch(event.message, /private-value|\[31m/);
  stdout.destroy(); stderr.destroy();
});

test('bridge error diagnostics identify the session and preserve sanitized failure details', () => {
  const diagnostics = new DiagnosticBuffer();
  const runtime = new SessionRuntime({
    sessionId: '11111111-2222-4333-8444-555555555555', projectPath: '/workspace',
    launchConfig: () => ({ workspace: '/workspace', trusted: true }), diagnostics,
  });
  const client = audioContextEventClient();
  (runtime as any).wireClient(client, 0);
  client.emit('event', { type: 'error', kind: { type: 'internal' }, message: 'Fusion failed token=private-value' });
  client.emit('event', { type: 'slash_command_result', is_error: true, display: 'invalid fusion configuration' });
  const entries = diagnostics.snapshot().filter(entry => entry.level === 'error');
  assert.equal(entries.length, 2);
  const error = JSON.parse(entries[0]!.message);
  assert.equal(error.event, 'client_error');
  assert.equal(error.sessionId, '11111111-2222-4333-8444-555555555555');
  assert.equal(error.projectPath, '/workspace');
  assert.match(error.message, /Fusion failed/);
  assert.doesNotMatch(JSON.stringify(entries), /private-value/);
  assert.equal(JSON.parse(entries[1]!.message).event, 'slash_command_error');
});

for (const stage of ['credentials', 'catalog', 'binding'] as const) {
  test(`cancel during scheduled ${stage} never sends a delayed automatic turn`, async () => {
    const runtime = new SessionRuntime({ sessionId: '11111111-2222-4333-8444-555555555555', projectPath: '/fixture' } as any);
    const entered = deferred();
    const gate = deferred();
    const sent: string[] = [];
    const client = { cancel: () => sent.push('cancel'), sendCommand: (command: { type: string }) => sent.push(command.type) };
    (runtime as any).requireClient = () => client;
    (runtime as any).client = client;
    (runtime as any).credentialRoutingSettings = {};
    const waitAt = async (name: string) => { if (stage === name) { entered.resolve(); await gate.promise; } };
    (runtime as any).ensureModelProviderCredential = () => waitAt('credentials');
    (runtime as any).scheduledModelCatalog = async () => {
      await waitAt('catalog');
      return { details: [{ reference: 'provider/model', reasoning: { options: [], provider_default: { type: 'automatic' } } }] };
    };
    let bound = false;
    const work = runtime.runScheduledTurn('cancelled-run', { prompt: 'automatic task', automation: { model: 'provider/model', reasoning: { type: 'automatic' } } } as any, async () => {
      bound = true;
      await waitAt('binding');
    });
    await entered.promise;
    runtime.cancelTurn(undefined);
    const rejected = assert.rejects(work, /^Error: cancelled:/);
    gate.resolve();
    await new Promise(resolve => setImmediate(resolve));
    const pending = (runtime as any).pendingScheduledTurns.get('cancelled-run');
    if (pending) pending.reject(new Error('unexpected scheduled dispatch'));
    await rejected;
    assert.deepEqual(sent, ['cancel']);
    assert.equal(bound, stage === 'binding');
    assert.equal(runtime.turnActive, false);
  });
}


/**
 * `runScheduledTurn` parks a promise in `pendingScheduledTurns` and hand-sets
 * `activeTurn`. Nothing else ever settles either one: only the
 * `scheduled_run_finished` branch resolves the promise and drops the latch, and
 * only the connection-loss paths reject it. Those three sites had no coverage,
 * and a scheduled run that never settles leaves `activeTurn` true forever —
 * which blocks credential writes, Codex login, engine restart and every
 * rewriting git operation on that session.
 */
function scheduledRuntime(mode?: 'chat' | 'code') {
  const runtime = new SessionRuntime({ sessionId: '11111111-2222-4333-8444-555555555555', projectPath: '/fixture' } as any, mode);
  (runtime as any).draftModeRestorationRequired = false;
  const sent: unknown[] = [];
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void; cancel(): void };
  client.sendCommand = (command) => { sent.push(command); };
  client.cancel = () => undefined;
  (runtime as any).requireClient = () => client;
  (runtime as any).client = client;
  (runtime as any).credentialRoutingSettings = {};
  (runtime as any).ensureModelProviderCredential = async () => undefined;
  (runtime as any).scheduledModelCatalog = async () => ({
    details: [{ reference: 'provider/model', reasoning: { options: [], provider_default: { type: 'automatic' } } }],
  });
  (runtime as any).wireClient(client, 0);
  const work = runtime.runScheduledTurn('run-1', {
    prompt: 'automatic task',
    automation: { model: 'provider/model', reasoning: { type: 'automatic' } },
  } as any);
  return { runtime, client, sent, work };
}

test('an admitted scheduled turn retires the mode-only draft launch', async () => {
  const state = scheduledRuntime('chat');
  await dispatched(state);
  assert.equal(state.runtime.sessionMode, 'chat');
  assert.equal((state.runtime as any).draftSessionMode, undefined);
  state.client.emit('event', { type: 'scheduled_run_finished', run_id: 'run-1', summary: 'done' });
  assert.equal(await state.work, 'done');
});

test('a scheduled send failure leaves the draft mode available for retry', async () => {
  const state = scheduledRuntime('chat');
  state.client.sendCommand = () => { throw new Error('send failed'); };
  await assert.rejects(state.work, /send failed/);
  assert.equal((state.runtime as any).draftSessionMode, 'chat');
});

/** Park until the dispatch has actually gone out, so the latch is really set. */
async function dispatched(state: ReturnType<typeof scheduledRuntime>) {
  for (let attempt = 0; attempt < 50 && state.sent.length === 0; attempt++) {
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal((state.sent[0] as { type: string } | undefined)?.type, 'scheduled_run_turn');
  assert.equal(state.runtime.turnActive, true);
}

test('scheduled_run_finished resolves the parked run and releases the turn latch', async () => {
  const state = await scheduledRuntime();
  await dispatched(state);
  state.client.emit('event', { type: 'scheduled_run_finished', run_id: 'run-1', summary: 'done' });
  assert.equal(await state.work, 'done');
  assert.equal(state.runtime.turnActive, false);
});

test('a failed scheduled run rejects, and only a busy: failure keeps the latch', async () => {
  const failed = await scheduledRuntime();
  await dispatched(failed);
  failed.client.emit('event', { type: 'scheduled_run_finished', run_id: 'run-1', error: 'the model refused' });
  await assert.rejects(failed.work, /the model refused/);
  assert.equal(failed.runtime.turnActive, false);

  // `busy:` means the engine never took the turn over — the turn that IS
  // running belongs to someone else, so clearing the latch would be a lie.
  const busy = await scheduledRuntime();
  await dispatched(busy);
  busy.client.emit('event', { type: 'scheduled_run_finished', run_id: 'run-1', error: 'busy: a turn is already running' });
  await assert.rejects(busy.work, /busy:/);
  assert.equal(busy.runtime.turnActive, true);
});

test('losing the connection rejects the parked run instead of latching it forever', async () => {
  const state = await scheduledRuntime();
  await dispatched(state);
  (state.runtime as any).setState({ status: 'disconnected' });
  await assert.rejects(state.work, /Scheduled connection closed/);
  assert.equal(state.runtime.turnActive, false);
  assert.equal((state.runtime as any).pendingScheduledTurns.size, 0);
});

test('cron_run_bound settles the binding wait the scheduler blocks on', async () => {
  const runtime = new SessionRuntime({ sessionId: '11111111-2222-4333-8444-555555555555', projectPath: '/fixture' } as any);
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: unknown): void };
  client.sendCommand = () => undefined;
  (runtime as any).requireClient = () => client;
  (runtime as any).client = client;
  (runtime as any).wireClient(client, 0);

  const bound = runtime.markCronRunStarted('bind-1', 'session-1', { transcriptPath: '/fixture/session-1.jsonl', mode: 'code', scopeKey: '/fixture' });
  assert.equal((runtime as any).pendingRunBindings.size, 1);
  client.emit('event', { type: 'cron_run_bound', run_id: 'bind-1' });
  await bound;
  assert.equal((runtime as any).pendingRunBindings.size, 0);

  const refused = runtime.markCronRunStarted('bind-2', 'session-1', { transcriptPath: '/fixture/session-1.jsonl', mode: 'code', scopeKey: '/fixture' });
  client.emit('event', { type: 'cron_run_bound', run_id: 'bind-2', error: 'that session is gone' });
  await assert.rejects(refused, /that session is gone/);
  assert.equal((runtime as any).pendingRunBindings.size, 0);
});

/**
 * `GitActivityTracker`'s own comment says an agent counted as running "pinned
 * the worktree guard AND the session's engine runtime" — but `hasActiveAgents`
 * had no reader outside the git handler, so the runtime half was never true.
 * A coordinator worker outlives the turn that started it; evicting its runtime
 * kills the engine process it is still running inside.
 */
test('a session with a coordinator worker still running survives cache pressure', async () => {
  const refs = [1, 2].map((n) => ({ projectPath: '/workspace', mode: 'code' as const, sessionId: `eeeeeeee-ffff-4aaa-8bbb-00000000000${n}` }));
  const originalStart = SessionRuntime.prototype.start;
  const originalResume = SessionRuntime.prototype.resumeOwnedSession;
  SessionRuntime.prototype.start = async function () { (this as any).state = { status: 'connected' }; };
  SessionRuntime.prototype.resumeOwnedSession = async function () {};
  const open = async () => {
    const manager = new SessionRuntimeManager({ resolveSavedTranscript: async ref => ({ path: `/saved/${ref.sessionId}.jsonl`, mode: 'code' as const }), maxCachedRuntimes: 1, launchConfig: (r) => ({ workspace: r.projectPath, sessionId: r.sessionId, trusted: true }) });
    const first = await manager.openSession(refs[0]!);
    const client = audioContextEventClient();
    (first as any).wireClient(client, (first as any).generation);
    return { manager, first, client };
  };
  try {
    const idle = await open();
    await idle.manager.openSession(refs[1]!);
    (idle.manager as any).trimCache();
    assert.equal(idle.first.hasActiveAgents, false);
    assert.equal(idle.manager.get(refs[0]!.sessionId), undefined, 'the control must actually be evictable');
    await idle.manager.dispose();

    const busy = await open();
    busy.client.emit('event', { type: 'coordinator_status', active_workers: 1 });
    assert.equal(busy.first.hasActiveAgents, true, 'the event must reach the tracker');
    await busy.manager.openSession(refs[1]!);
    (busy.manager as any).trimCache();
    assert.strictEqual(busy.manager.get(refs[0]!.sessionId), busy.first, 'a running worker pins its runtime');
    await busy.manager.dispose();
  } finally {
    SessionRuntime.prototype.start = originalStart;
    SessionRuntime.prototype.resumeOwnedSession = originalResume;
  }
});

test('Windows managed child exits through the authenticated command before its channel closes', async () => {
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!;
  Object.defineProperty(process, 'platform', { ...platform, value: 'win32' });
  try {
    const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }), stopTimeoutMs: 100 });
    const child = Object.assign(new EventEmitter(), { exitCode: null as number | null, signalCode: null as NodeJS.Signals | null, pid: 4242 });
    const actions: string[] = [];
    const client = Object.assign(new EventEmitter(), {
      sendCommand: (command: { type: string }) => {
        actions.push(command.type);
        setTimeout(() => { child.exitCode = 0; actions.push('exit'); child.emit('exit', 0, null); }, 10);
      },
      close: () => { assert.equal(child.exitCode, 0); actions.push('close'); },
    });
    (runtime as any).child = child;
    (runtime as any).client = client;
    (runtime as any).signalChildTree = () => { throw new Error('graceful Windows exit must not force-kill the child'); };
    await (runtime as any).stopBridge();
    assert.deepEqual(actions, ['request_exit', 'exit', 'close']);
    await new Promise(resolve => setTimeout(resolve, 120));
    assert.deepEqual(actions, ['request_exit', 'exit', 'close'], 'clean exit clears the forced-termination deadline');
  } finally { Object.defineProperty(process, 'platform', platform); }
});

test('Windows externally reused peer is disconnected without requesting its exit', async () => {
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!;
  Object.defineProperty(process, 'platform', { ...platform, value: 'win32' });
  try {
    const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
    const actions: string[] = [];
    (runtime as any).client = Object.assign(new EventEmitter(), {
      sendCommand: () => { throw new Error('unowned peer must not receive RequestExit'); },
      close: () => actions.push('close'),
    });
    (runtime as any).adoptedPid = 4242;
    (runtime as any).adoptedProcessOwned = false;
    (runtime as any).stopAdoptedBridge = () => { throw new Error('unowned peer must not be terminated'); };
    await (runtime as any).stopBridge();
    assert.deepEqual(actions, ['close']);
  } finally { Object.defineProperty(process, 'platform', platform); }
});

test('Windows owned adopted process drains after RequestExit without a synthetic signal', async () => {
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')!;
  const originalKill = process.kill;
  const keepAlive = setInterval(() => {}, 10);
  const signals: Array<NodeJS.Signals | number | undefined> = [];
  Object.defineProperty(process, 'platform', { ...platform, value: 'win32' });
  process.kill = ((_pid: number, signal?: NodeJS.Signals | number) => { signals.push(signal); return true; }) as typeof process.kill;
  try {
    const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }), stopTimeoutMs: 150 });
    const actions: string[] = [];
    let alive = true;
    (runtime as any).adoptedPid = 4242;
    (runtime as any).adoptedProcessOwned = true;
    (runtime as any).processIsAlive = () => alive;
    (runtime as any).client = Object.assign(new EventEmitter(), {
      sendCommand: (command: { type: string }) => {
        actions.push(command.type);
        setTimeout(() => { alive = false; actions.push('exit'); }, 10);
      },
      close: () => { assert.equal(alive, false); actions.push('close'); },
    });
    await (runtime as any).stopBridge();
    assert.deepEqual(signals, []);
    assert.deepEqual(actions, ['request_exit', 'exit', 'close']);
  } finally {
    clearInterval(keepAlive);
    process.kill = originalKill;
    Object.defineProperty(process, 'platform', platform);
  }
});

test('Mod UI requests and event frames stay bound to their session and request ids', async () => {
  const sent: Array<{ channel: string; payload: unknown }> = [];
  const webContents = fakeWebContents(sent);
  const makeRuntime = (sessionId: string) => {
    const runtime = new SessionRuntime({
      launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
      sessionId,
      projectPath: '/workspace',
      envelopeEvents: true,
    });
    runtime.registerWindow(webContents as any, 'app://desktop/index.html');
    const commands: any[] = [];
    const client = new EventEmitter() as EventEmitter & { sendCommand(command: any): void };
    client.sendCommand = (command) => { commands.push(command); };
    (runtime as any).client = client;
    (runtime as any).activeWorkspace = '/workspace';
    (runtime as any).activeWorkspaceTrusted = true;
    (runtime as any).generation = 1;
    (runtime as any).wireClient(client, 1);
    return { runtime, commands, client };
  };
  const firstId = '11111111-2222-4333-8444-555555555555';
  const secondId = '22222222-3333-4444-8555-666666666666';
  const first = makeRuntime(firstId);
  const second = makeRuntime(secondId);
  const request = {
    subtype: 'ui_render', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1',
    props: { plugin_owned: { display: true } }, future_control_field: 'strip-me',
  } as const;
  const firstPending = first.runtime.dispatchModUiControl(request);
  const secondPending = second.runtime.dispatchModUiControl(request);
  const firstCommand = first.commands[0]!;
  const secondCommand = second.commands[0]!;
  assert.equal(firstCommand.type, 'ui_render');
  assert.equal(secondCommand.type, 'ui_render');
  assert.notEqual(firstCommand.request_id, secondCommand.request_id);
  assert.deepEqual(JSON.parse(firstCommand.request_json), {
    subtype: 'ui_render', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1',
    props: { plugin_owned: { display: true } },
  });

  let secondSettled = false;
  void secondPending.then(() => { secondSettled = true; });
  const response = JSON.stringify({ tree: { type: 'text', text: 'ok' }, props: {}, rewritten: false, hooked: true });
  const metadata = JSON.stringify({ renderRevision: 7, clientStateToken: '42' });
  second.client.emit('event', { type: 'ui_control_result', request_id: firstCommand.request_id, response_json: response, metadata_json: metadata });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(secondSettled, false, 'a matching request id from another session cannot settle this caller');
  second.client.emit('event', { type: 'ui_control_result', request_id: secondCommand.request_id, response_json: response, metadata_json: metadata });
  first.client.emit('event', { type: 'ui_control_result', request_id: firstCommand.request_id, response_json: response, metadata_json: metadata });
  assert.deepEqual(await firstPending, { response: JSON.parse(response), metadata: { renderRevision: 7, clientStateToken: '42' } });
  assert.deepEqual(await secondPending, { response: JSON.parse(response), metadata: { renderRevision: 7, clientStateToken: '42' } });

  const parentRequests = [
    { subtype: 'ui_press', plugin: 'plugin-a', handle: 4, key: 'button', client_id: 'renderer-1' },
    { subtype: 'ui_input', plugin: 'plugin-a', handle: 5, kind: 'submit', value: 'typed', key: 'field' },
    { subtype: 'ui_select', plugin: 'plugin-a', handle: 6, value: 'selected', key: 'choice' },
  ] as const;
  const parentResponses = [
    { handled: true, element: 'button' },
    { handled: true, element: 'field', value: 'typed' },
    { handled: false },
  ];
  for (const [index, parentRequest] of parentRequests.entries()) {
    const pending = first.runtime.dispatchModUiControl(parentRequest);
    const parentCommand = first.commands.at(-1)!;
    assert.equal(parentCommand.type, parentRequest.subtype);
    const requestJson = JSON.parse(parentCommand.request_json);
    assert.deepEqual(requestJson, { ...parentRequest, surface: 'desktop' });
    first.client.emit('event', {
      type: 'ui_control_result', request_id: parentCommand.request_id,
      response_json: JSON.stringify(parentResponses[index]),
    });
    assert.deepEqual(await pending, { response: parentResponses[index] });
  }

  const operation = {
    type: 'mount', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1',
    plugin: 'plugin-a', client: 'client-a', module: 'main', render_revision: 7, columns: 80, rows: 24,
  } as const;
  const operationPending = first.runtime.dispatchModUiOperation(operation);
  const operationCommand = first.commands.at(-1)!;
  assert.equal(operationCommand.type, 'ui_client_operation');
  assert.deepEqual(JSON.parse(operationCommand.operation_json), operation);
  first.client.emit('event', {
    type: 'ui_control_result', request_id: operationCommand.request_id,
    response_json: JSON.stringify({ runtimeId: 'runtime-1', renderRevision: 7, frameSequence: 1, tree: { type: 'text', text: 'mount' }, hasPointerListener: true, hasKeyListener: false }),
  });
  assert.deepEqual(await operationPending, {
    runtimeId: 'runtime-1', renderRevision: 7, frameSequence: 1, tree: { type: 'text', text: 'mount' },
    hasPointerListener: true, hasKeyListener: false,
  });

  first.client.emit('event', {
    type: 'ui_client_frame', runtime_id: 'runtime-1',
    frame_json: JSON.stringify({
      renderRevision: 7,
      frameSequence: 2,
      tree: { type: 'text', text: 'frame' },
      hasPointerListener: true,
      hasKeyListener: false,
      parent: { surface: 'desktop', component: operation.component, instance_id: operation.instance_id },
      client: { plugin: operation.plugin, key: operation.client, module: operation.module },
    }),
  });
  first.client.emit('event', {
    type: 'ui_client_frame', runtime_id: 'runtime-1',
    frame_json: JSON.stringify({
      renderRevision: 7,
      fault: { phase: 'run', reason: 'asynchronous client callback failed', source: 'worker' },
    }),
  });
  first.client.emit('event', {
    type: 'ui_invalidate', session_id: `sess:${firstId}`, uuid: 'invalidate-1',
    instances_json: JSON.stringify([{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1' }]),
  });
  first.client.emit('event', { type: 'ui_invalidate', session_id: secondId, uuid: 'wrong-session' });
  const frames = sent.filter((event) => event.channel === CH_MOD_UI_FRAME);
  const invalidations = sent.filter((event) => event.channel === CH_MOD_UI_INVALIDATE);
  assert.deepEqual(frames, [{
    channel: CH_MOD_UI_FRAME,
    payload: {
      sessionId: firstId,
      event: {
        runtimeId: 'runtime-1',
        frame: {
          renderRevision: 7,
          frameSequence: 2,
          tree: { type: 'text', text: 'frame' },
          hasPointerListener: true,
          hasKeyListener: false,
          parent: { surface: 'desktop', component: operation.component, instance_id: operation.instance_id },
          client: { plugin: operation.plugin, key: operation.client, module: operation.module },
        },
      },
    },
  }, {
    channel: CH_MOD_UI_FRAME,
    payload: {
      sessionId: firstId,
      event: {
        runtimeId: 'runtime-1',
        frame: {
          renderRevision: 7,
          fault: { phase: 'run', reason: 'asynchronous client callback failed', source: 'worker' },
        },
      },
    },
  }]);
  assert.deepEqual(invalidations, [{
    channel: CH_MOD_UI_INVALIDATE,
    payload: { sessionId: firstId, event: {
      uuid: 'invalidate-1',
      instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1' }],
    } },
  }]);
  assert.equal(sent.some((event) => event.channel === CH_EVENT && (event.payload as any)?.event?.type?.startsWith('ui_')), false,
    'private UI results/frames do not enter the general sequenced event stream');
});

test('Mod UI requests beyond the outstanding window wait in order for earlier results', async () => {
  const sessionId = '33333333-4444-4555-8666-777777777777';
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
    sessionId,
    projectPath: '/workspace',
    envelopeEvents: true,
  });
  const commands: any[] = [];
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: any): void };
  client.sendCommand = (command) => { commands.push(command); };
  (runtime as any).client = client;
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).generation = 1;
  (runtime as any).wireClient(client, 1);
  // A long transcript mounts a render site per tool row at once.
  const pending = Array.from({ length: 40 }, (_, index) => runtime.dispatchModUiControl({
    subtype: 'ui_render', surface: 'desktop', component: 'ToolResult', instance_id: `tool-result:${index}`,
    props: { plugin_owned: { display: true } },
  }));
  assert.equal(commands.length, 16, 'only a bounded window of requests reaches the Bridge command lane');

  const response = JSON.stringify({ tree: { type: 'text', text: 'ok' }, props: {}, rewritten: false, hooked: true });
  const metadata = JSON.stringify({ renderRevision: 7, clientStateToken: '42' });
  client.emit('event', { type: 'ui_control_result', request_id: commands[0].request_id, response_json: response, metadata_json: metadata });
  assert.deepEqual(await pending[0], { response: JSON.parse(response), metadata: { renderRevision: 7, clientStateToken: '42' } });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(commands.length, 17);
  assert.equal(JSON.parse(commands[16].request_json).instance_id, 'tool-result:16', 'a waiting request is sent in arrival order');

  (runtime as any).clearPendingConnectionOperations();
  const settled = await Promise.allSettled(pending.slice(1));
  assert.ok(settled.every((result) => result.status === 'rejected' && /Mod UI request interrupted/.test(String(result.reason))),
    'an interrupted connection fails outstanding and waiting requests alike');
  assert.equal(commands.length, 17, 'nothing waiting is sent after the interruption');
});

test('Mod UI render accepts only the no-host null-tree fallback without metadata', async () => {
  const sessionId = '44444444-5555-4666-8777-888888888888';
  const runtime = new SessionRuntime({
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }),
    sessionId,
    projectPath: '/workspace',
    envelopeEvents: true,
  });
  const commands: any[] = [];
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: any): void };
  client.sendCommand = (command) => { commands.push(command); };
  (runtime as any).client = client;
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).activeWorkspaceTrusted = true;
  (runtime as any).generation = 1;
  (runtime as any).wireClient(client, 1);

  const request = {
    subtype: 'ui_render', surface: 'desktop', component: 'AbovePrompt', instance_id: 'no-host', props: {},
  } as const;
  const respond = (response: unknown, metadata_json?: string) => {
    const pending = runtime.dispatchModUiControl(request);
    const command = commands.at(-1)!;
    client.emit('event', {
      type: 'ui_control_result', request_id: command.request_id,
      response_json: JSON.stringify(response),
      ...(metadata_json === undefined ? {} : { metadata_json }),
    });
    return pending;
  };

  const noHostResponse = { tree: null, props: {}, rewritten: false, hooked: false };
  assert.deepEqual(await respond(noHostResponse), { response: noHostResponse });

  await assert.rejects(
    respond({ tree: { type: 'engine', ref: 1 }, props: {}, rewritten: false, hooked: false }),
    /UI render result is missing its render revision/,
  );
  await assert.rejects(
    respond(noHostResponse, JSON.stringify({ renderRevision: '7' })),
    /UI render revision/,
    'the no-host fallback does not bypass malformed metadata validation',
  );
  await assert.rejects(
    respond(noHostResponse, JSON.stringify({ clientStateToken: '42' })),
    /UI render result is missing its render revision/,
    'a metadata object without the render revision is not the no-host fallback',
  );
});

test('live and pending Side Chats prevent implicit stop, restart and archive', async () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  let stopped = 0;
  (runtime as any).stopBridge = async () => { stopped++; };
  (runtime as any).pendingSideCreates.add('create-1');
  assert.equal(runtime.hasActiveWork, true);
  await assert.rejects(runtime.stop(), /End all Side Chats/);
  await assert.rejects(runtime.restart(), /End all Side Chats/);
  (runtime as any).pendingSideCreates.clear();
  (runtime as any).liveSideChats.add('side-1');
  assert.throws(() => runtime.beginArchive(), /active work/);
  await assert.rejects(runtime.stop(), /End all Side Chats/);
  assert.equal(stopped, 0);
  (runtime as any).liveSideChats.clear();
  await runtime.stop();
  assert.equal(stopped, 1);
});

test('Side Chat envelopes bypass parent turn state and retain independent identity', () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const client = new EventEmitter();
  const events: unknown[] = [];
  (runtime as any).broadcastClientEvent = (event: unknown) => events.push(event);
  (runtime as any).wireClient(client, (runtime as any).generation);
  client.emit('event', { type: 'side_chat_created', request_id: 'create-1', side_id: 'side-1' });
  client.emit('event', { type: 'side_chat_event', side_id: 'side-1', event_json: JSON.stringify({ type: 'turn_ended' }) });
  assert.equal(runtime.turnActive, false);
  assert.equal(runtime.hasActiveWork, true);
  assert.equal(events.length, 2);
  client.emit('event', { type: 'side_chat_ended', side_id: 'side-1' });
  assert.equal(runtime.hasActiveWork, false);
});

test('effort selections reach the engine during an active turn without cancelling it', async () => {
  const { manager, commands } = trustedManager();
  (manager as any).activeTurn = true;
  const command = { type: 'set_reasoning_selection', selection: { type: 'level', id: 'high' } };
  await manager.dispatchCommand(command);
  assert.deepEqual(commands, [command]);
  assert.equal((manager as any).turnActive, true);
});

test('computer policy failure revokes immediately and stops despite active Side Chats', async () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const calls: string[] = [];
  const gate = deferred();
  (runtime as any).restartChain = gate.promise;
  (runtime as any).revokeComputerAccess = () => { calls.push('revoke'); };
  (runtime as any).stopBridge = async () => { calls.push('stop'); };
  (runtime as any).liveSideChats.add('side-1');
  (runtime as any).pendingSideCreates.add('create-1');
  const stopped = runtime.stopForComputerPolicyFailure();
  assert.deepEqual(calls, ['revoke']);
  gate.resolve();
  await stopped;
  assert.deepEqual(calls, ['revoke', 'stop']);
  assert.equal((runtime as any).state.status, 'error');
  assert.match((runtime as any).state.message, /Computer permissions/);
});

test('computer policy failure still tears down when revocation transport has failed', async () => {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  let stopped = false;
  (runtime as any).revokeComputerAccess = () => { throw new Error('transport closed'); };
  (runtime as any).stopBridge = async () => { stopped = true; };
  await runtime.stopForComputerPolicyFailure();
  assert.equal(stopped, true);
  assert.equal((runtime as any).state.status, 'error');
});

test('audio queue rejection proves no dispatch only after reserving the exact identity', async () => {
  const commands: any[] = [];
  const checked: string[] = [];
  const service = {
    getCapabilities: () => ({ service_epoch: 3, support_revision: 1, supported_operations: [], readiness: [], max_payload_bytes: 1024 }),
    initializeCapabilities: async () => {},
    executeAudioRequest: async () => { throw new Error('queue-full request must not be dispatched'); },
    rejectAudioRequestBeforeDispatch: (identity: { id: string }) => { checked.push(identity.id); return identity.id === 'fresh'; },
    cancelAudioRequest: async () => {}, endAudioOwner: async () => {}, onEvent: () => () => {},
  };
  const runtime = new SessionRuntime({ launchConfig: { workspace: '/workspace', sessionId: 'session', trusted: true },
    sessionId: 'session', projectPath: '/workspace', audioService: service } as any);
  const client = { sendCommand: (value: unknown) => commands.push(value), close: () => undefined };
  (runtime as any).client = client; (runtime as any).generation = 4;
  const active = new Map<string, unknown>();
  for (let index = 0; index < 256; index++) active.set(`3:1:active-${index}`, {});
  (runtime as any).audioRequestsByGeneration.set(4, active);
  const request = (id: string) => ({ identity: { id, generation: 1, service_epoch: 3 },
    owner: { type: 'session', session_id: 'session' }, max_payload_bytes: 1024, operation: { type: 'status' } });
  await (runtime as any).dispatchAudioRequest(client, 4, request('fresh'));
  await (runtime as any).dispatchAudioRequest(client, 4, request('previously-seen'));
  await (runtime as any).dispatchAudioRequest(client, 4, request('active-0'));
  assert.deepEqual(checked, ['fresh', 'previously-seen']);
  assert.equal(commands.length, 2);
  assert.equal(commands[0].result.type, 'failed_without_resource');
  assert.equal(commands[1].result.type, 'failed');
  active.clear(); await runtime.dispose();
});

test('account-only bridge launch writes the canonical private envelope even with no plugin secrets', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-account-envelope-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'mock-bridge');
  writeFileSync(binary, `#!${process.execPath}\nlet input='';process.stdin.setEncoding('utf8');process.stdin.on('data',chunk=>input+=chunk);process.stdin.on('end',()=>process.stdout.write(JSON.stringify({args:process.argv.slice(2),input:JSON.parse(input)})));\n`, { mode: 0o700 });
  const runtime = new SessionRuntime({ serverBin: binary, launchConfig: () => ({ workspace: directory, trusted: true }) });
  const id = '11111111-1111-4111-8111-111111111111';
  const snapshot = { storeScope: 'mock-broker-development', configRevision: 1, accounts: [{ accountId: id, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key', displayName: 'Mock account', credentialRef: { storeScope: 'mock-broker-development', secretRef: `acct.${id}` }, enabled: true, userPriority: 0, configRevision: 1, identityGeneration: 1 }], health: [], modelEntitlements: [], modelAvailability: [] };
  const credentials = { [id]: { credential_revision: 1, identity_generation: 1, kind: { kind: 'api_key', key: 'synthetic-account-key' } } };
  const child = (runtime as any).spawnServer({ workspace: directory, trusted: true, configHome: '/different/session-config', providerAccountHome: '/canonical/account-home', providerAccounts: snapshot, providerAccountCredentials: credentials }, directory);
  let output = '';
  child.stdout.setEncoding('utf8'); child.stdout.on('data', (chunk: string) => { output += chunk; });
  await new Promise<void>((resolve, reject) => { child.once('error', reject); child.once('close', (code: number) => code === 0 ? resolve() : reject(new Error(`mock bridge exited ${code}`))); });
  const result = JSON.parse(output);
  assert.ok(result.args.includes('--credential-stdin'));
  assert.equal(result.args.some((value: string) => value.includes('synthetic-account-key')), false);
  assert.deepEqual(result.input, { provider_accounts: snapshot, provider_account_credentials: credentials, provider_account_home: '/canonical/account-home', plugin_secrets: {} });
});
