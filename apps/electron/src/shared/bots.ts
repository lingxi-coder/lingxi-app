/**
 * What the desktop window may know of and ask of Desktop Bots: the Bots
 * themselves, their persona, memory and skills, and the sessions they run.
 * Plain data only; the main process validates everything it is sent and never
 * returns a path outside the Bot's own folder other than the folders the owner
 * bound to it.
 */

import type { PermissionModeId, SessionRowDto } from '@lingxi/bridge-client';
import type { BotMarketOrigin, BotMarketRequest, BotMarketResponse } from './botMarket.js';

/**
 * What a rejected update says when the Bot changed since the window read it. The
 * window tells this apart from other failures by the text: an error that crosses
 * the IPC boundary keeps its message and loses its class.
 */
export const BOT_CONFLICT_MESSAGE = 'This Bot changed. Reload and try again.';

/**
 * What a Bot that cannot be made or started says when its sandbox is on and this computer cannot provide
 * one. The window tells this apart from other failures by the text, as it does a conflict.
 */
export const BOT_SANDBOX_UNAVAILABLE_MESSAGE = "This device can't isolate this Bot.";

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
/**
 * Invoke(`botId: string, sessionId?: string, transcriptPath?: string, workspaceId?: string`) → `BotSessionRefUi`.
 * Without `sessionId` a new session starts, in the folder `workspaceId` names (the Bot's own folder if it is left out).
 */
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
/** Invoke(`botId?: string`) → `BotWorkspaceRequestUi[]`. What the Bots have asked to be given and the owner has not answered, newest last; of one Bot, or of all. */
export const CH_BOTS_WORKSPACE_REQUESTS_LIST = 'lingxi:bots:workspace-requests:list';
/** Invoke(`botId: string, requestId: string, decision: 'approve' | 'deny'`) → `BotUi`. The owner answers a Bot's request for a folder; approving binds the folder to the Bot. */
export const CH_BOTS_WORKSPACE_REQUEST_DECIDE = 'lingxi:bots:workspace-requests:decide';
/** Event(`BotWorkspaceRequestsChangedUi`). A Bot asked for a folder, or a request was answered or expired. The window reads the requests again. */
export const CH_BOTS_WORKSPACE_REQUESTS_CHANGED = 'lingxi:bots:workspace-requests:changed';
/** Invoke → `BotsLoginItemUi`. Whether the system opens the app in the background when the owner logs in, read from the system each time. */
export const CH_BOTS_LOGIN_ITEM_GET = 'lingxi:bots:login-item:get';
/** Invoke(`enabled: boolean`) → `BotsLoginItemUi`. Turns the login item on or off and answers how the system has it now. Rejects where the app cannot be a login item. */
export const CH_BOTS_LOGIN_ITEM_SET = 'lingxi:bots:login-item:set';
/** Invoke → `void`. Opens the Login Items pane of System Settings, where the owner allows the app. */
export const CH_BOTS_LOGIN_ITEMS_OPEN = 'lingxi:bots:login-item:open-settings';
/** Invoke(`appId: string`) → `BotReachUi[]`. The Bots whose scope names this Local App, read from their records on this computer. */
export const CH_BOTS_REACH_LOCAL_APP = 'lingxi:bots:reach:local-app';
/** Invoke(`source: string, dataset?: string`) → `BotReachUi[]`. The Bots whose scope names this data source, or this dataset of it. */
export const CH_BOTS_REACH_DATASET = 'lingxi:bots:reach:dataset';

/**
 * Whether this computer can confine the commands of a Bot's shell to its folders. When it cannot, `reason` is the
 * key of the text that says why: the window shows it, and a Bot's sandbox can only be switched off.
 */
export type BotSandboxAvailability = { available: true } | { available: false; reason: string };

/**
 * Whether Bots can be used here, and with the sandbox or not. They are unavailable on Windows, and to a window
 * that has no host to ask or whose host did not answer.
 */
