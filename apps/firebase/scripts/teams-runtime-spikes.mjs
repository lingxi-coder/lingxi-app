import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { mkdirSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';

assert.ok(process.env.FIRESTORE_EMULATOR_HOST && process.env.FIREBASE_AUTH_EMULATOR_HOST, 'run inside the isolated Team emulator runner');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const scratch = mkdtempSync(join(tmpdir(), 'lingxi-teams-runtimes-'));
const electronRoot = join(root, 'apps/electron');
const desktopRequire = createRequire(join(electronRoot, 'package.json'));
const { build } = desktopRequire('esbuild');
symlinkSync(join(electronRoot, 'node_modules'), join(scratch, 'node_modules'), 'dir');
mkdirSync(join(scratch, 'userData'));
await build({ entryPoints: [join(electronRoot, 'test/fixtures/teams-main-spike.ts')], bundle: true, platform: 'node', format: 'esm',
  external: ['electron', 'firebase/app', 'firebase/auth', 'firebase/firestore'], outfile: join(scratch, 'electron-main.mjs') });
const mainAdminApp = initializeApp({ projectId: 'demo-lingxi-teams-t0' }, 'teams-electron-seed');
const mainAdmin = getFirestore(mainAdminApp);
try {
  await mainAdmin.doc('teams/electron-spike').set({ name: 'Synthetic runtime probe' });
  await mainAdmin.doc('teams/electron-spike/members/alice').set({ role: 'member', generation: 1 });
  await mainAdmin.doc('teams/electron-spike/conversations/sdk-chat').set({ kind: 'dm', participants: ['user:alice'], memberUids: ['alice'], lastSeq: 0, lastChangeSeq: 0 });
  const [host, firestorePort] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  const [, authPort] = process.env.FIREBASE_AUTH_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  const environment = { ...process.env, TEAM_SPIKE_USER_DATA: join(scratch, 'userData'), TEAM_SPIKE_SESSION: JSON.stringify({
    config: { projectId: 'demo-lingxi-teams-t0', apiKey: 'emulator-only', appId: 'teams-real-electron' }, uid: 'alice',
    customToken: await getAuth(mainAdminApp).createCustomToken('alice'), emulator: { host, authPort: Number(authPort), firestorePort: Number(firestorePort) },
  }) };
  delete environment.ELECTRON_RUN_AS_NODE;
  const executable = desktopRequire('electron');
  await new Promise((resolve, reject) => {
    const child = spawn(executable, [join(scratch, 'electron-main.mjs')], { env: environment, stdio: ['ignore', 'pipe', 'inherit'] });
    let output = '', seeded = false;
    child.stdout.on('data', chunk => {
      process.stdout.write(chunk);
      output += chunk;
      if (!seeded && output.includes('TEAMS_ELECTRON_LISTENER_READY')) {
        seeded = true;
        mainAdmin.doc('teams/electron-spike/conversations/sdk-chat/changes/runtime01').set({ seq: 1, messageId: 'message01', operation: 'create', actorUid: 'alice', at: new Date() }).catch(reject);
      }
    });
    child.once('error', reject);
    child.once('exit', code => code === 0 && output.includes('TEAMS_ELECTRON_SPIKE_PASS') ? resolve() : reject(new Error(`Electron spike failed: ${code}`)));
  });
} finally { await mainAdmin.terminate(); await deleteApp(mainAdminApp); }

await build({ entryPoints: [join(root, 'apps/web/test/support/teamsBrowserSpike.ts')], bundle: true, platform: 'browser', format: 'esm', target: 'es2022', outfile: join(scratch, 'browser.mjs') });
const adminApp = initializeApp({ projectId: 'demo-lingxi-teams-t0' }, 'teams-real-browser');
const admin = getFirestore(adminApp);
let server;
try {
  await admin.doc('teams/browser-spike').set({ name: 'Synthetic browser runtime' });
  await admin.doc('teams/browser-spike/members/alice').set({ role: 'member', generation: 1 });
  await admin.doc('teams/browser-spike/conversations/sdk-chat').set({ kind: 'dm', participants: ['user:alice'], memberUids: ['alice'], lastSeq: 1, lastChangeSeq: 2 });
  await admin.doc('teams/browser-spike/conversations/sdk-chat/changes/change002').set({ seq: 2, messageId: 'message01', operation: 'recall', actorUid: 'alice', at: new Date() });
  const [host, firestorePort] = process.env.FIRESTORE_EMULATOR_HOST.split(':');
  assert.ok(['127.0.0.1', 'localhost'].includes(host));
  const settings = { projectId: 'demo-lingxi-teams-t0', customToken: await getAuth(adminApp).createCustomToken('alice'), authHost: process.env.FIREBASE_AUTH_EMULATOR_HOST, firestoreHost: host, firestorePort: Number(firestorePort) };
  const result = new Promise((resolve, reject) => {
    server = createServer(async (request, response) => {
      response.setHeader('Cache-Control', 'no-store');
      // Exercise browser SDK auth/WebChannel under an explicit local-only CSP.
      response.setHeader('Content-Security-Policy', `default-src 'none'; script-src 'self' 'unsafe-inline'; connect-src 'self' http://${host}:${firestorePort} http://${settings.authHost}; base-uri 'none'; frame-ancestors 'none'`);
      if (request.url === '/browser.mjs') { response.setHeader('Content-Type', 'text/javascript'); response.end(readFileSync(join(scratch, 'browser.mjs'))); }
      else if (request.method === 'POST' && request.url === '/result') {
        let body = '';
        for await (const chunk of request) { body += chunk; if (body.length > 8192) { request.destroy(); return; } }
        try {
          const result = JSON.parse(body);
          writeFileSync(join(scratch, 'browser-result.json'), JSON.stringify(result, null, 2));
          response.end('ok');
          result.passed ? resolve(result) : reject(new Error(result.error));
        } catch (error) { response.statusCode = 400; response.end(); reject(error); }
      } else {
        response.setHeader('Content-Type', 'text/html');
        response.end(`<!doctype html><meta charset="utf-8"><title>Teams SDK probe</title><h1>Teams browser SDK probe</h1><pre id="result">Connecting</pre><script>window.teamSpike=${JSON.stringify(settings)}</script><script type="module" src="/browser.mjs"></script>`);
      }
    });
    server.once('error', reject);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}`;
  console.log('TEAMS_BROWSER_SPIKE_URL', url, 'evidence:', scratch);
  let timeout;
  try {
    const completed = await Promise.race([result, new Promise((_, reject) => {
      timeout = setTimeout(() => reject(new Error('Browser Team spike timed out')), 120_000);
    })]);
    console.log('TEAMS_BROWSER_SPIKE_PASS', JSON.stringify(completed));
  } finally { clearTimeout(timeout); }
} finally {
  server?.closeAllConnections(); server?.close();
  await admin.terminate(); await deleteApp(adminApp);
}
