import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { initializeApp } from 'firebase-admin/app';
import { getFirestore } from 'firebase-admin/firestore';
import { getAuth } from 'firebase-admin/auth';
import { CatalogStore } from '../src/store.mjs';
import { createHandler } from '../src/catalog.mjs';

if (!process.env.FIRESTORE_EMULATOR_HOST || !process.env.FIREBASE_AUTH_EMULATOR_HOST) throw Error('This development server requires both Firebase emulators.');
const app = initializeApp({ projectId: 'demo-lingxi-backend' });
const seed = JSON.parse(readFileSync(new URL('../src/seed.json', import.meta.url), 'utf8'));
const handle = createHandler(new CatalogStore(getFirestore(app, 'model-catalog'), seed), (token) => getAuth(app).verifyIdToken(token, true), ['http://127.0.0.1:5186']);
createServer(async (req, res) => {
  try {
    const chunks = []; let bytes = 0;
    for await (const chunk of req) { bytes += chunk.length; if (bytes > 8 * 1024 * 1024) { res.writeHead(413); res.end(); return; } chunks.push(chunk); }
    const response = await handle(new Request(new URL(req.url, 'http://127.0.0.1:4700'), { method: req.method, headers: req.headers, ...(!['GET', 'HEAD'].includes(req.method) ? { body: Buffer.concat(chunks) } : {}) }));
    res.writeHead(response.status, Object.fromEntries(response.headers)); res.end(Buffer.from(await response.arrayBuffer()));
  } catch { res.writeHead(500); res.end('Development server error'); }
}).listen(4700, '127.0.0.1', () => console.log('Local catalog backend on http://127.0.0.1:4700 (emulators only)'));
