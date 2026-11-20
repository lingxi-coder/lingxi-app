import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { initializeTestEnvironment } from '@firebase/rules-unit-testing';
import { deleteApp, initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { createProductDataTransport } from '../src/lingxi-data-product.mjs';
import { createTeamsHandler, TeamsStore } from '../src/teams.mjs';

const { GCLOUD_PROJECT: project, FIREBASE_AUTH_EMULATOR_HOST: auth, FIRESTORE_EMULATOR_HOST: firestore,
  LINGXI_DATA_FIXTURE: fixturePath, LINGXI_DATA_STAGING_PORT: portValue } = process.env;
const webPort = Number(portValue);
const host = '127.0.0.1';
if (project !== 'demo-lingxi-data' || ![auth, firestore].every(value => /^127\.0\.0\.1:[0-9]+$/u.test(value ?? ''))
  || !Number.isInteger(webPort) || webPort < 1024 || webPort > 65535 || !fixturePath) {
  throw new Error('Staging requires an isolated loopback demo project and port.');
}

const fixture = JSON.parse(await readFile(fixturePath, 'utf8'));
const rules = await readFile(new URL('../../../infra/firebase/firestore.default.rules', import.meta.url), 'utf8');
const rulesEnvironment = await initializeTestEnvironment({ projectId: project,
  firestore: { host, port: Number(firestore.split(':')[1]), rules } });
await rulesEnvironment.cleanup();
const app = initializeApp({ projectId: project }, 'lingxi-data-staging');
const db = getFirestore(app);
const data = createProductDataTransport(fixture.endpoint);
const webOrigin = `http://${host}:${webPort}`;
const teams = createTeamsHandler({ store: new TeamsStore(db), data, verifyToken: token => getAuth(app).verifyIdToken(token, true),
  origins: [webOrigin] });
const ownerToken = (await readFile(fixture.owner[1], 'utf8')).trim();
const readerToken = (await readFile(fixture.reader[1], 'utf8')).trim();
const teamId = fixture.team;
const conversationId = 'orders-chat';
const scope = { kind: 'team', teamId, conversationId, generation: 1 };

async function command(path, token, body) {
  const response = await teams(new Request(webOrigin + path, {
    method: 'POST', headers: { Authorization: 'Bearer ' + token }, body: JSON.stringify(body),
  }));
  const value = await response.json();
  if (!response.ok) throw new Error(`Staging seed failed for ${body.action ?? body.command.action}: ${response.status} ${value.error?.code}`);
  return value;
}

await command('/v1/teams', ownerToken, { action: 'create', teamId, name: 'Data Staging' });
const invite = await command('/v1/teams', ownerToken, { action: 'invite', teamId, requestId: 'staging-reader',
  role: 'member', maxUses: 1, ttlSeconds: 3600 });
await command('/v1/teams', readerToken, { action: 'join', code: invite.code, requestId: 'staging-reader-join' });
await command('/v1/teams/messages', ownerToken, { action: 'create', teamId, conversationId, generation: 1,
  kind: 'channel', title: 'Orders', participantUids: [fixture.owner[0], fixture.reader[0]] });
const product = (commandBody, operation) => command('/v1/teams/data', ownerToken,
  { scope, command: commandBody, ...(operation ? { operation } : {}) });
const { audienceRevision } = await product({ action: 'previewAudience', editors: [] });
const { dataset } = await product({ action: 'createTeam', audienceRevision, editors: [],
  schema: { type: 'object', additionalProperties: false, required: ['title'], properties: { title: { type: 'string' } } } }, 'staging-orders');
const reference = await product({ action: 'put', dataset, record: 'order-1', schemaVersion: 1, expectedRevision: 0,
  value: { title: 'Prepare shipment' } }, 'staging-order');
await product({ action: 'setTemplateView', dataset, view: 'order-summary', schemaVersion: 1, expectedVersion: 0,
  definition: { version: 1, nodes: [{ kind: 'element', tag: 'h3', children: [{ kind: 'field', field: 'title' }] }] } });
await command('/v1/teams/messages', ownerToken, { action: 'mutate', teamId, conversationId, generation: 1, operation: 'create',
  messageId: 'staging-message', changeId: 'staging-change', text: 'Review the shipping order', dataRef: reference });

const api = createServer(async (incoming, outgoing) => {
  try {
    const url = new URL(incoming.url ?? '/', webOrigin);
    if (url.origin !== webOrigin || !/^\/v1\/(?:teams|services)(?:\/|$)/u.test(url.pathname)) {
      outgoing.writeHead(404).end();
      return;
    }
    const withBody = incoming.method !== 'GET' && incoming.method !== 'HEAD';
    const response = await teams(new Request(url, { method: incoming.method, headers: incoming.headers,
      ...(withBody ? { body: incoming, duplex: 'half' } : {}) }));
    outgoing.writeHead(response.status, Object.fromEntries(response.headers));
    outgoing.end(Buffer.from(await response.arrayBuffer()));
  } catch { outgoing.writeHead(400).end(); }
});
await new Promise((resolve, reject) => { api.once('error', reject); api.listen(0, host, resolve); });
const apiOrigin = `http://${host}:${api.address().port}`;

const webRoot = fileURLToPath(new URL('../../web/', import.meta.url));
const require = createRequire(new URL('../../web/package.json', import.meta.url));
const { createServer: createWeb } = await import(pathToFileURL(require.resolve('vite')).href);
Object.assign(process.env, {
  VITE_AUTH_EMULATOR_URL: 'http://' + auth,
  VITE_FIRESTORE_EMULATOR_HOST: firestore,
  VITE_FIREBASE_PROJECT_ID: project,
});
const web = await createWeb({ root: webRoot, server: { host, port: webPort, strictPort: true,
  proxy: { '/v1/teams': apiOrigin, '/v1/services': apiOrigin } } });
await web.listen();
console.log(JSON.stringify({ web: webOrigin + '/teams', api: apiOrigin, auth, firestore, project, teamId, conversationId, reference }));

async function close() {
  await web.close();
  await new Promise(resolve => api.close(resolve));
  await db.terminate();
  await deleteApp(app);
}
process.once('SIGTERM', () => { void close(); });
process.once('SIGINT', () => { void close(); });
