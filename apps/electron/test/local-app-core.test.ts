import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { LocalAppCoreClient, type LocalAppCoreTransport } from '../src/main/local-apps/coreClient.ts';
import { CoreLocalAppStore } from '../src/main/local-apps/coreStore.ts';
import { PublicationIntents } from '../src/main/local-apps/publicationIntents.ts';

const id = 'a0123456789abcdef0123456789abcdef';
const manifest = { schema: 2, name: 'App', entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'web' }], capabilities: [] };

test('Desktop adapter uses canonical digest consent and rejects stale authoring publication', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-core-test-')); t.after(() => fs.rm(root, { recursive: true, force: true }));
  const calls: Record<string, unknown>[] = [];
  let digest = 'first'; const executionGrants: string[] = [];
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
    calls.push(request);
    if (request.op === 'get') return { appId: id, currentRevision: 1, latestRevision: 2, name: 'App', summary: '', executionGrants, createdAtMs: 1, updatedAtMs: 2,
      authoring: { workspaceId: 'workspace-1', sourceDirectory: '/source/fixed', projectRef: { kind: 'desktopPath', path: '/source/fixed' }, authoringSessionId: 'session', appId: id } } as T;
    if (request.op === 'getRelease') return { appId: id, revision: 2, digest, manifest: { ...manifest, install: [{ command: 'npm ci' }] } } as T;
    if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
    if (request.op === 'grant') { executionGrants.push(String(request.digest)); return {} as T; }
    throw new Error('Unexpected request ' + request.op);
  } };
  const store = new CoreLocalAppStore(root, transport);
  const row = await store.get(id);
  assert.deepEqual(row.authoring, { workspaceId: 'workspace-1', sourceDirectory: '/source/fixed',
    projectRef: { kind: 'desktopPath', path: '/source/fixed' }, authoringSessionId: 'session', appId: id });
  assert.ok(row.manifest.capabilities?.includes('process'));
  row.grants.push('process'); await store.save(row);
  assert.deepEqual(executionGrants, ['first']);
  assert.equal(calls.filter(r => r.op === 'grant').length, 1);
  digest = 'changed'; await assert.rejects(store.save(row), /release changed/);
  await assert.rejects(store.publish({ id, html: 'new' }), /expectedRevision/);
  assert.ok(!calls.some(r => r.op === 'publish'));
});

test('Desktop publishes with the captured native authoring receipt over the private core channel', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-core-authoring-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const sourceId = '11111111-2222-4333-8444-555555555555';
  const authoring = { workspaceId: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', sourceDirectory: '/source/app',
    projectRef: { kind: 'desktopPath' as const, path: '/source/app' },
    authoringSessionId: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff', appId: id };
  let captured: Record<string, any> | undefined;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'publishAuthoring') {
      captured = request;
      return { record: { appId: id, currentRevision: 1, latestRevision: 2, name: 'App', summary: '', executionGrants: [],
        createdAtMs: 1, updatedAtMs: 2, authoring }, release: { appId: id, revision: 2, digest: 'new', manifest } } as T;
    }
    if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport);
  const requestId = randomUUID();
  await store.publish({ id, name: 'App', html: '<!doctype html>', expectedRevision: 1,
    publication: { requestId, sourceDirectory: authoring.sourceDirectory, authoringSessionId: authoring.authoringSessionId },
    nativeAuthoring: { sourceId, authoring, sessionMode: 'code', ownerAppId: id, setCurrent: false } });
  assert.equal(captured?.sourceId, sourceId);
  assert.equal(captured?.sourceDirectory, authoring.sourceDirectory);
  assert.deepEqual(captured?.projectRef, authoring.projectRef);
  assert.equal(captured?.authoringSessionId, authoring.authoringSessionId);
  assert.equal(captured?.sessionMode, 'code');
  assert.equal(captured?.setCurrent, false);
  assert.equal(captured?.requestId, requestId);
  assert.equal(captured?.stagedDigest, 'd'.repeat(64));
  assert.equal(captured?.request.appId, id);
  assert.equal(captured?.request.expectedRevision, 1);
  assert.equal('authoring' in captured!.request, false);
  await assert.rejects(store.publish({ id, name: 'App', html: '<!doctype html>', expectedRevision: 1, authoring }), /requires a native authoring receipt/);
});