export type BotsAvailability =
  | { available: true; sandbox: BotSandboxAvailability }
  | { available: false; reason?: string };

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

/** What the owner gives to bind a folder: one of theirs that the Bot may read, or read and change. */
export interface BotWorkspaceInputUi {
  path: string;
  mode: BotWorkspaceMode;
  /** What the owner calls the folder; absent: its name. */
  label?: string;
}

/** A folder as the Bot has it. */
export interface BotWorkspaceUi extends BotWorkspaceInputUi {
  /** Names the folder for as long as it is bound, and the same whenever it is bound again. */
  id: string;
  /** Epoch milliseconds. */
  addedAt: number;
}

/** The id of the Bot's own folder, which every Bot has and none gives up. It is not among the folders of a `BotUi`. */
export const BOT_DEFAULT_WORKSPACE_ID = 'default';

/** Whether the Bot's commands run in the engine's sandbox, and which hosts they may reach. */
export interface BotSandboxUi {
  enabled: boolean;
  allowedDomains: string[];
}

/** How many hosts a Bot's commands may be allowed to reach. */
export const BOT_ALLOWED_DOMAINS_MAX = 64;
const DOMAIN_LABEL = '[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?';
const DOMAIN = new RegExp(`^(?:\\*\\.)?${DOMAIN_LABEL}(?:\\.${DOMAIN_LABEL})*$`);

/**
 * A host a Bot's commands may reach, as the engine reads it: a name (`example.com`), or every name below one
 * (`*.example.com`), in lower case, with no scheme, port or path. `undefined` for anything else.
 */
export function allowedDomain(text: string): string | undefined {
  const name = text.trim().toLowerCase();
  return name.length > 0 && name.length <= 253 && DOMAIN.test(name) ? name : undefined;
}

export interface BotDatasetScopeUi {
  source: string;
  dataset: string;
  write: boolean;
  reportReview: boolean;
}

/** What of the owner's Local Apps and datasets the Bot may reach. */
export interface BotScopeUi {
  localApps: string[];
  datasets: BotDatasetScopeUi[];
}

/** A Bot that may reach something of the owner's, as the page of that thing lists it: who it is, and nothing of its scope. */
export interface BotReachUi {
  botId: string;
  name: string;
}

export type BotReplyApprovalUi = 'never' | 'uncertain' | 'always';

/** What an automatic-reply setting may hold: the store checks these on disk, the editor before it sends. */
export const BOT_REPLY_KEYWORDS_MAX = 32;
export const BOT_REPLY_KEYWORD_CHARS_MAX = 64;
export const BOT_REPLY_POLICY_CHARS_MAX = 4000;
/** How much of the policy the Decision service is shown: the rest is the maintainer's to cut. */
export const BOT_REPLY_POLICY_USED_CHARS = 1000;
export const BOT_REPLY_COOLDOWN_SEC_MAX = 24 * 60 * 60;
/** `HH:MM`, 24-hour. */
export const BOT_CLOCK_TIME = /^([01]\d|2[0-3]):[0-5]\d$/;
/** Whether `value` names a time zone this runtime knows: a short line of text that `Intl` accepts. */
export function validTimeZone(value: unknown): value is string {
  // eslint-disable-next-line no-control-regex
  if (typeof value !== 'string' || value.length === 0 || value.length > 64 || /[\u0000-\u001f\u007f]/.test(value)) return false;
  try { new Intl.DateTimeFormat('en-US', { timeZone: value }); return true; } catch { return false; }
}
/** The thresholds a Bot starts with: below the first it stays silent, from the second it answers. */
export const BOT_REPLY_THRESHOLDS_DEFAULT: Readonly<{ decline: number; run: number }> = { decline: 0.3, run: 0.75 };

export interface BotQuietHoursUi {
  /** `HH:MM`, 24-hour. */
  start: string;
  end: string;
  /** An IANA time zone name. */
  timeZone: string;
}

