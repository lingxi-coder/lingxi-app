// Bot v3 B7a: several Bots on several Desktops answering one Chat, and whom a message is for. The service opens a round for the messages nobody asked a Bot about,
// each Desktop bids, and the service's clock alone settles it; @mentions, replies to a Bot and a Bot's name are decided when the message is admitted.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore } from '../src/teams.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { admissionId } from '../src/team-execution-contract.mjs';
import { ReplyRoundJobs } from '../src/reply-round-jobs.mjs';
import { InboxJobs } from '../src/inbox-jobs.mjs';
import { settleReplyRound } from '../src/reply-rounds.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-reply-rounds', teamId = 'team_rounds_01', trip = 'chat_rounds_trip', dm = 'chat_rounds_dm';
const owner = { uid: 'owner', name: 'Owner' }, member = { uid: 'member', name: 'Member' }, other = { uid: 'other', name: 'Other' };
// Three Bots of three maintainers, each on a Desktop of its own.
const bot = (n, name, maintainer) => ({ id: `sharedbot_0${n}`, name, maintainer, host: { deploymentId: `deployment_0${n}`, hostId: `host_000${n}`, accountEpoch: 1 } });
const alpha = bot(1, 'Alpha bot', owner), beta = bot(2, 'Beta bot', member), gamma = bot(3, 'Gamma bot', other);
const all = [alpha, beta, gamma], ids = all.map(b => b.id);
const asking = 'Does anyone know a quiet hotel near the station?';
let app, db, teams, store, messages, now, serial = 0;
const command = (action, payload, commandId = `command_${++serial}`) => ({ schemaVersion: 1, commandId, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner, commandId) => store.execute(actor, command(action, payload, commandId));
const publication = (b, more = {}) => ({ sharedBotId: b.id, expectedRevision: 0, name: b.name, description: '', usePolicy: { mode: 'all' }, autoReply: { daily: 50 },
  deployment: { ...b.host, sharedBotId: b.id, revision: 1, owner: { uid: b.maintainer.uid, generation: 1 }, instanceId: `instance_${b.id}`,
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: true, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 }, ...more });
const taskRef = id => db.doc(`teams/${teamId}/tasks/${id}`);
const taskData = async id => (await taskRef(id).get()).data();
const chatRef = id => db.doc(`teams/${teamId}/conversations/${id}`);
const messageId = n => `msg_${String(n).padStart(4, '0')}`;
const taskOf = (n, b = alpha) => admissionId(messageId(n), b.id);
const roundData = async n => (await db.doc(`teams/${teamId}/replyRounds/${messageId(n)}`).get()).data();
const allTasks = async () => (await db.collection(`teams/${teamId}/tasks`).get()).docs.map(doc => doc.data());
const rounds = async () => (await db.collection(`teams/${teamId}/replyRounds`).get()).docs.map(doc => doc.data());
const said = (actor, n, text, more = {}) => messages.execute(actor, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(n),
  changeId: `chg_${String(n).padStart(4, '0')}`, operation: 'create', text, ...more });
const say = (actor, n, text = asking, more = {}) => said(actor, n, text, more);
const ask = (actor, n, targets, text = asking, more = {}) => execute('submit', { conversationId: trip, messageId: messageId(n), changeId: `chg_${String(n).padStart(4, '0')}`,
  text, targets: targets.map(b => b.id), ...more }, actor);
