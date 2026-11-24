/**
 * What the desktop window can do with Bots. Free of Electron; `HostController`
 * registers the handlers, after checking the sender of every call. Whatever
 * the window sends is checked here before it reaches a Bot's files.
 */

import type { PermissionModeId } from '@lingxi/bridge-client';
import { CH_BOTS_MARKET } from '../../shared/botMarket.js';

import {
  CH_BOTS_AVAILABILITY,
  CH_BOTS_CREATE,
  CH_BOTS_DELETE,
  CH_BOTS_GET,
  CH_BOTS_LIST,
  CH_BOTS_LOGIN_ITEM_GET,
  CH_BOTS_LOGIN_ITEM_SET,
  CH_BOTS_LOGIN_ITEMS_OPEN,
  CH_BOTS_MEMORY_DELETE,
  CH_BOTS_MEMORY_LIST,
  CH_BOTS_MEMORY_READ,
  CH_BOTS_MEMORY_WRITE,
  CH_BOTS_PERSONA_GET,
  CH_BOTS_PERSONA_SET,
  CH_BOTS_SESSION_OPEN,
  CH_BOTS_SESSIONS_LIST,
  CH_BOTS_SKILL_DELETE,
  CH_BOTS_SKILL_IMPORT,
  CH_BOTS_SKILLS_IMPORTABLE,
  CH_BOTS_SKILLS_LIST,
  CH_BOTS_TASK_CANCEL,
  CH_BOTS_TASKS_LIST,
  CH_BOTS_UPDATE,
  CH_BOTS_WORKSPACE_REQUEST_DECIDE,
  CH_BOTS_WORKSPACE_REQUESTS_LIST,
  type BotCreateRequest,
  type BotsLoginItemUi,
  type BotSessionRefUi,
  type BotSessionsUi,
  type BotTasksUi,
  type BotUi,
  type BotUpdateRequest,
  type BotWorkspaceInputUi,
  type BotWorkspaceRequestUi,
} from '../../shared/bots.js';
import { isPermissionModeId } from '../host-utils.js';
import { isSessionId } from '../sessionIdentity.js';
import { memoryName, skillName, type BotContent } from './content.js';
import type { BotService } from './service.js';
import { isBotId, type BotAutoReply, type BotRecord, type BotSandbox, type BotScope } from './store.js';
import { isTaskId } from './tasks.js';
import type { WorkspaceRequests } from './workspaceRequests.js';

export type BotsIpcHandler = (...args: unknown[]) => Promise<unknown> | unknown;

/** What of the tasks between Bots the window may see and do. */
export interface BotsUiTasks {
  forOwner(botId: string): BotTasksUi;
  cancelForOwner(botId: string, taskId: string): void;
}

/** What of the app as a login item the window may see and do. */
export interface BotsUiLoginItem {
  status(): BotsLoginItemUi;
  set(enabled: boolean): void;
  /** Show where the owner allows or removes login items. */
  openSettings(): Promise<void>;
}

export interface BotsUiDeps {
  service: BotService;
  content: BotContent;
  tasks: BotsUiTasks;
  loginItem: BotsUiLoginItem;
  /** The folders the Bots have asked for; without it none can be. */
  workspaceRequests?: WorkspaceRequests;
  market?: { handle(request: unknown): Promise<unknown> };
}

function plainObject(value: unknown, allowed: readonly string[]): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error('invalid request');
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw new Error('request contains unsupported fields');
  return value as Record<string, unknown>;
}

function botId(value: unknown): string {
  if (!isBotId(value)) throw new Error('invalid Bot id');
  return value;
}

function taskId(value: unknown): string {
  if (!isTaskId(value)) throw new Error('invalid task id');
  return value;
}

function text(value: unknown, what: string): string {
  if (typeof value !== 'string') throw new Error(`invalid ${what}`);
  return value;
}

function optionalText(value: unknown, what: string): string | undefined {
  return value === undefined ? undefined : text(value, what);
}

function permissionMode(value: unknown): PermissionModeId | undefined {
  if (value === undefined) return undefined;
  if (!isPermissionModeId(value)) throw new Error('invalid permission mode');
  return value;
}

function acceptFrom(value: unknown): string[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.some((id) => !isBotId(id))) throw new Error('invalid accept list');
  return [...value];
}

