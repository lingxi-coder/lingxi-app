import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  LocalAppAgentEvent, LocalAppAgentModelChoice, LocalAppAgentProgressPhase, LocalAppAgentResult,
  LocalAppAgentRun, LocalAppAgentRunInput, LocalAppAgentSettings, LocalAppAgentUsage,
} from '../../shared/localAppAgent.js';
import { atomicJson } from './store.js';
import { agentDigest, LocalAppAgentError, parseAgentRunInput, validateAgentSettings } from './agentPolicy.js';
export { LocalAppAgentError } from './agentPolicy.js';

/** Constructed by the trusted main-frame host, never accepted from application arguments. */
export interface LocalAppAgentOwner { appId: string; revision: number; invocationId: string; webContentsId?: number }
export interface LocalAppAgentAuthorization {
  settings: LocalAppAgentSettings;
  requestedTools: string[];
  model: LocalAppAgentModelChoice;
}
export interface LocalAppAgentExecutorResult { result: LocalAppAgentResult; usage?: LocalAppAgentUsage }
export interface LocalAppAgentExecution {
  runId: string;
  owner: LocalAppAgentOwner;
  input: LocalAppAgentRunInput;
  /** Contains the fixed, selected tool grant subset and effective limits. */
  authorization: LocalAppAgentAuthorization;
  signal: AbortSignal;
  emit(progress: { phase: LocalAppAgentProgressPhase; text?: string }): Promise<void>;
  /** Must also be called after trusted approval, before any new external request. */
  assertAuthorized(): Promise<void>;
}
export interface LocalAppAgentBrokerOptions {
  root: string;
  authorize(owner: LocalAppAgentOwner): Promise<LocalAppAgentAuthorization>;
  revalidate(owner: LocalAppAgentOwner, authorization: LocalAppAgentAuthorization): Promise<boolean>;
  execute(execution: LocalAppAgentExecution): Promise<LocalAppAgentExecutorResult>;
  now?: () => number;
  uuid?: () => string;
}
interface LedgerRecord {
  schema: 1;
  owner: LocalAppAgentOwner;
  fingerprint: string;
  run: LocalAppAgentRun;
}
interface ActiveRun {
  record: LedgerRecord;
  authorization: LocalAppAgentAuthorization;
  controller: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  appEpoch: number;
  settled?: Promise<void>;
}
const TERMINAL = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
const PHASES = new Set<LocalAppAgentProgressPhase>(['starting', 'thinking', 'tool', 'waiting_for_permission', 'waiting_for_answer']);
function owner(value: LocalAppAgentOwner): LocalAppAgentOwner {
  if (!value || !/^a[a-f0-9]{20}$/.test(value.appId) || !Number.isSafeInteger(value.revision) || value.revision < 1
    || typeof value.invocationId !== 'string' || !value.invocationId || value.invocationId.length > 256) throw new LocalAppAgentError('invalid_owner', 'Invalid application execution identity.');
  // The persisted principal does not depend on a transient Electron window ID.
  return { appId: value.appId, revision: value.revision, invocationId: value.invocationId };
}
function safeUsage(usage: LocalAppAgentUsage | undefined): LocalAppAgentUsage | undefined {
  if (!usage) return undefined;
  const result: LocalAppAgentUsage = {};
  for (const key of ['inputTokens', 'outputTokens', 'costUsd', 'turns'] as const) {
    const value = usage[key];
    if (value !== undefined && Number.isFinite(value) && value >= 0) result[key] = value;
  }
  return result;
}
function requestKey(value: LocalAppAgentOwner, requestId: string): string { return JSON.stringify([value.appId, value.revision, requestId]); }

