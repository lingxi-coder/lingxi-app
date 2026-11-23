import { createDatasetRender } from '../src/dataset-render.mjs';
import { getStorage } from 'firebase-admin/storage';
import { createDatasetPages } from '../src/dataset-pages.mjs';
import { createDatasetBrowserHttp } from '../src/dataset-browser-bridge.mjs';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { createPrivateDatasetFetch } from '../src/dataset-cloud-run.mjs';
import { createDatasetGateway } from '../src/dataset-product.mjs';
import { FriendsStore } from '../src/friends.mjs';
import { createDatasetAgentTurn, firestoreDatasetTurns } from '../src/dataset-agent-turn.mjs';
import { createChat } from '../src/llm-chat.mjs';
import { createLlmHandler } from '../src/llm-api.mjs';
import { upstreamTransport } from '../src/llm-egress.mjs';
import { parseKeyring, VaultStore } from '../src/llm-vault.mjs';
import { datasetBodyLimit, createTeamsHandler, TeamsStore } from '../src/teams.mjs';

const projectId = 'lingxi-dataset-v2-stg-261119';
if (process.env.LINGXI_DATA_PROJECT !== projectId || !process.env.K_SERVICE ||
    process.env.FIREBASE_AUTH_EMULATOR_HOST || process.env.FIRESTORE_EMULATOR_HOST) {
  throw new Error('Expected the dedicated Dataset staging workload');
}
const app = initializeApp({ projectId });
const db = getFirestore(app);
const auth = getAuth(app);
const lookup = new URL('https://identitytoolkit.googleapis.com/v1/accounts:lookup');
lookup.searchParams.set('key', process.env.LINGXI_FIREBASE_WEB_KEY);
async function verifySession(token) {
  const claims = await auth.verifyIdToken(token);
  const reply = await fetch(lookup, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ idToken: token }), redirect: 'error', signal: AbortSignal.timeout(5000) });
  if (!reply.ok) throw new Error('Invalid Firebase session');
  const user = (await reply.json()).users?.[0];
  const validSince = Number(user?.validSince);
  if (user?.localId !== claims.uid || user.disabled || !Number.isSafeInteger(validSince) || validSince <= 0 ||
      !Number.isSafeInteger(claims.auth_time) || claims.auth_time < validSince) throw new Error('Expired Firebase session');
  return claims;
}
const services = [process.env.DATASET_ORIGIN_A, process.env.DATASET_ORIGIN_B];
if (new Set(services).size !== 2) throw new Error('Expected two Dataset service origins');
const gateways = services.map(origin => createDatasetGateway(origin, createPrivateDatasetFetch(origin), verifySession));
let nextReplica = 0;
function next() {
  const gateway = gateways[nextReplica];
  nextReplica = (nextReplica + 1) % gateways.length;
  return gateway;
}
const pages = process.env.LINGXI_DATASET_PAGES_BUCKET ? createDatasetPages({
  db, bucket: getStorage(app).bucket(process.env.LINGXI_DATASET_PAGES_BUCKET), datasets: (...args) => next().datasets(...args),
}) : undefined;
const renderKeyFile = process.env.LINGXI_DATASET_RENDER_KEY_FILE;
const renderDataset = createDatasetRender({
  origin: process.env.LINGXI_DATASET_RENDER_ORIGIN,
  secret: renderKeyFile ? readFileSync(renderKeyFile, 'utf8').trim() : process.env.LINGXI_DATASET_RENDER_KEY,
  pages, db,
});
const origins = [`https://${projectId}.web.app`, `https://${projectId}.firebaseapp.com`];
const handle = createTeamsHandler({ store: new TeamsStore(db), friends: new FriendsStore(db, Date.now, { auth }),
  verifyToken: verifySession, datasets: (...args) => next().datasets(...args),
  datasetCommand: (...args) => next().command(...args), datasetRuns: (...args) => next().run(...args),
  datasetPages: pages?.execute, datasetRender: renderDataset,
  createCustomToken: uid => auth.createCustomToken(uid), origins });
const browser = createDatasetBrowserHttp({
  origin: process.env.LINGXI_DATASET_BROWSER_ORIGIN,
  call: (...args) => next().browser(...args),
});
const vaultFile = process.env.LINGXI_DATASET_VAULT_FILE;
let llm;
if (vaultFile) {
  const keyring = parseKeyring(readFileSync(vaultFile, 'utf8'));
  const vault = new VaultStore(db, () => keyring);
  const chat = createChat({ vault, transport: upstreamTransport() });
  const datasetTurn = createDatasetAgentTurn({
    turns: firestoreDatasetTurns(db), model: chat.start,
    data: (uid, token, body, signal) => next().datasets(db, { uid }, token, body, signal),
  });
  llm = createLlmHandler({ vault, chat: chat.chat, test: chat.test, datasetTurn, verifyToken: verifySession, origins });
}
const paths = new Set(['/v1/friends', '/v1/friends/profile', '/v1/teams', '/v1/services', '/v2/dataset-runs']);
createServer(async (incoming, outgoing) => {
  const abort = new AbortController();
  outgoing.once('close', () => { if (!outgoing.writableFinished) abort.abort(); });
  try {
    const url = new URL(incoming.url, 'https://dataset.internal');
    const model = url.pathname.startsWith('/v1/llm/');
    if (!paths.has(url.pathname) && !url.pathname.startsWith('/v1/teams/') &&
        !(browser && url.pathname.startsWith('/v1/dataset-browser/')) && !(llm && model)) {
      outgoing.writeHead(404).end(); return;
    }
    if (!['GET', 'POST', 'OPTIONS'].includes(incoming.method)) { outgoing.writeHead(405).end(); return; }
    const limit = datasetBodyLimit(url.pathname) ?? 3 * 1024 * 1024;
    const chunks = [];
    let bytes = 0;
    for await (const chunk of incoming) {
      bytes += chunk.length;
      if (bytes > limit) { outgoing.writeHead(413).end(); return; }
      chunks.push(chunk);
    }
    const request = new Request(url, { method: incoming.method, headers: incoming.headers, signal: abort.signal,
      ...(chunks.length ? { body: Buffer.concat(chunks) } : {}) });
    const result = model ? await llm(request, abort.signal) : await browser?.(request) ?? await handle(request);
    outgoing.writeHead(result.status, Object.fromEntries(result.headers));
    if (result.body) await pipeline(Readable.fromWeb(result.body), outgoing, { signal: abort.signal });
    else outgoing.end();
  } catch {
    if (outgoing.destroyed) return;
    if (outgoing.headersSent) outgoing.destroy();
    else outgoing.writeHead(503, { 'Cache-Control': 'no-store' }).end();
  }
}).listen(Number(process.env.PORT ?? 8080), '0.0.0.0');
