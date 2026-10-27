import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { test, type TestContext } from 'node:test';
import { BRIDGE_PROTOCOL_VERSION, CLIENT_PROTOCOL_VERSION } from '@lingxi/bridge-client';
import type { ClientEvent } from '@lingxi/bridge-client';
import { HostNotifier } from '../src/main/notifications.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { DEFAULT_IDLE_NOTIF_THRESHOLD_MS } from '../src/shared/notificationPreferences.js';

const require = createRequire(new URL('../../../packages/bridge-client/package.json', import.meta.url));
const { WebSocketServer } = require('ws');
const ref = { projectPath: '/host-turn-workspace', sessionId: '11111111-2222-4333-8444-555555555555' };
const authToken = '0123456789abcdef0123456789abcdef';

function fakeProcess(pid: number) {
  return Object.assign(new EventEmitter(), {
    pid, exitCode: null as number | null, signalCode: null as string | null,
    stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(),
  });
}

const cost = (totalUsd: number) => ({ total_usd: totalUsd, input_tokens: 10, output_tokens: 20, api_calls: 1, session_duration_secs: 1, formatted: `$${totalUsd}` });
const assistant = (text: string) => ({ role: 'assistant', blocks: [{ type: 'text' as const, text }] });

interface Prompt { text: string; turnId: number }

/** A session of a real runtime, connected to an engine that does what the test scripts when it is sent a prompt. */
async function fixture(t: TestContext, options: { notifier?: HostNotifier } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-host-turn-'));
  const bridgeRoot = join(directory, 'managed');
  const launchDir = join(directory, 'existing');
  mkdirSync(bridgeRoot, { mode: 0o700 });
  mkdirSync(launchDir, { mode: 0o700 });
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  await once(server, 'listening');
  const port = server.address().port;
  const lockfilePath = join(launchDir, `${port}.lock`);
  const pid = 999_902;
  writeFileSync(lockfilePath, JSON.stringify({
    pid, workspaceFolders: [ref.projectPath], ideName: 'LingXi-Bridge', transport: 'ws', runningInWindows: false, authToken,
  }), { mode: 0o600 });
  const prompts: Prompt[] = [];
  const cancels: Array<number | undefined> = [];
  let script: (prompt: Prompt, emit: (event: ClientEvent) => void) => void = () => undefined;
  let onCancel: (turnId: number | undefined, emit: (event: ClientEvent) => void) => void = () => undefined;
  const emitters = new Set<(event: ClientEvent) => void>();
  server.on('connection', (socket: any) => {
    const emit = (event: ClientEvent) => socket.send(JSON.stringify({ type: 'event', payload: event }));
    emitters.add(emit);
    socket.on('close', () => emitters.delete(emit));
    socket.on('message', (data: Buffer) => {
      const { payload } = JSON.parse(data.toString());
      if (payload.method === 'hello') {
        socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id, result: {
          protocol_version: BRIDGE_PROTOCOL_VERSION, server_name: 'fake-engine',
          capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true, supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION },
        } } }));
      } else if (payload.method === 'desktop_runtime_snapshot') {
        socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id, result: { events: [
          { type: 'session_agent_list', session_id: ref.sessionId, agents: [] },
          { type: 'coordinator_status', active_workers: 0 },
          { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: 0 },
        ] } } }));
      } else if (payload.method === 'send_prompt') {
        const prompt = { text: payload.params.text as string, turnId: payload.params.turn_id as number };
        prompts.push(prompt);
        // The bridge opens a turn it is sent a prompt for itself, with the turn id it was given.
        emit({ type: 'turn_started', turn_id: prompt.turnId });
        script(prompt, emit);
      } else if (payload.method === 'cancel') {
        cancels.push(payload.params.turn_id);
        onCancel(payload.params.turn_id, emit);
      }
    });
  });
  const manager = new SessionRuntimeManager({
    bridgeRoot,
    ...(options.notifier !== undefined ? { notifier: options.notifier } : {}),
    accessState: () => ({ workspace: ref.projectPath, trusted: true }),
    launchConfig: () => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }),
    readProcessCommand: () => undefined, listProcessCommands: () => [], lockfileTimeoutMs: 1000,
  });
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  internal.activeWorkspace = ref.projectPath;
  internal.activeWorkspaceTrusted = true;
  internal.launchDir = launchDir;
  internal.child = fakeProcess(pid);
  internal.processIsAlive = () => true;
  internal.signalChildTree = () => undefined;
  internal.spawnServer = () => { throw new Error('unexpected sidecar spawn'); };
  await internal.connectBridgeClient(lockfilePath, internal.generation, false);
  t.after(async () => {
    internal.child = null;
    await manager.dispose();
    for (const socket of server.clients) socket.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  });
  return {
    manager, runtime, internal, prompts, cancels,
    onPrompt: (handler: typeof script) => { script = handler; },
    onCancel: (handler: typeof onCancel) => { onCancel = handler; },
    broadcast: (event: ClientEvent) => { for (const emit of emitters) emit(event); },
    disconnect: async () => {
      const closed = once(internal.client, 'close');
      for (const socket of server.clients) socket.terminate();
      await closed;
    },
  };
}

