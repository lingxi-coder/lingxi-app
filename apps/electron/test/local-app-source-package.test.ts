import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { packSource, unpackSource, sha256, canonical } from '../src/main/local-apps/sourcePackage.ts';
import { LocalAppMarketClient, desktopTargets } from '../src/main/local-apps/marketClient.ts';
import { CoreLocalAppStore, type AppRelease } from '../src/main/local-apps/coreStore.ts';
import type { DesktopAppManifest, DesktopLocalApp } from '../src/shared/localApps.ts';
import type { LocalAppCoreTransport } from '../src/main/local-apps/coreClient.ts';

async function packageFixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-zip-test-')); t.after(() => fs.rm(root, { recursive: true, force: true }));
  const source = join(root, 'source'); await fs.mkdir(source);
  const manifest = { schema: 2, name: 'Portable', summary: '', entry: { type: 'static', path: 'index.html' }, targets: [{ platform: desktopTargets()[0]!.platform, environment: 'web' }], package: { files: ['index.html', 'run.sh'], dependencyLocks: [] } };
  const data = { 'lingxi-app.json': JSON.stringify(manifest), 'index.html': '<!doctype html><title>App</title>', 'run.sh': '#!/bin/sh\nprintf ok\n' };
  const inventory = Object.entries(data).map(([path, value]) => ({ path, executable: path.endsWith('.sh'), sizeBytes: Buffer.byteLength(value), sha256: sha256(Buffer.from(value)) })).sort((a, b) => a.path.localeCompare(b.path));
  for (const [path, value] of Object.entries(data)) await fs.writeFile(join(source, path), value);
  return { root, manifest, inventory, bytes: await packSource(source, inventory) };
}
test('source ZIP round trip preserves only verified bytes and executable mode', async t => {
  const { root, bytes, inventory } = await packageFixture(t);
  const destination = join(root, 'restored'); await unpackSource(bytes, inventory, destination);
  assert.match(await fs.readFile(join(destination, 'index.html'), 'utf8'), /title>App/);
  if (process.platform !== 'win32') assert.equal((await fs.stat(join(destination, 'run.sh'))).mode & 0o777, 0o700);
  await assert.rejects(unpackSource(bytes, inventory.map(file => ({ ...file, sha256: '0'.repeat(64) })), join(root, 'bad-hash')), /digest/);
  await assert.rejects(unpackSource(bytes, inventory.slice(1), join(root, 'missing')), /inventory/);
});
test('source ZIP rejects traversal, encrypted entries and inventory duplicates', async t => {
  const { root, bytes, inventory } = await packageFixture(t);
  const encrypted = Buffer.from(bytes), central = encrypted.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
  encrypted.writeUInt16LE(encrypted.readUInt16LE(central + 8) | 1, central + 8);
  await assert.rejects(unpackSource(encrypted, inventory, join(root, 'encrypted')), /Unsupported/);
  await assert.rejects(unpackSource(bytes, [...inventory, inventory[0]!], join(root, 'duplicate')), /inventory/);
  await assert.rejects(packSource(root, [{ path: '../secret' }]), /Unsafe/);
});
test('market install chooses highest compatible source and never forwards account token to artifact origin', async t => {
  const fixture = await packageFixture(t), { root, manifest, inventory, bytes } = fixture;
  const artifact = { sizeBytes: bytes.length, sha256: sha256(bytes), downloadUrl: 'https://content.example/source.zip' };
  const distribution = { schema: 2, appId: 'remote', versionId: 'v3', sequence: 3, manifest, files: inventory, dependencies: [], artifact,
    releaseDigest: sha256(Buffer.from(canonical({ schema: 2, artifactSha256: artifact.sha256, manifest, files: inventory, dependencies: [] }))) };
  let tokens = 0; const calls: Record<string, unknown>[] = [];
  const core: LocalAppCoreTransport = { dispose: async () => undefined, request: async <T>(request: Record<string, unknown>) => {
    calls.push(request);
    if (request.op === 'import') { assert.match(await fs.readFile(join(String(request.sourcePath), 'index.html'), 'utf8'), /title>App/); return { record: { appId: 'a' + '1'.repeat(32) } } as T; }
    if (request.op === 'get') return { appId: 'a' + '1'.repeat(32), name: 'Portable', summary: '', currentRevision: 1, latestRevision: 1, executionGrants: [] } as T;
    if (request.op === 'getRelease') return { manifest, digest: 'local-digest', revision: 1 } as T;
    if (request.op === 'layout') return { sourcePath: root, dataPath: root } as T;
    throw new Error('Unexpected ' + request.op);
  } };
  const store = new CoreLocalAppStore(join(root, 'config/local-apps/v2'), core);
  const fetcher = (async (input: URL | RequestInfo, init?: RequestInit) => {
    const url = String(input);
    if (url.startsWith('https://content.example')) { assert.equal(new Headers(init?.headers).has('Authorization'), false); return new Response(bytes); }
    if (url.endsWith('/distribution')) return new Response(JSON.stringify({ data: { ...distribution, sequence: JSON.parse(String(init?.body)).target.environment === 'web' ? 3 : 2 } }));
    throw new Error('Unexpected URL');
  }) as typeof fetch;
  const client = new LocalAppMarketClient(store, async () => { tokens++; return 'private-token'; }, 'https://market.example/api/v2', fetcher);
  const installed = await client.install('remote'); assert.equal(installed.releaseDigest, 'local-digest');
  assert.equal(tokens, 0); assert.equal(calls.find(r => r.op === 'import')?.marketOrigin && (calls.find(r => r.op === 'import')!.marketOrigin as { digest: string }).digest, distribution.releaseDigest);
});

