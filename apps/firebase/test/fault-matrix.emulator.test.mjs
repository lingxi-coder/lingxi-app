// Fault injection for Team task execution (M2 Q09; acceptance C3, C4, B5, C8, J6).
//
// A scripted host runs one task with one tool step against the real service in the emulator and stops at one of seven
// boundaries: creating the request, obtaining the permit, saving the checkpoint, dispatching the tool, receiving the
// tool result, writing the final result, and the service's acknowledgement. A restarted host then does what the Desktop
// host does on start. Each boundary runs ten rounds over the four write categories.
//
// The system the tool reaches is `Outside`. It keeps its own ledger, which the service cannot change, so the checks do
// not rest on what the service says about itself: a keyed (idempotent) write has one effect per operation, a write that
// cannot be repeated is called at most once and is never sent again by a recovery.
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { createHash } from 'node:crypto';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';

// Racing transactions wait on each other's locks in the emulator: the two-process rounds take minutes, not seconds.
const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 600000 };
const ROUNDS = 10, PROCESSES = 2, CATEGORIES = ['store_idempotent', 'provider_idempotent', 'queryable', 'unqueryable'];
const projectId = 'demo-lingxi-teams-faults', teamId = 'team_faults', conversationId = 'chat_faults', botId = 'sharedbot_01';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const checkpoint = 'c'.repeat(64), keyed = category => category === 'store_idempotent' || category === 'provider_idempotent';
let app, db, teams, store, now, serial = 0, outside;

/** The system a tool call reaches, with a ledger of its own. */
class Outside {
  calls = new Map(); effects = new Map();
  /** A provider that honours an idempotency key applies one effect per key however often it is called; any other applies every call. */
  call(operationId, category) {
    this.calls.set(operationId, this.callsOf(operationId) + 1);
    if (!keyed(category) || !this.happened(operationId)) this.effects.set(operationId, this.effectsOf(operationId) + 1);
  }
  callsOf(operationId) { return this.calls.get(operationId) ?? 0; }
  effectsOf(operationId) { return this.effects.get(operationId) ?? 0; }
  happened(operationId) { return this.effectsOf(operationId) > 0; }
}

const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner, commandId) => store.execute(actor, command(action, payload, commandId));
const chatRef = () => db.doc(`teams/${teamId}/conversations/${conversationId}`);
const botRef = () => db.doc(`teams/${teamId}/sharedBots/${botId}`);
const taskData = async id => (await db.doc(`teams/${teamId}/tasks/${id}`).get()).data();
const lockRef = () => db.collection(`teams/${teamId}/chatExecutionLocks`).doc(createHash('sha256').update(JSON.stringify([botId, conversationId])).digest('hex'));
const messages = async () => (await chatRef().collection('messages').get()).docs.map(doc => ({ id: doc.id, ...doc.data() }));
const input = (kind, resourceId) => ({ kind, resourceId, revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } });
const publication = (override = {}) => ({ sharedBotId: botId, expectedRevision: 0, name: 'Travel assistant', description: 'Shared travel notes',
  usePolicy: { mode: 'all' }, deployment: { ...host, sharedBotId: botId, revision: 1, owner: { uid: owner.uid, generation: 1 },
    instanceId: 'instance_01', definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: false, writeOperations: true, delegation: false }, enabled: true },
  inputs: [input('persona', 'persona_01'), input('file', 'travel_file')], quotas: { team: 100, member: 20, bot: 100 }, ...override });

/** A new Team with one published Bot that may write, its host online, and a Bot chat of `member`. */
async function reset() {
  await db.recursiveDelete(db.collection('teams')); await db.recursiveDelete(db.collection('users'));
  now = Date.UTC(2026, 10, 5); outside = new Outside();
  teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Fault matrix' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  await execute('publishSharedBot', publication());
  // The rounds of one boundary are more requests than a member's default allowance.
  await botRef().update({ quotas: { team: 1000, member: 1000, bot: 1000 } });
  await execute('heartbeatDeployment', { ...host, ready: true });
  await execute('createConversation', { conversationId, kind: 'bot_dm', title: 'Travel', participantUids: [member.uid], sharedBotIds: [botId] }, member);
}
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'team-fault-matrix'); db = getFirestore(app);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

