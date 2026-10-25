import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';
import { CoreLocalAppStore, type AppRun } from '../src/main/local-apps/coreStore.ts';
import { LocalAppRuntime, shellQuote } from '../src/main/local-apps/runtime.ts';
import { LocalAppServiceBroker } from '../src/main/local-apps/serviceBroker.ts';
const binary = process.env.LINGXI_LOCAL_APP_CORE_BIN;

test('real Rust library owns Desktop static, Python and Node fixture lifecycles', { skip: !binary, timeout: 120000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-real-core-')), config = join(root, 'config');
  const core = new LocalAppCoreClient(binary!, config);
  const store = new CoreLocalAppStore(join(config, 'local-apps/v2'), core), broker = new LocalAppServiceBroker();
  const runtime = new LocalAppRuntime(store, () => undefined, { environment: { PATH: process.env.PATH, HOME: root }, onRunEnded: (appId, runId) => broker.revoke(appId, runId) });
  t.after(async () => { await runtime.dispose(); await broker.dispose(); await core.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  await fs.mkdir(store.importsRoot, { recursive: true });
  for (const name of ['static', 'python', 'node']) {
    const source = join(store.importsRoot, name);
    await fs.cp(fileURLToPath(new URL(`../../../fixtures/local-apps/v2/${name}`, import.meta.url)), source, { recursive: true, filter: path => !path.split('/').includes('node_modules') });
    const row = await store.publish({ staged_path: source });
    if (name !== 'static') {
      await assert.rejects(runtime.start(row.id), /permission/);
      row.grants = row.manifest.capabilities ?? []; await store.save(row);
    }
    await runtime.start(row.id);
    const owner = runtime.owner(row.id)!; assert.match(owner.runId, /^a[0-9a-f]{32}$/); assert.equal(owner.releaseDigest, row.releaseDigest);
    const runs = await core.request<AppRun[]>({ op: 'runs', appId: row.id }); assert.equal(runs.find(run => run.runId === owner.runId)?.phase, 'running');
    let socketClosed: Promise<unknown> | undefined;
    if (name === 'node') {
      const { default: WebSocket } = await import(pathToFileURL(join(runtime.workspace(row.id)!, 'node_modules/ws/index.js')).href);
      const url = await broker.socketUrl(row.id, owner.runId, 'api', runtime.service(row.id, 'api')!.port, '/socket', `lingxi-app://${row.id}`);
      const socket = new WebSocket(url, { origin: `lingxi-app://${row.id}` });
      await new Promise<void>((resolve, reject) => { socket.once('open', resolve); socket.once('error', reject); });
      const echo = new Promise<Buffer>(resolve => socket.once('message', resolve)); socket.send(Buffer.from([0, 255, 7]));
      assert.deepEqual(await echo, Buffer.from([0, 255, 7])); socketClosed = new Promise(resolve => socket.once('close', resolve));
    }
    if (name === 'static') {
      assert.match(await fs.readFile(join(runtime.workspace(row.id)!, 'dist/index.html'), 'utf8'), /html/);
    } else {
      const service = runtime.service(row.id, 'api')!;
      assert.equal(await (await broker.fetch(row.id, owner.runId, 'api', service.port, '/health', new Request('http://fixture/health'))).text(), name === 'python' ? 'PYTHON-READY' : 'NODE-READY');
      const binary = new Uint8Array([0, 1, 255, 42]);
      const response = await broker.fetch(row.id, owner.runId, 'api', service.port, '/echo', new Request('http://fixture/echo', { method: 'POST', body: binary }));
      assert.deepEqual(new Uint8Array(await response.arrayBuffer()), binary);
      const events = await broker.fetch(row.id, owner.runId, 'api', service.port, '/events', new Request('http://fixture/events'));
      assert.match(await events.text(), /data: 2/);
    }
    await runtime.stop(row.id); if (socketClosed) await socketClosed;
    const stopped = await core.request<AppRun[]>({ op: 'runs', appId: row.id }); assert.equal(stopped.find(run => run.runId === owner.runId)?.phase, 'stopped');
    await runtime.start(row.id); assert.notEqual(runtime.owner(row.id)!.runId, owner.runId); await runtime.stop(row.id);
    if (name === 'static') {
      const dataPath = store.dataPath(row.id);
      await fs.writeFile(join(dataPath, 'user-note.txt'), 'keep across code versions');
      await fs.appendFile(join(source, 'dist/index.html'), '\n<!-- second release -->');
      const next = await store.publish({ id: row.id, staged_path: source, expectedRevision: row.revision });
      assert.equal(next.revision, 2); assert.equal(next.currentRevision, 1);
      await runtime.start(row.id);
      const secondOwner = runtime.owner(row.id)!; assert.equal(secondOwner.revision, 2);
      await assert.rejects(runtime.start(row.id, false, undefined, true, 1, 1), /current version changed/);
      assert.equal(runtime.owner(row.id)?.runId, secondOwner.runId);
      await runtime.start(row.id, false, undefined, true, 1, 2);
      assert.equal(runtime.owner(row.id)?.revision, 1);
      assert.equal((await store.get(row.id)).currentRevision, 1);
      assert.equal(await fs.readFile(join(dataPath, 'user-note.txt'), 'utf8'), 'keep across code versions');
      await runtime.stop(row.id);
    }
  }
});

test('Desktop retains the serving release on commit failure and reuses prepared environments until Repair', { skip: !binary, timeout: 30000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-core-commit-')), config = join(root, 'config');
  const core = new LocalAppCoreClient(binary!, config), store = new CoreLocalAppStore(join(config, 'local-apps/v2'), core);
  let failCleanupFor: string | undefined;
  const runtime = new LocalAppRuntime(store, () => undefined, { onRunEnded: (_id, runId) => {
    if (runId === failCleanupFor) throw new Error('Transient broker cleanup failure');
  } });
  t.after(async () => { await runtime.dispose(); await core.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  const source = join(store.importsRoot, 'source'); await fs.mkdir(source, { recursive: true });
  const node = shellQuote(process.execPath), platform = process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux';
  const manifest = { schema: 2, name: 'Commit and environment', entry: { type: 'server', process: 'api' }, targets: [{ platform, environment: 'native' }],
    install: [{ command: `${node} -e "require('fs').appendFileSync('install-count','x')"` }],
    processes: [{ name: 'api', command: `${node} server.cjs` }], package: { files: ['server.cjs'], dependencyLocks: [] } };
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify(manifest));
  await fs.writeFile(join(source, 'server.cjs'), "require('http').createServer((req,res)=>res.end('first')).listen(process.env.PORT,'127.0.0.1')");
  const row = await store.publish({ staged_path: source }); row.grants = ['process']; await store.save(row);
  await runtime.start(row.id);
  const originalWorkspace = runtime.workspace(row.id)!;
  await runtime.stop(row.id); await runtime.start(row.id);
  assert.equal(runtime.workspace(row.id), originalWorkspace);
  assert.equal(await fs.readFile(join(originalWorkspace, 'install-count'), 'utf8'), 'x');
  const oldRun = runtime.owner(row.id)!.runId, oldUrl = runtime.service(row.id, 'api')!.url;
  await fs.writeFile(join(source, 'server.cjs'), "require('http').createServer((req,res)=>res.end('second')).listen(process.env.PORT,'127.0.0.1')");
  const next = await store.publish({ id: row.id, expectedRevision: row.revision, staged_path: source }); next.grants = ['process']; await store.save(next);
  const activate = store.activate.bind(store);
  store.activate = async () => { throw new Error('Activation write failed'); };
  await assert.rejects(runtime.start(row.id, false, undefined, true), /Activation write failed/);
  assert.equal(runtime.owner(row.id)?.runId, oldRun);
  assert.equal(await (await fetch(oldUrl)).text(), 'first');
  assert.equal((await store.get(row.id)).currentRevision, 1);
  store.activate = activate;
  failCleanupFor = oldRun;
  await runtime.start(row.id, false, undefined, true);
  assert.equal(runtime.status(next).phase, 'running');
  assert.equal((await store.get(row.id)).currentRevision, 2);
  assert.equal(await (await fetch(runtime.service(row.id, 'api')!.url)).text(), 'second');
  await assert.rejects(fetch(oldUrl));
  const secondWorkspace = runtime.workspace(row.id)!;
  const currentUrl = runtime.service(row.id, 'api')!.url;
  await assert.rejects(runtime.stop(row.id), /cleanup needs retry/);
  await assert.rejects(fetch(currentUrl), 'a retired cleanup failure must not leave the current process running');
  failCleanupFor = undefined; await runtime.stop(row.id);
  await runtime.start(row.id, true);
  assert.notEqual(runtime.workspace(row.id), secondWorkspace);
  assert.equal(await fs.readFile(join(runtime.workspace(row.id)!, 'install-count'), 'utf8'), 'x');
  await runtime.stop(row.id);
});
