import { readFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, writeFile, rename, lstat } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join } from 'node:path';
import type { CronJobDto, CronRequestDto, CronAutomationDto, CronRunDto } from '@lingxi/bridge-client';
import type { ScheduledScope } from '../shared/scheduled.js';
import type { SettingsStore } from './settings.js';
import type { SessionRuntime } from './bridge.js';
import { isSessionId } from './sessionIdentity.js';
import { createScheduledRunWorkspace, scheduledWorkspaceInstructions } from './scheduled-workspaces.js';
import { scheduledRunIdentity } from './scheduled-run-identity.js';
import type { SessionRuntimeManager } from './sessionRuntimeManager.js';
import type { KeepAwake } from './bots/keepAwake.js';
import type { ProjectSessionCatalog, ProjectSessionCatalogRow } from './session-catalog.js';
import type { SessionRef } from '../shared/settings.js';
import type { ConversationSpace } from '../shared/conversationSpaces.js';

type ScheduledRunResult = { sessionId: string; summary: string };

/** What of a session catalog the scheduler reads: the owner's, or the one a Bot keeps in its own home. */
export interface RoutineCatalog {
  list(projectPath: string, scope?: 'current' | 'repository' | 'all',
    include?: (row: ProjectSessionCatalogRow) => boolean): Promise<{ sessions: ProjectSessionCatalogRow[] }>;
  find(projectPath: string, sessionId: string): Promise<ProjectSessionCatalogRow | undefined>;
}

/** What the scheduler needs of one Bot to run its routines. */
export interface RoutineBot {
  botId: string;
  name: string;
  /** The Bot's working directory: the scope its routines live in. */
  workspace: string;
  /** The Bot's own model, if it has one. */
  model?: string;
  /** The catalog of the Bot's home, where its chats are; they are not in the owner's. */
  catalog(): RoutineCatalog;
}

/** What the scheduler asks of the Bots. */
export interface RoutineBots {
  /** The Bots whose routines run now: none where Bots cannot run, and not one that is being deleted. */
  routineBots(): RoutineBot[];
  /** The Bot whose workspace a working directory is, while its routines can run. */
  routineBotFor(workspace: string): RoutineBot | undefined;
  /** Whether a working directory is shaped like a Bot's workspace, even if the Bot is gone or being deleted. */
  isBotWorkspace(workspace: string): boolean;
}

const MAX_RECENT_SCHEDULED_RUNS = 500;
/** How long before a Bot's routine is due the computer starts being kept awake, so that it does not drift into sleep just before the routine runs. */
export const DUE_SOON_MS = 10 * 60_000;

export interface ScheduledTaskServiceOptions {
  /** Kept awake while a Bot's routine runs, and for a while before one is due. */
  keepAwake?: KeepAwake;
  now?: () => number;
}

/** Catalog file the Rust scheduler writes for one scope's workspace. */
const catalogPath = (workspace: string): string => join(workspace, '.lingxi', 'scheduled_tasks.json');

/** Catalog discovery only: Rust remains the sole authority for due-time claims. */
export class ScheduledTaskService {
  private readonly controllers = new Map<string, { ref: SessionRef; release: () => void }>();
  private readonly sessionQueues = new Map<string, Promise<unknown>>();
  private readonly pendingRuns = new Map<string, Promise<ScheduledRunResult>>();
  private readonly recentRuns = new Map<string, Promise<ScheduledRunResult>>();
  private timer?: ReturnType<typeof setInterval>;
  private syncing?: Promise<void>;
  private disposed = false;
  private notificationWrites: Promise<void> = Promise.resolve();
  private delivered?: Set<string>;
  private retention?: Promise<void>;
  private readonly scopeCache = new Map<string, { label: string; jobs: CronJobDto[] }>();
  private cacheWrites: Promise<void> = Promise.resolve();
  private bots?: RoutineBots;
  /** Held while a routine of a Bot is due within `DUE_SOON_MS`. */
  private releaseDue?: () => void;
  private readonly now: () => number;

