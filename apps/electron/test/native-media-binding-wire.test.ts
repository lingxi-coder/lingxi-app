import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { BRIDGE_PROTOCOL_VERSION, CLIENT_PROTOCOL_VERSION } from '@lingxi/bridge-client';
import type { MediaDeviceRequestEnvelopeDto } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';

const { WebSocketServer } = createRequire(new URL('../../../packages/bridge-client/package.json', import.meta.url))('ws');
const sessionId = '11111111-2222-4333-8444-555555555555';
const token = '0123456789abcdef0123456789abcdef';
const attachment = (generation: number) => ({ type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc',
  attachment_generation: generation, logical_kind: 'session', logical_owner_id: sessionId });
const open = (id: string, generation: number) => ({ request_id: id, request: { type: 'open', request: {
  identity: { id: `op-${id}`, generation: 1, service_epoch: 1 }, owner: attachment(generation), max_payload_bytes: 1024,
  request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'media-runtime:microphone' } },
} } });

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!check() && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 5));
  assert.ok(check(), 'timed out waiting for wire event');
}

test('authenticated wire contexts fence owner rotation and stale responses', { timeout: 10_000 }, async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-media-binding-'));
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  t.after(async () => {
    for (const socket of server.clients) socket.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  });
  await once(server, 'listening');
  const lockfile = join(directory, `${server.address().port}.lock`);
  writeFileSync(lockfile, JSON.stringify({ pid: process.pid, workspaceFolders: ['/workspace'], ideName: 'LingXi-Bridge',
    transport: 'ws', runningInWindows: false, authToken: token }), { mode: 0o600 });
  const commands: any[] = [];
  const sockets: any[] = [];
  server.on('connection', (socket: any, request: any) => {
    assert.equal(request.headers['x-lingxi-ide-authorization'], token);
    sockets.push(socket);
    socket.on('message', (data: Buffer) => {
      const frame = JSON.parse(data.toString());
      const { payload } = frame;
      if (payload.method === 'hello') {
        socket.send(JSON.stringify({ type: 'response', payload: { id: payload.id, result: {
          protocol_version: BRIDGE_PROTOCOL_VERSION, server_name: 'fake-bridge',
          capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true,
            supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION },
        } } }));
      } else commands.push({ socket, ...payload.params });
    });
  });
  const calls: MediaDeviceRequestEnvelopeDto[] = [];
  const seenOpenIds = new Set<string>();
  const ownerLeases = new Map<string, string>();
  const stoppedLeases = new Set<string>();
  const endRequestedOwners: ReturnType<typeof attachment>[] = [];
  const endedOwners: ReturnType<typeof attachment>[] = [];
  let stopRelease!: () => void;
  const stopGate = new Promise<void>((resolve) => { stopRelease = resolve; });
  let ownerEndRelease!: () => void;
  const ownerEndGate = new Promise<void>((resolve) => { ownerEndRelease = resolve; });
  let holdStop = true;
  const audio = {
    getCapabilities: () => undefined, getMediaSessionModes: () => ['audio'],
    drainMediaOpenFlights: async () => undefined,
    onEvent: () => () => undefined, initializeCapabilities: async () => undefined,
    hasMediaRequestId: (id: string) => seenOpenIds.has(id),
    endAudioOwner: async (owner: ReturnType<typeof attachment>) => {
      const lease = ownerLeases.get(JSON.stringify(owner));
      assert.ok(lease && stoppedLeases.has(lease), 'only end an owner after its exact media lease has stopped');
      endRequestedOwners.push(owner);
      if (owner.attachment_generation === 1) await ownerEndGate;
      endedOwners.push(owner);
    },
    executeMediaDevice: async (request: MediaDeviceRequestEnvelopeDto) => {
      calls.push(request);
      if (request.request.type === 'open') {
        assert.equal(seenOpenIds.has(request.request_id), false, 'an open identity reaches native only once');
        seenOpenIds.add(request.request_id);
        const lease = `lease-${calls.length}`;
        ownerLeases.set(JSON.stringify(request.request.request.owner), lease);
        return { request_id: request.request_id, result: { type: 'opened', lease: { lease_id: lease } } };
      }
      assert.equal(request.request.type, 'stop', 'only the exact lease can be stopped');
      if (request.request.type !== 'stop') throw new Error('unexpected native media request');
      const lease = request.request.lease_id;
      assert.ok([...ownerLeases.values()].includes(lease), 'the stop targets an opened lease');
      if (holdStop) { holdStop = false; await stopGate; }
      stoppedLeases.add(lease);
      return { request_id: request.request_id, result: { type: 'stopped' } };
    },
  };
  const runtime = new SessionRuntime({ sessionId, projectPath: '/workspace', audioService: audio as any,
    launchConfig: () => ({ workspace: '/workspace', sessionId, trusted: true }) });
  const internal = runtime as any;
  t.after(async () => {
    stopRelease();
    ownerEndRelease();
    internal.nativeMediaClient = undefined;
    internal.invalidateNativeMediaBinding();
    await internal.nativeMediaDeviceUpdate;
    internal.client?.close();
  });
  await internal.connectBridgeClient(lockfile, 0, false, false);
  const contextCommands = () => commands.filter((command) => command.type === 'get_audio_session_context');
  const responses = () => commands.filter((command) => command.type === 'media_device_response');
  const send = (socket: any, payload: unknown) => socket.send(JSON.stringify({ type: 'event', payload }));
  const context = (socket: any, requestId: string, generation: number) => send(socket, {
    type: 'audio_session_context', request_id: requestId, session_id: sessionId, media_owner: attachment(generation),
  });
  await until(() => contextCommands().length === 1);
  const firstId = contextCommands()[0].request_id;
  send(sockets[0], { type: 'media_device_request', request: open('first', 1) });
  context(sockets[0], 'stale-request-id', 1);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(calls.length, 0);
  assert.equal(responses().length, 0, 'open waits for the authenticated context response');
  context(sockets[0], firstId, 1);
  await until(() => responses().length === 1);
  assert.equal(responses()[0].response.result.type, 'opened');
  assert.deepEqual((calls[0].request as any).request.owner, attachment(1));

  send(sockets[0], { type: 'session_started', session_id: sessionId, mode: 'chat' });
  await until(() => contextCommands().length === 2 && calls.length === 2);
  send(sockets[0], { type: 'model_changed', model: 'model-a' });
  await until(() => contextCommands().length === 3);
  const staleId = contextCommands()[1].request_id;
  const latestId = contextCommands()[2].request_id;
  context(sockets[0], staleId, 1);
  context(sockets[0], latestId, 2);
  send(sockets[0], { type: 'media_device_request', request: open('second', 2) });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(calls.length, 2, 'new owner cannot open while old native stop lacks acknowledgement');
  assert.deepEqual(endRequestedOwners, [], 'the old owner cannot start ending before its lease stop');
  assert.deepEqual(endedOwners, [], 'the old owner cannot end before its lease stop');
  stopRelease();
  await until(() => endRequestedOwners.length === 1);
  assert.deepEqual(endRequestedOwners, [attachment(1)]);
  assert.deepEqual([...stoppedLeases], ['lease-1']);
  assert.equal(calls.length, 2, 'new owner cannot open while old audio teardown is unconfirmed');
  assert.equal(responses().length, 1, 'no open response precedes old owner teardown');
  ownerEndRelease();
  await until(() => responses().length === 2);
  assert.equal(responses()[1].response.result.type, 'opened');
  assert.deepEqual((calls[2].request as any).request.owner, attachment(2));
  assert.deepEqual(endedOwners, [attachment(1)]);
  assert.deepEqual([...stoppedLeases], ['lease-1']);
  send(sockets[0], { type: 'media_device_request', request: open('old-owner', 1) });
  await until(() => responses().length === 3);
  assert.equal(responses()[2].response.result.type, 'open_failed_without_resource');
  assert.equal(calls.length, 3);

  send(sockets[0], { type: 'session_resumed', session_id: sessionId, mode: 'chat', messages: [] });
  await until(() => contextCommands().length === 4);
  const oldConnectionRequest = contextCommands()[3].request_id;
  internal.generation++;
  await internal.connectBridgeClient(lockfile, internal.generation, false, false);
  await until(() => contextCommands().length === 5 && sockets.length === 2);
  const newConnectionRequest = contextCommands()[4].request_id;
  context(sockets[0], oldConnectionRequest, 1);
  context(sockets[1], newConnectionRequest, 3);
  send(sockets[1], { type: 'media_device_request', request: open('third', 3) });
  await until(() => responses().length === 4);
  assert.equal(responses()[3].response.result.type, 'opened');
  assert.deepEqual((calls.at(-1)!.request as any).request.owner, attachment(3));
  assert.deepEqual(endedOwners, [attachment(1), attachment(2)]);
  assert.deepEqual([...stoppedLeases], ['lease-1', 'lease-3']);
});
