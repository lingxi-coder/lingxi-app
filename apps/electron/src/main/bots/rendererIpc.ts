/**
 * What the desktop window can do with Bots. Free of Electron; `HostController`
 * registers the handlers, after checking the sender of every call. Whatever
 * the window sends is checked here before it reaches a Bot's files.
 */

import type { PermissionModeId } from '@lingxi/bridge-client';

import {
  CH_BOTS_AVAILABILITY,
  CH_BOTS_CREATE,
  CH_BOTS_DELETE,
  CH_BOTS_GET,
  CH_BOTS_LIST,
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
  CH_BOTS_UPDATE,
  type BotCreateRequest,
  type BotSessionRefUi,
  type BotSessionsUi,
  type BotUi,
  type BotUpdateRequest,
  type BotWorkspaceUi,
} from '../../shared/bots.js';
import { isPermissionModeId } from '../host-utils.js';
import { isSessionId } from '../sessionIdentity.js';
import { memoryName, skillName, type BotContent } from './content.js';
import type { BotService } from './service.js';
import { isBotId, type BotRecord } from './store.js';

export type BotsIpcHandler = (...args: unknown[]) => Promise<unknown> | unknown;

export interface BotsUiDeps {
  service: BotService;
  content: BotContent;
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

function workspaces(value: unknown): BotWorkspaceUi[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value)) throw new Error('invalid workspaces');
  return value.map((entry) => {
    const workspace = plainObject(entry, ['path', 'mode']);
    if (workspace['mode'] !== 'read' && workspace['mode'] !== 'write') throw new Error('invalid workspace mode');
    return { path: text(workspace['path'], 'workspace path'), mode: workspace['mode'] };
  });
}

function createRequest(value: unknown): BotCreateRequest {
  const request = plainObject(value, ['name', 'description', 'persona', 'model', 'permissionMode', 'workspaces']);
  const description = optionalText(request['description'], 'description');
  const persona = optionalText(request['persona'], 'persona');
  const model = optionalText(request['model'], 'model');
  const mode = permissionMode(request['permissionMode']);
  const bound = workspaces(request['workspaces']);
  return {
    name: text(request['name'], 'name'),
    ...(description !== undefined ? { description } : {}),
    ...(persona !== undefined ? { persona } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(mode !== undefined ? { permissionMode: mode } : {}),
    ...(bound !== undefined ? { workspaces: bound } : {}),
  };
}

function updateRequest(value: unknown): BotUpdateRequest {
  const request = plainObject(value, ['name', 'description', 'model', 'permissionMode', 'workspaces']);
  const name = optionalText(request['name'], 'name');
  const description = optionalText(request['description'], 'description');
  const model = request['model'] === null ? null : optionalText(request['model'], 'model');
  const mode = request['permissionMode'] === null ? null : permissionMode(request['permissionMode']);
  const bound = workspaces(request['workspaces']);
  return {
    ...(name !== undefined ? { name } : {}),
    ...(description !== undefined ? { description } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(mode !== undefined ? { permissionMode: mode } : {}),
    ...(bound !== undefined ? { workspaces: bound } : {}),
  };
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
    ...(record.model !== undefined ? { model: record.model } : {}),
    ...(record.permission_mode !== undefined ? { permissionMode: record.permission_mode } : {}),
    workspaces: record.workspaces.map((workspace) => ({ path: workspace.path, mode: workspace.mode })),
    ...(lastActivityAt !== undefined ? { lastActivityAt } : {}),
  };
}

export class BotsUi {
  constructor(private readonly deps: BotsUiDeps) {}

  /** Whether a working directory is the workspace of a Bot, and whose. */
  botIdFor(projectPath: string): string | undefined {
    return this.deps.service.botIdFor(projectPath);
  }

  /** The working directory of a Bot's sessions; absent for anything that is not a Bot's id. */
  workspaceOf(id: string): string | undefined {
    return isBotId(id) ? this.deps.service.workspaceOf(id) : undefined;
  }

  /** The invoke handlers by channel. */
  handlers(): Record<string, BotsIpcHandler> {
    const { service, content } = this.deps;
    const present = async (record: BotRecord): Promise<BotUi> => botUi(record, await service.lastActivityAt(record.bot_id));
    const handlers: Record<string, BotsIpcHandler> = {
      [CH_BOTS_LIST]: async (): Promise<BotUi[]> => Promise.all(service.list().map(present)),
      [CH_BOTS_GET]: (id): Promise<BotUi> => present(service.get(botId(id))),
      [CH_BOTS_CREATE]: (request): Promise<BotUi> => present(service.create(createRequest(request))),
      [CH_BOTS_UPDATE]: (id, expectedRevision, patch): Promise<BotUi> => present(service.update(botId(id), revision(expectedRevision), updateRequest(patch))),
      [CH_BOTS_DELETE]: async (id): Promise<void> => { await service.delete(botId(id)); },
      [CH_BOTS_SESSION_OPEN]: async (id, sessionId): Promise<BotSessionRefUi> => {
        if (sessionId !== undefined && !isSessionId(sessionId)) throw new Error('invalid session id');
        return service.openSession(botId(id), sessionId);
      },
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