for (const lostAt of ['listing', 'version', 'upload', 'publish', 'expired']) test(`publication recovers a lost ${lostAt} response after client recreation without duplicating a release`, async t => {
  const fixture = await packageFixture(t), { root, manifest, inventory } = fixture;
  const appId = 'a' + '2'.repeat(32), digest = 'b'.repeat(64), source = join(root, 'source');
  const row = { id: appId, revision: 1, releaseDigest: digest } as DesktopLocalApp;
  const release = { appId, revision: 1, digest, manifest: manifest as DesktopAppManifest } as AppRelease;
  const store = new CoreLocalAppStore(join(root, 'config/local-apps/v2'), { request: async () => { throw new Error('Unexpected core request'); }, dispose: async () => undefined });
  store.get = async () => row;
  const prepared = { row, release, directory: source, files: inventory };
  const mutations = new Map<string, { body: string; result: unknown }>();
  const stages: string[] = []; let dropped = false, listingCount = 0, versionCount = 0;
  let uploaded: Buffer | undefined;
  const fetcher = (async (input: URL | RequestInfo, init?: RequestInit) => {
    const url = new URL(String(input)), headers = new Headers(init?.headers), key = headers.get('Idempotency-Key');
    if (init?.method === 'GET') return Response.json({ data: lostAt === 'expired' && url.pathname.endsWith('/version-1') ? { status: 'failed', error: { code: 'UPLOAD_EXPIRED' } } : { status: 'awaiting_upload' } });
    assert.ok(key); assert.equal(headers.get('Authorization'), 'Bearer author');
    const stage = url.pathname.endsWith('/apps') ? 'listing' : url.pathname.endsWith('/versions') ? 'version' : url.pathname.endsWith('/artifact') ? 'upload' : 'publish';
    stages.push(stage);
    const body = stage === 'upload' ? Buffer.from(init!.body as Uint8Array).toString('base64') : String(init?.body);
    const previous = mutations.get(key);
    let result: unknown;
    if (previous) { assert.equal(body, previous.body, 'retry must retain the exact original request'); result = previous.result; }
    else {
      if (stage === 'listing') { listingCount++; result = { id: 'remote-app' }; }
      else if (stage === 'version') { versionCount++; result = { id: `version-${versionCount}`, upload: { path: `/api/v2/versions/version-${versionCount}/artifact` } }; }
      else if (stage === 'publish') result = { status: 'queued' };
      else { uploaded = Buffer.from(init!.body as Uint8Array); result = {}; }
      mutations.set(key, { body, result });
    }
    if ((stage === lostAt || lostAt === 'expired' && stage === 'upload') && !dropped) { dropped = true; throw new Error('Connection lost after server commit'); }
    return Response.json({ data: result });
  }) as typeof fetch;
  const makeClient = () => new LocalAppMarketClient(store, async () => 'author', 'https://market.example/api/v2', fetcher, () => 'owner');
  const metadata = { name: 'First title', summary: 'First summary', description: '', category: 'tools', notes: 'First notes' };
  await assert.rejects(makeClient().publish(prepared, metadata), /Connection lost/);
  await fs.rm(source, { recursive: true }); // Retrying uses the immutable reviewed ZIP, never another export.
  const result = await makeClient().publish(prepared, { ...metadata, name: 'Changed title', notes: 'Changed notes' });
  assert.deepEqual(result, { appId: 'remote-app', versionId: lostAt === 'expired' ? 'version-2' : 'version-1', status: 'queued' });
  assert.equal(listingCount, 1); assert.equal(versionCount, lostAt === 'expired' ? 2 : 1); assert.ok(uploaded);
  await unpackSource(uploaded!, inventory, join(root, 'uploaded'));
  assert.ok(stages.filter(stage => stage === (lostAt === 'expired' ? 'upload' : lostAt)).length >= 2);
});