const bid = (b, n, tier, score, source = 'decision') => execute('bidReply', { ...b.host, taskId: taskOf(n, b), tier, ...(score === undefined ? {} : { score }), source }, b.maintainer);
const begin = async (b, n) => execute('startTask', { ...b.host, taskId: taskOf(n, b), expectedLastChangeSeq: (await chatRef(trip).get()).data().lastChangeSeq }, b.maintainer);
const listed = b => execute('listHostTasks', b.host, b.maintainer);
const beat = (b, ready = true) => execute('heartbeatDeployment', { ...b.host, ready }, b.maintainer);
const result = (started, b, more = {}) => ({ ...b.host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text: 'The Station Inn is quiet.',
  finishedAt: now, inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const jobs = (phase, n) => settleReplyRound(db, () => now, { teamId, roundId: messageId(n), phase });
const order = taskId => createHash('sha256').update(taskId).digest('hex');
const settings = (revision, more = {}) => execute('setConversationAutoReply', { conversationId: trip, expectedRevision: revision, targets: ids, ...more }, member);
const states = async (n, bots = all) => Object.fromEntries(await Promise.all(bots.map(async b => { const t = await taskData(taskOf(n, b)); return [b.name, t && [t.state, t.declineReason, t.round?.outcome]]; })));

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'reply-rounds-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  for (const name of ['teams', 'users']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 26); teams = new TeamsStore(db, () => now); store = new SharedBotsStore(db, () => now); messages = new TeamMessagesStore(db, () => now);
  await teams.execute(owner, { action: 'create', teamId, name: 'Rounds test' });
  const { code } = await teams.execute(owner, { action: 'invite', teamId, requestId: 'invite_01', role: 'member', maxUses: 10, ttlSeconds: 3600 });
  for (const actor of [member, other]) await teams.execute(actor, { action: 'join', code, requestId: `join_${actor.uid}` });
  for (const b of all) { await execute('publishSharedBot', publication(b), b.maintainer); await beat(b); }
  await execute('createConversation', { conversationId: trip, kind: 'group', title: 'Trip', participantUids: [member.uid, other.uid], sharedBotIds: ids }, member);
  await settings(0);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('a message nobody asked a Bot about opens a round, and a task of it does not start before the round has chosen it', options, async () => {
  await say(member, 1);
  const round = await roundData(1);
  assert.deepEqual([round.state, round.maxReplies, round.deadlineAt, round.bids], ['open', 1, now + 15000, {}]);
  assert.deepEqual(round.candidates.map(c => c.sharedBotId).sort(), ids);
  for (const b of all) {
    const task = await taskData(taskOf(1, b));
    assert.deepEqual([task.roundId, task.optional, task.visibility, task.state, task.addressed], [messageId(1), true, 'hidden', 'queued', undefined]);
    assert.equal((await listed(b)).tasks[0].round, 'open', 'the host is told the round has not chosen yet');
  }
  await assert.rejects(begin(alpha, 1), { code: 'ROUND_OPEN' });
  // Nothing is charged for a round, whoever it chooses.
  assert.equal((await db.collection(`teams/${teamId}/requestQuotas`).get()).size, 0);
});

test('the last bid settles the round: the best tier wins, the rest are declined as coordination, and a bid is answered the same however often it is sent', options, async () => {
  await say(member, 1);
  for (const b of [alpha, beta]) await execute('recordTriage', { ...b.host, taskId: taskOf(1, b), action: 'run', source: 'decision', probability: 0.9 }, b.maintainer);
  assert.deepEqual(await bid(alpha, 1, 'may_decline', 0.99), { taskId: taskOf(1), round: 'open' });
  assert.deepEqual(await bid(beta, 1, 'run', 0.5), { taskId: taskOf(1, beta), round: 'open' });
  assert.equal((await roundData(1)).state, 'open');
  assert.deepEqual(await bid(gamma, 1, 'decline', undefined, 'quiet'), { taskId: taskOf(1, gamma), round: 'abstained' });
  // A tier counts before any score: the Bot that was surer of itself but only may answer loses to the one that runs.
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'coordination', 'lost'], 'Beta bot': ['queued', undefined, 'won'], 'Gamma bot': ['declined', 'triage', 'abstained'] });
  const round = await roundData(1);
  assert.deepEqual([round.state, round.winners, round.standby, round.startBy, round.resolvedAt], ['resolved', [taskOf(1, beta)], [taskOf(1)], now + 20000, now]);
  assert.deepEqual((await taskData(taskOf(1))).round, { outcome: 'lost', tier: 'may_decline' });
  assert.deepEqual((await taskData(taskOf(1, gamma))).round, { outcome: 'abstained', tier: 'decline' });
  assert.equal((await listed(beta)).tasks[0].round, 'won');
  // The same bid, another one and one from a task that lost: each is answered with what became of the task, and nothing changes.
  assert.deepEqual(await bid(alpha, 1, 'run', 1), { taskId: taskOf(1), round: 'lost' });
  assert.deepEqual(await bid(beta, 1, 'decline'), { taskId: taskOf(1, beta), round: 'won' });
  assert.deepEqual((await roundData(1)).bids[taskOf(1)], { tier: 'may_decline', score: 0.99, source: 'decision', at: now });
  const started = await begin(beta, 1);
  assert.equal(started.execute, true);
  assert.equal((await execute('completeTask', result(started, beta), member)).state, 'completed');
  // Only the Bot's maintainer reads what its Desktop bid, and only through the log; the Chat's members read nothing of a round.
  const row = (await execute('listTriage', { sharedBotId: alpha.id })).items.find(item => item.taskId === taskOf(1));
  assert.deepEqual([row.state, row.round], ['declined', { outcome: 'lost', tier: 'may_decline' }]);
  assert.equal((await execute('listTriage', { sharedBotId: beta.id }, member)).items[0].round.outcome, 'won');
  const view = await execute('getTask', { taskId: taskOf(1) }, other);
  assert.doesNotMatch(JSON.stringify(view), /"round"|coordination|"tier"|may_decline/u);
  assert.doesNotMatch(JSON.stringify(await execute('listTasks', { conversationId: trip }, other)), /"round"|coordination|"tier"/u);
});

test('equal bids are ordered by a hash of the task id, the same way every time', options, async () => {
  await say(member, 1);
  for (const b of all) await bid(b, 1, 'run', 0.8);
  const expected = all.map(b => taskOf(1, b)).sort((a, b) => order(a).localeCompare(order(b)));
  assert.deepEqual((await roundData(1)).winners, [expected[0]]);
  assert.deepEqual((await roundData(1)).standby, expected.slice(1));
});

