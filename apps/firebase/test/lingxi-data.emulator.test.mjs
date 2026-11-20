import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';

import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';

import { createProductDataTransport } from '../src/lingxi-data-product.mjs';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

test('Teams and Chat open and update a pinned LingXi Data record', { skip: !process.env.LINGXI_DATA_FIXTURE }, async t => {
  const fixture = JSON.parse(await readFile(process.env.LINGXI_DATA_FIXTURE, 'utf8'));
  const app = initializeApp({ projectId: process.env.GCLOUD_PROJECT }, 'lingxi-data-product');
  const db = getFirestore(app);
  t.after(async () => { await db.terminate(); await deleteApp(app); });
  const [owner, ownerFile] = fixture.owner;
  const [reader, readerFile] = fixture.reader;
  const tokens = await Promise.all([ownerFile, readerFile].map(file => readFile(file, 'utf8')));
  const team = db.doc(`teams/${fixture.team}`);
  const thread = team.collection('conversations').doc('orders-chat');
  const otherThread = team.collection('conversations').doc('other-chat');
  const binding = db.doc(`lingxiDataBindings/${fixture.reference.dataset}`);
  const member = uid => team.collection('members').doc(uid);
  const readers = [owner, reader];
  const conversation = { title: 'Orders', kind: 'channel', memberUids: readers,
    participants: [`user:${owner}`], createdBy: owner, lastSeq: 0, lastChangeSeq: 0, lastMessage: null };
  await db.batch().set(team, { state: 'active' })
    .set(member(owner), { generation: 1, role: 'owner' }).set(member(reader), { generation: 1, role: 'member' })
    .set(thread, conversation).set(otherThread, conversation)
    .set(binding, { kind: 'team', teamId: fixture.team, conversationId: thread.id,
      audience: [{ uid: owner, generation: 1, access: 'manage' }, { uid: reader, generation: 1, access: 'read' }] })
    .commit();
  const data = createProductDataTransport(fixture.endpoint);
  const handler = createTeamsHandler({ store: new TeamsStore(db), data, verifyToken: token => getAuth(app).verifyIdToken(token, true) });
  const scope = { kind: 'team', teamId: team.id, conversationId: thread.id, generation: 1 };
  function request(path, token, body) {
    return handler(new Request(`https://teams.test/v1/teams/${path}`, {
      method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: JSON.stringify(body),
    }));
  }
  assert.deepEqual(JSON.parse(await readFile(fixture.actorFile, 'utf8')), {});
  assert.equal((await request('data', 'not-a-token', { scope, command: { action: 'datasets' } })).status, 401);
  const read = { scope, command: { action: 'get', reference: fixture.reference } };
  const opened = await request('data', tokens[1], read);
  assert.equal(opened.status, 200, await opened.clone().text());
  assert.equal((await opened.json()).record.value.title, 'Prepare shipment');
  assert.equal((await request('data', tokens[1], { ...read, scope: { ...scope, conversationId: otherThread.id } })).status, 403);
  const discover = { scope, command: { action: 'datasets' } };
  assert.deepEqual(await (await request('data', tokens[1], discover)).json(), {
    datasets: [{ dataset: fixture.reference.dataset, access: 'read' }],
  });
  assert.deepEqual(await (await request('data', tokens[0], discover)).json(), {
    datasets: [{ dataset: fixture.reference.dataset, access: 'manage' }],
  });
  assert.deepEqual(await (await request('data', tokens[1], { ...discover, scope: { ...scope, conversationId: otherThread.id } })).json(), { datasets: [] });
  const write = { scope, operation: 'edit-order', command: { action: 'put', dataset: fixture.reference.dataset,
    record: fixture.reference.record, schemaVersion: 1, expectedRevision: 1, value: { title: 'Ready to ship' } } };
  assert.equal((await request('data', tokens[1], write)).status, 403);
  const updated = await request('data', tokens[0], write);
  assert.equal(updated.status, 200, await updated.clone().text());
  const next = await updated.json();
  assert.equal(next.revision, 2);
  const latest = { scope, command: { action: 'latest', dataset: next.dataset, record: next.record } };
  assert.deepEqual(await (await request('data', tokens[1], latest)).json(), next);
  assert.equal((await request('data', tokens[1], { ...latest, scope: { ...scope, conversationId: otherThread.id } })).status, 403);
  assert.deepEqual(await (await request('data', tokens[0], write)).json(), next);
  const conflict = await request('data', tokens[0], { ...write, operation: 'conflict-order' });
  assert.equal(conflict.status, 409);
  assert.deepEqual(await conflict.json(), { error: { code: 'DATA_CONFLICT' } });
  const page = await request('data', tokens[1], { scope, command: { action: 'list', dataset: next.dataset } });
  assert.equal(page.status, 200);
  assert.equal((await page.json()).records[0].record.revision, next.revision);
  const message = { action: 'mutate', teamId: team.id, generation: 1, conversationId: thread.id,
    changeId: 'change-order', messageId: 'message-order', operation: 'create',
    text: 'Review the shipping order', dataRef: next };
  const sent = await request('messages', tokens[0], message);
  assert.equal(sent.status, 200, await sent.clone().text());
  assert.equal((await request('messages', tokens[0], { ...message, dataRef: fixture.reference })).status, 409);
  const synced = await request('messages', tokens[1], { action: 'sync', teamId: team.id,
    generation: 1, conversationId: thread.id, after: 0 });
  assert.equal(synced.status, 200, await synced.clone().text());
  const [row] = (await synced.json()).messages;
  assert.deepEqual(row.dataRef, next);
  assert.equal(row.text, 'Review the shipping order');
  assert.ok(!JSON.stringify(row).includes('Ready to ship'));
  const visible = await request('data', tokens[1], { scope, command: { action: 'get', reference: row.dataRef } });
  assert.equal((await visible.json()).record.value.title, 'Ready to ship');
  await member(reader).set({ generation: 2 });
  assert.equal((await request('data', tokens[1], read)).status, 409);
  assert.equal((await request('data', tokens[1], { ...read, scope: { ...scope, generation: 2 } })).status, 403);
  assert.deepEqual(await (await request('data', tokens[1], { ...discover, scope: { ...scope, generation: 2 } })).json(), { datasets: [] });
  const personal = { kind: 'personal' };
  const model = { type: 'object', additionalProperties: false, properties: { title: { type: 'string' } } };
  const create = { scope: personal, operation: 'personal-schema-create', command: { action: 'create', schema: model } };
  const made = await request('data', tokens[0], create);
  assert.equal(made.status, 200, await made.clone().text());
  const personalDataset = await made.json();
  assert.equal(personalDataset.schemaVersion, 1);
  assert.deepEqual(await (await request('data', tokens[0], create)).json(), personalDataset);
  assert.equal((await request('data', tokens[1], { scope: personal, command: { action: 'describe', dataset: personalDataset.dataset } })).status, 403);
  const revised = await request('data', tokens[0], { scope: personal, operation: 'personal-schema-edit', command: {
    action: 'schema', dataset: personalDataset.dataset, expectedEpoch: personalDataset.epoch,
    schema: { ...model, properties: { ...model.properties, reviewed: { type: 'boolean' } } },
  } });
  assert.equal(revised.status, 200, await revised.clone().text());
  assert.equal((await revised.json()).schemaVersion, 2);
  const personalRecord = await request('data', tokens[0], { scope: personal, operation: 'personal-order-draft',
    command: { action: 'put', dataset: personalDataset.dataset, record: 'draft', expectedRevision: 0, schemaVersion: 2,
      value: { title: 'Selected personal draft', reviewed: false } } });
  assert.equal(personalRecord.status, 200, await personalRecord.clone().text());
  const privateReference = await personalRecord.json();
  const personalDescription = { scope: personal, command: { action: 'describe', dataset: personalDataset.dataset } };
  const detail = await (await request('data', tokens[0], personalDescription)).json();
  const view = await request('data', tokens[0], { scope: personal, operation: 'personal-triage-view', command: {
    action: 'setView', dataset: personalDataset.dataset, view: 'triage-form', expectedEpoch: detail.epoch,
    definition: { kind: 'form', fields: ['title', 'reviewed'] },
  } });
  assert.equal(view.status, 200, await view.clone().text());
  const { epoch: sourceEpoch } = await view.json();
  const definition = { version: 1, nodes: [{ kind: 'element', tag: 'section', style: { color: '#423190' },
    children: [{ kind: 'field', field: 'title' }] }] };
  const templateCommand = { scope: personal, command: { action: 'setTemplateView', dataset: personalDataset.dataset,
    view: 'order-preview', schemaVersion: 2, expectedVersion: 0, definition } };
  assert.equal((await request('data', tokens[0], { ...templateCommand, command: { ...templateCommand.command,
    definition: { version: 1, nodes: [{ ...definition.nodes[0], tag: 'script' }] } } })).status, 400);
  assert.equal((await request('data', tokens[0], { ...templateCommand, command: { ...templateCommand.command,
    definition: { version: 1, nodes: [{ kind: 'field', field: 'privateUnknown' }] } } })).status, 400);
  const savedTemplate = await request('data', tokens[0], templateCommand);
  assert.equal(savedTemplate.status, 200, await savedTemplate.clone().text());
  assert.equal((await request('data', tokens[0], templateCommand)).status, 409);
  const templates = { action: 'templateViews', dataset: personalDataset.dataset };
  assert.equal((await request('data', tokens[1], { scope: personal, command: templates })).status, 403);
  assert.deepEqual(await (await request('data', tokens[0], { scope: personal, command: { action: 'datasets' } })).json(),
    { datasets: [{ dataset: personalDataset.dataset, access: 'manage' }] });
  assert.deepEqual(await (await request('data', tokens[1], { scope: personal, command: { action: 'datasets' } })).json(), { datasets: [] });
  const preview = { scope, command: { action: 'previewAudience', editors: [] } };
  assert.equal((await request('data', tokens[1], { ...preview, scope: { ...scope, generation: 2 } })).status, 403);
  assert.equal((await request('data', tokens[0], { ...preview, command: { ...preview.command, editors: [reader] } })).status, 400);
  const seen = await (await request('data', tokens[0], preview)).json();
  assert.deepEqual(seen.audience, [{ uid: owner, generation: 1, access: 'manage' }, { uid: reader, generation: 2, access: 'read' }]);
  const publish = { scope, operation: 'team-publish-order', command: {
    action: 'createTeam', schema: model, editors: [], audienceRevision: seen.audienceRevision,
  } };
  await member(reader).update({ generation: 3 });
  assert.equal((await request('data', tokens[0], publish)).status, 409);
  const adjusted = await (await request('data', tokens[0], preview)).json();
  publish.command.audienceRevision = adjusted.audienceRevision;
  const published = await request('data', tokens[0], publish);
  assert.equal(published.status, 200, await published.clone().text());
  const publishedDataset = await published.json();
  assert.equal(publishedDataset.epoch, 2);
  assert.deepEqual(await (await request('data', tokens[0], publish)).json(), publishedDataset);
  const readerScope = { ...scope, generation: 3 };
  const publication = { scope, operation: 'confirm-personal-publication', command: { action: 'publishPersonal',
    dataset: personalDataset.dataset, schemaVersion: 2, sourceEpoch, records: [privateReference], editors: [],
    audienceRevision: adjusted.audienceRevision, templates: [{ view: 'order-preview', version: 1 }] } };
  assert.equal((await request('data', tokens[0], { ...publication, command: { ...publication.command,
    templates: [{ view: 'order-preview', version: 2 }] } })).status, 409);
  const publishedPersonal = await request('data', tokens[0], publication);
  assert.equal(publishedPersonal.status, 200, await publishedPersonal.clone().text());
  const personalCopy = await publishedPersonal.json();
  assert.deepEqual(await (await request('data', tokens[0], publication)).json(), personalCopy);
  assert.equal(personalCopy.records.length, 1);
  const copied = await request('data', tokens[1], { scope: readerScope, command: { action: 'get', reference: personalCopy.records[0] } });
  assert.equal(copied.status, 200, await copied.clone().text());
  assert.deepEqual((await copied.json()).record.value, { title: 'Selected personal draft', reviewed: false });
  const copiedDescription = await request('data', tokens[1], { scope: readerScope, command: { action: 'describe', dataset: personalCopy.dataset } });
  assert.deepEqual((await copiedDescription.json()).views['triage-form'], { kind: 'form', fields: ['reviewed', 'title'] });
  const copiedTemplates = await request('data', tokens[1], { scope: readerScope, command: { ...templates, dataset: personalCopy.dataset } });
  assert.deepEqual(await copiedTemplates.json(), { views: [{ view: 'order-preview', version: 1, schemaVersion: 1, definition }] });
  assert.equal((await request('data', tokens[1], { scope: personal, command: { action: 'get', reference: privateReference } })).status, 403);
  const description = { scope: readerScope, command: { action: 'describe', dataset: publishedDataset.dataset } };
  assert.equal((await request('data', tokens[1], description)).status, 200);
  assert.equal((await request('data', tokens[1], { ...description, command: {
    ...write.command, dataset: publishedDataset.dataset,
  }, operation: 'unapproved-edit' })).status, 403);
  await member(reader).delete();
  assert.equal((await request('data', tokens[1], description)).status, 403);
  const recalled = await request('messages', tokens[0], { ...message, operation: 'recall', changeId: 'recall-order', text: '', dataRef: undefined });
  assert.equal(recalled.status, 200, await recalled.clone().text());
  assert.equal((await thread.collection('messages').doc(message.messageId).get()).data().dataRef, null);
  assert.deepEqual(JSON.parse(await readFile(fixture.actorFile, 'utf8')), {});

});
