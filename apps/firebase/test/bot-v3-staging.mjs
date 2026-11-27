// Bot v3 against the dedicated staging project: the deployed teams API (Hosting -> Cloud Run `dataset-product`), real Firestore indexes and real Auth sessions.
// Three disposable email users each maintain one Bot on a Desktop of their own; the script plays the three Desktops. Staging runs no Cloud Functions, so the
// script settles a reply round at its deadline itself, as the `inboxWork` job does in production.
//   node test/bot-v3-staging.mjs           through the deployed service (its account needs Firestore access), then delete the three users
//   LINGXI_STAGING_LOCAL=1 node ...        the same handler in this process, on staging Firestore and Auth with this machine's credentials
//   LINGXI_STAGING_KEEP=1                  keep the Team and the users' Firestore records (the Auth users are deleted either way)
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { getAuth } from 'firebase-admin/auth';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';
import { FriendsStore } from '../src/friends.mjs';
import { admissionId } from '../src/team-execution-contract.mjs';
import { settleReplyRound } from '../src/reply-rounds.mjs';
import { delegationId, delegationWaitMs } from '../src/delegations.mjs';

const projectId = 'lingxi-dataset-v2-stg-261119';
const base = `https://${projectId}.web.app`;
const webKey = process.env.LINGXI_STAGING_WEB_KEY;
if (!webKey || process.env.FIRESTORE_EMULATOR_HOST || process.env.FIREBASE_AUTH_EMULATOR_HOST) throw new Error('Expected the staging web key and no emulator');
const app = initializeApp({ projectId }), db = getFirestore(app);
const local = process.env.LINGXI_STAGING_LOCAL === '1' ? createTeamsHandler({ store: new TeamsStore(db), friends: new FriendsStore(db),
  verifyToken: token => getAuth(app).verifyIdToken(token, true), origins: [] }) : null;
const results = [], timings = [];
async function step(name, run) {
  const started = Date.now();
  try { await run(); results.push({ name, ok: true, ms: Date.now() - started }); console.log(`ok   ${name}`); }
  catch (error) { results.push({ name, ok: false, error: error.message }); console.log(`FAIL ${name}\n     ${error.message.slice(0, 1500).replaceAll('\n', '\n     ')}\n     ${error.stack?.split('\n').find(line => line.includes('bot-v3-staging'))}`); }
}

async function identity(method, body) {
  const reply = await fetch(`https://identitytoolkit.googleapis.com/v1/accounts:${method}?key=${webKey}`, { method: 'POST',
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), redirect: 'error', signal: AbortSignal.timeout(20000) });
  const value = await reply.json();
  if (!reply.ok) throw new Error(`auth ${method}: ${value.error?.message}`);
  return value;
}
async function user(role, name) {
  const email = `lingxi-botv3-${role}-${randomBytes(5).toString('hex')}@example.test`, password = randomBytes(24).toString('base64url');
  const created = await identity('signUp', { email, password, returnSecureToken: true });
  await identity('update', { idToken: created.idToken, displayName: name, returnSecureToken: false });
  const session = await identity('signInWithPassword', { email, password, returnSecureToken: true });
  return { uid: session.localId, name, token: session.idToken, email, password };
}

