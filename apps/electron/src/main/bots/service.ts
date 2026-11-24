/**
 * The Bots of this computer and how their sessions run. Free of Electron.
 *
 * A Bot's session is an ordinary session of the one runtime manager, with three
 * differences that all come from the Bot's folders: its working directory is one
 * of them (the Bot's own `<bot>/workspace`, or a folder the owner bound to it),
 * its configuration home is the Bot's home (so its sessions, memory, persona,
 * skills and settings are the Bot's and nothing of the owner's), and the other
 * folders are granted to the engine. The app asks this service whether a session
 * is a Bot's, and for these launch settings, wherever it would ask the project
 * list.
 *
 * Which Bot a session belongs to is not read off its working directory, because
 * a bound folder is the owner's and the owner's own sessions work there too.
 * A session is a Bot's because the Bot opened it (a claim, made before its
 * engine starts), because the Bot recorded it as its own with the transcript it
 * keeps in its home, or because it works in the Bot's own folder, which nothing
 * but a Bot uses and which is therefore never given the owner's configuration.
 *
 * Bots run on every computer. What confines the shell commands of a Bot to its
 * folders is the engine's sandbox, which a Bot has unless its owner switched it off
 * and which a computer may not be able to give (see `sandboxAvailability`): such a
 * computer makes no Bot with the sandbox on and starts none that has it.
 */

import { existsSync, lstatSync, realpathSync } from 'node:fs';
import { readdir, stat } from 'node:fs/promises';
import { basename, isAbsolute, join, resolve } from 'node:path';

import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BotReachUi, BotSandboxAvailability, BotsAvailability } from '../../shared/bots.js';
import type { BotMarketOrigin } from '../../shared/botMarket.js';
import type { SessionRef, WorkspaceLaunchSettings } from '../bridgeTypes.js';
import type { LocalAppBotAudience } from '../local-apps/mcpGateway.js';
import type { SessionRuntime } from '../bridge.js';
import type { RemoteBot, RemoteBotLookup, BotChat } from '../remoteControl/remoteBots.js';
import type { RoutineBot, RoutineCatalog, RoutineFolder } from '../scheduled.js';
import type { ProjectSessionCatalogRow } from '../session-catalog.js';
import { isSessionId } from '../sessionIdentity.js';
import { ENGINE_SANDBOX_REFUSAL } from '../host-utils.js';
import { writeAllBotSettings, writeBotSettings } from './botSettings.js';
import { ensureRouterEntry } from './mcpConfig.js';
import { BotSandboxUnavailableError, sandboxAvailability } from './sandboxAvailability.js';
import { ROUTER_TOKEN_ENV, ROUTER_URL_ENV } from './routerTools.js';
import {
  BotConflictError,
  BotNotFoundError,
  DEFAULT_WORKSPACE_ID,
  parseWorkspaceInput,
  stillBound,
  type BotCreateInput,
  type BotPatch,
  type BotRecord,
  type BotStore,
  type BotWorkspace,
  type BotWorkspaceInput,
  type BotWorkspaceMode,
} from './store.js';

/** What of the runtime manager the Bots use. */
export interface BotRuntimes {
  newSession(projectPath: string, model: string | undefined, ownership: {
    mode: 'chat' | 'code';
    projectId: string | null;
    /** Which unsent draft this is: a Bot's folder has one of its own, apart from the owner's and the other Bots'. */
    draftKey?: string;
    /** The session has its id and nothing has started yet. */
    onAllocated?: (ref: SessionRef) => Promise<void>;
  }): Promise<SessionRef>;
  openSession(ref: SessionRef, empty?: boolean, resumeModel?: string, activate?: boolean, transcriptPath?: string): Promise<unknown>;
  get?(sessionId: string): SessionRuntime | undefined;
  /** Stops every session of the folder; rejects while one of them is working. Only for a folder nothing but the Bot uses. */
  closeProject(projectPath: string): Promise<void>;
  /** Stops one session; rejects while it is working. */
  closeSession(ref: SessionRef): Promise<void>;
  /** Starts a session's engine again, which reads the Bot's settings and grants as they are now. Rejects while the session is working. */
  restart(ref: SessionRef): Promise<void>;
  /** The directories the engine of a session is launched with the next time. */
  setSessionDirectoryGrants(sessionId: string, directories: readonly string[]): void;
  /** Every session that is open, in whichever folder. */
  readonly runtimeSummaries: ReadonlyArray<{ projectPath: string; sessionId: string }>;
}