// ---- The member's client ----
const request = n => ({ conversationId, messageId: `message_${n}`, changeId: `change_${n}`, text: `Request ${n}`, targets: [botId] });

// ---- The scripted host ----
const startPayload = async taskId => ({ ...host, taskId, expectedLastChangeSeq: (await chatRef().get()).data().lastChangeSeq });
const recoveryPayload = run => ({ ...host, taskId: run.taskId, expectedEpoch: run.epoch, checkpointDigest: checkpoint, reason: 'recovery', dispatchedRequests: 1 });
const operationIdOf = (taskId, step, category) => createHash('sha256').update(JSON.stringify([taskId, taskId, step, category])).digest('hex');
const recordPayload = (run, permitId = run.permitId, step = run.step) => ({ ...host, taskId: run.taskId, permitId,
  operationId: operationIdOf(run.taskId, step, run.category), logicalStepId: step, category: run.category,
  tool: { name: 'fixture.write', version: '1' }, digest: '1'.repeat(64), dispatchedAt: now });
const settlePayload = (run, permitId, outcome) => ({ ...host, taskId: run.taskId, permitId, operationId: run.operationId, outcome,
  resultDigest: '2'.repeat(64), settledAt: now, ...(outcome === 'succeeded' ? { reference: `row-${run.n}` } : {}) });
const resultPayload = (run, permitId = run.permitId) => ({ ...host, taskId: run.taskId, permitId, outcome: 'completed', text: `Done ${run.n}.`, finishedAt: now,
  inputGrantIds: run.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' },
  document: { title: `Result ${run.n}`, body: `Done ${run.n}.` }, usage: { status: 'missing' } });

/** A member's request and the host's first permit for it. The category of the task's one write step follows the round. */
async function open(n, category = CATEGORIES[n % CATEGORIES.length]) {
  now += 1000; await execute('heartbeatDeployment', { ...host, ready: true });
  const receipt = await execute('submit', request(++serial), member), taskId = receipt.tasks[0].taskId;
  const started = await execute('startTask', await startPayload(taskId));
  assert.equal(started.execute, true);
  return runOf(n, category, taskId, started);
}
const runOf = (n, category, taskId, started) => ({ n, category, taskId, step: 'step/t1/c0', operationId: operationIdOf(taskId, 'step/t1/c0', category),
  permitId: started.permitId, epoch: started.epoch ?? 1, inputs: started.inputs });

/**
 * The tool step as the host runs it: the record is accepted first, only then is the outside system called, then the outcome is
 * settled. `stop` names the point where the process dies. Returns what the model hears: 'succeeded' or 'failed'.
 */
async function toolStep(run, permitId, stop) {
  if (stop === 'before-record') return 'died';
  const recorded = await execute('recordOperation', recordPayload(run, permitId));
  if (stop === 'after-record') return 'died';
  if (!recorded.dispatch) {
    // A receipt of an earlier attempt: the step is not run again.
    assert.ok(['succeeded', 'failed'].includes(recorded.state), `a step without a licence and without a receipt: ${recorded.state}`);
    return recorded.state;
  }
  outside.call(run.operationId, run.category);
  if (stop === 'after-call') return 'died';
  now += 100;
  assert.equal((await execute('settleOperation', settlePayload(run, permitId, 'succeeded'))).state, 'succeeded');
  return 'succeeded';
}
/**
 * A restarted host asks to go on. It settles what the outside system can still answer for, under the permit of the attempt
 * that sent it; `permitId` is what it saved before it died (none when it died before saving the permit).
 */