/** How the Bot answers in the channels it was published to. */
export interface BotAutoReplyUi {
  policy: string;
  keywords: string[];
  quietHours?: BotQuietHoursUi;
  cooldownSec: number;
  thresholds: { decline: number; run: number };
  approval: BotReplyApprovalUi;
}

export interface BotUi {
  marketOrigin?: BotMarketOrigin;
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
  /** Whether the owner's approved devices can reach this Bot through Remote Control. Absent: they cannot. */
  remote?: boolean;
  /** The folders of the owner's that the Bot may work in, besides its own. */
  workspaces: BotWorkspaceUi[];
  sandbox: BotSandboxUi;
  scope: BotScopeUi;
  autoReply: BotAutoReplyUi;
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
  workspaces?: BotWorkspaceInputUi[];
  sandbox?: BotSandboxUi;
  scope?: BotScopeUi;
  autoReply?: BotAutoReplyUi;
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
  /** Whether the owner's approved devices can reach this Bot through Remote Control. */
  remote?: boolean;
  /** Replaces the whole list. A folder that is in it already keeps its id; one that is left out is removed, and its chats end. */
  workspaces?: BotWorkspaceInputUi[];
  /** Each replaces the whole of what it names. */
  sandbox?: BotSandboxUi;
  scope?: BotScopeUi;
  autoReply?: BotAutoReplyUi;
}

/** Enough to attach the window's session view to a Bot's session. */
export interface BotSessionRefUi {
  botId: string;
  projectPath: string;
  sessionId: string;
  /** The folder of the Bot the session works in. */
  workspaceId?: string;
}

/** A chat of a Bot, and the folder it works in. */
export type BotSessionRowUi = SessionRowDto & {
  /** The folder of the Bot the chat worked in, if the Bot still works there. */
  workspaceId?: string;
  /** Where the chat worked, when that folder was removed from the Bot: the history is kept, and the chat is resumed by adding the folder again. */
  removedPath?: string;
};

export interface BotSessionsUi {
  sessions: BotSessionRowUi[];
}

/** A Bot asked for a folder; the owner decides. */
export interface BotWorkspaceRequestUi {
  requestId: string;
  botId: string;
  botName: string;
  path: string;
  mode: BotWorkspaceMode;
  /** What the Bot said it wants the folder for. One line. */
  reason: string;
  /** Epoch milliseconds. */
  createdAt: number;
}

export interface BotWorkspaceRequestsChangedUi {
  botIds: string[];
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
  market(request: BotMarketRequest): Promise<BotMarketResponse>;
  /** Never rejects for a computer that cannot run Bots: that is the answer. */
  availability(): Promise<BotsAvailability>;
  list(): Promise<BotUi[]>;
  get(botId: string): Promise<BotUi>;
  create(request: BotCreateRequest): Promise<BotUi>;
  update(botId: string, expectedRevision: number, patch: BotUpdateRequest): Promise<BotUi>;
  delete(botId: string): Promise<void>;
  openSession(botId: string, sessionId?: string, transcriptPath?: string, workspaceId?: string): Promise<BotSessionRefUi>;
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
  /** The folders the Bots asked for and the owner has not answered. */
  workspaceRequests: {
    list(botId?: string): Promise<BotWorkspaceRequestUi[]>;
    /** Approving binds the folder to the Bot; its open chats are given it as soon as they are not working. */
    decide(botId: string, requestId: string, decision: 'approve' | 'deny'): Promise<BotUi>;
    /** Called when a Bot asked for a folder or a request was answered or expired. */
    onChanged(callback: (change: BotWorkspaceRequestsChangedUi) => void): () => void;
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
  /** Which Bots may reach one of the owner's things, for the page of that thing. Answered from the Bots' records on this computer. */
  reach: {
    localApp(appId: string): Promise<BotReachUi[]>;
    /** Without a dataset, the Bots that were given any dataset of the source. */
    dataset(source: string, dataset?: string): Promise<BotReachUi[]>;
  };
}