  constructor(
    private readonly settings: SettingsStore,
    private readonly bridge: SessionRuntimeManager,
    private readonly catalog: ProjectSessionCatalog,
    /** Returns whether the notification was actually delivered — a refusal
     * releases the durable dedupe key so the run can be announced later. */
    private readonly notify: (title: string, body: string, ref?: SessionRef) => boolean,
    private readonly report: (error: unknown) => void,
    private readonly options: ScheduledTaskServiceOptions = {},
  ) {
    this.now = options.now ?? Date.now;
    if (typeof settings.settingsPath === 'string') {
      try {
        const rows: unknown = JSON.parse(readFileSync(join(dirname(settings.settingsPath), 'scheduled-scopes.json'), 'utf8'));
        if (Array.isArray(rows)) for (const row of rows) {
          if (row && typeof row.path === 'string' && typeof row.label === 'string' && Array.isArray(row.jobs)) this.scopeCache.set(row.path, { label: row.label, jobs: row.jobs });
        }
      } catch { /* Empty catalog on first launch. */ }
    }
  }

  /** Bots are known once the app has made them; until then only the owner's projects have routines. */
  attachBots(bots: RoutineBots): void {
    this.bots = bots;
  }

  scopes(): ScheduledScope[] {
    // A Bot's workspace is a scope of its own, named for the Bot. It is not a project, so it has no `projectPath`.
    const bots = (this.bots?.routineBots() ?? []).map((bot): ScheduledScope => ({ id: bot.workspace, label: bot.name, botId: bot.botId }));
    return [{ id: 'global', label: 'No project' }, ...this.settings.getPublic().projects.map((path) => ({ id: path, projectPath: path, label: basename(path) })), ...bots, ...[...this.scopeCache].filter(([path, value]) => path !== this.settings.scheduledWorkspace && !this.settings.hasProject(path) && !this.bots?.isBotWorkspace(path) && value.jobs.length > 0).map(([path, value]) => ({ id: path, projectPath: path, label: `${value.label} (unavailable)`, unavailable: true }))];
  }

  resolveScope(id: string): string {
    if (id === 'global') return this.settings.scheduledWorkspace;
    if (this.settings.hasProject(id)) return id;
    const bot = this.bots?.routineBotFor(id);
    if (bot) return bot.workspace;
    throw new Error(this.bots?.isBotWorkspace(id) ? 'This Bot is unavailable.' : 'Project is unavailable. Select an existing project.');
  }

  resolveSession(scopeId: string, sessionId: string): SessionRef {
    return this.targetRef(this.resolveScope(scopeId), sessionId);
  }

  private targetRef(scopePath: string, sessionId: string): SessionRef {
    if (this.bots?.routineBotFor(scopePath)) return { projectPath: scopePath, sessionId };
    const owner = this.settings.conversationSpaces.findBySessionId(sessionId);
    if (owner) {
      const projectId = scopePath === this.settings.scheduledWorkspace ? null : scopePath;
      if (owner.projectId !== projectId) throw new Error('paused: The target chat belongs to another scope.');
      return { projectPath: owner.projectPath, sessionId: owner.sessionId, mode: owner.mode };
    }
    const managed = scopePath === this.settings.scheduledWorkspace
      ? this.settings.scheduledSessionWorkspace(sessionId) : undefined;
    return { projectPath: managed?.projectPath ?? scopePath, sessionId };
  }

  start(): void {
    void this.sync().catch(this.report);
    this.timer = setInterval(() => void this.sync().catch(this.report), 30_000);
    this.timer.unref();
  }

  /** The computer woke up, and the tick stood still while it slept: look after the routines now, not at the next tick. Nothing happens before `start`. */
  wake(): void {
    if (this.timer === undefined || this.disposed) return;
    void this.sync().catch(this.report);
  }

  private controllerRef(path: string): SessionRef {
    const hash = createHash('sha256').update(`lingxi-scheduled-controller:${path}`).digest('hex');
    return { projectPath: path, mode: 'code', sessionId: `${hash.slice(0, 8)}-${hash.slice(8, 12)}-4${hash.slice(13, 16)}-a${hash.slice(17, 20)}-${hash.slice(20, 32)}` };
  }

  isControllerSession(path: string, sessionId: string): boolean {
    return this.controllerRef(path).sessionId === sessionId;
  }

