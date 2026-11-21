import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';
import { promisify } from 'node:util';
import test from 'node:test';

import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';

import { createFirebaseBotTokens } from '../src/lingxi-data-bot-tokens.mjs';
import { sourceBotPrincipal } from '../src/lingxi-data-sources.mjs';
import { SharedBotsStore } from '../src/shared-bots.mjs';

const exec = promisify(execFile);

test('product Bot permits gate MySQL WebRTC and durable shared record operations', {
  skip: !process.env.LINGXI_DATA_PREVIEW_KEY, timeout: 310_000,
}, async t => {
  const fixture = JSON.parse(await readFile(process.env.LINGXI_DATA_FIXTURE, 'utf8'));
  const { source, bot, config, ca, ice, cli, folder, publicKey } = fixture.private;
  const [owner, ownerFile] = fixture.owner, [reader, readerFile] = fixture.reader;
  const [ownerToken, readerToken] = await Promise.all([ownerFile, readerFile].map(path => readFile(path, 'utf8')));
  const app = initializeApp({ projectId: process.env.GCLOUD_PROJECT }, 'private-preview-test');
  const db = getFirestore(app);
  t.after(async () => { await db.terminate(); await deleteApp(app); });
  const team = db.collection('teams').doc(fixture.team);
  const effectiveBot = sourceBotPrincipal(fixture.team, source, bot);
  const mint = createFirebaseBotTokens({ auth: getAuth(app), projectId: process.env.GCLOUD_PROJECT, apiKey: 'demo',
    emulator: process.env.FIREBASE_AUTH_EMULATOR_HOST, fetchRequest: async (url, options) => {
      const response = await fetch(url, options);
      const body = await response.clone().json();
      assert.equal(response.status, 200, body.error?.message);
      assert.equal(typeof body.idToken, 'string');
      assert.ok(Number(body.expiresIn) >= 60, 'missing Firebase Auth ID token expiry');
      return response;
    } });
  assert.equal((await getAuth(app).verifyIdToken(await mint(effectiveBot))).uid, effectiveBot);

  const conversationId = 'private-chat';
  const room = team.collection('conversations').doc(conversationId);
  const commands = new SharedBotsStore(db);
  let commandId = 1;
  const run = (uid, action, payload) => commands.execute({ uid }, {
    schemaVersion: 1, teamId: fixture.team, generation: 1, commandId: 'preview-command-' + commandId++, action, payload,
  });
  const host = { deploymentId: 'private-deployment', hostId: 'private-host', accountEpoch: 1 };
  const requester = { uid: reader, generation: 1 };
  await db.batch().set(team, { state: 'active' })
    .set(team.collection('members').doc(owner), { role: 'owner', generation: 1, displayName: 'Maintainer' })
    .set(team.collection('members').doc(reader), { role: 'member', generation: 1, displayName: 'Requester' }).commit();

  const publishBot = (sharedBotId, deployment, name) => run(owner, 'publishSharedBot', {
    sharedBotId, expectedRevision: 0, name, description: 'Data fixture Bot',
    usePolicy: { mode: 'all' }, deployment: { ...deployment, sharedBotId, revision: 1, owner: { uid: owner, generation: 1 },
      instanceId: 'private-instance', definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
      capabilities: { durableWait: false, writeOperations: true, delegation: false }, enabled: true },
    inputs: [{ kind: 'persona', resourceId: 'private-persona', revision: 1, digest: 'a'.repeat(64), sourceAudience: { mode: 'team' } }],
    quotas: { team: 100, member: 20, bot: 100 } });
  await publishBot(bot, host, 'Source Bot');
  await run(owner, 'heartbeatDeployment', { ...host, ready: true });
  await run(owner, 'createConversation', { conversationId, kind: 'group', title: 'Private preview',
    participantUids: [owner, reader], sharedBotIds: [bot] });

  const children = new Set();
  t.after(async () => { await Promise.all([...children].map(stop)); });
  function child(program, args) {
    const subprocess = spawn(program, args, { stdio: 'ignore' });
    children.add(subprocess);
    subprocess.once('exit', () => children.delete(subprocess));
    return subprocess;
  }
  async function stop(process) {
    if (process.exitCode !== null || process.signalCode !== null) return;
    const exit = once(process, 'exit');
    process.kill();
    await exit;
  }
  const post = (path, token, body, taskToken) => fetch(fixture.endpoint + path, {
    method: 'POST', headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json',
      ...(taskToken ? { 'X-Lingxi-Task-Token': taskToken } : {}) }, body: JSON.stringify(body),
  });
  async function startAdapter() {
    const process = child(processCommand(), ['scripts/serve-lingxi-data.mjs']);
    for (let i = 0; i < 100; i++) {
      if (process.exitCode !== null) assert.fail('product adapter exited');
      try { await post('/v1/product-data', 'invalid', {}); return process; }
      catch { await wait(100); }
    }
    assert.fail('product adapter did not start');
  }
  function processCommand() { return process.execPath; }
  let adapter = await startAdapter();
  const personalSource = 'personal-orders';
  const personalConfig = join(folder, 'personal-source.json');
  await writeFile(personalConfig, JSON.stringify({ ...JSON.parse(await readFile(config, 'utf8')), source: personalSource }), { mode: 0o600 });
  const personalRegistration = { action: 'configureDataSource', source: personalSource, name: 'Personal orders', kind: 'mysql', key: publicKey,
    fields: ['id', 'title'], scope: { kind: 'personal' }, expectedVersion: 0, enabled: true, requesters: [], grants: [] };
  const personal = await post('/v1/product-data', ownerToken, personalRegistration);
  assert.equal(personal.status, 200, await personal.clone().text());
  const personalHost = child(cli, ['host', '--endpoint', fixture.endpoint, '--auth-file', ownerFile, '--ca', ca, '--config', personalConfig]);
  let onlinePersonal = false;
  for (let spin = 0; spin < 60 && !onlinePersonal; spin++) {
    const response = await post('/v1/data/signaling', ownerToken, { action: 'directory' });
    assert.equal(response.status, 200);
    onlinePersonal = (await response.json())[personalSource]?.online === true;
    if (!onlinePersonal) await wait(150);
  }
  assert.ok(onlinePersonal);
  assert.deepEqual(await (await post('/v1/data/signaling', readerToken, { action: 'directory' })).json(), {});
  const personalPage = await exec(cli, ['preview', '--endpoint', fixture.endpoint, '--auth-file', ownerFile, '--ca', ca,
    '--source', personalSource, '--ice', ice], { timeout: 30_000 });
  assert.deepEqual(JSON.parse(personalPage.stdout).rows[0], { id: 'a', title: 'Original title' });
  await stop(personalHost);
  const sourceConfig = { action: 'configureDataSource', source, name: 'Private test orders', kind: 'mysql', key: publicKey,
    scope: { kind: 'team', teamId: team.id, conversationId, generation: 1 }, expectedVersion: 0, enabled: true,
    requesters: [requester.uid], grants: [{ sharedBotId: bot, fields: ['id', 'title'], preset: 'main' }] };
  const configuredSource = await post('/v1/product-data', ownerToken, sourceConfig);
  assert.equal(configuredSource.status, 200, await configuredSource.clone().text());
  assert.ok((await configuredSource.json()).enabled);
  const discoverSources = { scope: sourceConfig.scope, command: { action: 'privateSourceOffers' } };
  assert.deepEqual(await (await post('/v1/product-data', readerToken, discoverSources)).json(), {
    sources: [{ source, name: sourceConfig.name, sharedBotId: bot, botName: 'Source Bot' }],
  });
  assert.deepEqual(await (await post('/v1/product-data', ownerToken, discoverSources)).json(), { sources: [] });
  const request = { conversationId, messageId: 'private-message', changeId: 'private-change', text: 'REQUESTER_PRIVATE_TEST_TEXT',
    targets: [bot], privateDataSource: source };
  const receipt = await run(reader, 'submit', request);
  const { taskId } = receipt.tasks[0];
  const start = { ...host, taskId, expectedLastChangeSeq: 1 };
  await assert.rejects(run(owner, 'startTask', start), { code: 'DATA_PRIVATE_TASK' });
  const { permitId, inputs } = await run(owner, 'startTask', { ...start, privateData: true });
  const proof = { ...start, permitId, source, teamId: team.id, generation: 1, action: 'privatePreviewPermit' };
  assert.equal((await room.collection('messages').doc(request.messageId).get()).data().text, 'Private data preview requested.');
  const getPermit = (token = ownerToken, request = proof) => post('/v1/product-data', token, request);
  const directory = token => post('/v1/data/signaling', ownerToken, { action: 'directory' }, token);
  const sourceProcess = child(cli, ['host', '--endpoint', fixture.endpoint, '--auth-file', ownerFile, '--ca', ca, '--config', config]);
  assert.equal((await getPermit(readerToken)).status, 403);
  assert.equal((await getPermit(ownerToken, { ...proof, permitId: 'wrong-permit' })).status, 403);
  const accepted = await getPermit();
  assert.equal(accepted.status, 200, await accepted.clone().text());
  const lease = await accepted.json();
  assert.equal((await post('/v1/data/signaling', readerToken, { action: 'directory' }, lease.token)).status, 403);
  const permitFile = join(folder, 'product-task-token');
  await writeFile(permitFile, lease.token, { mode: 0o600 });
  async function online() {
    for (let i = 0; i < 60; i++) {
      if (sourceProcess.exitCode !== null) assert.fail('source host exited');
      const reply = await directory(lease.token);
      assert.equal(reply.status, 200, await reply.clone().text());
      if ((await reply.json())[source]?.online) return;
      await wait(200);
    }
    assert.fail('source host did not become reachable');
  }
  const readPage = () => exec(cli, ['preview', '--endpoint', fixture.endpoint, '--auth-file', ownerFile,
    '--task-token-file', permitFile, '--ca', ca, '--source', source, '--ice', ice], { timeout: 30_000 });
  async function preview() {
    await online();
    return JSON.parse((await readPage()).stdout);
  }
  const page = await preview();
  assert.deepEqual(page.rows[0], { id: 'a', title: 'Original title' });
  assert.ok(!JSON.stringify(page).includes('private_note'));
  await stop(adapter);
  adapter = await startAdapter();
  assert.deepEqual(await (await getPermit()).json(), lease);
  assert.deepEqual((await preview()).rows, page.rows);
  const result = { ...host, taskId, permitId, outcome: 'completed', finishedAt: Date.now(), privateData: true, text: '',
    inputGrantIds: inputs.map(grant => grant.grantId), outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } };
  await assert.rejects(run(owner, 'completeTask', { ...result, privateData: undefined, text: 'Original title' }), { code: 'DATA_PRIVATE_TASK' });
  await assert.rejects(run(owner, 'readTaskList', { ...host, taskId, permitId }), { code: 'DATA_PRIVATE_TASK' });
  const sessions = db.collection('lingxiDataPreviewSessions');
  const prior = new Set((await sessions.get()).docs.map(doc => doc.id));
  await online();
  sourceProcess.kill('SIGSTOP');
  const disable = () => post('/v1/product-data', ownerToken, { ...sourceConfig, expectedVersion: 1, enabled: false });
  let disabled;
  try {
    const inflight = readPage().then(value => ({ value }), error => ({ error }));
    let offer;
    for (let spin = 0; spin < 60 && !offer; spin++) {
      offer = (await sessions.get()).docs.find(doc => !prior.has(doc.id));
      if (!offer) await wait(100);
    }
    assert.ok(offer, 'WebRTC offer must exist before authority changes');
    disabled = disable();
    const binding = db.collection('lingxiDataSourceBindings').doc(source);
    for (let spin = 0; spin < 80 && (await binding.get()).data()?.enabled; spin++) await wait(100);
    assert.equal((await binding.get()).data()?.enabled, false);
    const status = await post('/v1/data/signaling', ownerToken, { action: 'status', id: offer.id }, lease.token);
    assert.equal(status.status, 403, 'revocation must deny status for the pending offer');
    const revoked = await inflight;
    assert.ok('error' in revoked && revoked.error.stdout === '', 'revoked handshake must return no source rows');
  } finally { sourceProcess.kill('SIGCONT'); }
  let stopped = await disabled;
  if (stopped.status === 503) stopped = await disable();
  assert.equal(stopped.status, 200, await stopped.clone().text());
  assert.equal((await directory(lease.token)).status, 403);
  assert.equal((await getPermit()).status, 403);
  const restored = await post('/v1/product-data', ownerToken, { ...sourceConfig, expectedVersion: 2 });
  assert.equal(restored.status, 200, await restored.clone().text());
  assert.equal((await getPermit()).status, 403, 'restoring access must not revive an older task');
  await run(owner, 'completeTask', { ...result, outcome: 'failed' });
  const messages = (await room.collection('messages').get()).docs.map(row => row.data());
  assert.equal(messages.length, 1);
  assert.ok(!JSON.stringify(messages).includes('Original title'));

  const orders = 'bot-orders';
  const receiver = 'sharedbot_02';
  const receiverHost = { ...host, deploymentId: 'receiver-deployment' };
  await publishBot(receiver, receiverHost, 'Receiving Bot');
  await run(owner, 'createConversation', { conversationId: orders, kind: 'group', title: 'Bot orders',
    participantUids: [owner, reader], sharedBotIds: [bot, receiver] });
  const scope = { kind: 'team', teamId: team.id, conversationId: orders, generation: 1 };
  const product = (body, token = ownerToken) => post('/v1/product-data', token, body);
  const audience = await (await product({ scope, command: { action: 'previewAudience', editors: [] } })).json();
  const created = await product({ scope, operation: 'bot-orders-create', command: { action: 'createTeam',
    audienceRevision: audience.audienceRevision, editors: [],
    schema: { type: 'object', additionalProperties: false, properties: { title: { type: 'string' } } },
  } });
  assert.equal(created.status, 200, await created.clone().text());
  const { dataset } = await created.json();
  const grant = { action: 'setBotDataAccess', scope, dataset, sharedBotId: bot, access: 'edit', expectedVersion: 0,
    reportReview: true, handoff: { targets: [receiver], dailyLimit: 4 } };
  assert.equal((await product(grant, readerToken)).status, 403);
  const granted = await product(grant);
  assert.equal(granted.status, 200, await granted.clone().text());
  assert.deepEqual(await granted.json(), { version: 1, access: 'edit' });
  await run(owner, 'heartbeatDeployment', { ...host, ready: true });
  const shared = await run(owner, 'submit', { conversationId: orders, messageId: 'bot-order-message',
    changeId: 'bot-order-change', text: 'Record the order', targets: [bot] });
  const grantedTask = { ...host, taskId: shared.tasks[0].taskId, expectedLastChangeSeq: 1 };
  const permit = await run(owner, 'startTask', grantedTask);
  const write = { ...grantedTask, permitId: permit.permitId, teamId: team.id, generation: 1,
    action: 'botDataOperation', accessVersion: 1, stepId: 'write-order',
    operation: { action: 'put', dataset, record: 'new-order', expectedRevision: 0, schemaVersion: 1,
      value: { title: 'Written by Bot' } } };
  const offers = { ...grantedTask, permitId: permit.permitId, teamId: team.id, generation: 1, action: 'botDataOffers' };
  const offer = { dataset, version: 1, write: true, reportReview: true, handoffTargets: [receiver] };
  assert.deepEqual(await (await product(offers)).json(), { datasets: [offer] });
  assert.equal((await product(write, readerToken)).status, 403);
  assert.equal((await product({ ...write, permitId: 'invalid-permit' })).status, 403);
  const saved = await product(write);
  assert.equal(saved.status, 200, await saved.clone().text());
  const reference = await saved.json();
  assert.deepEqual(await (await product(write)).json(), reference);
  assert.equal((await product({ ...write, operation: { ...write.operation, value: { title: 'Conflicting retry' } } })).status, 409);
  const opened = await product({ scope, command: { action: 'get', reference } }, readerToken);
  assert.equal(opened.status, 200, await opened.clone().text());
  const { record } = await opened.json();
  assert.equal(record.origin, 'agent');
  assert.equal(record.actor.task, grantedTask.taskId);
  assert.deepEqual(record.value, { title: 'Written by Bot' });
  const report = async command => {
    const response = await product({ scope, command });
    assert.equal(response.status, 200, await response.clone().text());
    return response.json();
  };
  const ctx = await report({ action: 'reportContext', dataset });
  const { epoch } = await report({ action: 'describe', dataset });
  const plan = { origin: { kind: 'team', dataset, epoch }, parent: null, audienceRevision: ctx.audienceRevision,
    readers: [owner, reader].map(uid => ({ uid, partitions: '*' })), bots: [bot], spec: {
      title: 'Order count', partition: 'title', group: 'title', metric: { kind: 'count' } } };
  const { confirmation } = await report({ action: 'prepareReport', dataset, plan });
  const { release } = await report({ action: 'publishReport', dataset, plan, confirmation });
  const readTotals = { ...write, stepId: undefined, operation: { action: 'getReport', dataset, release } };
  const totals = await product(readTotals);
  assert.equal(totals.status, 200, await totals.clone().text());
  assert.deepEqual((await totals.json()).points, [{ group: 'Written by Bot', value: '1' }]);
  const cell = { action: 'openReportReview', dataset, release, partition: 'Written by Bot', group: 'Written by Bot' };
  const reviewCase = await product({ ...write, stepId: 'open-case', operation: cell });
  assert.equal(reviewCase.status, 200, await reviewCase.clone().text());
  assert.deepEqual(await reviewCase.json(), await report(cell));
  await report({ action: 'limitReportReader', dataset, uid: reader, generation: 1, partitions: [], expectedVersion: 0 });
  assert.deepEqual((await (await product(readTotals)).json()).points, []);
  assert.equal((await product({ ...write, stepId: 'open-case', operation: cell })).status, 403);
  async function runModel(proof, desired) {
    await run(owner, 'heartbeatDeployment', { ...host, ready: true });
    const modelFixture = join(folder, 'product-model-input.json');
    await writeFile(modelFixture, JSON.stringify({ endpoint: fixture.endpoint, ownerFile, folder,
      offer, proof, desired,
      scope: { teamId: team.id, sharedBotId: bot, conversationId: orders, sessionEpoch: 1 },
    }), { mode: 0o600 });
    const integration = await exec(process.execPath, ['--import', '../../packages/bridge-client/node_modules/tsx/dist/loader.mjs',
      'test/lingxi-data-model.integration.ts', modelFixture], { cwd: resolve('../electron'), timeout: 110_000 });
    t.diagnostic(integration.stdout.trim());
    return JSON.parse(integration.stdout.trim());
  }
  if (process.env.LINGXI_TEST_MODEL_ENDPOINT) {
    const { action: _action, ...modelProof } = offers;
    await runModel(modelProof, { ...write.operation, record: 'model-order', value: { title: 'Order written by real model' } });
    const modelRecord = await product({ scope, command: { action: 'get', reference: {
      dataset, record: 'model-order', revision: 1, schemaVersion: 1,
    } } }, readerToken);
    assert.equal(modelRecord.status, 200, await modelRecord.clone().text());
    assert.deepEqual((await modelRecord.json()).record.value, { title: 'Order written by real model' });
  }
  const receiverGrant = { ...grant, sharedBotId: receiver, access: 'read', handoff: undefined, reportReview: undefined };
  assert.deepEqual(await (await product(receiverGrant)).json(), { version: 1, access: 'read' });
  await run(owner, 'heartbeatDeployment', { ...host, ready: true });
  await run(owner, 'completeTask', { ...host, taskId: grantedTask.taskId, permitId: permit.permitId, outcome: 'completed',
    finishedAt: Date.now(), text: 'Order saved.', inputGrantIds: permit.inputs.map(input => input.grantId),
    outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } });

  if (process.env.LINGXI_TEST_MODEL_ENDPOINT) {
    await report({ action: 'limitReportReader', dataset, uid: reader, generation: 1, partitions: '*', expectedVersion: 1 });
    await run(owner, 'heartbeatDeployment', { ...host, ready: true });
    const reportTask = await run(owner, 'submit', { conversationId: orders, messageId: 'review-report-message',
      changeId: 'review-report-change', text: 'Open the published report review.', targets: [bot] });
    const conversation = await team.collection('conversations').doc(orders).get();
    const fields = { ...host, taskId: reportTask.tasks[0].taskId, expectedLastChangeSeq: conversation.data().lastChangeSeq };
    const approval = await run(owner, 'startTask', fields);
    const proof = { ...fields, teamId: team.id, generation: 1, permitId: approval.permitId };
    const actual = await runModel(proof, cell);
    assert.deepEqual(actual.result, await report(cell));
    const cases = await db.collection('lingxiReportSeries').doc(dataset).collection('reviews').get();
    assert.deepEqual(cases.docs.map(entry => entry.id), [actual.result.review]);
    await run(owner, 'completeTask', { ...host, taskId: fields.taskId, permitId: approval.permitId, outcome: 'completed',
      finishedAt: Date.now(), text: 'Review case open.', inputGrantIds: approval.inputs.map(input => input.grantId),
      outputAudience: { mode: 'published_inputs' }, usage: { status: 'missing' } });
  }

  const rule = { action: 'setDataRule', scope, dataset, rule: 'review-order', expectedVersion: 0,
    config: { sharedBotId: bot, dailyLimit: 4, definition: { kind: 'transition', field: 'title', equals: 'Pending review' } } };
  const configured = await product(rule);
  assert.equal(configured.status, 200, await configured.clone().text());
  assert.deepEqual(await configured.json(), { version: 1, enabled: true });
  const automation = { scope, command: { action: 'dataAutomation', dataset } };
  assert.equal((await product(automation, readerToken)).status, 403);
  assert.deepEqual((await (await product(automation)).json()).rules, [{ rule: rule.rule, version: 1, config: rule.config }]);
  await stop(adapter);
  adapter = await startAdapter();
  const trigger = { scope, operation: 'trigger-review', command: { ...write.operation, expectedRevision: 1, value: { title: 'Pending review' } } };
  const changed = await product(trigger);
  assert.equal(changed.status, 200, await changed.clone().text());
  const review = await changed.json();
  assert.deepEqual(await (await product(trigger)).json(), review);
  const teamTasks = team.collection('tasks');
  let triggered = [];
  for (let spin = 0; spin < 80 && !triggered.length; spin++) {
    triggered = (await teamTasks.get()).docs.map(doc => doc.data()).filter(task => task.dataCause?.kind === 'rule');
    if (!triggered.length) await wait(150);
  }
  assert.equal(triggered.length, 1);
  assert.deepEqual(triggered[0].dataCause.reference, review);
  const ruleFields = { ...host, taskId: triggered[0].taskId,
    expectedLastChangeSeq: (await team.collection('conversations').doc(orders).get()).data().lastChangeSeq };
  const rulePermit = await run(owner, 'startTask', ruleFields);
  assert.ok(rulePermit.execute);
  const handoffProof = { ...ruleFields, teamId: team.id, generation: 1, permitId: rulePermit.permitId };
  const desiredHandoff = { action: 'handoff', sharedBotId: receiver, reference: review };
  const handoff = { ...handoffProof, ...desiredHandoff, action: 'botDataHandoff', accessVersion: 1, stepId: 'handoff-order' };
  assert.equal((await product({ ...handoff, reference: { ...review, record: 'model-order' } })).status, 403);
  let forwarded;
  if (process.env.LINGXI_TEST_MODEL_ENDPOINT) {
    const actual = await runModel(handoffProof, desiredHandoff);
    forwarded = actual.result;
    handoff.stepId = actual.stepId;
  } else {
    const delivered = await product(handoff);
    assert.equal(delivered.status, 200, await delivered.clone().text());
    forwarded = await delivered.json();
  }
  assert.deepEqual(await (await product(handoff)).json(), forwarded);
  assert.equal((await product({ ...handoff, stepId: 'a-second-handoff' })).status, 409);
  const handoffMessage = (await team.collection('conversations').doc(orders).collection('messages').doc(forwarded.messageId).get()).data();
  assert.equal(handoffMessage.author.sharedBotId, bot);
  assert.deepEqual(handoffMessage.dataRef, review);
  await run(owner, 'heartbeatDeployment', { ...receiverHost, ready: true });
  const receiverTask = { ...receiverHost, taskId: forwarded.taskId, expectedLastChangeSeq: handoffMessage.changeSeq };
  const receiverPermit = await run(owner, 'startTask', receiverTask);
  assert.notEqual(receiverPermit.permitId, permit.permitId);
  const receiverProof = { ...receiverTask, teamId: team.id, generation: 1, permitId: receiverPermit.permitId };
  assert.equal((await product({ ...handoff, ...receiverProof, sharedBotId: bot })).status, 403);
  const childRead = await product({ action: 'botDataOperation', ...receiverProof, accessVersion: 1,
    operation: { action: 'get', reference: review } });
  assert.equal(childRead.status, 200, await childRead.clone().text());
  assert.equal((await childRead.json()).record.value.title, 'Pending review');
  assert.deepEqual(await (await product({ ...receiverGrant, access: null, expectedVersion: 1 })).json(), { version: 2, access: null });
  assert.equal((await product({ action: 'botDataOffers', ...receiverProof })).status, 403);
  assert.deepEqual(await (await product({ ...rule, expectedVersion: 1, config: null })).json(), { version: 2, enabled: false });
  assert.equal((await product({ action: 'botDataOffers', ...handoffProof })).status, 403);
  assert.deepEqual(await (await product({ ...grant, handoff: undefined, reportReview: undefined, access: null, expectedVersion: 1 })).json(), { version: 2, access: null });
  assert.equal((await teamTasks.get()).docs.filter(doc => doc.data().dataCause?.kind === 'rule').length, 1);
  assert.equal((await product(write)).status, 403);
});