/** What of a session catalog the Bots use: one that reads the Bot's home, not the owner's. */
export interface BotCatalog {
  list(projectPath: string, scope?: 'current' | 'repository' | 'all', include?: (row: ProjectSessionCatalogRow) => boolean): Promise<{ sessions: ProjectSessionCatalogRow[] }>;
  find(projectPath: string, sessionId: string): Promise<ProjectSessionCatalogRow | undefined>;
  select(projectPath: string, sessionId: string, selection: { scope: 'current'; transcriptPath: string }): Promise<ProjectSessionCatalogRow | undefined>;
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

/** What of the Bot router the service uses: the address it listens at, and a token for each session that may call it. */
export interface BotRouterAccess {
  /** The router's address once it listens; rejects if it cannot. */
  ready(): Promise<string>;
  /** The token that speaks for a session; a session has one at a time. */
  issue(botId: string, sessionId: string): string;
  /** None of the Bot's sessions may call the router any more. */
  revokeBot(botId: string): void;
}

/** What of the tasks between Bots the service uses. */
export interface BotDelegation {
  /** The Bot is about to go: its tasks end, and the work on them stops. */
  botDeleting(botId: string): Promise<void>;
}

/**
 * What of the Local Apps' MCP gateway the service uses. Each Bot that was given applications has a
 * credential of its own, kept in its home for its engine to find, which names those applications
 * and no others.
 */
export interface BotLocalApps {
  /** The Bot's engine may use these applications and no others; none takes the Bot's way in back. */
  publish(audience: LocalAppBotAudience): Promise<void>;
  /** The Bot is gone: its credential stops working at once. */
  revoke(botId: string): Promise<void>;
}

export interface BotServiceOptions {
  store: BotStore;
  runtimes: BotRuntimes;
  catalogFor(configHome: string): BotCatalog;
  /** The routine scheduler; absent where nothing is scheduled. */
  routines?: BotRoutines;
  /** The Bot router; without it a Bot's sessions cannot reach the other Bots. */
  router?: BotRouterAccess;
  /** Tasks between Bots; absent where there are none. */
  delegation?: BotDelegation;
  /** The Local Apps' MCP gateway; without it no Bot can use a Local App. */
  localApps?: BotLocalApps;
  /** The working directory a session is open in, if it is open; without it only a chat's transcript tells whose it is. */
  openProject?: (sessionId: string) => string | undefined;
  /** A Bot was made, changed or deleted, which Remote Control must hear of: its switch, name and chats are what viewers see. */
  changed?: () => void;
  /** A Bot was deleted: whatever is kept of it elsewhere (what it asked for, say) goes too. */
  deleted?: (botId: string) => void;
  /** Revoke private Team runs before a local policy change or deletion awaits, and again once a deleted Bot is trashed (a run may start while the deletion waits). */
  teamRevoking?: (botId: string) => void;
  /** Whether this computer can confine a Bot's commands to its folders, asked each time (`bwrap` can be installed while the app runs); this computer's by default. */
  sandbox?: () => BotSandboxAvailability;
  /** The platform the app runs on; this process's by default. */
  platform?: NodeJS.Platform;
  /** How long a session that was working when its Bot's folders changed is left alone before it is looked at again. */
  retryMs?: number;
  /** Something went wrong that does not stop the operation. */
  warn?: (message: string) => void;
}

const MAX_ACTIVITY_FILES = 500;
const DEFAULT_RETRY_MS = 3000;
/** About half an hour at the default delay. */
const MAX_RETRIES = 600;
const MAX_REMEMBERED_MISSES = 2000;

export interface BotSessionRef {
  botId: string;
  projectPath: string;
  sessionId: string;
  /** The folder of the Bot the session works in. */
  workspaceId: string;
}

/** A chat of a Bot as the window lists it. */
export type BotSessionRow = SessionRowDto & {
  /** The folder of the Bot the chat worked in, if the Bot still works there. */
  workspaceId?: string;
  /** Where the chat worked, when that folder is no longer one of the Bot's: the chat can be read and is resumed by adding the folder again. */
  removedPath?: string;
};

/**
 * What decides what the engine of a session was given (the Bot's folders, whether and how its commands are
 * confined, and whether it was given Local Apps, which its settings allow and which its engine finds a way
 * to only when it starts): when it differs from the Bot's, the session is launched again. Which Local Apps
 * they are is no part of it: a session that has the way in is told when they change.
 */
function launchKeyOf(record: BotRecord): string {
  return JSON.stringify([record.workspaces.map((workspace) => [workspace.path, workspace.mode]), record.sandbox, record.scope.localApps.length > 0]);
}

/** Whether two lists name the same applications, whatever the order. */
function sameApps(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((id) => right.includes(id));
}

function realOrSame(path: string): string {
  try { return realpathSync.native(path); } catch { return resolve(path); }
}

export class BotService {
  private readonly store: BotStore;
  private readonly catalogs = new Map<string, BotCatalog>();
  private readonly deleting = new Set<string>();
  /** Session id → the Bot that opened it. */
  private readonly claims = new Map<string, string>();
  /** Session and working directory pairs looked for in the Bots' records and not found. */
  private readonly misses = new Set<string>();
  /** Session id → what its engine was launched with: {@link launchKeyOf}. */
  private readonly launched = new Map<string, string>();
  private readonly retries = new Map<string, { timer: ReturnType<typeof setTimeout>; attempts: number }>();
  private readonly syncing = new Set<Promise<void>>();

  constructor(private readonly options: BotServiceOptions) {
    this.store = options.store;
  }

  /** Bots can be used on every computer; whether it can give them a sandbox is the part of the answer that differs. */
  /**
   * Bots run wherever the app does except on Windows: the rules that keep a Bot to its folders are written for
   * POSIX paths, which the engine does not take there.
   */
  availability(): BotsAvailability {
    if ((this.options.platform ?? process.platform) === 'win32') return { available: false, reason: 'Bots are not available on Windows yet.' };
    return { available: true, sandbox: this.sandbox() };
  }

  /** Whether this computer can confine a Bot's commands to its folders. */
  sandbox(): BotSandboxAvailability {
    return (this.options.sandbox ?? (() => sandboxAvailability()))();
  }

  /** Nothing about a computer as a whole refuses Bots now; where one does again, this is the place that says so. */
  assertAvailable(): void {
    const availability = this.availability();
    if (!availability.available) throw new Error(availability.reason ?? 'Bots are not available on this computer.');
  }

  /** A Bot whose sandbox is on cannot be made, switched on or started where the computer has none to give it. */
  private assertSandboxPossible(record: BotRecord): void {
    if (record.sandbox.enabled && !this.sandbox().available) throw new BotSandboxUnavailableError();
  }

  /** What went wrong starting an engine, with the engine's own refusal to run without its sandbox said the way the window knows it. */
  private refusal(error: unknown): unknown {
    return error instanceof Error && ENGINE_SANDBOX_REFUSAL.test(error.message) ? new BotSandboxUnavailableError() : error;
  }

  private warn(message: string): void {
    this.options.warn?.(message);
  }

  private changed(): void {
    try { this.options.changed?.(); } catch (error) { this.warn(`a change to the Bots could not be passed on: ${error instanceof Error ? error.message : String(error)}`); }
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
    const record = this.store.create(input, (draft) => { this.assertSandboxPossible(draft); this.writeSettings(draft); });
    // Every other Bot must now keep out of this one.
    writeAllBotSettings(this.store, (message) => this.warn(message), record.bot_id);
    this.changed();
    return record;
  }

  /** A market copy uses the same settings isolation and change notifications as any personal Bot. */
  createFromMarket(input: BotCreateInput, origin: BotMarketOrigin, prepare: (record: BotRecord) => void): BotRecord {
    this.assertAvailable();
    const record = this.store.createFromMarket(input, origin, draft => { this.assertSandboxPossible(draft); prepare(draft); this.writeSettings(draft); });
    writeAllBotSettings(this.store, message => this.warn(message), record.bot_id);
    this.changed();
    return record;
  }

