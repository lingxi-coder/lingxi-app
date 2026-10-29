import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalAppStore } from './fixtures/local-app-store.ts';
import { LocalAppRuntime, appEnvironment, shellQuote } from '../src/main/local-apps/runtime.ts';
import { parseManifest } from '../src/main/local-apps/manifest.ts';
import { permissionCapabilities } from '../src/main/local-apps/permissions.ts';
import { CancellationTombstones, createInvocationRef, InvocationRegistry } from '../src/main/local-apps/upstream/dyadInvocation.ts';

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-local-app-test-'));
  const store = new LocalAppStore(root);
  const runtime = new LocalAppRuntime(store, () => undefined);
  t.after(async () => { await runtime.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  return { root, store, runtime };
}
async function project(store: LocalAppStore, manifest: unknown, files: Record<string, string> = {}) {
  const source = await fs.mkdtemp(join(await fs.mkdir(join(store.root, 'imports'), { recursive: true }).then(() => join(store.root, 'imports')), '/project-'));
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify(manifest));
  for (const [name, value] of Object.entries(files)) { await fs.mkdir(dirname(join(source, name)), { recursive: true }); await fs.writeFile(join(source, name), value); }
  const row = await store.publish({ staged_path: source }); row.grants = row.manifest.capabilities ?? []; await store.save(row);
  return row;
}
const node = shellQuote(process.execPath);
test('camera-only checks do not require microphone access and match Electron media requests', () => {
  assert.deepEqual(permissionCapabilities('media', { mediaType: 'video' }), ['camera']);
  assert.deepEqual(permissionCapabilities('media', { mediaTypes: ['video'] }), ['camera']);
  assert.deepEqual(permissionCapabilities('media', { mediaType: 'audio' }), ['microphone']);
  assert.deepEqual(permissionCapabilities('media', { mediaTypes: ['video', 'audio'] }), ['camera', 'microphone']);
  assert.deepEqual(permissionCapabilities('geolocation', {}), []);
});
test('manifest supports service dependencies and rejects cycles, traversal and undeclared entry processes', () => {
  const value = { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Full app', entry: { type: 'server', process: 'front' }, processes: [
    { name: 'front', command: 'node front.js', dependsOn: ['api'] }, { name: 'api', command: 'python api.py' }] };
  const manifest = parseManifest(value);
  assert.deepEqual(manifest.processes?.map(p => p.name), ['api', 'front']);
  assert.deepEqual(manifest.capabilities, ['process']);
  assert.throws(() => parseManifest({ ...value, processes: [{ name: 'front', command: 'x', dependsOn: ['api'] }] }), /dependencies/);
  assert.throws(() => parseManifest({ ...value, entry: { type: 'static', path: '../secret' } }), /path|entry/);
  assert.throws(() => parseManifest({ ...value, processes: [{ name: 'api-v1', command: 'x' }, { name: 'api_v1', command: 'x' }] }), /unique/);
  assert.throws(() => parseManifest({ ...value, processes: [{ name: 'api', command: 'x' }, { name: 'API', command: 'x' }] }), /unique/);
  assert.equal(parseManifest({ ...value, install: [{ command: 'env', env: { CUDA_VISIBLE_DEVICES: '' } }] }).install?.[0]?.env?.CUDA_VISIBLE_DEVICES, '');
});
test('publishes immutable revisions, preserves grants and app data, rejects an invalid update atomically', async t => {
  const { store } = await fixture(t);
  const a = await store.publish({ name: 'Characters', html: '<!doctype html><title>Characters</title>' });
  await fs.mkdir(store.dataPath(a.id), { recursive: true }); await fs.writeFile(join(store.dataPath(a.id), 'progress'), '3');
  await assert.rejects(store.publish({ id: a.id, html: '' }), /empty/);
  assert.equal((await store.get(a.id)).revision, 1);
  const b = await store.publish({ id: a.id, html: '<!doctype html><title>Updated</title>' });
  assert.equal(b.revision, 2); assert.equal(await fs.readFile(join(store.dataPath(a.id), 'progress'), 'utf8'), '3');
  assert.match(await fs.readFile(join(store.revisionPath(a.id, 1), 'index.html'), 'utf8'), /Characters/);
});
test('bundle snapshots exclude credentials and reject source symlinks', async t => {
  const { store } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Files', entry: { type: 'static', path: 'index.html' } }, { 'index.html': '<!doctype html>', '.env': 'PRIVATE=secret' });
  await assert.rejects(fs.access(join(store.revisionPath(row.id, 1), '.env')));
  const source = join(store.root, 'imports', 'links'); await fs.mkdir(source); await fs.symlink('/etc/hosts', join(source, 'index.html'));
  await assert.rejects(store.publish({ staged_path: source }), /symlink/);
});
test('install runs once, a real local backend becomes ready, and stop releases its port', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Server', entry: { type: 'server', process: 'api' },
    install: [{ command: `${node} -e "require('fs').appendFileSync(process.env.LOCAL_APP_DATA_DIR+'/installs','x')"` }],
    processes: [{ name: 'api', command: `${node} server.cjs`, ready: { path: '/health', timeoutMs: 10000 } }] }, {
    'server.cjs': "require('http').createServer((req,res)=>res.end('ready')).listen(process.env.PORT,'127.0.0.1')",
  });
  await Promise.all([runtime.start(row.id), runtime.start(row.id)]);
  assert.equal(runtime.status(row).phase, 'running');
  const url = runtime.service(row.id, 'api')!.url;
  assert.equal(await (await fetch(url)).text(), 'ready');
  await runtime.stop(row.id); await assert.rejects(fetch(url));
  await runtime.start(row.id); assert.equal(await fs.readFile(join(runtime.workspace(row.id)!, '.lingxi-prepare-data', 'installs'), 'utf8'), 'x');
});
test('stop before queue admission cancels startup and does not publish late readiness', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Cancel', html: '<!doctype html>' });
  const start = runtime.start(row.id);
  const rejected = assert.rejects(start, /cancelled/);
  await runtime.stop(row.id); await rejected;
  assert.equal(runtime.status(row).phase, 'stopped');
});
test('explicit background invocation resume restarts services without rerunning install or changing owner', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Resume', entry: { type: 'server', process: 'api' },
    install: [{ command: `${node} -e "require('fs').appendFileSync(process.env.LOCAL_APP_DATA_DIR+'/installs','x')"` }],
    processes: [{ name: 'api', command: `${node} server.cjs`, ready: { path: '/health', timeoutMs: 5000 } }] }, {
    'server.cjs': "require('http').createServer((req,res)=>res.end('ready')).listen(process.env.PORT,'127.0.0.1')",
  });
  await runtime.start(row.id); const invocation = runtime.invocationId(row.id)!;
  const previousUrl = runtime.service(row.id, 'api')!.url;
  await runtime.stop(row.id); await assert.rejects(fetch(previousUrl));
  await runtime.resume(row.id, row.revision, invocation);
  assert.equal(runtime.invocationId(row.id), invocation);
  assert.equal(await (await fetch(runtime.service(row.id, 'api')!.url)).text(), 'ready');
  assert.equal(await fs.readFile(join(store.workspacePath(row.id, row.revision), '.lingxi-prepare-data', 'installs'), 'utf8'), 'x');
  await assert.rejects(fs.access(join(store.dataPath(row.id), 'installs')));
  await runtime.stop(row.id);
  await assert.rejects(runtime.resume(row.id, row.revision, 'forged'), /invocation changed/);
  await store.publish({ id: row.id, html: '<!doctype html>' });
  await assert.rejects(runtime.resume(row.id, row.revision, invocation), /invocation changed/);
});
test('stop cancels a queued background invocation resume before services restart', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Resume cancel', html: '<!doctype html>' });
  await runtime.start(row.id); const invocation = runtime.invocationId(row.id)!; await runtime.stop(row.id);
  let release!: () => void;
  const gate = runtime.exclusive(row.id, () => new Promise<void>(resolve => { release = resolve; }));
  await delay(0);
  const resumed = assert.rejects(runtime.resume(row.id, row.revision, invocation), /cancelled/);
  const stopped = runtime.stop(row.id); release();
  await Promise.all([gate, resumed, stopped]);
  assert.equal(runtime.phase(row.id), 'stopped');
});
test('a shared background resume survives one caller cancelling', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Shared resume', html: '<!doctype html>' });
  await runtime.start(row.id); const invocation = runtime.invocationId(row.id)!; await runtime.stop(row.id);
  let release!: () => void;
  const gate = runtime.exclusive(row.id, () => new Promise<void>(resolve => { release = resolve; }));
  await delay(0);
  const first = new AbortController(), second = new AbortController();
  const rejected = assert.rejects(runtime.resume(row.id, row.revision, invocation, first.signal));
  const other = runtime.resume(row.id, row.revision, invocation, second.signal);
  first.abort(); await rejected; release();
  await Promise.all([gate, other]);
  assert.equal(runtime.phase(row.id), 'running'); assert.equal(runtime.invocationId(row.id), invocation);
});
test('stop cancels every queued repair before any installation side effect', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Queued', entry: { type: 'static', path: 'index.html' }, install: [{ command: 'echo wrong > "$LOCAL_APP_DATA_DIR/installed"' }] }, { 'index.html': '<!doctype html>' });
  let release!: () => void;
  const blocked = runtime.exclusive(row.id, () => new Promise<void>(resolve => { release = resolve; }));
  await delay(0);
  const first = assert.rejects(runtime.start(row.id, true), /cancelled/);
  const second = assert.rejects(runtime.start(row.id, true), /cancelled/);
  const stopped = runtime.stop(row.id);
  release(); await Promise.all([blocked, first, second, stopped]);
  await assert.rejects(fs.access(join(store.dataPath(row.id), 'installed')));
});
test('caller cancellation interrupts only its owned startup and preserves an already running app', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Caller', entry: { type: 'static', path: 'index.html' }, install: [{ command: `${node} -e "console.log('begun');setInterval(()=>{},1000)"` }] }, { 'index.html': '<!doctype html>' });
  const controller = new AbortController();
  const stopped = assert.rejects(runtime.start(row.id, false, controller.signal));
  for (let i = 0; i < 100 && !runtime.logs(row.id).some(log => log.text === 'begun\n'); i++) await delay(20);
  controller.abort(); await stopped;
  assert.equal(runtime.phase(row.id), 'stopped');
  await store.publish({ id: row.id, html: '<!doctype html>' }); await runtime.start(row.id);
  const again = new AbortController(); await runtime.start(row.id, false, again.signal); again.abort();
  assert.equal(runtime.phase(row.id), 'running');
});
test('shared startup survives either waiter cancelling and stops when all waiters leave', async t => {
  for (const cancelledFirst of [true, false]) {
    const { store, runtime } = await fixture(t);
    const row = await store.publish({ name: 'Shared callers', html: '<!doctype html>' });
    const prepare = store.prepareWorkspace.bind(store);
    let release!: () => void, entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    store.prepareWorkspace = async app => { entered(); await gate; return prepare(app); };
    const first = new AbortController(), second = new AbortController();
    const a = runtime.start(row.id, false, first.signal);
    await waiting;
    const b = runtime.start(row.id, false, second.signal);
    const cancelled = assert.rejects(cancelledFirst ? a : b);
    (cancelledFirst ? first : second).abort();
    await cancelled; release();
    await (cancelledFirst ? b : a);
    assert.equal(runtime.phase(row.id), 'running');
  }
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'No callers', html: '<!doctype html>' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void, preparations = 0;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { if (++preparations === 1) { entered(); await gate; } return prepare(app); };
  const first = new AbortController(), second = new AbortController();
  const a = assert.rejects(runtime.start(row.id, false, first.signal));
  await waiting;
  const b = assert.rejects(runtime.start(row.id, false, second.signal));
  first.abort(); second.abort();
  // A new owner arriving during cleanup must start a fresh invocation.
  const replacement = runtime.start(row.id);
  release(); await Promise.all([a, b, replacement]);
  assert.equal(preparations, 2); assert.equal(runtime.phase(row.id), 'running');
});
test('the final cancelled caller observes a startup already committed for host reconciliation', async t => {
  const { store } = await fixture(t);
  const row = await store.publish({ name: 'Committed startup', html: '<!doctype html>' });
  const controller = new AbortController();
  const runtime = new LocalAppRuntime(store, () => {
    if (runtime.phase(row.id) === 'running') controller.abort();
  });
  t.after(() => runtime.dispose());
  await runtime.start(row.id, false, controller.signal);
  assert.equal(controller.signal.aborted, true);
  assert.equal(runtime.phase(row.id), 'running');
});
test('a new caller can cancel while an abandoned startup is still cleaning up', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Abandoned start', html: '<!doctype html>' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { entered(); await gate; return prepare(app); };
  const first = new AbortController(), second = new AbortController();
  const firstResult = assert.rejects(runtime.start(row.id, false, first.signal));
  await waiting; first.abort();
  const secondResult = assert.rejects(runtime.start(row.id, false, second.signal));
  second.abort();
  try {
    await Promise.race([secondResult, delay(300).then(() => { throw new Error('Cancelled caller waited for another startup cleanup'); })]);
  } finally { release(); await Promise.all([firstResult, secondResult]); }
});
test('an incompatible waiting start can cancel without interrupting the original', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Separate intents', html: '<!doctype html>' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { entered(); await gate; return prepare(app); };
  const first = runtime.start(row.id);
  await waiting;
  const controller = new AbortController();
  const second = assert.rejects(runtime.start(row.id, false, controller.signal, true));
  controller.abort();
  try {
    await Promise.race([second, delay(300).then(() => { throw new Error('Cancelled caller waited for another startup'); })]);
  } finally { release(); await Promise.all([first, second]); }
  assert.equal(runtime.phase(row.id), 'running');
});
test('stop cancels an incompatible start waiting outside the runtime queue', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Stop waiting start', html: '<!doctype html>' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void, preparations = 0;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { if (++preparations === 1) { entered(); await gate; } return prepare(app); };
  const first = assert.rejects(runtime.start(row.id));
  await waiting;
  const replacement = assert.rejects(runtime.start(row.id, false, undefined, true), /cancelled/i);
  const stopping = runtime.stop(row.id);
  release(); await Promise.all([first, replacement, stopping]);
  assert.equal(preparations, 1); assert.equal(runtime.phase(row.id), 'stopped');
  await runtime.start(row.id);
  assert.equal(preparations, 2); assert.equal(runtime.phase(row.id), 'running');
});
test('stop cancels a background resume waiting for an incompatible start', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Stop waiting resume', html: '<!doctype html>' });
  await runtime.start(row.id); const invocation = runtime.invocationId(row.id)!; await runtime.stop(row.id);
  let release!: () => void, entered!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  const prepare = store.prepareWorkspace.bind(store);
  store.prepareWorkspace = async app => { entered(); await gate; return prepare(app); };
  const first = assert.rejects(runtime.start(row.id, false, undefined, true));
  await waiting;
  const resume = assert.rejects(runtime.resume(row.id, row.revision, invocation), /cancelled/i);
  const stopping = runtime.stop(row.id);
  release(); await Promise.all([first, resume, stopping]);
  assert.equal(runtime.phase(row.id), 'stopped');
});
test('a new start after stop waits for cleanup and starts a fresh invocation', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Start after stop', html: '<!doctype html>' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void, preparations = 0;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { if (++preparations === 1) { entered(); await gate; } return prepare(app); };
  const first = assert.rejects(runtime.start(row.id)); await waiting;
  const stopping = runtime.stop(row.id);
  const next = runtime.start(row.id);
  release(); await Promise.all([first, stopping, next]);
  assert.equal(preparations, 2); assert.equal(runtime.phase(row.id), 'running');
});
test('stop cancels a start waiting on an existing running invocation', async t => {
  const { store, runtime } = await fixture(t);
  const row = await store.publish({ name: 'Stop existing start', html: '<!doctype html>' });
  await runtime.start(row.id);
  let release!: () => void, entered!: () => void;
  const enteredQueue = new Promise<void>(resolve => { entered = resolve; });
  const gate = runtime.exclusive(row.id, () => new Promise<void>(resolve => { release = resolve; entered(); }));
  await enteredQueue;
  const start = assert.rejects(runtime.start(row.id), /abort|cancel/i);
  const stop = runtime.stop(row.id);
  release(); await Promise.all([gate, start, stop]);
  assert.equal(runtime.phase(row.id), 'stopped');
});
for (const action of ['remove', 'dispose'] as const) {
  test(`${action} cancels an incompatible start waiting outside the runtime queue`, async t => {
    const { store, runtime } = await fixture(t);
    const row = await store.publish({ name: `Waiting ${action}`, html: '<!doctype html>' });
    const prepare = store.prepareWorkspace.bind(store);
    let release!: () => void, entered!: () => void, preparations = 0;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    store.prepareWorkspace = async app => { if (++preparations === 1) { entered(); await gate; } return prepare(app); };
    const first = assert.rejects(runtime.start(row.id)); await waiting;
    const replacement = assert.rejects(runtime.start(row.id, false, undefined, true), /cancelled/i);
    const ending = action === 'remove' ? runtime.remove(row.id) : runtime.dispose();
    await assert.rejects(runtime.start(row.id), /stopping|deleted/);
    release(); await Promise.all([first, replacement, ending]);
    assert.equal(preparations, 1);
  });
}
test('different startup intents retain replacement and expected revision checks', async t => {
  const { store, runtime } = await fixture(t);
  const original = await store.publish({ name: 'Versioned', html: '<!doctype html>one' });
  const prepare = store.prepareWorkspace.bind(store);
  let release!: () => void, entered!: () => void, blocked = true;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  store.prepareWorkspace = async app => { if (blocked) { blocked = false; entered(); await gate; } return prepare(app); };
  const first = runtime.start(original.id);
  await waiting;
  const next = await store.publish({ id: original.id, html: '<!doctype html>two' });
  const replacement = runtime.start(original.id, false, undefined, true, next.revision);
  const badVersion = assert.rejects(runtime.start(original.id, false, undefined, true, undefined, 999), /current version changed/);
  release(); await first; await replacement; await badVersion;
  assert.equal(runtime.revision(original.id), next.revision);
  await assert.rejects(runtime.start(original.id, false, undefined, false, undefined, 999), /current version changed/);
});
test('readiness survives oversized output and markers split between chunks', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Verbose', entry: { type: 'static', path: 'index.html' }, processes: [{ name: 'worker', command: `${node} -e "process.stdout.write('REA');setTimeout(()=>{process.stdout.write('DY'+'.'.repeat(20000));setInterval(()=>{},1000)},10)"`, ready: { output: 'READY', timeoutMs: 2000 } }] }, { 'index.html': '<!doctype html>' });
  await runtime.start(row.id); assert.equal(runtime.phase(row.id), 'running');
  assert.ok(runtime.logs(row.id).every(log => log.text.length <= 8192));
});
test('a dependency crash while another service starts remains a reported failure', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Crash', entry: { type: 'static', path: 'index.html' }, processes: [
    { name: 'api', command: `${node} -e "console.log('api-ready');setTimeout(()=>process.exit(7),300)"`, ready: { output: 'api-ready', timeoutMs: 2000 } },
    { name: 'worker', dependsOn: ['api'], command: `${node} -e "setInterval(()=>{},1000)"`, ready: { output: 'worker-ready', timeoutMs: 3000 } },
  ] }, { 'index.html': '<!doctype html>' });
  await assert.rejects(runtime.start(row.id), /api exited \(7\)/);
  assert.equal(runtime.phase(row.id), 'failed');
  assert.match(runtime.status(row).error!, /api exited \(7\)/);
  assert.ok(runtime.logs(row.id).some(log => log.text.includes('api exited (7)')));
});
test('stop interrupts install and new invocation owns later output', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Slow', entry: { type: 'static', path: 'index.html' },
    install: [{ command: `${node} -e "console.log('install-begun');setInterval(()=>{},1000)"` }] }, { 'index.html': '<!doctype html>' });
  const start = runtime.start(row.id); const rejected = assert.rejects(start);
  for (let n=0;n<100 && !runtime.logs(row.id).some(l=>l.text.includes('install-begun'));n++) await delay(20);
  await runtime.stop(row.id); await rejected;
  assert.equal(runtime.status(row).phase, 'stopped');
  await store.publish({ id: row.id, html: '<!doctype html><title>New</title>' });
  await runtime.start(row.id);
  const latest = runtime.logs(row.id).at(-1)!; assert.equal(latest.text, 'Application is ready');
});
test('failed install preserves logs and can be repaired without removing user data', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Repair', entry: { type: 'static', path: 'index.html' }, install: [{ command: 'echo install-error >&2; exit 7' }] }, { 'index.html': '<!doctype html>' });
  await assert.rejects(runtime.start(row.id), /code 7/);
  assert.equal(runtime.status(row).phase, 'failed'); assert.ok(runtime.logs(row.id).some(l=>l.text.includes('install-error')));
});
test('native capability is enforced before installation', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Permission', entry: { type: 'static', path: 'index.html' }, install: [{ command: 'echo should-not-run' }] }, { 'index.html': '<!doctype html>' });
  row.grants = []; await store.save(row);
  await assert.rejects(runtime.start(row.id), /permission/);
  assert.equal(runtime.logs(row.id).length, 0);
});
test('child environment does not inherit provider secrets or Node injection', () => {
  const env = appEnvironment({ PATH: '/bin', HOME: '/tmp', ANTHROPIC_API_KEY: 'secret', OPENAI_API_KEY: 'secret', NODE_OPTIONS: '--require evil', LINGXI_HOST_TOKEN: 'secret' });
  assert.deepEqual(env, { PATH: '/bin', HOME: '/tmp' });
});
test('upstream Dyad identities reject stale completions and retain cancel-before-registration', () => {
  const one = createInvocationRef('app', '1', { next: () => 'first' }), two = createInvocationRef('app', '1', { next: () => 'second' });
  const registry = new InvocationRegistry<string>(); registry.register(one, 'old'); registry.register(two, 'new');
  assert.equal(registry.claim(one).kind, 'stale'); assert.equal(registry.claim(two).kind, 'claimed');
  const canceled = new CancellationTombstones(2); canceled.add(one); assert.equal(canceled.has(one), true); assert.equal(canceled.has(two), false);
});

