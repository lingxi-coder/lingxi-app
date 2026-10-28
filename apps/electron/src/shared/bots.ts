/**
 * What the desktop window may know of and ask of Desktop Bots: the Bots
 * themselves, their persona, memory and skills, and the sessions they run.
 * Plain data only; the main process validates everything it is sent and never
 * returns a path outside the Bot's own folder other than the folders the owner
 * bound to it.
 */

import type { PermissionModeId, SessionRowDto } from '@lingxi/bridge-client';

/**
 * What a rejected update says when the Bot changed since the window read it. The
 * window tells this apart from other failures by the text: an error that crosses
 * the IPC boundary keeps its message and loses its class.
 */
export const BOT_CONFLICT_MESSAGE = 'This Bot changed. Reload and try again.';

/** Invoke → `BotsAvailability`. The only channel that answers when Bots cannot be used here. */
export const CH_BOTS_AVAILABILITY = 'lingxi:bots:availability';
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
/**
 * Event(`BotSessionRefUi`). The host opened a session of a Bot on its own (the owner clicked
 * a notification) and made it the one the window shows; the window follows.
 */
export const CH_BOTS_SESSION_SHOWN = 'lingxi:bots:sessions:shown';
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
/** Invoke → `BotSkillUi[]`. The skills of the owner's own configuration that can be copied into a Bot. */
export const CH_BOTS_SKILLS_IMPORTABLE = 'lingxi:bots:skills:importable';
/** Invoke(`botId: string`) → `BotTasksUi`. The tasks the Bot was given and the ones it handed to other Bots. */
export const CH_BOTS_TASKS_LIST = 'lingxi:bots:tasks:list';
/** Invoke(`botId: string, taskId: string`) → `void`. The owner stops a task of the Bot that is still going on; one that has ended is left as it is. */
export const CH_BOTS_TASK_CANCEL = 'lingxi:bots:tasks:cancel';
/**
 * Event(`BotTasksChangedUi`). Tasks of these Bots changed. Sent once a burst of
 * changes has settled, so the window reads the list again instead of polling it.
 */
export const CH_BOTS_TASKS_CHANGED = 'lingxi:bots:tasks:changed';
/**
 * Event(`BotPromptSentUi`). The host sent a prompt to a session of a Bot on its own (a task
 * was handed to it). The engine's events carry no user prompts, so the window shows the row itself.
 */
export const CH_BOTS_PROMPT_SENT = 'lingxi:bots:prompt:sent';
/** Invoke → `BotsLoginItemUi`. Whether the system opens the app in the background when the owner logs in, read from the system each time. */
export const CH_BOTS_LOGIN_ITEM_GET = 'lingxi:bots:login-item:get';
/** Invoke(`enabled: boolean`) → `BotsLoginItemUi`. Turns the login item on or off and answers how the system has it now. Rejects where the app cannot be a login item. */
export const CH_BOTS_LOGIN_ITEM_SET = 'lingxi:bots:login-item:set';
/** Invoke → `void`. Opens the Login Items pane of System Settings, where the owner allows the app. */
export const CH_BOTS_LOGIN_ITEMS_OPEN = 'lingxi:bots:login-item:open-settings';

/** Whether Bots can be used on this computer, and if not, why (in words fit for the owner). */
export interface BotsAvailability {
  available: boolean;
  reason?: string;
}

/** The app as a login item, as the system has it. */
export interface BotsLoginItemUi {
  /** Only the installed app on a Mac can be one. */
  available: boolean;
  /** The system opens the app when the owner logs in. */
  enabled: boolean;
  /** The app is registered, but the owner has yet to allow it in System Settings → General → Login Items. */
  needsApproval: boolean;
}

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
  /** What a session of the Bot starts in. Absent: the engine's own default, which asks before it acts. */
  permissionMode?: PermissionModeId;
  /** The ids of the Bots that may delegate tasks to this one. Absent: none may. */
  acceptFrom?: string[];
  workspaces: BotWorkspaceUi[];
  /** Epoch milliseconds; absent while the Bot has had no session. */
  lastActivityAt?: number;
}

