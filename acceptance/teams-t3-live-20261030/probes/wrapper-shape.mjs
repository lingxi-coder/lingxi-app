import { createRequire } from 'node:module';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
const P = fileURLToPath(new URL('../../../', import.meta.url)).replace(/\/$/, '');
const require = createRequire(`${P}/apps/firebase/package.json`);
const { onDocumentCreated } = require('firebase-functions/v2/firestore');
const { initializeApp, deleteApp } = require('firebase-admin/app');
const { onTeamMessageCreated } = await import(`${P}/apps/firebase/src/team-message-push.mjs`);
const app = initializeApp({ projectId: 'demo-t3-shape-only' });
const now = new Date(), iso = now.toISOString(), timestamp = { seconds: Math.floor(now.getTime() / 1000), nanos: (now.getTime() % 1000) * 1000000 };
const document = 'teams/t3-shape-team/conversations/t3-shape-thread/messages/t3-shape-message';
const value = { name: `projects/demo-t3-shape-only/databases/(default)/documents/${document}`, fields: {
  author: { mapValue: { fields: { kind: { stringValue: 'user' }, uid: { stringValue: 'synthetic-origin' } } } },
  changeId: { stringValue: 't3-shape-change' }, createdAt: { timestampValue: iso },
}, createTime: iso, updateTime: iso };
const protoValue = structuredClone(value);
protoValue.fields.createdAt.timestampValue = timestamp; protoValue.createTime = timestamp; protoValue.updateTime = timestamp;
const EventData = require(`${P}/apps/firebase/node_modules/firebase-functions/protos/compiledFirestore.js`).google.events.cloud.firestore.v1.DocumentEventData;
const protobuf = Buffer.from(EventData.encode(EventData.fromObject({ value: protoValue })).finish());
const base = { specversion: '1.0', id: 'synthetic-event', type: 'google.cloud.firestore.document.v1.created',
  source: '//firestore.googleapis.com/projects/demo-t3-shape-only/databases/(default)', subject: `documents/${document}`,
  time: iso, project: 'demo-t3-shape-only', database: '(default)', namespace: '(default)', document };
const cases = [
  ['cloud-event-json', { ...base, datacontenttype: 'application/json', data: { value } }],
  ['cloud-event-protobuf', { ...base, datacontenttype: 'application/protobuf', data: protobuf }],
  ['legacy-event-data-only-protobuf', protobuf],
  ['legacy-event-data-only-json', { value }],
  ['http-request-shape', { method: 'POST', body: protobuf, headers: { 'ce-document': document } }],
];
const results = [];
try {
  for (const [name, input] of cases) {
    let capture, reads = 0;
    const fn = onDocumentCreated({ document: 'teams/{teamId}/conversations/{conversationId}/messages/{messageId}', database: '(default)' }, event => {
      const original = event.data?.data();
      capture = { paramsKeys: Object.keys(event.params), idLengths: Object.fromEntries(Object.entries(event.params).map(([key, value]) => [key, value?.length])),
        snapshotExists: event.data?.exists ?? false, originalKeys: original ? Object.keys(original) : [],
        createdAtType: original?.createdAt?.constructor?.name ?? null, createdAtHasToMillis: typeof original?.createdAt?.toMillis === 'function' };
      return onTeamMessageCreated({ db: { doc() { reads++; throw Object.assign(new Error('READ_BOUNDARY'), { code: 'READ_BOUNDARY' }); } },
        pushStore: {}, messaging: {}, ...event.params, original });
    });
    let outcome;
    try { await fn(input); outcome = 'RETURNED_WITHOUT_DB_READ'; }
    catch (error) { outcome = error.code === 'READ_BOUNDARY' ? 'ENTERED_DB_READ' : error.code ?? error.name; }
    results.push({ name, outcome, dbReads: reads, ...capture });
  }
} finally { await deleteApp(app); }
const proof = { localOnly: true, networkCalls: 0, actualSdkWrapper: true, actualProductHandlerGuard: true, results };
for (const result of results) {
  const cloudEvent = result.name.startsWith('cloud-event-');
  assert.equal(result.outcome, cloudEvent ? 'ENTERED_DB_READ' : 'RETURNED_WITHOUT_DB_READ');
  assert.equal(result.dbReads, cloudEvent ? 1 : 0);
  assert.equal(result.paramsKeys.length, cloudEvent ? 3 : 0);
  assert.equal(result.snapshotExists, cloudEvent);
  assert.equal(result.createdAtHasToMillis, cloudEvent);
}
console.log(JSON.stringify(proof, null, 2));