  /**
   * Change a Bot. A folder that is left out of the list is removed: the Bot's chats there end (they
   * are not touched while one of them is working), and what the Bot did there stays in its home.
   */
  async update(botId: string, expectedRevision: number, patch: BotPatch): Promise<BotRecord> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    this.options.teamRevoking?.(botId);
    const before = this.store.get(botId);
    if (Number.isSafeInteger(expectedRevision) && expectedRevision >= 1 && before.revision !== expectedRevision) throw new BotConflictError();
    const removed = patch.workspaces === undefined ? [] : this.leftOut(before, patch.workspaces);
    this.assertNothingWorking(botId, removed);
    const record = this.store.update(botId, expectedRevision, patch, (draft) => {
      // Only turning the sandbox on asks the computer for one: a Bot that has it on and cannot run can still be given up, or made free.
      if (!before.sandbox.enabled) this.assertSandboxPossible(draft);
      this.writeSettings(draft);
    });
    if (record.revision !== expectedRevision) {
      if (!sameApps(before.scope.localApps, record.scope.localApps)) await this.publishLocalApps(record);
      await this.endChatsIn(botId, removed);
      this.syncSoon(botId);
      this.changed();
    }
    return record;
  }

  /** The folders of the Bot that a replacement list no longer has. */
  private leftOut(record: BotRecord, list: ReadonlyArray<BotWorkspaceInput>): BotWorkspace[] {
    if (!Array.isArray(list)) throw new Error('invalid workspaces');
    const kept = new Set(list.map((entry) => {
      const input = parseWorkspaceInput(entry);
      return this.store.workspace(input.path, input.mode).path;
    }));
    return record.workspaces.filter((workspace) => !kept.has(workspace.path));
  }

