import assert from 'node:assert/strict';
import { initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { deliverDatasetTaskOutput } from '../src/dataset-task-delivery.mjs';

const projectId = process.env.GCLOUD_PROJECT;
if (!projectId?.startsWith('demo-') || !process.env.FIRESTORE_EMULATOR_HOST) throw new Error('Expected fixture project');
const db = getFirestore(initializeApp({ projectId }));
const id = process.argv[2];
assert.match(id, /^[A-Za-z0-9_-]{43}$/u);
assert.equal(await deliverDatasetTaskOutput(db, id), 'delivered');
assert.equal(await deliverDatasetTaskOutput(db, id), 'delivered');
const entry = (await db.collection('datasetV2Deliveries').doc(id).get()).data();
const { team, conversation } = entry.destination;
const msg = (await db.doc(`teams/${team}/conversations/${conversation}/messages/${entry.messageId}`).get()).data();
assert.deepEqual(msg.resourceRef, entry.reference);
assert.equal(msg.author.kind, 'task');
assert.equal(msg.author.taskId, entry.task.task);
assert.equal(msg.author.logicalRun, entry.task.logicalRun);