function workspaces(value: unknown): BotWorkspaceInputUi[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value)) throw new Error('invalid workspaces');
  return value.map((entry) => {
    const workspace = plainObject(entry, ['path', 'mode', 'label']);
    if (workspace['mode'] !== 'read' && workspace['mode'] !== 'write') throw new Error('invalid workspace mode');
    const label = optionalText(workspace['label'], 'workspace label');
    return { path: text(workspace['path'], 'workspace path'), mode: workspace['mode'], ...(label !== undefined ? { label } : {}) };
  });
}

/** An object the Bot store reads strictly: here only that it is one, so that nothing else is read from a stranger's shape. */
function configuration<T>(value: unknown, what: string): T | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${what}`);
  return value as T;
}

function createRequest(value: unknown): BotCreateRequest {
  const request = plainObject(value, ['name', 'description', 'persona', 'model', 'permissionMode', 'acceptFrom', 'workspaces', 'sandbox', 'scope', 'autoReply']);
  const description = optionalText(request['description'], 'description');
  const persona = optionalText(request['persona'], 'persona');
  const model = optionalText(request['model'], 'model');
  const mode = permissionMode(request['permissionMode']);
  const accepted = acceptFrom(request['acceptFrom']);
  const bound = workspaces(request['workspaces']);
  const sandbox = configuration<BotSandbox>(request['sandbox'], 'sandbox');
  const scope = configuration<BotScope>(request['scope'], 'scope');
  const autoReply = configuration<BotAutoReply>(request['autoReply'], 'automatic reply settings');
  return {
    name: text(request['name'], 'name'),
    ...(description !== undefined ? { description } : {}),
    ...(persona !== undefined ? { persona } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(mode !== undefined ? { permissionMode: mode } : {}),
    ...(accepted !== undefined ? { acceptFrom: accepted } : {}),
    ...(bound !== undefined ? { workspaces: bound } : {}),
    ...(sandbox !== undefined ? { sandbox } : {}),
    ...(scope !== undefined ? { scope } : {}),
    ...(autoReply !== undefined ? { autoReply } : {}),
  };
}

function updateRequest(value: unknown): BotUpdateRequest {
  const request = plainObject(value, ['name', 'description', 'model', 'permissionMode', 'acceptFrom', 'remote', 'workspaces', 'sandbox', 'scope', 'autoReply']);
  const name = optionalText(request['name'], 'name');
  const description = optionalText(request['description'], 'description');
  const model = request['model'] === null ? null : optionalText(request['model'], 'model');
  const mode = request['permissionMode'] === null ? null : permissionMode(request['permissionMode']);
  const accepted = acceptFrom(request['acceptFrom']);
  const remote = request['remote'] === undefined ? undefined : flag(request['remote'], 'Remote Control setting');
  const bound = workspaces(request['workspaces']);
  const sandbox = configuration<BotSandbox>(request['sandbox'], 'sandbox');
  const scope = configuration<BotScope>(request['scope'], 'scope');
  const autoReply = configuration<BotAutoReply>(request['autoReply'], 'automatic reply settings');
  return {
    ...(name !== undefined ? { name } : {}),
    ...(description !== undefined ? { description } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(mode !== undefined ? { permissionMode: mode } : {}),
    ...(accepted !== undefined ? { acceptFrom: accepted } : {}),
    ...(remote !== undefined ? { remote } : {}),
    ...(bound !== undefined ? { workspaces: bound } : {}),
    ...(sandbox !== undefined ? { sandbox } : {}),
    ...(scope !== undefined ? { scope } : {}),
    ...(autoReply !== undefined ? { autoReply } : {}),
  };
}

function flag(value: unknown, what: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`invalid ${what}`);
  return value;
}

function revision(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 1) throw new Error('invalid revision');
  return value as number;
}

export function botUi(record: BotRecord, lastActivityAt?: number): BotUi {
  return {
    botId: record.bot_id,
    name: record.name,
    description: record.description,
    createdAt: record.created_at,
    updatedAt: record.updated_at,
    revision: record.revision,
    ...(record.marketOrigin ? { marketOrigin: { ...record.marketOrigin } } : {}),
    ...(record.model !== undefined ? { model: record.model } : {}),
    ...(record.permission_mode !== undefined ? { permissionMode: record.permission_mode } : {}),
    ...(record.accept_from !== undefined ? { acceptFrom: [...record.accept_from] } : {}),
    ...(record.remote === true ? { remote: true } : {}),
    workspaces: record.workspaces.map((workspace) => ({
      id: workspace.id,
      path: workspace.path,
      mode: workspace.mode,
      ...(workspace.label !== undefined ? { label: workspace.label } : {}),
      addedAt: workspace.addedAt,
    })),
    sandbox: { enabled: record.sandbox.enabled, allowedDomains: [...record.sandbox.allowedDomains] },
    scope: { localApps: [...record.scope.localApps], datasets: record.scope.datasets.map((dataset) => ({ ...dataset })) },
    autoReply: {
      ...record.autoReply,
      keywords: [...record.autoReply.keywords],
      thresholds: { ...record.autoReply.thresholds },
      ...(record.autoReply.quietHours !== undefined ? { quietHours: { ...record.autoReply.quietHours } } : {}),
    },
    ...(lastActivityAt !== undefined ? { lastActivityAt } : {}),
  };
}

export class BotsUi {
  constructor(private readonly deps: BotsUiDeps) {}

  /** Whether a working directory is the own folder of a Bot, and whose. */
  botIdFor(projectPath: string): string | undefined {
    return this.deps.service.botIdFor(projectPath);
  }

  /** The Bot a session is the chat of, in whichever of the Bot's folders it works. */
  botIdOfSession(ref: { projectPath: string; sessionId: string }): string | undefined {
    return this.deps.service.botIdForSession(ref);
  }

  /** The Bot's own working directory; absent for anything that is not a Bot's id. */
  workspaceOf(id: string): string | undefined {
    return isBotId(id) ? this.deps.service.workspaceOf(id) : undefined;
  }

  /** The id of the folder of the Bot that a working directory is, if it is one. */
  workspaceIdAt(id: string, projectPath: string): string | undefined {
    return isBotId(id) ? this.deps.service.workspaceIdAt(id, projectPath) : undefined;
  }

  /** Add a folder the owner picked to a Bot. It starts read-only; the owner changes that in the Bot's settings. */
  async addWorkspace(id: string, path: string, mode: 'read' | 'write' = 'read'): Promise<BotUi> {
    const record = await this.deps.service.addWorkspace(botId(id), { path, mode });
    return botUi(record, await this.deps.service.lastActivityAt(record.bot_id));
  }

  /** Every working directory of a Bot, its own first; none for anything that is not a Bot's id. */
  workspacePathsOf(id: string): string[] {
    return isBotId(id) ? this.deps.service.workspacePathsOf(id) : [];
  }

  /**
   * A new session of the Bot, or the one named, started the way the window asks
   * for it. The host calls this too, to open the session of a routine's run.
   */
  async openSession(id: unknown, sessionId?: unknown, transcriptPath?: unknown, activate = true, workspaceId?: unknown): Promise<BotSessionRefUi> {
    if (sessionId !== undefined && !isSessionId(sessionId)) throw new Error('invalid session id');
    if (transcriptPath !== undefined && typeof transcriptPath !== 'string') throw new Error('invalid Bot session selection');
    if (workspaceId !== undefined && (typeof workspaceId !== 'string' || !/^[A-Za-z0-9_-]{1,32}$/.test(workspaceId))) throw new Error('invalid folder');
    return this.deps.service.openSession(botId(id), sessionId, transcriptPath, activate, workspaceId);
  }

  /** What the Bots have asked for and the owner has not answered, as the window shows it. */
  workspaceRequests(id?: unknown): BotWorkspaceRequestUi[] {
    const requests = this.deps.workspaceRequests;
    if (requests === undefined) return [];
    return requests.pending(id === undefined ? undefined : botId(id)).flatMap((request) => {
      let name: string;
      try { name = this.deps.service.get(request.botId).name; } catch { return []; }
      return [{ requestId: request.id, botId: request.botId, botName: name, path: request.path, mode: request.mode, reason: request.reason, createdAt: request.createdAt }];
    });
  }

  /** The invoke handlers by channel. */
  handlers(): Record<string, BotsIpcHandler> {
    const { service, content, tasks, loginItem } = this.deps;
    const present = async (record: BotRecord): Promise<BotUi> => botUi(record, await service.lastActivityAt(record.bot_id));
    const handlers: Record<string, BotsIpcHandler> = {
      [CH_BOTS_MARKET]: request => { if (!this.deps.market) throw new Error('Bot market unavailable'); return this.deps.market.handle(request); },
      [CH_BOTS_LIST]: async (): Promise<BotUi[]> => Promise.all(service.list().map(present)),
      [CH_BOTS_GET]: (id): Promise<BotUi> => present(service.get(botId(id))),
      [CH_BOTS_CREATE]: (request): Promise<BotUi> => present(service.create(createRequest(request))),
      [CH_BOTS_UPDATE]: async (id, expectedRevision, patch): Promise<BotUi> => present(await service.update(botId(id), revision(expectedRevision), updateRequest(patch))),
      [CH_BOTS_DELETE]: async (id): Promise<void> => { await service.delete(botId(id)); },
      [CH_BOTS_SESSION_OPEN]: (id, sessionId, transcriptPath, workspaceId): Promise<BotSessionRefUi> => this.openSession(id, sessionId, transcriptPath, true, workspaceId),
      [CH_BOTS_SESSIONS_LIST]: async (id): Promise<BotSessionsUi> => ({ sessions: await service.listSessions(botId(id)) }),
      [CH_BOTS_PERSONA_GET]: (id) => content.persona(botId(id)),
      [CH_BOTS_PERSONA_SET]: async (id, persona): Promise<void> => { await content.setPersona(botId(id), text(persona, 'persona')); },
      [CH_BOTS_MEMORY_LIST]: (id) => content.listMemory(botId(id)),
      [CH_BOTS_MEMORY_READ]: (id, name) => content.readMemory(botId(id), memoryName(name)),
      [CH_BOTS_MEMORY_WRITE]: async (id, name, body): Promise<void> => { await content.writeMemory(botId(id), memoryName(name), text(body, 'memory content')); },
      [CH_BOTS_MEMORY_DELETE]: async (id, name): Promise<void> => { await content.deleteMemory(botId(id), memoryName(name)); },
      [CH_BOTS_SKILLS_LIST]: (id) => content.listSkills(botId(id)),
      [CH_BOTS_SKILL_DELETE]: async (id, name): Promise<void> => { await content.deleteSkill(botId(id), skillName(name)); },
      [CH_BOTS_SKILL_IMPORT]: (id, name) => content.importUserSkill(botId(id), skillName(name)),
      [CH_BOTS_SKILLS_IMPORTABLE]: () => content.listImportableSkills(),
      [CH_BOTS_TASKS_LIST]: (id): BotTasksUi => tasks.forOwner(botId(id)),
      [CH_BOTS_TASK_CANCEL]: (id, task): void => { tasks.cancelForOwner(botId(id), taskId(task)); },
      [CH_BOTS_WORKSPACE_REQUESTS_LIST]: (id): BotWorkspaceRequestUi[] => this.workspaceRequests(id),
      [CH_BOTS_WORKSPACE_REQUEST_DECIDE]: async (id, requestId, decision): Promise<BotUi> => {
        const requests = this.deps.workspaceRequests;
        if (requests === undefined) throw new Error('Folder requests are unavailable.');
        if (typeof requestId !== 'string' || requestId.length === 0 || requestId.length > 64) throw new Error('invalid request id');
        if (decision !== 'approve' && decision !== 'deny') throw new Error('invalid decision');
        await requests.decide(botId(id), requestId, decision);
        return present(service.get(botId(id)));
      },
      [CH_BOTS_LOGIN_ITEM_GET]: (): BotsLoginItemUi => loginItem.status(),
      [CH_BOTS_LOGIN_ITEM_SET]: (enabled): BotsLoginItemUi => {
        loginItem.set(flag(enabled, 'login item setting'));
        return loginItem.status();
      },
      [CH_BOTS_LOGIN_ITEMS_OPEN]: (): Promise<void> => loginItem.openSettings(),
    };
    // Where Bots cannot run, nothing but the question whether they can is answered.
    return {
      [CH_BOTS_AVAILABILITY]: () => service.availability(),
      ...Object.fromEntries(Object.entries(handlers).map(([channel, handler]): [string, BotsIpcHandler] => [
        channel,
        (...args) => { service.assertAvailable(); return handler(...args); },
      ])),
    };
  }
}
