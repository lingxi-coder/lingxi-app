import { readFileSync } from 'node:fs';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { onRequest } from 'firebase-functions/v2/https';
import { defineSecret } from 'firebase-functions/params';
import { createMarketGateway } from './market-gateway.mjs';
import { createHandler } from './catalog.mjs';
import { CatalogStore } from './store.mjs';

const app = initializeApp();
const seed = JSON.parse(readFileSync(new URL('./seed.json', import.meta.url), 'utf8'));
const handle = createHandler(new CatalogStore(getFirestore(app, 'model-catalog'), seed),
  (token) => getAuth(app).verifyIdToken(token, true),
  ['https://lingxios.net', 'https://lingxi-5e4b2.web.app', 'https://lingxi-5e4b2.firebaseapp.com', 'http://localhost:5173', 'http://127.0.0.1:5173']);

export const catalogApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '512MiB', timeoutSeconds: 60, invoker: 'public', serviceAccount: 'catalog-api@lingxi-5e4b2.iam.gserviceaccount.com' }, async (req, res) => {
  const url = new URL(req.url, 'https://catalog.internal');
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const input = new Request(url, { method: req.method, headers, ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) });
  const output = await handle(input);
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

const marketProxySecret = defineSecret('market-proxy-secret');
export const marketGateway = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, concurrency: 4, memory: '512MiB', timeoutSeconds: 60, invoker: 'public', serviceAccount: 'market-api@lingxi-5e4b2.iam.gserviceaccount.com', secrets: [marketProxySecret] }, createMarketGateway({ origin: 'https://market-api-55470483519.us-central1.run.app', secret: () => marketProxySecret.value() }));
