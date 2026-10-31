import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import type { MediaDeviceRequestEnvelopeDto } from '@lingxi/bridge-client';
import { BridgeClient } from '@lingxi/bridge-client';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager.js';
import { SessionRuntime } from '../src/main/bridge.js';

const owner = { type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc',
  attachment_generation: 1, logical_kind: 'session', logical_owner_id: 'session-a' } as const;
function open(id: string, timeout = 200): MediaDeviceRequestEnvelopeDto {
  return { request_id: id, request: { type: 'open', request: {
    identity: { id: `op-${id}`, generation: 1, service_epoch: 10 }, owner, timeout_budget_ms: timeout,
    max_payload_bytes: 1024, request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'media-runtime:microphone' } },
  } } };
}
async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!check() && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 5));
  assert.ok(check(), 'timed out waiting for JSONL helper');
}
async function remainsPending(value: Promise<unknown>): Promise<void> {
  const result = await Promise.race([value.then(() => 'settled'), new Promise<string>((resolve) => setTimeout(() => resolve('pending'), 40))]);
  assert.equal(result, 'pending');
}

function harness() {
  const children: ChildProcessWithoutNullStreams[] = [];
  const seen: Array<{ child: ChildProcessWithoutNullStreams; request: MediaDeviceRequestEnvelopeDto }> = [];
  const manager = new NativeAudioManager({ isPackaged: false, resourcesPath: '/unused', userDataPath: '/tmp/lingxi-media-flight-test',
    helperPath: '/unused/helper', diagnostics: new DiagnosticBuffer(), spawnHelper: () => {
      const child = spawn(process.execPath, [fileURLToPath(new URL('./fixtures/native-media-attachment-helper.cjs', import.meta.url))], { stdio: 'pipe' });
      children.push(child);
      let buffer = '';
      child.stderr.on('data', (chunk: Buffer) => {
        buffer += chunk.toString('utf8');
        const lines = buffer.split('\n'); buffer = lines.pop()!;
        for (const line of lines) seen.push({ child, request: JSON.parse(line) });
      });
      return child;
    } });
  return { manager, seen, children, control(child: ChildProcessWithoutNullStreams, action: string, extra: object = {}) {
    child.stdin.write(`${JSON.stringify({ kind: 'test_control', action, ...extra })}\n`);
  }, async close() { await manager.dispose(); for (const child of children) child.kill(); } };
}

test('timed-out open joins the original helper late-open and an exact close receipt', { timeout: 5_000 }, async () => {
  const h = harness();
  try {
    const opening = h.manager.executeMediaDevice(open('held-open-1'));
    await until(() => h.seen.length === 1);
    const child = h.children[0];
    h.control(child, 'hold_stops');
    await new Promise((resolve) => setTimeout(resolve, 250));
    await remainsPending(opening);
    const draining = h.manager.drainMediaOpenFlights();
    await remainsPending(draining);
    h.control(child, 'release_open');
    await until(() => h.seen.some(({ request }) => request.request.type === 'stop'));
    await remainsPending(draining);
    for (const [index, wrong] of ['lease', 'owner', 'identity', 'open_request'].entries()) {
      if (index > 0) {
        h.control(child, 'hold_stops');
        void h.manager.drainMediaOpenFlights();
        await until(() => h.seen.filter(({ request }) => request.request.type === 'stop').length === index + 1);
      }
      h.control(child, 'release_stop', { wrong_receipt: wrong });
      await until(() => !(h.manager as any).mediaOpenFlights.get('held-open-1')?.cleanup);
      await remainsPending(draining);
    }
    const retried = h.manager.drainMediaOpenFlights();
    await Promise.all([draining, retried]);
    const result = await opening;
    assert.equal(result.result.type, 'open_failed_without_resource');
    if (result.result.type === 'open_failed_without_resource') assert.equal(result.result.error.kind, 'timeout');
    const stops = h.seen.filter(({ request }) => request.request.type === 'stop');
    assert.equal(stops.length, 5);
    assert.ok(stops.every((item) => item.child === child && (item.request.request as { lease_id: string }).lease_id === 'lease-1'));
  } finally { await h.close(); }
});

test('only explicit no-resource proof settles a failed open flight', { timeout: 5_000 }, async () => {
  const h = harness();
  try {
    const unsupported = open('unsupported');
    if (unsupported.request.type === 'open') unsupported.request.request.request = { mode: 'video', intent: 'conversation' };
    assert.equal((await h.manager.executeMediaDevice(unsupported)).result.type, 'open_failed_without_resource');
    assert.equal(h.children.length, 0, 'host preflight rejection must precede helper dispatch');
    const rejected = await h.manager.executeMediaDevice(open('preflight-fail'));
    assert.equal(rejected.result.type, 'open_failed_without_resource');
    await h.manager.drainMediaOpenFlights();
    const unknown = await h.manager.executeMediaDevice(open('unknown-fail'));
    assert.equal(unknown.result.type, 'error');
    if (unknown.result.type === 'error') assert.equal(unknown.result.error.kind, 'permission_denied');
    assert.equal((await h.manager.executeMediaDevice(open('blocked-by-unknown', 100))).result.type, 'open_failed_without_resource');
    assert.equal(h.seen.some(({ request }) => request.request_id === 'blocked-by-unknown'), false);
    for (let i = 0; i < 4_096; i++) {
      await h.manager.executeMediaDevice({ request_id: `filler-${i}`, request: { type: 'stop', lease_id: 'absent' } });
    }
    assert.equal((await h.manager.executeMediaDevice(open('unknown-fail'))).result.type, 'error');
    assert.equal(h.seen.filter(({ request }) => request.request_id === 'unknown-fail').length, 1,
      'an unresolved flight remains protected after bounded request history rolls over');
    for (let i = 0; i < 20; i++) {
      const controller = new AbortController();
      const waiting = h.manager.drainMediaOpenFlights(controller.signal);
      controller.abort();
      await assert.rejects(waiting, { name: 'AbortError' });
    }
    assert.equal((h.manager as any).mediaDrainWaiters.size, 0, 'cancelled binding lookups release their waiters');
    await remainsPending(h.manager.drainMediaOpenFlights());
  } finally { await h.close(); }
});