export interface BotCreateRequest {
  name: string;
  description?: string;
  /** The first text of the Bot's `LINGXI.md`. */
  persona?: string;
  model?: string;
  permissionMode?: PermissionModeId;
  acceptFrom?: string[];
  workspaces?: BotWorkspaceUi[];
}

export interface BotUpdateRequest {
  name?: string;
  description?: string;
  /** `null` goes back to the default model. */
  model?: string | null;
  /** `null` goes back to the engine's default. */
  permissionMode?: PermissionModeId | null;
  /** Replaces the whole list; an empty list lets no Bot delegate to this one. */
  acceptFrom?: string[];
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

/** The states of a task between Bots, those of the A2A protocol. */
export type BotTaskStateUi = 'submitted' | 'working' | 'input-required' | 'completed' | 'failed' | 'canceled' | 'rejected';

/** Whether the Bot whose tasks are listed was given the task or handed it out. */
export type BotTaskDirection = 'received' | 'sent';

export interface BotTaskFileUi {
  name: string;
  /** Relative to the workspace of the Bot the file was copied to: `inbox/<task id>/<name>`. */
  path: string;
  size: number;
}

/** One task, as the owner reads it on the page of one of the two Bots it is between. */
export interface BotTaskUi {
  taskId: string;
  direction: BotTaskDirection;
  /** The other Bot of the task: the one that sent it to this Bot, or the one this Bot sent it to. */
  peer: { botId: string; name?: string };
  state: BotTaskStateUi;
  /** The first line of what was asked, kept short. */
  summary: string;
  /** Why the task is in this state, when that is not obvious. One line. */
  reason?: string;
  /** What the receiving Bot ended with, cut short when it is long. */
  result?: string;
  resultTruncated?: boolean;
  /** The files the receiving Bot attached, copied to the sending Bot's workspace. */
  artifacts: BotTaskFileUi[];
  costUsd?: number;
  /** Epoch milliseconds. */
  createdAt: number;
  updatedAt: number;
  deadlineAt: number;
  /** The session of the receiving Bot that does the task. It has a chat to open once it has taken a turn. */
  receiver: { botId: string; sessionId: string };
  turns: number;
}

export interface BotTasksUi {
  /** Newest first. */
  tasks: BotTaskUi[];
  /** Older tasks were left out. */
  truncated: boolean;
  /** How many of all the Bot's tasks, listed or not, are still going on. Deleting the Bot cancels them. */
  active: { received: number; sent: number };
}

export interface BotTasksChangedUi {
  botIds: string[];
}

export interface BotPromptSentUi {
  sessionId: string;
  text: string;
}

export interface BotsApi {
  /** Never rejects for a computer that cannot run Bots: that is the answer. */
  availability(): Promise<BotsAvailability>;
  list(): Promise<BotUi[]>;
  get(botId: string): Promise<BotUi>;
  create(request: BotCreateRequest): Promise<BotUi>;
  update(botId: string, expectedRevision: number, patch: BotUpdateRequest): Promise<BotUi>;
  delete(botId: string): Promise<void>;
  openSession(botId: string, sessionId?: string): Promise<BotSessionRefUi>;
  /** Called when the host made a session of a Bot the displayed one without being asked by the window. */
  onSessionShown(callback: (session: BotSessionRefUi) => void): () => void;
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
    /** What `importFromUser` can copy: the skills of the owner's configuration, whatever the Bot has. */
    listImportable(): Promise<BotSkillUi[]>;
  };
  tasks: {
    list(botId: string): Promise<BotTasksUi>;
    cancel(botId: string, taskId: string): Promise<void>;
    /** Called, after a burst of changes, with the Bots whose tasks changed. */
    onChanged(callback: (change: BotTasksChangedUi) => void): () => void;
  };
  /** Called when the host sent a prompt to a session of a Bot on its own. */
  onPromptSent(callback: (prompt: BotPromptSentUi) => void): () => void;
  /** Opening the app in the background at login. The system is the source of truth, so the window reads it again whenever it looks. */
  loginItem: {
    status(): Promise<BotsLoginItemUi>;
    /** Rejects where the app cannot be a login item. */
    set(enabled: boolean): Promise<BotsLoginItemUi>;
    /** Opens the Login Items pane of System Settings. */
    openSettings(): Promise<void>;
  };
}