/**
 * What an engine says for a turn that goes well: the text in pieces, the finished message, the cost, the end.
 * The fake engine opens the turn already, as the bridge does; one that says so a second time is no trouble.
 */
function goodTurn(text: string, totalUsd: number, announce = false) {
  return ({ turnId }: Prompt, emit: (event: ClientEvent) => void) => {
    if (announce) emit({ type: 'turn_started', turn_id: turnId });
    for (const piece of text.match(/.{1,5}/gs) ?? []) emit({ type: 'text_delta', text: piece });
    emit({ type: 'message_complete', stop_reason: 'end_turn', message: assistant(text) });
    emit({ type: 'cost_update', ...cost(totalUsd) });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: cost(totalUsd) });
  };
}

test('a host turn sends the prompt as the host and returns what the assistant ended with, and what the turn cost', async (t) => {
  const h = await fixture(t);
  h.onPrompt(goodTurn('Revenue grew by twelve percent.', 0.25));
  const result = await h.runtime.runHostTurn('Summarise the report.');
  assert.deepEqual(result, { status: 'completed', text: 'Revenue grew by twelve percent.', costUsd: 0.25 });
  assert.equal(h.prompts.length, 1);
  assert.equal(h.prompts[0]!.text, 'Summarise the report.');
  assert.ok(Number.isSafeInteger(h.prompts[0]!.turnId) && h.prompts[0]!.turnId > 0, 'a turn id of its own');
  assert.equal(h.runtime.turnActive, false, 'the session is idle again at once');
  const [entry] = h.runtime.remoteView().prompts;
  assert.equal(entry!.text, 'Summarise the report.');
  assert.equal(entry!.origin, 'scheduled', 'shown as the host\'s, not as typed in the window');
  assert.equal(entry!.turnId, h.prompts[0]!.turnId);

  h.onPrompt(goodTurn('And the second turn.', 0.4, true));
  const second = await h.runtime.runHostTurn('Again.', { source: { origin: 'device', deviceName: 'Phone' } });
  assert.ok(Math.abs(second.costUsd! - 0.15) < 1e-9, 'what the turn itself cost: the session\'s total went from 0.25 to 0.4');
  assert.equal(h.runtime.remoteView().prompts[1]!.origin, 'device');
  assert.notEqual(h.prompts[1]!.turnId, h.prompts[0]!.turnId);
});

test('the answer is the last message of the turn, whatever was said before a tool was used', async (t) => {
  const h = await fixture(t);
  h.onPrompt(({ turnId }, emit) => {
    emit({ type: 'text_delta', text: 'Let me look at the file.' });
    emit({ type: 'tool_heartbeat', id: 'tool-1', tool: 'Read', elapsed_ms: 5 });
    emit({ type: 'message_complete', stop_reason: 'tool_use', message: { role: 'assistant', blocks: [{ type: 'text', text: 'Let me look at the file.' }] } });
    emit({ type: 'text_delta', text: 'The file says ' });
    emit({ type: 'text_delta', text: 'hello.' });
    emit({ type: 'message_complete', stop_reason: 'end_turn', message: assistant('The file says hello.') });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: cost(0.1) });
  });
  assert.equal((await h.runtime.runHostTurn('Read it.')).text, 'The file says hello.');
});