test('a Desktop that never answers abstains at the deadline: the job settles once, early it asks to be tried again, and a late bid is told what became of the task', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run', 0.7);
  now += 14999;
  await assert.rejects(jobs('bids', 1), /REPLY_ROUND_NOT_DUE/u);
  now += 1;
  assert.deepEqual(await jobs('bids', 1), { settled: true });
  assert.deepEqual(await states(1), { 'Alpha bot': ['queued', undefined, 'won'], 'Beta bot': ['declined', 'coordination', 'abstained'], 'Gamma bot': ['declined', 'coordination', 'abstained'] });
  assert.equal((await taskData(taskOf(1, beta))).round.tier, undefined, 'it said nothing, so there is no bid to show');
  const before = (await roundData(1)).resolvedAt;
  now += 1000;
  assert.deepEqual(await jobs('bids', 1), { settled: false }, 'a round settles once');
  assert.equal((await roundData(1)).resolvedAt, before);
  assert.deepEqual(await bid(gamma, 1, 'run', 1), { taskId: taskOf(1, gamma), round: 'abstained' });
  assert.deepEqual(await jobs('bids', 99), { settled: false }, 'a round that does not exist is left alone');
});

test('a bid that comes after the deadline is not counted and settles the round', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'may_decline');
  now += 15000;
  assert.deepEqual(await bid(beta, 1, 'rule', 1), { taskId: taskOf(1, beta), round: 'abstained' });
  assert.deepEqual(await states(1), { 'Alpha bot': ['queued', undefined, 'won'], 'Beta bot': ['declined', 'coordination', 'abstained'], 'Gamma bot': ['declined', 'coordination', 'abstained'] });
  assert.deepEqual((await roundData(1)).bids[taskOf(1, beta)], undefined);
});

test('the Chat decides how many Bots may answer one message: two winners with maxReplies 2', options, async () => {
  assert.deepEqual(await settings(1, { maxReplies: 2 }), { revision: 2 });
  await say(member, 1);
  assert.equal((await roundData(1)).maxReplies, 2);
  await bid(alpha, 1, 'run', 0.6); await bid(beta, 1, 'rule'); await bid(gamma, 1, 'may_decline', 0.9);
  assert.deepEqual(await states(1), { 'Alpha bot': ['queued', undefined, 'won'], 'Beta bot': ['queued', undefined, 'won'], 'Gamma bot': ['declined', 'coordination', 'lost'] });
  assert.deepEqual((await roundData(1)).winners, [taskOf(1, beta), taskOf(1)]);
  // Both start: the Chat's lock is theirs one after the other, as for any two tasks.
  assert.equal((await begin(beta, 1)).execute, true);
});

test('the two settings a Chat may add are checked, kept when a caller leaves them out, and shown with the Chat', options, async () => {
  for (const bad of [{ maxReplies: 0 }, { maxReplies: 6 }, { maxReplies: 1.5 }, { maxReplies: '2' }, { humanMentions: 'always' }, { humanMentions: 1 }]) {
    await assert.rejects(settings(1, bad), /./u, JSON.stringify(bad));
  }
  assert.deepEqual(await settings(1, { maxReplies: 5, humanMentions: 'judge' }), { revision: 2 });
  assert.deepEqual(await settings(2), { revision: 3 }, 'a caller that does not know them leaves them as they are');
  const shown = (await messages.execute(member, { action: 'list', teamId, generation: 1 })).conversations.find(c => c.id === trip).autoReply;
  assert.deepEqual(shown, { revision: 3, targets: ids, maxReplies: 5, humanMentions: 'judge' });
  assert.deepEqual(await settings(3, { maxReplies: 1 }), { revision: 4 });
  assert.deepEqual((await chatRef(trip).get()).data().autoReply, { revision: 4, targets: ids, maxReplies: 1, humanMentions: 'judge' });
});

test('a winner that does not start in time gives its seat to the next bidder, once', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run', 0.8); await bid(beta, 1, 'may_decline', 0.9); await bid(gamma, 1, 'rule');
  assert.deepEqual((await roundData(1)).winners, [taskOf(1, gamma)]);
  assert.deepEqual((await roundData(1)).standby, [taskOf(1), taskOf(1, beta)]);
  now += 19999;
  await assert.rejects(jobs('start', 1), /REPLY_ROUND_NOT_DUE/u);
  now += 1;
  assert.deepEqual(await jobs('start', 1), { settled: true, promoted: [taskOf(1)] });
  assert.deepEqual(await states(1), { 'Alpha bot': ['queued', undefined, 'promoted'], 'Beta bot': ['declined', 'coordination', 'lost'], 'Gamma bot': ['declined', 'coordination', 'lost'] });
  const promoted = await taskData(taskOf(1));
  assert.deepEqual([promoted.declineReason, promoted.finishedAt, promoted.round], [undefined, undefined, { outcome: 'promoted', tier: 'run' }]);
  assert.equal((await listed(alpha)).tasks[0].round, 'promoted');
  assert.equal((await begin(alpha, 1)).execute, true, 'the promoted Bot starts like a winner');
  // Once: the same job again, a later one, and a promoted Bot that does not start either change nothing.
  assert.deepEqual(await jobs('start', 1), { settled: false });
  now += 60000;
  assert.deepEqual(await jobs('start', 1), { settled: false });
  assert.deepEqual((await states(1, [beta])), { 'Beta bot': ['declined', 'coordination', 'lost'] });
});

