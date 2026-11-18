import assert from 'node:assert/strict';
import { after, before, beforeEach, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { applyVisibility, describeVisibility, planVisibility, visibilityOf } from '../scripts/teams-backfill-task-visibility.mjs';

const options = { skip: !process.env.FIRESTORE_EMULATOR_HOST, timeout: 60000 };
const projectId = 'demo-lingxi-teams-t0', teamId = 'visibility-team';
let app, db, environment;

before(async () => {
  if (!process.env.FIRESTORE_EMULATOR_HOST) return;
  const [host, port] = process.env.FIRESTORE_EMULATOR_HOST.split(':'); assert.ok(['127.0.0.1', 'localhost'].includes(host));
  environment = await initializeTestEnvironment({ projectId, firestore: { host, port: Number(port), rules: readFileSync(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8') } });
  app = initializeApp({ projectId }, 'teams-backfill-task-visibility-tests'); db = getFirestore(app);
});
beforeEach(async () => { await environment?.clearFirestore(); });
after(async () => { await environment?.cleanup(); await db?.terminate(); if (app) await deleteApp(app); });

test('a task is asked unless it is an optional run in a group or channel; such a run is shown once it replied or needs checking', () => {
  assert.equal(visibilityOf({ state: 'completed' }, 'group'), 'asked');
  assert.equal(visibilityOf({ optional: true, state: 'declined' }, 'bot_dm'), 'asked');
  assert.equal(visibilityOf({ optional: true, state: 'declined' }, 'channel'), 'hidden');
  assert.equal(visibilityOf({ optional: true, state: 'completed', replyId: 'reply' }, 'group'), 'shown');
  assert.equal(visibilityOf({ optional: true, state: 'needs_reconciliation' }, 'group'), 'shown');
  assert.equal(visibilityOf({ optional: true, state: 'queued' }, undefined), 'hidden', 'a task whose Chat is gone is listed by nobody');
});

test('the backfill sets each task without a visibility once, and leaves one that has it', options, async () => {
  const team = db.collection('teams').doc(teamId);
  await team.collection('conversations').doc('group').set({ kind: 'group' });
  await team.collection('conversations').doc('dm').set({ kind: 'bot_dm' });
  const tasks = {
    asked: { conversationId: 'group', state: 'completed', replyId: 'r1' },
    dm: { conversationId: 'dm', optional: true, state: 'declined' },
    silent: { conversationId: 'group', optional: true, state: 'declined' },
    answered: { conversationId: 'group', optional: true, state: 'completed', replyId: 'r2' },
    current: { conversationId: 'group', optional: true, state: 'queued', visibility: 'hidden' },
  };
  for (const [id, data] of Object.entries(tasks)) await team.collection('tasks').doc(id).set(data);
  const plan = await planVisibility(db);
  assert.equal(plan.scanned, 5); assert.equal(plan.set, 1);
  assert.deepEqual(Object.fromEntries(plan.updates.map(update => [update.ref.id, update.visibility])), { asked: 'asked', dm: 'asked', silent: 'hidden', answered: 'shown' });
  assert.equal((await team.collection('tasks').doc('silent').get()).get('visibility'), undefined, 'planning writes nothing');
  assert.match(describeVisibility(plan, false).join('\n'), /Dry run/u);
  assert.equal(await applyVisibility(db, plan.updates), 4);
  const written = Object.fromEntries((await team.collection('tasks').get()).docs.map(doc => [doc.id, doc.get('visibility')]));
  assert.deepEqual(written, { answered: 'shown', asked: 'asked', current: 'hidden', dm: 'asked', silent: 'hidden' });
  assert.equal((await planVisibility(db)).updates.length, 0, 'running it again finds nothing to do');
});
