import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';
import { TeamMessagesStore } from '../src/team-messages.mjs';
import { HostedBotServices } from '../src/hosted-bot-service.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-hosted-service', serviceId = 'service_travel', sharedBotId = 'sharedbot_01';
const publisher = { uid: 'publisher', name: 'Pat Publisher' }, alice = { uid: 'alice', name: 'Alice' }, bob = { uid: 'bob', name: 'Bob' }, eve = { uid: 'eve', name: 'Eve' };
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
let app, db, services, messages, now, serial = 0;
const service = (action, fields = {}, actor = publisher) => services.execute(actor, { action, ...(action === 'listServices' ? {} : { serviceId }), ...fields });
/** A ledger command in the service's scope. */
const ledger = (action, payload, actor, generation = 1, commandId = `command_${++serial}`) =>
  messages.execute(actor, { schemaVersion: 1, commandId, serviceId, generation, action, payload });
const read = (action, fields, actor, generation = 1) => messages.execute(actor, { action, serviceId, generation, ...fields });
const definition = (override = {}) => ({ sharedBotId, name: 'Travel helper', description: 'Answers travel questions', audience: { mode: 'public' },
  limits: { userConcurrent: 2, serviceConcurrent: 3 }, retention: 'Sessions are kept for 30 days.', ...override });
const publication = (override = {}) => ({ sharedBotId, expectedRevision: 0, name: 'Travel helper', description: 'Answers travel questions',
  usePolicy: { mode: 'all' }, deployment: { ...host, sharedBotId, revision: 1, owner: { uid: publisher.uid, generation: 1 },
    instanceId: 'instance_01', definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: false, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ kind: 'persona', resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 3, bot: 100 }, ...override });
const session = (actor, conversationId = `session_${actor.uid}`) => ({ conversationId, kind: 'bot_dm', title: 'Travel', participantUids: [actor.uid], sharedBotIds: [sharedBotId] });
const ask = (conversationId, id = ++serial) => ({ conversationId, messageId: `message_${id}`, changeId: `change_${id}`, text: 'What should we pack?', targets: [sharedBotId] });
/** A service that is published, online and served. */
async function online(override) {
  await service('createService', definition(override));
  await ledger('publishSharedBot', publication(), publisher);
  await ledger('heartbeatDeployment', { ...host, ready: true }, publisher);
  await service('setServiceOnline', { online: true });
}
async function enter(actor) {
  const entered = await service('enterService', { acknowledged: true }, actor);
  await ledger('createConversation', session(actor), actor, entered.generation);
  return entered.generation;
}
/** The publisher's host answers a task. */
async function answer(taskId, conversationId, text = 'Pack your passports.') {
  const lastChangeSeq = (await db.doc(`services/${serviceId}/conversations/${conversationId}`).get()).data().lastChangeSeq;
  const started = await ledger('startTask', { ...host, taskId, expectedLastChangeSeq: lastChangeSeq }, publisher);
  assert.equal(started.execute, true);
  return ledger('completeTask', { ...host, taskId, permitId: started.permitId, outcome: 'completed', text, finishedAt: now,
    inputGrantIds: started.inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } }, publisher);
}
before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  assert.match(process.env.FIRESTORE_EMULATOR_HOST, /^(127\.0\.0\.1|localhost):\d+$/u);
  app = initializeApp({ projectId }, 'hosted-bot-service-tests'); db = getFirestore(app);
});
beforeEach(async () => {
  if (!db) return;
  for (const name of ['services', 'serviceUsers', 'serviceReports', 'teams', 'users']) await db.recursiveDelete(db.collection(name));
  now = Date.UTC(2026, 10, 13); services = new HostedBotServices(db, () => now); messages = new TeamMessagesStore(db, () => now);
});
after(async () => { await db?.terminate(); if (app) await deleteApp(app); });