async function recover(run, permitId = run.permitId) {
  now += 1000;
  let answer = await execute('continueTask', recoveryPayload(run));
  if (answer.reconcile) {
    assert.deepEqual(answer.reconcile, [run.operationId]); assert.equal(run.category, 'queryable');
    await execute('settleOperation', settlePayload(run, permitId, outside.happened(run.operationId) ? 'succeeded' : 'failed'));
    answer = await execute('continueTask', recoveryPayload(run));
  }
  if (answer.execute) { run.permitId = answer.permitId; run.epoch = answer.epoch; }
  return answer;
}
/** The maintainer checks the outside system and says what happened. */
async function maintainerResolves(run) {
  now += 1000;
  return execute('resolveOperation', { taskId: run.taskId, operationId: run.operationId, resolution: outside.happened(run.operationId) ? 'succeeded' : 'failed' });
}
/** What every round must leave behind, whatever was injected. */
async function audit(run, { state, operation, calls, effects, reply }) {
  const label = `round ${run.n} (${run.category})`;
  const view = await execute('getTask', { taskId: run.taskId }, member);
  assert.equal(view.state, state, `${label}: task state`);
  assert.equal(view.unverifiedOperations, undefined, `${label}: nothing is left unverified`);
  assert.equal((await lockRef().get()).exists, false, `${label}: the Bot chat is free`);
  assert.ok(!(await execute('listHostTasks', host)).tasks.some(task => task.taskId === run.taskId), `${label}: the host has nothing left to do`);
  const operations = (await execute('listOperations', { taskId: run.taskId }, member)).operations;
  assert.deepEqual(operations.map(o => [o.logicalStepId, o.state]), operation ? [[run.step, operation]] : [], `${label}: ledger`);
  assert.equal(outside.callsOf(run.operationId), calls, `${label}: calls to the outside system`);
  assert.equal(outside.effectsOf(run.operationId), effects, `${label}: effects in the outside system`);
  // The ledger never claims what the outside system does not show.
  if (operation === 'succeeded') assert.equal(outside.happened(run.operationId), true, `${label}: a success that did not happen`);
  if (operation === 'failed') assert.equal(outside.happened(run.operationId), false, `${label}: a failure that happened`);
  if (!keyed(run.category)) assert.ok(outside.callsOf(run.operationId) <= 1, `${label}: a write that cannot be repeated was sent again`);
  const replies = (await messages()).filter(message => message.taskId === run.taskId && message.author.kind === 'shared_bot');
  assert.equal(replies.length, reply ? 1 : 0, `${label}: replies`);
  if (reply) assert.equal(replies[0].text, `Done ${run.n}.`);
}
const done = { state: 'completed', operation: 'succeeded', calls: 1, effects: 1, reply: true };
async function finish(run) {
  now += 1000;
  const delivered = await execute('completeTask', resultPayload(run));
  assert.equal(delivered.state, 'completed');
  return delivered;
}
/** One boundary: ten rounds, each on a fresh task in the same Bot chat, then the totals of the outside ledger. */
function boundary(name, round) {
  test(`fault matrix: ${name}`, options, async () => {
    await reset();
    const runs = [];
    for (let n = 0; n < ROUNDS; n += 1) runs.push(await round(n));
    assert.equal(runs.length, ROUNDS);
    assert.equal(new Set(runs.map(run => run.taskId)).size, ROUNDS);
    assert.equal((await db.collection(`teams/${teamId}/tasks`).get()).size, ROUNDS, 'one task per request');
    assert.equal((await messages()).filter(message => message.author.kind === 'user').length, ROUNDS, 'one message per request');
    for (const run of runs) assert.ok(outside.effectsOf(run.operationId) <= 1, `round ${run.n}: more than one effect`);
  });
}

