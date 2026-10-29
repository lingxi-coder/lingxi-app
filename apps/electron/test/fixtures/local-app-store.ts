// Deterministic disk fixture for native runtime unit tests. Production uses the Rust core.
import { promises as fs } from 'node:fs';
import { join, dirname, relative, sep } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { DesktopAuthoringRef, DesktopLocalApp } from '../../src/shared/localApps.js';
import { parseManifest } from '../../src/main/local-apps/manifest.js';
import { appId, atomicJson, containedFile, copySource, type LocalAppStore as LocalAppStoreContract } from '../../src/main/local-apps/store.js';
export class LocalAppStore implements LocalAppStoreContract {
  constructor(readonly root: string) {}
  directory(id: string): string { return join(this.root, appId(id)); }
  revisionPath(id: string, revision: number): string {
    if (!Number.isSafeInteger(revision) || revision < 1) throw new Error('Invalid app revision');
    return join(this.directory(id), 'revisions', String(revision));
  }
  dataPath(id: string): string { return join(this.directory(id), 'data'); }
  workspacePath(id: string, revision: number): string { return join(this.directory(id), 'workspaces', String(revision)); }
  async get(id: string): Promise<DesktopLocalApp> {
    const row = JSON.parse(await fs.readFile(join(this.directory(id), 'app.json'), 'utf8')) as DesktopLocalApp;
    if (row.id !== id) throw new Error('Application record identity mismatch');
    row.manifest = parseManifest(row.manifest);
    return row;
  }
  async getRevision(id: string, revision?: number): Promise<DesktopLocalApp> {
    const row = await this.get(id);
    if (revision === undefined || revision === row.revision) return row;
    const manifest = parseManifest(JSON.parse(await fs.readFile(join(this.revisionPath(id, revision), 'lingxi-app.json'), 'utf8')));
    return { ...row, revision, manifest };
  }
  async assertTarget(_row: DesktopLocalApp): Promise<void> {}
  async revoke(id: string): Promise<void> {
    const row = await this.get(id); row.grants = [];
    if (row.agent) row.agent = { ...row.agent, enabled: false, tools: [], background: false };
    await this.save(row);
  }
  async list(): Promise<DesktopLocalApp[]> {
    await fs.mkdir(this.root, { recursive: true, mode: 0o700 });
    const rows: DesktopLocalApp[] = [];
    for (const entry of await fs.readdir(this.root, { withFileTypes: true })) {
      if (!entry.isDirectory() || !/^a[a-f0-9]{32}$/.test(entry.name)) continue;
      try { rows.push(await this.get(entry.name)); }
      catch (error) {
        // First publication creates its private directory before the atomic
        // app.json commit. A crash can leave the same uncommitted directory.
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue;
        throw new Error(`Cannot load application ${entry.name}: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
      }
    }
    return rows.sort((a, b) => b.updatedAt - a.updatedAt);
  }
  async save(row: DesktopLocalApp): Promise<void> {
    const { phase: _phase, error: _error, runningRevision: _running, ...persisted } = row;
    await atomicJson(join(this.directory(row.id), 'app.json'), { ...persisted, phase: 'stopped' });
  }
  async bindAuthoring(id: string, authoring: DesktopAuthoringRef): Promise<DesktopAuthoringRef> {
    const owner = appId(id);
    if (authoring.appId !== undefined && authoring.appId !== owner) throw new Error('Authoring binding belongs to another application');
    if (authoring.projectRef.kind !== 'desktopPath' || !authoring.projectRef.path || !authoring.sourceDirectory
      || !authoring.workspaceId || !authoring.authoringSessionId) throw new Error('Invalid Desktop authoring binding');
    const row = await this.get(owner);
    row.authoring = { ...authoring, appId: owner };
    await this.save(row);
    return row.authoring;
  }
  async publish(input: { id?: string; name?: string; summary?: string; html?: string; staged_path?: string; authoring?: DesktopAuthoringRef; expectedRevision?: number }): Promise<DesktopLocalApp> {
    const previous = input.id ? await this.get(input.id) : undefined;
    const id = previous?.id ?? `a${randomUUID().replaceAll('-', '')}`;
    let latestRevision = previous?.revision ?? 0;
    try {
      for (const entry of await fs.readdir(join(this.directory(id), 'revisions'), { withFileTypes: true })) {
        const number = Number(entry.name);
        if (entry.isDirectory() && /^[1-9]\d*$/.test(entry.name) && Number.isSafeInteger(number)) latestRevision = Math.max(latestRevision, number);
      }
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    // A crash after installing a snapshot but before app.json commits must
    // never make that snapshot collide with every subsequent publication.
    const revision = latestRevision + 1;
    const temp = join(this.directory(id), `publish-${randomUUID()}`);
    const target = this.revisionPath(id, revision);
    await fs.mkdir(temp, { recursive: true, mode: 0o700 });
    let published = false;
    try {
      if (input.html !== undefined) {
        if (!input.html.trim()) throw new Error('The application document is empty');
        await fs.writeFile(join(temp, 'index.html'), input.html);
        await atomicJson(join(temp, 'lingxi-app.json'), { schema: 2, targets: [{ platform: 'macos', environment: 'web' }], name: input.name ?? previous?.name, summary: input.summary ?? previous?.summary,
          entry: { type: 'static', path: 'index.html' } });
      } else if (input.staged_path) {
        // Only the authenticated tool can submit its private staging directory.
        const imports = await fs.realpath(join(this.root, 'imports'));
        const staged = await fs.realpath(input.staged_path);
        if (!staged.startsWith(imports + sep) || relative(imports, staged).includes(sep)) throw new Error('Invalid application staging directory');
        await copySource(staged, temp);
      } else throw new Error('Publishing requires html or a staged project');
      const sourceManifest = parseManifest(JSON.parse(await fs.readFile(join(temp, 'lingxi-app.json'), 'utf8')));
      const manifest = parseManifest({ ...sourceManifest, name: input.name ?? sourceManifest.name, summary: input.summary ?? sourceManifest.summary });
      await atomicJson(join(temp, 'lingxi-app.json'), manifest);
      if (manifest.entry.type === 'static' && !manifest.build?.length) {
        if (!(await fs.stat(await containedFile(temp, manifest.entry.path))).isFile()) throw new Error('The application entry is not a file');
      }
      await fs.mkdir(dirname(target), { recursive: true, mode: 0o700 });
      await fs.rename(temp, target);
      published = true;
      const now = Date.now();
      const row: DesktopLocalApp = { id, revision, manifest, name: manifest.name, summary: manifest.summary ?? '',
        createdAt: previous?.createdAt ?? now, updatedAt: now, phase: 'stopped', background: previous?.background ?? false, agent: previous?.agent,
        releaseDigest: previous?.releaseDigest,
        grants: previous?.grants ?? [], authoring: input.authoring ?? previous?.authoring };
      await this.save(row);
      return row;
    } catch (error) {
      if (published) await fs.rm(target, { recursive: true, force: true });
      // An interrupted first publish must not leave a broken library row.
      if (!previous) await fs.rm(this.directory(id), { recursive: true, force: true });
      throw error;
    } finally { await fs.rm(temp, { recursive: true, force: true }); }
  }
  async prepareWorkspace(row: DesktopLocalApp): Promise<string> {
    const workspace = this.workspacePath(row.id, row.revision);
    try { await fs.access(join(workspace, 'lingxi-app.json')); }
    catch {
      const temp = `${workspace}.${randomUUID()}.tmp`;
      try {
        await copySource(this.revisionPath(row.id, row.revision), temp);
        await fs.mkdir(dirname(workspace), { recursive: true, mode: 0o700 });
        await fs.rename(temp, workspace);
      } finally { await fs.rm(temp, { recursive: true, force: true }); }
    }
    await fs.mkdir(this.dataPath(row.id), { recursive: true, mode: 0o700 });
    return workspace;
  }
  async remove(id: string): Promise<void> { await fs.rm(this.directory(id), { recursive: true, force: true }); }
}
