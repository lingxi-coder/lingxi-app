import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppCoreClient, type LocalAppCoreTransport } from '../src/main/local-apps/coreClient.ts';
import { CoreLocalAppStore } from '../src/main/local-apps/coreStore.ts';
import { desktopFixtureTargets } from './fixtures/local-app-desktop-targets.ts';
import type { DesktopLocalApp } from '../src/shared/localApps.ts';

const id = 'a0123456789abcdef0123456789abcdef';
const manifest = { schema: 2, name: 'App', entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'web' }], capabilities: [] };

test('Electron fixture declares its host platform and incompatible targets are rejected', async () => {
  const originalPlatform = Object.getOwnPropertyDescriptor(process, 'platform')!;
  try {
    for (const [nodePlatform, hostPlatform] of [['darwin', 'macos'], ['win32', 'windows'], ['linux', 'linux']] as const) {
      Object.defineProperty(process, 'platform', { ...originalPlatform, value: nodePlatform });
      const targets = desktopFixtureTargets(nodePlatform);
      assert.deepEqual(targets.native, [{ platform: hostPlatform, environment: 'native' }]);
      const requests: { platform: string; environment: string }[] = [];
      let declared: { platform: string; environment: string }[] = targets.native;
      const transport: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
        assert.equal(request.op, 'checkTarget');
        const target = request.target as { platform: string; environment: string };
        requests.push(target);
        return { compatible: declared.some(entry => entry.platform === target.platform && entry.environment === target.environment) } as T;
      } };
      const store = new CoreLocalAppStore('/unused', transport);
      const row = { id, revision: 1 } as DesktopLocalApp;
      await store.assertTarget(row);
      declared = targets.incompatible;
      await assert.rejects(store.assertTarget(row), /does not support this Desktop runtime/);
      assert.deepEqual(requests.map(({ platform, environment }) => ({ platform, environment })), [
        { platform: hostPlatform, environment: 'native' },
        { platform: hostPlatform, environment: 'native' },
        { platform: hostPlatform, environment: 'web' },
      ]);
    }
  } finally {
    Object.defineProperty(process, 'platform', originalPlatform);
  }
});

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
  first.agent = { enabled: true, model: {providerId:'configured',modelId:'vision'}, tools:[], skills:[], background:false,maxTurns:20,timeoutMs:600000 };
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