test('unknown native publish retains its host-owned staging for recovery', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-unknown-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const authoring = { workspaceId: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', sourceDirectory: '/source/app',
    projectRef: { kind: 'desktopPath' as const, path: '/source/app' },
    authoringSessionId: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff', appId: id };
  const paths: string[] = [];
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'publishAuthoring') {
      paths.push(request.request.sourcePath);
      throw Object.assign(new Error('SDK exited with unknown outcome'), { code: 'outcomeUnknown' });
    }
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport);
  const nativeAuthoring = { sourceId: '11111111-2222-4333-8444-555555555555', authoring, sessionMode: 'code' as const, ownerAppId: id, setCurrent: false };
  const publication = () => ({ requestId: randomUUID(), sourceDirectory: authoring.sourceDirectory, authoringSessionId: authoring.authoringSessionId });
  const first = publication();
  await assert.rejects(store.publish({ id, name: 'App', html: '<!doctype html>held', expectedRevision: 1, nativeAuthoring, publication: first }), /unknown outcome/);
  assert.equal(await fs.readFile(join(paths[0]!, 'index.html'), 'utf8'), '<!doctype html>held');
  const staged = join(store.importsRoot, 'tool-staging'); await fs.mkdir(staged, { recursive: true });
  await fs.writeFile(join(staged, 'index.html'), 'tool source');
  await assert.rejects(store.publish({ id, staged_path: staged, expectedRevision: 1, nativeAuthoring, publication: publication() }), /unknown outcome/);
  await assert.rejects(fs.stat(staged), /ENOENT/);
  assert.equal(await fs.readFile(join(paths[1]!, 'index.html'), 'utf8'), 'tool source');
  assert.deepEqual((await new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport).pendingPublications()).map(entry => entry.requestId).sort(),
    (await store.pendingPublications()).map(entry => entry.requestId).sort());
});

test('a completed publication whose SDK receipt disappeared cannot become a new mutation', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-lost-receipt-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID(); let writes = 0;
  const result = { record: { appId: id, currentRevision: 1, latestRevision: 1, name: 'App', summary: '', executionGrants: [], createdAtMs: 1, updatedAtMs: 2 },
    release: { appId: id, revision: 1, digest: 'd'.repeat(64), manifest } };
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'getPublication') return null as T;
    if (request.op === 'publishOnce') { writes++; return result as T; }
    if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport);
  const input = { name: 'App', html: '<p>Original</p>', publication: { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId } };
  await store.publish(input);
  await assert.rejects(store.publish(input), /committed Local App publication receipt is no longer available/);
  assert.equal(writes, 1, 'missing receipt never causes a second publish');
});

test('a pending publication rejects changed bytes or revision before any SDK mutation', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-pending-conflict-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID(), config = join(root, 'config', 'local-apps', 'v2');
  let writes = 0;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    if (request.op === 'getPublication') return null as T;
    if (request.op === 'fingerprintAuthoringPublication') {
      const bytes = await fs.readFile(join(request.sourcePath, 'index.html'));
      return { stagedDigest: createHash('sha256').update(bytes).digest('hex') } as T;
    }
    if (request.op === 'publishOnce') writes++;
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(config, transport); await fs.mkdir(store.importsRoot, { recursive: true });
  const imports = await fs.realpath(store.importsRoot), original = join(imports, 'old-input'), fixed = join(imports, `host-publish-${requestId}`);
  await fs.mkdir(original); await fs.writeFile(join(original, 'index.html'), 'original');
  const stagedDigest = createHash('sha256').update('original').digest('hex');
  const intents = new PublicationIntents(config, imports);
  await intents.capture({ requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId,
    appId: id, expectedRevision: 1, stagedDigest, sourcePath: fixed, stagingInputPath: original });
  await fs.rename(original, fixed); await intents.ready(requestId);
  const retry = join(imports, 'retry'); await fs.mkdir(retry); await fs.writeFile(join(retry, 'index.html'), 'changed');
  const publication = { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId };
  await assert.rejects(store.publish({ id, expectedRevision: 1, staged_path: retry, publication }), /conflicts/);
  await fs.writeFile(join(retry, 'index.html'), 'original');
  await assert.rejects(store.publish({ id, expectedRevision: 2, staged_path: retry, publication }), /conflicts/);
  assert.equal(writes, 0);
  assert.equal(await fs.readFile(join(fixed, 'index.html'), 'utf8'), 'original');
});