test('a promotion passes over a bidder whose Desktop is gone, and a round with nobody left promotes nobody', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run'); await bid(beta, 1, 'may_decline'); await bid(gamma, 1, 'rule');
  await beat(alpha, false);
  now += 20000;
  assert.deepEqual(await jobs('start', 1), { settled: true, promoted: [taskOf(1, beta)] });
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'coordination', 'lost'], 'Beta bot': ['queued', undefined, 'promoted'], 'Gamma bot': ['declined', 'coordination', 'lost'] });
  await beat(alpha);
  await say(other, 2);
  await bid(alpha, 2, 'decline'); await bid(beta, 2, 'decline'); await bid(gamma, 2, 'rule');
  now += 20000;
  assert.deepEqual(await jobs('start', 2), { settled: true, promoted: [] });
  assert.deepEqual((await roundData(2)).promoted, []);
});

test('a winner that started, or that went on to ask its maintainer, keeps the round', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run'); await bid(beta, 1, 'may_decline'); await bid(gamma, 1, 'rule');
  const started = await begin(gamma, 1);
  assert.equal(started.execute, true);
  now += 20000;
  assert.deepEqual(await jobs('start', 1), { settled: true, promoted: [] });
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'coordination', 'lost'], 'Beta bot': ['declined', 'coordination', 'lost'], 'Gamma bot': ['running', undefined, 'won'] });
  await execute('completeTask', result(started, gamma), gamma.maintainer);
  // Held for its maintainer's approval, then rejected: silence, and nobody takes its place.
  await beat(alpha); await beat(beta); await beat(gamma);
  await say(other, 2);
  await bid(alpha, 2, 'run'); await bid(beta, 2, 'may_decline'); await bid(gamma, 2, 'rule');
  const second = await begin(gamma, 2);
  await execute('holdReply', result(second, gamma, { text: 'Maybe.' }), gamma.maintainer);
  now += 20000;
  assert.deepEqual(await jobs('start', 2), { settled: true, promoted: [] });
  await execute('rejectReply', { taskId: taskOf(2, gamma) }, gamma.maintainer);
  assert.deepEqual(await states(2), { 'Alpha bot': ['declined', 'coordination', 'lost'], 'Beta bot': ['declined', 'coordination', 'lost'], 'Gamma bot': ['declined', 'approval_rejected', 'won'] });
});

test('the timers of a round are set from its tasks, once for the round, and the queue runs them', options, async () => {
  const queued = [], enqueue = async (payload, id, delay) => queued.push({ payload, id, delay });
  const timers = new ReplyRoundJobs({ db, enqueue, clock: () => now });
  await say(member, 1);
  for (const b of all) await timers.candidate(teamId, taskOf(1, b));
  assert.deepEqual(queued.map(job => job.payload), all.map(() => ({ type: 'replyRound', teamId, roundId: messageId(1), phase: 'bids' })));
  assert.deepEqual(new Set(queued.map(job => job.id)).size, 1, 'one job for the round');
  assert.equal(queued[0].delay, 16, 'fifteen seconds and one more, so the queue is never early');
  for (const b of all) await bid(b, 1, 'run', 0.5);
  queued.length = 0;
  const winner = (await roundData(1)).winners[0];
  await timers.winner(teamId, winner);
  assert.deepEqual(queued.map(job => [job.payload.phase, job.delay]), [['start', 21]]);
  // A round that was settled, or whose winners all started, sets no time.
  queued.length = 0;
  await timers.candidate(teamId, winner);
  assert.deepEqual(queued, []);
  now += 20000; await jobs('start', 1); queued.length = 0;
  await timers.winner(teamId, winner);
  assert.deepEqual(queued, [], 'a round that has promoted is over');
  // The queue's job reaches the round.
  await say(other, 2);
  now += 15000;
  await new InboxJobs(null, null, enqueue, null, timers).run({ type: 'replyRound', teamId, roundId: messageId(2), phase: 'bids' });
  assert.equal((await roundData(2)).state, 'resolved');
});

