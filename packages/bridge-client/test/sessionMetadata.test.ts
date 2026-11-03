import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { test, type TestContext } from 'node:test';
import WebSocket, { WebSocketServer } from 'ws';

import { BridgeClient } from '../src/client.js';
import { BRIDGE_PROTOCOL_VERSION, CLIENT_PROTOCOL_VERSION } from '../src/protocol.js';

const source = {
  sessionId: '11111111-2222-4333-8444-555555555555',
  transcriptPath: '/source/空 格/selected.jsonl',
  sessionMode: 'code' as const,
};
const token = '0123456789abcdef0123456789abcdef';

type WireRequest = {
  socket: WebSocket;
  connection: number;
  id: number;
  method: string;
  params: unknown;
};

async function wireFixture(t: TestContext) {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-session-metadata-wire-'));
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  const sockets: WebSocket[] = [];
  const requests: WireRequest[] = [];
  const queue: WireRequest[] = [];
  const heldHelloConnections = new Set<number>();
  const helloQueue: WireRequest[] = [];
  const available = new EventEmitter();
  t.after(async () => {
    for (const socket of sockets) socket.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  });
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const lockfilePath = join(directory, `${address.port}.lock`);
  writeFileSync(lockfilePath, JSON.stringify({ pid: process.pid, workspaceFolders: ['/workspace'],
    ideName: 'LingXi-Bridge', transport: 'ws', runningInWindows: false, authToken: token }), { mode: 0o600 });
  server.on('connection', (socket, request) => {
    assert.equal(request.url, '/mcp');
    assert.equal(request.headers['x-lingxi-ide-authorization'], token);
    const connection = sockets.push(socket) - 1;
    socket.on('message', (data) => {
      const frame = JSON.parse(data.toString());
      assert.equal(frame.type, 'request');
      const { id, method, params } = frame.payload;
      if (method === 'hello') {
        if (heldHelloConnections.has(connection)) {
          helloQueue.push({ socket, connection, id, method, params });
          available.emit('hello');
          return;
        }
        socket.send(JSON.stringify({ type: 'response', payload: { id, result: {
          protocol_version: BRIDGE_PROTOCOL_VERSION,
          server_name: 'metadata-fixture',
          capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true,
            supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION },
        } } }));
        return;
      }
      const item = { socket, connection, id, method, params };
      requests.push(item);
      queue.push(item);
      available.emit('request');
    });
  });
  const client = new BridgeClient({ lockfilePath });
  client.on('error', (error) => { throw error; });
  await client.connect();
  const next = async (): Promise<WireRequest> => {
    while (queue.length === 0) await once(available, 'request');
    return queue.shift()!;
  };
  const nextHeldHello = async (): Promise<WireRequest> => {
    while (helloQueue.length === 0) await once(available, 'hello');
    return helloQueue.shift()!;
  };
  const reply = (request: WireRequest, result: unknown) => request.socket.send(JSON.stringify({
    type: 'response', payload: { id: request.id, result },
  }));
  return { client, sockets, requests, heldHelloConnections, next, nextHeldHello, reply };
}

function captureReceipt(request: WireRequest): string {
  const params = request.params as { action: string; receipt: string };
  assert.equal(params.action, 'capture');
  assert.match(params.receipt, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  assert.deepEqual(params, { action: 'capture', ...source, receipt: params.receipt });
  return params.receipt;
}

async function capture(fixture: Awaited<ReturnType<typeof wireFixture>>) {
  const pending = fixture.client.captureSessionMetadataOwner(source);
  const request = await fixture.next();
  assert.equal(request.method, 'session_metadata');
  const receipt = captureReceipt(request);
  fixture.reply(request, { receipt });
  const owner = await pending;
  return { request, receipt, owner, mutate: owner.apply };
}

async function outcomeWithin<T>(promise: Promise<T>): Promise<{ error?: Error; value?: T; timeout?: true }> {
  return Promise.race([
    promise.then((value) => ({ value }), (error: Error) => ({ error })),
    delay(500).then(() => ({ timeout: true as const })),
  ]);
}

test('metadata capture and mutations keep their reserved receipt on the authenticated owner socket', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  const { request: captured, receipt, mutate } = await capture(fixture);
  for (const [action, title] of [['rename', '标题 "A"\nB'], ['touch', undefined], ['verify', undefined]] as const) {
    const pending = mutate(action, title);
    const request = await fixture.next();
    assert.equal(request.method, 'session_metadata');
    assert.equal(request.socket, captured.socket);
    assert.deepEqual(request.params, { action, receipt, ...(title === undefined ? {} : { title }) });
    fixture.reply(request, { ok: true });
    await pending;
  }
  assert.equal(fixture.requests.length, 4);
});

