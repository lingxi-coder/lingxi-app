import { CancellationTombstones, createInvocationRef, type InvocationRef } from './upstream/dyadInvocation.js';
/**
 * App-owned runtime. Adapted from Dyad's transport-neutral AppRuntimeService:
 * serialized operations, per-invocation output ownership, cancellation before
 * queue admission, and cleanup on failed readiness. Pinokio's shell steps
 * supply cwd/env/venv and independently-lived services. See REFERENCES.md.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { join, dirname, delimiter } from 'node:path';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:net';
import type { Duplex } from 'node:stream';
import { setTimeout as delay } from 'node:timers/promises';
import { resolveServerBin } from '../bridgeDiscovery.js';
import type { DesktopLocalApp, DesktopAppManifest, LocalAppLog, LocalAppPhase, LocalAppStep, LocalAppProcess } from '../../shared/localApps.js';
import { CoreLocalAppStore } from './coreStore.js';
import { guardedProcessArguments } from './processGuardian.js';
import { LocalAppProcessClient, resolveWindowsProgram, windowsProcessEnvironment } from './processClient.js';
import { LocalAppStore, atomicJson, containedFile } from './store.js';

type RuntimeChild = ChildProcess | LocalAppProcessClient;
interface Invocation {
  id: string; operationId: string; revision: number; releaseDigest?: string; controller: AbortController; phase: LocalAppPhase;
  children: Set<RuntimeChild>; services: Map<string, { port: number; url: string }>;
  manifest: DesktopAppManifest;
  readiness: Map<string, { needle: string; matched: boolean; stdout: string; stderr: string }>;
  workspace: string; preparationDataPath?: string; error?: string; retired?: Invocation[];
}
interface PersistentLogSummary { droppedEntries: number; droppedBytes: number; unconfirmedEntries: number; unconfirmedBytes: number; unconfirmedNotices: number }
interface PersistentLogQueue extends PersistentLogSummary {
  pending: Array<{ payload: string; bytes: number; summary?: PersistentLogSummary }>;
  bytes: number;
  active: boolean;
  tracked: boolean;
  failures: number;
  blockedUntil: number;
  retry?: ReturnType<typeof setTimeout>;
  totalDroppedEntries: number;
  totalDroppedBytes: number;
  error?: string;
}
export interface RuntimeOptions { onRunEnded?: (appId: string, runId: string, reason: string) => Promise<void> | void; nodeExecutable?: string; bunExecutable?: string; shell?: string; environment?: NodeJS.ProcessEnv; processHelperExecutable?: string; processHelperArguments?: string[]; resolveWindowsExecutable?: typeof resolveWindowsProgram }

export function orderedProcesses(processes: LocalAppProcess[]): LocalAppProcess[] {
  const remaining = [...processes], sorted: LocalAppProcess[] = [];
  while (remaining.length) {
    const next = remaining.findIndex(spec => (spec.dependsOn ?? []).every(dep => sorted.some(p => p.name === dep)));
    if (next < 0) throw new Error('Unknown or cyclic application process dependency');
    sorted.push(remaining.splice(next, 1)[0]!);
  }
  return sorted;
}

export function shellQuote(value: string): string { return `'${value.replaceAll("'", "'\\''")}'`; }
export function appEnvironment(source: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'LANG', 'LC_ALL', 'TMPDIR', 'TMP', 'TEMP', 'TZ', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA']) {
    if (source[key] !== undefined) env[key] = source[key];
  }
  return env;
}
async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer(); server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') { server.close(); reject(new Error('No local port')); return; }
      server.close(error => error ? reject(error) : resolve(address.port));
    });
  });
}
const stoppingChildren = new WeakMap<RuntimeChild, Promise<void>>();
const childClosures = new WeakMap<ChildProcess, Promise<{ code: number | null; signal: NodeJS.Signals | null }>>();
const STOP_CALLBACK_TIMEOUT_MS = 3000;
const PERSISTENT_LOG_RECORD_LIMIT = 64;
const PERSISTENT_LOG_BYTE_LIMIT = 256 * 1024;
const PERSISTENT_LOG_RUN_LIMIT = 16;
const PERSISTENT_LOG_FAILURE_LIMIT = 3;
function stopTree(child: RuntimeChild): Promise<void> {
  let pending = stoppingChildren.get(child);
  if (!pending) {
    pending = terminateTree(child).catch(error => { stoppingChildren.delete(child); throw error; });
    stoppingChildren.set(child, pending);
  }
  return pending;
}
async function terminateTree(child: RuntimeChild): Promise<void> {
  if (child instanceof LocalAppProcessClient) return child.stop();
  if (!child.pid) return;
  const pid = child.pid;
  if (process.platform === 'win32') {
    // taskkill resolves a bare PID after the leader can have exited and that
    // PID can have been reused. The ChildProcess handle identifies the leader,
    // but it cannot establish that its descendants have exited. Until this
    // runner owns a Windows Job, never report this tree as verified.
    if (child.exitCode === null && child.signalCode === null) {
      try {
        if (!child.kill('SIGKILL')) throw new Error('leader termination was not accepted');
      } catch (error) {
        throw new Error(`Application leader termination failed; Windows process-tree cleanup is unverified: ${String(error)}`);
      }
    }
    throw new Error('Windows process-tree cleanup is unverified; an owned Job is required');
  }
  const pipe = child.stdio[3] as Duplex | null;
  const closed = childClosures.get(child);
  if (!pipe || !closed) throw new Error('Application process guardian is unavailable; cleanup is unverified');
  // The guardian remains inside the owned group after its leader exits. EOF
  // asks that guardian to signal its own group; the host never signals a
  // possibly recycled process-group id.
  if (!pipe.destroyed && !pipe.writableEnded) pipe.end();
  const drained = await new Promise<boolean>(resolve => {
    const timer = setTimeout(() => resolve(false), 4000);
    void closed.then(() => { clearTimeout(timer); resolve(true); });
  });
  if (!drained) throw new Error('Application process output or guardian did not close; cleanup is unverified');
  // A probe cannot signal an unrelated reused group. EPERM means the group may
  // still exist and must not be reported as successful cleanup.
  const exists = (): boolean => { try { process.kill(-pid, 0); return true; } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ESRCH') return false;
    if (code === 'EPERM') return true;
    throw error;
  } };
  for (let n = 0; n < 60; n++) { if (!exists()) return; await delay(50); }
  throw new Error(`Application process group ${pid} did not stop`);
}

export class LocalAppRuntime {
  private readonly preparing = new Map<string, Invocation>();
  private readonly invocations = new Map<string, Invocation>();
  private readonly queues = new Map<string, Promise<unknown>>();
  private readonly output = new Map<string, LocalAppLog[]>();
  private readonly sequence = new Map<string, number>();
  private readonly deleting = new Set<string>();
  private disposed = false;
  private readonly starts = new Map<string, Promise<void>>();
  private readonly requests = new Map<string, Set<InvocationRef>>();
  private readonly stopNotifications = new WeakMap<Invocation, Promise<void>>();
  private readonly failedNotifications = new WeakMap<Invocation, Promise<void>>();
  private readonly failedNotificationStarted = new WeakSet<Invocation>();
  private readonly persistentLogs = new WeakMap<Invocation, PersistentLogQueue>();
  private readonly retainedPersistentLogs = new Set<PersistentLogQueue>();
  private readonly cancellations = new CancellationTombstones(1000);
  constructor(readonly store: LocalAppStore, private readonly changed: () => void, private readonly options: RuntimeOptions = {}) {}
  status(row: DesktopLocalApp): DesktopLocalApp {
    const run = this.preparing.get(row.id) ?? this.invocations.get(row.id);
    return { ...row, phase: run?.phase ?? 'stopped', runningRevision: run?.revision, runId: run?.id, error: run?.error };
  }
  hasRunning(): boolean { return [...this.invocations.values()].some(run => ['running', 'installing', 'building', 'starting'].includes(run.phase)); }
  logs(id: string): LocalAppLog[] { return [...(this.output.get(id) ?? [])]; }
  service(id: string, name: string): { port: number; url: string } | undefined { return this.invocations.get(id)?.services.get(name); }
  workspace(id: string): string | undefined { return this.invocations.get(id)?.workspace; }
  revision(id: string): number | undefined { return this.invocations.get(id)?.revision; }
  invocationId(id: string): string | undefined { return this.invocations.get(id)?.id; }
  owner(id: string): { appId: string; runId: string; revision: number; releaseDigest: string } | undefined {
    const run = this.invocations.get(id);
    return run?.phase === 'running' && run.releaseDigest ? { appId: id, runId: run.id, revision: run.revision, releaseDigest: run.releaseDigest } : undefined;
  }
  phase(id: string): LocalAppPhase { return this.invocations.get(id)?.phase ?? 'stopped'; }
  manifest(id: string): DesktopAppManifest | undefined { return this.invocations.get(id)?.manifest; }
  private log(id: string, run: Invocation, process: string, text: string, stream: LocalAppLog['stream'] = 'host'): void {
    if (this.invocations.get(id) !== run && this.preparing.get(id) !== run) return;
    const persistent = this.store instanceof CoreLocalAppStore && run.id !== run.operationId ? this.preparePersistentLogs(run) : undefined;
    if (persistent?.tracked) this.queueDiscardNotice(id, run, persistent);
    const readiness = run.readiness.get(process);
    if (readiness && !readiness.matched && stream !== 'host') {
      const combined = readiness[stream] + text;
      readiness.matched = combined.includes(readiness.needle);
      readiness[stream] = readiness.needle.length > 1 ? combined.slice(-(readiness.needle.length - 1)) : '';
    }
    const sequence = (this.sequence.get(id) ?? 0) + 1; this.sequence.set(id, sequence);
    const lines = this.output.get(id) ?? [];
    lines.push({ sequence, time: Date.now(), runId: run.id, process, stream, text: text.slice(-8192) });
    if (lines.length > 500) lines.splice(0, lines.length - 500);
    this.output.set(id, lines);
    if (persistent) this.enqueuePersistentLog(id, run, persistent, JSON.stringify(lines.at(-1)));
    this.changed();
  }
  private displayPersistenceNotice(id: string, run: Invocation, text: string): void {
    const visible = this.preparing.get(id) ?? this.invocations.get(id);
    if (!visible) return;
    const sequence = (this.sequence.get(id) ?? 0) + 1; this.sequence.set(id, sequence);
    const lines = this.output.get(id) ?? [];
    const message = visible === run ? text : `Previous run ${run.id}: ${text}`;
    lines.push({ sequence, time: Date.now(), runId: visible.id, process: 'host', stream: 'host', text: message.slice(-8192) });
    if (lines.length > 500) lines.splice(0, lines.length - 500);
    this.output.set(id, lines); this.changed();
  }
  private preparePersistentLogs(run: Invocation): PersistentLogQueue {
    let queue = this.persistentLogs.get(run);
    if (!queue) {
      queue = { pending: [], bytes: 0, active: false, tracked: false, failures: 0, blockedUntil: 0,
        droppedEntries: 0, droppedBytes: 0, unconfirmedEntries: 0, unconfirmedBytes: 0, unconfirmedNotices: 0,
        totalDroppedEntries: 0, totalDroppedBytes: 0 };
      this.persistentLogs.set(run, queue);
    }
    if (!queue.tracked && Date.now() >= queue.blockedUntil && this.retainedPersistentLogs.size < PERSISTENT_LOG_RUN_LIMIT) {
      queue.tracked = true; queue.failures = 0; this.retainedPersistentLogs.add(queue);
    }
    return queue;
  }
  private enqueuePersistentLog(id: string, run: Invocation, queue: PersistentLogQueue, payload: string): void {
    const bytes = Buffer.byteLength(payload);
    if (!queue.tracked || queue.droppedEntries || queue.unconfirmedEntries || queue.unconfirmedNotices || queue.pending.length >= PERSISTENT_LOG_RECORD_LIMIT || queue.bytes + bytes > PERSISTENT_LOG_BYTE_LIMIT) {
      queue.droppedEntries++; queue.droppedBytes += bytes;
      queue.totalDroppedEntries++; queue.totalDroppedBytes += bytes;
      if (queue.droppedEntries === 1 || queue.droppedEntries % 128 === 0) {
        this.displayPersistenceNotice(id, run, `Persistent log queue discarded ${queue.droppedEntries} entries (${queue.droppedBytes} serialized bytes) while storage was unavailable.`);
      }
    } else {
      queue.pending.push({ payload, bytes }); queue.bytes += bytes;
    }
    this.pumpPersistentLogs(id, run, queue);
  }
  private queueDiscardNotice(id: string, run: Invocation, queue: PersistentLogQueue): void {
    if (!queue.tracked || !(queue.droppedEntries || queue.unconfirmedEntries || queue.unconfirmedNotices)) return;
    const summary: PersistentLogSummary = { droppedEntries: queue.droppedEntries, droppedBytes: queue.droppedBytes,
      unconfirmedEntries: queue.unconfirmedEntries, unconfirmedBytes: queue.unconfirmedBytes, unconfirmedNotices: queue.unconfirmedNotices };
    const text = `Persistent log queue for run ${run.id}: discarded ${summary.droppedEntries} entries (${summary.droppedBytes} serialized bytes); ${summary.unconfirmedEntries} entries (${summary.unconfirmedBytes} serialized bytes) are unconfirmed and may already have been written.${summary.unconfirmedNotices ? ' A previous status notice may also have been written.' : ''}`;
    const sequence = (this.sequence.get(id) ?? 0) + 1;
    const line: LocalAppLog = { sequence, time: Date.now(), runId: run.id, process: 'host', stream: 'host', text };
    const payload = JSON.stringify(line), bytes = Buffer.byteLength(payload);
    if (queue.pending.length >= PERSISTENT_LOG_RECORD_LIMIT || queue.bytes + bytes > PERSISTENT_LOG_BYTE_LIMIT) return;
    queue.droppedEntries = 0; queue.droppedBytes = 0; queue.unconfirmedEntries = 0; queue.unconfirmedBytes = 0; queue.unconfirmedNotices = 0;
    queue.pending.push({ payload, bytes, summary }); queue.bytes += bytes;
    this.sequence.set(id, sequence);
    if (this.invocations.get(id) === run || this.preparing.get(id) === run) {
      const lines = this.output.get(id) ?? [];
      lines.push(line); if (lines.length > 500) lines.splice(0, lines.length - 500);
      this.output.set(id, lines); this.changed();
    }
  }
  private pumpPersistentLogs(id: string, run: Invocation, queue: PersistentLogQueue): void {
    if (!queue.tracked || queue.active || queue.retry || !(this.store instanceof CoreLocalAppStore)) return;
    if (!queue.pending.length) {
      this.queueDiscardNotice(id, run, queue);
      if (!queue.pending.length) { queue.tracked = false; this.retainedPersistentLogs.delete(queue); return; }
    }
    queue.active = true;
    const item = queue.pending[0]!;
    void this.store.appendLog(id, run.id, item.payload).then(() => {
      const previousError = queue.error; queue.error = undefined;
      if (previousError && run.error === previousError) run.error = undefined;
      if (previousError) this.displayPersistenceNotice(id, run, 'Persistent log storage recovered.');
      queue.pending.shift(); queue.bytes -= item.bytes; queue.active = false; queue.failures = 0;
      this.queueDiscardNotice(id, run, queue);
      this.pumpPersistentLogs(id, run, queue);
    }, error => {
      // appendLog is non-idempotent: a lost reply can follow a successful
      // write, so never submit this same record a second time.
      queue.pending.shift(); queue.bytes -= item.bytes; queue.active = false; queue.failures++;
      if (item.summary) {
        queue.droppedEntries += item.summary.droppedEntries; queue.droppedBytes += item.summary.droppedBytes;
        queue.unconfirmedEntries += item.summary.unconfirmedEntries; queue.unconfirmedBytes += item.summary.unconfirmedBytes;
        queue.unconfirmedNotices += item.summary.unconfirmedNotices + 1;
      } else { queue.unconfirmedEntries++; queue.unconfirmedBytes += item.bytes; }
      const previousError = queue.error;
      queue.error = `Persistent log append for run ${run.id} was not confirmed and may already have been written: ${String(error).slice(-8192)}`;
      if (!run.error || run.error === previousError) run.error = queue.error;
      this.displayPersistenceNotice(id, run, queue.error); this.changed();
      this.queueDiscardNotice(id, run, queue);
      if (!this.disposed && queue.failures < PERSISTENT_LOG_FAILURE_LIMIT && queue.pending.length) {
        queue.retry = setTimeout(() => { queue.retry = undefined; this.pumpPersistentLogs(id, run, queue); }, 1000);
        queue.retry.unref();
      } else this.abandonPersistentLogs(id, run, queue);
    });
  }
  private abandonPersistentLogs(id: string, run: Invocation, queue: PersistentLogQueue): void {
    for (const item of queue.pending) {
      if (item.summary) {
        queue.droppedEntries += item.summary.droppedEntries; queue.droppedBytes += item.summary.droppedBytes;
        queue.unconfirmedEntries += item.summary.unconfirmedEntries; queue.unconfirmedBytes += item.summary.unconfirmedBytes;
        queue.unconfirmedNotices += item.summary.unconfirmedNotices;
      } else {
        queue.droppedEntries++; queue.droppedBytes += item.bytes;
        queue.totalDroppedEntries++; queue.totalDroppedBytes += item.bytes;
      }
    }
    queue.pending.length = 0; queue.bytes = 0; queue.tracked = false; queue.blockedUntil = Date.now() + 30_000;
    this.retainedPersistentLogs.delete(queue);
    this.displayPersistenceNotice(id, run, `Persistent log queue discarded ${queue.droppedEntries} entries (${queue.droppedBytes} serialized bytes); ${queue.unconfirmedEntries} entries (${queue.unconfirmedBytes} serialized bytes) are unconfirmed and may already have been written.`);
  }
  async exclusive<T>(id: string, operation: () => Promise<T>): Promise<T> {
    if (this.disposed || this.deleting.has(id)) throw new Error('Application service is stopping or application is being deleted');
    const before = this.queues.get(id) ?? Promise.resolve();
    const pending = before.catch(() => undefined).then(operation);
    this.queues.set(id, pending);
    try { return await pending; }
    finally { if (this.queues.get(id) === pending) this.queues.delete(id); }
  }
  async start(id: string, repair = false, signal?: AbortSignal, replace = false, revision?: number, expectedCurrentRevision?: number): Promise<void> {
    signal?.throwIfAborted();
    if (!repair && this.starts.has(id)) return this.starts.get(id)!;
    // Cancellation is owned by the invocation, not the currently selected UI.
    const existing = this.invocations.get(id);
    if (existing && ['installing', 'building', 'starting', 'running'].includes(existing.phase) && !repair && !replace) {
      await this.queues.get(id); return;
    }
    const ref = createInvocationRef("local-app", id, { next: () => randomUUID() });
    const requests = this.requests.get(id) ?? new Set<InvocationRef>();
    requests.add(ref); this.requests.set(id, requests);
    const cancel = () => {
      this.cancellations.add(ref);
      const run = this.preparing.get(id) ?? this.invocations.get(id);
      if (run?.operationId === ref.operationId) run.controller.abort();
    };
    signal?.addEventListener('abort', cancel, { once: true });
    const operation = this.exclusive(id, async () => {
      if (this.preparing.has(id)) throw new Error('An application update is still stopping; retry Stop before starting it');
      if (this.invocations.get(id)?.phase === 'stopping') throw new Error('The previous application is still stopping; retry Stop');
      if (this.cancellations.has(ref)) throw new Error("Application operation cancelled");
      const previous = this.invocations.get(id);
      if (this.cancellations.has(ref)) throw new Error("Application operation cancelled");
      const row = revision !== undefined && this.store instanceof CoreLocalAppStore ? await this.store.getRevision(id, revision) : await this.store.get(id);
      if (expectedCurrentRevision !== undefined && row.currentRevision !== expectedCurrentRevision) throw new Error('The current version changed; refresh before switching versions');
      if (this.cancellations.has(ref)) throw new Error("Application operation cancelled");
      if (this.store instanceof CoreLocalAppStore) await this.store.assertTarget(row);
      const missing = (row.manifest.capabilities ?? []).filter(c => c !== 'agent' && !row.grants.includes(c));
      if (missing.length) throw new Error(`Application permission required: ${missing.join(', ')}`);
      // A failed invocation remains the owner of its processes and its
      // run-ended callback until both have actually completed.
      if (previous && previous.phase !== 'running' && previous.phase !== 'stopped') await this.stopInvocation(id, previous);
      if (this.cancellations.has(ref)) throw new Error('Application operation cancelled');
      const run: Invocation = { id: ref.operationId, operationId: ref.operationId, revision: row.revision, releaseDigest: row.releaseDigest, controller: new AbortController(), phase: 'starting', children: new Set(), services: new Map(), workspace: '', manifest: row.manifest, readiness: new Map() };
      if (previous?.phase === 'running') this.preparing.set(id, run); else this.invocations.set(id, run);
      this.changed();
      try {
        run.workspace = await (this.store instanceof CoreLocalAppStore ? this.store.prepareWorkspace(row, repair) : this.store.prepareWorkspace(row));
        if (this.store instanceof CoreLocalAppStore) { run.id = this.store.run(id)!.runId; run.preparationDataPath = this.store.preparationDataPath(id); }
        const marker = join(run.workspace, '.lingxi-prepared.json');
        let prepared = false;
        try { prepared = JSON.parse(await fs.readFile(marker, 'utf8')).revision === row.revision; } catch { /* first install */ }
        if (repair || !prepared) {
          for (const [phase, steps] of [['installing', row.manifest.install ?? []], ['building', row.manifest.build ?? []]] as const) {
            run.phase = phase; await this.syncRun(id, run); this.changed();
            for (const step of steps) {
              run.controller.signal.throwIfAborted();
              this.log(id, run, phase, step.command);
              const child = await this.launch(id, run, step, phase, {}, phase === 'installing');
              await this.waitExit(child, run.controller.signal);
            }
          }
          run.controller.signal.throwIfAborted();
          await atomicJson(marker, { revision: row.revision });
        }
        run.phase = 'starting'; this.changed();
        for (const spec of orderedProcesses(row.manifest.processes ?? [])) {
          run.controller.signal.throwIfAborted();
          const port = await freePort();
          run.services.set(spec.name, { port, url: `http://127.0.0.1:${port}` });
          const serviceEnv: NodeJS.ProcessEnv = { PORT: String(port), HOST: '127.0.0.1' };
          for (const [name, service] of run.services) {
            const key = name.toUpperCase().replace(/[-.]/g, '_');
            serviceEnv[`LOCAL_APP_${key}_URL`] = service.url;
            serviceEnv[`LOCAL_APP_${key}_PORT`] = String(service.port);
          }
          if (spec.ready?.output) run.readiness.set(spec.name, { needle: spec.ready.output, matched: false, stdout: '', stderr: '' });
          const child = await this.launch(id, run, spec, spec.name, serviceEnv, false);
          this.watchService(id, run, spec.name, child, () => this.invocations.get(id) === run || this.preparing.get(id) === run);
          await this.waitReady(id, run, spec, child, port);
        }
        if (row.manifest.entry.type === 'static' && !(await fs.stat(await containedFile(run.workspace, row.manifest.entry.path))).isFile()) throw new Error('The application entry is not a file');
        run.controller.signal.throwIfAborted();
        run.phase = 'running';
        await this.syncRun(id, run);
        // Commit the version while the previous run still serves. The commit
        // is a transaction boundary: an abandoned waiter cannot undo it.
        signal?.removeEventListener('abort', cancel);
        run.controller.signal.throwIfAborted();
        if (this.store instanceof CoreLocalAppStore) await this.store.activate(id, run.revision);
        // The previously running version can fail while this version is
        // preparing. Do not discard its ownership at the handover.
        if (previous && previous.phase !== 'running' && previous.phase !== 'stopped') await this.stopInvocation(id, previous);
        this.preparing.delete(id); this.invocations.set(id, run);
        if (previous && previous !== run && previous.phase === 'running') {
          run.retired = [previous];
          try { await this.stopInvocation(id, previous); run.retired = []; }
          catch (error) {
            run.error = `Previous application cleanup needs retry: ${String(error)}`;
            this.log(id, run, 'host', run.error, 'stderr');
          }
        }
        this.log(id, run, 'host', 'Application is ready'); this.changed();
      } catch (error) {
        const failure = run.error;
        const canceled = run.controller.signal.aborted && !failure;
        await this.cleanupFailedStartup(id, run, failure ?? String(error instanceof Error ? error.message : error), canceled);
        this.preparing.delete(id);
        this.changed();
        throw failure ? new Error(failure, { cause: error }) : error;
      }
    });
    this.starts.set(id, operation);
    try { await operation; } finally {
      if (this.starts.get(id) === operation) this.starts.delete(id);
      signal?.removeEventListener('abort', cancel);
      requests.delete(ref); if (!requests.size) this.requests.delete(id);
    }
  }
  /** Resume services for an expressly retained background Agent invocation. */
  async resume(id: string, revision: number, invocationId: string, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    if (this.starts.has(id)) await this.starts.get(id);
    const run = this.invocations.get(id);
    if (!run || run.revision !== revision || run.id !== invocationId) throw new Error('Application invocation changed before resume');
    if (run.phase === 'running') return;
    const ref = createInvocationRef('local-app', id, { next: () => randomUUID() });
    const requests = this.requests.get(id) ?? new Set<InvocationRef>(); requests.add(ref); this.requests.set(id, requests);
    const cancel = () => { this.cancellations.add(ref); if (this.invocations.get(id) === run) run.controller.abort(); };
    signal?.addEventListener('abort', cancel, { once: true });
    if (signal?.aborted) cancel();
    const operation = this.exclusive(id, async () => {
      if (this.cancellations.has(ref)) throw new Error('Application operation cancelled');
      const row = this.store instanceof CoreLocalAppStore ? await this.store.getRevision(id, revision) : await this.store.get(id);
      if (this.cancellations.has(ref)) throw new Error('Application operation cancelled');
      if (this.invocations.get(id) !== run || row.revision !== revision || run.phase !== 'stopped') throw new Error('Application invocation changed before resume');
      const missing = (run.manifest.capabilities ?? []).filter(capability => !row.grants.includes(capability));
      if (missing.length) throw new Error(`Application permission required: ${missing.join(', ')}`);
      // A retained invocation can be stopped again after resume; its previous
      // successful stop notification belongs only to the prior lifecycle.
      this.stopNotifications.delete(run);
      this.failedNotifications.delete(run);
      this.failedNotificationStarted.delete(run);
      run.controller = new AbortController(); run.error = undefined; run.readiness.clear();
      try { await this.startServices(id, run); await this.syncRun(id, run); }
      catch (error) {
        const failure = run.error, canceled = run.controller.signal.aborted && !failure;
        await this.cleanupFailedStartup(id, run, failure ?? String(error instanceof Error ? error.message : error), canceled);
        this.changed(); throw failure ? new Error(failure, { cause: error }) : error;
      }
    });
    this.starts.set(id, operation);
    try { await operation; }
    finally {
      if (this.starts.get(id) === operation) this.starts.delete(id);
      signal?.removeEventListener('abort', cancel);
      requests.delete(ref); if (!requests.size) this.requests.delete(id);
    }
  }
  private async cleanupFailedStartup(id: string, run: Invocation, failure: string, canceled: boolean): Promise<void> {
    run.controller.abort(); run.phase = 'stopping'; run.error = canceled ? undefined : failure;
    if (run.error) this.log(id, run, 'host', run.error, 'stderr');
    // The candidate stays reachable until its process tree, notification,
    // and persisted state have all completed. Start these independently so
    // a stalled broker cannot prevent a process from being stopped.
    const cleanup = await Promise.allSettled([
      this.waitRunNotification(id, run, canceled ? 'stopped' : 'failed'),
      Promise.resolve().then(() => this.killChildren(run)),
    ]);
    const errors = cleanup.flatMap(result => result.status === 'rejected' ? [result.reason] : []);
    run.phase = errors.length ? 'stopping' : canceled ? 'stopped' : 'failed';
    if (errors.length) run.error = [run.error, ...errors.map(String)].filter(Boolean).join('; ');
    try { await this.syncRun(id, run); } catch (syncError) {
      errors.push(syncError); run.phase = 'stopping';
      run.error = [run.error, String(syncError)].filter(Boolean).join('; ');
    }
    if (errors.length) { this.changed(); throw new AggregateError(errors, 'Application cleanup needs retry'); }
  }
  private cleanupFailedRun(id: string, run: Invocation): void {
    // Broker shutdown and process cleanup are independent: a rejected or
    // stalled callback must not prevent other services from being stopped.
    void this.runNotification(id, run, 'failed')
      .catch(error => this.log(id, run, 'host', String(error), 'stderr'));
    void Promise.resolve().then(() => this.killChildren(run)).then(() => this.syncRun(id, run))
      .catch(error => this.log(id, run, 'host', String(error), 'stderr'));
  }
  private watchService(id: string, run: Invocation, name: string, child: RuntimeChild, isCurrent: () => boolean): void {
    const controller = run.controller;
    const failed = (message: string) => {
      if (!isCurrent() || run.controller !== controller || controller.signal.aborted) return;
      run.error = message; run.phase = 'failed';
      this.log(id, run, name, message, 'stderr'); controller.abort(); this.changed();
      this.cleanupFailedRun(id, run);
    };
    const exited = (code: number | null, signal: NodeJS.Signals | null) => failed(`${name} exited (${code ?? signal})`);
    if (child instanceof LocalAppProcessClient) {
      child.onExit(exited);
      child.onFailure(error => failed(`${name}: ${error.message}`));
    } else child.once('exit', exited);
  }
  private async startServices(id: string, run: Invocation): Promise<void> {
    run.phase = 'starting'; this.changed();
    const controller = run.controller;
    for (const spec of orderedProcesses(run.manifest.processes ?? [])) {
      controller.signal.throwIfAborted();
      const port = await freePort();
      run.services.set(spec.name, { port, url: `http://127.0.0.1:${port}` });
      const serviceEnv: NodeJS.ProcessEnv = { PORT: String(port), HOST: '127.0.0.1' };
      for (const [name, service] of run.services) { const key = name.toUpperCase().replace(/[-.]/g, '_'); serviceEnv[`LOCAL_APP_${key}_URL`] = service.url; serviceEnv[`LOCAL_APP_${key}_PORT`] = String(service.port); }
      if (spec.ready?.output) run.readiness.set(spec.name, { needle: spec.ready.output, matched: false, stdout: '', stderr: '' });
      const child = await this.launch(id, run, spec, spec.name, serviceEnv, false);
      this.watchService(id, run, spec.name, child, () => this.invocations.get(id) === run);
      await this.waitReady(id, run, spec, child, port);
    }
    if (run.manifest.entry.type === 'static' && !(await fs.stat(await containedFile(run.workspace, run.manifest.entry.path))).isFile()) throw new Error('The application entry is not a file');
    controller.signal.throwIfAborted();
    run.phase = 'running'; this.log(id, run, 'host', 'Application is ready'); this.changed();
  }
  private async launch(id: string, run: Invocation, step: LocalAppStep, name: string, extra: NodeJS.ProcessEnv, createEnvironment: boolean): Promise<RuntimeChild> {
    const cwd = await containedFile(run.workspace, step.cwd ?? '.');
    const preparing = name === 'installing' || name === 'building';
    const dataDirectory = preparing ? run.preparationDataPath ?? join(run.workspace, '.lingxi-prepare-data') : this.store.dataPath(id);
    if (preparing) await fs.mkdir(dataDirectory, { recursive: true, mode: 0o700 });
    const env: NodeJS.ProcessEnv = { ...appEnvironment(this.options.environment), ...step.env, ...extra, LOCAL_APP_DATA_DIR: dataDirectory, LOCAL_APP_ID: id, LOCAL_APP_RUN_ID: run.id, LOCAL_APP_BASE_PATH: '/' };
    // Build scripts get a disposable preparation location, never the live data alias.
    // Finder/Dock launches have a minimal PATH. Reuse the same installed tools
    // Bash provisions, and fall back to the already packaged Node/Bun runtimes.
    const home = env.HOME ?? env.USERPROFILE;
    const paths = [env.PATH ?? '', ...(home ? ['.local/bin', '.local/share/mise/shims', '.volta/bin', '.bun/bin', '.cargo/bin'].map(p => join(home, p)) : [])];
    if (process.platform !== 'win32') paths.push('/opt/homebrew/bin', '/usr/local/bin', '/usr/bin', '/bin');
    if (this.options.nodeExecutable || this.options.bunExecutable) {
      const bin = join(this.store.directory(id), 'runtime-bin');
      await fs.mkdir(bin, { recursive: true, mode: 0o700 });
      for (const [name, executable] of [['node', this.options.nodeExecutable], ['bun', this.options.bunExecutable]] as const) {
        if (!executable) continue;
        await fs.writeFile(join(bin, name), `#!/bin/sh\n${name === 'node' ? 'ELECTRON_RUN_AS_NODE=1 ' : ''}exec ${shellQuote(executable)} "$@"\n`, { mode: 0o700 });
      }
      paths.push(bin);
    }
    env.PATH = paths.filter(Boolean).join(delimiter);
    const command = step.command;
    if (step.venv) {
      const venv = join(run.workspace, step.venv);
      const executable = join(venv, process.platform === 'win32' ? 'Scripts/python.exe' : 'bin/python');
      if (createEnvironment) {
        try { await fs.access(executable); }
        catch {
          const prepare = step.python ? `uv venv --python ${shellQuote(step.python)} ${shellQuote(venv)}` : `python3 -m venv ${shellQuote(venv)}`;
          const child = await this.spawnProcess(id, run, name, prepare, cwd, env);
          await this.waitExit(child, run.controller.signal);
        }
      }
      await fs.access(executable);
      env.PATH = `${dirname(executable)}${process.platform === 'win32' ? ';' : ':'}${env.PATH ?? ''}`;
      Object.assign(env, { VIRTUAL_ENV: venv, PYTHONNOUSERSITE: '1' });
    }
    return this.spawnProcess(id, run, name, command, cwd, env);
  }
  private async spawnProcess(id: string, run: Invocation, name: string, command: string, cwd: string, env: NodeJS.ProcessEnv): Promise<RuntimeChild> {
    run.controller.signal.throwIfAborted();
    if (process.platform !== 'win32') return this.spawn(id, run, name, command, cwd, env);
    const executable = this.options.processHelperExecutable ?? resolveServerBin();
    const shell = this.options.shell ?? 'bash.exe';
    const processEnv = windowsProcessEnvironment(env);
    const resolved = await (this.options.resolveWindowsExecutable ?? resolveWindowsProgram)(shell, cwd, processEnv);
    run.controller.signal.throwIfAborted();
    const child = new LocalAppProcessClient({ executable, helperArgs: this.options.processHelperArguments ?? ['--local-app-process'],
      command: resolved, args: ['-c', command], cwd, env: processEnv, helperEnv: appEnvironment(this.options.environment),
      onOutput: (stream, text) => this.log(id, run, name, text, stream),
      onDiagnostic: text => this.log(id, run, name, text, 'stderr') });
    run.children.add(child);
    child.once('released', () => run.children.delete(child));
    await child.waitStarted(run.controller.signal);
    return child;
  }
  private spawn(id: string, run: Invocation, name: string, command: string, cwd: string, env: NodeJS.ProcessEnv): ChildProcess {
    run.controller.signal.throwIfAborted();
    const shell = this.options.shell ?? (process.platform === 'win32' ? 'bash.exe' : '/bin/bash');
    const guarded = process.platform !== 'win32';
    const child = spawn(shell, guarded ? guardedProcessArguments(command) : ['-c', command], { cwd, env, stdio: guarded ? ['ignore', 'pipe', 'pipe', 'pipe'] : ['ignore', 'pipe', 'pipe'], detached: guarded, windowsHide: true });
    run.children.add(child);
    if (guarded) {
      childClosures.set(child, new Promise(resolve => child.once('close', (code, signal) => resolve({ code, signal }))));
      const guardian = child.stdio[3] as Duplex | null;
      // Observe peer closure and prevent an asynchronous EPIPE from becoming
      // an unhandled process error; group absence is checked separately.
      guardian?.on('error', () => undefined);
      guardian?.resume();
      // `close` waits for inherited output handles, so request cleanup when
      // the leader exits. The guardian retains the group's identity for us.
      child.once('exit', () => { void stopTree(child).catch(() => undefined); });
    }
    child.once('close', code => {
      if (!guarded) {
        // Windows has no anchored process-group capability here. Natural
        // command completion only releases the exited leader; cancellation
        // and failed commands keep ownership for explicit cleanup.
        if (code === 0 && !run.controller.signal.aborted) run.children.delete(child);
        return;
      }
      void stopTree(child).then(() => run.children.delete(child)).catch(error => this.log(id, run, name, String(error), 'stderr'));
    });
    child.stdout?.setEncoding('utf8').on('data', data => this.log(id, run, name, data, 'stdout'));
    child.stderr?.setEncoding('utf8').on('data', data => this.log(id, run, name, data, 'stderr'));
    child.on('error', error => this.log(id, run, name, error.message, 'stderr'));
    return child;
  }
  private waitExit(child: RuntimeChild, signal: AbortSignal): Promise<void> {
    if (child instanceof LocalAppProcessClient) return child.waitExit(signal).then(({ code, signal: exitSignal }) => {
      if (code !== 0) throw new Error(exitSignal ? `Command exited with signal ${exitSignal}` : `Command exited with code ${code}`);
    });
    return new Promise((resolve, reject) => {
      let settled = false;
      const tracked = childClosures.get(child);
      const clean = () => { signal.removeEventListener('abort', abort); child.removeListener('error', error); child.removeListener('close', close); child.removeListener('exit', exited); };
      const error = (err: Error) => { if (settled) return; settled = true; clean(); reject(err); };
      const close = (code: number | null, exitSignal: NodeJS.Signals | null) => {
        if (settled) return;
        settled = true;
        clean();
        if (code === 0) resolve();
        else reject(new Error(exitSignal ? `Command exited with signal ${exitSignal}` : `Command exited with code ${code}`));
      };
      const abort = () => error(new Error('Application operation cancelled'));
      const exited = () => { if (tracked && process.platform !== 'win32') void stopTree(child).catch(error); };
      child.once('error', error); child.once('exit', exited); signal.addEventListener('abort', abort, { once: true });
      if (tracked) {
        void tracked.then(({ code, signal: exitSignal }) => {
          if (process.platform === 'win32') close(code, exitSignal);
          else void stopTree(child).then(() => close(code, exitSignal), error);
        });
      } else child.once('close', close);
      if (signal.aborted) abort();
      else if (child.exitCode !== null || child.signalCode !== null) {
        if (tracked) exited(); else close(child.exitCode, child.signalCode);
      }
    });
  }
  private async waitReady(id: string, run: Invocation, spec: LocalAppProcess, child: RuntimeChild, port: number): Promise<void> {
    const deadline = Date.now() + (spec.ready?.timeoutMs ?? 120000);
    while (Date.now() < deadline) {
      run.controller.signal.throwIfAborted();
      if (child instanceof LocalAppProcessClient ? child.hasExited || !child.started : child.exitCode !== null || child.signalCode !== null || !child.pid) throw new Error(`${spec.name} exited before it was ready`);
      if (spec.ready?.output) {
        if (run.readiness.get(spec.name)?.matched) return;
      } else {
        try {
          const response = await fetch(`http://127.0.0.1:${port}${spec.ready?.path ?? '/'}`, { signal: AbortSignal.any([run.controller.signal, AbortSignal.timeout(1000)]), redirect: 'error' });
          await response.body?.cancel();
          if (response.ok) return;
        } catch { /* A refused connection during startup is expected. */ }
      }
      await delay(100, undefined, { signal: run.controller.signal });
    }
    throw new Error(`${spec.name} did not become ready; see its logs`);
  }
  private async syncRun(id: string, run: Invocation): Promise<void> {
    if (this.store instanceof CoreLocalAppStore) await this.store.updateRun(id, run.id, run.phase, [...run.services].map(([name, value]) => ({ name, port: value.port })), run.error);
  }
  private async killChildren(run: Invocation): Promise<void> { await Promise.all([...run.children].map(stopTree)); run.children.clear(); run.services.clear(); }
  private runNotification(id: string, run: Invocation, reason: 'failed' | 'stopped'): Promise<void> {
    const notifications = reason === 'failed' ? this.failedNotifications : this.stopNotifications;
    if (reason === 'failed') this.failedNotificationStarted.add(run);
    let pending = notifications.get(run);
    if (!pending) {
      const notification = Promise.resolve().then(() => this.options.onRunEnded?.(id, run.id, reason));
      pending = notification.catch(error => { notifications.delete(run); throw error; });
      notifications.set(run, pending);
    }
    return pending;
  }
  private waitRunNotification(id: string, run: Invocation, reason: 'failed' | 'stopped'): Promise<void> {
    const pending = this.runNotification(id, run, reason);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Application run-ended callback did not finish; cleanup is unverified')), STOP_CALLBACK_TIMEOUT_MS);
      void pending.then(() => { clearTimeout(timer); resolve(); }, error => { clearTimeout(timer); reject(error); });
    });
  }
  private async stopInvocation(id: string, run: Invocation): Promise<void> {
    const logs = this.persistentLogs.get(run);
    if (logs && !logs.tracked && (logs.droppedEntries || logs.unconfirmedEntries || logs.unconfirmedNotices)) {
      this.displayPersistenceNotice(id, run, `Persistent log queue discarded ${logs.droppedEntries} entries (${logs.droppedBytes} serialized bytes); ${logs.unconfirmedEntries} entries (${logs.unconfirmedBytes} serialized bytes) are unconfirmed and may already have been written.`);
    }
    run.controller.abort();
    run.phase = 'stopping'; this.changed();
    const retired = run.retired ?? [];
    // Start every cleanup independently: a stuck broker or retired run must
    // not keep the current invocation's processes alive.
    const results = await Promise.allSettled([
      ...retired.map(previous => this.stopInvocation(id, previous)),
      this.waitRunNotification(id, run, this.failedNotificationStarted.has(run) ? 'failed' : 'stopped'),
      Promise.resolve().then(() => this.killChildren(run)),
    ]);
    const errors: unknown[] = [];
    run.retired = retired.filter((_, index) => results[index]?.status === 'rejected');
    for (const result of results) if (result.status === 'rejected') errors.push(result.reason);
    run.phase = errors.length ? 'stopping' : 'stopped';
    run.error = errors.length ? errors.map(String).join('; ') : undefined;
    try { await this.syncRun(id, run); } catch (error) {
      errors.push(error); run.phase = 'stopping'; run.error = errors.map(String).join('; ');
    }
    this.changed();
    if (errors.length) throw new AggregateError(errors, 'Application cleanup needs retry');
  }
  private async stopCurrent(id: string, includePreparing = true): Promise<void> {
    const preparing = includePreparing ? this.preparing.get(id) : undefined;
    const run = this.invocations.get(id);
    const runs = [preparing, run === preparing ? undefined : run].filter((value): value is Invocation => value !== undefined);
    const results = await Promise.allSettled(runs.map(current => this.stopInvocation(id, current)));
    const errors: unknown[] = [];
    for (const [index, result] of results.entries()) {
      if (result.status === 'rejected') errors.push(result.reason);
      else if (runs[index] === preparing) this.preparing.delete(id);
    }
    if (errors.length) throw new AggregateError(errors, 'Application cleanup needs retry');
  }
  async stop(id: string): Promise<void> {
    // Abort before waiting for the queue: Stop must interrupt an in-progress install/readiness wait.
    for (const ref of this.requests.get(id) ?? []) this.cancellations.add(ref);
    this.preparing.get(id)?.controller.abort();
    this.invocations.get(id)?.controller.abort();
    await this.exclusive(id, () => this.stopCurrent(id));
  }
  async execute(id: string, command: string): Promise<{ output: string }> {
    const run = this.invocations.get(id);
    if (!run || run.phase !== 'running') throw new Error('Application is not running');
    const name = `command-${randomUUID()}`;
    const child = await this.launch(id, run, { command }, name, {}, false);
    await this.waitExit(child, run.controller.signal);
    return { output: this.logs(id).filter(l => l.runId === run.id && l.process === name).map(l => l.text).join('') };
  }
  async remove(id: string): Promise<void> {
    for (const ref of this.requests.get(id) ?? []) this.cancellations.add(ref);
    this.preparing.get(id)?.controller.abort();
    this.invocations.get(id)?.controller.abort();
    const removal = this.exclusive(id, async () => { await this.stopCurrent(id); await this.store.remove(id); this.invocations.delete(id); this.output.delete(id); this.changed(); });
    this.deleting.add(id);
    try { await removal; } finally { this.deleting.delete(id); }
  }
  async dispose(): Promise<void> {
    this.disposed = true;
    for (const requests of this.requests.values()) for (const ref of requests) this.cancellations.add(ref);
    for (const run of this.preparing.values()) run.controller.abort();
    for (const run of this.invocations.values()) run.controller.abort();
    await Promise.allSettled([...this.queues.values()]);
    await Promise.all([...this.invocations.keys()].map(id => this.stopCurrent(id)));
  }
}
