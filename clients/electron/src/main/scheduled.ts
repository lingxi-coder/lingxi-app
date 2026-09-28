import { readFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, writeFile, rename, lstat } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';
import type { CronJobDto, CronRequestDto, CronAutomationDto, CronRunDto } from '@lingxi/bridge-client';
import type { ScheduledScope } from '../shared/scheduled.js';
import type { SettingsStore } from './settings.js';
import type { SessionRuntime } from './bridge.js';
import { isSessionId } from './sessionIdentity.js';
import type { SessionRuntimeManager } from './sessionRuntimeManager.js';
import type { ProjectSessionCatalog } from './session-catalog.js';
import type { SessionRef } from '../shared/settings.js';

/** Catalog discovery only: Rust remains the sole authority for due-time claims. */
export class ScheduledTaskService {
  private readonly controllers = new Map<string, { ref: SessionRef; release: () => void }>();
  private readonly sessionQueues = new Map<string, Promise<unknown>>();
  private timer?: ReturnType<typeof setInterval>;
  private syncing?: Promise<void>;
  private disposed = false;
  private notificationWrites: Promise<void> = Promise.resolve();
  private delivered?: Set<string>;
  private retention?: Promise<void>;
  private readonly scopeCache = new Map<string, { label: string; jobs: CronJobDto[] }>();
  private cacheWrites: Promise<void> = Promise.resolve();

  constructor(
    private readonly settings: SettingsStore,
    private readonly bridge: SessionRuntimeManager,
    private readonly catalog: ProjectSessionCatalog,
    /** Returns whether the notification was actually delivered — a refusal
     * releases the durable dedupe key so the run can be announced later. */
    private readonly notify: (title: string, body: string, ref?: SessionRef) => boolean,
    private readonly report: (error: unknown) => void,
  ) {
    if (typeof settings.settingsPath === 'string') {
      try {
        const rows: unknown = JSON.parse(readFileSync(join(dirname(settings.settingsPath), 'scheduled-scopes.json'), 'utf8'));
        if (Array.isArray(rows)) for (const row of rows) {
          if (row && typeof row.path === 'string' && typeof row.label === 'string' && Array.isArray(row.jobs)) this.scopeCache.set(row.path, { label: row.label, jobs: row.jobs });
        }
      } catch { /* Empty catalog on first launch. */ }
    }
  }

  scopes(): ScheduledScope[] {
    return [{ id: 'global', label: 'No project' }, ...this.settings.getPublic().projects.map((path) => ({ id: path, projectPath: path, label: basename(path) })), ...[...this.scopeCache].filter(([path, value]) => path !== this.settings.scheduledWorkspace && !this.settings.hasProject(path) && value.jobs.length > 0).map(([path, value]) => ({ id: path, projectPath: path, label: `${value.label} (unavailable)`, unavailable: true }))];
  }

  resolveScope(id: string): string {
    if (id === 'global') return this.settings.scheduledWorkspace;
    if (!this.settings.hasProject(id)) throw new Error('Project is unavailable. Select an existing project.');
    return id;
  }

  start(): void {
    void this.sync().catch(this.report);
    this.timer = setInterval(() => void this.sync().catch(this.report), 30_000);
    this.timer.unref();
  }

  private controllerRef(path: string): SessionRef {
    const hash = createHash('sha256').update(`lingxi-scheduled-controller:${path}`).digest('hex');
    return { projectPath: path, sessionId: `${hash.slice(0, 8)}-${hash.slice(8, 12)}-4${hash.slice(13, 16)}-a${hash.slice(17, 20)}-${hash.slice(20, 32)}` };
  }

  isControllerSession(path: string, sessionId: string): boolean {
    return this.controllerRef(path).sessionId === sessionId;
  }

  private async controller(path: string): Promise<SessionRuntime> {
    if (this.disposed) throw new Error('Scheduled task service is closed');
    if (path === this.settings.scheduledWorkspace) {
      await mkdir(path, { recursive: true, mode: 0o700 });
      if ((await lstat(path)).isSymbolicLink()) throw new Error('Managed scheduled workspace must not be a symlink.');
    }
    const ref = this.controllers.get(path)?.ref ?? this.controllerRef(path);
    let entry = this.controllers.get(path);
    if (!entry) { entry = { ref, release: this.bridge.retainBackgroundSession(ref) }; this.controllers.set(path, entry); }
    try { return await this.bridge.ensure(ref, true, this.settings.getPublic().model); }
    catch (error) {
      // Release the lease THIS call created, and only while it is still the
      // registered one. Two `controller()` calls can be in flight (the 30s
      // `sync` tick beside an IPC `manage`); reading the map again here let a
      // late failure release a newer, successful call's lease, leaving that
      // controller runtime evictable while it was still being driven.
      if (this.controllers.get(path) === entry) {
        entry.release();
        this.controllers.delete(path);
      }
      throw error;
    }
  }