test('a bid belongs to the Bot\'s current deployment: after a switch the old Desktop is refused, and the new one has no task of the round', options, async () => {
  await say(member, 1);
  const moved = { deploymentId: 'deployment_09', hostId: 'host_0009', accountEpoch: 1 };
  await execute('publishSharedBot', publication(alpha, { expectedRevision: 1, deployment: { ...publication(alpha).deployment, ...moved, revision: 2, instanceId: 'instance_09' } }), owner);
  await execute('heartbeatDeployment', { ...moved, ready: true });
  await assert.rejects(bid(alpha, 1, 'run'), { code: 'DEPLOYMENT_REQUIRED' });
  await assert.rejects(execute('bidReply', { ...moved, taskId: taskOf(1), tier: 'run', source: 'rule' }), { code: 'TASK_REQUIRED' });
  await bid(beta, 1, 'run'); await bid(gamma, 1, 'may_decline');
  assert.equal((await roundData(1)).state, 'open', 'the Bot whose Desktop changed has not bid');
  now += 15000;
  await jobs('bids', 1);
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'coordination', 'abstained'], 'Beta bot': ['queued', undefined, 'won'], 'Gamma bot': ['declined', 'coordination', 'lost'] });
});

test('only the Bot\'s own Desktop bids, for an optional task of a round, with a tier it may claim', options, async () => {
  await say(member, 1);
  const taskId = taskOf(1);
  await assert.rejects(execute('bidReply', { ...alpha.host, taskId, tier: 'run', source: 'rule' }, member), { code: 'DEPLOYMENT_REQUIRED' });
  for (const wrong of [{ hostId: 'host_other' }, { accountEpoch: 2 }, { deploymentId: 'deployment_09' }]) {
    await assert.rejects(execute('bidReply', { ...alpha.host, ...wrong, taskId, tier: 'run', source: 'rule' }), { code: 'DEPLOYMENT_REQUIRED' }, JSON.stringify(wrong));
  }
  await assert.rejects(execute('bidReply', { ...alpha.host, taskId: taskOf(1, beta), tier: 'run', source: 'rule' }), { code: 'TASK_REQUIRED' }, 'never for another Bot\'s task');
  await assert.rejects(execute('bidReply', { ...alpha.host, taskId: 'task_does_not_exist', tier: 'run', source: 'rule' }), { code: 'TASK_REQUIRED' });
  // `addressed` is the service's tier: a task nobody spoke to cannot claim it.
  await assert.rejects(bid(alpha, 1, 'addressed'), { code: 'BID_TIER_INVALID' });
  for (const bad of [{ tier: 'best' }, { tier: 'run', score: 1.5 }, { tier: 'run', score: -1 }, { tier: 'run', source: 'model' }, { tier: 'run', text: 'I want it' }]) {
    await assert.rejects(execute('bidReply', { ...alpha.host, taskId, source: 'rule', ...bad }), /./u, JSON.stringify(bad));
  }
  // A task somebody asked for is judged by nobody.
  const asked = await ask(member, 2, [alpha]);
  await assert.rejects(execute('bidReply', { ...alpha.host, taskId: asked.tasks[0].taskId, tier: 'run', source: 'rule' }), { code: 'TASK_NOT_OPTIONAL' });
  // An optional task of a Bot DM is judged alone and has no round.
  await execute('createConversation', { conversationId: dm, kind: 'bot_dm', title: 'Alpha', participantUids: [member.uid], sharedBotIds: [alpha.id] }, member);
  await execute('setConversationAutoReply', { conversationId: dm, expectedRevision: 0, targets: [alpha.id] }, member);
  const dmAsk = await execute('submit', { conversationId: dm, messageId: 'dm_msg_0001', changeId: 'dm_chg_0001', text: 'Hi', targets: [alpha.id] }, member);
  assert.equal((await taskData(dmAsk.tasks[0].taskId)).optional, true);
  await assert.rejects(execute('bidReply', { ...alpha.host, taskId: dmAsk.tasks[0].taskId, tier: 'run', source: 'rule' }), { code: 'ROUND_REQUIRED' });
  assert.equal((await rounds()).length, 1, 'only the Chat\'s message has a round');
  // A republished Bot has no say for tasks of its earlier revision.
  await say(member, 3);
  await execute('publishSharedBot', publication(alpha, { expectedRevision: 1, deployment: { ...publication(alpha).deployment, revision: 2 } }), owner);
  await assert.rejects(bid(alpha, 3, 'run'), { code: 'TASK_REVOKED' });
});

test('a newer message of the Chat ends a round that was not settled, and leaves one that was', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run', 0.8);
  now += 1000;
  await say(other, 2);
  assert.equal((await roundData(1)).state, 'voided');
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'superseded', 'superseded'], 'Beta bot': ['declined', 'superseded', 'superseded'], 'Gamma bot': ['declined', 'superseded', 'superseded'] });
  assert.deepEqual((await taskData(taskOf(1))).round, { outcome: 'superseded', tier: 'run' }, 'what the Bot bid is kept for its maintainer');
  assert.deepEqual(await bid(beta, 1, 'run'), { taskId: taskOf(1, beta), round: 'superseded' });
  assert.equal((await roundData(2)).state, 'open');
  assert.deepEqual(await jobs('bids', 1), { settled: false });
  // A round that has chosen is not undone by what is said next: its winner still has the Chat's attention.
  for (const b of all) await bid(b, 2, 'run', 0.5);
  const winner = (await roundData(2)).winners[0];
  await say(member, 3);
  assert.equal((await roundData(2)).state, 'resolved');
  assert.equal((await taskData(winner)).state, 'queued');
  // A message that asks a Bot ends the open round too.
  await ask(member, 4, [beta]);
  assert.equal((await roundData(3)).state, 'voided');
  assert.deepEqual(await rounds().then(list => list.map(r => r.state).sort()), ['resolved', 'voided', 'voided']);
});