test('the streamed text stands in when the engine sends no finished message, and the message wins when it does', async (t) => {
  const h = await fixture(t);
  h.onPrompt((_prompt, emit) => {
    emit({ type: 'text_delta', text: 'Only ' });
    emit({ type: 'text_delta', text: 'deltas.' });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: cost(0) });
  });
  assert.equal((await h.runtime.runHostTurn('Go.')).text, 'Only deltas.');
  h.onPrompt((_prompt, emit) => {
    emit({ type: 'text_delta', text: 'draft' });
    emit({ type: 'message_complete', stop_reason: 'end_turn', message: { role: 'assistant', blocks: [{ type: 'thinking', thinking: 'hm' }, { type: 'text', text: 'first' }, { type: 'text', text: 'second' }] } });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: cost(0) });
  });
  assert.equal((await h.runtime.runHostTurn('Go again.')).text, 'first\nsecond');
});

test('a turn the engine reports an error for fails with its message, and keeps what was said before', async (t) => {
  const h = await fixture(t);
  h.onPrompt((_prompt, emit) => {
    emit({ type: 'text_delta', text: 'Half an ans' });
    emit({ type: 'error', kind: { type: 'transport' }, message: 'The provider closed the connection.' });
    emit({ type: 'error', kind: { type: 'internal' }, message: 'A second complaint.' });
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'error', cost: cost(0.05) });
  });
  assert.deepEqual(await h.runtime.runHostTurn('Go.'), { status: 'failed', error: 'The provider closed the connection.', text: 'Half an ans', costUsd: 0.05 });
  h.onPrompt((_prompt, emit) => {
    emit({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'error', cost: cost(0.05) });
  });
  assert.equal((await h.runtime.runHostTurn('Go again.')).error, 'The turn ended with an error.');
  h.onPrompt((_prompt, emit) => {
    emit({ type: 'message_complete', stop_reason: 'max_tokens', message: assistant('Cut off in the mid') });
    emit({ type: 'turn_ended', outcome: { type: 'max_turns' }, stop_reason: 'max_tokens', cost: cost(0.05) });
  });
  const cut = await h.runtime.runHostTurn('Go a third time.');
  assert.equal(cut.status, 'failed');
  assert.match(cut.error!, /length limit/);
  assert.equal(cut.text, 'Cut off in the mid');
});

test('what is said before the turn is opened, or by a turn someone else opened, is not the answer', async (t) => {
  const h = await fixture(t);
  const sendPrompt = h.internal.sendPrompt.bind(h.internal);
  // A prompt can be a while in being prepared, as the provider credentials are loaded first, and the engine's own turns carry on meanwhile.
  h.internal.sendPrompt = (...args: unknown[]) => {
    const note = (event: ClientEvent) => h.internal.noteRemoteEvent(h.internal.eventEnvelope(event, false));
    note({ type: 'text_delta', text: 'noise from before' });
    note({ type: 'error', kind: { type: 'internal' }, message: 'an old complaint' });
    note({ type: 'turn_started', turn_id: 42 });
    note({ type: 'text_delta', text: 'noise of another turn' });
    return sendPrompt(...args);
  };
  h.onPrompt(goodTurn('the answer', 0.5));
  assert.deepEqual(await h.runtime.runHostTurn('Go.'), { status: 'completed', text: 'the answer', costUsd: 0.5 });
});