test('bridge rotation waits for the same timed-out JSONL open to close before reporting proof', { timeout: 5_000 }, async () => {
  const h = harness(); const commands: any[] = [];
  const runtime = new SessionRuntime({ sessionId: 'session-a', projectPath: '/workspace', audioService: h.manager as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId: 'session-a', trusted: true }) });
  const internal = runtime as any;
  const client = new BridgeClient();
  (client as any).ws = { readyState: 1, bufferedAmount: 0, send: (wire: string) => commands.push(JSON.parse(wire).payload.params) };
  internal.wireClient(client, 0);
  const deliver = (payload: unknown) => (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload })));
  const contexts = () => commands.filter((command) => command.type === 'get_audio_session_context');
  const responses = () => commands.filter((command) => command.type === 'media_device_response');
  try {
    internal.requestAudioSessionContext(client, 0);
    deliver({ type: 'audio_session_context', request_id: contexts()[0].request_id, session_id: 'session-a', media_owner: owner });
    deliver({ type: 'media_device_request', request: open('held-open-bridge') });
    await until(() => h.seen.length === 1);
    h.control(h.children[0], 'hold_stops');
    await new Promise((resolve) => setTimeout(resolve, 250));
    assert.equal(responses().length, 0, 'timeout is not a terminal no-resource proof');
    deliver({ type: 'model_changed', model: 'next' });
    const nextOwner = { ...owner, attachment_generation: 2 };
    deliver({ type: 'audio_session_context', request_id: contexts()[1].request_id, session_id: 'session-a', media_owner: nextOwner });
    const next = open('next-bridge', 1_000);
    if (next.request.type === 'open') next.request.request.owner = nextOwner;
    deliver({ type: 'media_device_request', request: next });
    const superseded = internal.nativeMediaDeviceUpdate as Promise<void>;
    await remainsPending(superseded);
    deliver({ type: 'model_changed', model: 'next-again' });
    deliver({ type: 'audio_session_context', request_id: contexts()[2].request_id, session_id: 'session-a', media_owner: nextOwner });
    await superseded; // Repeated refreshes do not accumulate behind a blocked flight.
    h.control(h.children[0], 'release_open');
    await until(() => h.seen.some(({ request }) => request.request.type === 'stop'));
    assert.equal(h.seen.filter(({ request }) => request.request.type === 'open').length, 1);
    h.control(h.children[0], 'release_stop');
    await until(() => responses().length === 2);
    assert.equal(responses().find((command) => command.response.request_id === 'held-open-bridge').response.result.type, 'open_failed_without_resource');
    assert.equal(responses().find((command) => command.response.request_id === 'next-bridge').response.result.type, 'opened');
  } finally {
    internal.nativeMediaClient = undefined;
    internal.invalidateNativeMediaBinding();
    await internal.nativeMediaDeviceUpdate;
    await h.close();
  }
});

test('losing the original helper does not treat exit as cleanup or send an old stop to its replacement', { timeout: 5_000 }, async () => {
  const h = harness();
  try {
    const opening = h.manager.executeMediaDevice(open('held-open-exit'));
    await until(() => h.seen.length === 1);
    const first = h.children[0];
    first.kill();
    assert.equal((await opening).result.type, 'error');
    const second = await h.manager.executeMediaDevice(open('replacement', 100));
    assert.equal(second.result.type, 'open_failed_without_resource');
    assert.equal(h.children.length, 2);
    await remainsPending(h.manager.drainMediaOpenFlights());
    assert.equal(h.seen.some(({ child }) => child === h.children[1]), false, 'the replacement helper receives neither a new open nor an old stop');
  } finally { await h.close(); }
});

test('a direct manager open remains reserved when a wrapper is absent after invalidation', { timeout: 5_000 }, async () => {
  const h = harness(); const sent: any[] = [];
  const runtime = new SessionRuntime({ sessionId: 'session-a', projectPath: '/workspace', audioService: h.manager as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId: 'session-a', trusted: true }) });
  const state = runtime as any;
  const client = { sendCommand: (value: any) => sent.push(value) };
  state.client = client; state.nativeMediaClient = client; state.generation = 0;
  state.requestAudioSessionContext = () => {};
  try {
    const pending = h.manager.executeMediaDevice(open('held-open-external', 1000));
    await until(() => h.seen.length === 1);
    await state.dispatchMediaDeviceRequest(client, 0, open('held-open-external'));
    assert.equal(sent.length, 0, 'an already-dispatched open cannot receive a fresh rejection proof');
    h.control(h.children[0], 'release_open');
    assert.equal((await pending).result.type, 'opened');
  } finally { await h.close(); }
});
