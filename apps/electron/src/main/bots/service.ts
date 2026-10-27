/**
 * The Bots of this computer and how their sessions run. Free of Electron.
 *
 * A Bot's session is an ordinary session of the one runtime manager, with three
 * differences that all come from the Bot's folder: its working directory is the
 * Bot's workspace, its configuration home is the Bot's home (so its sessions,
 * memory, persona, skills and settings are the Bot's and nothing of the
 * owner's), and the folders bound to it are granted to the engine. The app
 * asks this service whether a working directory is a Bot's, and for these
 * launch settings, wherever it would ask the project list.
 *
 * Bots are macOS-only in this version: their isolation relies on the engine's
 * sandbox and on POSIX path rules. Anywhere else every operation is refused.
 */

import { existsSync } from 'node:fs';
import { readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';

import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BotsAvailability } from '../../shared/bots.js';
import type { SessionRef, WorkspaceLaunchSettings } from '../bridgeTypes.js';
import type { RoutineBot } from '../scheduled.js';
import type { ProjectSessionCatalogRow } from '../session-catalog.js';
import { isSessionId } from '../sessionIdentity.js';
import { writeAllBotSettings, writeBotSettings } from './botSettings.js';
import { BotNotFoundError, type BotCreateInput, type BotPatch, type BotRecord, type BotStore, type BotWorkspace } from './store.js';

/** What of the runtime manager the Bots use. */
export interface BotRuntimes {
  newSession(projectPath: string, model?: string): Promise<SessionRef>;
  openSession(ref: SessionRef, empty?: boolean, resumeModel?: string): Promise<unknown>;
  /** Stops every session of the folder; rejects while one of them is working. */
  closeProject(projectPath: string): Promise<void>;
}

/** What of a session catalog the Bots use: one that reads the Bot's home, not the owner's. */
export interface BotCatalog {
  list(projectPath: string): Promise<{ sessions: ProjectSessionCatalogRow[] }>;
  find(projectPath: string, sessionId: string): Promise<ProjectSessionCatalogRow | undefined>;
  workspaceSettings(projectPath: string): Promise<WorkspaceLaunchSettings>;
  /** Forget what was listed, so that the next listing reads the home again. */
  invalidate(): void;
}

/**
 * What of the routine scheduler the Bots use. A Bot's routines run in a hidden
 * controller session of its workspace, which is no chat of the Bot's.
 */
export interface BotRoutines {
  /** Whether a session of a working directory is the hidden one that watches the clock. */
  isControllerSession(workspace: string, sessionId: string): boolean;
  /** Let go of what the scheduler holds for a working directory, before its sessions are stopped. */
  releaseScope(workspace: string): void;
}

export interface BotServiceOptions {
  store: BotStore;
  runtimes: BotRuntimes;
  catalogFor(configHome: string): BotCatalog;
  /** The routine scheduler; absent where nothing is scheduled. */
  routines?: BotRoutines;
  /** The platform the app runs on; this process's by default. */
  platform?: NodeJS.Platform;
  /** Something went wrong that does not stop the operation. */
  warn?: (message: string) => void;
}

const MAX_ACTIVITY_FILES = 500;

/** Whether Bots can run on a platform. They need the engine's sandbox, which Desktop uses on macOS only. */
export function botsAvailability(platform: NodeJS.Platform = process.platform): BotsAvailability {
  return platform === 'darwin' ? { available: true } : { available: false, reason: 'Bots are available on macOS only.' };
}

export interface BotSessionRef {
  botId: string;
  projectPath: string;
  sessionId: string;
}

export class BotService {
  private readonly store: BotStore;
  private readonly catalogs = new Map<string, BotCatalog>();
  private readonly deleting = new Set<string>();

  constructor(private readonly options: BotServiceOptions) {
    this.store = options.store;
  }

  availability(): BotsAvailability {
    return botsAvailability(this.options.platform);
  }

  /** Refuse, in words the owner can read, wherever Bots cannot run. */
  assertAvailable(): void {
    const { available, reason } = this.availability();
    if (!available) throw new Error(reason ?? 'Bots are not available on this computer.');
  }

  private warn(message: string): void {
    this.options.warn?.(message);
  }