  private async controller(path: string): Promise<SessionRuntime> {
    if (this.disposed) throw new Error('Scheduled task service is closed');
    const bot = this.bots?.routineBotFor(path);
    // No lease for a Bot that is going away: it would outlive the Bot's folder.
    if (!bot && this.bots?.isBotWorkspace(path)) throw new Error('This Bot is unavailable.');
    if (path === this.settings.scheduledWorkspace) {
      await mkdir(path, { recursive: true, mode: 0o700 });
      if ((await lstat(path)).isSymbolicLink()) throw new Error('Managed scheduled workspace must not be a symlink.');
    }
    const ref = this.controllers.get(path)?.ref ?? this.controllerRef(path);
    let entry = this.controllers.get(path);
    if (!entry) { entry = { ref, release: this.bridge.retainBackgroundSession(ref) }; this.controllers.set(path, entry); }
    // A Bot's controller starts on the Bot's model; the owner's would win over it.
    try { return await this.bridge.ensure(ref, true, bot ? bot.model : this.settings.getPublic().model, undefined, 'code'); }
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
      // When the active routines of the Bots run next.
      const botRuns: number[] = [];
      await Promise.allSettled([...paths].map(async (path) => {
        try {
          const document = JSON.parse(await readFile(catalogPath(path), 'utf8')) as { tasks?: { automation?: { status?: string } }[] };
          if (document.tasks?.some((task) => !task.automation || task.automation.status === 'active')) {
            const runtime = await this.controller(path);
            const jobs = await runtime.manageCron({ action: 'list' });
            await this.cacheJobs(path, jobs);
            if (this.bots?.routineBotFor(path)) botRuns.push(...jobs.flatMap((job) => job.next_run_at !== undefined && (!job.automation || job.automation.status === 'active') ? [job.next_run_at] : []));
          }
          else { this.controllers.get(path)?.release(); this.controllers.delete(path); }
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== 'ENOENT') this.report(error);
        }
      }));
      this.keepAwakeForDue(botRuns);
      await this.enforceHistoryLimit();
    };
    this.syncing = work().finally(() => { this.syncing = undefined; });
    return this.syncing;
  }

  /**
   * The computer is kept awake while a routine of a Bot is due within
   * `DUE_SOON_MS`; from the moment it runs, the run itself holds it. One that is
   * overdue counts too: it is about to be claimed.
   */
  private keepAwakeForDue(runs: readonly number[]): void {
    const due = !this.disposed && runs.some((at) => at <= this.now() + DUE_SOON_MS);
    if (due && this.releaseDue === undefined) this.releaseDue = this.options.keepAwake?.hold();
    else if (!due && this.releaseDue !== undefined) {
      this.releaseDue();
      this.releaseDue = undefined;
    }
  }

  /** The chats of a scope are in the Bot's home for a Bot's workspace, and in the owner's otherwise. */
  private catalogFor(path: string): RoutineCatalog {
    return this.bots?.routineBotFor(path)?.catalog() ?? this.catalog;
  }

  async context(scopeId: string, mode: ConversationSpace) {
    const path = this.resolveScope(scopeId);
    const runtime = await this.controller(path);
    const isBot = Boolean(this.bots?.routineBotFor(path));
    const projectId = scopeId === 'global' ? null : scopeId;
    const belongs = (row: ProjectSessionCatalogRow) => {
      if (row.mode !== mode || this.isControllerSession(path, row.uuid)) return false;
      const owner = !isBot && this.settings.conversationSpaces.findBySessionId(row.uuid);
      return (!owner || (owner.mode === mode && owner.projectId === projectId))
        && !this.settings.isSessionArchived(owner || { projectPath: path, sessionId: row.uuid });
    };
    const owned = isBot ? [] : this.settings.conversationSpaces.snapshot().sessions
      .filter(owner => owner.mode === mode && owner.projectId === projectId).slice().reverse();
    const refs = [...owned, ...(scopeId === 'global' ? this.settings.scheduledSessionRefs().slice().reverse() : [])];
    const seen = new Set<string>();
    const unique = refs.filter(ref => !seen.has(ref.sessionId) && Boolean(seen.add(ref.sessionId)));
    const loadOwned = async () => {
      const rows: ProjectSessionCatalogRow[] = [];
      for (let offset = 0; offset < unique.length && rows.length < 200; offset += 8) {
        const batch = await Promise.all(unique.slice(offset, offset + 8).map(async ref => {
          if (this.settings.isSessionArchived(ref)) return undefined;
          try {
            const row = await this.catalog.find(ref.projectPath, ref.sessionId);
            return row && belongs(row) ? row : undefined;
          } catch (error) { this.report(error); return undefined; }
        }));
        rows.push(...batch.filter((row): row is ProjectSessionCatalogRow => Boolean(row)));
      }
      return rows;
    };
    const [models, catalog, ownedRows] = await Promise.all([
      runtime.scheduledModelCatalog(), this.catalogFor(path).list(path, 'current', belongs), loadOwned(),
    ]);
    const listed = new Set<string>();
    const sessions = [...ownedRows, ...catalog.sessions.filter(belongs)]
      .filter(row => !listed.has(row.uuid) && Boolean(listed.add(row.uuid))).slice(0, 200);
    return { models: models.details ?? [], currentModel: models.current,
      sessions: sessions.map(session => ({ uuid: session.uuid, title: session.title })) };
  }

  async manage(scopeId: string, request: CronRequestDto, mode: ConversationSpace = this.settings.conversationSpaces.snapshot().activeMode): Promise<CronJobDto[]> {
    if (scopeId !== 'global' && !this.settings.hasProject(scopeId) && !this.bots?.routineBotFor(scopeId)) {
      // A Bot that is gone took its routines with it; nothing is kept to show.
      if (this.bots?.isBotWorkspace(scopeId)) throw new Error('This Bot is unavailable.');
      const cached = this.scopeCache.get(scopeId);
      if (cached && (request.action === 'list' || request.action === 'history')) return cached.jobs.map((job) => ({ ...job, ...(job.automation && job.automation.status !== 'completed' ? { automation: { ...job.automation, status: 'paused' as const, statusReason: 'Project is unavailable. Add it again to manage this task.' } } : {}) }));
      throw new Error('Project is unavailable. Add it again to manage this task.');
    }
    const path = this.resolveScope(scopeId);
    if ((request.action === 'create' || request.action === 'update') && request.automation) {
      // The Global task list spans both spaces. Editing its title or schedule
      // must preserve the task's captured mode, even from the other space.
      const saved = request.action === 'update'
        ? (await (await this.controller(path)).manageCron({ action: 'list' })).find(job => job.id === request.id)?.automation
        : undefined;
      if (request.action === 'update' && !saved) throw new Error('Scheduled task is unavailable.');
      const sameTarget = saved?.runMode === request.automation.runMode
        && (saved.runMode !== 'selected_session' || saved.targetSessionId === request.automation.targetSessionId);
      const capturedMode = sameTarget ? (saved.runMode === 'selected_session' ? saved.targetSessionMode : saved.sessionMode) : mode;
      if (capturedMode !== 'chat' && capturedMode !== 'code') throw new Error('The scheduled conversation mode is unavailable.');
      if (request.automation.runMode === 'selected_session') {
        const target = await this.validateTarget(path, request.automation.targetSessionId, capturedMode);
        if (sameTarget && (saved.targetSessionTranscriptPath !== target.path || saved.targetSessionScopeKey !== scopeId)) {
          throw new Error('The target chat source changed. Select the chat again.');
        }
        request = { ...request, automation: { ...request.automation,
          targetSessionTranscriptPath: target.path, targetSessionMode: target.mode, targetSessionScopeKey: scopeId } };
      } else {
        request = { ...request, automation: { ...request.automation, sessionMode: capturedMode, sessionScopeKey: scopeId } };
      }
    }
    // A read must not boot an engine to learn that the scope has nothing to
    // read. Every project scope would otherwise start a sidecar (~3-4s, and
    // they contend) before the Scheduled panel could paint, including the
    // projects that never had a task.
    if ((request.action === 'list' || request.action === 'history') && !this.controllers.has(path) && (await this.persistedTaskCount(path)) === 0) {
      // The persisted catalog is the whole answer here, so retire any cached
      // jobs from before it emptied: `scopes()` advertises cached jobs for
      // projects that are gone, and a stale entry would keep offering tasks
      // that no longer exist once this project is removed.
      if (this.scopeCache.get(path)?.jobs.length) await this.cacheJobs(path, []);
      return [];
    }
    const runtime = await this.controller(path);
    const jobs = await runtime.manageCron(request);
    await this.cacheJobs(path, jobs);
    if (!jobs.some((job) => !job.automation || job.automation.status === 'active')) {
      this.controllers.get(path)?.release(); this.controllers.delete(path);
    }
    return jobs;
  }

  /** Persisted task count for a scope; `undefined` when the catalog is
   * unreadable, so a damaged file still reports through the engine path. */
  private async persistedTaskCount(path: string): Promise<number | undefined> {
    try {
      const document = JSON.parse(await readFile(catalogPath(path), 'utf8')) as { tasks?: unknown };
      // Only `{ "tasks": [] }` is a definitive empty scope. A body with no
      // `tasks` array is corrupt state that the engine reports as an error
      // ("expected an object with a `tasks` array") and must never be
      // reinterpreted as "no tasks" here.
      return Array.isArray(document.tasks) ? document.tasks.length : undefined;
    } catch (error) {
      return (error as NodeJS.ErrnoException).code === 'ENOENT' ? 0 : undefined;
    }
  }

  private cacheJobs(path: string, jobs: CronJobDto[]): Promise<void> {
    // The cache keeps the tasks of projects that were removed. A Bot's tasks
    // are not kept: they go with the Bot, and a run still finishing after the
    // Bot was deleted must not bring them back as an unavailable project.
    if (this.bots?.isBotWorkspace(path)) return Promise.resolve();
    this.scopeCache.set(path, { label: basename(path), jobs });
    return this.persistCache();
  }

  /**
   * Stop looking after a scope: let go of its controller and forget the tasks
   * cached for it. A Bot's scope goes this way before the Bot is deleted, so
   * that no lease keeps an engine on the folder that is about to move.
   */
  releaseScope(path: string): void {
    const controller = this.controllers.get(path);
    if (controller) { controller.release(); this.controllers.delete(path); }
    if (this.scopeCache.delete(path)) void this.persistCache();
  }

  private persistCache(): Promise<void> {
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
          const doc = JSON.parse(await readFile(catalogPath(path), 'utf8')) as { tasks?: { id: string; automation?: CronAutomationDto }[] };
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

  private async validateTarget(scopePath: string, sessionId?: string, mode?: ConversationSpace): Promise<{ path: string; mode: ConversationSpace; ref: SessionRef }> {
    if (!isSessionId(sessionId)) throw new Error('paused: Select a valid target chat.');
    const ref = this.targetRef(scopePath, sessionId);
    if (mode && ref.mode && ref.mode !== mode) throw new Error('paused: The target chat belongs to another conversation space.');
    if (this.settings.isSessionArchived(ref)) throw new Error('paused: The target chat was archived. Select another chat.');
    const live = this.bridge.get(sessionId);
    const source = live ? await (async () => {
      if (live.projectPath !== ref.projectPath || !live.sessionMode) throw new Error('paused: The target chat owner is unavailable.');
      const state = await live.getWorkspaceState();
      if (this.bridge.get(sessionId) !== live || !state.transcript_path || !isAbsolute(state.transcript_path)
        || basename(state.transcript_path).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`) {
        throw new Error('paused: The target chat source is unavailable.');
      }
      return { mode: live.sessionMode, path: state.transcript_path };
    })() : await this.bridge.requireSavedSessionSource(ref);
    const current = this.targetRef(scopePath, sessionId);
    if (current.projectPath !== ref.projectPath || current.mode !== ref.mode || (mode && source.mode !== mode)
      || (ref.mode && source.mode !== ref.mode)) throw new Error('paused: The target chat owner changed. Select the chat again.');
    return { ...source, ref: { ...ref, mode: source.mode } };
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

  run(source: SessionRuntime, runId: string, task: CronJobDto): Promise<ScheduledRunResult> {
    // Rust advances the generation when a queued occurrence is claimed again.
    const identity = scheduledRunIdentity(runId, task);
    const key = JSON.stringify([source.projectPath, identity.occurrenceId, identity.claimGeneration]);
    const existing = this.pendingRuns.get(key) ?? this.recentRuns.get(key);
    if (existing) return existing;
    const operation: Promise<ScheduledRunResult> = this.runOnce(source, runId, task, identity.occurrenceId, identity.hostAdmitted).finally(() => {
      this.pendingRuns.delete(key);
      this.recentRuns.set(key, operation);
      while (this.recentRuns.size > MAX_RECENT_SCHEDULED_RUNS) {
        this.recentRuns.delete(this.recentRuns.keys().next().value!);
      }
    });
    this.pendingRuns.set(key, operation);
    return operation;
  }

  private assertRunAllowed(): void {
    if (this.disposed) throw new Error('cancelled: Scheduled task service is closed.');
  }

  /** A task runs in one of the owner's projects, in the scheduled workspace, or in the workspace of a Bot that is not being deleted. */
  private assertTrusted(path: string): void {
    if (this.settings.isTrustedWorkspace(path) || this.bots?.routineBotFor(path)) return;
    throw new Error(this.bots?.isBotWorkspace(path) ? 'paused: The Bot is unavailable.' : 'paused: The project is unavailable.');
  }

  private async runOnce(source: SessionRuntime, runId: string, task: CronJobDto, occurrenceId: string, hostAdmitted: boolean): Promise<ScheduledRunResult> {
    try {
      this.assertRunAllowed();
      if (hostAdmitted) throw new Error('interrupted: This scheduled run was already started, but its completion is unavailable. Retry the task to start a new run.');
      const result = await this.keepingAwake(source.projectPath, () => this.execute(source, runId, task));
      if (task.automation?.notificationPolicy === 'all') await this.notifyOnce(`${source.projectPath}:${occurrenceId}`, task.prompt.split('\n')[0].replace(/^# /, ''), result.summary.slice(0, 240) || 'Scheduled task completed.', source.projectPath === this.settings.scheduledWorkspace ? this.resolveSession('global', result.sessionId) : { projectPath: source.projectPath, sessionId: result.sessionId });
      return result;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (task.automation?.notificationPolicy !== 'none' && !message.startsWith('busy:') && !message.startsWith('cancelled:')) await this.notifyOnce(`${source.projectPath}:${occurrenceId}`, 'Scheduled task needs attention', message);
      throw error;
    }
  }

  /** A routine of a Bot keeps the computer awake while it runs. The owner's own routines do not. */
  private async keepingAwake<T>(path: string, run: () => Promise<T>): Promise<T> {
    const release = this.bots?.routineBotFor(path) ? this.options.keepAwake?.hold() : undefined;
    try {
      return await run();
    } finally {
      release?.();
    }
  }

  private async execute(source: SessionRuntime, runId: string, task: CronJobDto): Promise<ScheduledRunResult> {
    const config = task.automation;
    if (!config) throw new Error('paused: Scheduled task configuration is missing.');
    const path = source.projectPath;
    this.assertTrusted(path);
    const existing = config.runMode === 'selected_session' ? config.targetSessionId : config.runMode === 'task_session' ? config.ownedSessionId : undefined;
    const target = existing ? await this.validateTarget(path, existing) : undefined;
    if (!existing && config.runMode === 'selected_session') throw new Error('paused: Select a target chat.');
    const scopeKey = path === this.settings.scheduledWorkspace ? 'global' : path;
    if (existing && config.runMode === 'selected_session'
      && (config.targetSessionScopeKey !== scopeKey || config.targetSessionMode !== target?.mode
        || config.targetSessionTranscriptPath !== target?.path)) {
      throw new Error('paused: The target chat source changed. Select the chat again.');
    }
    if (existing && config.runMode === 'task_session'
      && (config.ownedSessionScopeKey !== scopeKey || config.ownedSessionMode !== target?.mode
        || config.ownedSessionTranscriptPath !== target?.path || config.sessionMode !== target?.mode)) {
      throw new Error('paused: The task chat source changed. Create a new task.');
    }
    if (!existing && (!config.sessionMode || config.sessionScopeKey !== scopeKey)) {
      throw new Error('paused: The scheduled conversation owner is unavailable.');
    }
    const sessionId = existing ?? randomUUID();
    const managed = path === this.settings.scheduledWorkspace ? this.settings.scheduledSessionWorkspace(sessionId) : undefined;
    const mode = target?.mode ?? config.sessionMode;
    if (mode !== 'chat' && mode !== 'code') throw new Error('paused: The scheduled conversation mode is unavailable.');
    const ref = target?.ref ?? { projectPath: managed?.projectPath ?? path, sessionId, mode };
    const previous = this.sessionQueues.get(ref.sessionId) ?? Promise.resolve();
    const operation = previous.catch(() => undefined).then(async () => {
      // Revalidate after waiting: the service, projects and targets may disappear while queued.
      this.assertRunAllowed();
      this.assertTrusted(path);
      const refreshedTarget = existing ? await this.validateTarget(path, existing) : undefined;
      if (refreshedTarget && (refreshedTarget.mode !== mode || refreshedTarget.path !== target?.path || refreshedTarget.ref.projectPath !== ref.projectPath)) {
        throw new Error('paused: The target chat source changed. Select the chat again.');
      }
      this.assertRunAllowed();
      let executionTask = task;
      if (path === this.settings.scheduledWorkspace && !existing) {
        const { workspaceRoot } = await this.catalog.productPaths();
        this.assertRunAllowed();
        const workspace = await createScheduledRunWorkspace(workspaceRoot, task.id, ref.sessionId, config.name ?? task.prompt.split('\n')[0].replace(/^# /, ''));
        ref.projectPath = workspace.directory;
        this.settings.registerScheduledSession(ref, task.id, workspace.memory);
        this.bridge.setSessionDirectoryGrants(ref.sessionId, [dirname(workspace.memory)]);
        executionTask = { ...task, prompt: `${task.prompt}\n\n${scheduledWorkspaceInstructions(workspace)}` };
      } else if (managed) {
        if (managed.taskId === task.id) this.bridge.setSessionDirectoryGrants(ref.sessionId, [dirname(managed.memory)]);
        executionTask = { ...task, prompt: `${task.prompt}\n\n${scheduledWorkspaceInstructions({ directory: ref.projectPath, work: join(ref.projectPath, 'work'), outputs: join(ref.projectPath, 'outputs'), memory: managed.taskId === task.id ? managed.memory : undefined })}` };
      }
      this.assertRunAllowed();
      // A live runtime already owns its exact transcript. A cold target uses
      // the freshly validated catalog path before any startup hook can write.
      const transcriptPath = this.bridge.get(ref.sessionId) ? undefined : refreshedTarget?.path;
      return this.bridge.withBackgroundSession(ref, !existing, config.model, async (runtime) => {
        this.assertRunAllowed();
        const summary = await runtime.runScheduledTurn(runId, executionTask, async () => {
          this.assertRunAllowed();
          const state = await runtime.getWorkspaceState();
          if (runtime.sessionMode !== mode || !state.transcript_path || !isAbsolute(state.transcript_path)
            || basename(state.transcript_path).toLowerCase() !== `${ref.sessionId.toLowerCase()}.jsonl`
            || (target && state.transcript_path !== target.path)) {
            throw new Error('paused: The scheduled conversation source changed before admission.');
          }
          if (!this.bots?.routineBotFor(path)) {
            this.settings.conversationSpaces.register({ ...ref, mode, projectId: scopeKey === 'global' ? null : path });
          }
          await source.markCronRunStarted(runId, ref.sessionId, { transcriptPath: state.transcript_path, mode, scopeKey });
        });
        return { sessionId: ref.sessionId, summary };
      }, transcriptPath, !existing ? (config.name?.trim() || task.prompt.split('\n')[0].replace(/^# /, '').trim() || 'Scheduled task') : undefined);
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
    this.releaseDue?.();
    this.releaseDue = undefined;
    if (this.timer) clearInterval(this.timer);
    for (const controller of this.controllers.values()) controller.release();
    this.controllers.clear();
  }
}