class ApiError extends Error { constructor(status, code) { super(`${status} ${code}`); this.status = status; this.code = code; } }
async function call(path, actor, body) {
  const init = { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(30000),
    headers: { Authorization: `Bearer ${actor.token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(body) };
  const began = Date.now();
  const reply = local ? await local(new Request(base + path, init)) : await fetch(base + path, init);
  timings.push([body.action, Date.now() - began]);
  const text = await reply.text();
  let value;
  try { value = JSON.parse(text); } catch { throw new ApiError(reply.status, `NON_JSON ${text.slice(0, 80)}`); }
  if (!reply.ok) throw new ApiError(reply.status, value.error?.code ?? 'UNKNOWN');
  return value;
}
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const owner = await user('owner', 'Staging Owner'), member = await user('member', 'Staging Member'), other = await user('other', 'Staging Other');
const people = [owner, member, other];
const run = randomUUID().slice(0, 8);
let teamId;
const command = (action, payload) => ({ schemaVersion: 1, commandId: `cmd_${randomUUID().replaceAll('-', '')}`, teamId, generation: 1, action, payload });
const execute = (action, payload, actor = owner) => call('/v1/teams/messages', actor, command(action, payload));
const bot = (key, name, maintainer, more = {}) => ({ id: `sharedbot_${key}_${run}`, name, maintainer,
  host: { deploymentId: `deployment_${key}_${run}`, hostId: `host_${key}_${run}`, accountEpoch: 1 }, more });
const alpha = bot('alpha', 'Alpha bot', owner, { delegateTo: [], scope: { localApps: [{ id: 'notes', name: 'Notes' }], datasets: [] } });
const beta = bot('beta', 'Beta bot', member, { acceptDelegation: 'team_bots' });
const gamma = bot('gamma', 'Gamma bot', other);
alpha.more.delegateTo = [beta.id]; alpha.more.delegation = { daily: 10 };
const bots = [alpha, beta, gamma], ids = bots.map(b => b.id);
const trip = `chat_trip_${run}`;
const asking = 'Does anyone know a quiet hotel near the station?';
let serial = 0;
const nextMessage = () => { const n = ++serial; return { messageId: `msg_${run}_${String(n).padStart(3, '0')}`, changeId: `chg_${run}_${String(n).padStart(3, '0')}` }; };
const say = (actor, text = asking) => { const ids_ = nextMessage();
  return call('/v1/teams/messages', actor, { action: 'mutate', teamId, generation: 1, conversationId: trip, ...ids_, operation: 'create', text }).then(() => ids_.messageId); };
const ask = async (actor, targets, text = asking) => { const ids_ = nextMessage();
  const receipt = await execute('submit', { conversationId: trip, ...ids_, text, targets: targets.map(b => b.id) }, actor); return { messageId: ids_.messageId, receipt }; };
const chat = () => db.doc(`teams/${teamId}/conversations/${trip}`);
const taskOf = (messageId, b) => admissionId(messageId, b.id);
const task = async id => (await db.doc(`teams/${teamId}/tasks/${id}`).get()).data();
const round = async messageId => (await db.doc(`teams/${teamId}/replyRounds/${messageId}`).get()).data();
const posted = async () => (await chat().collection('messages').orderBy('seq').get()).docs.map(doc => ({ id: doc.id, ...doc.data() }));
const beat = () => Promise.all(bots.map(b => execute('heartbeatDeployment', { ...b.host, ready: true }, b.maintainer)));
const bid = (b, messageId, tier, score, source = 'decision') => execute('bidReply', { ...b.host, taskId: taskOf(messageId, b), tier, ...(score === undefined ? {} : { score }), source }, b.maintainer);
const start = async (b, taskId) => execute('startTask', { ...b.host, taskId, expectedLastChangeSeq: (await chat().get()).data().lastChangeSeq }, b.maintainer);
const result = (started, b, text, more = {}) => ({ ...b.host, taskId: started.task.taskId, permitId: started.permitId, outcome: 'completed', text, finishedAt: Date.now(),
  inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' }, ...more });
const states = async messageId => Object.fromEntries(await Promise.all(bots.map(async b => { const t = await task(taskOf(messageId, b)); return [b.name, t && [t.state, t.declineReason, t.round?.outcome]]; })));
const rejects = async (promise, code) => { await assert.rejects(promise, error => { assert.equal(error.code, code); return true; }); };

try {
  await step('team: create, invite, join', async () => {
    teamId = `team_botv3_${run}`;
    await call('/v1/teams', owner, { action: 'create', teamId, name: `Bot v3 staging ${run}` });
    const { code } = await call('/v1/teams', owner, { action: 'invite', teamId, requestId: `invite_${run}`, role: 'member', maxUses: 5, ttlSeconds: 3600 });
    for (const actor of [member, other]) await call('/v1/teams', actor, { action: 'join', code, requestId: `join_${run}_${actor.uid.slice(0, 6)}` });
  });
  await step('publish three Bots of three maintainers (scope, delegation opt-in) and bring their Desktops online', async () => {
    for (const b of bots) {
      await execute('publishSharedBot', { sharedBotId: b.id, expectedRevision: 0, name: b.name, description: `${b.name} on staging`, usePolicy: { mode: 'all' }, autoReply: { daily: 50 },
        deployment: { ...b.host, sharedBotId: b.id, revision: 1, owner: { uid: b.maintainer.uid, generation: 1 }, instanceId: `instance_${b.id}`,
          definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
          capabilities: { durableWait: true, writeOperations: false, delegation: true }, enabled: true },
        inputs: [{ kind: 'persona', resourceId: `persona_${run}`, revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
        quotas: { team: 100, member: 20, bot: 100 }, ...b.more }, b.maintainer);
    }
    await beat();
    const directory = await execute('listSharedBots', {}, other);
    const card = id => directory.bots.find(entry => entry.sharedBotId === id);
    assert.deepEqual(ids.map(id => Boolean(card(id))), [true, true, true]);
    assert.deepEqual(card(alpha.id).scope?.localApps?.map(app => app.id), ['notes']);
    await execute('createConversation', { conversationId: trip, kind: 'group', title: 'Trip', participantUids: [member.uid, other.uid], sharedBotIds: ids }, member);
    await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 0, targets: ids }, member);
  });
  await step('round: an unasked message opens a round, no task starts before it settles, the last bid settles it (tier before score)', async () => {
    const said = Date.now(), m = await say(member);
    // This machine is seconds from us-central1: the three Desktops bid at once to stay inside the window.
    await Promise.all([
      execute('recordTriage', { ...alpha.host, taskId: taskOf(m, alpha), action: 'run', source: 'decision', probability: 0.9 }, owner).then(() => bid(alpha, m, 'may_decline', 0.99)),
      bid(beta, m, 'run', 0.5), bid(gamma, m, 'decline', undefined, 'quiet')]);
    const open = await round(m);
    console.log(`     message -> last bid: ${Date.now() - said} ms of the ${open.deadlineAt - open.createdAt} ms window`);
    assert.deepEqual([open.state, open.maxReplies, open.candidates.map(c => c.sharedBotId).sort(), Object.keys(open.bids).length], ['resolved', 1, [...ids].sort(), 3]);
    assert.equal(open.expireAt.toMillis() - open.createdAt, 7 * 86400000);
    assert.deepEqual(await bid(alpha, m, 'run', 1), { taskId: taskOf(m, alpha), round: 'lost' }, 'a later bid is told what became of the task');
    assert.deepEqual(await states(m), { 'Alpha bot': ['declined', 'coordination', 'lost'], 'Beta bot': ['queued', undefined, 'won'], 'Gamma bot': ['declined', 'triage', 'abstained'] });
    const started = await start(beta, taskOf(m, beta));
    assert.equal(started.execute, true);
    assert.equal((await execute('completeTask', result(started, beta, 'The Station Inn is quiet.'), member)).state, 'completed');
    assert.equal((await posted()).at(-1).author.sharedBotId, beta.id);
    const row = (await execute('listTriage', { sharedBotId: alpha.id }, owner)).items.find(item => item.taskId === taskOf(m, alpha));
    assert.deepEqual([row.state, row.round], ['declined', { outcome: 'lost', tier: 'may_decline' }]);
    const view = JSON.stringify(await execute('getTask', { taskId: taskOf(m, alpha) }, other));
    assert.doesNotMatch(view, /"round"|coordination|"tier"|may_decline/u);
  });
  await step('round: Desktops that never bid abstain at the deadline (the job the service queues, run here on the real clock)', async () => {
    await beat();
    const m = await say(other, 'Where should we eat tonight?');
    const [, hosted] = await Promise.all([bid(alpha, m, 'run', 0.7), execute('listHostTasks', beta.host, member), rejects(start(gamma, taskOf(m, gamma)), 'ROUND_OPEN')]);
    assert.equal(hosted.tasks.find(t => t.taskId === taskOf(m, beta))?.round, 'open', 'the host is told the round has not chosen yet');
    const { deadlineAt } = await round(m);
    await assert.rejects(settleReplyRound(db, Date.now, { teamId, roundId: m, phase: 'bids' }), /REPLY_ROUND_NOT_DUE/u);
    await sleep(Math.max(0, deadlineAt - Date.now()) + 1500);
    assert.deepEqual(await settleReplyRound(db, Date.now, { teamId, roundId: m, phase: 'bids' }), { settled: true });
    assert.deepEqual(await states(m), { 'Alpha bot': ['queued', undefined, 'won'], 'Beta bot': ['declined', 'coordination', 'abstained'], 'Gamma bot': ['declined', 'coordination', 'abstained'] });
    assert.deepEqual(await bid(beta, m, 'run', 1), { taskId: taskOf(m, beta), round: 'abstained' });
    const started = await start(alpha, taskOf(m, alpha));
    await execute('completeTask', result(started, alpha, 'Try the noodle bar.'), owner);
  });
  await step('mentions: a message that asks one Bot opens no round and makes no task for the others', async () => {
    const { messageId, receipt } = await ask(member, [gamma], 'Gamma, what time is checkout?');
    assert.deepEqual(receipt.tasks.map(t => t.taskId), [taskOf(messageId, gamma)]);
    assert.equal(await round(messageId), undefined);
    assert.equal(await task(taskOf(messageId, alpha)), undefined);
    const started = await start(gamma, taskOf(messageId, gamma));
    await execute('completeTask', result(started, gamma, 'Checkout is at eleven.'), other);
  });
  await step('held reply: the maintainer reads the draft, approves one (posted) and rejects one (not posted); nobody else may', async () => {
    await beat();
    const first = await ask(member, [alpha], 'Alpha, book us a table.');
    const started = await start(alpha, taskOf(first.messageId, alpha));
    const before = (await posted()).length;
    const held = await execute('holdReply', result(started, alpha, 'Booked for eight.'), owner);
    assert.equal(held.state, 'awaiting_approval');
    assert.equal((await posted()).length, before);
    const drafts = await execute('listHeldReplies', { sharedBotId: alpha.id }, owner);
    assert.deepEqual(drafts.replies.map(item => [item.taskId, item.text]), [[taskOf(first.messageId, alpha), 'Booked for eight.']]);
    await rejects(execute('listHeldReplies', { sharedBotId: alpha.id }, member), 'MAINTAINER_REQUIRED');
    await rejects(execute('approveReply', { taskId: taskOf(first.messageId, alpha) }, member), 'MAINTAINER_REQUIRED');
    const approved = await execute('approveReply', { taskId: taskOf(first.messageId, alpha) }, owner);
    assert.equal(approved.state, 'completed');
    assert.deepEqual([(await posted()).at(-1).author.sharedBotId, (await posted()).at(-1).text], [alpha.id, 'Booked for eight.']);
    const second = await ask(member, [alpha], 'Alpha, cancel the table.');
    const again = await start(alpha, taskOf(second.messageId, alpha));
    await execute('holdReply', result(again, alpha, 'Cancelled.'), owner);
    const count = (await posted()).length;
    await execute('rejectReply', { taskId: taskOf(second.messageId, alpha) }, owner);
    assert.equal((await posted()).length, count);
    assert.equal((await task(taskOf(second.messageId, alpha))).state, 'declined');
  });
  await step('delegation that waits: Alpha asks Beta, Beta hands its reply back instead of posting it, Alpha goes on with it', async () => {
    await beat();
    const { messageId } = await ask(member, [alpha], 'Alpha, find out what the 2026 figures were.');
    const taskId = taskOf(messageId, alpha), started = await start(alpha, taskId);
    assert.deepEqual((started.delegates ?? []).map(entry => entry.sharedBotId), [beta.id]);
    const waitId = `wait_${run}_01`, logicalStepId = `${taskId}/step_1`;
    const paused = await execute('pauseTask', { ...alpha.host, taskId, permitId: started.permitId, waitId, kind: 'delegation',
      delegation: { logicalStepId, target: beta.id, request: 'Find the 2026 figures.' }, requestDigest: 'c'.repeat(64), checkpointDigest: 'd'.repeat(64),
      pausedAt: Date.now(), expiresInMs: delegationWaitMs, dispatchedRequests: 2, inputGrantIds: started.inputs.map(g => g.grantId), outputAudience: { mode: 'published_inputs' } }, owner);
    assert.equal(paused.state, 'waiting');
    const record = (await db.doc(`teams/${teamId}/delegations/${delegationId(taskId, logicalStepId)}`).get()).data();
    assert.deepEqual([record.targetBotId, record.expectReply, record.state], [beta.id, true, 'open']);
    assert.equal(await round(record.messageId), undefined, 'a delegation message opens no round');
    const lines = (await posted()).length;
    const b = await start(beta, record.targetTaskId);
    const done = await execute('completeTask', result(b, beta, 'Revenue was 12.4 million.'), member);
    assert.deepEqual([done.state, done.replyId], ['completed', null]);
    assert.equal((await posted()).length, lines, 'the reply was handed back, not posted');
    assert.doesNotMatch(JSON.stringify(await execute('listTasks', { conversationId: trip }, other)), /12\.4/u);
    const prepared = await execute('prepareTask', { ...alpha.host, taskId }, owner);
    assert.deepEqual(prepared.wait?.delegation?.text, 'Revenue was 12.4 million.');
    const continued = await execute('continueTask', { ...alpha.host, taskId, expectedEpoch: 1, checkpointDigest: 'd'.repeat(64), reason: 'answer', dispatchedRequests: 3 }, owner);
    assert.equal(continued.execute, true);
    await execute('completeTask', result(continued, alpha, 'Per Beta, revenue was 12.4 million.'), owner);
    assert.equal((await posted()).at(-1).text, 'Per Beta, revenue was 12.4 million.');
  });
  await step('scope: a member of the Chat narrows a Bot within what it published; more than that is refused', async () => {
    assert.deepEqual(await execute('setConversationBotScope', { conversationId: trip, sharedBotId: alpha.id, expectedRevision: 0, scope: { localApps: ['notes'] } }, member), { revision: 1 });
    await rejects(execute('setConversationBotScope', { conversationId: trip, sharedBotId: alpha.id, expectedRevision: 1, scope: { localApps: ['mail'] } }, member), 'SCOPE_NOT_PUBLISHED');
    await rejects(execute('setConversationBotScope', { conversationId: trip, sharedBotId: alpha.id, expectedRevision: 0, scope: null }, member), 'BOT_SCOPE_REVISION_CONFLICT');
    const listed = await call('/v1/teams/messages', member, { action: 'list', teamId, generation: 1 });
    assert.deepEqual(listed.conversations.find(c => c.id === trip)?.botScopes?.[alpha.id], { revision: 1, localApps: ['notes'] });
  });
  await step('chat settings: maxReplies 2 lets two Bots win one round', async () => {
    await beat();
    await execute('setConversationAutoReply', { conversationId: trip, expectedRevision: 1, targets: ids, maxReplies: 2 }, member);
    const m = await say(member, 'Any museum tips?');
    assert.equal((await round(m)).maxReplies, 2);
    await Promise.all([bid(alpha, m, 'run', 0.6), bid(beta, m, 'run', 0.9), bid(gamma, m, 'decline', undefined, 'quiet')]);
    const settled = await round(m);
    assert.deepEqual([settled.state, settled.winners.length], ['resolved', 2]);
    for (const b of [alpha, beta]) { const s = await start(b, taskOf(m, b)); await execute('completeTask', result(s, b, `${b.name}: the city museum.`), b.maintainer); }
  });
  await step('history: a member reads the Chat as the clients do', async () => {
    const history = await call('/v1/teams/messages', other, { action: 'history', teamId, conversationId: trip, generation: 1 });
    const texts = history.messages.map(message => message.text);
    for (const text of ['The Station Inn is quiet.', 'Booked for eight.', 'Per Beta, revenue was 12.4 million.']) assert.ok(texts.includes(text), text);
    assert.ok(!texts.includes('Cancelled.') && !texts.includes('Revenue was 12.4 million.'));
  });
} finally {
  for (const actor of people) await identity('signInWithPassword', { email: actor.email, password: actor.password, returnSecureToken: true })
    .then(session => identity('delete', { idToken: session.idToken })).catch(error => console.log(`cleanup: ${error.message}`));
  // What the run made in Firestore goes with its users: the Team and each user's index of it.
  if (teamId && process.env.LINGXI_STAGING_KEEP !== '1') {
    await db.recursiveDelete(db.doc(`teams/${teamId}`));
    for (const actor of people) await db.recursiveDelete(db.doc(`users/${actor.uid}`));
  }
  const failed = results.filter(r => !r.ok);
  const by = {};
  for (const [action, ms] of timings) (by[action] ??= []).push(ms);
  console.log(Object.entries(by).map(([action, list]) => `${action} n=${list.length} median=${list.sort((a, b) => a - b)[list.length >> 1]}ms max=${list.at(-1)}ms`).join('\n'));
  console.log(JSON.stringify({ teamId, run, passed: results.length - failed.length, failed: failed.length }));
  if (failed.length) process.exitCode = 1;
  await db.terminate();
}
