import { promises as fs, constants as fsConstants, openSync, closeSync, writeFileSync, fsyncSync, lstatSync, unlinkSync } from 'node:fs';
import type { Dirent } from 'node:fs';
import { dirname, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { BaseEvent, Message, RunAgentInput } from '@ag-ui/core';
import { EventSchema, MessageSchema } from '@lingxi/local-app-agui/client';
import type {
  LocalAppAgentChatHistory, LocalAppAgentChatListPage, LocalAppAgentChatSummary, LocalAppAgentModelChoice,
  LocalAppAgentSettings, LocalAppAgentTask, LocalAppAgentTaskState, LocalAppAgentUsage,
} from '../../shared/localAppAgent.js';
import { LOCAL_APP_AGENT_MAX_TIMEOUT_MS, LOCAL_APP_AGENT_MAX_TURNS } from '../../shared/localAppAgent.js';
import { agentDigest, LocalAppAgentError, parseRunAgentInput, validateAgentSettings, type LocalAppAgentExecutionInput, type ParsedLocalAppAgentInput } from './agentPolicy.js';
import { LocalAppChatAssetJournal } from './agentChatAssets.js';
export { LocalAppAgentError } from './agentPolicy.js';

/** Constructed by the trusted main-frame host; never accepted from the page. */
export interface LocalAppAgentOwner { appId: string; releaseDigest: string; invocationId: string; webContentsId?: number; remoteSessionId?: string }
export interface LocalAppAgentAuthorization {
  settings: LocalAppAgentSettings;
  requestedTools: string[];
  requestedSkills: string[];
  releaseRevision: number;
  releaseSourceDirectory: string;
  skillSpecs: Array<{ id: string; path: string }>;
  releaseFiles: Array<{ path: string; sha256: string; bytes: number; executable: boolean }>;
  model: LocalAppAgentModelChoice;
}
export interface LocalAppAgentAppChat { chatId: string; sessionId: string; sessionDirectory: string }
export interface LocalAppAgentPrivateResult { text: string; output?: unknown; attachments: []; chatId?: string; messageId?: string }
export interface LocalAppAgentPrivateTask {
  state: LocalAppAgentTaskState;
  createdAt: number;
  updatedAt: number;
  limits: { maxTurns: number; timeoutMs: number };
  usage?: LocalAppAgentUsage;
  progress?: { completedModelTurns: number; turnLimit: number };
}
export interface LocalAppAgentExecutorResult { result: LocalAppAgentPrivateResult; usage?: LocalAppAgentUsage }
export type LocalAppAgentProgressPhase = 'starting' | 'thinking' | 'tool' | 'waiting_for_permission' | 'waiting_for_answer';
export interface LocalAppAgentExecution {
  runId: string;
  /** Private UUID used for app-owned task files and isolated workspace paths. */
  taskStorageId: string;
  owner: LocalAppAgentOwner;
  input: LocalAppAgentExecutionInput;
  /** Present only for an App-owned persistent chat turn. */
  chat?: LocalAppAgentAppChat;
  /** Contains the fixed, selected tool grant subset and effective limits. */
  authorization: LocalAppAgentAuthorization;
  signal: AbortSignal;
  emit(progress: { phase: LocalAppAgentProgressPhase; completedModelTurns?: number; usage?: LocalAppAgentUsage }): Promise<void>;
  /** Must also be called after trusted approval, before any new external request. */
  assertAuthorized(): Promise<void>;
}
export interface LocalAppAgentBrokerOptions {
  /** `<config>/local-apps/v2` */
  root: string;
  authorize(owner: LocalAppAgentOwner): Promise<LocalAppAgentAuthorization>;
  revalidate(owner: LocalAppAgentOwner, authorization: LocalAppAgentAuthorization): Promise<boolean>;
  execute(execution: LocalAppAgentExecution): Promise<LocalAppAgentExecutorResult>;
  now?: () => number;
  uuid?: () => string;
}

interface PrivateRun extends LocalAppAgentPrivateTask {
  sequence: number;
  phase?: LocalAppAgentProgressPhase;
  error?: { code: string; message: string };
}
interface LedgerRecord {
  schema: 1;
  privateTaskId: string;
  owner: LocalAppAgentOwner;
  publicRunId: string;
  threadId: string;
  fingerprint: string;
  eventSequence: number;
  events: BaseEvent[];
  run: PrivateRun;
  chat?: { chatId: string; userMessageId: string };
  /** Crash-recovery copy of a standard assistant Message, never returned in task.get/list. */
  assistantMessage?: Message;
  /** Success is not public until all resource projections are committed. */
  completionPending?: true;
  /** A restart is durably reconciling an interrupted in-flight run. */
  interruptionPending?: true;
}
interface ChatRecord {
  schema: 2;
  creationFingerprint: string;
  chatId: string;
  appId: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  archived: boolean;
  state: LocalAppAgentChatSummary['state'];
  /** Private Harness identity. Never returned by a public API. */
  sessionId: string;
  activeRunId?: string;
  messages: Array<{ message: unknown; runId?: string }>;
}
interface ChatTombstone { schema: 2; appId: string; chatId: string; creationFingerprint: string; deleted: true }
interface ChatCreation { requestId: string; title?: string }
interface ActiveRun {
  record: LedgerRecord;
  owner: LocalAppAgentOwner;
  authorization: LocalAppAgentAuthorization;
  controller: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  appEpoch: number;
  chat?: LocalAppAgentAppChat;
  parsed: ParsedLocalAppAgentInput;
  assetJournal?: LocalAppChatAssetJournal;
  lastUsage?: LocalAppAgentUsage;
  settled?: Promise<void>;
}

const TERMINAL = new Set<LocalAppAgentTaskState>(['succeeded', 'failed', 'cancelled', 'interrupted']);
const PHASES = new Set<LocalAppAgentProgressPhase>(['starting', 'thinking', 'tool', 'waiting_for_permission', 'waiting_for_answer']);
const CHAT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const PRIVATE_TASK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const MAX_INPUT_BYTES = 116 * 1024 * 1024;
const MAX_EVENT_PAGE = 128;
// Creation identity is an application resource contract, not an AG-UI event.
function chatCreation(value: unknown): ChatCreation {
  const input = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
  if (!input || Object.keys(input).some(key => key !== 'requestId' && key !== 'title')
    || typeof input.requestId !== 'string' || !CHAT_ID.test(input.requestId)) {
    throw new LocalAppAgentError('invalid_chat', 'Chat creation requires a canonical UUIDv4 requestId.');
  }
  if (Object.hasOwn(input, 'title')) {
    const title = input.title;
    if (typeof title !== 'string' || title.length > 512 || Buffer.byteLength(title, 'utf8') > 512 || /\p{Cc}/u.test(title)
      || /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(title)
      || [...title.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '')].length < 1
      || [...title.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, '')].length > 80) {
      throw new LocalAppAgentError('invalid_chat', 'The Chat title must contain 1–80 characters.');
    }
    return { requestId: input.requestId, title };
  }
  return { requestId: input.requestId };
}
function chatCreationFingerprint(input: ChatCreation): string {
  return agentDigest(Object.hasOwn(input, 'title') ? { title: input.title } : {});
}
async function syncDirectory(path: string): Promise<void> {
  // Windows does not support opening a directory for fsync.
  if (process.platform === 'win32') return;
  const directory = await fs.open(path, 'r');
  try { await directory.sync(); } finally { await directory.close(); }
}
interface AgentJsonWrite { renameDispatched: boolean }
async function durableAgentJson(path: string, value: unknown, beforeCommit?: () => Promise<void>, write?: AgentJsonWrite): Promise<void> {
  const temp = `${path}.${randomUUID()}.tmp`;
  try {
    const file = await fs.open(temp, 'wx', 0o600);
    try { await file.writeFile(JSON.stringify(value, null, 2)); await file.sync(); } finally { await file.close(); }
    if (beforeCommit) await beforeCommit();
    if (write) write.renameDispatched = true;
    await fs.rename(temp, path);
    await syncDirectory(dirname(path));
  } finally { await fs.rm(temp, { force: true }); }
}
function validPublicId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && Buffer.byteLength(value, 'utf8') <= 128 && !/\p{Cc}/u.test(value);
}

function owner(value: LocalAppAgentOwner): LocalAppAgentOwner {
  if (!value || !/^a[a-f0-9]{32}$/.test(value.appId) || typeof value.releaseDigest !== 'string' || !/^[a-f0-9]{64}$/.test(value.releaseDigest)
    || typeof value.invocationId !== 'string' || !value.invocationId || value.invocationId.length > 256
    || (value.remoteSessionId !== undefined && (typeof value.remoteSessionId !== 'string' || !/^[A-Za-z0-9_-]{43}$/.test(value.remoteSessionId)))) throw new LocalAppAgentError('invalid_owner', 'Invalid application execution identity.');
  return { appId: value.appId, releaseDigest: value.releaseDigest, invocationId: value.invocationId, ...(value.remoteSessionId ? { remoteSessionId: value.remoteSessionId } : {}) };
}
function safeUsage(value: LocalAppAgentUsage | undefined): LocalAppAgentUsage | undefined {
  if (value === undefined) return undefined;
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new LocalAppAgentError('invalid_result', 'The Agent returned invalid usage data.');
  const result: LocalAppAgentUsage = {};
  for (const key of ['inputTokens', 'outputTokens', 'costUsd', 'turns'] as const) {
    const number = value[key];
    if (number !== undefined) {
      if (typeof number !== 'number' || !Number.isFinite(number) || number < 0) throw new LocalAppAgentError('invalid_result', 'The Agent returned invalid usage data.');
      result[key] = number;
    }
  }
  if (value.costEstimated !== undefined) {
    if (typeof value.costEstimated !== 'boolean') throw new LocalAppAgentError('invalid_result', 'The Agent returned invalid usage data.');
    result.costEstimated = value.costEstimated;
  }
  return result;
}
function requestKey(value: Pick<LocalAppAgentOwner, 'appId'>, publicRunId: string): string { return JSON.stringify([value.appId, publicRunId]); }
function cursorOffset(cursor: unknown): number {
  if (cursor === undefined) return 0;
  if (typeof cursor !== 'string' || cursor.length > 128) throw new LocalAppAgentError('invalid_input', 'Invalid list cursor.');
  const decoded = Buffer.from(cursor, 'base64url').toString('utf8');
  if (!/^(0|[1-9][0-9]{0,8})$/.test(decoded)) throw new LocalAppAgentError('invalid_input', 'Invalid list cursor.');
  return Number(decoded);
}
function nextCursor(offset: number, more: boolean): string | undefined { return more ? Buffer.from(String(offset)).toString('base64url') : undefined; }
function pageLimit(value: unknown, fallback = 50): number {
  if (value === undefined) return fallback;
  if (!Number.isSafeInteger(value) || (value as number) < 1 || (value as number) > 100) throw new LocalAppAgentError('invalid_input', 'List limits must be between 1 and 100.');
  return value as number;
}
function messageRecord(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}
function messageId(value: unknown): string | undefined {
  const row = messageRecord(value);
  return typeof row?.id === 'string' ? row.id : undefined;
}
function publicState(state: LocalAppAgentTaskState): LocalAppAgentTaskState { return state; }
function publicTask(record: LedgerRecord): LocalAppAgentTask {
  return {
    runId: record.publicRunId, threadId: record.threadId, state: publicState(record.run.state), sequence: record.run.sequence,
    ...(record.run.phase ? { phase: record.run.phase } : {}),
    createdAt: record.run.createdAt, updatedAt: record.run.updatedAt,
    limits: structuredClone(record.run.limits),
    ...(record.run.usage ? { usage: structuredClone(record.run.usage) } : {}),
    ...(record.run.progress ? { progress: structuredClone(record.run.progress) } : {}),
    ...(record.run.error ? { error: structuredClone(record.run.error) } : {}),
  };
}