test('a recalled message takes its round and its waiting tasks with it', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run');
  const recall = (n, changeId) => messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(n), changeId, operation: 'recall', text: '' });
  await recall(1, 'chg_recall_1');
  assert.equal((await roundData(1)).state, 'voided');
  assert.deepEqual(await states(1), { 'Alpha bot': ['declined', 'superseded', 'superseded'], 'Beta bot': ['declined', 'superseded', 'superseded'], 'Gamma bot': ['declined', 'superseded', 'superseded'] });
  // An asked Bot's task that has not started is cancelled; one that has started is left to its own checks.
  const receipt = await ask(member, 2, [alpha, beta]);
  const running = await begin(beta, 2);
  assert.equal(running.execute, true);
  await recall(2, 'chg_recall_2');
  assert.equal((await taskData(receipt.tasks[0].taskId)).state, 'cancelled');
  assert.equal((await taskData(receipt.tasks[1].taskId)).state, 'running');
  assert.equal((await taskData(receipt.tasks[0].taskId)).finishedAt, now);
  // A message recalled after its round was settled ends the winner that has not started and voids the round, so its timer promotes nobody.
  await say(other, 3);
  for (const b of all) await bid(b, 3, 'run', 0.5);
  const winner = (await roundData(3)).winners[0];
  await messages.execute(other, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(3), changeId: 'chg_recall_3', operation: 'recall', text: '' });
  assert.deepEqual([(await roundData(3)).state, (await taskData(winner)).state, (await taskData(winner)).declineReason], ['voided', 'declined', 'superseded']);
  now += 20000;
  assert.deepEqual(await jobs('start', 3), { settled: false });
  assert.deepEqual(Object.values(await states(3)).map(([state]) => state), ['declined', 'declined', 'declined']);
});

test('an edit never wakes a Bot: no task, no round, whatever the new text says', options, async () => {
  await say(member, 1, 'Anyone around?');
  const before = (await allTasks()).length;
  await messages.execute(member, { action: 'mutate', teamId, generation: 1, conversationId: trip, messageId: messageId(1), changeId: 'chg_edit_1', operation: 'edit',
    text: '@other Gamma bot, what do you think?', mentionEntities: [{ userId: other.uid, offset: 0, length: 6 }] });
  assert.equal((await allTasks()).length, before);
  assert.equal((await rounds()).length, 1);
});

test('asking a Bot is for that Bot alone: no other Bot joins in, no round is opened, and a message that asks several tells each who else was asked', options, async () => {
  const one = await ask(member, 1, [alpha]);
  assert.equal(one.tasks.length, 1);
  assert.deepEqual((await allTasks()).map(t => [t.sharedBotId, t.optional, t.visibility, t.roundId]), [[alpha.id, undefined, 'asked', undefined]]);
  assert.deepEqual(await rounds(), []);
  assert.equal((await execute('prepareTask', { ...alpha.host, taskId: taskOf(1) })).coMentioned, undefined);
  // Two Bots asked: both answer, neither is told to wait for a round, and each is told of the other. The third is not asked, so it stays out.
  const two = await ask(member, 2, [alpha, beta], 'Alpha bot, Beta bot: where should we eat?');
  assert.equal(two.tasks.length, 2);
  assert.deepEqual((await allTasks()).filter(t => t.messageId === messageId(2)).map(t => [t.sharedBotId, t.optional, t.roundId]).sort(), [[alpha.id, undefined, undefined], [beta.id, undefined, undefined]]);
  assert.deepEqual((await execute('prepareTask', { ...alpha.host, taskId: taskOf(2) })).coMentioned, ['Beta bot']);
  assert.deepEqual((await execute('prepareTask', { ...beta.host, taskId: taskOf(2, beta) }, member)).coMentioned, ['Alpha bot']);
  assert.equal((await begin(alpha, 1)).execute, true, 'an asked task starts at once: it was asked for, and no round has a say');
  assert.deepEqual(await rounds(), []);
  await assert.rejects(bid(alpha, 2, 'run'), { code: 'TASK_NOT_OPTIONAL' });
});