// 1. Creating the request: the answer to `submit` is lost, the client sends the same request again, also after a restart.
boundary('1 the request is created once however often it is sent', async n => {
  now += 1000; await execute('heartbeatDeployment', { ...host, ready: true });
  const payload = request(++serial), repeats = 2 + n % 2;
  const receipts = await Promise.all(Array.from({ length: repeats }, () => execute('submit', payload, member)));
  receipts.push(await execute('submit', payload, member));
  assert.equal(new Set(receipts.map(receipt => receipt.tasks[0].taskId)).size, 1);
  await assert.rejects(execute('submit', { ...payload, text: 'Another request under the same id' }, member), { code: 'MESSAGE_ID_CONFLICT' });
  const taskId = receipts[0].tasks[0].taskId, category = CATEGORIES[n % CATEGORIES.length];
  const run = runOf(n, category, taskId, await execute('startTask', await startPayload(taskId)));
  assert.equal(await toolStep(run, run.permitId), 'succeeded');
  await finish(run); await audit(run, done);
  return run;
});

// 2. Obtaining the permit: the service granted it and the host died before it had the answer.
boundary('2 a permit that never arrived is not granted again; the restarted host asks for a new one', async n => {
  now += 1000; await execute('heartbeatDeployment', { ...host, ready: true });
  const receipt = await execute('submit', request(++serial), member), taskId = receipt.tasks[0].taskId;
  const payload = await startPayload(taskId), lost = await execute('startTask', payload, owner, `start_command_${n}`);
  assert.equal(lost.execute, true);
  // Neither the same command sent again nor a new one hands out the capability a second time.
  assert.deepEqual(await execute('startTask', payload, owner, `start_command_${n}`), { execute: false, taskId });
  assert.deepEqual(await execute('startTask', payload), { execute: false, taskId });
  const run = runOf(n, CATEGORIES[n % CATEGORIES.length], taskId, { ...lost, permitId: 'permit_never_received' });
  assert.equal(outside.callsOf(run.operationId), 0);
  await assert.rejects(execute('recordOperation', recordPayload(run)), { code: 'EXECUTION_PERMIT_REQUIRED' });
  const next = await recover(run, null);
  assert.equal(next.execute, true); assert.equal(next.epoch, 2); assert.notEqual(next.permitId, lost.permitId);
  // The permit that was lost is void once the next one exists.
  await assert.rejects(execute('recordOperation', recordPayload(run, lost.permitId)), { code: 'EXECUTION_PERMIT_REQUIRED' });
  assert.equal(await toolStep(run, run.permitId), 'succeeded');
  await finish(run); await audit(run, done);
  return run;
});

// 3. Saving the checkpoint: the attempt died before its first tool call. In odd rounds the old process is still alive.
boundary('3 an attempt that died at its checkpoint is replaced, and the process it replaced can do nothing', async n => {
  const run = await open(n), old = run.permitId;
  const next = await recover(run);
  assert.equal(next.execute, true); assert.equal(next.epoch, 2);
  // A second request to recover the same attempt is refused: the epoch has moved.
  assert.equal((await execute('continueTask', { ...recoveryPayload(run), expectedEpoch: 1 })).execute, false);
  if (n % 2) {
    await assert.rejects(execute('recordOperation', recordPayload(run, old)), { code: 'EXECUTION_PERMIT_REQUIRED' });
    await assert.rejects(execute('completeTask', resultPayload(run, old)));
    assert.equal(outside.callsOf(run.operationId), 0);
  }
  assert.equal(await toolStep(run, run.permitId), 'succeeded');
  await finish(run); await audit(run, done);
  return run;
});