/** Durable app-scoped task and chat index. Harness JSONL remains model history. */
export class LocalAppAgentBroker {
  private ready?: Promise<void>;
  private queue = Promise.resolve();
  private readonly records = new Map<string, LedgerRecord>();
  private readonly requests = new Map<string, string>();
  private readonly uncertainTasks = new Set<string>();
  private readonly chats = new Map<string, ChatRecord>();
  private readonly chatTombstones = new Map<string, ChatTombstone>();
  private readonly uncertainChats = new Set<string>();
  private readonly unsupportedApps = new Set<string>();
  private readonly unavailableApps = new Set<string>();
  private readonly active = new Map<string, ActiveRun>();
  private readonly settlementListeners = new Set<(owner: LocalAppAgentOwner) => void>();
  private readonly epochs = new Map<string, number>();
  private readonly appOperations = new Map<string, AbortController>();
  private stopped = false;
  constructor(private readonly options: LocalAppAgentBrokerOptions) {}
  hasActiveApp(appId: string): boolean { return [...this.active.values()].some(active => active.record.owner.appId === appId); }
  onSettled(listener: (owner: LocalAppAgentOwner) => void): () => void {
    this.settlementListeners.add(listener);
    return () => { this.settlementListeners.delete(listener); };
  }
  private now(): number { return this.options.now?.() ?? Date.now(); }
  private uuid(): string { return this.options.uuid?.() ?? randomUUID(); }
  private operationSignal(appId: string, signal?: AbortSignal): AbortSignal {
    if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
    let controller = this.appOperations.get(appId);
    if (!controller) { controller = new AbortController(); this.appOperations.set(appId, controller); }
    return signal ? AbortSignal.any([controller.signal, signal]) : controller.signal;
  }
  private exclusive<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.queue.then(operation, operation);
    this.queue = result.then(() => undefined, () => undefined);
    return result;
  }
  initialize(): Promise<void> { return this.ready ??= this.load(); }
  private async ensureAppStore(appId: string): Promise<void> {
    await this.initialize();
    if (this.unsupportedApps.has(appId)) {
      throw new LocalAppAgentError('agent_format_unsupported', 'This Local App has unsupported Agent storage. Its data has been preserved.');
    }
    if (this.unavailableApps.has(appId)) {
      throw new LocalAppAgentError('agent_storage_unavailable', 'This Local App has invalid or unsafe Agent storage. Its data has been preserved.');
    }
  }
  private appRoot(appId: string): string { return join(this.options.root, 'apps', appId, 'agent'); }
  private taskPath(principal: Pick<LocalAppAgentOwner, 'appId'>, taskStorageId: string): string {
    if (!PRIVATE_TASK_ID.test(taskStorageId)) throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    return join(this.appRoot(principal.appId), 'tasks', `${taskStorageId}.json`);
  }
  private chatDirectory(principal: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): string {
    if (!CHAT_ID.test(chatId)) throw new LocalAppAgentError('not_found', 'Agent chat was not found.');
    return join(this.appRoot(principal.appId), 'chats', chatId);
  }
  private chatPath(principal: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): string { return join(this.chatDirectory(principal, chatId), 'chat.json'); }
  private terminationPath(active: ActiveRun): string { return `${this.taskPath(active.record.owner, active.record.privateTaskId)}.termination`; }
  /** Persist the revocation fence before yielding to an in-flight file commit. */
  private fenceActiveRun(active: ActiveRun): void {
    const path = this.terminationPath(active), key = requestKey(active.record.owner, active.record.publicRunId);
    try {
      let descriptor: number;
      try { descriptor = openSync(path, fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL | (fsConstants.O_NOFOLLOW ?? 0), 0o600); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
        const stat = lstatSync(path);
        if (!stat.isFile() || stat.isSymbolicLink()) throw new Error('unsafe termination fence');
        return;
      }
      try {
        writeFileSync(descriptor, JSON.stringify({ schema: 1, appId: active.record.owner.appId, runId: active.record.publicRunId, taskId: active.record.privateTaskId }));
        fsyncSync(descriptor);
      } finally { closeSync(descriptor); }
      if (process.platform !== 'win32') {
        const directory = openSync(dirname(path), 'r');
        try { fsyncSync(directory); } finally { closeSync(directory); }
      }
    } catch {
      this.uncertainTasks.add(key);
      if (active.record.chat) this.uncertainChats.add(this.chatKey(active.record.owner.appId, active.record.chat.chatId));
      throw new LocalAppAgentError('agent_storage_unavailable', 'Agent termination could not be confirmed.');
    }
  }
  /** A terminal result becomes visible only after this synchronous commit. */
  private clearSettledRunFence(active: ActiveRun): void {
    const key = requestKey(active.record.owner, active.record.publicRunId), path = this.terminationPath(active);
    try {
      unlinkSync(path);
      if (process.platform !== 'win32') {
        const directory = openSync(dirname(path), 'r');
        try { fsyncSync(directory); } finally { closeSync(directory); }
      }
    } catch {
      this.uncertainTasks.add(key);
      if (active.record.chat) this.uncertainChats.add(this.chatKey(active.record.owner.appId, active.record.chat.chatId));
      throw new LocalAppAgentError('agent_storage_unavailable', 'Agent completion could not be confirmed.');
    }
  }
  private async clearTerminationFence(active: ActiveRun): Promise<void> {
    const key = requestKey(active.record.owner, active.record.publicRunId), record = this.records.get(key);
    if (!record || !TERMINAL.has(record.run.state) || this.uncertainTasks.has(key)) throw new LocalAppAgentError('agent_storage_unavailable', 'Agent termination could not be confirmed.');
    try { await fs.rm(this.terminationPath(active), { force: true }); await syncDirectory(dirname(this.terminationPath(active))); }
    catch {
      this.uncertainTasks.add(key);
      if (record.chat) this.uncertainChats.add(this.chatKey(record.owner.appId, record.chat.chatId));
      throw new LocalAppAgentError('agent_storage_unavailable', 'Agent termination could not be confirmed.');
    }
  }
  private async safeDirectory(path: string): Promise<void> {
    await fs.mkdir(path, { recursive: true, mode: 0o700 });
    const stat = await fs.lstat(path);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe directory.');
    if (process.platform !== 'win32') await fs.chmod(path, 0o700);
  }
  private async ensureAgentRoot(appId: string): Promise<void> {
    await this.safeDirectory(this.options.root);
    await this.safeDirectory(join(this.options.root, 'apps'));
    await this.safeDirectory(join(this.options.root, 'apps', appId));
    await this.safeDirectory(this.appRoot(appId));
  }
  private async syncAgentParents(appId: string, group: 'tasks' | 'chats'): Promise<void> {
    for (const path of [join(this.appRoot(appId), group), this.appRoot(appId), join(this.options.root, 'apps', appId), join(this.options.root, 'apps'), this.options.root]) await syncDirectory(path);
  }
  private async saveChat(chat: ChatRecord, beforeCommit?: () => Promise<void>, write?: AgentJsonWrite): Promise<void> {
    const key = this.chatKey(chat.appId, chat.chatId);
    let guardFailed = false;
    const guardCommit = beforeCommit ? async () => {
      try { await beforeCommit(); } catch (error) { guardFailed = true; throw error; }
    } : undefined;
    try {
      await this.ensureAgentRoot(chat.appId);
      await this.safeDirectory(this.chatDirectory({ appId: chat.appId }, chat.chatId));
      chat.updatedAt = this.now();
      await durableAgentJson(this.chatPath({ appId: chat.appId }, chat.chatId), chat, guardCommit, write);
      this.chats.set(key, chat);
    } catch (error) {
      // Guard rejection happens before rename is dispatched, so the previous
      // durable record remains intact and must not become unavailable.
      if (guardFailed) throw error;
      this.uncertainChats.add(key);
      throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
    }
  }
  private chatKey(appId: string, chatId: string): string { return `${appId}:${chatId}`; }
  private async cleanupDeletedChat(principal: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): Promise<void> {
    const directory = this.chatDirectory(principal, chatId), stat = await fs.lstat(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('unsafe Chat directory');
    for (const name of await fs.readdir(directory)) if (name !== 'chat.json') await fs.rm(join(directory, name), { recursive: true, force: true });
    await syncDirectory(directory);
  }

  /** Identify unmarked Agent data without modifying or interpreting it. */
  private async findUnmarkedAgentData(): Promise<string[]> {
    const appsPath = join(this.options.root, 'apps');
    await this.safeDirectory(appsPath);
    const unsupported: string[] = [];
    for (const entry of await fs.readdir(appsPath, { withFileTypes: true })) {
      if (!/^a[a-f0-9]{32}$/.test(entry.name)) continue;
      try {
        if (!entry.isDirectory() || entry.isSymbolicLink()) { unsupported.push(entry.name); continue; }
        const agentPath = join(appsPath, entry.name, 'agent');
        let agentStat;
        try { agentStat = await fs.lstat(agentPath); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
        if (!agentStat.isDirectory() || agentStat.isSymbolicLink()) { unsupported.push(entry.name); continue; }
        let hasData = false;
        for (const child of await fs.readdir(agentPath, { withFileTypes: true })) {
          if (!['tasks', 'chats', 'assets'].includes(child.name) || !child.isDirectory() || child.isSymbolicLink()
            || (await fs.readdir(join(agentPath, child.name))).length) { hasData = true; break; }
        }
        if (hasData) unsupported.push(entry.name);
      } catch {
        // An unreadable or unsafe App store must not prevent other Apps from
        // starting or be mistaken for empty storage.
        unsupported.push(entry.name);
      }
    }
    return unsupported.sort();
  }
  private async ensureStorageFormat(): Promise<void> {
    const path = join(this.options.root, 'desktop-agent-format.json');
    try {
      const marker = JSON.parse(await fs.readFile(path, 'utf8'));
      if (marker?.schemaVersion !== 1 || marker?.protocol !== 'ag-ui/1.0'
        || (marker.quarantinedAppIds !== undefined && (!Array.isArray(marker.quarantinedAppIds)
          || marker.quarantinedAppIds.some((id: unknown) => typeof id !== 'string' || !/^a[a-f0-9]{32}$/.test(id))
          || new Set(marker.quarantinedAppIds).size !== marker.quarantinedAppIds.length))) {
        throw new LocalAppAgentError('ledger_invalid', 'The Desktop Agent store format is unsupported.');
      }
      for (const appId of marker.quarantinedAppIds ?? []) this.unsupportedApps.add(appId);
      return;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    const quarantinedAppIds = await this.findUnmarkedAgentData();
    // Quarantine is metadata only: the original bytes are never removed,
    // migrated, or treated as absent on later restarts.
    await durableAgentJson(path, { schemaVersion: 1, protocol: 'ag-ui/1.0', quarantinedAppIds });
    for (const appId of quarantinedAppIds) this.unsupportedApps.add(appId);
  }
  private appendEvent(record: LedgerRecord, value: unknown): BaseEvent {
    const event = EventSchema.parse(value) as BaseEvent;
    record.events.push(event);
    record.eventSequence = record.events.length;
    return event;
  }
  private async readTerminationFence(path: string, record: LedgerRecord): Promise<'missing' | 'valid' | 'invalid'> {
    let file: Awaited<ReturnType<typeof fs.open>> | undefined;
    try {
      const stat = await fs.lstat(path);
      if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 4096) return 'invalid';
      file = await fs.open(path, fsConstants.O_RDONLY | (fsConstants.O_NOFOLLOW ?? 0));
      const opened = await file.stat();
      if (!opened.isFile() || opened.size > 4096) return 'invalid';
      const value = JSON.parse(await file.readFile({ encoding: 'utf8' }));
      return value?.schema === 1 && value.appId === record.owner.appId && value.runId === record.publicRunId
        && value.taskId === record.privateTaskId ? 'valid' : 'invalid';
    } catch (error) {
      return (error as NodeJS.ErrnoException).code === 'ENOENT' ? 'missing' : 'invalid';
    } finally { await file?.close().catch(() => undefined); }
  }
  private async readAgentJson(path: string): Promise<unknown> {
    const stat = await fs.lstat(path);
    if (!stat.isFile() || stat.isSymbolicLink()) throw new Error('unsafe Agent record');
    const file = await fs.open(path, fsConstants.O_RDONLY | (fsConstants.O_NOFOLLOW ?? 0));
    try {
      const opened = await file.stat();
      if (!opened.isFile() || opened.dev !== stat.dev || opened.ino !== stat.ino) throw new Error('Agent record changed during inspection');
      return JSON.parse(await file.readFile({ encoding: 'utf8' }));
    } finally { await file.close(); }
  }
  private async load(): Promise<void> {
    await this.safeDirectory(this.options.root);
    await this.ensureStorageFormat();
    const appsPath = join(this.options.root, 'apps');
    await this.safeDirectory(appsPath);
    for (const appEntry of await fs.readdir(appsPath, { withFileTypes: true })) {
      if (!/^a[a-f0-9]{32}$/.test(appEntry.name)) continue;
      const appId = appEntry.name;
      if (this.unsupportedApps.has(appId)) continue;
      try {
        if (!appEntry.isDirectory() || appEntry.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe application directory.');
        await this.loadApp(appId);
      } catch {
        // A rejected App store must neither poison shared initialization nor
        // leave partially loaded records available to APIs or later recovery.
        this.unavailableApps.add(appId);
        for (const [key, record] of this.records) if (record.owner.appId === appId) {
          this.records.delete(key); this.requests.delete(key); this.uncertainTasks.delete(key);
        }
        for (const [key, chat] of this.chats) if (chat.appId === appId) this.chats.delete(key);
        for (const [key, chat] of this.chatTombstones) if (chat.appId === appId) this.chatTombstones.delete(key);
        for (const key of this.uncertainChats) if (key.startsWith(`${appId}:`)) this.uncertainChats.delete(key);
      }
    }
  }
  private async loadApp(appId: string): Promise<void> {
    const agentRoot = this.appRoot(appId);
    let agentStat;
    try { agentStat = await fs.lstat(agentRoot); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return; throw error; }
    if (!agentStat.isDirectory() || agentStat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe application directory.');

    // Inspect the entire App before changing any of it. Recovery can update a
    // task, rewrite a Chat or delete tombstoned assets, so doing those while a
    // later record is still unchecked would alter data belonging to a rejected App.
    const groups = new Map<string, { path: string; entries: Dirent[] }>();
    for (const group of ['tasks', 'chats', 'assets']) {
      const groupPath = join(agentRoot, group);
      let stat;
      try { stat = await fs.lstat(groupPath); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('unsafe Agent directory');
      if (group !== 'assets') groups.set(group, { path: groupPath, entries: await fs.readdir(groupPath, { withFileTypes: true }) });
    }

    const tasks: Array<{ path: string; record: LedgerRecord; principal: LocalAppAgentOwner; fence: 'missing' | 'valid' | 'invalid'; uncertain: boolean }> = [];
    const tasksByKey = new Map<string, LedgerRecord>();
    const taskGroup = groups.get('tasks');
    for (const entry of taskGroup?.entries ?? []) {
      if (!entry.name.endsWith('.json')) continue;
      if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('unsafe Agent task record');
      const path = join(taskGroup!.path, entry.name), record = await this.readAgentJson(path) as LedgerRecord;
      const principal = owner(record.owner);
      if (principal.appId !== appId || record.schema !== 1 || !PRIVATE_TASK_ID.test(record.privateTaskId) || `${record.privateTaskId}.json` !== entry.name
        || !validPublicId(record.publicRunId) || !validPublicId(record.threadId) || !/^[a-f0-9]{64}$/.test(record.fingerprint) || !Array.isArray(record.events)
        || record.eventSequence !== record.events.length || !['queued', 'running', ...TERMINAL].includes(record.run?.state)
        || !Number.isSafeInteger(record.run?.sequence) || record.run.sequence < 1 || !Number.isFinite(record.run?.createdAt) || !Number.isFinite(record.run?.updatedAt)
        || !Number.isSafeInteger(record.run?.limits?.maxTurns) || record.run.limits.maxTurns < 1 || record.run.limits.maxTurns > LOCAL_APP_AGENT_MAX_TURNS
        || !Number.isSafeInteger(record.run?.limits?.timeoutMs) || record.run.limits.timeoutMs < 1 || record.run.limits.timeoutMs > LOCAL_APP_AGENT_MAX_TIMEOUT_MS) {
        throw new Error('invalid Agent task record');
      }
      if (record.chat && (!CHAT_ID.test(record.chat.chatId) || !validPublicId(record.chat.userMessageId) || record.threadId !== record.chat.chatId)) {
        throw new Error('invalid Agent task Chat identity');
      }
      if (record.assistantMessage && (!messageId(record.assistantMessage) || messageRecord(record.assistantMessage)?.role !== 'assistant')) {
        throw new Error('invalid Agent assistant message');
      }
      if (record.completionPending !== undefined && (record.completionPending !== true || record.run.state !== 'succeeded' || (record.chat && !record.assistantMessage))) {
        throw new Error('invalid Agent completion marker');
      }
      if (record.interruptionPending !== undefined && (record.interruptionPending !== true || record.run.state !== 'interrupted'
        || record.run.error?.code !== 'host_restarted' || record.completionPending !== undefined)) {
        throw new Error('invalid Agent interruption marker');
      }
      record.events = record.events.map(event => EventSchema.parse(event));
      const started = record.events[0];
      if (started?.type !== 'RUN_STARTED' || started.runId !== record.publicRunId || started.threadId !== record.threadId) throw new Error('invalid Agent event journal');
      const key = requestKey(principal, record.publicRunId);
      if (tasksByKey.has(key) || this.requests.has(key) || this.records.has(key)) throw new Error('conflicting Agent run IDs');
      const fence = await this.readTerminationFence(`${path}.termination`, record);
      const nonterminal = !TERMINAL.has(record.run.state);
      const noResultEvidence = !record.assistantMessage && record.events.length === 1 && record.events[0]?.type === 'RUN_STARTED';
      const interruptionEvent = record.events.at(-1);
      const validInterruption = !record.interruptionPending || (!record.assistantMessage && record.events.length === 2
        && interruptionEvent?.type === 'RUN_ERROR' && interruptionEvent.code === 'host_restarted');
      const resumableInterruption = (record.interruptionPending === true && validInterruption) || (nonterminal && noResultEvidence);
      const uncertain = fence === 'invalid' || record.completionPending === true || !validInterruption
        || (fence === 'valid' && !resumableInterruption) || (nonterminal && !noResultEvidence);
      tasksByKey.set(key, record);
      tasks.push({ path, record, principal, fence, uncertain });
    }
    const taskIds = new Set(tasks.map(({ record }) => record.privateTaskId));
    for (const entry of taskGroup?.entries ?? []) {
      if (entry.name.endsWith('.json')) continue;
      // A named fence belongs to its task even when its contents or type are
      // unsafe. Keep it untouched and quarantine that task instead of
      // confusing it with an unowned entry in the shared task directory.
      if (entry.name.endsWith('.json.termination')
        && taskIds.has(entry.name.slice(0, -'.json.termination'.length))) continue;
      if (entry.isSymbolicLink()) throw new Error('unsafe task storage entry');
      if (PRIVATE_TASK_ID.test(entry.name) && entry.isDirectory() && taskIds.has(entry.name)) continue;
      const temporary = /^([0-9a-f-]+)\.json\.([0-9a-f-]+)\.tmp$/i.exec(entry.name);
      if (temporary && PRIVATE_TASK_ID.test(temporary[1]!) && PRIVATE_TASK_ID.test(temporary[2]!) && entry.isFile()) continue;
      throw new Error('unrecognised task storage entry');
    }

    const chats = new Map<string, { path: string; row: ChatRecord | ChatTombstone }>();
    const sessionIds = new Set<string>();
    const chatGroup = groups.get('chats');
    for (const entry of chatGroup?.entries ?? []) {
      if (!CHAT_ID.test(entry.name) || !entry.isDirectory() || entry.isSymbolicLink()) throw new Error('unsafe Chat directory');
      const directory = join(chatGroup!.path, entry.name), path = join(directory, 'chat.json');
      const stat = await fs.lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('unsafe Chat directory');
      const row = await this.readAgentJson(path) as ChatRecord | ChatTombstone;
      if (!row || row.schema !== 2 || row.appId !== appId || row.chatId !== entry.name || !/^[a-f0-9]{64}$/.test(row.creationFingerprint)) throw new Error('invalid Chat identity');
      const deleted = 'deleted' in row;
      if (deleted) {
        if (row.deleted !== true || Object.keys(row).some(key => !['schema', 'appId', 'chatId', 'creationFingerprint', 'deleted'].includes(key))) {
          throw new Error('invalid Chat tombstone');
        }
      } else {
        const chat = row as ChatRecord;
        if (typeof chat.sessionId !== 'string' || !PRIVATE_TASK_ID.test(chat.sessionId) || sessionIds.has(chat.sessionId.toLowerCase())
          || typeof chat.title !== 'string' || !Number.isFinite(chat.createdAt) || !Number.isFinite(chat.updatedAt)
          || typeof chat.archived !== 'boolean' || (chat.activeRunId !== undefined && !validPublicId(chat.activeRunId))
          || !Array.isArray(chat.messages) || chat.messages.some(message => !message || typeof message !== 'object' || !('message' in message))
          || !['idle', 'running', 'interrupted'].includes(chat.state)
          || (chat.state === 'running') !== (chat.activeRunId !== undefined)) throw new Error('invalid Chat record');
        sessionIds.add(chat.sessionId.toLowerCase());
      }
      // Do not follow a substituted assets directory or a symlink encountered
      // during the preflight. A tombstone may legitimately retain arbitrary
      // files until cleanup, so its entry names are not interpreted as assets.
      const assets = join(directory, 'assets');
      let assetStat;
      try { assetStat = await fs.lstat(assets); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      if (assetStat) {
        if (!assetStat.isDirectory() || assetStat.isSymbolicLink()) throw new Error('unsafe Chat assets directory');
        for (const asset of await fs.readdir(assets, { withFileTypes: true })) {
          if (asset.isSymbolicLink()) throw new Error('unsafe Chat asset');
          if (!deleted && (!asset.isFile() || (!/^[a-f0-9]{64}\.bin$/.test(asset.name) && !(asset.name.startsWith('.') && asset.name.endsWith('.tmp'))))) {
            throw new Error('invalid Chat asset');
          }
        }
      }
      if (!deleted) {
        const journal = new LocalAppChatAssetJournal(directory);
        for (const message of (row as ChatRecord).messages) MessageSchema.parse(await journal.rehydrateMessage(message.message));
      }
      chats.set(entry.name, { path, row });
    }

    // Cross-check both directions. In particular, an orphaned Chat row could be
    // an interrupted admission or a lost task file; neither case proves that
    // deleting the row or its assets is safe.
    const userRows = new Map<string, number>();
    for (const [chatId, { row }] of chats) {
      if ('deleted' in row) continue;
      const messageIds = new Set<string>();
      for (const item of row.messages) {
        if (typeof item.runId !== 'string') throw new Error('Chat message has no task identity');
        const key = requestKey({ appId }, item.runId), record = tasksByKey.get(key), message = messageRecord(item.message), id = messageId(item.message);
        if (!record?.chat || record.chat.chatId !== chatId || !message || !id || messageIds.has(id)) throw new Error('inconsistent Chat message');
        messageIds.add(id);
        if (id === record.chat.userMessageId && message.role === 'user') userRows.set(key, (userRows.get(key) ?? 0) + 1);
        else if (!record.assistantMessage || id !== messageId(record.assistantMessage) || message.role !== 'assistant'
          || agentDigest(message) !== agentDigest(record.assistantMessage)) throw new Error('inconsistent Chat message');
      }
      if (row.activeRunId !== undefined) {
        const active = tasksByKey.get(requestKey({ appId }, row.activeRunId));
        if (!active?.chat || active.chat.chatId !== chatId) throw new Error('inconsistent active Chat');
      }
    }
    const pendingChats = new Set<string>();
    for (const { record, principal } of tasks) {
      if (!record.chat) continue;
      const chat = chats.get(record.chat.chatId)?.row;
      if (!chat) throw new Error('task Chat is missing');
      if ('deleted' in chat) {
        if (!TERMINAL.has(record.run.state) || record.interruptionPending) throw new Error('active task refers to deleted Chat');
        continue;
      }
      const key = requestKey(principal, record.publicRunId);
      if (userRows.get(key) !== 1) throw new Error('task user message is missing');
      if (!TERMINAL.has(record.run.state) || record.interruptionPending) {
        if (pendingChats.has(record.chat.chatId)) throw new Error('conflicting active Chat tasks');
        pendingChats.add(record.chat.chatId);
        if (!((chat.state === 'running' && chat.activeRunId === record.publicRunId)
          || (chat.state === 'interrupted' && chat.activeRunId === undefined))) throw new Error('inconsistent interrupted Chat');
      }
    }

    // A well-formed task with an unconfirmed outcome is not replayed or
    // repaired. Mark all related records first so recovery cannot touch their
    // Chat or tombstone, even when it appears earlier in directory order.
    for (const { record, principal, uncertain } of tasks) {
      const key = requestKey(principal, record.publicRunId);
      this.records.set(key, record); this.requests.set(key, key);
      if (uncertain) {
        this.uncertainTasks.add(key);
        if (record.chat) this.uncertainChats.add(this.chatKey(appId, record.chat.chatId));
      }
    }
    for (const { record, principal } of tasks) {
      if (record.chat && this.uncertainChats.has(this.chatKey(appId, record.chat.chatId))) {
        this.uncertainTasks.add(requestKey(principal, record.publicRunId));
      }
    }
    const interrupted = new Map<string, { path: string; record: LedgerRecord; hadFence: boolean }>();
    for (const { path, record, principal, fence } of tasks) {
      const key = requestKey(principal, record.publicRunId);
      if (this.uncertainTasks.has(key)) continue;
      if (!TERMINAL.has(record.run.state)) {
        // Never replay an interrupted Agent or any of its external actions.
        record.run.state = 'interrupted'; record.run.phase = undefined; record.run.sequence += 1; record.run.updatedAt = this.now();
        record.run.error = { code: 'host_restarted', message: 'The host stopped before this task completed. Tool effects and results may be unknown. The task was not replayed.' };
        this.appendEvent(record, { type: 'RUN_ERROR', message: record.run.error.message, code: record.run.error.code, timestamp: this.now() });
        record.interruptionPending = true;
        await durableAgentJson(path, record); await this.syncAgentParents(appId, 'tasks');
      }
      if (record.interruptionPending) interrupted.set(key, { path, record, hadFence: fence === 'valid' });
    }
    for (const [chatId, { path, row }] of chats) {
      const key = this.chatKey(appId, chatId);
      if (this.uncertainChats.has(key)) continue;
      if ('deleted' in row) {
        this.chatTombstones.set(key, row);
        continue;
      }
      const chat = row as ChatRecord;
      if (chat.state === 'running' || chat.activeRunId) { chat.state = 'interrupted'; chat.activeRunId = undefined; chat.updatedAt = this.now(); await durableAgentJson(path, chat); }
      this.chats.set(key, chat);
    }
    for (const { path, record, hadFence } of interrupted.values()) {
      const chatKey = record.chat ? this.chatKey(appId, record.chat.chatId) : undefined;
      if (chatKey && !this.chats.has(chatKey)) throw new Error('interrupted task Chat is unavailable');
      if (hadFence) { await fs.unlink(`${path}.termination`); await syncDirectory(dirname(path)); }
      delete record.interruptionPending;
      await durableAgentJson(path, record); await this.syncAgentParents(appId, 'tasks');
    }
    // Recover a terminal task whose Chat projection did not reach disk.
    for (const { record } of tasks) if (record.chat && record.run.state === 'succeeded' && record.assistantMessage) {
      if (this.uncertainTasks.has(requestKey(record.owner, record.publicRunId)) || this.uncertainChats.has(this.chatKey(appId, record.chat.chatId))) continue;
      const chat = this.chats.get(this.chatKey(appId, record.chat.chatId));
      if (chat && !chat.messages.some(message => messageId(message.message) === record.assistantMessage!.id)) {
        chat.messages.push({ message: structuredClone(record.assistantMessage), runId: record.publicRunId });
        chat.state = 'idle'; chat.activeRunId = undefined; await this.saveChat(chat);
      }
    }
    // Cleanup is last: an earlier recovery failure must not delete a separate
    // Chat's assets before this App has finished reconciling its records.
    for (const [chatId, { row }] of chats) {
      if ('deleted' in row && !this.uncertainChats.has(this.chatKey(appId, chatId))) await this.cleanupDeletedChat({ appId }, chatId);
    }
  }
  private lookup(caller: LocalAppAgentOwner, runId: string): LedgerRecord {
    const key = requestKey(caller, runId);
    const record = this.records.get(key);
    if (this.uncertainTasks.has(key)) throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
    if (!record || record.owner.appId !== caller.appId || (!TERMINAL.has(record.run.state)
      && (record.owner.invocationId !== caller.invocationId || record.owner.releaseDigest !== caller.releaseDigest))) {
      throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    }
    return record;
  }
  private getChatRecord(caller: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): ChatRecord {
    const key = this.chatKey(caller.appId, chatId);
    if (this.uncertainChats.has(key)) throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
    const chat = this.chats.get(key);
    if (!chat) throw new LocalAppAgentError('not_found', 'Agent chat was not found.');
    return chat;
  }
  private validateChatPrefix(chat: ChatRecord, parsed: ParsedLocalAppAgentInput, journal: LocalAppChatAssetJournal): void {
    if (parsed.threadId !== chat.chatId) throw new LocalAppAgentError('thread_mismatch', 'The AG-UI thread does not match this Local App chat.');
    const supplied = parsed.publicInput.messages.slice(0, -1);
    if (!supplied.length) return; // Host owns the canonical transcript; a client may send only the new user message.
    const stored = chat.messages.map(row => row.message);
    if (supplied.length > stored.length) throw new LocalAppAgentError('history_mismatch', 'The supplied AG-UI chat history is not in the host transcript.');
    const start = stored.length - supplied.length;
    for (let index = 0; index < supplied.length; index += 1) {
      const canonical = journal.canonicalizeMessage(supplied[index]);
      if (agentDigest(canonical) !== agentDigest(stored[start + index])) throw new LocalAppAgentError('history_mismatch', 'The supplied AG-UI chat history does not match the host transcript.');
    }
  }
  private publicOwnerMatchesActive(record: LedgerRecord, principal: LocalAppAgentOwner): boolean {
    return record.owner.appId === principal.appId && record.owner.releaseDigest === principal.releaseDigest && record.owner.invocationId === principal.invocationId;
  }
  private eventPage(record: LedgerRecord, afterSequence = 0): { events: BaseEvent[]; sequence: number } {
    if (!Number.isSafeInteger(afterSequence) || afterSequence < 0 || afterSequence > record.eventSequence) throw new LocalAppAgentError('invalid_input', 'Invalid AG-UI event cursor.');
    const events = structuredClone(record.events.slice(afterSequence, afterSequence + MAX_EVENT_PAGE));
    return { events, sequence: afterSequence + events.length };
  }
  async run(caller: LocalAppAgentOwner, value: unknown, signal?: AbortSignal): Promise<{ runId: string }> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId);
    const threadId = messageRecord(value)?.threadId;
    if (typeof threadId === 'string' && this.uncertainChats.has(this.chatKey(principal.appId, threadId))) throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
    if (typeof threadId === 'string' && this.chatTombstones.has(this.chatKey(principal.appId, threadId))) throw new LocalAppAgentError('chat_create_deleted', 'This Chat has been deleted.');
    const chat = typeof threadId === 'string' ? this.chats.get(this.chatKey(principal.appId, threadId)) : undefined;
    return this.admit(caller, value, chat, signal);
  }
  async sendChat(caller: LocalAppAgentOwner, chatId: string, value: unknown, signal?: AbortSignal): Promise<{ runId: string }> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId);
    const chat = this.getChatRecord(principal, chatId);
    return this.admit(caller, value, chat, signal);
  }
  private async admit(caller: LocalAppAgentOwner, value: unknown, chat?: ChatRecord, signal?: AbortSignal): Promise<{ runId: string }> {
    const principal = owner(caller);
    let encodedBytes: number;
    try { encodedBytes = Buffer.byteLength(JSON.stringify(value)); } catch { throw new LocalAppAgentError('invalid_input', 'The AG-UI run input is not valid JSON.'); }
    if (encodedBytes > MAX_INPUT_BYTES) throw new LocalAppAgentError('invalid_input', 'The AG-UI run input exceeds the host size limit.');
    // Parse the official envelope and fingerprint it before any canonical
    // history validation. This lets an exact terminal Chat retry return its
    // existing run after the host transcript has advanced.
    const parsed = parseRunAgentInput(value, { persistentChat: true });
    const fingerprint = agentDigest(parsed.publicInput);
    const epoch = this.epochs.get(principal.appId) ?? 0;
    await this.initialize();
    return this.exclusive(async () => {
      signal?.throwIfAborted();
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      if (chat && this.getChatRecord(principal, chat.chatId) !== chat) throw new LocalAppAgentError('not_found', 'Agent chat was not found.');
      if (!chat) {
        const chatKey = this.chatKey(principal.appId, parsed.threadId);
        if (this.uncertainChats.has(chatKey)) throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
        if (this.chatTombstones.has(chatKey)) throw new LocalAppAgentError('chat_create_deleted', 'This Chat has been deleted.');
        if (this.chats.has(chatKey)) throw new LocalAppAgentError('thread_conflict', 'This thread belongs to a persistent Chat.');
      }
      const key = requestKey(principal, parsed.runId), existingId = this.requests.get(key);
      if (existingId) {
        const existing = this.lookup(principal, parsed.runId);
        if (existing.fingerprint !== fingerprint || (chat && existing.chat?.chatId !== chat.chatId)) throw new LocalAppAgentError('request_conflict', 'This AG-UI runId was already used with different input.');
        if (!TERMINAL.has(existing.run.state) && !this.publicOwnerMatchesActive(existing, principal)) throw new LocalAppAgentError('not_found', 'Agent task was not found.');
        return { runId: parsed.runId };
      }
      if (!chat && (parsed.publicInput.messages.length !== 1 || parsed.publicInput.messages[0]?.role !== 'user')) {
        throw new LocalAppAgentError('stateless_history_unsupported', 'An independent Local App Agent run requires exactly one user message on a fresh thread.');
      }
      let assetJournal: LocalAppChatAssetJournal | undefined;
      if (chat) {
        assetJournal = new LocalAppChatAssetJournal(this.chatDirectory(principal, chat.chatId));
        this.validateChatPrefix(chat, parsed, assetJournal);
        const currentId = messageId(parsed.currentMessage);
        if (!currentId || chat.messages.some(message => messageId(message.message) === currentId)) throw new LocalAppAgentError('message_conflict', 'This AG-UI user message ID already exists in the chat.');
      }
      const authorization = structuredClone(await this.options.authorize(caller));
      validateAgentSettings(authorization.settings);
      signal?.throwIfAborted();
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      if ((this.epochs.get(principal.appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while starting the task.');
      if ([...this.active.values()].some(active => active.record.owner.appId === principal.appId)) throw new LocalAppAgentError('busy', 'This application already has an Agent task in progress.');
      if (chat && (chat.archived || chat.activeRunId)) throw new LocalAppAgentError(chat.archived ? 'chat_archived' : 'chat_busy', chat.archived ? 'This Agent chat is archived.' : 'This Agent chat already has a turn in progress.');
      const model = authorization.model, settings = authorization.settings, input = parsed.executionInput;
      if (!model.canSelect || model.modelId !== settings.model.modelId || model.providerId !== settings.model.providerId) throw new LocalAppAgentError('model_unavailable', 'The selected Agent model is not configured.');
      const hasAttachments = input.contentParts.some(part => part.kind === 'attachment');
      if (hasAttachments && !model.supportsAttachments) throw new LocalAppAgentError('attachments_unsupported', 'The selected model does not support attachments.');
      if (input.tools.length && !model.supportsStructuredOutput) throw new LocalAppAgentError('tools_unsupported', 'The selected model does not support tool calls.');
      if (input.outputSchema && !model.supportsStructuredOutput) throw new LocalAppAgentError('structured_output_unsupported', 'The selected model does not support structured results.');
      for (const id of input.tools) if (!authorization.requestedTools.includes(id) || !settings.tools.some(tool => tool.id === id)) throw new LocalAppAgentError('tool_not_granted', 'A requested tool is not declared and granted for this application.');
      for (const id of input.skills) if (!authorization.requestedSkills.includes(id) || !settings.skills.includes(id)) throw new LocalAppAgentError('skill_not_granted', 'A requested Skill is not declared and granted for this application.');
      if ((input.maxTurns ?? settings.maxTurns) > settings.maxTurns || (input.timeoutMs ?? settings.timeoutMs) > settings.timeoutMs) throw new LocalAppAgentError('limit_exceeded', 'Applications can only lower the host task limits.');
      authorization.settings.tools = settings.tools.filter(tool => input.tools.includes(tool.id));
      authorization.settings.skills = settings.skills.filter(skill => input.skills.includes(skill));
      authorization.settings.maxTurns = input.maxTurns ?? settings.maxTurns;
      authorization.settings.timeoutMs = input.timeoutMs ?? settings.timeoutMs;
      if (!(await this.options.revalidate(caller, authorization)) || (this.epochs.get(principal.appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while starting the task.');
      signal?.throwIfAborted();
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      const time = this.now(), privateTaskId = this.uuid();
      if (!validPublicId(parsed.runId) || this.records.has(key)) throw new LocalAppAgentError('id_conflict', 'This AG-UI runId is already in use.');
      if (!PRIVATE_TASK_ID.test(privateTaskId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate an Agent task identifier.');
      const chatMeta = chat ? { chatId: chat.chatId, userMessageId: parsed.currentMessage.id } : undefined;
      const record: LedgerRecord = {
        schema: 1, privateTaskId, owner: principal, publicRunId: parsed.runId, threadId: parsed.threadId, fingerprint,
        eventSequence: 0, events: [],
        run: { state: 'queued', sequence: 1, phase: 'starting', createdAt: time, updatedAt: time,
          limits: { maxTurns: authorization.settings.maxTurns, timeoutMs: authorization.settings.timeoutMs } },
        ...(chatMeta ? { chat: chatMeta } : {}),
      };
      this.appendEvent(record, { type: 'RUN_STARTED', threadId: parsed.threadId, runId: parsed.runId, protocolVersion: '1.0', timestamp: time });
      await this.ensureAgentRoot(principal.appId);
      await this.safeDirectory(join(this.appRoot(principal.appId), 'tasks'));
      signal?.throwIfAborted();
      const previousChat = chat ? structuredClone(chat) : undefined;
      const taskWrite: AgentJsonWrite = { renameDispatched: false };
      try {
        if (chat && assetJournal) {
          const stored = await assetJournal.storeMessage(parsed.currentMessage);
          signal?.throwIfAborted();
          chat.messages.push({ message: stored.message, runId: parsed.runId });
          chat.state = 'running'; chat.activeRunId = parsed.runId;
          await this.saveChat(chat, async () => { signal?.throwIfAborted(); });
        }
        await this.persist(record, async () => { signal?.throwIfAborted(); }, taskWrite);
        signal?.throwIfAborted();
      }
      catch (error) {
        if (signal?.aborted && !taskWrite.renameDispatched) {
          // No reader can observe provisional admission while this queue is
          // held. Roll back only this attempt's owned task and Chat projection.
          try {
            if (previousChat && assetJournal) {
              await durableAgentJson(this.chatPath(previousChat, previousChat.chatId), previousChat);
              this.chats.set(this.chatKey(previousChat.appId, previousChat.chatId), previousChat);
              await assetJournal.pruneUnreferenced(previousChat.messages.map(row => row.message));
              this.uncertainChats.delete(this.chatKey(previousChat.appId, previousChat.chatId));
            }
            await fs.rm(this.taskPath(principal, privateTaskId), { force: true });
            await this.syncAgentParents(principal.appId, 'tasks');
            this.uncertainTasks.delete(key);
          } catch {
            this.uncertainTasks.add(key);
            if (previousChat) this.uncertainChats.add(this.chatKey(previousChat.appId, previousChat.chatId));
            throw new LocalAppAgentError('agent_storage_unavailable', 'Cancelled Agent admission could not be reconciled.');
          }
          signal.throwIfAborted();
        }
        if (signal?.aborted) {
          this.uncertainTasks.add(key);
          if (chat) this.uncertainChats.add(this.chatKey(chat.appId, chat.chatId));
        }
        // Rename may have committed even if its acknowledgement failed. Keep
        // the request reserved until the task ledger is reopened, and retain
        // the Chat write for recovery instead of guessing that it is absent.
        this.records.set(key, record); this.requests.set(key, key);
        throw error;
      }
      const active: ActiveRun = {
        record, owner: structuredClone(caller), authorization, controller: new AbortController(), appEpoch: epoch, parsed,
        ...(assetJournal ? { assetJournal } : {}),
        ...(chat ? { chat: { chatId: chat.chatId, sessionId: chat.sessionId, sessionDirectory: this.chatDirectory(principal, chat.chatId) } } : {}),
      };
      this.records.set(key, record); this.requests.set(key, key);
      // Reserve the durable in-flight fence before any model or tool runs.
      this.fenceActiveRun(active);
      this.active.set(key, active);
      active.timer = setTimeout(() => { void this.terminate(key, 'failed', { code: 'timeout', message: 'The Agent task exceeded its time limit.' }).catch(() => undefined); }, record.run.limits.timeoutMs);
      active.timer.unref?.();
      active.settled = Promise.resolve().then(() => this.execute(active, caller)).finally(() => {
        this.active.delete(key);
        // Do not wait for listeners: cancellation can itself be waiting for
        // this promise while a host cleanup is waiting for cancellation.
        for (const listener of this.settlementListeners) {
          try { listener(structuredClone(record.owner)); } catch { /* A listener cannot disrupt task settlement. */ }
        }
      });
      return { runId: parsed.runId };
    });
  }
  private async assertRunAuthorized(active: ActiveRun): Promise<void> {
    const valid = !active.controller.signal.aborted && !this.stopped && (this.epochs.get(active.owner.appId) ?? 0) === active.appEpoch
      && await this.options.revalidate(active.owner, active.authorization).catch(() => false);
    if (!valid || this.stopped || (this.epochs.get(active.owner.appId) ?? 0) !== active.appEpoch || active.controller.signal.aborted) {
      active.controller.abort();
      throw new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.');
    }
  }
  private async execute(active: ActiveRun, caller: LocalAppAgentOwner): Promise<void> {
    const { record, controller, parsed } = active;
    const recordKey = requestKey(record.owner, record.publicRunId);
    const assertAuthorized = () => this.assertRunAuthorized(active);
    try {
      await assertAuthorized();
      await this.exclusive(async () => {
        if (TERMINAL.has(record.run.state)) return;
        record.run = { ...record.run, state: 'running', phase: 'thinking', sequence: record.run.sequence + 1, updatedAt: this.now() };
        await this.persist(record);
      });
      await assertAuthorized();
      const completion = await this.options.execute({
        runId: parsed.runId, taskStorageId: record.privateTaskId, owner: structuredClone(caller), input: structuredClone(parsed.executionInput),
        ...(active.chat ? { chat: structuredClone(active.chat) } : {}), authorization: structuredClone(active.authorization), signal: controller.signal, assertAuthorized,
        emit: async progress => {
          await assertAuthorized();
          if (!PHASES.has(progress.phase)) throw new LocalAppAgentError('invalid_event', 'The Agent returned an invalid progress event.');
          active.lastUsage = safeUsage(progress.usage) ?? active.lastUsage;
          await this.exclusive(async () => {
            if (TERMINAL.has(record.run.state)) return;
            record.run = { ...record.run,
              phase: progress.phase === 'waiting_for_permission' || progress.phase === 'waiting_for_answer' ? progress.phase : 'thinking',
              sequence: record.run.sequence + 1, updatedAt: this.now(),
              ...(active.lastUsage ? { usage: active.lastUsage } : {}),
              ...(progress.completedModelTurns !== undefined && Number.isSafeInteger(progress.completedModelTurns) && progress.completedModelTurns >= 0
                ? { progress: { completedModelTurns: progress.completedModelTurns, turnLimit: record.run.limits.maxTurns } } : {}) };
            await this.persist(record);
          });
        },
      });
      await assertAuthorized();
      active.lastUsage = safeUsage(completion?.usage) ?? active.lastUsage;
      if (!completion?.result || typeof completion.result.text !== 'string' || !Array.isArray(completion.result.attachments)
        || completion.result.attachments.length !== 0 || Buffer.byteLength(JSON.stringify(completion.result)) > 4 * 1024 * 1024) {
        throw new LocalAppAgentError('invalid_result', 'The Agent result is invalid or too large.');
      }
      if (parsed.executionInput.outputSchema && completion.result.output === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent did not produce a validated structured result.');
      if (parsed.executionInput.outputSchema && completion.result.output === null) throw new LocalAppAgentError('structured_output_not_representable', 'The structured output cannot be represented as an AG-UI run result.');
      if (typeof completion.result.messageId !== 'string' || !completion.result.messageId || completion.result.messageId.length > 256) throw new LocalAppAgentError('invalid_result', 'The Agent result is missing its native message identifier.');
      const usage = active.lastUsage;
      if (usage === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent completed without reporting usage data.');
      await this.finish(recordKey, 'succeeded', undefined, completion.result, usage);
    } catch (error) {
      const known = this.stopped ? { code: 'host_stopped', message: 'The Agent host stopped before this task completed.' }
        : error instanceof LocalAppAgentError ? { code: error.code, message: error.message }
        : { code: 'execution_failed', message: 'The Agent task could not be completed. Check the host configuration and try again.' };
      const state: LocalAppAgentTaskState = this.stopped ? 'interrupted' : controller.signal.aborted ? 'cancelled' : ['interrupted', 'executor_closed'].includes(known.code) ? 'interrupted' : 'failed';
      await this.finish(recordKey, state, known, undefined, active.lastUsage).catch(() => undefined);
    }
  }
  private async persist(record: LedgerRecord, beforeCommit?: () => Promise<void>, write?: AgentJsonWrite): Promise<void> {
    const key = requestKey(record.owner, record.publicRunId);
    if (this.uncertainTasks.has(key)) throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
    let guardFailed = false;
    const guardCommit = beforeCommit ? async () => {
      try { await beforeCommit(); } catch (error) { guardFailed = true; throw error; }
    } : undefined;
    try {
      await durableAgentJson(this.taskPath(record.owner, record.privateTaskId), record, guardCommit, write);
      await this.syncAgentParents(record.owner.appId, 'tasks');
    }
    catch (error) {
      if (guardFailed) throw error;
      if (error instanceof LocalAppAgentError && error.code === 'permission_revoked') throw error;
      this.uncertainTasks.add(key);
      if (record.chat) this.uncertainChats.add(this.chatKey(record.owner.appId, record.chat.chatId));
      throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
    }
  }
  async get(caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentTask> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => publicTask(this.lookup(principal, runId)));
  }
  async list(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown } = {}): Promise<{ tasks: LocalAppAgentTask[]; nextCursor?: string }> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => {
      if ([...this.records.values()].some(record => record.owner.appId === principal.appId && this.uncertainTasks.has(requestKey(record.owner, record.publicRunId)))) {
        throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
      }
      const rows = [...this.records.values()].filter(record => record.owner.appId === principal.appId).sort((a, b) => b.run.updatedAt - a.run.updatedAt || b.run.createdAt - a.run.createdAt);
      const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit), page = rows.slice(offset, offset + limit);
      const next = nextCursor(offset + page.length, offset + page.length < rows.length);
      return { tasks: page.map(publicTask), ...(next ? { nextCursor: next } : {}) };
    });
  }
  async events(caller: LocalAppAgentOwner, runId: string, afterSequence = 0): Promise<{ events: BaseEvent[]; sequence: number }> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => this.eventPage(this.lookup(principal, runId), afterSequence));
  }
  async cancel(caller: LocalAppAgentOwner, runId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId); signal.throwIfAborted(); this.lookup(principal, runId);
    await this.terminate(requestKey(principal, runId), 'cancelled', { code: 'cancelled', message: 'The Agent task was cancelled.' });
  }
  private summary(chat: ChatRecord): LocalAppAgentChatSummary {
    return { chatId: chat.chatId, title: chat.title, createdAt: chat.createdAt, updatedAt: chat.updatedAt, archived: chat.archived, state: chat.state };
  }
  private creationResult(principal: LocalAppAgentOwner, input: ChatCreation): LocalAppAgentChatSummary | null {
    const key = this.chatKey(principal.appId, input.requestId);
    if (this.uncertainChats.has(key)) throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
    const chat = this.chats.get(key), tombstone = this.chatTombstones.get(key);
    const row = chat ?? tombstone;
    if (!row) return null;
    if (row.creationFingerprint !== chatCreationFingerprint(input)) throw new LocalAppAgentError('chat_create_conflict', 'This Chat requestId is bound to different input.');
    if (tombstone) throw new LocalAppAgentError('chat_create_deleted', 'This Chat has been deleted.');
    return this.summary(chat!);
  }
  private async authorizeChatCreation(caller: LocalAppAgentOwner, appId: string, epoch: number, signal?: AbortSignal): Promise<LocalAppAgentAuthorization> {
    signal?.throwIfAborted();
    if (this.stopped || (this.epochs.get(appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while accessing the Chat.');
    const authorization = await this.options.authorize(caller);
    signal?.throwIfAborted();
    validateAgentSettings(authorization.settings);
    if (this.stopped || (this.epochs.get(appId) ?? 0) !== epoch
      || !(await this.options.revalidate(caller, authorization))
      || this.stopped || (this.epochs.get(appId) ?? 0) !== epoch) {
      throw new LocalAppAgentError('permission_revoked', 'Application access changed while accessing the Chat.');
    }
    signal?.throwIfAborted();
    return authorization;
  }
  async chatCreationStatus(caller: LocalAppAgentOwner, value: unknown, signal?: AbortSignal): Promise<LocalAppAgentChatSummary | null> {
    const principal = owner(caller), input = chatCreation(value), epoch = this.epochs.get(principal.appId) ?? 0;
    signal = this.operationSignal(principal.appId, signal);
    await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => {
      await this.authorizeChatCreation(caller, principal.appId, epoch, signal);
      return this.creationResult(principal, input);
    });
  }
  async createChat(caller: LocalAppAgentOwner, value: unknown, signal?: AbortSignal): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller), input = chatCreation(value), epoch = this.epochs.get(principal.appId) ?? 0;
    signal = this.operationSignal(principal.appId, signal);
    await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => {
      const authorization = await this.authorizeChatCreation(caller, principal.appId, epoch, signal);
      const existing = this.creationResult(principal, input);
      if (existing) return existing;
      const sessionId = this.uuid(), now = this.now(), chatId = input.requestId;
      if (!PRIVATE_TASK_ID.test(sessionId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate an Agent session identifier.');
      const chat: ChatRecord = { schema: 2, appId: principal.appId, chatId, sessionId, creationFingerprint: chatCreationFingerprint(input),
        title: input.title === undefined ? 'New chat' : input.title.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, ''), createdAt: now, updatedAt: now,
        archived: false, state: 'idle', messages: [] };
      const directory = this.chatDirectory(principal, chatId), key = this.chatKey(principal.appId, chatId);
      let directoryCreated = false;
      const chatWrite: AgentJsonWrite = { renameDispatched: false };
      let commitGuardFailure: unknown;
      try {
        await this.ensureAgentRoot(principal.appId);
        await this.safeDirectory(join(this.appRoot(principal.appId), 'chats'));
        signal.throwIfAborted();
        // A pre-existing directory is an unresolved claim, never an absent Chat.
        await fs.mkdir(directory, { mode: 0o700 });
        directoryCreated = true;
        await this.safeDirectory(join(directory, 'assets'));
        await this.saveChat(chat, async () => {
          try {
            signal.throwIfAborted();
            if (this.stopped || (this.epochs.get(principal.appId) ?? 0) !== epoch
              || !(await this.options.revalidate(caller, authorization))
              || this.stopped || (this.epochs.get(principal.appId) ?? 0) !== epoch) {
              throw new LocalAppAgentError('permission_revoked', 'Application access changed while creating the Chat.');
            }
            signal.throwIfAborted();
          } catch (error) {
            commitGuardFailure = error;
            throw error;
          }
        }, chatWrite);
        await this.syncAgentParents(principal.appId, 'chats');
        signal.throwIfAborted();
        return this.summary(chat);
      } catch (error) {
        if (directoryCreated && !chatWrite.renameDispatched && (signal.aborted || commitGuardFailure)) {
          try {
            await fs.rm(directory, { recursive: true, force: true });
            await this.syncAgentParents(principal.appId, 'chats');
            this.chats.delete(key); this.uncertainChats.delete(key);
          } catch {
            this.uncertainChats.add(key);
            throw new LocalAppAgentError('chat_storage_uncertain', 'Cancelled Chat creation could not be reconciled.');
          }
          if (signal.aborted) signal.throwIfAborted();
          throw commitGuardFailure;
        }
        if (!directoryCreated && signal.aborted) signal.throwIfAborted();
        this.uncertainChats.add(key);
        throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
      }
    });
  }
  async getChat(caller: LocalAppAgentOwner, chatId: string): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => this.summary(this.getChatRecord(principal, chatId)));
  }
  async listChats(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown; includeArchived?: unknown } = {}): Promise<LocalAppAgentChatListPage> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    if (options.includeArchived !== undefined && typeof options.includeArchived !== 'boolean') throw new LocalAppAgentError('invalid_input', 'Invalid archived-chat filter.');
    return this.exclusive(async () => {
      const prefix = `${principal.appId}:`;
      const unavailable = [...this.uncertainChats].filter(key => key.startsWith(prefix))
        .map(key => key.slice(prefix.length)).filter(chatId => CHAT_ID.test(chatId)).sort();
      const rows = [...this.chats.values()].filter(chat => chat.appId === principal.appId
        && !this.uncertainChats.has(this.chatKey(chat.appId, chat.chatId)) && (options.includeArchived || !chat.archived))
        .sort((a, b) => b.updatedAt - a.updatedAt || b.createdAt - a.createdAt || a.chatId.localeCompare(b.chatId));
      // One cursor covers both categories, with unavailable IDs after the
      // ordered Chat summaries. Every page signals whether any are present.
      const entries = [...rows.map(chat => ({ chat })), ...unavailable.map(chatId => ({ chatId }))];
      const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit), page = entries.slice(offset, offset + limit);
      const next = nextCursor(offset + page.length, offset + page.length < entries.length);
      const chats = page.flatMap(entry => 'chat' in entry ? [this.summary(entry.chat)] : []);
      const unavailableChats = page.flatMap(entry => 'chatId' in entry ? [{ chatId: entry.chatId, code: 'chat_storage_uncertain' as const }] : []);
      return { chats: structuredClone(chats), ...(unavailable.length ? { hasUncertainChats: true } : {}),
        ...(unavailableChats.length ? { unavailableChats } : {}), ...(next ? { nextCursor: next } : {}) };
    });
  }
  async archiveChat(caller: LocalAppAgentOwner, chatId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId);
    await this.exclusive(async () => {
      signal.throwIfAborted();
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be archived.');
      await this.saveChat({ ...chat, archived: true }, async () => { signal.throwIfAborted(); });
    });
  }
  async deleteChat(caller: LocalAppAgentOwner, chatId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId);
    await this.exclusive(async () => {
      signal.throwIfAborted();
      const key = this.chatKey(principal.appId, chatId);
      if (this.chatTombstones.has(key)) {
        try { await this.cleanupDeletedChat(principal, chatId); this.uncertainChats.delete(key); return; }
        catch { this.uncertainChats.add(key); throw new LocalAppAgentError('chat_storage_uncertain', 'Chat deletion could not be confirmed.'); }
      }
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be deleted.');
      await this.ensureAgentRoot(principal.appId);
      const directory = this.chatDirectory(principal, chatId), stat = await fs.lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent chat storage contains an unsafe directory.');
      const tombstone: ChatTombstone = { schema: 2, appId: principal.appId, chatId, creationFingerprint: chat.creationFingerprint, deleted: true };
      let guardFailed = false;
      try {
        await durableAgentJson(this.chatPath(principal, chatId), tombstone, async () => {
          try { signal.throwIfAborted(); } catch (error) { guardFailed = true; throw error; }
        });
        this.chatTombstones.set(key, tombstone);
        this.chats.delete(key);
        await this.cleanupDeletedChat(principal, chatId);
      } catch (error) {
        if (guardFailed) throw error;
        this.uncertainChats.add(key);
        throw new LocalAppAgentError('chat_storage_uncertain', 'Chat deletion could not be confirmed.');
      }
    });
  }
  async chatHistory(caller: LocalAppAgentOwner, chatId: string, options: { cursor?: unknown; limit?: unknown } = {}): Promise<LocalAppAgentChatHistory> {
    const principal = owner(caller); await this.ensureAppStore(principal.appId);
    return this.exclusive(async () => {
      await this.ensureAgentRoot(principal.appId);
      const chat = this.getChatRecord(principal, chatId), journal = new LocalAppChatAssetJournal(this.chatDirectory(principal, chatId));
      const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit, 100), stored = chat.messages.slice(offset, offset + limit);
      const { messages } = await journal.rehydrateMessages(stored.map(row => row.message));
      const next = nextCursor(offset + stored.length, offset + stored.length < chat.messages.length);
      return { messages: messages as Message[], ...(next ? { nextCursor: next } : {}) };
    });
  }
  async cancelChat(caller: LocalAppAgentOwner, chatId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.ensureAppStore(principal.appId); signal.throwIfAborted();
    // A finishing Chat holds the queue while its last rename is in flight.
    // Fence its active run before waiting for that queue, just like task.cancel.
    const active = [...this.active.values()].find(run => run.record.owner.appId === principal.appId && run.record.chat?.chatId === chatId);
    if (active) {
      this.lookup(principal, active.record.publicRunId);
      await this.terminate(requestKey(principal, active.record.publicRunId), 'cancelled', { code: 'cancelled', message: 'The Agent task was cancelled.' }, active);
      return;
    }
    const activeRunId = await this.exclusive(async () => this.getChatRecord(principal, chatId).activeRunId);
    if (activeRunId) await this.cancel(principal, activeRunId, signal);
  }
  async cancelApp(appId: string, reason = 'Application Agent access changed.'): Promise<void> {
    this.epochs.set(appId, (this.epochs.get(appId) ?? 0) + 1);
    const operations = this.appOperations.get(appId);
    this.appOperations.delete(appId);
    operations?.abort(new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.'));
    // No await before every active task has a durable termination fence.
    const captured = [...this.active.values()].filter(active => active.record.owner.appId === appId);
    let fenceFailure: unknown;
    for (const active of captured) {
      try { this.fenceActiveRun(active); }
      catch (error) { fenceFailure ??= error; }
      active.controller.abort();
    }
    await this.initialize();
    const runs = await this.exclusive(async () => {
      const byId = new Map(captured.map(active => [requestKey(active.record.owner, active.record.publicRunId), active]));
      for (const active of this.active.values()) if (active.record.owner.appId === appId) byId.set(requestKey(active.record.owner, active.record.publicRunId), active);
      return [...byId.values()];
    });
    const results = await Promise.allSettled(runs.map(active => this.terminate(requestKey(active.record.owner, active.record.publicRunId), 'cancelled', { code: 'application_stopped', message: reason.slice(0, 1024) }, active)));
    if (fenceFailure) throw fenceFailure;
    const failed = results.find(result => result.status === 'rejected');
    if (failed?.status === 'rejected') throw failed.reason;
  }
  /** Revoke only the active tasks actually admitted by this remote session. */
  async cancelRemoteSession(caller: LocalAppAgentOwner): Promise<void> {
    const principal = owner(caller);
    if (!principal.remoteSessionId) throw new LocalAppAgentError('invalid_owner', 'A remote browser session is required.');
    const runs = [...this.active.values()].filter(active => {
      const taskOwner = active.record.owner;
      return taskOwner.appId === principal.appId && taskOwner.releaseDigest === principal.releaseDigest
        && taskOwner.invocationId === principal.invocationId && taskOwner.remoteSessionId === principal.remoteSessionId;
    });
    // Set durable fences and abort every executor before yielding to the ledger
    // queue, which may still be held by another session's pending admission.
    let fenceFailure: unknown;
    for (const active of runs) {
      try { this.fenceActiveRun(active); }
      catch (error) { fenceFailure ??= error; }
      active.controller.abort();
    }
    const results = await Promise.allSettled(runs.map(active => this.terminate(requestKey(active.record.owner, active.record.publicRunId),
      'cancelled', { code: 'session_ended', message: 'The remote browser session ended.' }, active)));
    if (fenceFailure) throw fenceFailure;
    const failed = results.find(result => result.status === 'rejected');
    if (failed?.status === 'rejected') throw failed.reason;
  }
  private async terminate(recordKey: string, state: 'cancelled' | 'failed' | 'interrupted', error: { code: string; message: string }, captured?: ActiveRun): Promise<void> {
    const active = this.active.get(recordKey) ?? captured;
    if (active) {
      try { this.fenceActiveRun(active); }
      catch (error) { active.controller.abort(); throw error; }
      active.controller.abort();
    }
    await this.finish(recordKey, state, error, undefined, active?.lastUsage);
    await active?.settled;
    if (active) await this.clearTerminationFence(active);
  }
  private async finish(recordKey: string, state: LocalAppAgentTaskState, error?: { code: string; message: string }, result?: LocalAppAgentPrivateResult, usage?: LocalAppAgentUsage): Promise<void> {
    await this.exclusive(async () => {
      const active = this.active.get(recordKey), current = this.records.get(recordKey);
      if (this.uncertainTasks.has(recordKey)) throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
      if (!current || TERMINAL.has(current.run.state)) return;
      // Publish the terminal state and events only after their journal write.
      // If access changes during a successful write, replace it with a
      // cancelled outcome before releasing the serial queue to readers.
      while (true) {
        if (state === 'succeeded') {
          if (!active) throw new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.');
          await this.assertRunAuthorized(active);
        }
        const record = structuredClone(current);
        const finalUsage = safeUsage(usage);
        record.run = { ...record.run, state, phase: undefined, sequence: record.run.sequence + 1, updatedAt: this.now(),
          ...(finalUsage ? { usage: finalUsage } : {}), ...(error ? { error } : {}) };
        if (result && state === 'succeeded') {
          if (record.chat && (result.chatId !== record.chat.chatId || !result.messageId)) throw new LocalAppAgentError('invalid_result', 'The Agent returned invalid chat message provenance.');
          if (!result.messageId) throw new LocalAppAgentError('invalid_result', 'The Agent result is missing its native message identifier.');
          if (record.chat) record.assistantMessage = { id: result.messageId, role: 'assistant', content: result.text };
          this.appendEvent(record, { type: 'TEXT_MESSAGE_START', messageId: result.messageId, role: 'assistant', timestamp: this.now() });
          this.appendEvent(record, { type: 'TEXT_MESSAGE_CONTENT', messageId: result.messageId, delta: result.text, timestamp: this.now() });
          this.appendEvent(record, { type: 'TEXT_MESSAGE_END', messageId: result.messageId, timestamp: this.now() });
          let tokenUsage: Array<Record<string, number>> | undefined;
          if (finalUsage && (finalUsage.inputTokens !== undefined || finalUsage.outputTokens !== undefined)) {
            tokenUsage = [{ ...(finalUsage.inputTokens === undefined ? {} : { inputTokens: finalUsage.inputTokens }),
              ...(finalUsage.outputTokens === undefined ? {} : { outputTokens: finalUsage.outputTokens }) }];
          }
          const structuredResult = result.output;
          this.appendEvent(record, { type: 'RUN_FINISHED', threadId: record.threadId, runId: record.publicRunId, timestamp: this.now(),
            ...(structuredResult === undefined ? {} : { result: structuredClone(structuredResult) }), ...(tokenUsage ? { usage: tokenUsage } : {}) });
        } else if (state === 'cancelled') {
          const tokenUsage = finalUsage && (finalUsage.inputTokens !== undefined || finalUsage.outputTokens !== undefined)
            ? [{ ...(finalUsage.inputTokens === undefined ? {} : { inputTokens: finalUsage.inputTokens }),
              ...(finalUsage.outputTokens === undefined ? {} : { outputTokens: finalUsage.outputTokens }) }]
            : undefined;
          this.appendEvent(record, { type: 'RUN_FINISHED', threadId: record.threadId, runId: record.publicRunId, timestamp: this.now(), outcome: { type: 'cancelled' },
            ...(tokenUsage ? { usage: tokenUsage } : {}) });
        } else {
          const safeError = error ?? { code: 'execution_failed', message: 'The Agent task could not be completed.' };
          this.appendEvent(record, { type: 'RUN_ERROR', message: safeError.message, code: safeError.code, timestamp: this.now(),
            ...(finalUsage && (finalUsage.inputTokens !== undefined || finalUsage.outputTokens !== undefined) ? { usage: [{
              ...(finalUsage.inputTokens === undefined ? {} : { inputTokens: finalUsage.inputTokens }),
              ...(finalUsage.outputTokens === undefined ? {} : { outputTokens: finalUsage.outputTokens }),
            }] } : {}) });
        }
        if (state === 'succeeded') record.completionPending = true;
        await this.persist(record, state === 'succeeded' ? () => this.assertRunAuthorized(active!) : undefined);
        let previousChat: ChatRecord | undefined;
        const cancelPendingSuccess = async () => {
          if (previousChat) {
            previousChat.activeRunId = undefined; previousChat.state = 'interrupted';
            try { await this.saveChat(previousChat); }
            catch (error) { this.uncertainTasks.add(recordKey); throw error; }
          }
          state = 'cancelled';
          error = { code: 'permission_revoked', message: 'Application Agent access was revoked or changed.' };
          result = undefined;
        };
        if (state === 'succeeded') {
          try { await this.assertRunAuthorized(active!); }
          catch { await cancelPendingSuccess(); continue; }
        }
        if (record.chat) {
          const chat = this.chats.get(this.chatKey(record.owner.appId, record.chat.chatId));
          if (!chat) {
            this.uncertainTasks.add(recordKey);
            this.uncertainChats.add(this.chatKey(record.owner.appId, record.chat.chatId));
            throw new LocalAppAgentError('chat_storage_uncertain', 'Chat storage could not be confirmed.');
          }
          previousChat = structuredClone(chat);
          chat.activeRunId = undefined; chat.state = state === 'succeeded' ? 'idle' : 'interrupted';
          if (state === 'succeeded' && record.assistantMessage && !chat.messages.some(message => messageId(message.message) === record.assistantMessage!.id)) {
            chat.messages.push({ message: structuredClone(record.assistantMessage), runId: record.publicRunId });
          }
          try { await this.saveChat(chat); }
          catch (error) { this.uncertainTasks.add(recordKey); throw error; }
          if (active?.assetJournal) {
            try { await active.assetJournal.pruneUnreferenced(chat.messages.map(message => message.message)); }
            catch {
              this.uncertainTasks.add(recordKey);
              this.uncertainChats.add(this.chatKey(record.owner.appId, record.chat.chatId));
              throw new LocalAppAgentError('agent_storage_unavailable', 'Agent task storage could not be confirmed.');
            }
          }
        }
        if (state === 'succeeded') {
          try { await this.assertRunAuthorized(active!); }
          catch { await cancelPendingSuccess(); continue; }
        }
        if (record.completionPending) {
          delete record.completionPending;
          try { await this.persist(record, () => this.assertRunAuthorized(active!)); }
          catch (error) {
            if (error instanceof LocalAppAgentError && error.code === 'permission_revoked') {
              await cancelPendingSuccess(); continue;
            }
            throw error;
          }
          try { await this.assertRunAuthorized(active!); }
          catch { await cancelPendingSuccess(); continue; }
        }
        if (state === 'succeeded') {
          // No asynchronous gap between the last generation check, fence
          // removal and publication: this is the completion linearization point.
          if (!active || this.stopped || active.controller.signal.aborted || (this.epochs.get(active.owner.appId) ?? 0) !== active.appEpoch) {
            await cancelPendingSuccess(); continue;
          }
        }
        if (active) this.clearSettledRunFence(active);
        Object.assign(current, record);
        if (active?.timer) clearTimeout(active.timer);
        break;
      }
    });
  }
  async dispose(): Promise<void> {
    this.stopped = true;
    const operations = [...this.appOperations.values()];
    this.appOperations.clear();
    for (const controller of operations) controller.abort(new LocalAppAgentError('host_stopped', 'The Agent host is stopping.'));
    // Shutdown may race a final rename just like an explicit cancellation.
    const captured = [...this.active.values()];
    let fenceFailure: unknown;
    for (const active of captured) {
      try { this.fenceActiveRun(active); }
      catch (error) { fenceFailure ??= error; }
      active.controller.abort();
    }
    await this.initialize();
    const runs = await this.exclusive(async () => {
      const byId = new Map(captured.map(active => [requestKey(active.record.owner, active.record.publicRunId), active]));
      for (const active of this.active.values()) byId.set(requestKey(active.record.owner, active.record.publicRunId), active);
      return [...byId.values()];
    });
    const results = await Promise.allSettled(runs.map(active => this.terminate(requestKey(active.record.owner, active.record.publicRunId), 'interrupted',
      { code: 'host_stopped', message: 'The Agent host stopped before this task completed.' }, active)));
    if (fenceFailure) throw fenceFailure;
    const failed = results.find(result => result.status === 'rejected');
    if (failed?.status === 'rejected') throw failed.reason;
  }
}