test('metadata rejects malformed capture and mutation confirmations', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  for (const result of [null, [], {}, { receipt: '' }, { receipt: 1 }, { receipt: '22222222-2222-4222-8222-222222222222' }]) {
    const pending = fixture.client.captureSessionMetadataOwner(source);
    const rejected = assert.rejects(pending, /Invalid session metadata capture response/);
    const request = await fixture.next();
    const receipt = captureReceipt(request);
    fixture.reply(request, result);
    await rejected;
    const release = await fixture.next();
    assert.deepEqual(release.params, { action: 'release', receipt });
    fixture.reply(release, { ok: true });
  }
  const { mutate } = await capture(fixture);
  for (const result of [null, [], {}, { ok: false }, { ok: 'true' }]) {
    const rejected = assert.rejects(mutate('verify'), /Invalid session metadata response/);
    fixture.reply(await fixture.next(), result);
    await rejected;
  }
});

test('metadata release is socket-bound, idempotent and retryable after an unconfirmed response', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  const { request: captured, receipt, owner } = await capture(fixture);
  const mutation = owner.apply('touch');
  const mutationRequest = await fixture.next();
  const release = owner.release();
  assert.equal(owner.release(), release, 'concurrent release calls share one request');
  await assert.rejects(owner.apply('verify'), /released/);
  const releaseRequest = await fixture.next();
  assert.equal(releaseRequest.socket, captured.socket);
  assert.deepEqual(releaseRequest.params, { action: 'release', receipt });
  fixture.reply(mutationRequest, { ok: true });
  await mutation;
  fixture.reply(releaseRequest, { ok: false });
  await assert.rejects(release, /Invalid session metadata release response/);
  const retry = owner.release();
  const retryRequest = await fixture.next();
  assert.deepEqual(retryRequest.params, { action: 'release', receipt });
  fixture.reply(retryRequest, { ok: true });
  await retry;
  await owner.release();
  assert.equal(fixture.requests.length, 4, 'confirmed release is never sent twice');
});

test('a timed-out capture releases its reserved token before a late response', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const pending = fixture.client.captureSessionMetadataOwner(source);
  const rejected = assert.rejects(pending, /timed out/);
  const request = await fixture.next();
  const receipt = captureReceipt(request);
  t.mock.timers.tick(30_001);
  await rejected;
  const release = await fixture.next();
  assert.equal(release.socket, request.socket);
  assert.deepEqual(release.params, { action: 'release', receipt });
  fixture.reply(release, { ok: true });
  fixture.reply(request, { receipt });
  // The next capture's acknowledgement is a same-socket FIFO sentinel: both
  // preceding responses have been consumed when it resolves.
  const next = await capture(fixture);
  assert.notEqual(next.receipt, receipt);
  assert.equal((fixture.client as any).pendingResponses.size, 0, 'the late capture cannot revive its rejected request');
});

test('an admitted metadata write waits past the ordinary RPC deadline for its actual response', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  const { owner } = await capture(fixture);
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let settled = false;
  const mutation = owner.apply('rename', 'title').finally(() => { settled = true; });
  const request = await fixture.next();
  t.mock.timers.tick(120_000);
  await Promise.resolve();
  assert.equal(settled, false, 'a timer is not evidence that the writer drained');
  fixture.reply(request, { ok: true });
  await mutation;
  assert.equal(settled, true);
});

test('a replaced socket never receives the previous metadata receipt release', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  const { owner } = await capture(fixture);
  await fixture.client.connect();
  await owner.release();
  const replacement = await capture(fixture);
  assert.equal(replacement.request.connection, 1);
  assert.deepEqual(fixture.requests.filter(item => item.connection === 1).map(item => item.params), [
    { action: 'capture', ...source, receipt: replacement.receipt },
  ]);
});

