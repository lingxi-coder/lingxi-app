import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { open, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';
import { promisify } from 'node:util';
import { dialog, type BrowserWindow } from 'electron';
import type { LocalDataSourceAnswer, LocalDataSourceCommand, ManagedDataSource } from '../../../../../packages/teams/lingxi-data.js';
import type { Account, TeamsService } from './service.js';
import type { ReportCapture, ReportDraft } from '../../../../../packages/teams/reports.js';
import { readPrivateDataPage } from './dataPreview.js';
import { id, readDefinition } from './dataSourceDefinition.js';

const exec = promisify(execFile);
const childEnv = { PATH: '/usr/bin:/bin' };

function launcher(): { bin: string; endpoint: string; ca: string[] } {
  const { LINGXI_DATA_PREVIEW_BIN: bin, LINGXI_DATA_PREVIEW_ORIGIN: origin, LINGXI_DATA_PREVIEW_CA: ca } = process.env;
  if (process.platform !== 'darwin' || !bin || !isAbsolute(bin) || !origin || (ca && !isAbsolute(ca))) throw new Error('DATA_UNAVAILABLE');
  const url = new URL(origin);
  if (url.protocol !== 'https:' || url.href !== url.origin + '/') throw new Error('DATA_UNAVAILABLE');
  return { bin, endpoint: url.origin, ca: ca ? ['--ca', ca] : [] };
}

export class DesktopDataSources {
  private readonly processes = new Map<string, { uid: string; epoch: number; child: ChildProcess; authFile: string; attempt: string }>();
  private readonly stopAccount: () => void;
  private readonly refresh: NodeJS.Timeout;

  constructor(private readonly account: Account, private readonly teams: TeamsService, private readonly userData: string) {
    this.stopAccount = account.onChanged(() => {
      const next = teams.hostContext();
      for (const [source, value] of this.processes) if (value.uid !== next?.uid || value.epoch !== next?.epoch) void this.stop(source);
    });
    this.refresh = setInterval(() => { void this.renew(); }, 10 * 60_000);
    this.refresh.unref();
  }

  async execute(uid: string, request: LocalDataSourceCommand, parent: BrowserWindow): Promise<LocalDataSourceAnswer> {
    const context = this.teams.hostContext();
    if (!context || context.uid !== uid) throw new Error('INVALID_SESSION');
    const current = () => this.teams.assertHostContext(context);
    if (request.action === 'list') {
      const { sources } = await this.teams.manageSources<{ sources: ManagedDataSource[] }>(context, { action: 'dataSources' });
      return sources.map(source => ({ ...source, running: this.processes.has(uid + '/' + source.source) }));
    }
    if (request.action === 'import') {
      const selection = await dialog.showOpenDialog(parent, { properties: ['openFile'], filters: [{ name: 'Data host', extensions: ['json'] }] });
      current();
      if (selection.canceled || selection.filePaths.length !== 1) return null;
      const file = await open(selection.filePaths[0]!, 'r');
      let content: string;
      try {
        const metadata = await file.stat();
        if (!metadata.isFile() || metadata.size > 64 * 1024) throw new Error('INVALID_INPUT');
        const bytes = Buffer.alloc(64 * 1024 + 1);
        const { bytesRead } = await file.read(bytes, 0, bytes.length, 0);
        if (bytesRead === bytes.length) throw new Error('INVALID_INPUT');
        content = bytes.subarray(0, bytesRead).toString('utf8');
      } finally { await file.close(); }
      const draft = readDefinition(content, uid);
      const directory = await this.folder(uid);
      const staged = join(directory, randomUUID() + '.json');
      await writeFile(staged, content, { mode: 0o600, flag: 'wx' });
      try {
        await this.publicKey(staged);
        current();
        if (this.processes.has(uid + '/' + draft.source)) throw new Error('DATA_CONFLICT');
        await rename(staged, join(directory, draft.source + '.json'));
      } finally { await rm(staged, { force: true }); }
      return draft;
    }
    const source = request.action === 'configure' ? request.input?.source : request.source;
    if (typeof source !== 'string' || !id.test(source)) throw new Error('INVALID_INPUT');
    const processKey = uid + '/' + source;
    if (request.action === 'stop') { await this.stop(processKey); return { running: false }; }
    const directory = await this.folder(uid);
    const config = join(directory, source + '.json');
    const local = readDefinition(await readFile(config, 'utf8'), uid);
    if (local.source !== source) throw new Error('INVALID_INPUT');
    if (request.action === 'details') { current(); return local; }
    if (request.action === 'check') { await this.publicKey(config); current(); return { running: this.processes.has(processKey) }; }
    if (request.action === 'configure') {
      if (!Array.isArray(request.input.grants) || request.input.grants.some(grant => grant.preset !== 'main'
        || !Array.isArray(grant.fields) || grant.fields.some(field => !local.fields.includes(field)))) throw new Error('INVALID_INPUT');
      const key = await this.publicKey(config);
      current();
      try {
        await this.teams.manageSources(context, { ...request.input, key, kind: local.kind, action: 'configureDataSource',
          ...(request.input.scope.kind === 'personal' ? { fields: local.fields } : {}) });
      } finally {
        if (!request.input.enabled) await this.stop(processKey);
      }
      return { running: this.processes.has(processKey) };
    }
    if (request.action !== 'start' && request.action !== 'preview' && request.action !== 'captureReport') throw new Error('INVALID_INPUT');
    const { sources } = await this.teams.manageSources<{ sources: ManagedDataSource[] }>(context, { action: 'dataSources' });
    const configured = sources.find(row => row.source === source && row.enabled);
    if (!configured) throw new Error('DATA_ACCESS_REQUIRED');
    const { bin, endpoint, ca } = launcher();
    if (request.action === 'captureReport') {
      const { plan, scope, dataset } = request;
      if (plan.origin.kind !== 'source' || plan.origin.source !== source || plan.origin.version !== configured.version) throw new Error('DATA_CONFLICT');
      const { binding } = await this.teams.data<{ binding: string }>({ scope, command: { action: 'prepareReport', dataset, plan } }, uid);
      current();
      const attempt = await mkdtemp(join(tmpdir(), 'lingxi-report-'));
      try {
        const path = join(attempt, 'intent.json');
        await writeFile(path, JSON.stringify({ binding, spec: plan.spec }), { mode: 0o600, flag: 'wx' });
        const { stdout } = await exec(bin, ['capture-report', '--config', config, '--request', path],
          { cwd: directory, env: childEnv, maxBuffer: 3 * 1024 * 1024, timeout: 45_000 });
        const capture: ReportCapture = JSON.parse(stdout);
        const bytes = Buffer.from(capture.payload, 'base64url');
        const body: Pick<ReportDraft, 'facts' | 'binding'> = JSON.parse(bytes.toString('utf8'));
        if (body.binding !== binding || !Array.isArray(body.facts)) throw new Error('INVALID_INPUT');
        current();
        return { binding, capture, facts: body.facts, confirmation: createHash('sha256').update(bytes).digest('hex') };
      } finally { await rm(attempt, { recursive: true, force: true }); }
    }
    const token = await this.account.getIdToken();
    current();
    if (request.action === 'preview') {
      if (configured.scope.kind !== 'personal') throw new Error('DATA_ACCESS_REQUIRED');
      const page = await readPrivateDataPage(source, undefined, token, directory, AbortSignal.timeout(45_000));
      current();
      return { rows: page.rows };
    }
    if (this.processes.has(processKey)) return { running: true };
    const attempt = await mkdtemp(join(tmpdir(), 'lingxi-data-host-'));
    const authFile = join(attempt, 'identity');
    try {
      await writeFile(authFile, token, { mode: 0o600, flag: 'wx' });
      if (this.processes.has(processKey)) { await rm(attempt, { recursive: true, force: true }); return { running: true }; }
      current();
    } catch (error) { await rm(attempt, { recursive: true, force: true }); throw error; }
    const child = spawn(bin, ['host', '--endpoint', endpoint, '--auth-file', authFile, '--config', config, ...ca],
      { cwd: directory, env: childEnv, stdio: 'ignore' });
    this.processes.set(processKey, { uid, epoch: context.epoch, child, authFile, attempt });
    const disposed = () => {
      if (this.processes.get(processKey)?.child === child) this.processes.delete(processKey);
      void rm(attempt, { recursive: true, force: true }).catch(() => undefined);
    };
    child.once('error', disposed);
    child.once('exit', disposed);
    await wait(500);
    current();
    if (this.processes.get(processKey)?.child !== child) throw new Error('DATA_UNAVAILABLE');
    return { running: true };
  }

  close(): void {
    this.stopAccount();
    clearInterval(this.refresh);
    for (const source of this.processes.keys()) void this.stop(source);
  }

  private async folder(uid: string): Promise<string> {
    const path = join(this.userData, 'data-sources', createHash('sha256').update(uid).digest('hex'));
    await mkdir(path, { recursive: true, mode: 0o700 });
    return path;
  }

  private async publicKey(config: string): Promise<string> {
    try {
      const { bin } = launcher();
      const { stdout } = await exec(bin, ['host-key', '--config', config], { env: childEnv, timeout: 15_000, maxBuffer: 1024 });
      const key = stdout.trim();
      if (/^(?:[A-Za-z0-9_-]{44}|[A-Za-z0-9_-]{87})$/u.test(key)) return key;
    } catch { /* Contains only a local setup error. */ }
    throw new Error('DATA_UNAVAILABLE');
  }

  private async stop(processKey: string): Promise<void> {
    const value = this.processes.get(processKey);
    if (!value) return;
    this.processes.delete(processKey);
    value.child.kill();
    await rm(value.attempt, { recursive: true, force: true }).catch(() => undefined);
  }

  private async renew(): Promise<void> {
    const context = this.teams.hostContext();
    if (!context || !this.processes.size) return;
    try {
      const token = await this.account.getIdToken();
      this.teams.assertHostContext(context);
      for (const { uid, authFile } of this.processes.values()) {
        if (uid !== context.uid) continue;
        const staged = authFile + '.' + randomUUID();
        try {
          await writeFile(staged, token, { mode: 0o600, flag: 'wx' });
          this.teams.assertHostContext(context);
          if ([...this.processes.values()].some(value => value.authFile === authFile)) await rename(staged, authFile);
        } finally { await rm(staged, { force: true }); }
      }
    } catch { for (const source of this.processes.keys()) await this.stop(source); }
  }
}
