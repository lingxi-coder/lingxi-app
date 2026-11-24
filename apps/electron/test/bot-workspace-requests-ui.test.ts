import assert from 'node:assert/strict';
import test from 'node:test';

import type { BotUi, BotWorkspaceRequestUi, BotWorkspaceRequestsChangedUi, BotsApi } from '../src/shared/bots';
import { BotWorkspaceRequestsController, requestsOfBot } from '../src/renderer/bridge/botWorkspaceRequests';

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: '', createdAt: 1, updatedAt: 1, revision: 1, workspaces: [], sandbox: { enabled: true, allowedDomains: [] }, ...overrides };
}

function request(requestId: string, botId: string, path = '/work/a'): BotWorkspaceRequestUi {
  return { requestId, botId, botName: botId, path, mode: 'read', reason: 'to read the notes', createdAt: 1 };
}

/** The host's side of the requests: what is waiting, what was answered, and the way to tell the window that it changed. */
function fakeApi(initial: BotWorkspaceRequestUi[], options: { decide?: (botId: string, requestId: string, decision: 'approve' | 'deny') => Promise<BotUi> } = {}) {
  const state = { waiting: [...initial], listed: 0, decided: [] as Array<[string, string, string]>, listeners: new Set<(change: BotWorkspaceRequestsChangedUi) => void>(), failList: undefined as Error | undefined };
  const api: BotsApi['workspaceRequests'] = {
    async list() {
      state.listed++;
      if (state.failList) throw state.failList;
      return [...state.waiting];
    },
    async decide(botId, requestId, decision) {
      state.decided.push([botId, requestId, decision]);
      const result = options.decide ? await options.decide(botId, requestId, decision) : bot({ botId, revision: 2 });
      state.waiting = state.waiting.filter((entry) => entry.requestId !== requestId);
      return result;
    },
    onChanged(callback) { state.listeners.add(callback); return () => { state.listeners.delete(callback); }; },
  };
  return { api, state };
}

const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

test('the requests are read once, of every Bot, and a Bot\'s own are the ones named after it', async () => {
  const { api, state } = fakeApi([request('r1', 'b1'), request('r2', 'b2'), request('r3', 'b1', '/work/b')]);
  const controller = new BotWorkspaceRequestsController(api, () => undefined);
  assert.equal(controller.getSnapshot().requests, undefined, 'not known until read');
  await controller.load();
  assert.equal(state.listed, 1);
  assert.deepEqual(requestsOfBot(controller.getSnapshot(), 'b1').map((entry) => entry.requestId), ['r1', 'r3']);
  assert.deepEqual(requestsOfBot(controller.getSnapshot(), 'b2').map((entry) => entry.requestId), ['r2']);
  assert.deepEqual(requestsOfBot(controller.getSnapshot(), 'nobody'), []);
});

test('a list that cannot be read says why and keeps what was known', async () => {
  const { api, state } = fakeApi([request('r1', 'b1')]);
  const controller = new BotWorkspaceRequestsController(api, () => undefined);
  await controller.load();
  state.failList = new Error("Error invoking remote method 'lingxi:bots:workspace-requests:list': Error: The requests are not available.");
  await controller.load();
  assert.equal(controller.getSnapshot().error, 'The requests are not available.');
  assert.equal(controller.getSnapshot().requests?.length, 1);
  state.failList = undefined;
  await controller.load();
  assert.equal(controller.getSnapshot().error, undefined);
});

test('the list is read again when the host says a request came or went, and no more once watching is stopped', async () => {
  const { api, state } = fakeApi([]);
  const controller = new BotWorkspaceRequestsController(api, () => undefined);
  const stop = controller.watch();
  state.waiting.push(request('r1', 'b1'));
  for (const listener of state.listeners) listener({ botIds: ['b1'] });
  await settle();
  assert.equal(state.listed, 1);
  assert.deepEqual(requestsOfBot(controller.getSnapshot(), 'b1').map((entry) => entry.requestId), ['r1']);
  stop();
  assert.equal(state.listeners.size, 0);
});

test('allowing a request gives the folder to the Bot, tells the list of Bots, and reads the requests again', async () => {
  const bound = bot({ revision: 2, workspaces: [{ id: 'wA', path: '/work/a', mode: 'read', addedAt: 2 }] });
  const { api, state } = fakeApi([request('r1', 'b1'), request('r2', 'b1', '/work/b')], { decide: async () => bound });
  const told: BotUi[] = [];
  const controller = new BotWorkspaceRequestsController(api, (record) => told.push(record));
  await controller.load();
  const answer = await controller.decide('b1', 'r1', 'approve');
  assert.equal(answer, bound);
  assert.deepEqual(state.decided, [['b1', 'r1', 'approve']]);
  assert.deepEqual(told, [bound]);
  assert.deepEqual(requestsOfBot(controller.getSnapshot(), 'b1').map((entry) => entry.requestId), ['r2'], 'the one that was answered is gone');
  assert.equal(controller.getSnapshot().deciding, undefined);
});

test('denying a request changes nothing of the Bot', async () => {
  const { api, state } = fakeApi([request('r1', 'b1')]);
  const told: BotUi[] = [];
  const controller = new BotWorkspaceRequestsController(api, (record) => told.push(record));
  await controller.load();
  assert.equal(await controller.decide('b1', 'r1', 'deny'), undefined);
  assert.deepEqual(state.decided, [['b1', 'r1', 'deny']]);
  assert.deepEqual(told, []);
  assert.deepEqual(controller.getSnapshot().requests, []);
});

test('an answer that is refused says why for that request, and the list is read again, since the request may be gone', async () => {
  const { api, state } = fakeApi([request('r1', 'b1')], { decide: async () => { throw new Error("Error invoking remote method 'x': Error: That request has run out."); } });
  const controller = new BotWorkspaceRequestsController(api, () => undefined);
  await controller.load();
  state.waiting = [];
  assert.equal(await controller.decide('b1', 'r1', 'approve'), undefined);
  assert.deepEqual(controller.getSnapshot().decideError, { requestId: 'r1', message: 'That request has run out.' });
  assert.equal(controller.getSnapshot().deciding, undefined);
  assert.deepEqual(controller.getSnapshot().requests, [], 'what the host has now');
});

test('one request is answered at a time', async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const { api, state } = fakeApi([request('r1', 'b1'), request('r2', 'b1', '/work/b')], { decide: async (botId) => { await gate; return bot({ botId, revision: 2 }); } });
  const controller = new BotWorkspaceRequestsController(api, () => undefined);
  await controller.load();
  const first = controller.decide('b1', 'r1', 'approve');
  await settle();
  assert.deepEqual(controller.getSnapshot().deciding, { requestId: 'r1', decision: 'approve' });
  assert.equal(await controller.decide('b1', 'r2', 'approve'), undefined, 'ignored while the first is under way');
  release();
  await first;
  assert.deepEqual(state.decided, [['b1', 'r1', 'approve']]);
});