for (const stage of ['capture', 'mutation'] as const) {
  test(`disconnect rejects an in-flight metadata ${stage} and never replays it on reconnect`, { timeout: 5_000 }, async (t) => {
    const fixture = await wireFixture(t);
    const owner = stage === 'mutation' ? await capture(fixture) : undefined;
    const pending = owner ? owner.mutate('rename', 'new title') : fixture.client.captureSessionMetadataOwner(source);
    const rejected = assert.rejects(pending, /connection closed/i);
    const request = await fixture.next();
    assert.equal(request.connection, 0);
    request.socket.terminate();
    await rejected;
    await fixture.client.connect();
    if (owner) await assert.rejects(owner.mutate('touch'), /connection changed/i);
    const nextOwner = await capture(fixture);
    assert.equal(nextOwner.request.connection, 1);
    await delay(20);
    assert.deepEqual(fixture.requests.filter((item) => item.connection === 1).map((item) => item.params), [
      { action: 'capture', ...source, receipt: nextOwner.receipt },
    ]);
  });

  test(`replacing the socket discards an old metadata ${stage} result without replay`, { timeout: 5_000 }, async (t) => {
    const fixture = await wireFixture(t);
    const owner = stage === 'mutation' ? await capture(fixture) : undefined;
    const pending = owner ? owner.mutate('touch') : fixture.client.captureSessionMetadataOwner(source);
    let oldSettled = false;
    const oldOutcome = outcomeWithin<unknown>(pending.finally(() => { oldSettled = true; }));
    const oldRequest = await fixture.next();
    await fixture.client.connect();
    const newPending = fixture.client.captureSessionMetadataOwner(source);
    let newSettled = false;
    const newOutcome = outcomeWithin(newPending.finally(() => { newSettled = true; }));
    const newRequest = await fixture.next();
    assert.equal(newRequest.connection, 1);
    const oldResult = stage === 'capture' ? { receipt: captureReceipt(oldRequest) } : { ok: true };
    fixture.reply(oldRequest, oldResult);
    // Even if the replacement sends the old correlation id, it cannot
    // complete a request that belonged to the previous socket.
    fixture.reply({ ...oldRequest, socket: newRequest.socket }, oldResult);
    await delay(20);
    assert.equal(oldSettled, false, 'an obsolete socket response must not acknowledge the old operation');
    assert.equal(newSettled, false, 'an obsolete socket response must not settle the new request');
    oldRequest.socket.terminate();
    assert.ok((await oldOutcome).error, 'the old operation must reject when its socket closes');
    if (owner) await assert.rejects(owner.mutate('touch'), /connection changed/i);
    const newReceipt = captureReceipt(newRequest);
    fixture.reply(newRequest, { receipt: newReceipt });
    const current = await newOutcome;
    assert.equal(current.error, undefined);
    assert.equal(current.timeout, undefined);
    const verified = current.value!.apply('verify');
    const verifyRequest = await fixture.next();
    assert.equal(verifyRequest.connection, 1);
    assert.deepEqual(verifyRequest.params, { action: 'verify', receipt: newReceipt });
    fixture.reply(verifyRequest, { ok: true });
    await verified;
    assert.deepEqual(fixture.requests.filter((item) => item.connection === 1).map((item) => item.params), [
      { action: 'capture', ...source, receipt: newReceipt }, { action: 'verify', receipt: newReceipt },
    ]);
  });
}

test('closing a replaced socket does not reject a new owner\'s pending request', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  // Delay the server half of the original close handshake while the public
  // client reconnects and starts a new request.
  const oldPending = outcomeWithin(fixture.client.captureSessionMetadataOwner(source));
  assert.equal((await fixture.next()).connection, 0);
  fixture.sockets[0]!.pause();
  fixture.client.close();
  await fixture.client.connect();
  const pending = fixture.client.captureSessionMetadataOwner(source);
  const result = outcomeWithin(pending);
  const newRequest = await fixture.next();
  assert.equal(newRequest.connection, 1);
  const oldClosed = once(fixture.sockets[0]!, 'close');
  fixture.sockets[0]!.resume();
  await oldClosed;
  await delay(20);
  const oldOutcome = await oldPending;
  assert.ok(oldOutcome.error, 'the old connection request must reject');
  fixture.reply(newRequest, { receipt: captureReceipt(newRequest) });
  const outcome = await result;
  assert.equal(outcome.error, undefined, 'an old close must not reject a new connection request');
  assert.equal(outcome.timeout, undefined);
  const mutate = outcome.value!.apply;
  const verified = mutate('verify');
  const verifyRequest = await fixture.next();
  assert.equal(verifyRequest.connection, 1);
  fixture.reply(verifyRequest, { ok: true });
  await verified;
});

function mediaRequestId(request: WireRequest): string {
  assert.equal(request.method, 'media_service_request');
  const params = request.params as { type: string; request: { request_id: string } };
  assert.equal(params.type, 'media_service_request');
  return params.request.request_id;
}