test('a committed publication recovers by its saved request and digest even after staging disappears', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-recover-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const sourceId = randomUUID(), sessionId = randomUUID(), requestId = randomUUID();
  const authoring = { workspaceId: randomUUID(), sourceDirectory: '/source/app', projectRef: { kind: 'desktopPath' as const, path: '/source/app' }, authoringSessionId: sessionId, appId: id };
  const result = { record: { appId: id, currentRevision: 1, latestRevision: 2, name: 'App', summary: '', executionGrants: [], createdAtMs: 1, updatedAtMs: 2, authoring },
    release: { appId: id, revision: 2, digest: 'd'.repeat(64), manifest } };
  const calls: Record<string, any>[] = [];
  let first = true;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    calls.push(request);
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'getPublication') return result as T;
    if (request.op === 'publishAuthoring') {
      if (first) { first = false; throw Object.assign(new Error('connection lost after commit'), { code: 'outcomeUnknown' }); }
      return result as T;
    }
    if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const config = join(root, 'config', 'local-apps', 'v2');
  const firstStore = new CoreLocalAppStore(config, transport);
  await assert.rejects(firstStore.publish({ id, name: 'App', html: '<!doctype html>held', expectedRevision: 1,
    publication: { requestId, sourceDirectory: authoring.sourceDirectory, authoringSessionId: sessionId },
    nativeAuthoring: { sourceId, authoring, sessionMode: 'code', ownerAppId: id, setCurrent: false } }), /connection lost/);
  const before = calls.find(request => request.op === 'publishAuthoring')!;
  await fs.rm(dirname(before.request.sourcePath), { recursive: true });
  const recovered = new CoreLocalAppStore(config, transport);
  assert.equal((await recovered.pendingPublications())[0]?.requestId, requestId);
  assert.equal((await recovered.resumePublication(requestId)).id, id);
  const replay = calls.filter(request => request.op === 'publishAuthoring')[1]!;
  assert.equal(replay.replayOnly, true);
  const { replayOnly: _replayOnly, ...originalPayload } = replay;
  assert.deepEqual(originalPayload, before);
  assert.equal(calls.filter(request => request.op === 'getPublication').length, 1);
  assert.deepEqual(await recovered.pendingPublications(), []);
});

test('generic Code publication uses a durable request and rejects a changed payload under the same id', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-generic-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID(), calls: Record<string, any>[] = [];
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    calls.push(request);
    if (request.op === 'fingerprintAuthoringPublication') {
      const bytes = await fs.readFile(join(request.sourcePath, 'index.html'));
      return { stagedDigest: createHash('sha256').update(bytes).digest('hex') } as T;
    }
    if (request.op === 'getPublication') return null as T;
    if (request.op === 'publishOnce') throw Object.assign(new Error('process exited'), { code: 'outcomeUnknown' });
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport);
  const publication = { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId };
  await assert.rejects(store.publish({ name: 'App', html: '<p>one</p>', publication }), /process exited/);
  await assert.rejects(store.publish({ name: 'App', html: '<p>different</p>', publication }), /conflicts/);
  const [sent] = calls.filter(request => request.op === 'publishOnce');
  assert.equal(sent.requestId, requestId);
  assert.equal(sent.sourceDirectory, publication.sourceDirectory);
  assert.equal(calls.filter(request => request.op === 'publishOnce').length, 1);
});