  /** Bind one more folder to a Bot. Its open chats are launched again, when they are not working, to be given it. */
  async addWorkspace(botId: string, input: BotWorkspaceInput): Promise<BotRecord> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    this.options.teamRevoking?.(botId);
    const record = this.store.addWorkspace(botId, input, (draft) => { this.writeSettings(draft); });
    this.syncSoon(botId);
    this.changed();
    return record;
  }

  /** Give the Bot a folder, or the mode that is asked for of one it has: what approving a request for a folder does. */
  async grantWorkspace(botId: string, input: BotWorkspaceInput): Promise<BotRecord> {
    this.assertAvailable();
    const record = this.store.get(botId);
    const wanted = parseWorkspaceInput(input);
    const checked = this.store.workspace(wanted.path, wanted.mode);
    const held = record.workspaces.find((workspace) => workspace.path === checked.path);
    if (held === undefined) return this.addWorkspace(botId, wanted);
    // What the owner gave is never taken back by an answer to a request that was made earlier.
    if (held.mode === 'write' || wanted.mode === 'read') return record;
    return this.changeWorkspace(botId, held.id, { mode: wanted.mode });
  }

  /** Change the mode or the label of a bound folder. */
  async changeWorkspace(botId: string, workspaceId: string, change: { mode?: BotWorkspaceMode; label?: string | null }): Promise<BotRecord> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    this.options.teamRevoking?.(botId);
    const before = this.store.get(botId);
    const record = this.store.changeWorkspace(botId, workspaceId, change, (draft) => { this.writeSettings(draft); });
    if (record.revision !== before.revision) {
      this.syncSoon(botId);
      this.changed();
    }
    return record;
  }

  /** Remove a bound folder: the Bot's chats there end, and its history of them stays, to be read and resumed if the folder is added again. */
  async removeWorkspace(botId: string, workspaceId: string): Promise<BotRecord> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    this.options.teamRevoking?.(botId);
    const before = this.store.get(botId);
    const folder = before.workspaces.find((workspace) => workspace.id === workspaceId);
    this.assertNothingWorking(botId, folder === undefined ? [] : [folder]);
    const record = this.store.removeWorkspace(botId, workspaceId, (draft) => { this.writeSettings(draft); });
    await this.endChatsIn(botId, folder === undefined ? [] : [folder]);
    this.syncSoon(botId);
    this.changed();
    return record;
  }

  /** A market Bot's record moves to another release. The files were merged by the caller; conversations, memory and settings are not touched. */
  upgradeFromMarket(botId: string, expectedRevision: number, release: { releaseId: string; digest: string; name?: string; description?: string }): BotRecord {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    const record = this.store.upgradeFromMarket(botId, expectedRevision, release);
    this.changed();
    return record;
  }

  /** Stop the Bot's sessions, then move its folder to the trash folder. Rejects while a session is working. */
  async delete(botId: string): Promise<void> {
    this.assertAvailable();
    const workspace = this.store.workspaceDir(botId);
    if (!existsSync(this.store.botDir(botId))) throw new BotNotFoundError();
    this.assertNotDeleting(botId);
    this.deleting.add(botId);
    this.options.teamRevoking?.(botId);
    try {
      // The chats in the owner's folders are stopped one by one; nothing of what the owner's other
      // chats in those folders are doing is touched. None of them may be working.
      const elsewhere = this.openChatsOf(botId).filter((chat) => chat.projectPath !== workspace);
      if (elsewhere.some((chat) => this.working(chat.sessionId))) throw new Error('cancel active turns and pending interactions before removing a project');
      // No new task goes to the Bot or comes from it now; those under way end
      // before anything of the Bot is stopped.
      if (this.options.delegation !== undefined) await this.options.delegation.botDeleting(botId);
      // The scheduler no longer lists the Bot, and lets go of its controller so
      // that the controller does not hold an engine on the folder that moves.
      this.options.routines?.releaseScope(workspace);
      await this.options.runtimes.closeProject(workspace);
      for (const chat of elsewhere) await this.closeChat(chat);
      // What the Bot was given of the Local Apps stops working before its folder moves.
      try { await this.options.localApps?.revoke(botId); } catch (error) {
        this.warn(`Bot ${botId}: its Local Apps could not be taken back cleanly: ${error instanceof Error ? error.message : String(error)}`);
      }
      this.store.trash(botId);
      this.options.teamRevoking?.(botId);
      this.catalogs.delete(botId);
      this.forget(botId);
      this.options.router?.revokeBot(botId);
    } finally {
      this.deleting.delete(botId);
    }
    // The others no longer need to keep out of it.
    writeAllBotSettings(this.store, (message) => this.warn(message));
    try { this.options.deleted?.(botId); } catch (error) { this.warn(`a deleted Bot could not be forgotten: ${error instanceof Error ? error.message : String(error)}`); }
    this.changed();
  }

  // Which Bot a session is the chat of

  /**
   * The Bot a session belongs to, if it belongs to one. A session the Bot opened, or recorded as its
   * own, is the Bot's wherever it works; a session in the Bot's own folder is the Bot's whoever
   * asked for it. The owner's sessions in a folder they bound to a Bot stay the owner's.
   */
  botIdForSession(ref: { projectPath: string; sessionId: string }): string | undefined {
    if (typeof ref.sessionId !== 'string') return undefined;
    const sessionId = ref.sessionId.toLowerCase();
    const claimed = this.claims.get(sessionId);
    if (claimed !== undefined) return claimed;
    const own = this.store.botIdForWorkspace(ref.projectPath);
    if (own !== undefined) return own;
    if (!isSessionId(sessionId) || typeof ref.projectPath !== 'string' || !isAbsolute(ref.projectPath)) return undefined;
    const key = `${sessionId}\0${ref.projectPath}`;
    if (this.misses.has(key)) return undefined;
    const recorded = this.store.botIdOfRecordedSession(sessionId, ref.projectPath);
    if (recorded === undefined) {
      if (this.misses.size >= MAX_REMEMBERED_MISSES) this.misses.clear();
      this.misses.add(key);
      return undefined;
    }
    this.claims.set(sessionId, recorded);
    return recorded;
  }

  /** Whether a session is a Bot's, so that it is trusted without being a project and gets the Bot's configuration. */
  ownsSession(ref: { projectPath: string; sessionId: string }): boolean {
    return this.botIdForSession(ref) !== undefined;
  }

  /** The session is the Bot's from now on, before its engine starts. A session is one Bot's only. */
  claimSession(botId: string, sessionId: string): void {
    const id = sessionId.toLowerCase();
    const owner = this.claims.get(id);
    if (owner !== undefined && owner !== botId) throw new Error('session does not belong to this Bot');
    this.claims.set(id, botId);
    this.misses.clear();
  }

  private forgetSession(sessionId: string): void {
    const id = sessionId.toLowerCase();
    this.claims.delete(id);
    this.launched.delete(id);
  }

  /** Nothing is remembered of the Bot's sessions. */
  private forget(botId: string): void {
    for (const [sessionId, owner] of this.claims) if (owner === botId) this.forgetSession(sessionId);
    const retry = this.retries.get(botId);
    if (retry !== undefined) { clearTimeout(retry.timer); this.retries.delete(botId); }
  }

  /** The sessions of a Bot that are open, whichever folder they work in. The hidden one that watches the clock is no chat. */
  private openChatsOf(botId: string): Array<{ projectPath: string; sessionId: string }> {
    return this.options.runtimes.runtimeSummaries
      .filter((summary) => this.botIdForSession(summary) === botId
        && this.options.routines?.isControllerSession(this.store.workspaceDir(botId), summary.sessionId) !== true)
      .map(({ projectPath, sessionId }) => ({ projectPath, sessionId }));
  }

  private working(sessionId: string): boolean {
    return this.options.runtimes.get?.(sessionId)?.hasActiveWork === true;
  }

  private async closeChat(chat: { projectPath: string; sessionId: string }): Promise<void> {
    const mode = this.options.runtimes.get?.(chat.sessionId)?.sessionMode;
    await this.options.runtimes.closeSession({ projectPath: chat.projectPath, sessionId: chat.sessionId, ...(mode !== undefined ? { mode } : {}) });
    this.forgetSession(chat.sessionId);
  }

  /** A folder is not given up while a chat of the Bot is working in it. */
  private assertNothingWorking(botId: string, folders: readonly BotWorkspace[]): void {
    if (folders.length === 0) return;
    const paths = new Set(folders.map((folder) => folder.path));
    for (const chat of this.openChatsOf(botId)) {
      if (paths.has(chat.projectPath) && this.working(chat.sessionId)) {
        throw new Error(`A chat is working in ${basename(chat.projectPath)}. Wait for it to finish, or stop it, before removing the folder.`);
      }
    }
  }

  /** The Bot's chats in the folders it no longer has end. */
  private async endChatsIn(botId: string, folders: readonly BotWorkspace[]): Promise<void> {
    if (folders.length === 0) return;
    const paths = new Set(folders.map((folder) => folder.path));
    for (const chat of this.openChatsOf(botId)) {
      if (!paths.has(chat.projectPath)) continue;
      try { await this.closeChat(chat); } catch (error) {
        this.warn(`Bot ${botId}: a chat in ${chat.projectPath} could not be ended: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }

  // Keeping the engines of open chats in step with the Bot's folders

  /**
   * The Bot's open chats must be started again to be given what the Bot's folders are now: the engine
   * reads the permission rules of its settings once, when it starts. A chat that is working is left
   * alone, and looked at again in a while.
   */
  private syncSoon(botId: string): void {
    const run = this.syncChats(botId).catch((error: unknown) => {
      this.warn(`Bot ${botId}: its chats could not be given its folders: ${error instanceof Error ? error.message : String(error)}`);
    }).finally(() => { this.syncing.delete(run); });
    this.syncing.add(run);
  }

  /** Resolves when every change to a Bot's folders has been passed on to the chats that could take it. */
  async settled(): Promise<void> {
    while (this.syncing.size > 0) await Promise.all([...this.syncing]);
  }

  private async syncChats(botId: string): Promise<void> {
    const record = this.store.get(botId);
    const current = launchKeyOf(record);
    const earlier = this.retries.get(botId);
    if (earlier !== undefined) { clearTimeout(earlier.timer); this.retries.delete(botId); }
    let behind = false;
    for (const chat of this.openChatsOf(botId)) {
      const launched = this.launched.get(chat.sessionId.toLowerCase());
      if (launched === current) continue;
      const runtime = this.options.runtimes.get?.(chat.sessionId);
      const folder = this.store.allWorkspaces(record).find((workspace) => workspace.path === chat.projectPath);
      if (runtime === undefined || folder === undefined) continue;
      const bound = record.workspaces.filter((workspace) => stillBound(this.store, workspace));
      // Whatever starts the engine next is given these.
      this.options.runtimes.setSessionDirectoryGrants(chat.sessionId, this.grantsFor(record, bound, chat.projectPath));
      // An engine that has not been launched yet reads the settings as they are when it is.
      if (launched === undefined) continue;
      if (runtime.hasActiveWork) { behind = true; continue; }
      try {
        await this.options.runtimes.restart({ projectPath: chat.projectPath, sessionId: chat.sessionId, ...(runtime.sessionMode !== undefined ? { mode: runtime.sessionMode } : {}) });
      } catch (error) {
        behind = true;
        this.warn(`Bot ${botId}: a chat could not be started again with its folders yet: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    if (behind) this.retryLater(botId, (earlier?.attempts ?? 0) + 1);
  }

  private retryLater(botId: string, attempts: number): void {
    if (attempts > MAX_RETRIES) {
      this.warn(`Bot ${botId}: some chats were not given its folders: they kept working`);
      return;
    }
    const timer = setTimeout(() => {
      this.retries.set(botId, { timer, attempts });
      this.syncSoon(botId);
    }, this.options.retryMs ?? DEFAULT_RETRY_MS);
    timer.unref?.();
    this.retries.set(botId, { timer, attempts });
  }

  /** What an engine working in `cwd` is given besides it: the Bot's own folder, when it works elsewhere, and the owner's folders that are still what they were. */
  private grantsFor(record: BotRecord, bound: readonly BotWorkspace[], cwd: string): string[] {
    return [this.store.workspaceDir(record.bot_id), ...bound.map((workspace) => workspace.path)].filter((path) => path !== cwd);
  }

  // Opening sessions

  /** The folder a session may be started in: one the Bot has, and for a bound one a folder that still is what it was. */
  private openableFolder(record: BotRecord, workspaceId: string): BotWorkspace {
    const folder = this.store.workspaceById(record, workspaceId);
    if (folder === undefined) throw new Error('The Bot has no such folder.');
    if (folder.id !== DEFAULT_WORKSPACE_ID && !stillBound(this.store, folder)) throw new Error(`${folder.path} is no longer a folder a Bot may work in.`);
    return folder;
  }

  /** The folder of the Bot a path is, if it is one. */
  private folderAt(record: BotRecord, projectPath: string): BotWorkspace | undefined {
    const wanted = resolve(projectPath);
    return this.store.allWorkspaces(record).find((workspace) => workspace.path === wanted);
  }

  /** A new session of the Bot (in the folder named, else its own), or an existing one of its own. */
  async openSession(botId: string, sessionId?: string, transcriptPath?: string, activate = true, workspaceId?: string): Promise<BotSessionRef> {
    this.assertAvailable();
    this.assertNotDeleting(botId);
    const record = this.store.get(botId);
    if (sessionId === undefined) {
      if (!activate) throw new Error('A prepared Bot open requires an existing session.');
      if (transcriptPath !== undefined) throw new Error('invalid Bot session selection');
      const folder = this.openableFolder(record, workspaceId ?? DEFAULT_WORKSPACE_ID);
      this.assertSandboxPossible(record);
      let claimed: string | undefined;
      try {
        const ref = await this.options.runtimes.newSession(folder.path, record.model, {
          mode: 'code',
          projectId: null,
          draftKey: `bot:${botId}:${folder.id}`,
          onAllocated: async (allocated) => { this.claimSession(botId, allocated.sessionId); claimed = allocated.sessionId; },
        });
        return { botId, projectPath: ref.projectPath, sessionId: ref.sessionId, workspaceId: folder.id };
      } catch (error) {
        if (claimed !== undefined && this.options.runtimes.get?.(claimed) === undefined) this.forgetSession(claimed);
        throw this.refusal(error);
      }
    }
    if (workspaceId !== undefined) throw new Error('A chat that exists keeps the folder it works in.');
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    sessionId = sessionId.toLowerCase();
    // The hidden session that watches the clock is nobody's chat.
    if (this.options.routines?.isControllerSession(this.store.workspaceDir(botId), sessionId)) throw new Error('session does not belong to this Bot');
    const live = this.options.runtimes.get?.(sessionId);
    const saved = this.store.sessionSource(botId, sessionId);
    if (saved !== undefined && transcriptPath !== undefined && saved.path !== transcriptPath) throw new Error('The Bot session already has a different saved source.');
    const selectedPath = transcriptPath ?? saved?.path;
    let folder: BotWorkspace | undefined;
    if (live !== undefined) {
      if (this.botIdForSession({ projectPath: live.projectPath, sessionId }) !== botId) throw new Error('session does not belong to this Bot');
      folder = this.folderAt(record, live.projectPath);
    } else if (selectedPath !== undefined) {
      folder = this.folderOfTranscript(record, selectedPath);
    }
    if (folder === undefined) {
      if (live === undefined && selectedPath !== undefined) throw new Error('This chat worked in a folder that is not one of the Bot\'s any more. Add the folder again to continue it.');
      throw new Error('The Bot session source is unavailable. Choose an exact transcript.');
    }
    const projectPath = folder.path;
    if (!transcriptPath && live?.projectPath === projectPath && live.sessionMode) {
      await this.options.runtimes.openSession({ projectPath, sessionId, mode: live.sessionMode }, false, undefined, activate);
      return { botId, projectPath, sessionId, workspaceId: folder.id };
    }
    if (!selectedPath) throw new Error('The Bot session source is unavailable. Choose an exact transcript.');
    this.openableFolder(record, folder.id);
    const row = await this.verifiedSessionSource(botId, sessionId, selectedPath, folder, saved?.mode);
    if (live) {
      if (live.projectPath !== projectPath || live.sessionMode !== row.mode) {
        throw new Error('The Bot session is already owned by a different runtime.');
      }
      if (live.connectionState.status !== 'connected') {
        await this.options.runtimes.openSession({ projectPath, sessionId, mode: row.mode }, false, undefined, activate);
      }
      if (this.options.runtimes.get?.(sessionId) !== live || live.connectionState.status !== 'connected'
        || (await live.getWorkspaceState()).transcript_path !== row.path) {
        throw new Error('The Bot session is already owned by a different runtime.');
      }
    }
    // The session is the Bot's before its engine starts, in whichever folder it works.
    const claimedBefore = this.claims.get(sessionId) === botId;
    this.claimSession(botId, sessionId);
    try {
      await this.options.runtimes.openSession({ projectPath, sessionId: row.uuid, mode: row.mode }, row.empty_session, row.resume_model ?? record.model, activate, row.path);
    } catch (error) {
      if (!claimedBefore && this.options.runtimes.get?.(sessionId) === undefined) this.forgetSession(sessionId);
      throw this.refusal(error);
    }
    this.store.rememberSessionSource(botId, row.uuid, { path: row.path, mode: row.mode });
    return { botId, projectPath, sessionId: row.uuid, workspaceId: folder.id };
  }

  /** The folder of the Bot a transcript in its home belongs to, by the project folder it is kept in. */
  private folderOfTranscript(record: BotRecord, transcriptPath: string): BotWorkspace | undefined {
    if (!isAbsolute(transcriptPath) || transcriptPath.includes('\0')) return undefined;
    const sessionId = basename(transcriptPath).slice(0, -'.jsonl'.length);
    if (!isSessionId(sessionId)) return undefined;
    return this.store.allWorkspaces(record).find((workspace) => join(this.store.transcriptsDirOf(record.bot_id, workspace.path), `${sessionId}.jsonl`).toLowerCase() === transcriptPath.toLowerCase());
  }

  private async verifiedSessionSource(botId: string, sessionId: string, path: string, folder: BotWorkspace, mode?: 'chat' | 'code'): Promise<ProjectSessionCatalogRow> {
    if (!isAbsolute(path) || path.includes('\0') || basename(path).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`) throw new Error('invalid Bot session selection');
    const row = await this.catalog(botId).select(folder.path, sessionId, { scope: 'current', transcriptPath: path });
    if (!row || row.uuid.toLowerCase() !== sessionId.toLowerCase() || row.path !== path || (mode && row.mode !== mode)
      || (row.workspace_path && realOrSame(row.workspace_path) !== realOrSame(folder.path))) throw new Error('The Bot session source is unavailable.');
    return row;
  }

  async resolveSavedTranscript(ref: SessionRef): Promise<{ path: string; mode: 'chat' | 'code' } | undefined> {
    const botId = this.botIdForSession(ref);
    if (!botId || (ref.projectId !== undefined && ref.projectId !== null)) throw new Error('The Bot session owner is unavailable.');
    const saved = this.store.sessionSource(botId, ref.sessionId);
    if (!saved) return undefined;
    if (ref.mode && ref.mode !== saved.mode) throw new Error('The Bot session mode does not match its saved source.');
    const folder = this.folderAt(this.store.get(botId), ref.projectPath);
    if (folder === undefined) throw new Error('The Bot session owner is unavailable.');
    const row = await this.verifiedSessionSource(botId, ref.sessionId, saved.path, folder, saved.mode);
    return { path: row.path, mode: row.mode };
  }

  async captureSessionSource(ref: SessionRef, runtime: SessionRuntime): Promise<string> {
    const botId = this.botIdForSession(ref);
    if (!botId || runtime.projectPath !== ref.projectPath || runtime.sessionId.toLowerCase() !== ref.sessionId.toLowerCase()
      || !runtime.sessionMode || (ref.mode && ref.mode !== runtime.sessionMode)
      || (ref.projectId !== undefined && ref.projectId !== null)) throw new Error('The Bot session owner changed.');
    const folder = this.folderAt(this.store.get(botId), ref.projectPath);
    if (folder === undefined) throw new Error('The Bot session owner changed.');
    const state = await runtime.getWorkspaceState(), path = state.transcript_path;
    if (!path || !isAbsolute(path) || basename(path).toLowerCase() !== `${ref.sessionId.toLowerCase()}.jsonl`
      || realpathSync.native(state.cwd) !== realpathSync.native(ref.projectPath) || !lstatSync(path).isFile()) throw new Error('The Bot session has no verified transcript source.');
    await this.verifiedSessionSource(botId, ref.sessionId, path, folder, runtime.sessionMode);
    if (this.options.runtimes.get?.(ref.sessionId) !== runtime || runtime.projectPath !== ref.projectPath) throw new Error('The Bot session owner changed.');
    this.store.rememberSessionSource(botId, ref.sessionId, { path, mode: runtime.sessionMode });
    runtime.rememberTranscriptSelection(path);
    return path;
  }

  /**
   * The Bot's own sessions, read from its home, in whichever of its folders they worked. A chat of a
   * folder the Bot no longer has is listed with the folder's path and no folder id.
   */
  async listSessions(botId: string): Promise<BotSessionRow[]> {
    this.assertAvailable();
    const record = this.store.get(botId);
    // The owner asked to see them: show what is there now, not what was there a few seconds ago.
    this.catalog(botId).invalidate();
    const own = this.store.workspaceDir(botId);
    const { sessions } = await this.catalog(botId).list(own, 'all');
    const folders = this.store.allWorkspaces(record).map((workspace) => ({ workspace, real: realOrSame(workspace.path) }));
    return sessions
      .filter((row) => !this.options.routines?.isControllerSession(own, row.uuid))
      .map(({ empty_session: _empty, resume_model: _model, workspace_path: workspacePath, ...row }): BotSessionRow => {
        if (workspacePath === undefined) return { ...row, workspaceId: DEFAULT_WORKSPACE_ID };
        const real = realOrSame(workspacePath);
        const folder = folders.find((entry) => entry.real === real);
        return folder !== undefined ? { ...row, workspaceId: folder.workspace.id } : { ...row, removedPath: workspacePath };
      });
  }

  /**
   * When the Bot was last used: the newest modification time among its
   * transcripts, in whichever folder, read from the files rather than from a
   * running engine. The scheduler's hidden controller session is not use of the Bot.
   */
  async lastActivityAt(botId: string): Promise<number | undefined> {
    this.assertAvailable();
    const projects = join(this.store.homeDir(botId), 'projects');
    const workspace = this.store.workspaceDir(botId);
    const files: string[] = [];
    try {
      for (const folder of await readdir(projects)) {
        if (files.length >= MAX_ACTIVITY_FILES) break;
        let names: string[];
        try { names = await readdir(join(projects, folder)); } catch { continue; }
        for (const name of names) {
          if (name.endsWith('.jsonl') && !this.options.routines?.isControllerSession(workspace, name.slice(0, -'.jsonl'.length))) files.push(join(projects, folder, name));
        }
      }
    } catch { return undefined; }
    const times = await Promise.all(files.slice(0, MAX_ACTIVITY_FILES).map(async (file) => {
      try {
        const { mtimeMs } = await stat(file);
        return mtimeMs;
      } catch {
        return undefined;
      }
    }));
    let newest: number | undefined;
    for (const mtimeMs of times) {
      if (mtimeMs !== undefined && (newest === undefined || mtimeMs > newest)) newest = mtimeMs;
    }
    return newest === undefined ? undefined : Math.floor(newest);
  }

  // What the app asks when it launches or looks after a session

  /** The folders of the owner's that the Bot works in besides its own. */
  workspaces(botId: string): readonly BotWorkspace[] {
    return this.store.get(botId).workspaces;
  }

  /** A new chat of the Bot for a remote device: in the folder `workspaceId` names, or in the Bot's own folder if there is none. */
  async startSession(botId: string, workspaceId?: string): Promise<{ sessionId: string }> {
    const ref = await this.openSession(botId, undefined, undefined, true, workspaceId);
    return { sessionId: ref.sessionId };
  }

  /** The working directory of the Bot's own sessions. */
  workspaceOf(botId: string): string {
    return this.store.workspaceDir(botId);
  }

  /** The id of the folder of the Bot that a working directory is, if it is one. */
  workspaceIdAt(botId: string, projectPath: string): string | undefined {
    try { return this.folderAt(this.store.get(botId), projectPath)?.id; } catch { return undefined; }
  }

  /** Every working directory the Bot's sessions have, its own folder first. */
  workspacePathsOf(botId: string): string[] {
    try { return this.store.allWorkspaces(this.store.get(botId)).map((workspace) => workspace.path); } catch { return [this.store.workspaceDir(botId)]; }
  }

  /** The Bot whose own folder a working directory is, if it is one. What a session is, is {@link botIdForSession}'s to say. */
  botIdFor(projectPath: string): string | undefined {
    return this.store.botIdForWorkspace(projectPath);
  }

  /** The model of the Bot a session belongs to, if it has one. */
  modelFor(projectPath: string, sessionId?: string): string | undefined {
    const botId = sessionId === undefined ? this.botIdFor(projectPath) : this.botIdForSession({ projectPath, sessionId });
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

  /** The Bots whose routines run now: not one that is being deleted or cannot be read. */
  routineBots(): RoutineBot[] {
    let entries: ReturnType<BotStore['entries']>;
    try { entries = this.store.entries(); } catch { return []; }
    return entries.flatMap(({ botId, record }) => record !== undefined && !this.deleting.has(botId) ? [this.routineBot(record)] : []);
  }

  /** The Bot whose own folder a working directory is, while its routines can run. */
  routineBotFor(workspace: string): RoutineBot | undefined {
    const botId = this.botIdFor(workspace);
    if (botId === undefined || this.deleting.has(botId)) return undefined;
    try { return this.routineBot(this.store.get(botId)); } catch { return undefined; }
  }

  /** Whether the Bot is being deleted right now. */
  isDeleting(botId: string): boolean {
    return this.deleting.has(botId);
  }

  // What Remote Control asks

  /** Where a Bot stands for remote devices. Never throws: a Bot that cannot be read is not there. */
  remoteBot(botId: string): RemoteBotLookup {
    if (this.deleting.has(botId)) return { state: 'deleting' };
    let record: BotRecord;
    try { record = this.store.get(botId); } catch { return { state: 'unknown' }; }
    return record.remote === true ? { state: 'on', bot: this.remoteBotOf(record) } : { state: 'off' };
  }

  /** The Bots whose Remote Control switch is on, oldest first: not one being deleted or that cannot be read. */
  remoteBots(): RemoteBot[] {
    let entries: ReturnType<BotStore['entries']>;
    try { entries = this.store.entries(); } catch { return []; }
    return entries
      .flatMap(({ botId, record }) => record?.remote === true && !this.deleting.has(botId) ? [record] : [])
      .sort((left, right) => left.created_at - right.created_at || left.bot_id.localeCompare(right.bot_id))
      .map((record) => this.remoteBotOf(record));
  }

  /**
   * The Bot a chat belongs to, if it belongs to one, without waiting for the
   * disk: a chat open in one of the Bot's folders that the Bot opened, else a
   * transcript in a Bot's home. (A chat started from a phone has no transcript
   * until its first prompt.) The hidden session that watches the clock is no chat.
   */
  chat(sessionId: string): BotChat | undefined {
    // Only the spelling the engine writes: the id reaches a file name below.
    if (!isSessionId(sessionId) || sessionId !== sessionId.toLowerCase()) return undefined;
    const project = this.options.openProject?.(sessionId);
    let botId: string | undefined;
    let folder: string | undefined;
    if (project !== undefined) {
      botId = this.botIdForSession({ projectPath: project, sessionId });
      folder = project;
    } else {
      botId = this.store.botIdOfTranscript(sessionId);
      if (botId !== undefined) {
        try { folder = this.store.workspaceOfTranscript(this.store.get(botId), sessionId)?.path; } catch { folder = undefined; }
      }
    }
    if (botId === undefined || folder === undefined) return undefined;
    if (this.options.routines?.isControllerSession(this.store.workspaceDir(botId), sessionId)) return undefined;
    return { botId, ref: { projectPath: folder, sessionId }, remote: this.remoteBot(botId).state === 'on' };
  }

  /** Whether the Bot that works in this folder has its Remote Control switch on. */
  remoteEnabled(workspace: string): boolean {
    const botId = this.botIdFor(workspace);
    return botId !== undefined && this.remoteBot(botId).state === 'on';
  }

  private remoteBotOf(record: BotRecord): RemoteBot {
    return { botId: record.bot_id, name: record.name, description: record.description, workspace: this.store.workspaceDir(record.bot_id) };
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
      catalog: () => this.routineCatalog(botId),
      folders: () => this.routineFolders(botId),
      folderOf: (routineId) => this.routineFolderOf(botId, routineId),
      setFolder: (routineId, workspaceId) => this.store.setRoutineFolder(botId, routineId, workspaceId),
      chosenFolders: () => this.store.routineFolders(botId),
      claim: (sessionId) => this.claimSession(botId, sessionId),
      folderOfChat: (sessionId) => {
        const chat = this.chat(sessionId);
        return chat !== undefined && chat.botId === botId ? chat.ref.projectPath : undefined;
      },
    };
  }

  /** The folders a routine of the Bot may run in: its own, and the ones the owner gave it that are still what they were. */
  private routineFolders(botId: string): Array<RoutineFolder & { label: string }> {
    return this.store.allWorkspaces(this.store.get(botId))
      .filter((folder) => folder.id === DEFAULT_WORKSPACE_ID || stillBound(this.store, folder))
      .map((folder) => ({ id: folder.id, path: folder.path, label: folder.id === DEFAULT_WORKSPACE_ID ? '' : folder.label ?? basename(folder.path) }));
  }

  /** Where a routine of the Bot runs: its own folder unless the owner chose another. `undefined` when the one chosen is not the Bot's any more. */
  private routineFolderOf(botId: string, routineId: string): RoutineFolder | undefined {
    const chosen = this.store.routineFolders(botId)[routineId] ?? DEFAULT_WORKSPACE_ID;
    const folder = this.routineFolders(botId).find((candidate) => candidate.id === chosen);
    return folder === undefined ? undefined : { id: folder.id, path: folder.path };
  }

  /**
   * The chats a routine may target are the Bot's, in whichever of its folders they work now; the
   * scheduler asks for its own folder and is given the whole home.
   */
  private routineCatalog(botId: string): RoutineCatalog {
    const catalog = this.catalog(botId);
    const own = this.store.workspaceDir(botId);
    return {
      list: (_projectPath, _scope, include) => {
        const folders = new Set(this.routineFolders(botId).map((folder) => realOrSame(folder.path)));
        return catalog.list(own, 'all', (row) => (row.workspace_path === undefined || folders.has(realOrSame(row.workspace_path))) && (include?.(row) ?? true));
      },
      find: (projectPath, sessionId) => catalog.find(projectPath, sessionId),
    };
  }

  /**
   * What a session of the Bot is launched with, instead of the settings of the
   * owner's projects. The settings are made current first, so that the engine
   * reads the isolation as it is now. A session that works in a folder the Bot
   * was given is launched with the Bot's configuration home all the same, and
   * with its other folders granted.
   */
  async launchSettings(projectPath: string, sessionId?: string): Promise<WorkspaceLaunchSettings> {
    this.assertAvailable();
    const botId = sessionId === undefined ? this.store.botIdForWorkspace(projectPath) : this.botIdForSession({ projectPath, sessionId });
    if (botId === undefined) throw new Error('not the workspace of a Bot');
    this.assertNotDeleting(botId);
    const record = this.store.get(botId);
    const folder = this.folderAt(record, projectPath);
    if (folder === undefined) throw new Error('This chat\'s folder is not one of the Bot\'s any more.');
    // Every start of an engine for a Bot comes through here: one that would refuse to run is not started at all.
    this.assertSandboxPossible(record);
    const bound = this.writeSettings(record);
    if (folder.id !== DEFAULT_WORKSPACE_ID && !bound.some((workspace) => workspace.id === folder.id)) {
      throw new Error(`${folder.path} is no longer a folder a Bot may work in.`);
    }
    // Its way in to its Local Apps is there before the engine looks for it.
    await this.publishLocalApps(record);
    const home = this.store.homeDir(botId);
    const base = await this.catalog(botId).workspaceSettings(folder.path);
    const globalConfigPath = join(home, '.lingxi.json');
    const grants = this.grantsFor(record, bound, folder.path);
    if (sessionId !== undefined) {
      this.launched.set(sessionId.toLowerCase(), launchKeyOf(record));
      // The runtime manager prefers a session's own grants over these; what an earlier sync left there must not outlive the folders it named.
      this.options.runtimes.setSessionDirectoryGrants(sessionId, grants);
    }
    return {
      // The engine reads some of its configuration from the environment only,
      // and what the Bot's own settings say must not move the home elsewhere.
      environment: { ...base.environment, LINGXI_CONFIG_DIR: home, ...await this.routerEnvironment(botId, sessionId, globalConfigPath) },
      configHome: home,
      globalConfigPath,
      // A bound folder is someone's project: its own settings, hooks and permission rules must not
      // loosen the Bot's sandbox or isolation, so only the Bot's settings apply.
      userSettingsOnly: true,
      additionalDirectories: grants,
    };
  }

  private localAppAudience(record: BotRecord): LocalAppBotAudience {
    return { botId: record.bot_id, home: this.store.homeDir(record.bot_id), apps: record.scope.localApps };
  }

  /** Every Bot and the Local Apps it may use, those with none included: the gateway starts from this, and removes what an earlier run left for the others. */
  localAppAudiences(): LocalAppBotAudience[] {
    if (!this.availability().available) return [];
    return this.store.entries().flatMap((entry) => entry.record === undefined ? [] : [this.localAppAudience(entry.record)]);
  }

  /** The Bots whose scope names the Local App, oldest first. */
  botsReachingLocalApp(appId: string): BotReachUi[] {
    return this.botsWhere((scope) => scope.localApps.includes(appId));
  }

  /** The Bots whose scope names the dataset of the data source, or any dataset of it, oldest first. */
  botsReachingDataset(source: string, dataset?: string): BotReachUi[] {
    return this.botsWhere((scope) => scope.datasets.some((entry) => entry.source === source && (dataset === undefined || entry.dataset === dataset)));
  }

  /** A Bot that cannot be read, or is being deleted, reaches nothing. */
  private botsWhere(reaches: (scope: BotRecord['scope']) => boolean): BotReachUi[] {
    return this.store.entries()
      .flatMap((entry) => entry.record !== undefined && !this.deleting.has(entry.botId) && reaches(entry.record.scope) ? [entry.record] : [])
      .sort((left, right) => left.created_at - right.created_at || left.bot_id.localeCompare(right.bot_id))
      .map((record) => ({ botId: record.bot_id, name: record.name }));
  }

  /** Make the Bot's way in to its Local Apps what its record says. A Bot that cannot be given it still runs, without them. */
  private async publishLocalApps(record: BotRecord): Promise<void> {
    if (this.options.localApps === undefined) return;
    try { await this.options.localApps.publish(this.localAppAudience(record)); } catch (error) {
      this.warn(`Bot ${record.bot_id}: its Local Apps are not available to it: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /**
   * What lets a session's engine reach the Bot router: the router's entry among
   * the MCP servers of the Bot's configuration, and in the session's environment
   * the router's address and the token that speaks for this session. A session
   * that cannot be given them still runs, without the other Bots.
   */
  private async routerEnvironment(botId: string, sessionId: string | undefined, globalConfigPath: string): Promise<Record<string, string>> {
    const { router } = this.options;
    if (router === undefined || sessionId === undefined) return {};
    try {
      const url = await router.ready();
      ensureRouterEntry(globalConfigPath);
      return { [ROUTER_URL_ENV]: url, [ROUTER_TOKEN_ENV]: router.issue(botId, sessionId) };
    } catch (error) {
      this.warn(`Bot ${botId}: the session cannot reach the other Bots: ${error instanceof Error ? error.message : String(error)}`);
      return {};
    }
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
