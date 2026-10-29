import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { TeamsStore, createTeamsHandler } from '../src/teams.mjs';

if (!process.env.FIRESTORE_EMULATOR_HOST?.startsWith('127.0.0.1:') || !process.env.FIREBASE_AUTH_EMULATOR_HOST?.startsWith('127.0.0.1:')) throw new Error('Local emulators required');
const root = resolve(fileURLToPath(new URL('../../..', import.meta.url)));
const scratch = mkdtempSync(join(tmpdir(), 'lingxi-teams-members-runtime-'));
const projectId = 'demo-lingxi-teams-t0', app = initializeApp({ projectId }, 'teams-members-runtime');
const auth = getAuth(app), db = getFirestore(app), origins = [];
const handler = createTeamsHandler({ store: new TeamsStore(db), verifyToken: token => auth.verifyIdToken(token, true), createCustomToken: uid => auth.createCustomToken(uid), origins });
const server = createServer(async (req, res) => {
  try {
    const parts = []; for await (const part of req) parts.push(part);
    const response = await handler(new Request(new URL(req.url, 'http://fixture.local'), { method: req.method, headers: req.headers,
      ...(!['GET', 'HEAD'].includes(req.method) ? { body: Buffer.concat(parts) } : {}) }));
    res.writeHead(response.status, Object.fromEntries(response.headers)); res.end(Buffer.from(await response.arrayBuffer()));
  } catch { res.writeHead(500); res.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
let vite;
try {
  for (const uid of ['teams-owner', 'teams-member']) await auth.createUser({ uid, email: `${uid}@example.test`, password: 'Synthetic-pass-2026', displayName: uid === 'teams-owner' ? 'Team Owner' : 'Team Member', emailVerified: true });
  const tokenResponse = await fetch(`http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}/identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=synthetic-api-key`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ email: 'teams-owner@example.test', password: 'Synthetic-pass-2026', returnSecureToken: true }) });
  const { idToken } = await tokenResponse.json();
  const { build } = await import(pathToFileURL(join(root, 'apps/electron/node_modules/esbuild/lib/main.js')).href);
  const main = join(scratch, 'main.cjs'), preload = join(scratch, 'preload.cjs');
  await build({ entryPoints: [join(root, 'apps/electron/test/fixtures/teams-members-main.ts')], outfile: main, bundle: true, platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'error' });
  await build({ entryPoints: [join(root, 'apps/electron/src/preload/index.ts')], outfile: preload, bundle: true, platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'error' });
  await new Promise((resolve, reject) => {
    const child = spawn(join(root, 'apps/electron/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron'), [main], { stdio: 'inherit', env: { ...process.env,
      TEAMS_MEMBERS_FIXTURE: JSON.stringify({ uid: 'teams-owner', idToken, config: { apiKey: 'synthetic-api-key', projectId }, base, preload,
        userData: join(scratch, 'electron-profile'), emulator: { host: '127.0.0.1', authPort: Number(process.env.FIREBASE_AUTH_EMULATOR_HOST.split(':')[1]), firestorePort: Number(process.env.FIRESTORE_EMULATOR_HOST.split(':')[1]) } }) } });
    const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error('Electron T1 probe timed out')); }, 60000);
    child.on('error', reject); child.on('exit', code => { clearTimeout(timer); code === 0 ? resolve() : reject(new Error(`Electron probe exited ${code}`)); });
  });
  process.env.VITE_AUTH_EMULATOR_URL = `http://${process.env.FIREBASE_AUTH_EMULATOR_HOST}`;
  process.env.VITE_FIREBASE_PROJECT_ID = projectId;
  process.env.VITE_FIRESTORE_EMULATOR_HOST = process.env.FIRESTORE_EMULATOR_HOST;
  const { createServer: createVite } = await import(pathToFileURL(join(root, 'apps/web/node_modules/vite/dist/node/index.js')).href);
  vite = await createVite({ root: join(root, 'apps/web'), configFile: join(root, 'apps/web/vite.config.ts'), server: { host: '127.0.0.1', port: 5173, strictPort: true, proxy: { '/v1/teams': base } } });
  await vite.listen(); origins.push('http://127.0.0.1:5173');
  console.log(`TEAMS_MEMBERS_BROWSER_READY: http://127.0.0.1:5173/teams (API ${base}; synthetic owner/member@example.test; no production access)`);
  console.log(`Evidence scratch: ${scratch}. Close with SIGINT after browser checks.`);
  await new Promise(resolve => {
    const timer = setInterval(() => { if (existsSync(join(scratch, 'stop'))) { clearInterval(timer); resolve(); } }, 500);
    const stop = () => { clearInterval(timer); resolve(); };
    process.once('SIGINT', stop); process.once('SIGTERM', stop);
  });
} finally { await vite?.close(); await new Promise(resolve => server.close(resolve)); await db.terminate(); await deleteApp(app); }