test('aborting cancels the turn in the engine, which then ends as cancelled with what had been said', async (t) => {
  const h = await fixture(t);
  h.onPrompt((_prompt, emit) => emit({ type: 'text_delta', text: 'Working on it' }));
  h.onCancel((_turnId, emit) => { emit({ type: 'turn_ended', outcome: { type: 'cancelled' }, stop_reason: 'cancelled', cost: cost(0.02) }); });
  const controller = new AbortController();
  const running = h.runtime.runHostTurn('Go.', { signal: controller.signal });
  while (h.prompts.length === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  await new Promise((resolve) => setTimeout(resolve, 20));
  controller.abort();
  assert.deepEqual(await running, { status: 'cancelled', text: 'Working on it', costUsd: 0.02 });
  assert.deepEqual(h.cancels, [h.prompts[0]!.turnId], 'it cancelled its own turn');
  assert.equal(h.runtime.turnActive, false);
});

test('a turn that does not end when cancelled is let go after the grace it was given', async (t) => {
  const h = await fixture(t);
  h.onPrompt((_prompt, emit) => emit({ type: 'text_delta', text: 'Stuck' }));
  const controller = new AbortController();
  const running = h.runtime.runHostTurn('Go.', { signal: controller.signal, cancelGraceMs: 40 });
  while (h.prompts.length === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  const before = Date.now();
  controller.abort();
  controller.abort();
  assert.deepEqual(await running, { status: 'cancelled', text: 'Stuck' });
  assert.ok(Date.now() - before >= 30, 'it waited for the engine');
  assert.deepEqual(h.cancels, [h.prompts[0]!.turnId], 'and asked once');
});

test('a signal that is aborted already sends nothing', async (t) => {
  const h = await fixture(t);
  const controller = new AbortController();
  controller.abort();
  assert.deepEqual(await h.runtime.runHostTurn('Never sent.', { signal: controller.signal }), { status: 'cancelled', text: '' });
  assert.deepEqual(h.prompts, []);
  assert.equal(h.runtime.turnActive, false);
});

test('a session that is busy is refused rather than queued behind: with a turn running, a question waiting, or the session closed', async (t) => {
  const h = await fixture(t);
  h.onPrompt(() => undefined);
  const first = h.runtime.runHostTurn('First.');
  while (h.prompts.length === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  await assert.rejects(h.runtime.runHostTurn('Second.'), /^Error: busy: The session is working on something else\.$/);
  assert.equal(h.prompts.length, 1, 'the second prompt was not sent');
  h.broadcast({ type: 'turn_ended', outcome: { type: 'end_turn' }, stop_reason: 'end_turn', cost: cost(0) });
  assert.equal((await first).status, 'completed');

  h.internal.pendingAskUserQuestionIds.add(7);
  await assert.rejects(h.runtime.runHostTurn('Third.'), /busy/);
  h.internal.pendingAskUserQuestionIds.clear();

  h.internal.child = null;
  await h.manager.closeSession(ref);
  await assert.rejects(h.runtime.runHostTurn('Fourth.'), /session was closed/);
});

test('a turn that loses its connection fails as interrupted', async (t) => {
  const h = await fixture(t);
  h.onPrompt((_prompt, emit) => emit({ type: 'text_delta', text: 'Partial' }));
  const lost = h.runtime.runHostTurn('Go.');
  const failure = assert.rejects(lost, /^Error: interrupted: The connection to the session was lost\.$/);
  while (h.prompts.length === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  await new Promise((resolve) => setTimeout(resolve, 20));
  await h.disconnect();
  await failure;
});

test('a turn whose session is closed under it fails as closed, and the session is not kept waiting for it', async (t) => {
  const h = await fixture(t);
  h.onPrompt(() => undefined);
  const running = h.runtime.runHostTurn('Go.');
  const failure = assert.rejects(running, /^Error: The session was closed\.$/);
  while (h.prompts.length === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  h.internal.child = null;
  await h.runtime.dispose();
  await failure;
});

test('a prompt the runtime refuses to send is the error of the call', async (t) => {
  const h = await fixture(t);
  await assert.rejects(h.runtime.runHostTurn(''), /prompt/i);
  await assert.rejects(h.runtime.runHostTurn('x'.repeat(2_000_000)), /prompt/i);
  assert.deepEqual(h.prompts, []);
  assert.equal(h.runtime.turnActive, false);
  h.onPrompt(goodTurn('Fine.', 0));
  assert.equal((await h.runtime.runHostTurn('Fine?')).text, 'Fine.');
});

test('the prompt is reported once the engine has it, and not when it could not be sent', async (t) => {
  const h = await fixture(t);
  h.onPrompt(goodTurn('Done.', 0.1));
  let sent = 0;
  const result = await h.runtime.runHostTurn('Go.', { onSent: () => { sent += 1; } });
  assert.equal(result.status, 'completed');
  assert.equal(sent, 1);
  await assert.rejects(h.runtime.runHostTurn('', { onSent: () => { sent += 1; } }), /prompt/i);
  const controller = new AbortController();
  controller.abort();
  await h.runtime.runHostTurn('Never sent.', { signal: controller.signal, onSent: () => { sent += 1; } });
  assert.equal(sent, 1, 'a prompt that was never sent is not reported');
});

test('a prompt that is prepared first is reported when it has been sent, and one that fails to prepare is not reported at all', async (t) => {
  const h = await fixture(t);
  h.onPrompt(goodTurn('Done.', 0.1));
  const sendPrompt = h.internal.sendPrompt.bind(h.internal);
  let release: () => void = () => undefined;
  h.internal.sendPrompt = (...args: unknown[]) => new Promise<void>((resolve) => { release = () => { sendPrompt(...args); resolve(); }; });
  const reported: string[] = [];
  const running = h.runtime.runHostTurn('Go.', { onSent: () => { reported.push('sent'); } });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual([...reported], [], 'still being prepared');
  assert.equal(h.prompts.length, 0);
  release();
  assert.equal((await running).status, 'completed');
  assert.deepEqual([...reported], ['sent']);

  h.internal.sendPrompt = () => Promise.reject(new Error('Prompt credential loading was interrupted.'));
  await assert.rejects(h.runtime.runHostTurn('Go again.', { onSent: () => { reported.push('again'); } }), /credential loading was interrupted/);
  assert.deepEqual([...reported], ['sent']);
});

test('whoever is told that the prompt was sent cannot make the turn fail', async (t) => {
  const h = await fixture(t);
  h.onPrompt(goodTurn('Done.', 0.1));
  const result = await h.runtime.runHostTurn('Go.', { onSent: () => { throw new Error('the window is gone'); } });
  assert.deepEqual(result, { status: 'completed', text: 'Done.', costUsd: 0.1 });
});

/** A notifier that is told what the runtime tells it, and whose timers the test can read. */
function watchedNotifier() {
  const timers: number[] = [];
  const shown: string[] = [];
  const notifier = new HostNotifier({
    show: (title) => { shown.push(title); },
    isWindowFocused: () => false,
    schedule: ((_fn: () => void, ms: number) => { timers.push(ms); return timers.length as unknown as ReturnType<typeof setTimeout>; }) as never,
    unschedule: (() => undefined) as never,
  });
  return { notifier, timers, shown, idleArmed: () => timers.filter((ms) => ms === DEFAULT_IDLE_NOTIF_THRESHOLD_MS).length };
}

test('a turn the host ran does not arm the idle notification, and a turn of the owner in the same session still does', async (t) => {
  const watch = watchedNotifier();
  const h = await fixture(t, { notifier: watch.notifier });
  h.onPrompt(goodTurn('Done.', 0.1));
  assert.equal((await h.runtime.runHostTurn('Do the task.')).status, 'completed');
  assert.equal(watch.idleArmed(), 0, 'nobody is waiting to come back to a task');
  assert.equal((await h.runtime.runHostTurn('Do the follow-up.')).status, 'completed');
  assert.equal(watch.idleArmed(), 0);
  // The owner types in the chat of the same session.
  h.runtime.sendPrompt('A question of mine.', [], 5);
  while (watch.idleArmed() === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  assert.equal(watch.idleArmed(), 1);
  assert.deepEqual(watch.shown, []);
});

test('a turn that the owner started after a host turn ended is not taken for the host\'s', async (t) => {
  const watch = watchedNotifier();
  const h = await fixture(t, { notifier: watch.notifier });
  h.onPrompt(goodTurn('Done.', 0.1, true));
  await h.runtime.runHostTurn('Task.');
  h.runtime.sendPrompt('Mine.', [], 7);
  while (watch.idleArmed() === 0) await new Promise((resolve) => setTimeout(resolve, 2));
  h.runtime.sendPrompt('Mine again.', [], 8);
  while (watch.idleArmed() < 2) await new Promise((resolve) => setTimeout(resolve, 2));
  assert.equal(watch.idleArmed(), 2);
});
