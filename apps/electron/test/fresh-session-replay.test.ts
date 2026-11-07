import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { BridgeClient, type ClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';
import { emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import { createRuntimeEventReplayBuffer } from '../src/preload/event-replay';
import { localAppReference } from '../src/renderer/model/localAppReference';

const id = '11111111-2222-4333-8444-555555555555';
const appId = 'a' + '1'.repeat(32);
function transport(t: TestContext, options: { failHello?: boolean; wrongId?: boolean; wrongMode?: boolean; nativeResume?: boolean; badManifest?: boolean; holdManifest?: boolean } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'fresh-replay-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const clients = new Map<string, BridgeClient>();
  const commands: Array<{ type: string }> = [];
  const state = { spawns: 0, prompts: 0, hellos: 0, runtime: undefined as SessionRuntime | undefined, confirmManifest: undefined as (() => void) | undefined };
  t.mock.method(SessionRuntime.prototype as any, 'createLaunchDirectory', () => root);
  t.mock.method(SessionRuntime.prototype as any, 'waitForLockfile', async () => join(root, 'synthetic.lock'));
  t.mock.method(SessionRuntime.prototype as any, 'spawnServer', function (this: SessionRuntime) {
    state.runtime = this; state.spawns++;
    return Object.assign(new EventEmitter(), { pid: 77777777, stdout: null, stderr: null });
  });
  t.mock.method(SessionRuntime.prototype as any, 'stopBridge', async function (this: any) {
    this.client?.removeAllListeners(); this.client = undefined; this.child = undefined;
  });
  t.mock.method(BridgeClient.prototype, 'connect', async function (this: BridgeClient) {
    state.hellos++;
    if (options.failHello) throw new Error('handshake refused');
    const runtime = state.runtime!;
    clients.set(runtime.sessionId, this);
    this.sendCommand = command => {
      commands.push(command);
      if (command.type === 'get_context_manifest') {
        const emit = () => this.emit('event', { type: 'context_manifest', manifest_json: options.badManifest ? '{' : JSON.stringify({ session: {
          session_id: options.wrongId ? 'wrong-owner' : runtime.sessionId,
          mode: options.wrongMode ? 'chat' : 'code',
        } }) });
        if (options.holdManifest && (runtime as any).pendingDraftModeSnapshot) state.confirmManifest = emit;
        else queueMicrotask(emit);
      }
    };
    this.sendPrompt = () => { state.prompts++; };
    this.close = () => {};
    if (options.nativeResume) this.emit('event', { type: 'session_resumed', session_id: runtime.sessionId, mode: 'code',
      messages: [{ role: 'assistant', blocks: [{ type: 'text', text: 'Saved history' }] }] });
    return { server_name: 'synthetic/1', protocol_version: '1.0.0', capabilities: { client_protocol_version: '1.0.0' } } as any;
  });
  return { root, clients, commands, state };
}
function turn(runtime: SessionRuntime, client: BridgeClient) {
  runtime.sendPrompt('Create the app');
  const events: ClientEvent[] = [
    { type: 'turn_started', turn_id: 1 },
    { type: 'tool_use_started', id: 'bash', tool: 'Bash', input_json: '{}', header: { verb: 'generic', label: 'Bash', title: 'Bash' } },
    { type: 'tool_use_result', id: 'bash', tool: 'Bash', result_json: '"prepared"', is_error: false, display: { body_lines: 0 } },
    { type: 'tool_use_started', id: 'app', tool: 'LocalApp', input_json: '{}', header: { verb: 'generic', label: 'LocalApp', title: 'LocalApp' } },
    { type: 'tool_use_result', id: 'app', tool: 'LocalApp', result_json: JSON.stringify({ action: 'publish', id: appId, name: 'Fixture app', revision: 1 }), is_error: false, display: { body_lines: 0 } },
    { type: 'text_delta', text: 'Published the app.' },
    { type: 'message_complete' },
    { type: 'turn_ended', outcome: { type: 'end_turn' } },
  ];
  for (const event of events) client.emit('event', event);
}

test('owned fresh boot without native SessionStarted retains live tools and text for a new renderer subscriber', async t => {
  const f = transport(t);
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(() => manager.dispose());
  const ref = await manager.newSession('/fixture/source', undefined, { mode: 'code', projectId: '/fixture/source', durableDraftTitle: 'Fixture app' });
  const runtime = manager.get(ref.sessionId)!;
  turn(runtime, f.clients.get(ref.sessionId)!);
  const replay = manager.replaySnapshots.filter(event => event.sessionId === ref.sessionId);
  assert.equal(replay[0]?.event.type, 'session_started');
  let restored = emptyConversation();
  const delivered: number[] = [];
  const subscriber = createRuntimeEventReplayBuffer<ClientEvent>(envelope => {
    delivered.push(envelope.sequence);
    restored = reduceEvent(restored, envelope.event);
  });
  subscriber.push(replay.at(-1)!);
  subscriber.resolve(replay);
  assert.deepEqual(delivered, replay.map(envelope => envelope.sequence), 'a reloaded subscriber receives cached/live-racing events once');
  const tools = restored.items.filter(item => item.type === 'tool');
  assert.deepEqual(tools.map(item => item.tool), ['Bash', 'LocalApp']);
  assert.equal(localAppReference(tools[1]!)?.id, appId);
  assert.ok(restored.items.some(item => item.type === 'narration' && item.text === 'Published the app.'));
  const originalReplay = runtime.replaySnapshot();
  f.clients.get(ref.sessionId)!.emit('event', { type: 'context_manifest', manifest_json: JSON.stringify({ session: { session_id: ref.sessionId, mode: 'code' } }) });
  assert.deepEqual(runtime.replaySnapshot(), originalReplay, 'repeated manifest must not clear the transcript');
  assert.strictEqual(await manager.openSession(ref), runtime);
  assert.deepEqual(runtime.replaySnapshot(), originalReplay);
  await (runtime as any).connectBridgeClient(join(f.root, 'synthetic.lock'), (runtime as any).generation, false);
  await (runtime as any).restoreDraftSessionModeIfNeeded();
  assert.deepEqual(runtime.replaySnapshot(), originalReplay, 'same-generation reconnect must not reset history');
  assert.equal(f.state.spawns, 1);
  assert.equal(f.state.prompts, 1);
  assert.equal(f.commands.filter(command => command.type === 'new_session').length, 0);
});

for (const fault of ['failHello', 'wrongId', 'wrongMode', 'badManifest'] as const) test(`${fault} cannot publish an empty replay base`, async t => {
  const f = transport(t, { [fault]: true });
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(() => manager.dispose());
  await assert.rejects(manager.newSession('/fixture/source', undefined, { mode: 'code', projectId: '/fixture/source' }), /handshake refused|does not match|JSON|property name/);
  assert.deepEqual(f.state.runtime!.replaySnapshot(), []);
  assert.equal(f.state.prompts, 0);
});

for (const kind of ['existing-owner', 'selected-transcript', 'native-resume', 'no-draft-mode'] as const) test(`${kind} never receives a fabricated fresh base`, async t => {
  const f = transport(t, { nativeResume: kind === 'native-resume' });
  const runtime = new (SessionRuntime as any)({ sessionId: id, projectPath: '/fixture/source',
    ...(kind === 'selected-transcript' ? { resumeTranscriptPath: '/fixture/source/saved.jsonl' } : {}),
    launchConfig: () => ({ workspace: '/fixture/source', sessionId: id, trusted: true }),
  }, kind === 'no-draft-mode' ? undefined : 'code', undefined, kind !== 'existing-owner') as SessionRuntime;
  t.after(() => runtime.dispose());
  await runtime.start();
  assert.equal(runtime.replaySnapshot().some(({ event }) => event.type === 'session_started'), false);
  if (kind === 'native-resume') {
    const base = runtime.replaySnapshot()[0]!.event;
    assert.equal(base.type, 'session_resumed');
    if (base.type === 'session_resumed') assert.equal(base.messages[0]?.blocks[0]?.type, 'text');
  } else assert.deepEqual(runtime.replaySnapshot(), []);
});


test('fresh prompt admission remains closed until manifest identity is verified and replay base is published', async t => {
  const f = transport(t, { holdManifest: true });
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(() => manager.dispose());
  const starting = manager.newSession('/fixture/source', undefined, { mode: 'code', projectId: '/fixture/source' });
  for (let i = 0; i < 50 && !f.state.confirmManifest; i++) await new Promise(resolve => setImmediate(resolve));
  assert.ok(f.state.confirmManifest);
  assert.deepEqual(f.state.runtime!.replaySnapshot(), []);
  assert.throws(() => f.state.runtime!.sendPrompt('too early'), /conversation mode.*ready/);
  assert.equal(f.state.prompts, 0);
  f.state.confirmManifest();
  const ref = await starting;
  assert.equal(manager.get(ref.sessionId)!.replaySnapshot()[0]?.event.type, 'session_started');
  await manager.get(ref.sessionId)!.sendPrompt('ready');
  assert.equal(f.state.prompts, 1);
});
