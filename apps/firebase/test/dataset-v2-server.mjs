import { readFileSync } from 'node:fs';
import { createServer } from 'node:https';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { createDatasetGateway } from '../src/dataset-product.mjs';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

const [port, core, cert, key] = process.argv.slice(2);
const projectId = process.env.GCLOUD_PROJECT;
if (!projectId?.startsWith('demo-') || !process.env.FIRESTORE_EMULATOR_HOST || !process.env.FIREBASE_AUTH_EMULATOR_HOST) {
  throw new Error('Expected isolated emulators');
}
const app = initializeApp({ projectId }), db = getFirestore(app);
const api = createDatasetGateway(core);
const handle = createTeamsHandler({ store: new TeamsStore(db), datasets: api.datasets,
  datasetRuns: api.run, verifyToken: token => getAuth(app).verifyIdToken(token) });
createServer({ cert: readFileSync(cert), key: readFileSync(key) }, async (request, response) => {
  try {
    const parts = [];
    for await (const part of request) parts.push(part);
    const result = await handle(new Request('https://fixture.invalid' + request.url, { method: 'POST',
      headers: request.headers, body: Buffer.concat(parts) }));
    response.writeHead(result.status, { 'Content-Type': 'application/json' });
    response.end(Buffer.from(await result.arrayBuffer()));
  } catch {
    response.writeHead(503).end();
  }
}).listen(Number(port), '127.0.0.1');
