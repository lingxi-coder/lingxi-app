import assert from 'node:assert/strict';
import test from 'node:test';
import { ManualClock, RemoteRpcError, type RemotePending, type RemoteSeqRecord } from '@lingxi/bridge-client/remote';
import { RemoteClient, SessionController, isTeamApproval, type SessionIo } from '../src/remote/remoteClient';
import type { ViewerConnection } from '../src/remote/viewerConnection';

const noPending: RemotePending = { permissions: [], questions: [], computer_access: [] };
const approvalId = 'team-approval:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef';

function rig(id = 's') {
  const calls: Array<{ method: string; params: unknown }> = [];
  const io: SessionIo = {
    request: (async (method: string, params: unknown) => {
      calls.push({ method, params });
      if (method === 'prompt.send') return { turn_id: 1 };
      if (method === 'image.get') return { media_type: 'image/png', base64: 'AQID' };
      return {};
    }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  };
  return { controller: new SessionController(id, io, () => true), calls };
}

function snapshot(controller: SessionController, mode?: 'chat' | 'code', epoch = 'e1') {
  const session_id = controller.sessionId;
  controller.handleSnapshot({ session_id, phase: 'begin', epoch, first_message_index: 0,
    ...(mode === undefined ? {} : { session_mode: mode }) });
  controller.handleSnapshot({ session_id, phase: 'messages', messages: [{ role: 'user', blocks: [{ type: 'text', text: 'base' }] }] });
  controller.handleSnapshot({ session_id, phase: 'end', cursor: { epoch, seq: 0 }, pending: noPending, turn: { running: false } });
}

function started(seq: number, id: string, mode: 'chat' | 'code'): RemoteSeqRecord {
  return { seq, record: { type: 'event', event: { type: 'session_started', session_id: id, mode } } };
}

test('invalid attachment batches are rejected before any prompt is queued', () => {
  const { controller, calls } = rig();
  snapshot(controller, 'code');
  const image = { media_type: 'image/png', base64: 'AAAA' };
  assert.throws(() => controller.sendPrompt('keep', [image, { ...image, media_type: 'image/bmp' }]));
  assert.throws(() => controller.sendPrompt('keep', Array(5).fill(image)));
  assert.throws(() => controller.sendPrompt('keep', [], Array(5).fill('receipt')));
  assert.deepEqual(controller.getState().outgoing, []);
  assert.deepEqual(calls, []);
});

test('successful configuration commands finish without waiting for a prompt echo', async () => {
  const { controller } = rig();
  snapshot(controller, 'code');
  for (const text of ['/model', '/model chosen', '/effort high', '/plan', '/plan open', '/plan share']) {
    await controller.sendPrompt(text);
    assert.deepEqual(controller.getState().outgoing, [], text);
  }
  await controller.sendPrompt('/plan implement this');
  assert.equal(controller.getState().outgoing.length, 1);
  controller.handleEvent({ seq: 1, record: { type: 'prompt', turn_id: 1, text: '/plan implement this', origin: 'desktop' } });
  assert.deepEqual(controller.getState().outgoing, []);
});

test('retry retains a failed image or file until the running turn finishes', async () => {
  for (const withImage of [true, false]) {
    const calls: Array<{ method: string; params: any }> = [];
    const io: SessionIo = {
      request: (async (method: string, params: unknown) => {
        calls.push({ method, params });
        if (calls.length === 1) throw new Error('not accepted');
        return { turn_id: 22 };
      }) as SessionIo['request'],
      notify() {}, clock: new ManualClock(),
    };
    const controller = new SessionController('s', io, () => true);
    snapshot(controller, 'code');
    await controller.sendPrompt('preserve', withImage ? [{ media_type: 'image/png', base64: 'AAAA' }] : [], withImage ? [] : ['receipt']);
    const failed = controller.getState().outgoing[0]!;
    assert.equal(failed.status, 'failed');
    controller.handleEvent({ seq: 1, record: { type: 'event', event: { type: 'turn_started', turn_id: 9 } } });
    await assert.rejects(controller.retryPrompt(failed.id));
    assert.deepEqual(controller.getState().outgoing[0], failed);
    assert.equal(calls.length, 1);
    snapshot(controller, 'code', 'e2');
    await controller.retryPrompt(failed.id);
    assert.equal(calls.length, 2);
    assert.equal(calls[1]!.params.client_request_id, calls[0]!.params.client_request_id);
    assert.deepEqual(calls[1]!.params.images, calls[0]!.params.images);
    assert.deepEqual(calls[1]!.params.file_refs, calls[0]!.params.file_refs);
  }
});

test('an unknown mode holds the snapshot and records until its own lifecycle confirms it', async () => {
  const { controller, calls } = rig();
  snapshot(controller);
  controller.handleEvent({ seq: 1, record: { type: 'prompt', turn_id: 1, text: 'held', origin: 'desktop' } });
  assert.equal(controller.getState().status, 'loading');
  assert.equal(controller.getState().transcript.conversation.items.length, 0);
  assert.throws(() => controller.sendPrompt('keep this', []));
  await assert.rejects(controller.fetchImage('s:m:0:0'));
  assert.deepEqual(calls, []);
  controller.handleEvent(started(2, 'other', 'code'));
  assert.equal(controller.getState().status, 'loading');
  controller.handleEvent(started(3, 's', 'chat'));
  assert.equal(controller.getState().status, 'live');
  assert.equal(controller.getState().sessionMode, 'chat');
  assert.ok(JSON.stringify(controller.getState().transcript.conversation.items).includes('held'));
  assert.equal(await controller.fetchImage('s:m:0:0'), 'data:image/png;base64,AQID');
});

test('pending updates during an unknown snapshot survive mode confirmation', () => {
  const { controller } = rig();
  const pending: RemotePending = { permissions: [{ request_id: 7, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{}', default_allow: false } }], questions: [], computer_access: [] };
  controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e1', first_message_index: 0 });
  controller.handlePending(pending);
  controller.handleSnapshot({ session_id: 's', phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending: noPending, turn: { running: false } });
  controller.handleEvent(started(1, 's', 'code'));
  assert.equal(controller.getState().status, 'live');
  assert.deepEqual(controller.getState().pending, pending);
});

test('conflicting snapshots and lifecycle records keep the committed base and reject further access', async () => {
  for (const conflictInSnapshot of [true, false]) {
    const { controller, calls } = rig();
    snapshot(controller, 'code');
    const before = controller.getState().transcript;
    if (conflictInSnapshot) {
      controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e2', first_message_index: 0, session_mode: 'chat' });
    } else {
      controller.handleEvent(started(1, 's', 'chat'));
    }
    assert.equal(controller.getState().status, 'error');
    assert.equal(controller.getState().error?.code, 'SESSION_MODE_MISMATCH');
    assert.deepEqual(controller.getState().transcript, before);
    assert.throws(() => controller.sendPrompt('preserve draft'));
    await assert.rejects(controller.retryPrompt('x'));
    await assert.rejects(controller.cancelTurn());
    await assert.rejects(controller.fetchImage('x'));
    snapshot(controller, 'code', 'e3');
    assert.equal(controller.getState().status, 'error');
    assert.deepEqual(calls, []);
  }
});

test('a conflicting held lifecycle rejects the entire replacement snapshot', () => {
  const { controller } = rig();
  snapshot(controller, 'code');
  const before = controller.getState().transcript;
  controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e2', first_message_index: 0, session_mode: 'code' });
  controller.handleSnapshot({ session_id: 's', phase: 'messages', messages: [{ role: 'user', blocks: [{ type: 'text', text: 'wrong replacement' }] }] });
  controller.handleEvent({ seq: 1, record: { type: 'prompt', turn_id: 1, text: 'held', origin: 'desktop' } });
  controller.handleEvent(started(2, 's', 'chat'));
  controller.handleSnapshot({ session_id: 's', phase: 'end', cursor: { epoch: 'e2', seq: 0 }, pending: noPending, turn: { running: false } });
  assert.equal(controller.getState().status, 'error');
  assert.deepEqual(controller.getState().transcript, before);
});

test('a late image cannot escape or enter the cache after a mode conflict', async () => {
  let resolve!: (value: unknown) => void;
  let requests = 0;
  const request = (): Promise<unknown> => { requests += 1; return new Promise((done) => { resolve = done; }); };
  const io: SessionIo = { request: request as SessionIo['request'],
    notify() {}, clock: new ManualClock() };
  const controller = new SessionController('s', io, () => true);
  snapshot(controller, 'code');
  const pending = controller.fetchImage('s:m:0:0');
  controller.handleEvent(started(1, 's', 'chat'));
  resolve({ media_type: 'image/png', base64: 'AQID' });
  await assert.rejects(pending);
  await assert.rejects(controller.fetchImage('s:m:0:0'));
  assert.equal(requests, 1);
});

test('a confirmed session cannot fetch another session\'s image', async () => {
  const { controller, calls } = rig();
  snapshot(controller, 'code');
  await assert.rejects(controller.fetchImage('other:m:0:0'));
  await assert.rejects(controller.fetchImage('s-extra:m:0:0'));
  assert.equal(calls.length, 0);
  assert.equal(await controller.fetchImage('s:m:0:0'), 'data:image/png;base64,AQID');
  assert.deepEqual(calls.map(call => call.method), ['image.get']);
});

test('unknown snapshot pages and later events have a bounded sticky wait', () => {
  for (const oversized of [false, true]) {
    const { controller } = rig();
    if (oversized) {
      controller.handleSnapshot({ session_id: 's', phase: 'begin', epoch: 'e1', first_message_index: 0 });
      controller.handleSnapshot({ session_id: 's', phase: 'messages', messages: [{ role: 'user', blocks: [{ type: 'text', text: 'x'.repeat(4 * 1024 * 1024) }] }] });
    } else {
      snapshot(controller);
      for (let seq = 1; seq <= 129; seq += 1) controller.handleEvent({ seq, record: { type: 'prompt', turn_id: seq, text: 'x', origin: 'desktop' } });
    }
    assert.equal(controller.getState().status, 'error');
    assert.equal(controller.getState().error?.code, 'SESSION_MODE_OVERFLOW');
    controller.handleEvent(started(200, 's', 'code'));
    assert.equal(controller.getState().status, 'error');
  }
});

test('Team capability admits only a pending allow-once or deny and has no conversation mode', async () => {
  assert.equal(isTeamApproval(approvalId), true);
  assert.equal(isTeamApproval('team-approval:short'), false);
  assert.equal(isTeamApproval(`${approvalId}\n`), false);
  assert.equal(isTeamApproval(`${approvalId}-`), false);
  const { controller, calls } = rig(approvalId);
  const pending: RemotePending = { permissions: [{ request_id: 1, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{}', default_allow: false } }], questions: [], computer_access: [] };
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
  controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending, turn: { running: false } });
  assert.equal(controller.getState().status, 'live');
  assert.equal(controller.getState().sessionMode, undefined);
  await controller.respondPermission(1, { type: 'allow_once' });
  await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e2', first_message_index: 0 });
  controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e2', seq: 0 }, pending, turn: { running: false } });
  assert.deepEqual(controller.getState().pending.permissions, []);
  await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
  await assert.rejects(controller.respondPermission(2, { type: 'allow_once' }));
  assert.throws(() => controller.sendPrompt('forbidden'));
  await assert.rejects(controller.cancelTurn());
  await assert.rejects(controller.answerQuestion(1, {}));
  await assert.rejects(controller.setModel('x'));
  await assert.rejects(controller.fetchImage('x'));
  assert.deepEqual(calls.map((call) => call.method), ['permission.respond']);
  controller.handleEvent(started(1, approvalId, 'code'));
  assert.equal(controller.getState().status, 'error');
  await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
});

test('a Team approval reserves one in-flight response and permits a retry after failure', async () => {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  let calls = 0;
  const io: SessionIo = {
    request: (() => {
      calls += 1;
      return new Promise<void>((ok, fail) => { resolve = ok; reject = fail; });
    }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  };
  const controller = new SessionController(approvalId, io, () => true);
  const pending: RemotePending = { permissions: [{ request_id: 1, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{}', default_allow: false } }], questions: [], computer_access: [] };
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
  controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending, turn: { running: false } });
  const first = controller.respondPermission(1, { type: 'allow_once' });
  await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
  assert.equal(calls, 1);
  reject(new RemoteRpcError('capacity', 'host refused the request'));
  await assert.rejects(first);
  const retry = controller.respondPermission(1, { type: 'deny' });
  assert.equal(calls, 2);
  resolve();
  await retry;
  assert.deepEqual(controller.getState().pending.permissions, []);
  controller.handlePending(pending);
  assert.deepEqual(controller.getState().pending.permissions, []);
  await assert.rejects(controller.respondPermission(1, { type: 'allow_once' }));
});

test('an uncertain Team approval result cannot be answered with the opposite decision', async () => {
  for (const error of [new RemoteRpcError('timeout', 'reply lost'), new RemoteRpcError('closed', 'link lost'), new RemoteRpcError('internal', 'unknown result'), new Error('unexpected transport failure')]) {
    let calls = 0;
    const io: SessionIo = {
      request: (async () => { calls += 1; throw error; }) as SessionIo['request'],
      notify() {}, clock: new ManualClock(),
    };
    const controller = new SessionController(approvalId, io, () => true);
    const pending: RemotePending = { permissions: [{ request_id: 1, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{}', default_allow: false } }], questions: [], computer_access: [] };
    controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
    controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending, turn: { running: false } });
    await assert.rejects(controller.respondPermission(1, { type: 'allow_once' }));
    controller.handlePending(pending);
    assert.deepEqual(controller.getState().pending.permissions, []);
    await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
    assert.equal(calls, 1);
  }
});

test('a Team approval answered elsewhere cannot be resurrected by a stale pending update', async () => {
  let calls = 0;
  const io: SessionIo = {
    request: (async () => { calls += 1; throw new RemoteRpcError('not_pending', 'already answered'); }) as SessionIo['request'],
    notify() {}, clock: new ManualClock(),
  };
  const controller = new SessionController(approvalId, io, () => true);
  const pending: RemotePending = { permissions: [{ request_id: 1, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{}', default_allow: false } }], questions: [], computer_access: [] };
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
  controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e1', seq: 0 }, pending, turn: { running: false } });
  await controller.respondPermission(1, { type: 'deny' });
  assert.equal(controller.getState().answeredElsewhere, true);
  controller.handlePending(pending);
  assert.deepEqual(controller.getState().pending.permissions, []);
  await assert.rejects(controller.respondPermission(1, { type: 'allow_once' }));
  assert.equal(calls, 1);
});

test('Team capability rejects a history pagination marker', async () => {
  const { controller, calls } = rig(approvalId);
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 1 });
  assert.equal(controller.getState().status, 'error');
  assert.equal(controller.getState().hasOlder, false);
  await assert.rejects(controller.respondPermission(1, { type: 'deny' }));
  assert.deepEqual(calls, []);
});

test('Team capability rejects snapshot content as soon as a page arrives', () => {
  for (const content of ['messages', 'records'] as const) {
    const { controller } = rig(approvalId);
    controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
    if (content === 'messages') {
      controller.handleSnapshot({ session_id: approvalId, phase: 'messages', messages: [{ role: 'user', blocks: [{ type: 'text', text: 'forbidden' }] }] });
    } else {
      controller.handleSnapshot({ session_id: approvalId, phase: 'records', records: [{ seq: 1, record: { type: 'prompt', turn_id: 1, text: 'forbidden', origin: 'desktop' } }] });
    }
    assert.equal(controller.getState().status, 'error');
    assert.equal(controller.getState().transcript.conversation.items.length, 0);
  }
});

test('Team capability rejects a conversation cursor', () => {
  const { controller } = rig(approvalId);
  controller.handleSnapshot({ session_id: approvalId, phase: 'begin', epoch: 'e1', first_message_index: 0 });
  controller.handleSnapshot({ session_id: approvalId, phase: 'end', cursor: { epoch: 'e1', seq: 1 }, pending: noPending, turn: { running: false } });
  assert.equal(controller.getState().status, 'error');
});

test('file operations require a live confirmed conversation and never use Team capabilities', async () => {
  const calls: string[] = [];
  const connection = {
    state: { phase: 'ready' }, onNotification: () => () => {}, onReady: () => () => {}, subscribe: () => () => {},
    notify: () => {},
    request: async (method: string) => { calls.push(method); return { files: [] }; },
  } as unknown as ViewerConnection;
  const client = new RemoteClient(connection, { clock: new ManualClock() });
  const file = { transfer_id: 'ab'.repeat(16), ref: 'ab'.repeat(16), session_id: approvalId, name: 'x', size: 0, sha256: '0'.repeat(64) };
  await assert.rejects(client.listFiles('s'));
  await assert.rejects(client.listFiles(approvalId));
  await assert.rejects(client.uploadFile(approvalId, new File([], 'x')));
  await assert.rejects(client.downloadFile(file));
  snapshot(client.session('s'), 'code');
  assert.deepEqual(await client.listFiles('s'), []);
  assert.deepEqual(calls, ['file.list']);
  client.dispose();
});