  /**
   * Make the Bot's `settings.json` current; a Bot cannot be made or changed if
   * that fails. Returns the folders that are granted.
   */
  private writeSettings(record: BotRecord): BotWorkspace[] {
    const { bound, skipped } = writeBotSettings(this.store, record);
    for (const workspace of skipped) {
      this.warn(`Bot ${record.bot_id}: ${workspace.path} is no longer a folder a Bot may work in, and is not granted`);
    }
    return bound;
  }

  list(): BotRecord[] {
    this.assertAvailable();
    return this.store.list();
  }

  get(botId: string): BotRecord {
    this.assertAvailable();
    return this.store.get(botId);
  }

  create(input: BotCreateInput): BotRecord {
    this.assertAvailable();
    const record = this.store.create(input, (draft) => { this.writeSettings(draft); });
    // Every other Bot must now keep out of this one.
    writeAllBotSettings(this.store, (message) => this.warn(message), record.bot_id);
    return record;
  }

  update(botId: string, expectedRevision: number, patch: BotPatch): BotRecord {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    return this.store.update(botId, expectedRevision, patch, (draft) => { this.writeSettings(draft); });
  }

  /** Stop the Bot's sessions, then move its folder to the trash folder. Rejects while a session is working. */
  async delete(botId: string): Promise<void> {
    this.assertAvailable();
    const workspace = this.store.workspaceDir(botId);
    if (!existsSync(this.store.botDir(botId))) throw new BotNotFoundError();
    this.assertNotDeleting(botId);
    this.deleting.add(botId);
    try {
      // The scheduler no longer lists the Bot, and lets go of its controller so
      // that the controller does not hold an engine on the folder that moves.
      this.options.routines?.releaseScope(workspace);
      await this.options.runtimes.closeProject(workspace);
      this.store.trash(botId);
      this.catalogs.delete(botId);
    } finally {
      this.deleting.delete(botId);
    }
    // The others no longer need to keep out of it.
    writeAllBotSettings(this.store, (message) => this.warn(message));
  }

