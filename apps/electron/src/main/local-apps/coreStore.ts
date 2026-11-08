import { promises as fs } from 'node:fs';
import { basename, dirname, join, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { LocalAppAgentSettings } from '../../shared/localAppAgent.js';
import type { DesktopAppManifest, DesktopLocalApp, DesktopAuthoringRef, DesktopAuthoringProjectRef, DesktopAuthoringSourceRef, LocalAppCapability, LocalAppPhase } from '../../shared/localApps.js';
import { LocalAppStore, appId, atomicJson } from './store.js';
import { PublicationIntents, type PublicationIntent } from './publicationIntents.js';
import type { LocalAppCoreTransport } from './coreClient.js';

export interface AppRecord {
  appId: string; name: string; summary: string; currentRevision: number; latestRevision: number;
  authoring?: DesktopAuthoringRef | DesktopAuthoringSourceRef;
  marketOrigin?: { appId: string; releaseId: string; digest: string; remixedFrom?: string };
  executionGrants: string[]; createdAtMs: number; updatedAtMs: number;
}
export interface AppRelease { marketOrigin?: AppRecord['marketOrigin']; appId: string; revision: number; digest: string; manifest: DesktopAppManifest }
interface AppLayout { sourcePath: string; dataPath: string; releasesPath: string; runsPath: string }
export interface AppRun { appId: string; runId: string; revision: number; releaseDigest: string; phase: string; services: { name: string; port: number }[]; startedAtMs: number; updatedAtMs: number; error?: string }
export interface DesktopAuthoringSourceProof { sourceId: string; sourceDirectory: string; sourcePath: string; requestId?: string; ownerAppId?: string; targetAppId?: string }
export interface DesktopAuthoringGrant { sourceId: string; authoring: DesktopAuthoringRef; sessionMode: 'code'; ownerAppId?: string }
export interface PendingLocalAppPublication { requestId: string; appId?: string; sourceDirectory: string; expectedRevision: number }
interface RunPreparation { record: AppRecord; release: AppRelease; run: AppRun; sourcePath: string; workPath: string; dataPath: string; preparationDataPath: string }
interface Preferences { background: boolean; grantsByDigest: Record<string, LocalAppCapability[]>; agent?: LocalAppAgentSettings }
async function syncStagedTree(path: string): Promise<void> {
  const stat = await fs.lstat(path);
  if (stat.isSymbolicLink()) throw new Error('Application staging contains a symbolic link');
  if (stat.isDirectory()) {
    for (const name of await fs.readdir(path)) await syncStagedTree(join(path, name));
    if (process.platform === 'win32') return;
  } else if (!stat.isFile()) throw new Error('Application staging contains a special file');
  const handle = await fs.open(path, 'r');
  try { await handle.sync(); } finally { await handle.close(); }
}
/** Only the Rust library owns identities, releases, consent digests and run records. */
export class CoreLocalAppStore implements LocalAppStore {
  readonly importsRoot: string;
  private readonly layouts = new Map<string, AppLayout>();
  private readonly prepared = new Map<string, RunPreparation>();
  private readonly runRecords = new Map<string, AppRun>();
  private publicationIntentsPromise?: Promise<PublicationIntents>;
  constructor(readonly root: string, readonly core: LocalAppCoreTransport) { this.importsRoot = join(dirname(dirname(root)), 'local-app-imports'); }
  directory(id: string): string { return join(this.root, 'host', appId(id)); }
  revisionPath(id: string, revision: number): string {
    const layout = this.layouts.get(`${id}:${revision}`); if (!layout) throw new Error('Application layout has not been loaded'); return layout.sourcePath;
  }
  dataPath(id: string): string { const layout = [...this.layouts.entries()].find(([key]) => key.startsWith(id + ':'))?.[1]; if (!layout) throw new Error('Application data layout has not been loaded'); return layout.dataPath; }
  private async preferences(id: string): Promise<Preferences> {
    try { return JSON.parse(await fs.readFile(join(this.directory(id), 'preferences.json'), 'utf8')); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; return { background: false, grantsByDigest: {} }; }
  }
  private async row(record: AppRecord, release?: AppRelease): Promise<DesktopLocalApp> {
    const prefsPromise = this.preferences(record.appId);
    release ??= await this.core.request<AppRelease>({ op: 'getRelease', appId: record.appId, revision: record.latestRevision });
    const [layout, prefs] = await Promise.all([
      this.core.request<AppLayout>({ op: 'layout', appId: record.appId, revision: release.revision }),
      prefsPromise,
    ]);
    this.layouts.set(`${record.appId}:${release.revision}`, layout);
    const manifest = { ...release.manifest, capabilities: [...(release.manifest.capabilities ?? [])] };
    if (manifest.install?.length || manifest.build?.length || manifest.processes?.length || Object.keys(manifest.mcpServers ?? {}).length || manifest.entry.type === 'server') {
      if (!manifest.capabilities.includes('process')) manifest.capabilities.push('process');
    }
    const caps = manifest.capabilities;
    const grants = (prefs.grantsByDigest?.[release.digest] ?? []).filter(cap => caps.includes(cap) && cap !== 'process');
    if (record.executionGrants.includes(release.digest)) grants.push('process');
    return { id: record.appId, name: manifest.name, summary: manifest.summary ?? '', revision: release.revision, currentRevision: record.currentRevision,
      releaseDigest: release.digest, marketOrigin: release.marketOrigin ?? record.marketOrigin, manifest, createdAt: record.createdAtMs, updatedAt: record.updatedAtMs,
      authoring: record.authoring ? structuredClone(record.authoring) : undefined,
      background: prefs.background, grants, agent: prefs.agent, phase: 'stopped' };
  }
  async get(id: string): Promise<DesktopLocalApp> { return this.row(await this.core.request<AppRecord>({ op: 'get', appId: id })); }
  async getRevision(id: string, revision?: number): Promise<DesktopLocalApp> {
    const record = await this.core.request<AppRecord>({ op: 'get', appId: id });
    return this.row(record, await this.core.request<AppRelease>({ op: 'getRelease', appId: id, revision: revision ?? record.currentRevision }));
  }
  async list(): Promise<DesktopLocalApp[]> { return Promise.all((await this.core.request<AppRecord[]>({ op: 'list' })).map(row => this.row(row))); }
  async save(row: DesktopLocalApp): Promise<void> {
    const current = await this.getRevision(row.id, row.revision);
    if (current.releaseDigest !== row.releaseDigest) throw new Error('Application release changed while granting access; review the new release');
    if (row.grants.includes('process') && !current.grants.includes('process')) await this.core.request({ op: 'grant', appId: row.id, digest: row.releaseDigest });
    if (current.grants.includes('process') && !row.grants.includes('process')) await this.core.request({ op: 'revoke', appId: row.id });
    const prefs = await this.preferences(row.id);
    await atomicJson(join(this.directory(row.id), 'preferences.json'), { background: row.background, grantsByDigest: { ...prefs.grantsByDigest, [row.releaseDigest!]: row.grants.filter(cap => cap !== 'process') }, agent: row.agent });
  }
  async publish(input: Parameters<LocalAppStore['publish']>[0] & { expectedRevision?: number;
    nativeAuthoring?: DesktopAuthoringGrant & { setCurrent: boolean } }): Promise<DesktopLocalApp> {
    if (input.id && !Number.isSafeInteger(input.expectedRevision)) throw new Error('Updating an application requires expectedRevision from its latest read');
    if (input.authoring) throw new Error('Publishing with authoring requires a native authoring receipt');
    if (!input.publication) throw new Error('A durable Local App publication request is required');
    if (input.nativeAuthoring && (input.nativeAuthoring.authoring.sourceDirectory !== input.publication.sourceDirectory
      || input.nativeAuthoring.authoring.projectRef.path !== input.publication.sourceDirectory
      || input.nativeAuthoring.authoring.authoringSessionId !== input.publication.authoringSessionId)) {
      throw new Error('The Local App publication does not match its captured authoring receipt');
    }
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(input.publication.requestId)) {
      throw new Error('Invalid Local App publication request');
    }
    await fs.mkdir(this.importsRoot, { recursive: true, mode: 0o700 });
    const imports = await fs.realpath(this.importsRoot);
    const intents = await this.publicationIntents();
    const previous = await intents.get(input.publication.requestId);
    let stagingInputPath: string | undefined;
    let temporary: string | undefined;
    let preserveTemporary = false;
    try {
      if (input.html !== undefined) {
        if (!input.html.trim()) throw new Error('The application document is empty');
        temporary = join(imports, `host-prepare-${input.publication.requestId}-${randomUUID()}`);
        await fs.mkdir(temporary, { mode: 0o700 });
        await fs.writeFile(join(temporary, 'index.html'), input.html);
        await atomicJson(join(temporary, 'lingxi-app.json'), { schema: 2, name: input.name ?? (input.id ? (await this.get(input.id)).name : undefined), summary: input.summary,
          entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'web' }, { platform: 'windows', environment: 'web' }, { platform: 'linux', environment: 'web' }, { platform: 'android', environment: 'web' }, { platform: 'ios', environment: 'web' }], package: { files: ['index.html'], dependencyLocks: [] } });
        stagingInputPath = temporary;
      } else if (input.staged_path) {
        try { stagingInputPath = await fs.realpath(input.staged_path); }
        catch (error) { if (!previous || (error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        if (stagingInputPath && !stagingInputPath.startsWith(imports + sep)) throw new Error('Invalid application staging directory');
      } else throw new Error('Publishing requires html or a staged project');
      const native = input.nativeAuthoring;
      let committed: unknown | null = null;
      if (previous) committed = await this.core.request<unknown | null>({ op: 'getPublication', requestId: previous.requestId });
      // A completed Host intent proves this ID previously returned a durable
      // SDK result. If that receipt is missing, recreating the same staged
      // bytes must never turn a historical replay into a fresh mutation.
      if (previous?.state === 'completed' && !committed) throw new Error('The committed Local App publication receipt is no longer available');
      if (input.publicationReplayOnly && !committed) throw new Error('The committed Local App publication receipt is no longer available');
      let fingerprintPath = stagingInputPath;
      if (!fingerprintPath && previous) {
        for (const candidate of [previous.sourcePath, previous.stagingInputPath]) {
          try { await fs.access(candidate); fingerprintPath = candidate; break; }
          catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        }
      }
      if (!fingerprintPath && !committed) throw new Error('The saved Local App publication staging is missing');
      const stagedDigest = fingerprintPath
        ? (await this.core.request<{ stagedDigest: string }>({ op: 'fingerprintAuthoringPublication', sourcePath: fingerprintPath })).stagedDigest
        : previous!.stagedDigest;
      if (!/^[a-f0-9]{64}$/.test(stagedDigest)) throw new Error('Invalid Local App publication digest');
      if (fingerprintPath) await syncStagedTree(fingerprintPath);
      if (process.platform !== 'win32') {
        for (const path of [imports, dirname(imports)]) {
          const parent = await fs.open(path, 'r');
          try { await parent.sync(); } finally { await parent.close(); }
        }
      }
      // Persist the operation and exact input before taking ownership or
      // admitting any SDK mutation. A failed sync may still have installed
      // the intent, so retain its input until it can be inspected.
      preserveTemporary = true;
      const destination = join(imports, `host-publish-${input.publication.requestId}`);
      const intentInput = stagingInputPath && stagingInputPath !== destination ? stagingInputPath : previous?.stagingInputPath;
      if (!intentInput) throw new Error('The Local App staging source is not available');
      const intended = { ...input.publication, appId: input.id, expectedRevision: input.expectedRevision ?? 0,
        stagedDigest, sourcePath: destination, stagingInputPath: intentInput,
        ...(native ? { native: { sourceId: native.sourceId, setCurrent: native.setCurrent } } : {}) };
      let captured: Awaited<ReturnType<PublicationIntents['capture']>>;
      try { captured = await intents.capture(intended); }
      catch (error) {
        // A file rename may have committed the preparing intent even when its
        // directory sync failed. Take the exact input before the tool can
        // clean it up; no SDK publication has been admitted at this point.
        try {
          const landed = await intents.get(intended.requestId);
          if (landed?.state === 'preparing' && landed.sourceDirectory === intended.sourceDirectory
            && landed.authoringSessionId === intended.authoringSessionId && landed.appId === intended.appId
            && landed.expectedRevision === intended.expectedRevision && landed.stagedDigest === intended.stagedDigest
            && landed.sourcePath === intended.sourcePath && landed.stagingInputPath === intended.stagingInputPath
            && landed.native?.sourceId === intended.native?.sourceId) {
            try { await fs.lstat(landed.sourcePath); }
            catch (cause) {
              if ((cause as NodeJS.ErrnoException).code !== 'ENOENT') throw cause;
              await fs.rename(landed.stagingInputPath, landed.sourcePath);
            }
            await syncStagedTree(landed.sourcePath);
            if (process.platform !== 'win32') {
              const parent = await fs.open(dirname(landed.sourcePath), 'r');
              try { await parent.sync(); } finally { await parent.close(); }
            }
          }
        } catch { /* Keep the input for the SDK's own recovery on failure. */ }
        throw error;
      }
      let saved = captured.entry;
      if (!captured.created && !committed && saved.state === 'preparing' && stagingInputPath
        && stagingInputPath !== saved.stagingInputPath && stagingInputPath !== saved.sourcePath) {
        // The exact original digest was checked above. No SDK write is
        // admitted while preparing; a matching retained SDK input can repair
        // a path which vanished before host takeover.
        saved = await intents.replacePreparingInput(saved.requestId, stagingInputPath);
      }
      if (!captured.created && temporary && temporary !== saved.stagingInputPath) {
        await fs.rm(temporary, { recursive: true, force: true });
        temporary = undefined;
      }
      if (!committed) await this.preparePublicationStage(saved, intents);
      const result = await this.finishPublication(saved, intents, Boolean(committed) || input.publicationReplayOnly === true);
      preserveTemporary = false;
      return this.row(result.record, result.release);
    } finally { if (temporary && !preserveTemporary) await fs.rm(temporary, { recursive: true, force: true }); }
  }
  private async preparePublicationStage(saved: PublicationIntent, intents: PublicationIntents): Promise<void> {
    if (saved.state !== 'preparing') return;
    let present = true;
    try { await fs.lstat(saved.sourcePath); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; present = false; }
    const candidate = present ? saved.sourcePath : saved.stagingInputPath;
    const { stagedDigest } = await this.core.request<{ stagedDigest: string }>({ op: 'fingerprintAuthoringPublication', sourcePath: candidate });
    if (stagedDigest !== saved.stagedDigest) throw new Error('The saved Local App publication staging changed');
    if (!present) await fs.rename(saved.stagingInputPath, saved.sourcePath);
    await syncStagedTree(saved.sourcePath);
    if (process.platform !== 'win32') {
      const parent = await fs.open(dirname(saved.sourcePath), 'r');
      try { await parent.sync(); } finally { await parent.close(); }
    }
    await intents.ready(saved.requestId);
  }
  private async finishPublication(saved: PublicationIntent, intents: PublicationIntents, replayOnly = false): Promise<{ record: AppRecord; release: AppRelease }> {
    const request = { appId: saved.appId, expectedRevision: saved.expectedRevision, sourcePath: saved.sourcePath };
    const result = await this.core.request<{ record: AppRecord; release: AppRelease }>(saved.native ? {
      op: 'publishAuthoring', requestId: saved.requestId, stagedDigest: saved.stagedDigest,
      sourceId: saved.native.sourceId, sourceDirectory: saved.sourceDirectory,
      projectRef: { kind: 'desktopPath', path: saved.sourceDirectory }, authoringSessionId: saved.authoringSessionId,
      sessionMode: 'code', setCurrent: saved.native.setCurrent, ...(replayOnly ? { replayOnly: true } : {}), request,
    } : { op: 'publishOnce', requestId: saved.requestId, stagedDigest: saved.stagedDigest, sourceDirectory: saved.sourceDirectory,
      ...(replayOnly ? { replayOnly: true } : {}), request },
    { waitForCompletion: true });
    await intents.complete(saved.requestId);
    await fs.rm(saved.sourcePath, { recursive: true, force: true });
    await fs.rm(saved.stagingInputPath, { recursive: true, force: true });
    return result;
  }
  private async publicationIntents(): Promise<PublicationIntents> {
    if (!this.publicationIntentsPromise) this.publicationIntentsPromise = (async () => {
      let importsRoot: string;
      try { importsRoot = await fs.realpath(this.importsRoot); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
        // The operation receipt can still recover an SDK result after the
        // entire staging directory was removed. Its parent remains canonical.
        importsRoot = join(await fs.realpath(dirname(this.importsRoot)), basename(this.importsRoot));
      }
      return new PublicationIntents(this.root, importsRoot);
    })().catch(error => { this.publicationIntentsPromise = undefined; throw error; });
    return this.publicationIntentsPromise;
  }
  async pendingPublications(): Promise<PendingLocalAppPublication[]> {
    try {
      return (await (await this.publicationIntents()).pending()).map(({ requestId, appId, sourceDirectory, expectedRevision }) =>
        ({ requestId, appId, sourceDirectory, expectedRevision }));
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return []; throw error; }
  }
  async publicationIntent(requestId: string): Promise<PublicationIntent | undefined> {
    return (await this.publicationIntents()).get(requestId);
  }
  async resumePublication(requestId: string): Promise<DesktopLocalApp> {
    const intents = await this.publicationIntents(), saved = await intents.get(requestId);
    if (!saved || saved.state === 'completed') throw new Error('The pending Local App publication is no longer available');
    // A confirmed SDK receipt can replay without the original staging tree.
    // If the SDK has no receipt it revalidates the exact retained digest.
    const committed = await this.core.request<unknown | null>({ op: 'getPublication', requestId });
    if (!committed) await this.preparePublicationStage(saved, intents);
    const result = await this.finishPublication(saved, intents, Boolean(committed));
    return this.row(result.record, result.release);
  }
  async ensureAuthoringSource(sourceDirectory: string, projectRef: DesktopAuthoringProjectRef, requestId: string, targetAppId?: string): Promise<DesktopAuthoringSourceProof> {
    return this.core.request({ op: 'ensureAuthoringSource', requestId, sourceDirectory, projectRef,
      ...(targetAppId ? { targetAppId } : {}) }, { waitForCompletion: true });
  }
  async findAuthoringSource(sourceDirectory: string): Promise<DesktopAuthoringSourceProof | null> {
    return this.core.request({ op: 'findAuthoringSource', sourceDirectory });
  }
  async getAuthoringGrant(sourceDirectory: string, projectRef: DesktopAuthoringProjectRef, authoringSessionId: string): Promise<DesktopAuthoringGrant> {
    return this.core.request({ op: 'getAuthoringGrant', sourceDirectory, projectRef, authoringSessionId, sessionMode: 'code' });
  }
  async registerAuthoring(sourceId: string, requestId: string, authoring: DesktopAuthoringRef, setCurrent: boolean): Promise<DesktopAuthoringRef & { sessionMode: 'code' }> {
    return this.core.request({ op: 'registerAuthoring', sourceId, requestId, authoring, setCurrent, sessionMode: 'code' }, { waitForCompletion: true });
  }
  async assertTarget(row: DesktopLocalApp): Promise<void> {
    const platform = process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux';
    const architecture = process.arch === 'arm64' ? 'aarch64' : process.arch === 'ia32' ? 'x86' : process.arch;
    const capabilities = ['process', 'files', 'clipboard', 'notifications', 'camera', 'microphone', 'network', 'background', 'agent', 'mcp'];
    for (const environment of ['native', 'web']) {
      const result = await this.core.request<{ compatible: boolean }>({ op: 'checkTarget', appId: row.id, revision: row.revision,
        target: { platform, environment, architecture: architecture === 'x64' ? 'x86_64' : architecture, runtimeVersion: '0.1.0', channels: ['desktop'], capabilities } });
      if (result.compatible) return;
    }
    throw new Error('This application release does not support this Desktop runtime or requires unavailable capabilities');
  }
  async appendLog(id: string, runId: string, text: string): Promise<void> { await this.core.request({ op: 'appendLog', appId: id, runId, text }); }
  async prepareWorkspace(row: DesktopLocalApp, rebuild = false): Promise<string> {
    const prep = await this.core.request<RunPreparation>({ op: 'prepareRun', appId: row.id, revision: row.revision, rebuild }); this.prepared.set(row.id, prep); this.runRecords.set(prep.run.runId, prep.run);
    return prep.workPath;
  }
  preparationDataPath(id: string): string | undefined { return this.prepared.get(id)?.preparationDataPath; }
  run(id: string): AppRun | undefined { return this.prepared.get(id)?.run; }
  async updateRun(id: string, runId: string, phase: LocalAppPhase, services: { name: string; port: number }[], error?: string): Promise<void> {
    const run = this.runRecords.get(runId); if (!run || run.appId !== id) return;
    run.phase = phase === 'installing' || phase === 'building' ? 'preparing' : phase;
    run.services = services; run.error = error; run.updatedAtMs = Date.now();
    await this.core.request({ op: 'updateRun', run });
  }
  async activate(id: string, revision: number): Promise<void> { await this.core.request({ op: 'activateRelease', appId: id, revision }); }
  async revoke(id: string): Promise<void> {
    await this.core.request({ op: 'revoke', appId: id });
    const prefs = await this.preferences(id);
    await atomicJson(join(this.directory(id), 'preferences.json'), { ...prefs, grantsByDigest: {}, agent: prefs.agent ? { ...prefs.agent, enabled: false, tools: [], background: false } : undefined });
  }
  async remove(id: string): Promise<void> { await this.core.request({ op: 'remove', appId: id }); this.prepared.delete(id); }
}