test('a message that mentions only people is for them: no Bot answers, unless the Chat says to judge such messages as usual', options, async () => {
  const mention = { mentionEntities: [{ userId: other.uid, offset: 0, length: 6 }] };
  await say(member, 1);
  await say(member, 2, '@other have a look at this hotel', mention);
  assert.equal((await roundData(1)).state, 'voided', 'it is a new message of the Chat all the same');
  assert.deepEqual(await allTasks().then(list => list.filter(t => t.messageId === messageId(2))), []);
  assert.equal((await db.doc(`teams/${teamId}/replyRounds/${messageId(2)}`).get()).exists, false);
  await settings(1, { humanMentions: 'judge' });
  await say(member, 3, '@other have a look at this hotel', mention);
  assert.equal((await roundData(3)).candidates.length, 3, 'judged as any message is');
  await settings(2, { humanMentions: 'quiet' });
  await say(member, 4, '@other have a look at this hotel', mention);
  assert.equal((await db.doc(`teams/${teamId}/replyRounds/${messageId(4)}`).get()).exists, false);
  // A message that asks a Bot and mentions a person is the Bot's.
  const both = await ask(member, 5, [beta], '@other Beta bot please help', mention);
  assert.equal(both.tasks.length, 1);
  assert.deepEqual((await allTasks()).filter(t => t.messageId === messageId(5)).map(t => t.sharedBotId), [beta.id]);
});

test('a reply to a Bot\'s message is spoken to that Bot: its round has it alone, with the best tier', options, async () => {
  await ask(member, 1, [beta]);
  const started = await begin(beta, 1);
  const reply = await execute('completeTask', result(started, beta, { text: 'Try the Station Inn.' }), member);
  assert.equal(reply.state, 'completed');
  await say(other, 2, 'thanks!', { replyTo: reply.replyId });
  const round = await roundData(2);
  assert.deepEqual(round.candidates, [{ taskId: taskOf(2, beta), sharedBotId: beta.id, addressed: true }]);
  assert.deepEqual([round.maxReplies, (await taskData(taskOf(2, beta))).addressed], [1, true]);
  assert.equal((await listed(beta)).tasks.find(row => row.taskId === taskOf(2, beta)).addressed, true);
  assert.equal(await taskData(taskOf(2)), undefined);
  assert.equal(await taskData(taskOf(2, gamma)), undefined);
  // Its host judges as it always did and bids what it likes: the service has the Bot at the best tier, and the Bot may still stay silent when it runs.
  assert.deepEqual(await bid(beta, 2, 'may_decline', 0.4), { taskId: taskOf(2, beta), round: 'won' });
  assert.deepEqual((await taskData(taskOf(2, beta))).round, { outcome: 'won', tier: 'addressed' });
  // A reply to a person's message is not spoken to any Bot.
  await say(member, 3, 'ok', { replyTo: messageId(2) });
  assert.equal((await roundData(3)).candidates.length, 3);
  // A reply that also mentions a person, in a chat that does not judge those, is for the person.
  await say(member, 4, '@other see above', { replyTo: reply.replyId, mentionEntities: [{ userId: other.uid, offset: 0, length: 6 }] });
  assert.equal((await db.doc(`teams/${teamId}/replyRounds/${messageId(4)}`).get()).exists, false);
});

test('a Bot called by its name is spoken to, as a whole word, with case and width folded; a name inside a word is not a call', options, async () => {
  await say(member, 1, 'what does GAMMA BOT think?');
  assert.deepEqual((await roundData(1)).candidates.map(c => c.sharedBotId), [gamma.id]);
  await say(member, 2, 'ｇａｍｍａ　ｂｏｔ，你怎么看？');
  assert.deepEqual((await roundData(2)).candidates.map(c => c.sharedBotId), [gamma.id], 'full-width letters and the ideographic space are folded');
  for (const [n, text] of [[3, 'the Gamma bots are fine'], [4, 'xgamma bot said so'], [5, 'gammabot?'], [6, 'bot gamma']]) {
    await say(member, n, text);
    assert.equal((await roundData(n)).candidates.length, 3, text);
    assert.equal((await roundData(n)).maxReplies, 1, text);
  }
  // Two names: both are spoken to and both answer, however few the Chat lets answer a message nobody spoke to.
  await say(member, 7, 'Alpha bot and Beta bot, which hotel?');
  assert.deepEqual([(await roundData(7)).candidates.map(c => c.sharedBotId).sort(), (await roundData(7)).maxReplies, (await roundData(7)).candidates.every(c => c.addressed)], [[alpha.id, beta.id], 2, true]);
  await bid(alpha, 7, 'run', 0.1); await bid(beta, 7, 'decline');
  assert.deepEqual(await states(7, [alpha, beta]), { 'Alpha bot': ['queued', undefined, 'won'], 'Beta bot': ['queued', undefined, 'won'] }, 'a Bot spoken to cannot be passed over by bidding low');
  // A Bot that was named but may not answer unasked still keeps the others out.
  await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 1, targets: [alpha.id, beta.id] }, member);
  await say(member, 8, 'Gamma bot, what do you think?');
  assert.equal((await db.doc(`teams/${teamId}/replyRounds/${messageId(8)}`).get()).exists, false);
  assert.equal((await allTasks()).filter(t => t.messageId === messageId(8)).length, 0);
});