  /** A new session of the Bot, or an existing one of its own. */
  async openSession(botId: string, sessionId?: string): Promise<BotSessionRef> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    const record = this.store.get(botId);
    const projectPath = this.store.workspaceDir(botId);
    if (sessionId === undefined) {
      const ref = await this.options.runtimes.newSession(projectPath, record.model);
      return { botId, projectPath: ref.projectPath, sessionId: ref.sessionId };
    }
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    if (this.options.routines?.isControllerSession(projectPath, sessionId.toLowerCase())) throw new Error('session does not belong to this Bot');
    const row = await this.catalog(botId).find(projectPath, sessionId);
    if (!row) throw new Error('session does not belong to this Bot');
    await this.options.runtimes.openSession({ projectPath, sessionId: row.uuid }, row.empty_session, row.resume_model ?? record.model);
    return { botId, projectPath, sessionId: row.uuid };
  }

  /** The Bot's own sessions, read from its home. */
  async listSessions(botId: string): Promise<SessionRowDto[]> {
    this.assertAvailable();
    this.store.get(botId);
    // The owner asked to see them: show what is there now, not what was there a few seconds ago.
    this.catalog(botId).invalidate();
    const workspace = this.store.workspaceDir(botId);
    const { sessions } = await this.catalog(botId).list(workspace);
    return sessions
      .filter((row) => !this.options.routines?.isControllerSession(workspace, row.uuid))
      .map(({ empty_session: _empty, resume_model: _model, workspace_path: _workspace, ...row }) => row);
  }

  /**
   * When the Bot was last used: the newest modification time among its
   * transcripts, read from the files rather than from a running engine. The
   * scheduler's hidden controller session is not use of the Bot.
   */
  async lastActivityAt(botId: string): Promise<number | undefined> {
    this.assertAvailable();
    const directory = this.store.transcriptsDir(botId);
    const workspace = this.store.workspaceDir(botId);
    let names: string[];
    try {
      names = (await readdir(directory))
        .filter((name) => name.endsWith('.jsonl') && !this.options.routines?.isControllerSession(workspace, name.slice(0, -'.jsonl'.length)))
        .slice(0, MAX_ACTIVITY_FILES);
    } catch { return undefined; }
    let newest: number | undefined;
    for (const name of names) {
      try {
        const { mtimeMs } = await stat(join(directory, name));
        if (newest === undefined || mtimeMs > newest) newest = mtimeMs;
      } catch { /* A transcript that went away has no time to give. */ }
    }
    return newest === undefined ? undefined : Math.floor(newest);
  }

  // What the app asks when it launches or looks after a session

  /** The working directory of the Bot's sessions. */
  workspaceOf(botId: string): string {
    return this.store.workspaceDir(botId);
  }

  /** The Bot whose workspace a working directory is, if it is one and Bots can run here. */
  botIdFor(projectPath: string): string | undefined {
    return this.availability().available ? this.store.botIdForWorkspace(projectPath) : undefined;
  }

  /** Whether a working directory is the workspace of an existing Bot, which is then trusted without being a project. */
  ownsWorkspace(projectPath: string): boolean {
    return this.botIdFor(projectPath) !== undefined;
  }

  /** The model of the Bot a working directory belongs to, if it has one. */
  modelFor(projectPath: string): string | undefined {
    const botId = this.botIdFor(projectPath);
    if (botId === undefined) return undefined;
    try { return this.store.get(botId).model; } catch { return undefined; }
  }

  /** The name of the Bot a working directory belongs to, if it has one. */
  nameFor(projectPath: string): string | undefined {
    const botId = this.botIdFor(projectPath);
    if (botId === undefined) return undefined;
    try { return this.store.get(botId).name; } catch { return undefined; }
  }

  // What the routine scheduler asks

  /** The Bots whose routines run now: none where Bots cannot run, and not one that is being deleted or cannot be read. */
  routineBots(): RoutineBot[] {
    if (!this.availability().available) return [];
    let entries: ReturnType<BotStore['entries']>;
    try { entries = this.store.entries(); } catch { return []; }
    return entries.flatMap(({ botId, record }) => record !== undefined && !this.deleting.has(botId) ? [this.routineBot(record)] : []);
  }

  /** The Bot whose workspace a working directory is, while its routines can run. */
  routineBotFor(workspace: string): RoutineBot | undefined {
    const botId = this.botIdFor(workspace);
    if (botId === undefined || this.deleting.has(botId)) return undefined;
    try { return this.routineBot(this.store.get(botId)); } catch { return undefined; }
  }

  /** Whether a working directory is shaped like a Bot's workspace, whether or not the Bot still exists. */
  isBotWorkspace(path: string): boolean {
    return this.store.workspaceBotId(path) !== undefined;
  }

  private routineBot(record: BotRecord): RoutineBot {
    const botId = record.bot_id;
    return {
      botId,
      name: record.name,
      workspace: this.store.workspaceDir(botId),
      ...(record.model !== undefined ? { model: record.model } : {}),
      catalog: () => this.catalog(botId),
    };
  }

  /**
   * What a session of the Bot is launched with, instead of the settings of the
   * owner's projects. The settings are made current first, so that the engine
   * reads the isolation as it is now.
   */
  async launchSettings(projectPath: string): Promise<WorkspaceLaunchSettings> {
    this.assertAvailable();
    const botId = this.store.botIdForWorkspace(projectPath);
    if (botId === undefined) throw new Error('not the workspace of a Bot');
    this.assertNotDeleting(botId);
    const bound = this.writeSettings(this.store.get(botId));
    const home = this.store.homeDir(botId);
    const base = await this.catalog(botId).workspaceSettings(this.store.workspaceDir(botId));
    return {
      // The engine reads some of its configuration from the environment only,
      // and what the Bot's own settings say must not move the home elsewhere.
      environment: { ...base.environment, LINGXI_CONFIG_DIR: home },
      configHome: home,
      globalConfigPath: join(home, '.lingxi.json'),
      additionalDirectories: bound.map((workspace) => workspace.path),
    };
  }

  private catalog(botId: string): BotCatalog {
    let catalog = this.catalogs.get(botId);
    if (catalog === undefined) {
      catalog = this.options.catalogFor(this.store.homeDir(botId));
      this.catalogs.set(botId, catalog);
    }
    return catalog;
  }

  private assertNotDeleting(botId: string): void {
    if (this.deleting.has(botId)) throw new Error('This Bot is being deleted.');
  }
}