test('an uncommitted publication with lost or changed staging stays pending for review', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-missing-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID(), calls: Record<string, any>[] = [];
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    calls.push(request);
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'getPublication') return null as T;
    if (request.op === 'publishOnce') {
      if (calls.filter(row => row.op === 'publishOnce').length === 1) throw Object.assign(new Error('process exited'), { code: 'outcomeUnknown' });
      const file = await fs.readFile(join(request.request.sourcePath, 'index.html'), 'utf8');
      if (file !== '<p>original</p>') throw new Error('staged content does not match digest');
    }
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const config = join(root, 'config', 'local-apps', 'v2');
  const original = new CoreLocalAppStore(config, transport);
  await assert.rejects(original.publish({ name: 'App', html: '<p>original</p>',
    publication: { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId } }), /process exited/);
  const staged = calls.find(request => request.op === 'publishOnce')!.request.sourcePath;
  await fs.writeFile(join(staged, 'index.html'), '<p>changed</p>');
  const reopened = new CoreLocalAppStore(config, transport);
  await assert.rejects(reopened.resumePublication(requestId), /does not match digest/);
  assert.equal((await reopened.pendingPublications())[0]?.requestId, requestId);
  await fs.rm(staged, { recursive: true });
  await assert.rejects(reopened.resumePublication(requestId), /ENOENT/);
  assert.equal((await reopened.pendingPublications())[0]?.requestId, requestId);
  const attempts = calls.filter(request => request.op === 'publishOnce');
  assert.ok(attempts.every(request => request.requestId === requestId && request.stagedDigest === 'd'.repeat(64)));
});

test('a disappeared SDK receipt cannot turn a historical recovery into a new publish', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-replay-only-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID();
  const calls: Record<string, any>[] = [];
  let attempt = 0;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    calls.push(request);
    if (request.op === 'fingerprintAuthoringPublication') return { stagedDigest: 'd'.repeat(64) } as T;
    if (request.op === 'getPublication') return { record: { appId: id } } as T;
    if (request.op === 'publishOnce') {
      if (attempt++ === 0) throw Object.assign(new Error('process exited'), { code: 'outcomeUnknown' });
      assert.equal(request.replayOnly, true);
      throw new Error('committed receipt no longer exists');
    }
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(join(root, 'config', 'local-apps', 'v2'), transport);
  await assert.rejects(store.publish({ name: 'App', html: '<p>original</p>',
    publication: { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId } }), /process exited/);
  await assert.rejects(store.resumePublication(requestId), /receipt no longer exists/);
  assert.equal((await store.pendingPublications())[0]?.requestId, requestId);
  assert.equal(calls.filter(request => request.op === 'publishOnce').length, 2);
});

for (const moved of [false, true]) {
  test(`a preparing publication resumes the original bytes ${moved ? 'after' : 'before'} the host staging rename`, async t => {
    const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-preparing-'));
    t.after(() => fs.rm(root, { recursive: true, force: true }));
    const requestId = randomUUID(), sessionId = randomUUID(), config = join(root, 'config', 'local-apps', 'v2');
    let sent = 0;
    const result = { record: { appId: id, currentRevision: 1, latestRevision: 1, name: 'App', summary: '', executionGrants: [], createdAtMs: 1, updatedAtMs: 2 },
      release: { appId: id, revision: 1, digest: 'd'.repeat(64), manifest } };
    const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
      if (request.op === 'getPublication') return null as T;
      if (request.op === 'fingerprintAuthoringPublication') {
        assert.equal(await fs.readFile(join(request.sourcePath, 'index.html'), 'utf8'), '<p>captured</p>');
        return { stagedDigest: 'd'.repeat(64) } as T;
      }
      if (request.op === 'publishOnce') {
        sent++;
        assert.equal(request.requestId, requestId);
        assert.equal(request.stagedDigest, 'd'.repeat(64));
        assert.equal(await fs.readFile(join(request.request.sourcePath, 'index.html'), 'utf8'), '<p>captured</p>');
        return result as T;
      }
      if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
      throw new Error(`Unexpected request ${request.op}`);
    } };
    const store = new CoreLocalAppStore(config, transport);
    await fs.mkdir(store.importsRoot, { recursive: true });
    const imports = await fs.realpath(store.importsRoot), incoming = join(imports, 'original-tool-stage');
    const sourcePath = join(imports, `host-publish-${requestId}`);
    await fs.mkdir(incoming); await fs.writeFile(join(incoming, 'index.html'), '<p>captured</p>');
    const intents = new PublicationIntents(config, imports);
    await intents.capture({ requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId,
      expectedRevision: 0, stagedDigest: 'd'.repeat(64), sourcePath, stagingInputPath: incoming });
    if (moved) await fs.rename(incoming, sourcePath);
    const reopened = new CoreLocalAppStore(config, transport);
    assert.equal((await reopened.pendingPublications())[0]?.requestId, requestId);
    await reopened.resumePublication(requestId);
    assert.equal(sent, 1);
    assert.deepEqual(await reopened.pendingPublications(), []);
  });
}

