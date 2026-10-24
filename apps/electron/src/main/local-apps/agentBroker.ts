import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  LocalAppAgentChatHistory, LocalAppAgentChatMessage, LocalAppAgentChatSummary, LocalAppAgentEvent,
  LocalAppAgentModelChoice, LocalAppAgentProgressPhase, LocalAppAgentResult, LocalAppAgentRun,
  LocalAppAgentRunInput, LocalAppAgentSettings, LocalAppAgentUsage,
} from '../../shared/localAppAgent.js';
import { LOCAL_APP_AGENT_MAX_TIMEOUT_MS, LOCAL_APP_AGENT_MAX_TURNS } from '../../shared/localAppAgent.js';
import { atomicJson } from './store.js';
import { agentDigest, LocalAppAgentError, parseAgentRunInput, validateAgentSettings } from './agentPolicy.js';
export { LocalAppAgentError } from './agentPolicy.js';

/** Constructed by the trusted main-frame host; never accepted from the page. */
export interface LocalAppAgentOwner { appId: string; releaseDigest: string; invocationId: string; webContentsId?: number }
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
export interface LocalAppAgentReleaseFile { path: string; sha256: string; bytes: number; executable: boolean }
export interface LocalAppAgentExecutorResult { result: LocalAppAgentResult; usage?: LocalAppAgentUsage }
export interface LocalAppAgentExecution {
  runId: string;
  owner: LocalAppAgentOwner;
  input: LocalAppAgentRunInput;
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

interface LedgerRecord {
  schema: 2;
  owner: LocalAppAgentOwner;
  fingerprint: string;
  run: LocalAppAgentRun;
  chat?: { chatId: string; userMessageId: string; assistantMessageId: string };
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
  messages: LocalAppAgentChatMessage[];
}
interface ActiveRun {
  record: LedgerRecord;
  authorization: LocalAppAgentAuthorization;
  controller: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  appEpoch: number;
  chat?: LocalAppAgentAppChat;
  lastUsage?: LocalAppAgentUsage;
  settled?: Promise<void>;
}

const TERMINAL = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
const PHASES = new Set<LocalAppAgentProgressPhase>(['starting', 'thinking', 'tool', 'waiting_for_permission', 'waiting_for_answer']);
const CHAT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
function owner(value: LocalAppAgentOwner): LocalAppAgentOwner {
  if (!value || !/^a[a-f0-9]{32}$/.test(value.appId) || typeof value.releaseDigest !== 'string' || !/^[a-f0-9]{64}$/.test(value.releaseDigest)
    || typeof value.invocationId !== 'string' || !value.invocationId || value.invocationId.length > 256) throw new LocalAppAgentError('invalid_owner', 'Invalid application execution identity.');
  // A transient Electron window ID does not define durable ownership.
  return { appId: value.appId, releaseDigest: value.releaseDigest, invocationId: value.invocationId };
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
function requestKey(value: LocalAppAgentOwner, requestId: string): string { return JSON.stringify([value.appId, value.releaseDigest, requestId]); }
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

/** Durable app-scoped task and chat index. Harness JSONL remains model history. */
export class LocalAppAgentBroker {
  private ready?: Promise<void>;
  private queue = Promise.resolve();
  private readonly records = new Map<string, LedgerRecord>();
  private readonly requests = new Map<string, string>();
  private readonly chats = new Map<string, ChatRecord>();
  private readonly active = new Map<string, ActiveRun>();
  private readonly epochs = new Map<string, number>();
  private readonly listeners = new Map<string, Set<(event: LocalAppAgentEvent) => void>>();
  private stopped = false;
  constructor(private readonly options: LocalAppAgentBrokerOptions) {}
  /** Includes a terminal task until its executor has actually closed. */
  hasActiveApp(appId: string): boolean { return [...this.active.values()].some(active => active.record.owner.appId === appId); }
  private now(): number { return this.options.now?.() ?? Date.now(); }
  private uuid(): string { return this.options.uuid?.() ?? randomUUID(); }
  private exclusive<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.queue.then(operation, operation);
    this.queue = result.then(() => undefined, () => undefined);
    return result;
  }
  initialize(): Promise<void> { return this.ready ??= this.load(); }
  private appRoot(appId: string): string { return join(this.options.root, 'apps', appId, 'agent'); }
  private taskPath(principal: Pick<LocalAppAgentOwner, 'appId'>, runId: string): string {
    if (!/^[a-zA-Z0-9_-]{1,128}$/.test(runId)) throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    return join(this.appRoot(principal.appId), 'tasks', `${runId}.json`);
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
  private async saveChat(chat: ChatRecord): Promise<void> {
    chat.updatedAt = this.now();
    await atomicJson(this.chatPath({ appId: chat.appId }, chat.chatId), chat);
    this.chats.set(`${chat.appId}:${chat.chatId}`, chat);
  }
  private chatKey(appId: string, chatId: string): string { return `${appId}:${chatId}`; }
  private async load(): Promise<void> {
    await this.safeDirectory(this.options.root);
    const appsPath = join(this.options.root, 'apps');
    try { await this.safeDirectory(appsPath); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return; throw error; }
    const appEntries = await fs.readdir(appsPath, { withFileTypes: true });
    for (const appEntry of appEntries) {
      if (!appEntry.isDirectory() || appEntry.isSymbolicLink() || !/^a[a-f0-9]{32}$/.test(appEntry.name)) continue;
      const appId = appEntry.name, agentRoot = this.appRoot(appId);
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
            if (principal.appId !== appId || record.schema !== 2 || `${record.run?.runId}.json` !== entry.name || typeof record.fingerprint !== 'string'
              || !['queued', 'running', ...TERMINAL].includes(record.run.state) || !Number.isSafeInteger(record.run.sequence) || typeof record.run.requestId !== 'string') {
              throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger is invalid.');
            }
            if (!TERMINAL.has(record.run.state)) {
              record.run.state = 'interrupted'; record.run.phase = undefined; record.run.updatedAt = this.now(); record.run.sequence++;
              record.run.error = { code: 'host_restarted', message: 'The host stopped before this task completed. The task was not replayed.' };
              await atomicJson(path, record);
            }
            const key = requestKey(record.owner, record.run.requestId);
            if (this.requests.has(key) || this.records.has(record.run.runId)) throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger contains conflicting requests.');
            this.records.set(record.run.runId, record); this.requests.set(key, record.run.runId);
          }
        } else {
          for (const entry of entries) {
            if (!entry.isDirectory() || entry.isSymbolicLink() || !CHAT_ID.test(entry.name)) continue;
            const path = join(groupPath, entry.name, 'chat.json'), stat = await fs.lstat(join(groupPath, entry.name));
            if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            const file = await fs.lstat(path);
            if (!file.isFile() || file.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            const chat = JSON.parse(await fs.readFile(path, 'utf8')) as ChatRecord;
            if (chat.schema !== 1 || chat.appId !== appId || chat.chatId !== entry.name || typeof chat.sessionId !== 'string'
              || !Array.isArray(chat.messages) || !['idle', 'running', 'interrupted'].includes(chat.state)) throw new LocalAppAgentError('ledger_invalid', 'The Agent chat ledger is invalid.');
            if (chat.state === 'running' || chat.activeRunId) {
              chat.state = 'interrupted'; chat.activeRunId = undefined; chat.updatedAt = this.now(); await atomicJson(path, chat);
            }
            this.chats.set(this.chatKey(appId, chat.chatId), chat);
          }
        }
      }
    }
    // Repair a crash after task commit but before the cached assistant row.
    for (const record of this.records.values()) if (record.chat && record.run.state === 'succeeded' && record.run.result) {
      const chat = this.chats.get(this.chatKey(record.owner.appId, record.chat.chatId));
      if (chat && !chat.messages.some(message => message.messageId === record.chat!.assistantMessageId)) {
        chat.messages.push({ messageId: record.chat.assistantMessageId, runId: record.run.runId, role: 'assistant', text: record.run.result.text,
          createdAt: record.run.updatedAt, attachments: [] });
        chat.state = 'idle'; chat.activeRunId = undefined; await this.saveChat(chat);
      }
    }
  }
  private async admit(caller: LocalAppAgentOwner, value: unknown, chat?: ChatRecord): Promise<{ runId: string }> {
    const principal = owner(caller), input = parseAgentRunInput(value);
    const fingerprint = agentDigest(chat ? { input, chatId: chat.chatId } : { input });
    const epoch = this.epochs.get(principal.appId) ?? 0;
    await this.initialize();
    return this.exclusive(async () => {
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      if (chat && this.getChatRecord(principal, chat.chatId) !== chat) throw new LocalAppAgentError('not_found', 'Agent chat was not found.');
      const authorization = structuredClone(await this.options.authorize(caller));
      validateAgentSettings(authorization.settings);
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      if ((this.epochs.get(principal.appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while starting the task.');
      const key = requestKey(principal, input.requestId), previousId = this.requests.get(key);
      if (previousId) {
        const previous = this.lookup(principal, previousId);
        if (previous.fingerprint !== fingerprint) throw new LocalAppAgentError('request_conflict', 'This request identifier was already used with different input.');
        return { runId: previousId };
      }
      if ([...this.active.values()].some(active => active.record.owner.appId === principal.appId)) throw new LocalAppAgentError('busy', 'This application already has an Agent task in progress.');
      if (chat && (chat.archived || chat.activeRunId)) throw new LocalAppAgentError(chat.archived ? 'chat_archived' : 'chat_busy', chat.archived ? 'This Agent chat is archived.' : 'This Agent chat already has a turn in progress.');
      const model = authorization.model, settings = authorization.settings;
      if (!model.canSelect || model.modelId !== settings.model.modelId || model.providerId !== settings.model.providerId) throw new LocalAppAgentError('model_unavailable', 'The selected Agent model is not configured.');
      if (input.attachments?.length && !model.supportsAttachments) throw new LocalAppAgentError('attachments_unsupported', 'The selected model does not support attachments.');
      if (input.tools?.length && !model.supportsStructuredOutput) throw new LocalAppAgentError('tools_unsupported', 'The selected model does not support tool calls.');
      if (input.outputSchema && !model.supportsStructuredOutput) throw new LocalAppAgentError('structured_output_unsupported', 'The selected model does not support structured results.');
      const toolIds = input.tools ?? [];
      for (const id of toolIds) if (!authorization.requestedTools.includes(id) || !settings.tools.some(tool => tool.id === id)) throw new LocalAppAgentError('tool_not_granted', 'A requested tool is not declared and granted for this application.');
      const skillIds = input.skills ?? [];
      for (const id of skillIds) if (!authorization.requestedSkills.includes(id) || !settings.skills.includes(id)) throw new LocalAppAgentError('skill_not_granted', 'A requested Skill is not declared and granted for this application.');
      if ((input.maxTurns ?? settings.maxTurns) > settings.maxTurns || (input.timeoutMs ?? settings.timeoutMs) > settings.timeoutMs) throw new LocalAppAgentError('limit_exceeded', 'Applications can only lower the host task limits.');
      authorization.settings.tools = settings.tools.filter(tool => toolIds.includes(tool.id));
      authorization.settings.skills = settings.skills.filter(skill => skillIds.includes(skill));
      authorization.settings.maxTurns = input.maxTurns ?? settings.maxTurns;
      authorization.settings.timeoutMs = input.timeoutMs ?? settings.timeoutMs;
      if (!(await this.options.revalidate(caller, authorization)) || (this.epochs.get(principal.appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while starting the task.');
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      const runId = this.uuid(), time = this.now();
      if (!/^[a-zA-Z0-9_-]{1,128}$/.test(runId) || this.records.has(runId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate a task identifier.');
      const chatMeta = chat ? { chatId: chat.chatId, userMessageId: this.uuid(), assistantMessageId: this.uuid() } : undefined;
      const run: LocalAppAgentRun = { runId, requestId: input.requestId, state: 'queued', sequence: 1, createdAt: time, updatedAt: time,
        model: { providerId: settings.model.providerId, modelId: settings.model.modelId }, limits: { maxTurns: authorization.settings.maxTurns, timeoutMs: authorization.settings.timeoutMs } };
      const record: LedgerRecord = { schema: 2, owner: principal, fingerprint, run, ...(chatMeta ? { chat: chatMeta } : {}) };
      const taskPath = this.taskPath(principal, runId);
      await this.safeDirectory(join(this.appRoot(principal.appId), 'tasks'));
      if (chat && chatMeta) {
        const previous = structuredClone(chat);
        chat.messages.push({ messageId: chatMeta.userMessageId, runId, role: 'user', text: input.prompt, createdAt: time,
          attachments: (input.attachments ?? []).map(({ name, mediaType, base64 }) => ({ name, mediaType, size: Buffer.from(base64, 'base64').length })) });
        chat.state = 'running'; chat.activeRunId = runId;
        try { await this.saveChat(chat); await atomicJson(taskPath, record); }
        catch (error) { this.chats.set(this.chatKey(chat.appId, chat.chatId), previous); await atomicJson(this.chatPath(previous, previous.chatId), previous).catch(() => undefined); throw error; }
      } else await atomicJson(taskPath, record);
      const active: ActiveRun = { record, authorization, controller: new AbortController(), appEpoch: epoch,
        ...(chat && chatMeta ? { chat: { chatId: chat.chatId, sessionId: chat.sessionId, sessionDirectory: this.chatDirectory(principal, chat.chatId) } } : {}) };
      this.records.set(runId, record); this.requests.set(key, runId); this.active.set(runId, active);
      active.timer = setTimeout(() => { void this.terminate(runId, 'failed', { code: 'timeout', message: 'The Agent task exceeded its time limit.' }).catch(() => undefined); }, record.run.limits.timeoutMs);
      active.timer.unref?.(); this.broadcast(this.stateEvent(record));
      active.settled = Promise.resolve().then(() => this.execute(active, caller, input)).finally(() => this.active.delete(runId));
      return { runId };
    });
  }
  async run(caller: LocalAppAgentOwner, value: unknown): Promise<{ runId: string }> { return this.admit(caller, value); }
  async sendChat(caller: LocalAppAgentOwner, chatId: string, value: unknown): Promise<{ runId: string }> {
    const principal = owner(caller); await this.initialize();
    const chat = this.getChatRecord(principal, chatId);
    return this.admit(principal, value, chat);
  }
  private async execute(active: ActiveRun, caller: LocalAppAgentOwner, input: LocalAppAgentRunInput): Promise<void> {
    const { record, controller } = active;
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
        await this.commit(record, { ...record.run, state: 'running', sequence: record.run.sequence + 1, updatedAt: this.now() });
        this.broadcast(this.stateEvent(record));
      });
      await assertAuthorized();
      const completion = await this.options.execute({ runId: record.run.runId, owner: structuredClone(caller), input: structuredClone(input),
        ...(active.chat ? { chat: structuredClone(active.chat) } : {}), authorization: structuredClone(active.authorization), signal: controller.signal, assertAuthorized,
        emit: async progress => {
          await assertAuthorized();
          if (!PHASES.has(progress.phase)) throw new LocalAppAgentError('invalid_event', 'The Agent returned an invalid progress event.');
          active.lastUsage = safeUsage(progress.usage) ?? active.lastUsage;
          await this.exclusive(async () => {
            if (TERMINAL.has(record.run.state)) return;
            const update: LocalAppAgentRun = { ...record.run, sequence: record.run.sequence + 1, updatedAt: this.now() };
            if (progress.phase === 'waiting_for_permission' || progress.phase === 'waiting_for_answer') update.phase = progress.phase;
            else update.phase = undefined;
            if (progress.completedModelTurns !== undefined && Number.isSafeInteger(progress.completedModelTurns) && progress.completedModelTurns >= 0) {
              update.progress = { completedModelTurns: progress.completedModelTurns, turnLimit: record.run.limits.maxTurns };
            }
            if (active.lastUsage) update.usage = active.lastUsage;
            await this.commit(record, update); this.broadcast(this.stateEvent(record));
          });
        } });
      await assertAuthorized();
      if (!completion.result || typeof completion.result.text !== 'string' || !Array.isArray(completion.result.attachments)
        || completion.result.attachments.length !== 0 || Buffer.byteLength(JSON.stringify(completion.result)) > 4 * 1024 * 1024) {
        throw new LocalAppAgentError('invalid_result', 'The Agent result is invalid or too large.');
      }
      if (input.outputSchema && completion.result.output === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent did not produce a validated structured result.');
      const finalUsage = safeUsage(completion.usage) ?? active.lastUsage;
      if (finalUsage === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent completed without reporting usage data.');
      const chatMeta = record.chat;
      const result: LocalAppAgentResult = { text: completion.result.text, attachments: [],
        ...(completion.result.output === undefined ? {} : { output: structuredClone(completion.result.output) }),
        ...(chatMeta ? { chatId: chatMeta.chatId, messageId: chatMeta.assistantMessageId } : {}) };
      await this.finish(record.run.runId, 'succeeded', undefined, result, finalUsage);
    } catch (error) {
      const known = error instanceof LocalAppAgentError ? { code: error.code, message: error.message } : { code: 'execution_failed', message: 'The Agent task could not be completed. Check the host configuration and try again.' };
      const state = controller.signal.aborted ? 'cancelled' : ['interrupted', 'executor_closed'].includes(known.code) ? 'interrupted' : 'failed';
      await this.finish(record.run.runId, state, known, undefined, active.lastUsage).catch(() => undefined);
    }
  }
  private lookup(caller: LocalAppAgentOwner, runId: string): LedgerRecord {
    const record = this.records.get(runId);
    if (!record || record.owner.appId !== caller.appId || (!TERMINAL.has(record.run.state)
      && (record.owner.invocationId !== caller.invocationId || record.owner.releaseDigest !== caller.releaseDigest))) {
      throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    }
    return record;
  }
  async get(caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentRun> {
    owner(caller); await this.initialize(); return this.exclusive(async () => structuredClone(this.lookup(caller, runId).run));
  }
  async list(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown } = {}): Promise<LocalAppAgentRun[]> {
    const principal = owner(caller); await this.initialize();
    const rows = [...this.records.values()].filter(record => record.owner.appId === principal.appId).map(record => record.run).sort((a, b) => b.updatedAt - a.updatedAt || b.createdAt - a.createdAt);
    const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit);
    return structuredClone(rows.slice(offset, offset + limit));
  }
  async subscribe(caller: LocalAppAgentOwner, runId: string, listener: (event: LocalAppAgentEvent) => void, afterSequence = 0): Promise<() => void> {
    owner(caller); await this.initialize();
    return this.exclusive(async () => {
      const record = this.lookup(caller, runId);
      if (!Number.isSafeInteger(afterSequence) || afterSequence < 0) throw new LocalAppAgentError('invalid_input', 'Invalid event sequence.');
      let subscribers = this.listeners.get(runId); if (!subscribers) this.listeners.set(runId, subscribers = new Set());
      subscribers.add(listener);
      if (record.run.sequence > afterSequence) { try { listener(this.stateEvent(record)); } catch { /* a failed renderer cannot affect the task */ } }
      return () => { subscribers!.delete(listener); if (!subscribers!.size) this.listeners.delete(runId); };
    });
  }
  async cancel(caller: LocalAppAgentOwner, runId: string): Promise<void> {
    owner(caller); await this.initialize(); this.lookup(caller, runId);
    await this.terminate(runId, 'cancelled', { code: 'cancelled', message: 'The Agent task was cancelled.' });
  }
  private getChatRecord(caller: Pick<LocalAppAgentOwner, 'appId'>, chatId: string): ChatRecord {
    const chat = this.chats.get(this.chatKey(caller.appId, chatId));
    if (!chat) throw new LocalAppAgentError('not_found', 'Agent chat was not found.');
    return chat;
  }
  private summary(chat: ChatRecord): LocalAppAgentChatSummary {
    return { chatId: chat.chatId, title: chat.title, createdAt: chat.createdAt, updatedAt: chat.updatedAt, archived: chat.archived, state: chat.state };
  }
  async createChat(caller: LocalAppAgentOwner, value: unknown = {}): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller); await this.initialize();
    const input = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
    if (!input || Object.keys(input).some(key => key !== 'title') || (input.title !== undefined && (typeof input.title !== 'string' || input.title.trim().length > 160))) throw new LocalAppAgentError('invalid_input', 'Invalid Agent chat title.');
    const authorization = await this.options.authorize(caller); validateAgentSettings(authorization.settings);
    const chatId = this.uuid(), sessionId = this.uuid(), now = this.now();
    if (!CHAT_ID.test(chatId) || !CHAT_ID.test(sessionId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate an Agent chat identifier.');
    const chat: ChatRecord = { schema: 1, appId: principal.appId, chatId, sessionId,
      title: typeof input.title === 'string' && input.title.trim() ? input.title.trim() : 'New chat', createdAt: now, updatedAt: now, archived: false, state: 'idle', messages: [] };
    await this.safeDirectory(this.chatDirectory(principal, chatId));
    await this.safeDirectory(join(this.chatDirectory(principal, chatId), 'assets'));
    await this.saveChat(chat); return this.summary(chat);
  }
  async getChat(caller: LocalAppAgentOwner, chatId: string): Promise<LocalAppAgentChatSummary> {
    const principal = owner(caller); await this.initialize(); return this.summary(this.getChatRecord(principal, chatId));
  }
  async listChats(caller: LocalAppAgentOwner, options: { cursor?: unknown; limit?: unknown; includeArchived?: unknown } = {}): Promise<LocalAppAgentChatSummary[]> {
    const principal = owner(caller); await this.initialize();
    if (options.includeArchived !== undefined && typeof options.includeArchived !== 'boolean') throw new LocalAppAgentError('invalid_input', 'Invalid archived-chat filter.');
    const rows = [...this.chats.values()].filter(chat => chat.appId === principal.appId && (options.includeArchived || !chat.archived))
      .sort((a, b) => b.updatedAt - a.updatedAt || b.createdAt - a.createdAt);
    const offset = cursorOffset(options.cursor), limit = pageLimit(options.limit);
    return structuredClone(rows.slice(offset, offset + limit).map(chat => this.summary(chat)));
  }
  async archiveChat(caller: LocalAppAgentOwner, chatId: string): Promise<void> {
    const principal = owner(caller); await this.initialize();
    await this.exclusive(async () => {
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be archived.');
      chat.archived = true; await this.saveChat(chat);
    });
  }
  async deleteChat(caller: LocalAppAgentOwner, chatId: string): Promise<void> {
    const principal = owner(caller); await this.initialize();
    await this.exclusive(async () => {
      const chat = this.getChatRecord(principal, chatId);
      if (chat.activeRunId) throw new LocalAppAgentError('chat_busy', 'An active Agent chat cannot be deleted.');
      await fs.rm(this.chatDirectory(principal, chatId), { recursive: true, force: false });
      this.chats.delete(this.chatKey(principal.appId, chatId));
    });
  }
  async chatHistory(caller: LocalAppAgentOwner, chatId: string, options: { cursor?: unknown; limit?: unknown } = {}): Promise<LocalAppAgentChatHistory> {
    const principal = owner(caller); await this.initialize();
    const messages = this.getChatRecord(principal, chatId).messages, offset = cursorOffset(options.cursor), limit = pageLimit(options.limit, 100);
    const rows = structuredClone(messages.slice(offset, offset + limit));
    const next = nextCursor(offset + rows.length, offset + rows.length < messages.length);
    return { messages: rows, ...(next ? { nextCursor: next } : {}) };
  }
  async cancelChat(caller: LocalAppAgentOwner, chatId: string): Promise<void> {
    const principal = owner(caller); await this.initialize();
    const chat = this.getChatRecord(principal, chatId);
    if (chat.activeRunId) await this.cancel(principal, chat.activeRunId);
  }
  async cancelApp(appId: string, reason = 'Application Agent access changed.'): Promise<void> {
    this.epochs.set(appId, (this.epochs.get(appId) ?? 0) + 1); await this.initialize();
    const runs = await this.exclusive(async () => [...this.active.values()].filter(active => active.record.owner.appId === appId));
    await Promise.all(runs.map(active => this.terminate(active.record.run.runId, 'cancelled', { code: 'application_stopped', message: reason.slice(0, 1024) })));
  }
  private async terminate(runId: string, state: 'cancelled' | 'failed' | 'interrupted', error: { code: string; message: string }): Promise<void> {
    const active = this.active.get(runId); active?.controller.abort();
    await this.finish(runId, state, error, undefined, active?.lastUsage);
    await active?.settled;
  }
  private async finish(runId: string, state: 'succeeded' | 'failed' | 'cancelled' | 'interrupted', error?: LocalAppAgentRun['error'], result?: LocalAppAgentResult, usage?: LocalAppAgentUsage): Promise<void> {
    await this.exclusive(async () => {
      const active = this.active.get(runId), record = this.records.get(runId);
      if (!record || TERMINAL.has(record.run.state)) return;
      const finalUsage = safeUsage(usage);
      const next: LocalAppAgentRun = { ...record.run, state, phase: undefined, sequence: record.run.sequence + 1, updatedAt: this.now(),
        ...(error ? { error } : {}), ...(result ? { result } : {}),
        ...(finalUsage !== undefined ? { usage: finalUsage } : {}) };
      await this.commit(record, next);
      if (active?.timer) clearTimeout(active.timer);
      if (record.chat) {
        const chat = this.chats.get(this.chatKey(record.owner.appId, record.chat.chatId));
        if (chat) {
          chat.activeRunId = undefined; chat.state = state === 'succeeded' ? 'idle' : 'interrupted';
          if (state === 'succeeded' && result && !chat.messages.some(message => message.messageId === record.chat!.assistantMessageId)) {
            chat.messages.push({ messageId: record.chat.assistantMessageId, runId, role: 'assistant', text: result.text, createdAt: next.updatedAt, attachments: [] });
          }
          await this.saveChat(chat);
        }
      }
      this.broadcast(this.stateEvent(record));
    });
  }
  private async commit(record: LedgerRecord, run: LocalAppAgentRun): Promise<void> {
    await atomicJson(this.taskPath(record.owner, run.runId), { ...record, run }); record.run = run;
  }
  private stateEvent(record: LedgerRecord): LocalAppAgentEvent { return { type: 'state', runId: record.run.runId, sequence: record.run.sequence, time: record.run.updatedAt, task: structuredClone(record.run) }; }
  private broadcast(event: LocalAppAgentEvent): void {
    for (const listener of this.listeners.get(event.runId) ?? []) { try { listener(structuredClone(event)); } catch { /* renderer delivery is best effort */ } }
  }
  async dispose(): Promise<void> {
    this.stopped = true; await this.initialize();
    const runs = await this.exclusive(async () => [...this.active.keys()]);
    await Promise.all(runs.map(runId => this.terminate(runId, 'interrupted', { code: 'host_stopped', message: 'The Agent host stopped before this task completed.' })));
    this.listeners.clear();
  }
}
