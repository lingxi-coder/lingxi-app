import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { test } from 'node:test';
import { createDatasetAgentTurn } from '../src/dataset-agent-turn.mjs';
import { createLlmHandler } from '../src/llm-api.mjs';
import { VaultError } from '../src/llm-vault.mjs';

const chat = { providerId: 'test-provider', model: 'test-model', messages: [{ role: 'user', content: 'Create a stock table' }] };
const createCall = { id: 'same-provider-id', name: 'dataset_create', arguments: {
  title: 'Stock', fields: [{ id: 'qty', label: 'Quantity', type: 'integer' }],
} };
const putCall = dataset => ({ id: 'same-provider-id', name: 'dataset_put', arguments: { dataset, record: 'a',
  expectedRevision: 0, schemaVersion: 1, partition: 'west', value: { qty: 9 } } });
const latestCall = dataset => ({ id: 'same-provider-id', name: 'dataset_latest', arguments: { dataset, record: 'a' } });
const toolRound = call => ({ type: 'toolRound', assistant: { role: 'assistant' }, calls: [call] });
function driver({ failFirstPut = false, rejectRead = false, attachmentAllowed = true, repeatLatest = false, beforeRead } = {}) {
  const states = new Map(), effects = new Map(), attempts = [];
  const turns = { async update(uid, id, change) {
    const key = JSON.stringify([uid, id]);
    const next = change(structuredClone(states.get(key) ?? null));
    if (next) states.set(key, structuredClone(next));
    return next;
  } };
  let failed = false;
  const versions = [], prompts = [];
  async function model(uid, input) {
    versions.push(input.providerRevision);
    prompts.push(input.system);
    const rounds = input.rounds;
    async function* events() {
      const get = index => rounds[index]?.results['same-provider-id'];
      if (!rounds.length) yield toolRound(createCall);
      else if (rounds.length === 1) yield toolRound(putCall(get(0).reference.dataset));
      else if (rounds.length === 2) {
        const latest = latestCall(get(0).reference.dataset);
        yield { ...toolRound(latest), calls: repeatLatest ? [latest, { ...latest, id: 'duplicate-revision' }] : [latest] };
      }
      else {
        assert.deepEqual(get(2), rejectRead ? { error: 'DATA_ACTION_REJECTED' } : {
          reference: get(1).reference, value: { qty: 9 },
        });
        if (repeatLatest) assert.deepEqual(rounds[2].results['duplicate-revision'], get(2));
        yield { type: 'delta', text: rejectRead ? 'Access removed' : 'Recorded 9' };
        yield { type: 'done' };
      }
    }
    return { provider: { revision: 7 }, events: events() };
  }
  async function data(uid, token, body, signal) {
    assert.equal(token, 'test-token');
    attempts.push(body);
    const { action, record } = body.command;
    if (action === 'describe') return { reference: { kind: 'dataset', dataset: body.dataset }, canManage: attachmentAllowed };
    if (action === 'read') {
      await beforeRead?.(signal);
      if (rejectRead) throw new VaultError(403, 'DATA_ACCESS_REQUIRED');
      return { reference: body.command.reference, value: { qty: 9 } };
    }
    if (action === 'latest') return { reference: { kind: 'record', dataset: body.dataset, record, schemaVersion: 1, revision: 1 } };
    if (!effects.has(body.operation)) {
      const output = action === 'create' ? { reference: { kind: 'dataset', dataset: body.dataset }, schemaVersion: 1 } :
        { reference: { kind: 'record', dataset: body.dataset, record, schemaVersion: 1, revision: 1 } };
      effects.set(body.operation, output);
    }
    if (failFirstPut && action === 'put' && !failed) {
      failed = true;
      throw new VaultError(503, 'DATA_UNAVAILABLE');
    }
    return effects.get(body.operation);
  }
  const handle = createDatasetAgentTurn({ turns, model, data });
  const id = randomUUID();
  const send = (action, rest = {}, uid = 'test-user', signal) => handle(uid, 'test-token', { action, id, ...rest }, signal);
  return { send, effects, attempts, states, versions, prompts };
}

test('an attached personal Dataset is rechecked before the provider sees its identifier and cannot change on replay', async () => {
  const attachment = { kind: 'dataset', dataset: 'orders' };
  const permitted = driver();
  const allowed = await permitted.send('start', { chat, attachment });
  assert.equal(allowed.state, 'approval');
  assert.deepEqual(permitted.attempts.map(body => [body.dataset, body.command.action]), [['orders', 'describe']]);
  assert.match(permitted.prompts[0], /personal Dataset identified by "orders"/);
  assert.equal(permitted.effects.size, 0);
  await assert.rejects(permitted.send('start', { chat, attachment: { ...attachment, dataset: 'another' } }), { code: 'AGENT_TURN_CHANGED' });

  const unauthorized = driver({ attachmentAllowed: false });
  const denied = await unauthorized.send('start', { chat, attachment });
  assert.equal(denied.state, 'failed');
  assert.equal(denied.code, 'DATA_ACCESS_REQUIRED');
  assert.deepEqual(unauthorized.prompts, []);
});