// 4. Dispatching the tool: the process died before the outside system was called. In rounds 4 to 7 the record had not reached
// the service either; otherwise it had, and only its acknowledgement was lost.
boundary('4 a tool that was recorded but never sent is sent once or not at all', async n => {
  const run = await open(n), old = run.permitId, recorded = Math.floor(n / 4) % 2 === 0;
  assert.equal(await toolStep(run, old, recorded ? 'after-record' : 'before-record'), 'died');
  assert.equal(outside.callsOf(run.operationId), 0, 'nothing is sent before the record is acknowledged');
  const next = await recover(run, old);
  if (!recorded || keyed(run.category)) {
    // Nothing was recorded, or repeating it is harmless: the new attempt runs the step.
    assert.equal(next.execute, true);
    assert.equal(await toolStep(run, run.permitId), 'succeeded');
    await finish(run); await audit(run, done);
  } else if (run.category === 'queryable') {
    // The outside system was asked and had no trace: the step is closed as not done and is not sent by the recovery.
    assert.equal(next.execute, true);
    assert.equal(await toolStep(run, run.permitId), 'failed');
    await finish(run); await audit(run, { state: 'completed', operation: 'failed', calls: 0, effects: 0, reply: true });
  } else {
    // Nobody can ask the outside system: a person decides, and nothing is sent in the meantime.
    assert.deepEqual(next, { execute: false, taskId: run.taskId, state: 'needs_reconciliation' });
    assert.equal((await execute('getTask', { taskId: run.taskId }, member)).unverifiedOperations, 1);
    assert.equal((await execute('continueTask', recoveryPayload(run))).execute, false);
    await assert.rejects(execute('recordOperation', recordPayload(run, old)));
    assert.equal((await maintainerResolves(run)).state, 'interrupted');
    await audit(run, { state: 'interrupted', operation: 'failed', calls: 0, effects: 0, reply: false });
  }
  return run;
});

// 5. Receiving the tool result: the outside system was called and the process died before the outcome was saved.
boundary('5 a tool that was sent with an unsaved outcome is never sent blindly again', async n => {
  const run = await open(n), old = run.permitId;
  assert.equal(await toolStep(run, old, 'after-call'), 'died');
  assert.equal(outside.callsOf(run.operationId), 1);
  const next = await recover(run, old);
  if (keyed(run.category)) {
    // The key makes the repetition harmless: two calls, one effect.
    assert.equal(next.execute, true);
    assert.equal(await toolStep(run, run.permitId), 'succeeded');
    await finish(run); await audit(run, { ...done, calls: 2 });
  } else if (run.category === 'queryable') {
    // The outside system confirmed it: the new attempt gets the receipt instead of a licence.
    assert.equal(next.execute, true);
    assert.equal(await toolStep(run, run.permitId), 'succeeded');
    await finish(run); await audit(run, done);
  } else {
    assert.deepEqual(next, { execute: false, taskId: run.taskId, state: 'needs_reconciliation' });
    assert.equal((await execute('continueTask', recoveryPayload(run))).execute, false);
    await assert.rejects(execute('completeTask', resultPayload(run, old)));
    assert.equal((await maintainerResolves(run)).state, 'interrupted');
    await audit(run, { state: 'interrupted', operation: 'succeeded', calls: 1, effects: 1, reply: false });
  }
  return run;
});

// 6. Writing the final result: the tool's outcome was saved and the process died before it delivered the result.
boundary('6 a finished tool is not run again by the attempt that delivers the result', async n => {
  const run = await open(n), old = run.permitId;
  assert.equal(await toolStep(run, old), 'succeeded');
  const next = await recover(run, old);
  assert.equal(next.execute, true); assert.equal(next.epoch, 2);
  const receipt = await execute('recordOperation', recordPayload(run));
  assert.equal(receipt.dispatch, false); assert.equal(receipt.state, 'succeeded'); assert.equal(receipt.reference, `row-${n}`);
  assert.equal(await toolStep(run, run.permitId), 'succeeded');
  // In odd rounds the old process is still alive and tries to deliver as well.
  if (n % 2) await assert.rejects(execute('completeTask', resultPayload(run, old)));
  await finish(run); await audit(run, done);
  return run;
});

// 7. The service's acknowledgement: the result was saved and its answer was lost.
boundary('7 a delivered result is acknowledged again and changes nothing', async n => {
  const run = await open(n);
  assert.equal(await toolStep(run, run.permitId), 'succeeded');
  const payload = resultPayload(run), first = await execute('completeTask', payload, owner, `complete_${n}`);
  assert.equal(first.state, 'completed');
  const again = await Promise.all(Array.from({ length: 2 + n % 3 }, (_, i) => execute('completeTask', payload, owner, i ? undefined : `complete_${n}`)));
  for (const answer of again) assert.deepEqual(answer, first);
  await assert.rejects(execute('completeTask', { ...payload, text: 'Another result' }), { code: 'TASK_RESULT_CONFLICT' });
  // A host that restarted without knowing it had delivered gets no new attempt.
  assert.equal((await execute('continueTask', recoveryPayload(run))).execute, false);
  assert.deepEqual(await execute('startTask', await startPayload(run.taskId)), { execute: false, taskId: run.taskId });
  assert.equal((await db.collection(`teams/${teamId}/artifacts`).get()).size, n + 1, 'one document per delivered task');
  await audit(run, done);
  return run;
});