function sendMediaSources(socket: WebSocket, requestId: string) {
  socket.send(JSON.stringify({ type: 'event', payload: { type: 'media_service_response', response: {
    request_id: requestId,
    result: { type: 'operation', operation_id: { id: 'operation-1', generation: 1, service_epoch: 2 },
      incarnation: '00000000-0000-4000-8000-000000000001', result: { type: 'sources', sources: [] } },
    error: null,
  } } }));
}

test('a new socket cannot satisfy old media work and an old close cannot reject new media work', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  let oldSettled = false, newSettled = false;
  const oldOutcome = outcomeWithin(fixture.client.executeMedia({ type: 'sources' }).finally(() => { oldSettled = true; }));
  const oldRequest = await fixture.next();
  const oldId = mediaRequestId(oldRequest);
  oldRequest.socket.pause();
  fixture.client.close();
  await fixture.client.connect();
  const newOutcome = outcomeWithin(fixture.client.executeMedia({ type: 'sources' }).finally(() => { newSettled = true; }));
  const newRequest = await fixture.next();
  const newId = mediaRequestId(newRequest);
  assert.equal(newRequest.connection, 1);
  assert.notEqual(newId, oldId);
  // A response with the obsolete request id arriving on the replacement is
  // not evidence that the original owner completed the operation.
  sendMediaSources(newRequest.socket, oldId);
  await delay(20);
  if (oldSettled) assert.ok((await oldOutcome).error, 'old work may reject early, but may not be acknowledged by the new socket');
  assert.equal(newSettled, false);
  const oldClosed = once(oldRequest.socket, 'close');
  oldRequest.socket.resume();
  await oldClosed;
  const oldResult = await oldOutcome;
  assert.ok(oldResult.error, 'the old media request must reject when its owner closes');
  assert.equal(newSettled, false, 'the old close must not reject the new request');
  sendMediaSources(newRequest.socket, newId);
  const newResult = await newOutcome;
  assert.equal(newResult.error, undefined);
  assert.equal(newResult.timeout, undefined);
  assert.deepEqual(newResult.value?.result, { type: 'sources', sources: [] });
});

test('an older failed public connect cannot close a newer connected socket', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  fixture.heldHelloConnections.add(1);
  const oldOutcome = outcomeWithin(fixture.client.connect());
  const oldHello = await fixture.nextHeldHello();
  assert.equal(oldHello.connection, 1);
  await fixture.client.connect();
  const newOutcome = outcomeWithin(fixture.client.captureSessionMetadataOwner(source));
  const newRequest = await fixture.next();
  assert.equal(newRequest.connection, 2);
  oldHello.socket.terminate();
  assert.ok((await oldOutcome).error, 'the failed old connect must reject');
  fixture.reply(newRequest, { receipt: captureReceipt(newRequest) });
  const result = await newOutcome;
  assert.equal(result.error, undefined, 'the failed old connect must not close its replacement');
  assert.equal(result.timeout, undefined);
  const verified = result.value!.apply('verify');
  const verifyRequest = await fixture.next();
  assert.equal(verifyRequest.connection, 2);
  fixture.reply(verifyRequest, { ok: true });
  await verified;
});

test('a replacement socket cannot apply an old runtime snapshot response', { timeout: 5_000 }, async (t) => {
  const fixture = await wireFixture(t);
  const applied: string[] = [];
  const snapshot = { events: [
    { type: 'session_agent_list', session_id: 'session-a', agents: [] },
    { type: 'coordinator_status', active_workers: 0 },
    { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: 0 },
  ] };
  const oldOutcome = outcomeWithin(fixture.client.requestRuntimeSnapshot(() => { applied.push('old'); }));
  const oldRequest = await fixture.next();
  assert.equal(oldRequest.method, 'desktop_runtime_snapshot');
  await fixture.client.connect();
  const newOutcome = outcomeWithin(fixture.client.requestRuntimeSnapshot(() => { applied.push('new'); }));
  const newRequest = await fixture.next();
  assert.equal(newRequest.connection, 1);
  fixture.reply({ ...oldRequest, socket: newRequest.socket }, snapshot);
  await delay(20);
  assert.deepEqual(applied, [], 'a response from the wrong socket must not enter the accept callback');
  oldRequest.socket.terminate();
  assert.ok((await oldOutcome).error);
  assert.deepEqual(applied, []);
  fixture.reply(newRequest, snapshot);
  const current = await newOutcome;
  assert.equal(current.error, undefined);
  assert.deepEqual(current.value, snapshot.events);
  assert.deepEqual(applied, ['new']);
});