test('a task for a Bot whose Desktop is off waits a day in a group, and the message is still sent; a Bot DM says at once', options, async () => {
  await beat(alpha, false);
  const receipt = await ask(member, 1, [alpha, beta]);
  assert.equal(receipt.tasks.length, 2);
  assert.ok((await chatRef(trip).collection('messages').doc(messageId(1)).get()).exists, 'the message was sent');
  const task = await taskData(taskOf(1)), online = await taskData(taskOf(1, beta));
  assert.equal(task.deadline, now + 24 * 60 * 60 * 1000);
  assert.equal(online.deadline, now + 300000, 'a Bot whose Desktop is on keeps its five minutes');
  assert.equal((await execute('getTask', { taskId: taskOf(1) }, member)).state, 'queued');
  // Members are told it waits for the Desktop (the clients' "is offline and will reply when it's back online"); a task queued behind a Desktop that is on is not.
  assert.equal((await execute('getTask', { taskId: taskOf(1) }, member)).hostWait, true);
  assert.equal((await execute('getTask', { taskId: taskOf(1, beta) }, member)).hostWait, undefined);
  // The Desktop returns within the day and takes the task; one that never does lets it expire.
  now += 60 * 60 * 1000;
  await beat(alpha);
  assert.equal((await execute('prepareTask', { ...alpha.host, taskId: taskOf(1) })).task.state, 'queued');
  now += 23 * 60 * 60 * 1000;
  await assert.rejects(execute('prepareTask', { ...alpha.host, taskId: taskOf(1) }), { code: 'TASK_EXPIRED' });
  const lapsed = await execute('getTask', { taskId: taskOf(1) }, member);
  assert.equal(lapsed.state, 'expired'); assert.equal(lapsed.hostWait, undefined, 'the existing expired state, no longer a wait');
  // A Bot DM, where the Bot is the only one asked, still tells the member that nobody is there.
  await execute('createConversation', { conversationId: dm, kind: 'bot_dm', title: 'Alpha', participantUids: [member.uid], sharedBotIds: [alpha.id] }, member);
  await beat(alpha, false);
  await assert.rejects(execute('submit', { conversationId: dm, messageId: 'dm_msg_0001', changeId: 'dm_chg_0001', text: 'Hi', targets: [alpha.id] }, member), { code: 'HOST_UNAVAILABLE' });
  // A paused deployment is not an absent one: nothing is queued for it.
  await db.doc(`teams/${teamId}/deployments/${gamma.host.deploymentId}`).update({ enabled: false });
  await assert.rejects(ask(member, 2, [gamma]), { code: 'HOST_UNAVAILABLE' });
});

test('a Bot\'s own message never wakes a Bot: no task and no round, whoever it mentions or calls', options, async () => {
  const asked = await ask(member, 1, [beta]);
  const started = await begin(beta, 1);
  const before = (await allTasks()).length;
  const reply = await execute('completeTask', result(started, beta, { text: 'Alpha bot, Gamma bot, @other: your turn?' }), member);
  assert.equal(reply.state, 'completed');
  assert.equal((await allTasks()).length, before);
  assert.deepEqual(await rounds(), []);
  assert.equal(asked.tasks.length, 1);
  // Nor does a message that is not the Chat's: a service message asks nothing. The ids of a Bot's message are refused to members.
  await assert.rejects(ask(member, 2, [alpha], 'hello', { messageId: 'bot_message_0001' }), /./u);
});

test('a human answer to a Bot\'s question is a new message of the Chat: it ends an open round', options, async () => {
  const question = await ask(member, 1, [beta]);
  const started = await begin(beta, 1), waitId = 'wait_rounds_0001';
  await execute('pauseTask', { ...beta.host, taskId: question.tasks[0].taskId, permitId: started.permitId, waitId, requestDigest: 'a'.repeat(64), checkpointDigest: 'b'.repeat(64),
    pausedAt: now, expiresInMs: 600000, dispatchedRequests: 1, inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' },
    kind: 'human_input', question: 'Which day?' }, member);
  await say(other, 2);
  assert.equal((await roundData(2)).state, 'open');
  await execute('answerWait', { conversationId: trip, waitId, messageId: 'answer_msg_0001', changeId: 'answer_chg_0001', text: 'Tuesday' }, member);
  assert.equal((await roundData(2)).state, 'voided');
});

test('the round is the service\'s alone: it has no rule for a client, and what became of a task reaches only the Bot\'s maintainer', options, async () => {
  await say(member, 1);
  await bid(alpha, 1, 'run');
  const round = await roundData(1);
  assert.equal(JSON.stringify(Object.keys(round).sort()), JSON.stringify(['bids', 'candidates', 'conversationId', 'createdAt', 'deadlineAt', 'maxReplies', 'roundId', 'schemaVersion', 'state']));
  // `listHostTasks` carries the state of the round to the Desktop of each Bot, and nothing of the others' bids.
  const row = (await listed(beta)).tasks[0];
  assert.equal(row.round, 'open');
  assert.deepEqual(Object.keys(row).filter(key => /bid|tier|score/u.test(key)), []);
  // A host is told nothing of rounds in a task it asked nothing about: only the member's own tasks list, and without it.
  assert.doesNotMatch(JSON.stringify(await execute('listTasks', {}, member)), /round/u);
});