test('service: a new service is off; a caller is told why it cannot answer, and never anything of the publisher beyond a name', options, async () => {
  const created = await service('createService', definition());
  assert.equal(created.online, false);
  assert.equal(created.cost, 'publisher');
  assert.deepEqual(await service('createService', definition()), created);
  await assert.rejects(service('createService', definition(), alice), { code: 'SERVICE_ID_CONFLICT' });

  const before = await service('getService', {}, alice);
  assert.equal(before.availability, 'disabled');
  assert.equal(before.entered, false);
  await service('setServiceOnline', { online: true });
  assert.equal((await service('getService', {}, alice)).availability, 'not_ready');
  await ledger('publishSharedBot', publication(), publisher);
  assert.equal((await service('getService', {}, alice)).availability, 'host_offline');
  await ledger('heartbeatDeployment', { ...host, ready: true }, publisher);
  const view = await service('getService', {}, alice);
  assert.deepEqual(view, { serviceId, name: 'Travel helper', description: 'Answers travel questions', publisherName: 'Pat Publisher', sharedBotId,
    availability: 'available', entered: false, cost: 'publisher', visibility: 'requester_and_publisher', retention: 'Sessions are kept for 30 days.',
    limits: { userConcurrent: 2, userDaily: 3, userDailyLeft: 3 } });
  // The host's heartbeat runs out: the service says so instead of taking a request it cannot answer.
  now += 91000;
  assert.equal((await service('getService', {}, alice)).availability, 'host_offline');
  const generation = await enter(alice);
  await assert.rejects(ledger('submit', ask('session_alice'), alice, generation), { code: 'HOST_UNAVAILABLE' });
});

