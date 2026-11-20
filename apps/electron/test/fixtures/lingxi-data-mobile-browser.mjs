import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { freePort, loadSourceHost } from './local-app-source-host.mjs';

async function connectStaging(file) {
  const log = await readFile(file, 'utf8');
  const fixture = JSON.parse(log.split('\n').find(line => line.startsWith('{"web"')));
  assert.equal(fixture.project, 'demo-lingxi-data');
  assert.match(fixture.auth, /^127\.0\.0\.1:[0-9]+$/u);
  assert.match(fixture.api, /^http:\/\/127\.0\.0\.1:[0-9]+$/u);
  const signIn = await fetch(`http://${fixture.auth}/identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=demo`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email: 'owner@fixture.invalid', password: 'AccountFixture123', returnSecureToken: true }),
  });
  assert.equal(signIn.status, 200);
  const { idToken, localId: uid } = await signIn.json();
  const binding = {
    dataset: fixture.reference.dataset,
    scope: { kind: 'team', teamId: fixture.teamId, conversationId: fixture.conversationId, generation: 1 },
  };
  async function request(input) {
    const response = await fetch(fixture.api + '/v1/teams/data', {
      method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + idToken },
      body: JSON.stringify(input),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error.code);
    return result;
  }
  return { binding, request, uid };
}

async function makeRelease(directory, template, record, title) {
  await mkdir(directory);
  const source = `
    import { createRoot } from 'react-dom/client';
    import { DataTemplate } from './DataTemplate.tsx';

    const button = document.querySelector('button');
    const status = document.querySelector('[role=status]');
    const root = createRoot(document.querySelector('#visualization'));
    button.addEventListener('click', async () => {
      button.disabled = true;
      try {
        const commands = window.localApp.data;
        const reference = await commands.request('orders', {
          action: 'put', record: ${JSON.stringify(record)}, schemaVersion: 1, expectedRevision: 0,
          value: { title: ${JSON.stringify(title)} },
        }, ${JSON.stringify(record)});
        const { record: stored } = await commands.request('orders', { ...reference, action: 'get' });
        root.render(<DataTemplate spec={${JSON.stringify(template)}} value={stored.value} />);
        status.textContent = 'Saved revision ' + reference.revision;
      } catch {
        status.textContent = 'Could not save staging record';
        button.disabled = false;
      }
    });
    if (window.localApp?.data) button.disabled = false;
  `;
  const bundled = await build({
    stdin: { contents: source, resolveDir: fileURLToPath(new URL('../../../../packages/teams/', import.meta.url)), loader: 'tsx' },
    nodePaths: [fileURLToPath(new URL('../../node_modules/', import.meta.url))],
    jsx: 'automatic', bundle: true, write: false, platform: 'browser', minify: true,
  });
  await writeFile(join(directory, 'browser.js'), bundled.outputFiles[0].contents);
  await writeFile(join(directory, 'index.html'), `<!doctype html><html lang="en"><head><meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1"><title>Local App data staging</title>
  <script src="/browser.js" defer></script></head><body><main>
  <h1>Local App data staging</h1><button disabled>Save staging record</button>
  <p role="status"></p><section aria-label="Saved order template" id="visualization"></section>
  </main></body></html>`);
}

