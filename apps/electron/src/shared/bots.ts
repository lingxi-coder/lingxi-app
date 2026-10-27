/**
 * What the desktop window may know of and ask of Desktop Bots: the Bots
 * themselves, their persona, memory and skills, and the sessions they run.
 * Plain data only; the main process validates everything it is sent and never
 * returns a path outside the Bot's own folder other than the folders the owner
 * bound to it.
 */

import type { SessionRowDto } from '@lingxi/bridge-client';

/** Invoke → `BotUi[]`. */
export const CH_BOTS_LIST = 'lingxi:bots:list';
/** Invoke(`botId: string`) → `BotUi`. */
export const CH_BOTS_GET = 'lingxi:bots:get';
/** Invoke(`BotCreateRequest`) → `BotUi`. */
export const CH_BOTS_CREATE = 'lingxi:bots:create';
/** Invoke(`botId: string, expectedRevision: number, BotUpdateRequest`) → `BotUi`. Rejects when the Bot changed since `expectedRevision`. */
export const CH_BOTS_UPDATE = 'lingxi:bots:update';
/** Invoke(`botId: string`) → `void`. Stops the Bot's sessions, then moves its folder to the trash folder. */
export const CH_BOTS_DELETE = 'lingxi:bots:delete';
/** Invoke(`botId: string, sessionId?: string`) → `BotSessionRefUi`. Without `sessionId` a new session starts. */
export const CH_BOTS_SESSION_OPEN = 'lingxi:bots:sessions:open';
/** Invoke(`botId: string`) → `BotSessionsUi`. */
export const CH_BOTS_SESSIONS_LIST = 'lingxi:bots:sessions:list';
/** Invoke(`botId: string`) → `string`. */
export const CH_BOTS_PERSONA_GET = 'lingxi:bots:persona:get';
/** Invoke(`botId: string, text: string`) → `void`. */
export const CH_BOTS_PERSONA_SET = 'lingxi:bots:persona:set';
/** Invoke(`botId: string`) → `BotMemoryEntryUi[]`. */
export const CH_BOTS_MEMORY_LIST = 'lingxi:bots:memory:list';
/** Invoke(`botId: string, name: string`) → `string`. */
export const CH_BOTS_MEMORY_READ = 'lingxi:bots:memory:read';
/** Invoke(`botId: string, name: string, content: string`) → `void`. */
export const CH_BOTS_MEMORY_WRITE = 'lingxi:bots:memory:write';
/** Invoke(`botId: string, name: string`) → `void`. */
export const CH_BOTS_MEMORY_DELETE = 'lingxi:bots:memory:delete';
/** Invoke(`botId: string`) → `BotSkillUi[]`. */
export const CH_BOTS_SKILLS_LIST = 'lingxi:bots:skills:list';
/** Invoke(`botId: string, name: string`) → `void`. */
export const CH_BOTS_SKILL_DELETE = 'lingxi:bots:skills:delete';
/** Invoke(`botId: string, name: string`) → `BotSkillUi`. Copies `~/.lingxi/skills/<name>` into the Bot once; the copy is not kept in step. */
export const CH_BOTS_SKILL_IMPORT = 'lingxi:bots:skills:import';

export type BotWorkspaceMode = 'read' | 'write';

/** A folder of the owner's that the Bot may read, or read and change. */
export interface BotWorkspaceUi {
  path: string;
  mode: BotWorkspaceMode;
}

export interface BotUi {
  botId: string;
  name: string;
  description: string;
  /** Epoch milliseconds. */
  createdAt: number;
  updatedAt: number;
  /** Pass this back to `update`; it changes with every change to the Bot. */
  revision: number;
  /** Absent: the Bot uses the model new sessions use. */
  model?: string;
  workspaces: BotWorkspaceUi[];
}

export interface BotCreateRequest {
  name: string;
  description?: string;
  /** The first text of the Bot's `LINGXI.md`. */
  persona?: string;
  model?: string;
  workspaces?: BotWorkspaceUi[];
}

export interface BotUpdateRequest {
  name?: string;
  description?: string;
  /** `null` goes back to the default model. */
  model?: string | null;
  /** Replaces the whole list. */
  workspaces?: BotWorkspaceUi[];
}

/** Enough to attach the window's session view to a Bot's session. */
export interface BotSessionRefUi {
  botId: string;
  projectPath: string;
  sessionId: string;
}

export interface BotSessionsUi {
  sessions: SessionRowDto[];
}

export interface BotMemoryEntryUi {
  /** The file name, including `.md`. */
  name: string;
  /** From the file's front matter. */
  description?: string;
  type?: string;
  size: number;
  /** Epoch milliseconds. */
  modifiedAt: number;
}

export interface BotSkillUi {
  name: string;
  /** From the `SKILL.md` front matter. */
  description?: string;
}

export interface BotsApi {
  list(): Promise<BotUi[]>;
  get(botId: string): Promise<BotUi>;
  create(request: BotCreateRequest): Promise<BotUi>;
  update(botId: string, expectedRevision: number, patch: BotUpdateRequest): Promise<BotUi>;
  delete(botId: string): Promise<void>;
  openSession(botId: string, sessionId?: string): Promise<BotSessionRefUi>;
  listSessions(botId: string): Promise<BotSessionsUi>;
  persona: {
    get(botId: string): Promise<string>;
    set(botId: string, text: string): Promise<void>;
  };
  memory: {
    list(botId: string): Promise<BotMemoryEntryUi[]>;
    read(botId: string, name: string): Promise<string>;
    write(botId: string, name: string, content: string): Promise<void>;
    delete(botId: string, name: string): Promise<void>;
  };
  skills: {
    list(botId: string): Promise<BotSkillUi[]>;
    delete(botId: string, name: string): Promise<void>;
    importFromUser(botId: string, name: string): Promise<BotSkillUi>;
  };
}
