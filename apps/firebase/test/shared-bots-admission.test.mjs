import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { test } from 'node:test';
import { serviceScope } from '../src/hosted-bot-service.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';

test('continuation admission overlaps lock, queue and answer reads and keeps lock-read error precedence', async () => {
  const reads = [];
  const lockRead = Promise.withResolvers();
  const lockError = new Error('lock read failed');
  const queueError = new Error('queue read failed');
  const store = new SharedBotsStore({});
  store.requesterAuthorized = () => true;
  store.validateTask = async () => ({ ref: { collection: () => ({ doc: id => id }) } });
  store.sourceWatermarkCurrent = async () => true;
  store.earlierQueued = async () => { reads.push('queue'); throw queueError; };
  const team = { collection: () => ({ doc: () => 'lock' }) };
  const tx = { get: async ref => { reads.push(ref); if (ref === 'lock') return lockRead.promise; throw new Error('answer read failed'); } };
  const task = { sharedBotId: 'bot', conversationId: 'chat' };
  const deployment = { ready: true, onlineUntil: 100 };

  const outcome = store.admitRun(tx, team, task, {}, deployment, 0, 10, { answerMessageId: 'answer' })
    .then(() => undefined, error => error);
  await setImmediate();
  const readsBeforeLockSettled = [...reads];
  lockRead.reject(lockError);

  assert.equal(await outcome, lockError);
  assert.deepEqual(readsBeforeLockSettled, ['lock', 'queue', 'answer']);
});

test('hosted service batches each task once across caller and service concurrency lists', async () => {
  const ref = path => ({ path, id: path.split('/').at(-1),
    collection: name => ref(`${path}/${name}`), doc: id => ref(`${path}/${id}`) });
  const values = new Map([
    ['service/active/caller', { taskIds: ['ended', 'running'] }],
    ['service/active/_service', { taskIds: ['other', 'running', 'ended'] }],
    ['service/tasks/running', { state: 'running' }],
    ['service/tasks/other', { state: 'queued' }],
    ['service/tasks/ended', { state: 'completed' }],
  ]);
  const reads = [], writes = new Map();
  const tx = {
    get: async () => ({ exists: false }),
    getAll: async (...refs) => {
      reads.push(refs.map(ref => ref.path));
      return refs.map(ref => ({ id: ref.id, exists: values.has(ref.path), data: () => values.get(ref.path) }));
    },
    set: (ref, value) => writes.set(ref.path, value.taskIds),
  };
  const leave = await serviceScope.enter({ tx, root: ref('service'), service: { online: true, limits: { userConcurrent: 2, serviceConcurrent: 3 } },
    self: { role: 'member' }, identity: { uid: 'caller' }, action: 'submit',
    payload: { conversationId: 'chat', messageId: 'message', targets: ['bot'] }, now: 10, liveState: task => task.state });
  leave({ tasks: [{ taskId: 'new' }] });

  assert.deepEqual(reads, [
    ['service/active/caller', 'service/active/_service'],
    ['service/tasks/ended', 'service/tasks/running', 'service/tasks/other'],
  ]);
  assert.deepEqual(writes.get('service/active/caller'), ['running', 'new']);
  assert.deepEqual(writes.get('service/active/_service'), ['other', 'running', 'new']);
});
