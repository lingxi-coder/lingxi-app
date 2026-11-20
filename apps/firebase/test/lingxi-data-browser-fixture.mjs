import { readFile } from 'node:fs/promises';
import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';

import { createProductDataTransport } from '../src/lingxi-data-product.mjs';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

export async function createProductDataBrowserFixture(fixture) {
  const app = initializeApp({ projectId: process.env.GCLOUD_PROJECT }, 'lingxi-data-browser');
  const db = getFirestore(app);
  const data = createProductDataTransport(fixture.endpoint);
  const serve = createTeamsHandler({ store: new TeamsStore(db), data, verifyToken: token => getAuth(app).verifyIdToken(token, true) });
  const [uid, tokenFile] = fixture.owner;
  const token = await readFile(tokenFile, 'utf8');
  return {
    uid,
    record: fixture.reference.record,
    binding: { dataset: fixture.reference.dataset, scope: { kind: 'team', teamId: fixture.team, conversationId: 'orders-chat', generation: 1 } },
    async request(input) {
      const response = await serve(new Request('https://teams.test/v1/teams/data', {
        method: 'POST', headers: { Authorization: 'Bearer ' + token }, body: JSON.stringify(input),
      }));
      const result = await response.json();
      if (!response.ok) throw new Error(result.error.code);
      return result;
    },
    async close() { await db.terminate(); await deleteApp(app); },
  };
}