// Two processes of the same host race for every capability of one task.
boundary('two processes of one host share one permit, one licence and one result', async n => {
  now += 1000; await execute('heartbeatDeployment', { ...host, ready: true });
  const receipt = await execute('submit', request(++serial), member), taskId = receipt.tasks[0].taskId, payload = await startPayload(taskId);
  const starts = await Promise.all(Array.from({ length: PROCESSES }, () => execute('startTask', payload)));
  assert.equal(starts.filter(answer => answer.execute).length, 1);
  const run = runOf(n, 'unqueryable', taskId, starts.find(answer => answer.execute));
  // Both believe the attempt died and ask to replace it.
  now += 1000;
  const recoveries = await Promise.all(Array.from({ length: PROCESSES }, () => execute('continueTask', recoveryPayload(run))));
  assert.equal(recoveries.filter(answer => answer.execute).length, 1);
  const winner = recoveries.find(answer => answer.execute); run.permitId = winner.permitId; run.epoch = winner.epoch;
  // Both reach the same step of the same attempt: one licence, so one call.
  const records = await Promise.all(Array.from({ length: PROCESSES }, () => execute('recordOperation', recordPayload(run))));
  assert.equal(records.filter(answer => answer.dispatch).length, 1);
  for (const answer of records) if (answer.dispatch) outside.call(run.operationId, run.category);
  now += 100;
  const settles = await Promise.all(Array.from({ length: PROCESSES }, () => execute('settleOperation', settlePayload(run, run.permitId, 'succeeded'))));
  assert.ok(settles.every(answer => answer.state === 'succeeded'));
  now += 1000;
  const result = resultPayload(run), deliveries = await Promise.all(Array.from({ length: PROCESSES }, () => execute('completeTask', result)));
  for (const answer of deliveries) assert.deepEqual(answer, deliveries[0]);
  await audit(run, done);
  return run;
});