test('Python install creates an app-local venv and service dependencies receive ready backend URLs', async t => {
  const { store, runtime } = await fixture(t);
  const row = await project(store, { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Python', entry: { type: 'server', process: 'api' },
    install: [{ command: 'python -c "import sys; assert sys.prefix != sys.base_prefix"', venv: '.venv' }],
    processes: [{ name: 'api', command: 'python server.py', cwd: 'backend', venv: '.venv', ready: { timeoutMs: 15000 } }, { name: 'worker', command: `${node} -e "console.log(process.env.LOCAL_APP_API_URL); console.log('worker-ready');setInterval(()=>{},1000)"`, dependsOn: ['api'], ready: { output: 'worker-ready', timeoutMs: 5000 } }] }, {
    'backend/server.py': "import os,sys\nfrom http.server import BaseHTTPRequestHandler,HTTPServer\nclass H(BaseHTTPRequestHandler):\n def do_GET(self):\n  self.send_response(200); self.end_headers(); self.wfile.write(sys.prefix.encode())\nHTTPServer(('127.0.0.1',int(os.environ['PORT'])),H).serve_forever()\n",
  });
  await runtime.start(row.id);
  const url = runtime.service(row.id, 'api')!.url;
  assert.match(await (await fetch(url)).text(), /\.venv$/);
  assert.ok(runtime.logs(row.id).some(log => log.process === 'worker' && log.text.includes(url)));
  await runtime.stop(row.id); await assert.rejects(fetch(url));
});

test('failed replacement preparation preserves the running release and invokes only candidate cleanup', async t => {
  const { store, runtime } = await fixture(t);
  const old = await store.publish({ name: 'Stable', html: '<!doctype html>stable' });
  await runtime.start(old.id);
  const invocation = runtime.invocationId(old.id);
  const source = join(store.root, 'imports', 'bad-update'); await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, name: 'Bad', targets: [{ platform: 'macos', environment: 'native' }], entry: { type: 'static', path: 'index.html' }, install: [{ command: 'echo contaminated > "$LOCAL_APP_DATA_DIR/data"; exit 17' }] }));
  await fs.writeFile(join(source, 'index.html'), 'bad');
  const updated = await store.publish({ id: old.id, staged_path: source }); updated.grants = ['process']; await store.save(updated);
  await assert.rejects(runtime.start(old.id, false, undefined, true), /17/);
  assert.equal(runtime.invocationId(old.id), invocation);
  assert.equal(runtime.phase(old.id), 'running');
  assert.equal(runtime.revision(old.id), 1);
  await assert.rejects(fs.access(join(store.dataPath(old.id), 'data')));
});