async function run(file) {
  const product = await connectStaging(file);
  const { scope, dataset } = product.binding;
  const { views } = await product.request({ scope, command: { action: 'templateViews', dataset } });
  const template = views.find(view => view.view === 'order-summary')?.definition;
  assert.ok(template);

  const directory = await mkdtemp(join(tmpdir(), 'lingxi-mobile-data-'));
  let host, launcher;
  try {
    const id = randomUUID(), record = 'mobile-' + id, title = 'Packed on mobile ' + id.slice(0, 8);
    const release = join(directory, 'release');
    await makeRelease(release, template, record, title);
    const { LocalAppHost } = await loadSourceHost();
    const appId = 'a' + id.replaceAll('-', '');
    const revision = 1, runId = 'mobile-browser-staging';
    const owner = { accountUid: product.uid, deviceId: 'mobile-fixture', hostId: 'staging-fixture' };
    const row = { id: appId, name: 'Local App data staging', summary: '', revision, currentRevision: revision, releaseDigest: 'd'.repeat(64),
      background: false, grants: [], manifest: { schema: 2, targets: [], name: 'Local App data staging',
        entry: { type: 'static', path: 'index.html' }, capabilities: [], dataBindings: { orders: 'edit' } } };
    host = new LocalAppHost({ accountId: () => owner.accountUid, dataRequest: (request, uid) => {
      assert.equal(uid, product.uid);
      return product.request(request);
    }, configHome: directory, appProjectsRoot: join(directory, '.apps'), currentConversationMode: () => 'code',
    core: { onDisconnected() {}, async dispose() {} }, preload: '/unused', mainWindow: () => undefined, edit: async () => {} });
    host.store = { root: directory, get: async () => structuredClone(row), getRevision: async () => structuredClone(row),
      list: async () => [structuredClone(row)], directory: app => join(directory, app), assertTarget: async () => {},
      revisionPath: () => release, core: { request: async () => ({ digest: row.releaseDigest, files: [] }) } };
    let running = false;
    host.runtime = { phase: () => running ? 'running' : 'stopped', revision: () => running ? revision : undefined,
      invocationId: () => running ? runId : undefined,
      owner: () => running ? { appId, revision, runId, releaseDigest: row.releaseDigest } : undefined,
      manifest: () => row.manifest, workspace: () => release,
      exclusive: async (_id, operation) => operation(),
      async start(_id, _repair, signal) { signal?.throwIfAborted(); running = true; },
      async stop() { running = false; }, async dispose() { running = false; } };

    const gatewayPort = await freePort();
    process.env.LINGXI_LOCAL_APP_REMOTE_ORIGIN = `http://{appId}-{sessionId}.localhost:${gatewayPort}`;
    process.env.LINGXI_LOCAL_APP_GATEWAY_PORT = String(gatewayPort);
    await host.request({ action: 'set-data-binding', id: appId, revision, alias: 'orders', binding: product.binding },
      undefined, { kind: 'native', mode: 'chat' });
    await host.setRemoteAccess(appId, true);

    let launched = false;
    launcher = createServer(async (incoming, outgoing) => {
      try {
        if (incoming.method !== 'GET') return outgoing.writeHead(405).end();
        if (incoming.url === '/launch' && !launched) {
          launched = true;
          const { url } = await host.remoteOpen(appId, owner);
          outgoing.writeHead(303, { Location: url, 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer' }).end();
          return;
        }
        if (incoming.url !== '/result') return outgoing.writeHead(404).end();
        const { record: stored } = await product.request({ scope, command: {
          action: 'get', reference: { dataset, record, revision: 1, schemaVersion: 1 },
        } });
        assert.equal(stored.actor.id, owner.accountUid);
        assert.equal(stored.value.title, title);
        outgoing.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' })
          .end(JSON.stringify({ record, revision: 1, title }));
      } catch {
        outgoing.writeHead(409).end();
      }
    });
    await new Promise((resolve, reject) => { launcher.once('error', reject); launcher.listen(0, '127.0.0.1', resolve); });
    console.log(JSON.stringify({ url: `http://127.0.0.1:${launcher.address().port}/launch`, gatewayPort, title }));
    await new Promise(resolve => {
      process.once('SIGINT', resolve);
      process.once('SIGTERM', resolve);
    });
  } finally {
    if (launcher) await new Promise(resolve => { launcher.close(resolve); launcher.closeAllConnections(); });
    await host?.dispose();
    await rm(directory, { recursive: true, force: true });
  }
}

await run(process.argv[2]);
