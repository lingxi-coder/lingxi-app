import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { applyConversationKinds, describeConversationKinds, planConversationKinds } from '../scripts/inbox-backfill-conversation-kind.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-inbox-backfill';
let app, db, environment;

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':'); assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port), rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'inbox-backfill-conversation-kind-tests'); db = getFirestore(app);
});
beforeEach(async () => { await environment?.clearFirestore(); });
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('the backfill gives each team row without a conversation kind the kind of its conversation, once', options, async () => {
  const team = db.collection('teams').doc('t1');
  await team.collection('conversations').doc('dm').set({ kind: 'bot_dm' });
  await team.collection('conversations').doc('group').set({ kind: 'group' });
  const inbox = db.collection('users').doc('alice').collection('inbox');
  const rows = {
    bot: { ref: { kind: 'team', sourceId: 't1', conversationId: 'dm' } },
    people: { ref: { kind: 'team', sourceId: 't1', conversationId: 'group' } },
    current: { ref: { kind: 'team', sourceId: 't1', conversationId: 'group' }, conversationKind: 'group' },
    gone: { ref: { kind: 'team', sourceId: 't1', conversationId: 'deleted' } },
    service: { ref: { kind: 'service', sourceId: 's1', conversationId: 'dm' } },
    assistant: { ref: { kind: 'assistant', sourceId: 'h1', conversationId: 'x' } },
  };
  for (const [id, data] of Object.entries(rows)) await inbox.doc(id).set(data);
  // Another account's rows, whose user document does not exist (only its inbox does).
  await db.collection('users').doc('bob').collection('inbox').doc('bot').set(rows.bot);

  const plan = await planConversationKinds(db);
  assert.equal(plan.scanned, 5, 'team rows only'); assert.equal(plan.set, 1); assert.equal(plan.missing, 1);
  assert.deepEqual(plan.updates.map(update => [update.path, update.conversationKind]).sort(),
    [['users/alice/inbox/bot', 'bot_dm'], ['users/alice/inbox/people', 'group'], ['users/bob/inbox/bot', 'bot_dm']]);
  assert.equal((await inbox.doc('bot').get()).get('conversationKind'), undefined, 'planning writes nothing');
  assert.match(describeConversationKinds(plan, false).join('\n'), /Dry run/u);
  assert.equal(await applyConversationKinds(db, plan.updates), 3);
  assert.equal((await inbox.doc('bot').get()).get('conversationKind'), 'bot_dm');
  assert.equal((await inbox.doc('gone').get()).get('conversationKind'), undefined);
  assert.equal((await planConversationKinds(db)).updates.length, 0, 'running it again finds nothing to do');
});

test('a row changed since it was planned fails its batch rather than being overwritten', options, async () => {
  await db.collection('teams').doc('t1').collection('conversations').doc('dm').set({ kind: 'bot_dm' });
  const row = db.collection('users').doc('alice').collection('inbox').doc('bot');
  await row.set({ ref: { kind: 'team', sourceId: 't1', conversationId: 'dm' } });
  const plan = await planConversationKinds(db);
  await row.update({ preview: 'new' });
  await assert.rejects(applyConversationKinds(db, plan.updates));
});
