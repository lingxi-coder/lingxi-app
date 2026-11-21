// Invoked by the Core two-replica HTTPS fixture with its isolated Firebase project.
import { readFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { getAuth } from 'firebase-admin/auth';
import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import assert from 'node:assert/strict';
import { createDatasetPages } from '../src/dataset-pages.mjs';
import { createDatasetGateway } from '../src/dataset-product.mjs';
import { createDatasetRender } from '../src/dataset-render.mjs';
import { createRenderer } from '../../dataset-renderer/supervisor.mjs';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

const projectId = process.env.GCLOUD_PROJECT;
if (!projectId?.startsWith('demo-') || !process.env.FIRESTORE_EMULATOR_HOST || !process.env.FIREBASE_AUTH_EMULATOR_HOST) {
  throw new Error('Expected isolated Auth and Firestore emulators');
}
const [origin, ownerPath, readerPath, reportFixture] = process.argv.slice(2);
const app = initializeApp({ projectId });
const db = getFirestore(app);
const datasets = createDatasetGateway(origin).datasets;
const packages = new Map();
const bucket = { file: path => ({
  async save(text) {
    if (packages.has(path)) throw Object.assign(new Error(), { code: 412 });
    packages.set(path, Buffer.from(text));
  },
  async download() { const stored = packages.get(path); assert.ok(stored); return [stored]; },
}) };
const pages = createDatasetPages({ db, bucket, datasets });
const http = createTeamsHandler({ store: new TeamsStore(db), datasets, datasetPages: pages.execute,
  verifyToken: token => getAuth(app).verifyIdToken(token) });
const owner = await readFile(ownerPath, 'utf8'), reader = await readFile(readerPath, 'utf8');
async function invoke(token, path, body, status = 200) {
  const response = await http(new Request('https://fixture.invalid/v1/teams/' + path,
    { method: 'POST', headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(body) }));
  const value = await response.json();
  assert.equal(response.status, status, JSON.stringify(value));
  return value;
}
if (reportFixture) {
  const { destination, dataset, grant: via, release, revoked } = JSON.parse(reportFixture);
  const target = { destination, dataset, via };
  if (revoked) {
    const [{ id: version }] = (await db.collection('datasetPages/scheduled-report-page/versions').get()).docs;
    await invoke(reader, 'dataset-pages', { action: 'describe', page: 'scheduled-report-page', version }, 403);
    await invoke(reader, 'dataset-pages', { action: 'saveState', page: 'scheduled-report-page', version, expected: 0, state: '{}' }, 403);
  } else {
    const { audienceDigest } = await invoke(owner, 'dataset-pages', { action: 'prepare', ...target });
    await invoke(reader, 'dataset-pages', { action: 'prepare', ...target }, 403);
    const publication = { action: 'publish', ...target, generation: 1, audienceDigest,
      page: 'scheduled-report-page', operation: 'chart', bundle: {
        title: 'Scheduled report chart', html: '<main>Monthly orders</main>', assets: {},
        actions: { report: { mode: 'readFixed' } },
      } };
    await invoke(owner, 'dataset-pages', { ...publication, bundle: { ...publication.bundle,
      actions: { latest: { mode: 'readLatest' } } } }, 403);
    const { reference } = await invoke(owner, 'dataset-pages', publication);
    const action = { action: 'invoke', page: reference.page, version: reference.version, alias: 'report' };
    const first = await invoke(reader, 'dataset-pages', { ...action, parameters: { limit: 1 } });
    assert.deepEqual(first.reference, { kind: 'reportRelease', dataset, release });
    assert.deepEqual(first.groups, [{ group: 'Tools', units: '10000' }]);
    assert.deepEqual((await invoke(reader, 'dataset-pages', { ...action, parameters: { after: 'Tools' } })).groups, []);
  }
  console.log('PASS scheduled report page ' + (revoked ? 'revokes access with its task share' : 'reads immutable reader-scoped report'));
  await deleteApp(app);
  process.exit(0);
}
const destination = { team: 'friends_01', conversation: 'friend_chat' };
const command = (action, fields = {}, via) => ({ protocol: 2, dataset: 'orders', via, command: { action, ...fields } });
const audience = await invoke(owner, 'datasets', command('audience', { destination }));
const share = { action: 'share', dataset: 'orders', operation: 'gateway-share', destination, generation: 1,
  audienceDigest: audience.audienceDigest, text: 'Orders at publication', projection: { fields: ['qty'], partitions: ['north'] } };
let result;
do { result = await invoke(owner, 'datasets', share); } while (result.status === 'building');
const again = await invoke(owner, 'datasets', share);
assert.deepEqual(again, result);
const scope = { teamId: destination.team, conversationId: destination.conversation, generation: 1 };
const history = await invoke(reader, 'messages', { ...scope, action: 'history' });
const messages = history.messages.filter(message => message.id === result.messageId);
assert.equal(messages.length, 1);
assert.deepEqual(messages[0].resourceRef, result.reference);
assert.equal(messages[0].resourceGrant, result.grant);
const page = await invoke(reader, 'datasets', command('openSnapshot', { snapshot: result.reference.snapshot, block: 0 }, result.grant));
assert.ok(page.records.length > 0);
assert.deepEqual(Object.keys(page.records[0].value), ['qty']);
const pageTarget = { destination, dataset: 'orders', via: result.grant };
const prepared = await invoke(owner, 'dataset-pages', { action: 'prepare', ...pageTarget });
const publication = { action: 'publish', ...pageTarget, page: 'orders-page', operation: 'page-v1', generation: 1,
  audienceDigest: prepared.audienceDigest, bundle: { title: 'Orders viewer', html: '<main>Fixture</main>', assets: {},
    actions: { records: { mode: 'readFixed' } } } };
const published = await invoke(owner, 'dataset-pages', publication);
assert.deepEqual(await invoke(owner, 'dataset-pages', publication), published);
assert.equal(packages.size, 1);
const view = { page: published.reference.page, version: published.reference.version };
const metadata = await invoke(reader, 'dataset-pages', { action: 'describe', ...view });
assert.equal(metadata.bundle, undefined);
assert.equal(metadata.stateVersion, 0);
assert.deepEqual(await invoke(reader, 'dataset-pages', { action: 'invoke', ...view, alias: 'records' }), page);
await invoke(reader, 'dataset-pages', { action: 'invoke', ...view, alias: 'unapproved' }, 403);
await invoke(reader, 'dataset-pages', { action: 'saveState', ...view, expected: 0, state: '{"filter":"north"}' });
await invoke(reader, 'dataset-pages', { action: 'saveState', ...view, expected: 0, state: '{}' }, 409);
assert.equal((await invoke(owner, 'dataset-pages', { action: 'describe', ...view })).state, null);
assert.equal((await invoke(reader, 'dataset-pages', { action: 'describe', ...view })).state, '{"filter":"north"}');
const readerActor = await getAuth(app).verifyIdToken(reader);
assert.equal((await pages.openForRenderer(readerActor, reader, view.page, view.version)).bundle.html, '<main>Fixture</main>');
await invoke(owner, 'messages', { ...scope, action: 'mutate', operation: 'recall', messageId: result.messageId, changeId: 'takeback01', text: '' });
await invoke(reader, 'datasets', command('openSnapshot', { snapshot: result.reference.snapshot, block: 0 }, result.grant), 403);
await invoke(reader, 'dataset-pages', { action: 'describe', ...view }, 403);
console.log('PASS private Dataset page version, idempotent publishing, declared actions, per-reader state and source revocation');
console.log('PASS product Teams API delivers a pinned Dataset once, and recalling the real message revokes access');

const dataset = 'editable-page-fixture';
const seed = { record: 'example', partition: 'north', schemaVersion: 1 };
const action = (name, fields, operation) => ({ protocol: 2, dataset, operation, command: { action: name, ...fields } });
await invoke(owner, 'datasets', action('create', { schema: { type: 'object', properties: { qty: { type: 'integer' } },
  required: ['qty'], additionalProperties: false } }, 'create'));
await invoke(owner, 'datasets', action('put', { ...seed, expectedRevision: 0, value: { qty: 7 } }, 'seed'));
const editAudience = await invoke(owner, 'datasets', action('audience', { destination }));
const editShare = await invoke(owner, 'datasets', { ...share, dataset, mode: 'edit', operation: 'share-edit',
  audienceDigest: editAudience.audienceDigest });
const editTarget = { destination, dataset, via: editShare.grant };
const editPrepare = await invoke(owner, 'dataset-pages', { action: 'prepare', ...editTarget });
const editPage = (await invoke(owner, 'dataset-pages', { ...publication, ...editTarget, page: 'editable-page',
  audienceDigest: editPrepare.audienceDigest, bundle: { ...publication.bundle, actions: {
    changeQty: { mode: 'putRecord', record: seed.record, fields: ['qty'] },
  } } })).reference;
const editScope = { page: editPage.page, version: editPage.version };
const proposed = { id: 'pending-qty', alias: 'changeQty', operation: 'update-qty',
  parameters: { partition: seed.partition, schemaVersion: 1, expectedRevision: 1, value: { qty: 9 } } };
const direct = { action: 'invoke', ...editScope, alias: proposed.alias, operation: proposed.operation, parameters: proposed.parameters };
assert.equal((await invoke(reader, 'dataset-pages', direct, 403)).error.code, 'DATA_APPROVAL_REQUIRED');
assert.equal((await invoke(reader, 'datasets', { action: 'page', request: direct }, 403)).error.code, 'DATA_APPROVAL_REQUIRED');

const secret = randomBytes(32).toString('base64url');
const renderer = createRenderer({ secret, worker: () => ({
  async request(command) {
    return { png: 'test-frame', accessibility: [], actions: command.action === 'mount' ? [proposed] : [] };
  },
  stop() {},
}) });
try {
  const render = createDatasetRender({ origin: 'https://renderer.fixture.invalid', secret, pages, db,
    fetchRequest: (url, options) => renderer.fetch(new Request(url, options)) });
  const frame = await render(readerActor, reader, { action: 'mount', ...editScope, dimensions: { width: 620, height: 480 } });
  assert.deepEqual(frame.approvals.map(value => value.id), [proposed.id]);
  assert.equal((await invoke(owner, 'datasets', action('latest', { record: seed.record }))).value.qty, 7);
  const approve = { action: 'approve', ...editScope, session: frame.session, id: proposed.id };
  await render(readerActor, reader, approve);
  await render(readerActor, reader, approve);
  const written = await invoke(owner, 'datasets', action('latest', { record: seed.record }));
  assert.equal(written.value.qty, 9);
  assert.equal(written.reference.revision, 2);
} finally {
  renderer.dispose();
}
console.log('PASS page writes require a renderer approval and replay commits one record revision');

const bot = 'chart_review_bot', deployment = 'chart_review_host', prompt = 'Review the monthly report.';
const team = db.doc(`teams/${destination.team}`);
const thread = team.collection('conversations').doc(destination.conversation);
const ownerActor = await getAuth(app).verifyIdToken(owner);
const { participants } = (await thread.get()).data();
await thread.update({ participants: [...participants, 'shared_bot:' + bot] });
await team.collection('sharedBots').doc(bot).set({
  identity: { schemaVersion: 1, teamId: team.id, sharedBotId: bot, maintainer: { uid: ownerActor.uid, generation: 1 },
    revision: 1, state: 'published', usePolicy: { mode: 'all' }, activeDeploymentId: deployment },
  grants: [], quotas: { team: 100, member: 100, bot: 100 },
});
await team.collection('deployments').doc(deployment).set({ enabled: true, ready: true, onlineUntil: Date.now() + 120_000 });
const { reference: taskPage } = await invoke(owner, 'dataset-pages', {
  ...publication, ...editTarget, page: 'task-page', audienceDigest: editPrepare.audienceDigest,
  bundle: { ...publication.bundle, actions: { review: { mode: 'startTask', bot, prompt } } },
});
const taskScope = { page: taskPage.page, version: taskPage.version };
const taskIntent = { id: 'start-review', alias: 'review', operation: 'review-one', parameters: {} };
const { id: _, ...taskInvoke } = taskIntent;
assert.equal((await invoke(reader, 'dataset-pages', { action: 'invoke', ...taskScope, ...taskInvoke }, 403)).error.code, 'DATA_APPROVAL_REQUIRED');
const taskRenderer = createRenderer({ secret, worker: () => ({
  async request(command) {
    return { png: 'test-frame', accessibility: [], actions: command.action === 'mount' ? [taskIntent] : [] };
  },
  stop() {},
}) });
try {
  const render = createDatasetRender({ origin: 'https://renderer.fixture.invalid', secret, pages, db,
    fetchRequest: (url, options) => taskRenderer.fetch(new Request(url, options)) });
  const mount = { action: 'mount', ...taskScope, dimensions: { width: 620, height: 480 } };
  const declined = await render(readerActor, reader, mount);
  assert.deepEqual(declined.approvals, [{ id: taskIntent.id, mode: 'startTask', alias: taskIntent.alias, bot, prompt }]);
  await render(readerActor, reader, { action: 'deny', ...taskScope, session: declined.session, id: taskIntent.id });
  assert.equal((await team.collection('tasks').where('sharedBotId', '==', bot).get()).size, 0);
  await render(readerActor, reader, { action: 'close', ...taskScope, session: declined.session });
  const accepted = await render(readerActor, reader, mount);
  const accept = { action: 'approve', ...taskScope, session: accepted.session, id: taskIntent.id };
  await render(readerActor, reader, accept);
  await render(readerActor, reader, accept);
  const tasks = await team.collection('tasks').where('sharedBotId', '==', bot).get();
  assert.equal(tasks.size, 1);
  const task = tasks.docs[0].data();
  assert.equal(task.requester.uid, readerActor.uid);
  assert.equal(task.state, 'queued');
  assert.equal((await thread.collection('messages').doc(task.messageId).get()).data().text, prompt);
} finally {
  taskRenderer.dispose();
  await thread.update({ participants });
}
console.log('PASS page Bot task requires explicit approval and replay queues only one declared task');
