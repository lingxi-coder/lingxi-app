import { promises as fs } from 'node:fs';
import { dirname, join, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { LocalAppAgentSettings } from '../../shared/localAppAgent.js';
import type { DesktopAppManifest, DesktopLocalApp, DesktopAuthoringRef, DesktopAuthoringSourceRef, LocalAppCapability, LocalAppPhase } from '../../shared/localApps.js';
import { isDesktopAuthoringRef } from '../../shared/localApps.js';
import { LocalAppStore, appId, atomicJson } from './store.js';
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
interface RunPreparation { record: AppRecord; release: AppRelease; run: AppRun; sourcePath: string; workPath: string; dataPath: string; preparationDataPath: string }
interface Preferences { background: boolean; grantsByDigest: Record<string, LocalAppCapability[]>; agent?: LocalAppAgentSettings }
/** Only the Rust library owns identities, releases, consent digests and run records. */
export class CoreLocalAppStore implements LocalAppStore {
  readonly importsRoot: string;
  private readonly layouts = new Map<string, AppLayout>();
  private readonly prepared = new Map<string, RunPreparation>();
  private readonly runRecords = new Map<string, AppRun>();
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
    release ??= await this.core.request<AppRelease>({ op: 'getRelease', appId: record.appId, revision: record.latestRevision });
    const layout = await this.core.request<AppLayout>({ op: 'layout', appId: record.appId, revision: release.revision });
    this.layouts.set(`${record.appId}:${release.revision}`, layout);
    const prefs = await this.preferences(record.appId);
    const manifest = { ...release.manifest, capabilities: [...(release.manifest.capabilities ?? [])] };
    if (manifest.install?.length || manifest.build?.length || manifest.processes?.length || manifest.entry.type === 'server') {
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
  async publish(input: Parameters<LocalAppStore['publish']>[0] & { expectedRevision?: number }): Promise<DesktopLocalApp> {
    if (input.id && !Number.isSafeInteger(input.expectedRevision)) throw new Error('Updating an application requires expectedRevision from its latest read');
    let sourcePath = input.staged_path;
    let temporary: string | undefined;
    if (input.html !== undefined) {
      if (!input.html.trim()) throw new Error('The application document is empty');
      temporary = join(this.importsRoot, randomUUID()); await fs.mkdir(temporary, { recursive: true });
      await fs.writeFile(join(temporary, 'index.html'), input.html);
      await atomicJson(join(temporary, 'lingxi-app.json'), { schema: 2, name: input.name ?? (input.id ? (await this.get(input.id)).name : undefined), summary: input.summary,
        entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'web' }, { platform: 'windows', environment: 'web' }, { platform: 'linux', environment: 'web' }, { platform: 'android', environment: 'web' }, { platform: 'ios', environment: 'web' }], package: { files: ['index.html'], dependencyLocks: [] } });
      sourcePath = temporary;
    } else if (sourcePath) {
      const imports = await fs.realpath(this.importsRoot), staged = await fs.realpath(sourcePath);
      if (!staged.startsWith(imports + sep)) throw new Error('Invalid application staging directory');
    } else throw new Error('Publishing requires html or a staged project');
    try {
      const previous = input.id ? await this.get(input.id) : undefined;
      const result = await this.core.request<{ record: AppRecord; release: AppRelease }>({ op: 'publish', appId: input.id, expectedRevision: input.expectedRevision ?? 0, sourcePath,
        authoring: previous?.authoring ?? input.authoring });
      return this.row(result.record, result.release);
    } finally { if (temporary) await fs.rm(temporary, { recursive: true, force: true }); }
  }
  async bindAuthoring(id: string, authoring: DesktopAuthoringRef): Promise<DesktopAuthoringRef> {
    const owner = appId(id);
    if (authoring.appId !== undefined && authoring.appId !== owner) throw new Error('Authoring binding belongs to another application');
    if (authoring.projectRef.kind !== 'desktopPath' || !authoring.projectRef.path || !authoring.sourceDirectory
      || !authoring.workspaceId || !authoring.authoringSessionId) throw new Error('Invalid Desktop authoring binding');
    await this.core.request({ op: 'bindAuthoring', appId: owner, authoring: { ...authoring, appId: owner } });
    const saved = await this.get(owner);
    if (!isDesktopAuthoringRef(saved.authoring) || saved.authoring.workspaceId !== authoring.workspaceId || saved.authoring.authoringSessionId !== authoring.authoringSessionId) throw new Error('Authoring binding was not saved');
    return saved.authoring;
  }
  async assertTarget(row: DesktopLocalApp): Promise<void> {
    const platform = process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux';
    const architecture = process.arch === 'arm64' ? 'aarch64' : process.arch === 'ia32' ? 'x86' : process.arch;
    const capabilities = ['process', 'files', 'clipboard', 'notifications', 'camera', 'microphone', 'network', 'background', 'agent'];
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
