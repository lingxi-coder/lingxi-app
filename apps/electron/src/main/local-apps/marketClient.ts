import { promises as fs } from 'node:fs';
import { dirname, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { localAppMarketApiRoot } from '../../shared/productConfig.js';
import type { DesktopAppManifest, DesktopLocalApp, LocalAppTarget } from '../../shared/localApps.js';
import { CoreLocalAppStore, type AppRecord, type AppRelease } from './coreStore.js';
import { canonical, packSource, sha256, unpackSource, type SourceFile } from './sourcePackage.js';

interface Distribution { schema: number; appId: string; versionId: string; sequence: number; releaseDigest: string; manifest: DesktopAppManifest; files: SourceFile[]; dependencies: SourceFile[]; artifact: { sizeBytes: number; sha256: string; downloadUrl: string } }
export interface MarketPublication { name: string; summary: string; description: string; category: string; notes: string }
interface PublicationRelease {
  archiveHash: string; sizeBytes: number; createKey: string; uploadKey: string; publishKey: string;
  versionBody: { format: 'zip'; filename: 'source.zip'; sizeBytes: number; sha256: string; notes: string };
  version?: { id: string; upload: { path: string } }; status?: string;
  expiredAttempts?: { id: string; createKey: string; uploadKey: string; publishKey: string }[];
}
interface PublicationReceipt {
  schema: 1; createKey: string; createBody: Omit<MarketPublication, 'notes'> & { instructions: string };
  appId?: string; releases: Record<string, PublicationRelease>;
}
async function durableWrite(path: string, bytes: string | Buffer): Promise<void> {
  await fs.mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${randomUUID()}.tmp`;
  try {
    const handle = await fs.open(temporary, 'wx', 0o600);
    try { await handle.writeFile(bytes); await handle.sync(); } finally { await handle.close(); }
    await fs.rename(temporary, path);
    if (process.platform !== 'win32') {
      const directory = await fs.open(dirname(path), 'r');
      try { await directory.sync(); } finally { await directory.close(); }
    }
  } finally { await fs.rm(temporary, { force: true }); }
}
export function desktopTargets(): LocalAppTarget[] {
  const platform = process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux';
  const architecture = process.arch === 'arm64' ? 'aarch64' : process.arch === 'x64' ? 'x86_64' : process.arch === 'ia32' ? 'x86' : process.arch;
  return ['web', 'native'].map(environment => ({ platform, environment: environment as 'web' | 'native', architecture, runtimeVersion: '0.1.0', channels: ['desktop'], capabilities: ['process', 'files', 'clipboard', 'notifications', 'camera', 'microphone', 'background', 'agent', 'network'] }));
}
export class LocalAppMarketClient {
  private static readonly publications = new Map<string, Promise<unknown>>();
  readonly base: URL;
  constructor(private readonly store: CoreLocalAppStore, private readonly token: () => Promise<string>, base = localAppMarketApiRoot, private readonly fetcher: typeof fetch = fetch, private readonly accountId?: () => string | undefined) {
    this.base = new URL(base.replace(/\/$/, '') + '/');
    if (this.base.protocol !== 'https:' && !['127.0.0.1', 'localhost'].includes(this.base.hostname)) throw new Error('Market requires HTTPS');
  }
  async request<T>(path: string, body?: unknown, authenticated = false, method = body === undefined ? 'GET' : 'POST', extraHeaders: Record<string, string> = {}): Promise<T> {
    const url = new URL(path.replace(/^\//, ''), this.base);
    if (url.origin !== this.base.origin || !url.pathname.startsWith(this.base.pathname)) throw new Error('Invalid market API route');
    const headers: Record<string, string> = { Accept: 'application/json', ...extraHeaders };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    if (authenticated) headers.Authorization = `Bearer ${await this.token()}`;
    if (method !== 'GET') headers['Idempotency-Key'] ??= randomUUID();
    const response = await this.fetcher(url, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), redirect: 'error', signal: AbortSignal.timeout(30_000) });
    const result = await response.json() as { data?: T; error?: { code?: string; message?: string } };
    if (!response.ok) throw Object.assign(new Error(result.error?.message ?? `Market request failed (${response.status})`), { code: result.error?.code });
    return result.data as T;
  }
  async install(remoteId: string, existing?: DesktopLocalApp): Promise<DesktopLocalApp> {
    if (existing && existing.marketOrigin?.appId !== remoteId) throw new Error('Update does not belong to this local installation');
    const candidates: Distribution[] = [];
    for (const target of desktopTargets()) {
      try { candidates.push(await this.request<Distribution>(`apps/${encodeURIComponent(remoteId)}/distribution`, { target })); }
      catch (error) { if ((error as { code?: string }).code !== 'NO_COMPATIBLE_RELEASE') throw error; }
    }
    const distribution = candidates.sort((a, b) => b.sequence - a.sequence)[0];
    if (!distribution || distribution.schema !== 2 || distribution.appId !== remoteId) throw new Error('No compatible release for this Desktop');
    if (existing?.marketOrigin?.releaseId === distribution.versionId) return existing;
    if (distribution.artifact.sizeBytes <= 0 || distribution.artifact.sizeBytes > 25 * 1024 * 1024) throw new Error('Package exceeds download limit');
    const artifact = new URL(distribution.artifact.downloadUrl, this.base);
    if (artifact.protocol !== 'https:' && !(artifact.origin === this.base.origin && ['127.0.0.1', 'localhost'].includes(artifact.hostname))) throw new Error('Invalid package download URL');
    // Artifact requests deliberately never carry the account Authorization header.
    const response = await this.fetcher(artifact, { redirect: 'error', signal: AbortSignal.timeout(60_000) });
    if (!response.ok || !response.body) throw new Error('Package download failed');
    const chunks: Buffer[] = []; let size = 0;
    for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) { size += chunk.byteLength; if (size > distribution.artifact.sizeBytes) throw new Error('Package exceeds declared size'); chunks.push(Buffer.from(chunk)); }
    const bytes = Buffer.concat(chunks);
    if (size !== distribution.artifact.sizeBytes || sha256(bytes) !== distribution.artifact.sha256) throw new Error('Package checksum mismatch');
    const digest = sha256(Buffer.from(canonical({ schema: 2, artifactSha256: distribution.artifact.sha256, manifest: distribution.manifest, files: distribution.files, dependencies: distribution.dependencies })));
    if (digest !== distribution.releaseDigest) throw new Error('Market release checksum mismatch');
    const stage = join(this.store.importsRoot, randomUUID()); await fs.mkdir(stage, { recursive: true, mode: 0o700 });
    try {
      const sourcePath = join(stage, 'source'); await unpackSource(bytes, distribution.files, sourcePath);
      if (canonical(JSON.parse(await fs.readFile(join(sourcePath, 'lingxi-app.json'), 'utf8'))) !== canonical(distribution.manifest)) throw new Error('Package manifest differs from approved release');
      const marketOrigin = { appId: remoteId, releaseId: distribution.versionId, digest: distribution.releaseDigest };
      const result = await this.store.core.request<{ record: AppRecord }>({ op: existing ? 'publish' : 'import', sourcePath, marketOrigin,
        ...(existing ? { appId: existing.id, expectedRevision: existing.revision } : {}) });
      return this.store.get(result.record.appId);
    } finally { await fs.rm(stage, { recursive: true, force: true }); }
  }
  async preparePublication(id: string, expectedRevision: number): Promise<{ row: DesktopLocalApp; release: AppRelease; directory: string; files: { path: string; executable: boolean }[] }> {
    const row = await this.store.get(id), directory = join(this.store.importsRoot, `export-${randomUUID()}`);
    if (row.revision !== expectedRevision) throw new Error('Application changed; review its latest revision before publishing');
    const release = await this.store.core.request<AppRelease>({ op: 'getRelease', appId: id, revision: row.revision });
    const exported = await this.store.core.request<{ files: { path: string; executable: boolean }[] }>({ op: 'export', appId: id, revision: row.revision, destination: directory });
    return { row, release, directory, files: exported.files };
  }
  async publish(prepared: Awaited<ReturnType<LocalAppMarketClient['preparePublication']>>, metadata: MarketPublication): Promise<{ appId: string; versionId: string; status: string }> {
    const key = this.store.directory(prepared.row.id);
    const previous = LocalAppMarketClient.publications.get(key) ?? Promise.resolve();
    const next = previous.catch(() => undefined).then(() => this.publishSerial(prepared, metadata));
    LocalAppMarketClient.publications.set(key, next);
    try { return await next; } finally { if (LocalAppMarketClient.publications.get(key) === next) LocalAppMarketClient.publications.delete(key); }
  }
  private async publishSerial(prepared: Awaited<ReturnType<LocalAppMarketClient['preparePublication']>>, metadata: MarketPublication): Promise<{ appId: string; versionId: string; status: string }> {
    await this.token();
    const owner = this.accountId?.();
    const writer = new LocalAppMarketClient(this.store, async () => {
      const token = await this.token();
      if (this.accountId && this.accountId() !== owner) throw new Error('Account changed during publication; retry with the intended author account');
      const latest = await this.store.get(prepared.row.id);
      if (latest.revision !== prepared.row.revision || latest.releaseDigest !== prepared.release.digest) throw new Error('Application changed; review its latest revision before publishing');
      if (this.accountId && this.accountId() !== owner) throw new Error('Account changed during publication; retry with the intended author account');
      return token;
    }, this.base.href, this.fetcher, this.accountId);
    await writer.token();
    const scope = sha256(Buffer.from(`${this.base.href}:${owner ?? 'default'}`));
    const directory = join(this.store.directory(prepared.row.id), 'publications', scope);
    const receiptPath = join(directory, 'receipt.json');
    let receipt: PublicationReceipt;
    try {
      receipt = JSON.parse(await fs.readFile(receiptPath, 'utf8'));
      if (receipt.schema !== 1 || !receipt.createKey || !receipt.createBody || !receipt.releases || typeof receipt.releases !== 'object') throw new Error('Invalid publication receipt; preserve it for recovery');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      receipt = { schema: 1, createKey: randomUUID(), createBody: { name: metadata.name, summary: metadata.summary, description: metadata.description, category: metadata.category, instructions: '' }, releases: {} };
    }
    const save = () => durableWrite(receiptPath, JSON.stringify(receipt));
    const digest = prepared.release.digest;
    if (!/^[a-f0-9]{64}$/.test(digest)) throw new Error('Invalid local release digest');
    const archive = join(directory, digest, 'source.zip');
    let release = receipt.releases[digest];
    let bytes: Buffer;
    if (release) {
      if (![release.createKey, release.uploadKey, release.publishKey].every(key => typeof key === 'string' && key.length > 0) || !release.versionBody || release.versionBody.sha256 !== release.archiveHash || release.versionBody.sizeBytes !== release.sizeBytes) throw new Error('Invalid release publication receipt; preserve it for recovery');
      bytes = await fs.readFile(archive);
      if (sha256(bytes) !== release.archiveHash || bytes.length !== release.sizeBytes) throw new Error('Prepared public archive changed; preserve it for recovery');
    } else {
      // No mutation can precede this receipt. An orphan archive is safe to replace.
      bytes = await packSource(prepared.directory, prepared.files);
      await durableWrite(archive, bytes);
      release = { archiveHash: sha256(bytes), sizeBytes: bytes.length, createKey: randomUUID(), uploadKey: randomUUID(), publishKey: randomUUID(),
        versionBody: { format: 'zip', filename: 'source.zip', sizeBytes: bytes.length, sha256: sha256(bytes), notes: metadata.notes } };
      receipt.releases[digest] = release;
    }
    // Persist fixed bodies and keys before contacting the server, including retries.
    await save();
    const validId = (value: string) => { if (!/^[A-Za-z0-9_-]{1,128}$/.test(value)) throw new Error('Invalid market identity'); return value; };
    if (!receipt.appId) {
      const app = await writer.request<{ id: string }>('apps', receipt.createBody, true, 'POST', { 'Idempotency-Key': receipt.createKey });
      receipt.appId = validId(app.id); await save();
    }
    const appId = validId(receipt.appId);
    if (release.version) {
      const previous = await writer.request<{ status: string; error?: { code: string } }>(`versions/${validId(release.version.id)}`, undefined, true);
      if (previous.status === 'failed' && previous.error?.code === 'UPLOAD_EXPIRED') {
        // This click explicitly retries a confirmed terminal upload. Preserve
        // its identity and journal a fresh attempt before any new mutation.
        (release.expiredAttempts ??= []).push({ id: release.version.id, createKey: release.createKey, uploadKey: release.uploadKey, publishKey: release.publishKey });
        release.createKey = randomUUID(); release.uploadKey = randomUUID(); release.publishKey = randomUUID();
        delete release.version; delete release.status; await save();
      }
    }
    if (!release.version) {
      release.version = await writer.request<{ id: string; upload: { path: string } }>(`apps/${encodeURIComponent(appId)}/versions`, release.versionBody, true, 'POST', { 'Idempotency-Key': release.createKey });
      validId(release.version.id); await save();
    }
    const version = release.version; validId(version.id);
    const upload = new URL(version.upload.path, this.base);
    if (upload.origin !== this.base.origin || upload.search || upload.hash || upload.pathname !== this.base.pathname + `versions/${encodeURIComponent(version.id)}/artifact`) throw new Error('Invalid source upload route');
    const response = await this.fetcher(upload, { method: 'PUT', headers: { Authorization: `Bearer ${await writer.token()}`, 'Content-Type': 'application/octet-stream', 'Idempotency-Key': release.uploadKey }, body: bytes as unknown as BodyInit, redirect: 'error', signal: AbortSignal.timeout(60_000) });
    if (!response.ok) throw new Error(`Source upload failed (${response.status})`);
    const result = await writer.request<{ status: string }>(`versions/${encodeURIComponent(version.id)}/publish`, {}, true, 'POST', { 'Idempotency-Key': release.publishKey });
    release.status = result.status; await save();
    return { appId, versionId: version.id, status: result.status };
  }
}