test('a preparing publication with missing original input remains visible and is not resampled', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-prepare-missing-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const requestId = randomUUID(), sessionId = randomUUID(), config = join(root, 'config', 'local-apps', 'v2');
  let publishes = 0;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
    if (request.op === 'getPublication') return null as T;
    if (request.op === 'fingerprintAuthoringPublication') { await fs.stat(request.sourcePath); return { stagedDigest: 'd'.repeat(64) } as T; }
    if (request.op === 'publishOnce') publishes++;
    throw new Error(`Unexpected request ${request.op}`);
  } };
  const store = new CoreLocalAppStore(config, transport);
  await fs.mkdir(store.importsRoot, { recursive: true });
  const imports = await fs.realpath(store.importsRoot), incoming = join(imports, 'missing-tool-stage');
  const intents = new PublicationIntents(config, imports);
  await intents.capture({ requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId,
    expectedRevision: 0, stagedDigest: 'd'.repeat(64), sourcePath: join(imports, `host-publish-${requestId}`), stagingInputPath: incoming });
  await assert.rejects(store.resumePublication(requestId), /ENOENT/);
  assert.equal((await store.pendingPublications())[0]?.requestId, requestId);
  assert.equal(publishes, 0);
});

for (const takeoverFails of [false, true]) {
  test(`preparing intent fsync failure preserves or safely repairs staging when takeover ${takeoverFails ? 'also fails' : 'succeeds'}`, async t => {
    const root = await fs.mkdtemp(join(tmpdir(), 'local-app-publish-fsync-'));
    t.after(() => fs.rm(root, { recursive: true, force: true }));
    const requestId = randomUUID(), sessionId = randomUUID(), config = join(root, 'config', 'local-apps', 'v2');
    const result = { record: { appId: id, currentRevision: 1, latestRevision: 1, name: 'App', summary: '', executionGrants: [], createdAtMs: 1, updatedAtMs: 2 },
      release: { appId: id, revision: 1, digest: 'd'.repeat(64), manifest } };
    let sent = 0;
    const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, any>) => {
      if (request.op === 'fingerprintAuthoringPublication') {
        const bytes = await fs.readFile(join(request.sourcePath, 'index.html'));
        return { stagedDigest: createHash('sha256').update(bytes).digest('hex') } as T;
      }
      if (request.op === 'getPublication') return null as T;
      if (request.op === 'publishOnce') { sent++; assert.equal(request.requestId, requestId); return result as T; }
      if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
      throw new Error(`Unexpected request ${request.op}`);
    } };
    const store = new CoreLocalAppStore(config, transport);
    await fs.mkdir(store.importsRoot, { recursive: true });
    const imports = await fs.realpath(store.importsRoot), inputPath = join(imports, 'sdk-original');
    const fixedPath = join(imports, `host-publish-${requestId}`), intentDirectory = join(config, 'publication-intents');
    await fs.mkdir(inputPath); await fs.writeFile(join(inputPath, 'index.html'), '<p>captured</p>');
    const publication = { requestId, sourceDirectory: '/ordinary/project', authoringSessionId: sessionId };
    const originalOpen = fs.open, originalRename = fs.rename;
    let failedSync = false;
    (fs as any).open = async (...args: any[]) => {
      if (!failedSync && args[0] === intentDirectory && args[1] === 'r') {
        failedSync = true; throw Object.assign(new Error('simulated intent directory fsync failure'), { code: 'EIO' });
      }
      return (originalOpen as any)(...args);
    };
    (fs as any).rename = async (...args: any[]) => {
      if (takeoverFails && args[0] === inputPath && args[1] === fixedPath) throw Object.assign(new Error('simulated takeover failure'), { code: 'EIO' });
      return (originalRename as any)(...args);
    };
    try { await assert.rejects(store.publish({ name: 'App', staged_path: inputPath, publication }), /simulated intent directory fsync failure/); }
    finally { (fs as any).open = originalOpen; (fs as any).rename = originalRename; }
    assert.equal(failedSync, true);
    assert.equal(sent, 0, 'the SDK mutation was not admitted');
    assert.equal((await store.pendingPublications())[0]?.requestId, requestId);
    if (takeoverFails) {
      await fs.rm(inputPath, { recursive: true });
      const retained = join(imports, 'sdk-retained-identical');
      await fs.mkdir(retained); await fs.writeFile(join(retained, 'index.html'), '<p>changed</p>');
      await assert.rejects(store.publish({ name: 'App', staged_path: retained, publication }), /conflicts/);
      assert.equal(sent, 0);
      await fs.writeFile(join(retained, 'index.html'), '<p>captured</p>');
      await store.publish({ name: 'App', staged_path: retained, publication });
    } else {
      assert.equal(await fs.readFile(join(fixedPath, 'index.html'), 'utf8'), '<p>captured</p>');
      await fs.rm(inputPath, { recursive: true, force: true });
      await store.resumePublication(requestId);
    }
    assert.equal(sent, 1);
    assert.deepEqual(await store.pendingPublications(), []);
  });
}