/** Durable ownership, idempotency and lifecycle enforcement without Electron dependencies. */
export class LocalAppAgentBroker {
  private ready?: Promise<void>;
  private queue = Promise.resolve();
  private readonly records = new Map<string, LedgerRecord>();
  private readonly requests = new Map<string, string>();
  private readonly active = new Map<string, ActiveRun>();
  private readonly epochs = new Map<string, number>();
  private readonly listeners = new Map<string, Set<(event: LocalAppAgentEvent) => void>>();
  private stopped = false;
  constructor(private readonly options: LocalAppAgentBrokerOptions) {}
  /** Includes a terminal task until its executor has actually closed. */
  hasActiveApp(appId: string): boolean { return [...this.active.values()].some(active => active.record.owner.appId === appId); }
  private now(): number { return this.options.now?.() ?? Date.now(); }
  private exclusive<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.queue.then(operation, operation);
    this.queue = result.then(() => undefined, () => undefined);
    return result;
  }
  initialize(): Promise<void> { return this.ready ??= this.load(); }
  private path(runId: string): string {
    if (!/^[a-zA-Z0-9_-]{1,128}$/.test(runId)) throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    return join(this.options.root, `${runId}.json`);
  }
  private async load(): Promise<void> {
    await fs.mkdir(this.options.root, { recursive: true, mode: 0o700 });
    for (const entry of await fs.readdir(this.options.root, { withFileTypes: true })) {
      if (!entry.name.endsWith('.json')) continue;
      if (!entry.isFile() || entry.isSymbolicLink()) throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger is invalid.');
      const record = JSON.parse(await fs.readFile(join(this.options.root, entry.name), 'utf8')) as LedgerRecord;
      owner(record.owner);
      if (record.schema !== 1 || `${record.run?.runId}.json` !== entry.name || typeof record.fingerprint !== 'string'
        || !['queued', 'running', ...TERMINAL].includes(record.run.state) || !Number.isSafeInteger(record.run.sequence)
        || typeof record.run.requestId !== 'string') throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger is invalid.');
      this.path(record.run.runId);
      if (!TERMINAL.has(record.run.state)) {
        record.run.state = 'interrupted'; record.run.updatedAt = this.now(); record.run.sequence++;
        record.run.error = { code: 'host_restarted', message: 'The host stopped before this task completed. The task was not replayed.' };
        await atomicJson(this.path(record.run.runId), record);
      }
      const key = requestKey(record.owner, record.run.requestId);
      if (this.requests.has(key)) throw new LocalAppAgentError('ledger_invalid', 'The Agent task ledger contains conflicting requests.');
      this.records.set(record.run.runId, record); this.requests.set(key, record.run.runId);
    }
  }
  async run(caller: LocalAppAgentOwner, value: unknown): Promise<{ runId: string }> {
    const principal = owner(caller), input = parseAgentRunInput(value), fingerprint = agentDigest(input);
    const epoch = this.epochs.get(principal.appId) ?? 0;
    await this.initialize();
    return this.exclusive(async () => {
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
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
      const model = authorization.model, settings = authorization.settings;
      if (!model.canSelect || model.modelId !== settings.model.modelId || model.providerId !== settings.model.providerId) throw new LocalAppAgentError('model_unavailable', 'The selected Agent model is not configured.');
      if (input.images?.length && !model.supportsImages) throw new LocalAppAgentError('images_unsupported', 'The selected model does not support images.');
      // Structured results use a Harness result tool in this release, so the
      // catalog derives this flag from the same underlying tool capability.
      if (input.tools?.length && !model.supportsStructuredOutput) throw new LocalAppAgentError('tools_unsupported', 'The selected model does not support tool calls.');
      if (input.outputSchema && !model.supportsStructuredOutput) throw new LocalAppAgentError('structured_output_unsupported', 'The selected model does not support structured results.');
      const toolIds = input.tools ?? [];
      for (const id of toolIds) if (!authorization.requestedTools.includes(id) || !settings.tools.some(tool => tool.id === id)) throw new LocalAppAgentError('tool_not_granted', 'A requested tool is not declared and granted for this application.');
      if ((input.maxTurns ?? settings.maxTurns) > settings.maxTurns || (input.timeoutMs ?? settings.timeoutMs) > settings.timeoutMs) throw new LocalAppAgentError('limit_exceeded', 'Applications can only lower the host task limits.');
      authorization.settings.tools = settings.tools.filter(tool => toolIds.includes(tool.id));
      authorization.settings.maxTurns = input.maxTurns ?? settings.maxTurns;
      authorization.settings.timeoutMs = input.timeoutMs ?? settings.timeoutMs;
      if (!(await this.options.revalidate(caller, authorization)) || (this.epochs.get(principal.appId) ?? 0) !== epoch) throw new LocalAppAgentError('permission_revoked', 'Application access changed while starting the task.');
      if (this.stopped) throw new LocalAppAgentError('host_stopped', 'The Agent host is stopping.');
      const runId = this.options.uuid?.() ?? randomUUID(), time = this.now();
      if (this.records.has(runId)) throw new LocalAppAgentError('id_conflict', 'Could not allocate a task identifier.');
      const record: LedgerRecord = { schema: 1, owner: principal, fingerprint, run: { runId, requestId: input.requestId, state: 'queued', sequence: 1,
        createdAt: time, updatedAt: time, model: { ...settings.model }, limits: { maxTurns: authorization.settings.maxTurns, timeoutMs: authorization.settings.timeoutMs } } };
      // The committed intent is the point of no replay. Never persist credentials, image bytes or application prompts.
      await atomicJson(this.path(runId), record);
      const active: ActiveRun = { record, authorization, controller: new AbortController(), appEpoch: epoch };
      this.records.set(runId, record); this.requests.set(key, runId); this.active.set(runId, active);
      active.timer = setTimeout(() => { void this.terminate(runId, 'failed', { code: 'timeout', message: 'The Agent task exceeded its time limit.' }).catch(() => undefined); }, record.run.limits.timeoutMs);
      active.timer.unref?.();
      this.broadcast(this.stateEvent(record));
      // Do not await execution from inside the serialized ledger operation.
      active.settled = Promise.resolve().then(() => this.execute(active, caller, input)).finally(() => {
        this.active.delete(runId);
      });
      return { runId };
    });
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
        authorization: structuredClone(active.authorization), signal: controller.signal, assertAuthorized,
        emit: async progress => {
          await assertAuthorized();
          if (!PHASES.has(progress.phase) || (progress.text !== undefined && typeof progress.text !== 'string')) throw new LocalAppAgentError('invalid_event', 'The Agent returned an invalid progress event.');
          await this.exclusive(async () => {
            if (TERMINAL.has(record.run.state)) return;
            // Persist sequence before publication so reconnect never goes backwards.
            await this.commit(record, { ...record.run, sequence: record.run.sequence + 1, updatedAt: this.now() });
            this.broadcast({ type: 'progress', runId: record.run.runId, sequence: record.run.sequence, time: record.run.updatedAt,
              phase: progress.phase, ...(progress.text ? { text: progress.text.slice(0, 4096) } : {}) });
          });
        } });
      await assertAuthorized();
      if (!completion.result || (completion.result.text !== undefined && typeof completion.result.text !== 'string')
        || Buffer.byteLength(JSON.stringify(completion.result)) > 4 * 1024 * 1024) throw new LocalAppAgentError('invalid_result', 'The Agent result is invalid or too large.');
      if (input.outputSchema && completion.result.output === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent did not produce a validated structured result.');
      const result: LocalAppAgentResult = {
        ...(completion.result.text === undefined ? {} : { text: completion.result.text }),
        ...(completion.result.output === undefined ? {} : { output: structuredClone(completion.result.output) }),
      };
      await this.finish(record.run.runId, 'succeeded', undefined, result, safeUsage(completion.usage));
    } catch (error) {
      const known = error instanceof LocalAppAgentError ? { code: error.code, message: error.message } : { code: 'execution_failed', message: 'The Agent task could not be completed. Check the host configuration and try again.' };
      const state = controller.signal.aborted ? 'cancelled' : ['interrupted', 'executor_closed'].includes(known.code) ? 'interrupted' : 'failed';
      await this.finish(record.run.runId, state, known).catch(() => undefined);
    }
  }
  private lookup(caller: LocalAppAgentOwner, runId: string): LedgerRecord {
    const record = this.records.get(runId);
    if (!record || record.owner.appId !== caller.appId || record.owner.revision !== caller.revision
      || (!TERMINAL.has(record.run.state) && record.owner.invocationId !== caller.invocationId)) throw new LocalAppAgentError('not_found', 'Agent task was not found.');
    return record;
  }
  async get(caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentRun> {
    owner(caller); await this.initialize();
    return this.exclusive(async () => structuredClone(this.lookup(caller, runId).run));
  }
  async subscribe(caller: LocalAppAgentOwner, runId: string, listener: (event: LocalAppAgentEvent) => void, afterSequence = 0): Promise<() => void> {
    owner(caller); await this.initialize();
    return this.exclusive(async () => {
      const record = this.lookup(caller, runId);
      if (!Number.isSafeInteger(afterSequence) || afterSequence < 0) throw new LocalAppAgentError('invalid_input', 'Invalid event sequence.');
      let subscribers = this.listeners.get(runId);
      if (!subscribers) this.listeners.set(runId, subscribers = new Set());
      subscribers.add(listener);
      // Reconnect gets a complete current snapshot, not an unbounded text replay.
      if (record.run.sequence >= afterSequence) { try { listener(this.stateEvent(record)); } catch { /* a failed renderer cannot affect the task */ } }
      return () => { subscribers!.delete(listener); if (!subscribers!.size) this.listeners.delete(runId); };
    });
  }
  async cancel(caller: LocalAppAgentOwner, runId: string): Promise<void> {
    owner(caller); await this.initialize(); this.lookup(caller, runId);
    await this.terminate(runId, 'cancelled', { code: 'cancelled', message: 'The Agent task was cancelled.' });
  }
  async cancelApp(appId: string, reason = 'Application Agent access changed.'): Promise<void> {
    // Invalidate synchronously, including an authorization currently in flight.
    this.epochs.set(appId, (this.epochs.get(appId) ?? 0) + 1);
    await this.initialize();
    // Drain admissions that may already be committing their intent. Otherwise
    // a lifecycle stop could return before that child is entered in active.
    const runs = await this.exclusive(async () => [...this.active.values()].filter(active => active.record.owner.appId === appId));
    await Promise.all(runs.map(active => this.terminate(active.record.run.runId, 'cancelled', { code: 'application_stopped', message: reason.slice(0, 1024) })));
  }
  private async terminate(runId: string, state: 'cancelled' | 'failed' | 'interrupted', error: { code: string; message: string }): Promise<void> {
    const active = this.active.get(runId);
    active?.controller.abort();
    await this.finish(runId, state, error);
    // The executor must settle after its child has closed. A terminal record
    // alone must not permit workspace deletion or a second concurrent child.
    await active?.settled;
  }
  private async finish(runId: string, state: 'succeeded' | 'failed' | 'cancelled' | 'interrupted', error?: LocalAppAgentRun['error'], result?: LocalAppAgentResult, usage?: LocalAppAgentUsage): Promise<void> {
    await this.exclusive(async () => {
      const active = this.active.get(runId), record = this.records.get(runId);
      if (!record || TERMINAL.has(record.run.state)) return;
      await this.commit(record, { ...record.run, state, sequence: record.run.sequence + 1, updatedAt: this.now(), ...(error ? { error } : {}), ...(result ? { result } : {}), ...(usage ? { usage } : {}) });
      if (active?.timer) clearTimeout(active.timer);
      this.broadcast(this.stateEvent(record));
    });
  }
  private async commit(record: LedgerRecord, run: LocalAppAgentRun): Promise<void> {
    await atomicJson(this.path(run.runId), { ...record, run });
    record.run = run;
  }
  private stateEvent(record: LedgerRecord): LocalAppAgentEvent { return { type: 'state', runId: record.run.runId, sequence: record.run.sequence, time: record.run.updatedAt, run: structuredClone(record.run) }; }
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