// B5 and C8: authority that ends while a task runs. The step already sent stays what it is; nothing further is admitted.
const revocations = {
  async 'the maintainer pauses the Bot'() { await execute('pauseSharedBot', { sharedBotId: botId, expectedRevision: 1 }); },
  async 'the requester is removed from the Team'() { await teams.execute(owner, { action: 'remove', teamId, uid: member.uid, generation: 1 }); },
  async 'the requester may no longer use the Bot'() {
    const bot = (await botRef().get()).data();
    await botRef().update({ 'identity.usePolicy': { mode: 'listed', members: [{ uid: owner.uid, generation: 1 }] },
      grants: bot.grants.map(grant => ({ ...grant, usePolicy: { mode: 'listed', members: [{ uid: owner.uid, generation: 1 }] } })) });
  },
  async 'the configurer signs in again on the host'() {
    await execute('activateDeployment', { deploymentId: host.deploymentId, hostId: host.hostId, expectedAccountEpoch: 1, newAccountEpoch: 2 });
  },
};
test('fault matrix: authority that ends mid-run admits no further tool call and leaves the sent one to be checked', options, async () => {
  const causes = Object.keys(revocations);
  for (let n = 0; n < ROUNDS; n += 1) {
    await reset();
    const cause = causes[n % causes.length], run = await open(n, 'unqueryable'), label = `round ${n}: ${cause}`;
    assert.equal(await toolStep(run, run.permitId, 'after-call'), 'died');
    await revocations[cause]();
    now += 1000;
    const second = recordPayload(run, run.permitId, 'step/t2/c0');
    await assert.rejects(execute('recordOperation', second), error => ['TASK_REVOKED', 'DEPLOYMENT_REQUIRED'].includes(error.code), label);
    assert.equal(outside.callsOf(second.operationId), 0, label);
    assert.equal((await db.doc(`teams/${teamId}/operations/${second.operationId}`).get()).exists, false, `${label}: nothing was recorded`);
    // No new attempt either, from this host or after it comes back.
    const retry = await execute('continueTask', recoveryPayload(run)).catch(error => ({ refused: error.code }));
    assert.ok(retry.refused || retry.execute === false, `${label}: a new attempt was granted`);
    // The step that was sent is still on record as sent, and its effect is still there: nothing claims it was undone.
    const sent = (await db.doc(`teams/${teamId}/operations/${run.operationId}`).get()).data();
    assert.ok(['dispatched', 'unknown'].includes(sent.state), `${label}: ${sent.state}`);
    assert.equal(outside.effectsOf(run.operationId), 1, label);
    // Once the run can no longer report, the task is one to check, and the maintainer closes it with what the outside system shows.
    now = Math.max(now, (await taskData(run.taskId)).deadline);
    const task = await taskData(run.taskId);
    assert.equal(task.unverifiedOperations, 1, label);
    const resolved = await execute('resolveOperation', { taskId: run.taskId, operationId: run.operationId, resolution: 'succeeded' });
    assert.equal(resolved.operationState, 'succeeded', label);
    assert.ok(!['queued', 'running', 'waiting', 'needs_reconciliation', 'completed'].includes(resolved.state), `${label}: ${resolved.state}`);
    assert.equal(outside.callsOf(run.operationId), 1, label);
  }
});

// A result and a change of the Chat's readers race. Whatever the order, the result is published to everyone it may reach or to nobody.
test('fault matrix: a result racing a new Chat reader is published whole or not at all', options, async () => {
  let published = 0, refused = 0;
  for (let n = 0; n < ROUNDS; n += 1) {
    await reset();
    const bot = (await botRef().get()).data();
    // The published inputs may be shown to `member` only.
    await botRef().update({ grants: bot.grants.map(grant => ({ ...grant, usePolicy: { mode: 'listed', members: [{ uid: member.uid, generation: 1 }] } })) });
    const run = await open(n, 'store_idempotent');
    assert.equal(await toolStep(run, run.permitId), 'succeeded');
    now += 1000;
    const join = () => chatRef().update({ memberUids: [member.uid, other.uid] });
    const [delivered] = await (n % 2 ? Promise.all([execute('completeTask', resultPayload(run)), join()])
      : Promise.all([join(), execute('completeTask', resultPayload(run))]).then(([, answer]) => [answer]));
    const text = JSON.stringify(await messages()), artifacts = await db.collection(`teams/${teamId}/artifacts`).get();
    if (delivered.state === 'completed') {
      published += 1;
      assert.ok(delivered.artifactId); assert.ok(text.includes(`Done ${n}.`)); assert.equal(artifacts.size, 1);
      // It was saved for the readers the Chat had at that moment: the later reader is not among them.
      assert.deepEqual(artifacts.docs[0].data().readers.map(reader => reader.uid), [member.uid]);
      await assert.rejects(execute('readArtifact', { artifactId: delivered.artifactId }, other), { code: 'ARTIFACT_READ_REQUIRED' });
    } else {
      refused += 1;
      assert.equal(delivered.state, 'authorization_required'); assert.equal(delivered.artifactId, null);
      assert.ok(!text.includes(`Done ${n}.`), 'a refused result leaked into the Chat'); assert.equal(artifacts.size, 0);
    }
    assert.deepEqual(await execute('completeTask', resultPayload(run)), delivered, 'the same delivery is the same answer');
    assert.equal((await lockRef().get()).exists, false);
    assert.equal(outside.effectsOf(run.operationId), 1);
  }
  assert.equal(published + refused, ROUNDS);
});