test('standalone library client rejects pending work on EOF and requires explicit host recovery', { skip: process.platform === 'win32' }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-client-test-')); t.after(() => fs.rm(root, { recursive: true, force: true }));
  const source = join(root, 'server.mjs'), executable = join(root, 'bridge');
  await fs.writeFile(source, `import {createInterface} from 'node:readline';\ncreateInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line); if(r.request.op==='exit')process.exit(0);else process.stdout.write(JSON.stringify({id:r.id,ok:true,value:{op:r.request.op}})+'\\n');});`);
  await fs.writeFile(executable, `#!/bin/sh\nexec '${process.execPath}' '${source}' "$@"\n`, { mode: 0o700 });
  const client = new LocalAppCoreClient(executable, root); t.after(() => client.dispose());
  assert.deepEqual(await client.request({ op: 'list' }), { op: 'list' });
  await assert.rejects(client.request({ op: 'exit' }), /exited/);
  assert.throws(() => client.request({ op: 'list' }), /restart/);
  const recovered = new LocalAppCoreClient(executable, root); t.after(() => recovered.dispose());
  assert.deepEqual(await recovered.request({ op: 'list' }), { op: 'list' });
  await client.dispose(); assert.throws(() => client.request({ op: 'list' }), /closed/);
});

test('an admitted native mutation waits past the read timeout for its response or actual process exit', { skip: process.platform === 'win32' }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-drain-test-')); t.after(() => fs.rm(root, { recursive: true, force: true }));
  const source = join(root, 'server.mjs'), executable = join(root, 'bridge');
  await fs.writeFile(source, `import {createInterface} from 'node:readline';\ncreateInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);if(r.request.op==='exit')process.exit(0);else if(r.request.op!=='hang')setTimeout(()=>process.stdout.write(JSON.stringify({id:r.id,ok:true,value:{op:r.request.op}})+'\\n'),80);});`);
  await fs.writeFile(executable, `#!/bin/sh\nexec '${process.execPath}' '${source}' "$@"\n`, { mode: 0o700 });
  const client = new LocalAppCoreClient(executable, root, () => undefined, 20); t.after(() => client.dispose());
  await assert.rejects(client.request({ op: 'slow-read' }), /timed out/);
  assert.deepEqual(await client.request({ op: 'publishAuthoring' }, { waitForCompletion: true }), { op: 'publishAuthoring' });
  let settled = false;
  const pending = client.request({ op: 'hang' }, { waitForCompletion: true }).then(
    () => { settled = true; return null; }, error => { settled = true; return error; },
  );
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(settled, false, 'the host must retain the mutation after the ordinary request deadline');
  const exit = client.request({ op: 'exit' }, { waitForCompletion: true }).catch(error => error);
  assert.match(String(await pending), /exited/);
  assert.match(String(await exit), /exited/);
});

