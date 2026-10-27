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
 */

import { existsSync } from 'node:fs';
import { join } from 'node:path';

import type { SessionRowDto } from '@lingxi/bridge-client';
import type { SessionRef, WorkspaceLaunchSettings } from '../bridgeTypes.js';
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
}

export interface BotServiceOptions {
  store: BotStore;
  runtimes: BotRuntimes;
  catalogFor(configHome: string): BotCatalog;
  /** Something went wrong that does not stop the operation. */
  warn?: (message: string) => void;
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
    return this.store.list();
  }

  get(botId: string): BotRecord {
    return this.store.get(botId);
  }

  create(input: BotCreateInput): BotRecord {
    const record = this.store.create(input, (draft) => { this.writeSettings(draft); });
    // Every other Bot must now keep out of this one.
    writeAllBotSettings(this.store, (message) => this.warn(message), record.bot_id);
    return record;
  }

  update(botId: string, expectedRevision: number, patch: BotPatch): BotRecord {
    this.assertNotDeleting(botId);
    return this.store.update(botId, expectedRevision, patch, (draft) => { this.writeSettings(draft); });
  }

  /** Stop the Bot's sessions, then move its folder to the trash folder. Rejects while a session is working. */
  async delete(botId: string): Promise<void> {
    const workspace = this.store.workspaceDir(botId);
    if (!existsSync(this.store.botDir(botId))) throw new BotNotFoundError();
    this.assertNotDeleting(botId);
    this.deleting.add(botId);
    try {
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
    this.assertNotDeleting(botId);
    const record = this.store.get(botId);
    const projectPath = this.store.workspaceDir(botId);
    if (sessionId === undefined) {
      const ref = await this.options.runtimes.newSession(projectPath, record.model);
      return { botId, projectPath: ref.projectPath, sessionId: ref.sessionId };
    }
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    const row = await this.catalog(botId).find(projectPath, sessionId);
    if (!row) throw new Error('session does not belong to this Bot');
    await this.options.runtimes.openSession({ projectPath, sessionId: row.uuid }, row.empty_session, row.resume_model ?? record.model);
    return { botId, projectPath, sessionId: row.uuid };
  }

  /** The Bot's own sessions, read from its home. */
  async listSessions(botId: string): Promise<SessionRowDto[]> {
    this.store.get(botId);
    const { sessions } = await this.catalog(botId).list(this.store.workspaceDir(botId));
    return sessions.map(({ empty_session: _empty, resume_model: _model, workspace_path: _workspace, ...row }) => row);
  }

  // What the app asks when it launches or looks after a session

  /** Whether a working directory is the workspace of an existing Bot, which is then trusted without being a project. */
  ownsWorkspace(projectPath: string): boolean {
    return this.store.botIdForWorkspace(projectPath) !== undefined;
  }

  /** The model of the Bot a working directory belongs to, if it has one. */
  modelFor(projectPath: string): string | undefined {
    const botId = this.store.botIdForWorkspace(projectPath);
    if (botId === undefined) return undefined;
    try { return this.store.get(botId).model; } catch { return undefined; }
  }

  /**
   * What a session of the Bot is launched with, instead of the settings of the
   * owner's projects. The settings are made current first, so that the engine
   * reads the isolation as it is now.
   */
  async launchSettings(projectPath: string): Promise<WorkspaceLaunchSettings> {
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
