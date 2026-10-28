import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { BaseEvent, Message, RunAgentInput } from '@ag-ui/core';
import { EventSchema } from '@lingxi/local-app-agui/client';
import type {
  LocalAppAgentChatHistory, LocalAppAgentChatSummary, LocalAppAgentModelChoice,
  LocalAppAgentSettings, LocalAppAgentTask, LocalAppAgentTaskState, LocalAppAgentUsage,
} from '../../shared/localAppAgent.js';
import { LOCAL_APP_AGENT_MAX_TIMEOUT_MS, LOCAL_APP_AGENT_MAX_TURNS } from '../../shared/localAppAgent.js';
import { atomicJson } from './store.js';
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
}
interface ChatRecord {
  schema: 1;
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
interface ActiveRun {
  record: LedgerRecord;
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
const CHAT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const PRIVATE_TASK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const MAX_INPUT_BYTES = 116 * 1024 * 1024;
const MAX_EVENT_PAGE = 128;
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
  private readonly chats = new Map<string, ChatRecord>();
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
  private async saveChat(chat: ChatRecord, signal?: AbortSignal): Promise<void> {
    await this.ensureAgentRoot(chat.appId);
    await this.safeDirectory(this.chatDirectory({ appId: chat.appId }, chat.chatId));
    signal?.throwIfAborted();
    chat.updatedAt = this.now();
    await atomicJson(this.chatPath({ appId: chat.appId }, chat.chatId), chat, signal);
    this.chats.set(this.chatKey(chat.appId, chat.chatId), chat);
  }
  private chatKey(appId: string, chatId: string): string { return `${appId}:${chatId}`; }
  private async resetPrototypeAgentStorage(): Promise<void> {
    const appsPath = join(this.options.root, 'apps');
    await this.safeDirectory(appsPath);
    for (const entry of await fs.readdir(appsPath, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.isSymbolicLink() || !/^a[a-f0-9]{32}$/.test(entry.name)) continue;
      const agentPath = join(appsPath, entry.name, 'agent');
      let agentStat;
      try { agentStat = await fs.lstat(agentPath); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
      if (!agentStat.isDirectory() || agentStat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe directory.');
      for (const name of ['tasks', 'chats', 'assets']) {
        const path = join(agentPath, name);
        let stat;
        try { stat = await fs.lstat(path); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
        if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe legacy entry.');
        await fs.rm(path, { recursive: true, force: false });
      }
    }
  }
  private async ensureStorageFormat(): Promise<void> {
    const path = join(this.options.root, 'desktop-agent-format.json');
    try {
      const marker = JSON.parse(await fs.readFile(path, 'utf8'));
      if (marker?.schemaVersion !== 1 || marker?.protocol !== 'ag-ui/1.0') throw new LocalAppAgentError('ledger_invalid', 'The Desktop Agent store format is unsupported.');
      return;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    await this.resetPrototypeAgentStorage();
    await atomicJson(path, { schemaVersion: 1, protocol: 'ag-ui/1.0' });
  }
  private appendEvent(record: LedgerRecord, value: unknown): BaseEvent {
    const event = EventSchema.parse(value) as BaseEvent;
    record.events.push(event);
    record.eventSequence = record.events.length;
    return event;
  }
  private async load(): Promise<void> {
    await this.safeDirectory(this.options.root);
    await this.ensureStorageFormat();
    const appsPath = join(this.options.root, 'apps');
    await this.safeDirectory(appsPath);
    for (const appEntry of await fs.readdir(appsPath, { withFileTypes: true })) {
      if (!appEntry.isDirectory() || appEntry.isSymbolicLink() || !/^a[a-f0-9]{32}$/.test(appEntry.name)) continue;
      const appId = appEntry.name, agentRoot = this.appRoot(appId);
      let agentStat;
      try { agentStat = await fs.lstat(agentRoot); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
      if (!agentStat.isDirectory() || agentStat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe application directory.');
      for (const group of ['tasks', 'chats']) {
        const groupPath = join(agentRoot, group);
        try { await fs.access(groupPath); } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
        const groupStat = await fs.lstat(groupPath);
        if (!groupStat.isDirectory() || groupStat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent storage contains an unsafe directory.');
        const entries = await fs.readdir(groupPath, { withFileTypes: true });
        if (group === 'tasks') {
          for (const entry of entries) {
            if (!entry.name.endsWith('.json')) continue;
            if (!entry.isFile() || entry.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger is invalid.');
            const path = join(groupPath, entry.name), record = JSON.parse(await fs.readFile(path, 'utf8')) as LedgerRecord;
            const principal = owner(record.owner);
            if (principal.appId !== appId || record.schema !== 1 || !PRIVATE_TASK_ID.test(record.privateTaskId) || `${record.privateTaskId}.json` !== entry.name
              || !validPublicId(record.publicRunId) || !validPublicId(record.threadId) || typeof record.fingerprint !== 'string' || !Array.isArray(record.events)
              || record.eventSequence !== record.events.length || !['queued', 'running', ...TERMINAL].includes(record.run?.state)
              || !Number.isSafeInteger(record.run?.sequence) || !Number.isFinite(record.run?.createdAt) || !Number.isFinite(record.run?.updatedAt)) {
              throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger is invalid.');
            }
            record.events = record.events.map(event => EventSchema.parse(event));
            const started = record.events[0];
            if (started?.type !== 'RUN_STARTED' || started.runId !== record.publicRunId || started.threadId !== record.threadId) throw new LocalAppAgentError('ledger_invalid', 'The Agent event journal is invalid.');
            if (!TERMINAL.has(record.run.state)) {
              record.run.state = 'interrupted'; record.run.phase = undefined; record.run.sequence += 1; record.run.updatedAt = this.now();
              record.run.error = { code: 'host_restarted', message: 'The host stopped before this task completed. The task was not replayed.' };
              this.appendEvent(record, { type: 'RUN_ERROR', message: record.run.error.message, code: record.run.error.code, timestamp: this.now() });
              await atomicJson(path, record);
            }
            const key = requestKey(principal, record.publicRunId);
            if (this.requests.has(key) || this.records.has(key)) throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger contains conflicting run IDs.');
            this.records.set(key, record); this.requests.set(key, key);
          }
        } else {
          for (const entry of entries) {
            if (!entry.isDirectory() || entry.isSymbolicLink() || !CHAT_ID.test(entry.name)) continue;
            const directory = join(groupPath, entry.name), path = join(directory, 'chat.json');
            const stat = await fs.lstat(directory);
            if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            const file = await fs.lstat(path);
            if (!file.isFile() || file.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            const chat = JSON.parse(await fs.readFile(path, 'utf8')) as ChatRecord;
            if (chat.schema !== 1 || chat.appId !== appId || chat.chatId !== entry.name || typeof chat.sessionId !== 'string'
              || !Array.isArray(chat.messages) || chat.messages.some(message => !message || typeof message !== 'object' || !('message' in message))
              || !['idle', 'running', 'interrupted'].includes(chat.state)) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            if (chat.state === 'running' || chat.activeRunId) { chat.state = 'interrupted'; chat.activeRunId = undefined; chat.updatedAt = this.now(); await atomicJson(path, chat); }
            this.chats.set(this.chatKey(appId, chat.chatId), chat);
          }
        }
      }
    }
    // Drop a user-message write whose paired task record was not committed.
    // Admission commits the chat row then the task file; a crash between them
    // must not leave a ghost turn that blocks an exact retry.
    for (const chat of this.chats.values()) {
      const previousLength = chat.messages.length;
      chat.messages = chat.messages.filter(row => {
        if (typeof row.runId !== 'string') return false;
        const record = typeof row.runId === 'string' ? this.records.get(requestKey({ appId: chat.appId }, row.runId)) : undefined;
        if (!record?.chat || record.chat.chatId !== chat.chatId || record.owner.appId !== chat.appId) return false;
        const id = messageId(row.message);
        return id === record.chat.userMessageId || (record.assistantMessage && id === record.assistantMessage.id);
      });
      if (chat.messages.length !== previousLength) {
        await this.saveChat(chat);
        await new LocalAppChatAssetJournal(this.chatDirectory({ appId: chat.appId }, chat.chatId))
          .pruneUnreferenced(chat.messages.map(message => message.message));
      }
    }
    // Recover the small commit window between a task's terminal event and its chat read-model write.
    for (const record of this.records.values()) if (record.chat && record.run.state === 'succeeded' && record.assistantMessage) {
      const chat = this.chats.get(this.chatKey(record.owner.appId, record.chat.chatId));
      if (chat && !chat.messages.some(message => messageId(message.message) === record.assistantMessage!.id)) {
        chat.messages.push({ message: structuredClone(record.assistantMessage), runId: record.publicRunId });
        chat.state = 'idle'; chat.activeRunId = undefined; await this.saveChat(chat);
      }
    }
  }
  private lookup(caller: LocalAppAgentOwner, runId: string): LedgerRecord {
    const key = requestKey(caller, runId);
    const record = this.records.get(key);
    if (!record || record.owner.appId !== caller.appId || (!TERMINAL.has(record.run.state)
      && (record.owner.invocationId !== caller.invocationId || record.owner.releaseDigest !== caller.releaseDigest))) {
      throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    }
    return record;
  }
  private getChatRecord(caller: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): ChatRecord {
    const chat = this.chats.get(this.chatKey(caller.appId, chatId));
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
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize();
    const threadId = messageRecord(value)?.threadId;
    const chat = typeof threadId === 'string' ? this.chats.get(this.chatKey(principal.appId, threadId)) : undefined;
    return this.admit(principal, value, chat, signal);
  }
  async sendChat(caller: LocalAppAgentOwner, chatId: string, value: unknown, signal?: AbortSignal): Promise<{ runId: string }> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize();
    const chat = this.getChatRecord(principal, chatId);
    return this.admit(principal, value, chat, signal);
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
      const taskPath = this.taskPath(principal, privateTaskId);
      await this.ensureAgentRoot(principal.appId);
      await this.safeDirectory(join(this.appRoot(principal.appId), 'tasks'));
      signal?.throwIfAborted();
      if (chat && assetJournal) {
        const previous = structuredClone(chat);
        let taskWritten = false;
        try {
          const stored = await assetJournal.storeMessage(parsed.currentMessage);
          signal?.throwIfAborted();
          chat.messages.push({ message: stored.message, runId: parsed.runId });
          chat.state = 'running'; chat.activeRunId = parsed.runId;
          await this.saveChat(chat, signal);
          signal?.throwIfAborted();
          await atomicJson(taskPath, record, signal); taskWritten = true;
          signal?.throwIfAborted();
        } catch (error) {
          this.chats.set(this.chatKey(chat.appId, chat.chatId), previous);
          await atomicJson(this.chatPath(previous, previous.chatId), previous).catch(() => undefined);
          if (taskWritten) await fs.rm(taskPath, { force: true }).catch(() => undefined);
          await assetJournal.pruneUnreferenced(previous.messages.map(message => message.message)).catch(() => undefined);
          throw error;
        }
      } else {
        await atomicJson(taskPath, record, signal);
        if (signal?.aborted) {
          await fs.rm(taskPath, { force: true }).catch(() => undefined);
          signal.throwIfAborted();
        }
      }
      const active: ActiveRun = {
        record, authorization, controller: new AbortController(), appEpoch: epoch, parsed,
        ...(assetJournal ? { assetJournal } : {}),
        ...(chat ? { chat: { chatId: chat.chatId, sessionId: chat.sessionId, sessionDirectory: this.chatDirectory(principal, chat.chatId) } } : {}),
      };
      this.records.set(key, record); this.requests.set(key, key); this.active.set(key, active);
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
  private async execute(active: ActiveRun, caller: LocalAppAgentOwner): Promise<void> {
    const { record, controller, parsed } = active;
    const recordKey = requestKey(record.owner, record.publicRunId);
    const assertAuthorized = async () => {
      controller.signal.throwIfAborted();
      if (this.stopped || (this.epochs.get(caller.appId) ?? 0) !== active.appEpoch || !(await this.options.revalidate(caller, active.authorization))) {
        controller.abort(); throw new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.');
      }
      controller.signal.throwIfAborted();
    };
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
      const known = error instanceof LocalAppAgentError ? { code: error.code, message: error.message }
        : { code: 'execution_failed', message: 'The Agent task could not be completed. Check the host configuration and try again.' };
      const state: LocalAppAgentTaskState = controller.signal.aborted ? 'cancelled' : ['interrupted', 'executor_closed'].includes(known.code) ? 'interrupted' : 'failed';
      await this.finish(recordKey, state, known, undefined, active.lastUsage).catch(() => undefined);
    }
  }
  private async persist(record: LedgerRecord): Promise<void> { await atomicJson(this.taskPath(record.owner, record.privateTaskId), record); }
  async get(caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentTask> {
    const principal = owner(caller); await this.initialize();
    return this.exclusive(async () => publicTask(this.lookup(principal, runId)));
  }
  async list(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown } = {}): Promise<{ tasks: LocalAppAgentTask[]; nextCursor?: string }> {
    const principal = owner(caller); await this.initialize();
    return this.exclusive(async () => {
      const rows = [...this.records.values()].filter(record => record.owner.appId === principal.appId).sort((a, b) => b.run.updatedAt - a.run.updatedAt || b.run.createdAt - a.run.createdAt);
      const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit), page = rows.slice(offset, offset + limit);
      const next = nextCursor(offset + page.length, offset + page.length < rows.length);
      return { tasks: page.map(publicTask), ...(next ? { nextCursor: next } : {}) };
    });
  }
  async events(caller: LocalAppAgentOwner, runId: string, afterSequence = 0): Promise<{ events: BaseEvent[]; sequence: number }> {
    const principal = owner(caller); await this.initialize();
    return this.exclusive(async () => this.eventPage(this.lookup(principal, runId), afterSequence));
  }
  async cancel(caller: LocalAppAgentOwner, runId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize(); signal.throwIfAborted(); this.lookup(principal, runId);
    await this.terminate(requestKey(principal, runId), 'cancelled', { code: 'cancelled', message: 'The Agent task was cancelled.' });
  }
  private summary(chat: ChatRecord): LocalAppAgentChatSummary {
    return { chatId: chat.chatId, title: chat.title, createdAt: chat.createdAt, updatedAt: chat.updatedAt, archived: chat.archived, state: chat.state };
  }
  async createChat(caller: LocalAppAgentOwner, value: unknown = {}, signal?: AbortSignal): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize();
    return this.exclusive(async () => {
      signal.throwIfAborted();
      const input = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
      if (!input || Object.keys(input).some(key => key !== 'title') || (input.title !== undefined && (typeof input.title !== 'string' || input.title.trim().length > 160))) throw new LocalAppAgentError('invalid_input', 'Invalid Agent chat title.');
      const authorization = await this.options.authorize(caller); validateAgentSettings(authorization.settings);
      signal?.throwIfAborted();
      const chatId = this.uuid(), sessionId = this.uuid(), now = this.now();
      if (!CHAT_ID.test(chatId) || !CHAT_ID.test(sessionId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate an Agent chat identifier.');
      const chat: ChatRecord = { schema: 1, appId: principal.appId, chatId, sessionId,
        title: typeof input.title === 'string' && input.title.trim() ? input.title.trim() : 'New chat', createdAt: now, updatedAt: now,
        archived: false, state: 'idle', messages: [] };
      await this.ensureAgentRoot(principal.appId);
      await this.safeDirectory(join(this.appRoot(principal.appId), 'chats'));
      signal?.throwIfAborted();
      const directory = this.chatDirectory(principal, chatId);
      try { await fs.mkdir(directory, { mode: 0o700 }); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'EEXIST') throw new LocalAppAgentError('id_conflict', 'Could not allocate an Agent chat identifier.');
        throw error;
      }
      try {
        await this.safeDirectory(join(directory, 'assets'));
        await this.saveChat(chat, signal);
        return this.summary(chat);
      } catch (error) {
        await fs.rm(directory, { recursive: true, force: true }).catch(() => undefined);
        throw error;
      }
    });
  }
  async getChat(caller: LocalAppAgentOwner, chatId: string): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller); await this.initialize();
    return this.exclusive(async () => this.summary(this.getChatRecord(principal, chatId)));
  }
  async listChats(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown; includeArchived?: unknown } = {}): Promise<{ chats: LocalAppAgentChatSummary[]; nextCursor?: string }> {
    const principal = owner(caller); await this.initialize();
    if (options.includeArchived !== undefined && typeof options.includeArchived !== 'boolean') throw new LocalAppAgentError('invalid_input', 'Invalid archived-chat filter.');
    return this.exclusive(async () => {
      const rows = [...this.chats.values()].filter(chat => chat.appId === principal.appId && (options.includeArchived || !chat.archived))
        .sort((a, b) => b.updatedAt - a.updatedAt || b.createdAt - a.createdAt);
      const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit), page = rows.slice(offset, offset + limit);
      const next = nextCursor(offset + page.length, offset + page.length < rows.length);
      return { chats: structuredClone(page.map(chat => this.summary(chat))), ...(next ? { nextCursor: next } : {}) };
    });
  }
  async archiveChat(caller: LocalAppAgentOwner, chatId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize();
    await this.exclusive(async () => {
      signal?.throwIfAborted();
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be archived.');
      await this.saveChat({ ...chat, archived: true }, signal);
    });
  }
  async deleteChat(caller: LocalAppAgentOwner, chatId: string, signal?: AbortSignal): Promise<void> {
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize();
    await this.exclusive(async () => {
      signal?.throwIfAborted();
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be deleted.');
      await this.ensureAgentRoot(principal.appId);
      const directory = this.chatDirectory(principal, chatId), stat = await fs.lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'Agent chat storage contains an unsafe directory.');
      signal?.throwIfAborted();
      await fs.rm(directory, { recursive: true, force: false });
      this.chats.delete(this.chatKey(principal.appId, chatId));
    });
  }
  async chatHistory(caller: LocalAppAgentOwner, chatId: string, options: { cursor?: unknown; limit?: unknown } = {}): Promise<LocalAppAgentChatHistory> {
    const principal = owner(caller); await this.initialize();
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
    const principal = owner(caller); signal = this.operationSignal(principal.appId, signal); await this.initialize(); signal.throwIfAborted();
    const chat = this.getChatRecord(principal, chatId);
    if (chat.activeRunId) await this.cancel(principal, chat.activeRunId, signal);
  }
  async cancelApp(appId: string, reason = 'Application Agent access changed.'): Promise<void> {
    this.epochs.set(appId, (this.epochs.get(appId) ?? 0) + 1);
    const operations = this.appOperations.get(appId);
    this.appOperations.delete(appId);
    operations?.abort(new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.'));
    const runs = [...this.active.values()].filter(active => active.record.owner.appId === appId);
    for (const active of runs) active.controller.abort();
    await Promise.all(runs.map(active => this.terminate(requestKey(active.record.owner, active.record.publicRunId), 'cancelled', { code: 'application_stopped', message: reason.slice(0, 1024) })));
  }
  /** Session expiry must cancel its actual tasks, never a different session's idempotent retry. */
  async cancelRemoteSession(caller: LocalAppAgentOwner): Promise<void> {
    const principal = owner(caller);
    if (!principal.remoteSessionId) throw new LocalAppAgentError('invalid_owner', 'A remote browser session is required.');
    // Abort active work synchronously: the ledger lock may be waiting on a
    // different session's authorization or disk I/O. Persist its terminal state
    // through the normal queue after the executor has received cancellation.
    const runs = [...this.active.values()].filter(active => {
      const taskOwner = active.record.owner;
      return taskOwner.appId === principal.appId && taskOwner.releaseDigest === principal.releaseDigest
        && taskOwner.invocationId === principal.invocationId && taskOwner.remoteSessionId === principal.remoteSessionId;
    });
    for (const active of runs) active.controller.abort();
    await Promise.all(runs.map(active => this.terminate(requestKey(active.record.owner, active.record.publicRunId), 'cancelled', { code: 'session_ended', message: 'The remote browser session ended.' })));
  }
  private async terminate(recordKey: string, state: 'cancelled' | 'failed' | 'interrupted', error: { code: string; message: string }): Promise<void> {
    const active = this.active.get(recordKey); active?.controller.abort();
    await this.finish(recordKey, state, error, undefined, active?.lastUsage);
    await active?.settled;
  }
  private async finish(recordKey: string, state: LocalAppAgentTaskState, error?: { code: string; message: string }, result?: LocalAppAgentPrivateResult, usage?: LocalAppAgentUsage): Promise<void> {
    await this.exclusive(async () => {
      const active = this.active.get(recordKey), record = this.records.get(recordKey);
      if (!record || TERMINAL.has(record.run.state)) return;
      // Revocation can happen after the executor's last authorization check
      // while this success is waiting behind another ledger operation.
      if (state === 'succeeded' && this.stopped && active && !active.controller.signal.aborted) {
        throw new LocalAppAgentError('executor_closed', 'The Agent host stopped before this task completed.');
      }
      if (state === 'succeeded' && (!active || active.controller.signal.aborted
        || (this.epochs.get(record.owner.appId) ?? 0) !== active.appEpoch)) {
        throw new LocalAppAgentError('permission_revoked', 'Application Agent access was revoked or changed.');
      }
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
      await this.persist(record);
      if (active?.timer) clearTimeout(active.timer);
      if (record.chat) {
        const chat = this.chats.get(this.chatKey(record.owner.appId, record.chat.chatId));
        if (chat) {
          chat.activeRunId = undefined; chat.state = state === 'succeeded' ? 'idle' : 'interrupted';
          if (state === 'succeeded' && record.assistantMessage && !chat.messages.some(message => messageId(message.message) === record.assistantMessage!.id)) {
            chat.messages.push({ message: structuredClone(record.assistantMessage), runId: record.publicRunId });
          }
          await this.saveChat(chat);
          if (active?.assetJournal) await active.assetJournal.pruneUnreferenced(chat.messages.map(message => message.message));
        }
      }
    });
  }
  async dispose(): Promise<void> {
    this.stopped = true;
    const operations = [...this.appOperations.values()];
    this.appOperations.clear();
    for (const controller of operations) controller.abort(new LocalAppAgentError('host_stopped', 'The Agent host is stopping.'));
    const runs = [...this.active.entries()];
    for (const [, active] of runs) active.controller.abort();
    const terminations = runs.map(([recordKey]) => this.terminate(recordKey, 'interrupted', { code: 'host_stopped', message: 'The Agent host stopped before this task completed.' }));
    const drained = this.exclusive(async () => {});
    await Promise.all([this.initialize(), drained, ...terminations]);
  }
}