test('device consent belongs to the exact release and historical consent does not authorize new code', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-consent-test-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const executionGrants = ['digest-1'];
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
    if (request.op === 'get') return { appId: id, currentRevision: 1, latestRevision: 2, name: 'App', summary: '', executionGrants, createdAtMs: 1, updatedAtMs: 2 } as T;
    if (request.op === 'getRelease') return { appId: id, revision: request.revision, digest: `digest-${request.revision}`, manifest: { ...manifest, capabilities: ['process', 'clipboard'] } } as T;
    if (request.op === 'layout') return { sourcePath: `/release/${request.revision}`, dataPath: '/data' } as T;
    if (request.op === 'revoke') { executionGrants.length = 0; return {} as T; }
    throw new Error('Unexpected request ' + request.op);
  } };
  const store = new CoreLocalAppStore(root, transport);
  const previous = await store.getRevision(id, 1);
  previous.grants = ['process', 'clipboard']; await store.save(previous);
  assert.deepEqual((await store.getRevision(id, 1)).grants, ['clipboard', 'process']);
  assert.deepEqual((await store.get(id)).grants, []);
  // Removing Linux/process permission must work even while retaining clipboard.
  previous.grants = ['clipboard']; await store.save(previous);
  assert.deepEqual(executionGrants, []);
  assert.deepEqual((await store.getRevision(id, 1)).grants, ['clipboard']);
  await store.revoke(id);
  assert.deepEqual((await store.getRevision(id, 1)).grants, []);
});

test('Agent preferences survive updates but grants remain bound to exact release digests', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-core-agent-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  let latest = 1;
  const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
    if (request.op === 'get') return { appId: id, currentRevision: 1, latestRevision: latest, name: 'App', summary: '', executionGrants: [], createdAtMs: 1, updatedAtMs: 2 } as T;
    if (request.op === 'getRelease') return { appId: id, revision: request.revision, digest: `digest-${request.revision}`, manifest: { ...manifest, capabilities: ['agent'], agent: {tools: []} } } as T;
    if (request.op === 'layout') return { sourcePath: '/release/source', dataPath: '/data' } as T;
    if (request.op === 'revoke') return {} as T;
    throw new Error('Unexpected request ' + request.op);
  } };
  const store = new CoreLocalAppStore(root, transport);
  const first = await store.get(id);
  first.agent = { enabled: true, model: {providerId:'configured',modelId:'vision'}, tools:[], background:false,maxTurns:20,timeoutMs:600000 };
  first.grants = ['agent']; await store.save(first);
  latest = 2;
  const second = await store.get(id);
  assert.deepEqual(second.agent, first.agent, 'native model choices are retained');
  assert.deepEqual(second.grants, [], 'a new code digest receives no existing capability grants');
  assert.deepEqual((await store.getRevision(id, 1)).grants, ['agent']);
  await store.revoke(id);
  const revoked = await store.getRevision(id, 1);
  assert.deepEqual(revoked.grants, []);
  assert.equal(revoked.agent?.enabled, false);
  assert.deepEqual(revoked.agent?.tools, []);
});