test('approval, replay after lost write response, and authenticated readback have one durable write', async () => {
  const { send, effects, attempts, states, versions } = driver({ failFirstPut: true });
  const first = await send('start', { chat });
  assert.equal(first.state, 'approval');
  assert.equal(effects.size, 0);
  await assert.rejects(send('get', {}, 'other-user'), { code: 'AGENT_TURN_GONE' });
  await assert.rejects(send('approve', { approval: randomUUID() }), { code: 'AGENT_PROPOSAL_CHANGED' });
  assert.deepEqual(await send('start', { chat }), first);
  assert.equal((await send('approve', { approval: first.approval })).state, 'ready');
  assert.equal((await send('approve', { approval: first.approval })).state, 'ready');
  const second = await send('advance');
  assert.notEqual(second.approval, first.approval);
  assert.equal((await send('approve', { approval: second.approval })).state, 'executing');
  assert.equal((await send('advance')).state, 'ready');
  const puts = attempts.filter(attempt => attempt.command.action === 'put');
  assert.equal(puts.length, 2);
  assert.deepEqual(puts[0], puts[1]);
  assert.equal(effects.size, 2, 'one create and one write');
  const third = await send('advance');
  assert.equal((await send('approve', { approval: third.approval })).state, 'ready');
  const stored = [...states.values()][0];
  assert.deepEqual(Object.keys(stored.rounds[2].results['same-provider-id']), ['reference']);
  assert.deepEqual(await send('advance'), { id: first.id, state: 'done', answer: 'Recorded 9' });
  assert.deepEqual(versions, [undefined, 7, 7, 7]);
});

for (const decision of ['cancel', 'deny']) {
  test(decision + ' before approval leaves no side effects and survives replay', async () => {
    const { send, attempts } = driver();
    const turn = await send('start', { chat });
    const result = await send(decision, decision === 'deny' ? { approval: turn.approval } : {});
    assert.equal(result.state, decision === 'deny' ? 'denied' : 'cancelled');
    assert.deepEqual(await send('approve', { approval: turn.approval }), result);
    assert.deepEqual(attempts, []);
  });
}

test('previous read receipts do not disclose revoked data to the provider on resume', async () => {
  const { send } = driver({ rejectRead: true });
  let turn = await send('start', { chat });
  for (let i = 0; i < 3; i++) {
    assert.equal(turn.state, 'approval');
    assert.equal((await send('approve', { approval: turn.approval })).state, 'ready');
    turn = await send('advance');
  }
  assert.equal(turn.state, 'done');
  assert.equal(turn.answer, 'Access removed');
});

test('a model continuation authorizes each distinct immutable revision once', async () => {
  const { send, attempts } = driver({ repeatLatest: true });
  let turn = await send('start', { chat });
  for (let round = 0; round < 3; round++) {
    await send('approve', { approval: turn.approval });
    turn = await send('advance');
  }
  assert.equal(turn.state, 'done');
  assert.equal(attempts.filter(({ command }) => command.action === 'latest').length, 2);
  assert.equal(attempts.filter(({ command }) => command.action === 'read').length, 1);
});

test('cancelling before model dispatch aborts in-flight Dataset reads and releases the lease', async () => {
  let enter;
  const entered = new Promise(resolve => { enter = resolve; });
  let first = true;
  const { send, versions } = driver({ beforeRead: async signal => {
    if (!first) return;
    first = false;
    enter();
    signal.throwIfAborted();
    await new Promise((_resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
  } });
  let turn = await send('start', { chat });
  for (let round = 0; round < 2; round++) {
    await send('approve', { approval: turn.approval });
    turn = await send('advance');
  }
  await send('approve', { approval: turn.approval });
  const controller = new AbortController();
  const blocked = send('advance', {}, 'test-user', controller.signal);
  await entered;
  controller.abort();
  await blocked;
  assert.equal(versions.length, 3);
  assert.equal((await send('advance')).state, 'done');
  assert.equal(versions.length, 4);
});

test('the agent route is opt-in and operates with the verified user token', async () => {
  const path = 'https://example.test/v1/llm/dataset-turn';
  const verifyToken = async token => { assert.equal(token, 'test-token'); return { uid: 'test-user' }; };
  const init = { method: 'POST', headers: { Authorization: 'Bearer test-token' }, body: JSON.stringify({ action: 'get', id: randomUUID() }) };
  const disabled = createLlmHandler({ vault: {}, verifyToken });
  assert.equal((await disabled(new Request(path, init))).status, 404);
  const enabled = createLlmHandler({ vault: {}, verifyToken, datasetTurn: async (uid, token, body) => {
    assert.equal(uid, 'test-user');
    assert.equal(token, 'test-token');
    assert.deepEqual(body, JSON.parse(init.body));
    return { state: 'approval' };
  } });
  assert.equal((await enabled(new Request(path, { ...init, headers: {} }))).status, 401);
  assert.deepEqual(await (await enabled(new Request(path, init))).json(), { state: 'approval' });
});