  sync(): Promise<void> {
    if (this.syncing) return this.syncing;
    const work = async () => {
      const paths = new Set(this.scopes().filter((scope) => !scope.unavailable).map((scope) => this.resolveScope(scope.id)));
      for (const [path, controller] of this.controllers) {
        if (!paths.has(path)) { controller.release(); this.controllers.delete(path); }
      }
      await Promise.allSettled([...paths].map(async (path) => {
        try {
          const document = JSON.parse(await readFile(join(path, '.lingxi', 'scheduled_tasks.json'), 'utf8')) as { tasks?: { automation?: { status?: string } }[] };
          if (document.tasks?.some((task) => !task.automation || task.automation.status === 'active')) { const runtime = await this.controller(path); await this.cacheJobs(path, await runtime.manageCron({ action: 'list' })); }
          else { this.controllers.get(path)?.release(); this.controllers.delete(path); }
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== 'ENOENT') this.report(error);
        }
      }));
      await this.enforceHistoryLimit();
    };
    this.syncing = work().finally(() => { this.syncing = undefined; });
    return this.syncing;
  }

  async context(scopeId: string) {
    const path = this.resolveScope(scopeId);
    const runtime = await this.controller(path);
    const [models, catalog] = await Promise.all([runtime.scheduledModelCatalog(), this.catalog.list(path)]);
    return { models: models.details ?? [], currentModel: models.current, sessions: catalog.sessions.filter((session) => !this.isControllerSession(path, session.uuid) && !this.settings.isSessionArchived({ projectPath: path, sessionId: session.uuid })).map((session) => ({ uuid: session.uuid, title: session.title })) };
  }

  async manage(scopeId: string, request: CronRequestDto): Promise<CronJobDto[]> {
    if (scopeId !== 'global' && !this.settings.hasProject(scopeId)) {
      const cached = this.scopeCache.get(scopeId);
      if (cached && (request.action === 'list' || request.action === 'history')) return cached.jobs.map((job) => ({ ...job, ...(job.automation && job.automation.status !== 'completed' ? { automation: { ...job.automation, status: 'paused' as const, statusReason: 'Project is unavailable. Add it again to manage this task.' } } : {}) }));
      throw new Error('Project is unavailable. Add it again to manage this task.');
    }
    const path = this.resolveScope(scopeId);
    if ((request.action === 'create' || request.action === 'update') && request.automation?.runMode === 'selected_session') await this.validateTarget(path, request.automation.targetSessionId);
    const runtime = await this.controller(path);
    const jobs = await runtime.manageCron(request);
    await this.cacheJobs(path, jobs);
    if (!jobs.some((job) => !job.automation || job.automation.status === 'active')) {
      this.controllers.get(path)?.release(); this.controllers.delete(path);
    }
    return jobs;
  }

  private cacheJobs(path: string, jobs: CronJobDto[]): Promise<void> {
    this.scopeCache.set(path, { label: basename(path), jobs });
    if (typeof this.settings.settingsPath !== 'string') return Promise.resolve();
    const operation = this.cacheWrites.then(async () => {
      const file = join(dirname(this.settings.settingsPath), 'scheduled-scopes.json');
      await mkdir(dirname(file), { recursive: true, mode: 0o700 });
      const temporary = `${file}.${randomUUID()}.tmp`;
      await writeFile(temporary, JSON.stringify([...this.scopeCache].map(([path, value]) => ({ path, ...value }))), { mode: 0o600 });
      await rename(temporary, file);
    });
    this.cacheWrites = operation.catch(this.report);
    return this.cacheWrites;
  }

  private enforceHistoryLimit(): Promise<void> {
    if (this.retention) return this.retention;
    const work = async () => {
      const scopes = await Promise.all(this.scopes().filter((scope) => !scope.unavailable).map(async (scope) => {
        const path = this.resolveScope(scope.id);
        try {
          const doc = JSON.parse(await readFile(join(path, '.lingxi', 'scheduled_tasks.json'), 'utf8')) as { tasks?: { id: string; automation?: CronAutomationDto }[] };
          return (doc.tasks ?? []).flatMap((task) => (task.automation?.runs ?? []).filter((run) => ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(run.status)).map((run) => ({ path, task, run })));
        } catch { return []; }
      }));
      const obsolete = scopes.flat().sort((a, b) => (b.run.finishedAt ?? b.run.scheduledAt) - (a.run.finishedAt ?? a.run.scheduledAt)).slice(500);
      const removals = new Map<string, { path: string; id: string; config: CronAutomationDto; runs: CronRunDto[] }>();
      for (const entry of obsolete) {
        const key = `${entry.path}:${entry.task.id}`;
        const item = removals.get(key) ?? { path: entry.path, id: entry.task.id, config: entry.task.automation!, runs: [] };
        item.runs.push(entry.run); removals.set(key, item);
      }
      for (const item of removals.values()) {
        const runtime = await this.controller(item.path);
        await this.cacheJobs(item.path, await runtime.manageCron({ action: 'prune_history', id: item.id, automation: { ...item.config, runs: item.runs } }));
      }
    };
    this.retention = work().finally(() => { this.retention = undefined; });
    return this.retention;
  }

  private async validateTarget(path: string, sessionId?: string): Promise<void> {
    if (!isSessionId(sessionId)) throw new Error('paused: Select a valid target chat.');
    if (this.settings.isSessionArchived({ projectPath: path, sessionId })) throw new Error('paused: The target chat was archived. Select another chat.');
    const catalog = await this.catalog.list(path);
    if (!catalog.sessions.some((session) => session.uuid === sessionId)) throw new Error('paused: The target chat is unavailable. Select another chat.');
  }

  private async notifyOnce(key: string, title: string, body: string, ref?: SessionRef): Promise<void> {
    const operation = this.notificationWrites.then(async () => {
      const file = typeof this.settings.settingsPath === 'string' ? join(dirname(this.settings.settingsPath), 'scheduled-notifications.json') : undefined;
      if (!this.delivered) {
        try { this.delivered = new Set<string>(file ? JSON.parse(await readFile(file, 'utf8')) : []); }
        catch { this.delivered = new Set(); }
      }
      if (this.delivered.has(key)) return;
      const persist = async () => {
        if (!file) return;
        await mkdir(dirname(file), { recursive: true, mode: 0o700 });
        const temporary = `${file}.${randomUUID()}.tmp`;
        await writeFile(temporary, JSON.stringify([...this.delivered!]), { mode: 0o600 });
        await rename(temporary, file);
      };
      this.delivered.add(key);
      this.delivered = new Set([...this.delivered].slice(-500));
      // Reserve before delivering; a process restart must not notify twice.
      await persist();
      // …but a refused delivery (notifications disabled, the notifier already
      // disposed after `before-quit`, or the kind switched off) must give the
      // key back, or re-enabling the toggle can never recover this run and a
      // restart cannot either.
      if (!this.notify(title, body.replace(/^(paused|cancelled|interrupted):\s*/, ''), ref)) {
        this.delivered.delete(key);
        await persist();
      }
    });
    this.notificationWrites = operation.catch(this.report);
    await this.notificationWrites;
  }

  async run(source: SessionRuntime, runId: string, task: CronJobDto): Promise<{ sessionId: string; summary: string }> {
    try {
      const result = await this.execute(source, runId, task);
      if (task.automation?.notificationPolicy === 'all') await this.notifyOnce(`${source.projectPath}:${runId}`, task.prompt.split('\n')[0].replace(/^# /, ''), result.summary.slice(0, 240) || 'Scheduled task completed.', { projectPath: source.projectPath, sessionId: result.sessionId });
      return result;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (task.automation?.notificationPolicy !== 'none' && !message.startsWith('busy:') && !message.startsWith('cancelled:')) await this.notifyOnce(`${source.projectPath}:${runId}`, 'Scheduled task needs attention', message);
      throw error;
    }
  }

  private async execute(source: SessionRuntime, runId: string, task: CronJobDto): Promise<{ sessionId: string; summary: string }> {
    const config = task.automation;
    if (!config) throw new Error('paused: Scheduled task configuration is missing.');
    const path = source.projectPath;
    if (!this.settings.isTrustedWorkspace(path)) throw new Error('paused: The project is unavailable.');
    const existing = config.runMode === 'selected_session' ? config.targetSessionId : config.runMode === 'task_session' ? config.ownedSessionId : undefined;
    if (existing) await this.validateTarget(path, existing);
    else if (config.runMode === 'selected_session') throw new Error('paused: Select a target chat.');
    const ref = { projectPath: path, sessionId: existing ?? randomUUID() };
    const previous = this.sessionQueues.get(ref.sessionId) ?? Promise.resolve();
    const operation = previous.catch(() => undefined).then(async () => {
      // Revalidate after waiting: projects and targets may disappear while queued.
      if (!this.settings.isTrustedWorkspace(path)) throw new Error('paused: The project is unavailable.');
      if (existing) await this.validateTarget(path, existing);
      return this.bridge.withBackgroundSession(ref, !existing, config.model, async (runtime) => {
        const summary = await runtime.runScheduledTurn(runId, task, () => source.markCronRunStarted(runId, ref.sessionId));
        return { sessionId: ref.sessionId, summary };
      });
    });
    this.sessionQueues.set(ref.sessionId, operation);
    try {
      const result = await operation;
      return result;
    } catch (error) {
      throw error;
    } finally {
      if (this.sessionQueues.get(ref.sessionId) === operation) this.sessionQueues.delete(ref.sessionId);
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer) clearInterval(this.timer);
    for (const controller of this.controllers.values()) controller.release();
    this.controllers.clear();
  }
}