test('service: a caller enters by agreeing, asks in a session of their own, and gets the answer; nobody else but the publisher reads it', options, async () => {
  await online();
  // Nothing in the scope is usable before entering, and entering needs the caller's agreement to the disclosure.
  await assert.rejects(ledger('createConversation', session(alice), alice), { code: 'SERVICE_ENTRY_REQUIRED' });
  await assert.rejects(services.execute(alice, { action: 'enterService', serviceId }), { name: 'ZodError' });
  const generation = await enter(alice);
  assert.equal(generation, 1);
  await enter(bob);
  assert.equal((await service('enterService', { acknowledged: true }, alice)).generation, 1);

  const receipt = await ledger('submit', ask('session_alice'), alice);
  assert.equal(receipt.tasks.length, 1);
  const task = (await db.doc(`services/${serviceId}/tasks/${receipt.tasks[0].taskId}`).get()).data();
  // The scope is the service: the requester is the caller with their entitlement, and no Team is named anywhere.
  assert.deepEqual(task.requester, { uid: 'alice', generation: 1 });
  assert.equal((await db.collection('teams').get()).size, 0);
  await answer(receipt.tasks[0].taskId, 'session_alice');

  const synced = await read('sync', { conversationId: 'session_alice', after: 0 }, alice);
  assert.deepEqual(synced.messages.map(message => [message.author.kind, message.text]), [['user', 'What should we pack?'], ['shared_bot', 'Pack your passports.']]);
  assert.deepEqual((await read('list', {}, alice)).conversations.map(c => c.id), ['session_alice']);
  // Another caller of the same Bot sees their own session only.
  assert.deepEqual((await read('list', {}, bob)).conversations.map(c => c.id), ['session_bob']);
  await assert.rejects(read('sync', { conversationId: 'session_alice', after: 0 }, bob), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(ledger('submit', ask('session_alice'), bob), { code: 'CONVERSATION_REQUIRED' });
  await assert.rejects(ledger('getTask', { taskId: receipt.tasks[0].taskId }, bob), { code: 'TASK_REQUIRED' });
  // The publisher maintains the Bot and reads its sessions, as each caller was told; it cannot write in them.
  assert.equal((await read('list', {}, publisher)).conversations.length, 2);
  assert.equal((await read('sync', { conversationId: 'session_alice', after: 0 }, publisher)).messages.length, 2);
  await assert.rejects(ledger('submit', ask('session_alice'), publisher), { code: 'SERVICE_SCOPE_UNSUPPORTED' });
  // Someone who never entered reads nothing.
  await assert.rejects(read('list', {}, eve), { code: 'SERVICE_ENTRY_REQUIRED' });
  assert.deepEqual((await service('listServices', {}, alice)).entered.map(s => s.serviceId), [serviceId]);
  assert.deepEqual((await service('listServices', {}, publisher)).published.map(s => s.serviceId), [serviceId]);
});

test('service: the scope takes only what it lists; nothing made for a Team reaches it and the two are never mistaken for each other', options, async () => {
  await online();
  const generation = await enter(alice);
  // A caller cannot publish into the scope, see a directory, share a list, hand a Bot over, attach a file or open a group.
  const refused = [];
  for (const [action, payload] of [['publishSharedBot', publication({ sharedBotId: 'sharedbot_evil', deployment: { ...publication().deployment, sharedBotId: 'sharedbot_evil', owner: { uid: 'alice', generation: 1 } } })],
    ['listSharedBots', {}], ['readList', { sharedBotId }], ['heartbeatDeployment', { ...host, ready: true }], ['listHostTasks', { ...host }],
    ['getTransfer', { transferId: 'transfer_01' }], ['setConversationPolicy', { conversationId: 'session_alice', expectedRevision: 1, targets: [] }],
    ['readArtifact', { artifactId: 'artifact_01' }], ['acceptTransfer', { transferId: 'transfer_01' }]]) {
    refused.push([action, await ledger(action, payload, alice, generation).then(() => 'accepted', error => error.code ?? error.name)]);
  }
  assert.deepEqual(refused.filter(([, code]) => code !== 'SERVICE_SCOPE_UNSUPPORTED'), []);
  await assert.rejects(ledger('createConversation', { ...session(alice, 'session_group'), kind: 'group', participantUids: ['alice', 'bob'] }, alice), { code: 'INVALID_SERVICE_SESSION' });
  await assert.rejects(read('mutate', { conversationId: 'session_alice', messageId: 'message_x1', changeId: 'change_x1', operation: 'create', text: 'hi' }, alice), { code: 'SERVICE_SCOPE_UNSUPPORTED' });
  // The publisher's own Team-only commands are not offered either, and its publication must be the service's Bot for everyone in it.
  await assert.rejects(ledger('readList', { sharedBotId }, publisher), { code: 'SERVICE_SCOPE_UNSUPPORTED' });
  await assert.rejects(ledger('publishSharedBot', publication({ expectedRevision: 1, usePolicy: { mode: 'listed', members: [{ uid: 'publisher', generation: 1 }] } }), publisher), { code: 'INVALID_SERVICE_PUBLICATION' });
  await assert.rejects(ledger('publishSharedBot', publication({ sharedBotId: 'sharedbot_02', deployment: { ...publication().deployment, sharedBotId: 'sharedbot_02' } }), publisher), { code: 'INVALID_SERVICE_PUBLICATION' });

  // A service id is not a Team id and a Team id is not a service id, whoever asks.
  await assert.rejects(messages.execute(alice, { schemaVersion: 1, commandId: 'command_team01', teamId: serviceId, generation: 1, action: 'listSharedBots', payload: {} }), { code: 'MEMBERSHIP_REQUIRED' });
  await assert.rejects(messages.execute(alice, { action: 'list', teamId: serviceId, generation: 1 }), { code: 'MEMBERSHIP_REQUIRED' });
  const teams = new TeamsStore(db, () => now);
  await teams.execute(alice, { action: 'create', teamId: 'team_of_alice', name: 'Alice team' });
  await assert.rejects(messages.execute(alice, { schemaVersion: 1, commandId: 'command_svc001', serviceId: 'team_of_alice', generation: 1, action: 'listTasks', payload: {} }), { code: 'NOT_FOUND' });
  await assert.rejects(services.execute(alice, { action: 'getService', serviceId: 'team_of_alice' }), { code: 'NOT_FOUND' });
  // A command naming both scopes is not a command.
  await assert.rejects(messages.execute(alice, { schemaVersion: 1, commandId: 'command_both01', serviceId, teamId: 'team_of_alice', generation: 1, action: 'listTasks', payload: {} }), { name: 'ZodError' });
});

test('service: the off switch, the daily quota and the concurrency limits each stop a request with their own reason', options, async () => {
  await online({ limits: { userConcurrent: 1, serviceConcurrent: 2 } });
  const generation = await enter(alice);
  await ledger('createConversation', session(alice, 'session_alice_2'), alice, generation);
  await enter(bob); await enter(eve);

  // One request of a caller at a time, across their sessions. Sending the same message again is not a second request.
  const first = ask('session_alice'), receipt = await ledger('submit', first, alice);
  assert.deepEqual(await ledger('submit', first, alice), receipt);
  await assert.rejects(ledger('submit', ask('session_alice_2'), alice), { code: 'SERVICE_USER_BUSY' });
  // Two requests in the whole service at a time.
  const second = await ledger('submit', ask('session_bob'), bob);
  await assert.rejects(ledger('submit', ask('session_eve'), eve), { code: 'SERVICE_BUSY' });
  // A refused request left nothing behind: no message, no task, no quota used.
  assert.equal((await db.collection(`services/${serviceId}/conversations/session_eve/messages`).get()).size, 0);
  assert.equal((await db.collection(`services/${serviceId}/tasks`).get()).size, 2);
  // An answered request frees its place.
  await answer(receipt.tasks[0].taskId, 'session_alice');
  await ledger('submit', ask('session_eve'), eve);
  await assert.rejects(ledger('submit', ask('session_alice_2'), alice), { code: 'SERVICE_BUSY' });
  await answer(second.tasks[0].taskId, 'session_bob');

  // The switch: no new request, and the host is given no further work. Reading stays.
  await service('setServiceOnline', { online: false });
  await assert.rejects(ledger('submit', ask('session_bob'), bob), { code: 'SERVICE_DISABLED' });
  const waiting = (await db.collection(`services/${serviceId}/tasks`).where('state', '==', 'queued').get()).docs[0].id;
  await assert.rejects(ledger('startTask', { ...host, taskId: waiting, expectedLastChangeSeq: 1 }, publisher), { code: 'SERVICE_DISABLED' });
  assert.equal((await service('getService', {}, bob)).availability, 'disabled');
  assert.equal((await read('sync', { conversationId: 'session_bob', after: 0 }, bob)).messages.length, 2);
  await service('setServiceOnline', { online: true });

  // The per-person daily quota is the ledger's, counted for this scope alone.
  now += 600000;
  await ledger('heartbeatDeployment', { ...host, ready: true }, publisher);
  for (let i = 0; i < 2; i++) {
    const next = await ledger('submit', ask('session_bob'), bob);
    await answer(next.tasks[0].taskId, 'session_bob');
  }
  await assert.rejects(ledger('submit', ask('session_bob'), bob), { code: 'REQUEST_QUOTA_EXCEEDED' });
  const view = await service('getService', {}, bob);
  assert.equal(view.availability, 'user_quota_exhausted');
  assert.equal(view.limits.userDailyLeft, 0);
  assert.equal((await service('getService', {}, alice)).availability, 'available');
});

test('service: narrowing the audience ends access and what was waiting; coming back is a new entitlement', options, async () => {
  await online({ audience: { mode: 'users', uids: ['alice', 'bob'] } });
  // To anyone the audience does not name, the service does not exist.
  await assert.rejects(service('getService', {}, eve), { code: 'NOT_FOUND' });
  await assert.rejects(service('enterService', { acknowledged: true }, eve), { code: 'NOT_FOUND' });
  await assert.rejects(read('list', {}, eve), { code: 'NOT_FOUND' });
  await enter(alice); await enter(bob);
  const waiting = await ledger('submit', ask('session_alice'), alice);

  const current = await service('getService');
  await assert.rejects(service('updateService', { expectedRevision: current.revision + 5, audience: { mode: 'users', uids: ['bob'] } }), { code: 'SERVICE_REVISION_CONFLICT' });
  await assert.rejects(service('updateService', { expectedRevision: current.revision, audience: { mode: 'users', uids: ['bob'] } }, alice), { code: 'PUBLISHER_REQUIRED' });
  const narrowed = await service('updateService', { expectedRevision: current.revision, audience: { mode: 'users', uids: ['bob'] } });
  assert.equal(narrowed.sweep, undefined);
  // Alice's old page and old generation get nothing: no reading, no asking, no entering.
  await assert.rejects(read('sync', { conversationId: 'session_alice', after: 0 }, alice), { code: 'NOT_FOUND' });
  await assert.rejects(ledger('submit', ask('session_alice'), alice), { code: 'NOT_FOUND' });
  await assert.rejects(service('getService', {}, alice), { code: 'NOT_FOUND' });
  assert.deepEqual((await service('listServices', {}, alice)).entered, []);
  // What she had waiting is not started for her.
  await assert.rejects(ledger('startTask', { ...host, taskId: waiting.tasks[0].taskId, expectedLastChangeSeq: waiting.changeSeq }, publisher), { code: 'TASK_REVOKED' });
  // Bob is untouched.
  const kept = await ledger('submit', ask('session_bob'), bob);
  await answer(kept.tasks[0].taskId, 'session_bob');

  // Named again, she enters anew: a new generation, and the old one is stale.
  const reopened = await service('updateService', { expectedRevision: narrowed.revision, audience: { mode: 'public' } });
  assert.equal(reopened.audience.mode, 'public');
  await assert.rejects(ledger('submit', ask('session_alice'), alice), { code: 'SERVICE_ENTRY_REQUIRED' });
  const again = await service('enterService', { acknowledged: true }, alice);
  assert.equal(again.generation, 2);
  await assert.rejects(ledger('submit', ask('session_alice'), alice, 1), { code: 'STALE_MEMBERSHIP' });
  await ledger('submit', ask('session_alice'), alice, 2);

  // Leaving is the caller's own way out; public to named people removes everyone not named.
  assert.deepEqual(await service('leaveService', {}, bob), { left: true });
  await assert.rejects(read('list', {}, bob), { code: 'SERVICE_ENTRY_REQUIRED' });
  await enter(eve);
  await service('updateService', { expectedRevision: reopened.revision, audience: { mode: 'users', uids: ['eve'] } });
  assert.deepEqual((await db.collection(`services/${serviceId}/members`).get()).docs.map(doc => doc.id).sort(), ['eve', 'publisher']);
});

test('service: withdrawing ends it for everyone; a report goes to the platform and not to the publisher', options, async () => {
  await online();
  await enter(alice);
  const receipt = await ledger('submit', ask('session_alice'), alice);
  await answer(receipt.tasks[0].taskId, 'session_alice');

  const report = { reportId: 'report_0001', reason: 'deceptive', text: 'It claimed to be a travel agency.', sessionId: 'session_alice' };
  assert.deepEqual(await service('reportService', report, alice), { reported: true });
  assert.deepEqual(await service('reportService', report, alice), { reported: true });
  await assert.rejects(service('reportService', { ...report, text: 'Something else' }, alice), { code: 'REPORT_ID_CONFLICT' });
  // A session can be named only by the person whose session it is.
  await assert.rejects(service('reportService', { ...report, reportId: 'report_bob01' }, bob), { code: 'CONVERSATION_REQUIRED' });
  const stored = (await db.collection('serviceReports').get()).docs.map(doc => doc.data());
  assert.equal(stored.length, 1);
  assert.equal(stored[0].reporter, 'alice');
  assert.equal(stored[0].publisher, 'publisher');
  // Nothing the publisher can ask for carries a report or who made it.
  assert.equal(JSON.stringify([await service('getService'), await service('listServices')]).includes('report'), false);
  for (let i = 2; i <= 5; i++) await service('reportService', { reportId: `report_000${i}`, reason: 'spam', text: '' }, alice);
  await assert.rejects(service('reportService', { reportId: 'report_0006', reason: 'spam', text: '' }, alice), { code: 'REPORT_LIMIT' });

  await assert.rejects(service('withdrawService', {}, alice), { code: 'PUBLISHER_REQUIRED' });
  assert.equal((await service('withdrawService')).state, 'withdrawn');
  await assert.rejects(ledger('submit', ask('session_alice'), alice), { code: 'SERVICE_WITHDRAWN' });
  await assert.rejects(read('sync', { conversationId: 'session_alice', after: 0 }, alice), { code: 'SERVICE_WITHDRAWN' });
  await assert.rejects(ledger('heartbeatDeployment', { ...host, ready: true }, publisher), { code: 'SERVICE_WITHDRAWN' });
  await assert.rejects(service('setServiceOnline', { online: true }), { code: 'SERVICE_WITHDRAWN' });
  await assert.rejects(service('enterService', { acknowledged: true }, bob), { code: 'SERVICE_WITHDRAWN' });
  assert.equal((await service('getService', {}, alice)).availability, 'withdrawn');
  assert.deepEqual((await service('listServices', {}, alice)).entered, []);
  // A report can still be made about a service that is gone.
  assert.deepEqual(await service('reportService', { reportId: 'report_late1', reason: 'other', text: 'Withdrawn after complaints.' }, bob), { reported: true });
});

test('service: the HTTP API takes service commands at /v1/services and answers with the service\'s own codes', options, async () => {
  await online();
  const handler = createTeamsHandler({ store: new TeamsStore(db, () => now), verifyToken: async token => ({ uid: token, name: token }),
    createCustomToken: async () => 'unused', origins: [] });
  const post = async (path, token, body) => {
    const response = await handler(new Request(`https://teams.internal${path}`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(body) }));
    return { status: response.status, body: await response.json() };
  };
  assert.equal((await post('/v1/services', 'alice', { action: 'getService', serviceId })).body.availability, 'available');
  assert.deepEqual(await post('/v1/services', 'alice', { action: 'getService', serviceId: 'service_missing' }), { status: 404, body: { error: { code: 'NOT_FOUND' } } });
  assert.deepEqual(await post('/v1/services', 'alice', { action: 'setServiceOnline', serviceId, online: false }), { status: 403, body: { error: { code: 'PUBLISHER_REQUIRED' } } });
  assert.equal((await post('/v1/services', 'alice', { action: 'enterService', serviceId, acknowledged: true })).body.generation, 1);
  assert.deepEqual(await post('/v1/teams/messages', 'alice', { schemaVersion: 1, commandId: 'command_http01', serviceId, generation: 1, action: 'listSharedBots', payload: {} }),
    { status: 403, body: { error: { code: 'SERVICE_SCOPE_UNSUPPORTED' } } });
  assert.equal((await post('/v1/teams/messages', 'alice', { action: 'list', serviceId, generation: 1 })).status, 200);
  assert.equal((await post('/v1/services', 'alice', { action: 'enterService', serviceId })).status, 400);
});
