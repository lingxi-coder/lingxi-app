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
import { setTimeout as delay } from 'node:timers/promises';
import type { DesktopLocalApp, DesktopAppManifest, LocalAppLog, LocalAppPhase, LocalAppStep, LocalAppProcess } from '../../shared/localApps.js';
import { CoreLocalAppStore } from './coreStore.js';
import { guardedProcessArguments } from './processGuardian.js';
import { LocalAppStore, atomicJson, containedFile } from './store.js';

interface Invocation {
  id: string; operationId: string; revision: number; releaseDigest?: string; controller: AbortController; phase: LocalAppPhase;
  children: Set<ChildProcess>; services: Map<string, { port: number; url: string }>;
  manifest: DesktopAppManifest;
  readiness: Map<string, { needle: string; matched: boolean; stdout: string; stderr: string }>;
  workspace: string; preparationDataPath?: string; error?: string; retired?: Invocation[];
}
export interface RuntimeOptions { onRunEnded?: (appId: string, runId: string, reason: string) => Promise<void> | void; nodeExecutable?: string; bunExecutable?: string; shell?: string; environment?: NodeJS.ProcessEnv }

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
const stoppingChildren = new WeakMap<ChildProcess, Promise<void>>();
function stopTree(child: ChildProcess): Promise<void> {
  let pending = stoppingChildren.get(child);
  if (!pending) {
    pending = terminateTree(child).catch(error => { stoppingChildren.delete(child); throw error; });
    stoppingChildren.set(child, pending);
  }
  return pending;
}
async function terminateTree(child: ChildProcess): Promise<void> {
  if (!child.pid) return;
  const pid = child.pid;
  if (process.platform === 'win32') {
    await new Promise<void>(resolve => {
      const killer = spawn('taskkill', ['/PID', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
      killer.once('close', () => resolve()); killer.once('error', () => resolve());
    });
    return;
  }
  const signal = (s: NodeJS.Signals | 0): boolean => { try { process.kill(-pid, s); return true; } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ESRCH' || (code === 'EPERM' && (child.exitCode !== null || child.signalCode !== null))) return false;
    // On macOS an exiting group can briefly be owned by init before libuv
    // delivers the root's exit event. EPERM from a probe means it still exists;
    // keep polling. An actual TERM/KILL denial remains a cleanup failure.
    if (code === 'EPERM' && s === 0) return true;
    throw error;
  } };
  if (!signal('SIGTERM')) return;
  for (let n = 0; n < 30; n++) { if (!signal(0)) return; await delay(50); }
  signal('SIGKILL');
  for (let n = 0; n < 30; n++) { if (!signal(0)) return; await delay(50); }
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
    if (this.store instanceof CoreLocalAppStore && run.id !== run.operationId) {
      void this.store.appendLog(id, run.id, JSON.stringify(lines.at(-1))).catch(error => { run.error = `Could not persist application log: ${String(error)}`; this.changed(); });
    }
    this.changed();
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
          child.once('exit', (code, signal) => {
            if ((this.invocations.get(id) !== run && this.preparing.get(id) !== run) || run.controller.signal.aborted) return;
            run.error = `${spec.name} exited (${code ?? signal})`; run.phase = 'failed';
            this.log(id, run, spec.name, run.error, 'stderr'); run.controller.abort(); this.changed();
            void Promise.resolve(this.options.onRunEnded?.(id, run.id, 'failed')).then(() => this.killChildren(run)).then(() => this.syncRun(id, run)).catch(error => this.log(id, run, 'host', String(error)));
          });
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
        run.controller.abort();
        run.phase = 'stopping';
        await this.killChildren(run);
        run.phase = canceled ? 'stopped' : 'failed';
        run.error = canceled ? undefined : failure ?? String(error instanceof Error ? error.message : error);
        if (run.error) this.log(id, run, 'host', run.error, 'stderr');
        await this.syncRun(id, run);
        this.preparing.delete(id);
        await this.options.onRunEnded?.(id, run.id, canceled ? 'stopped' : 'failed');
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
      run.controller = new AbortController(); run.error = undefined; run.readiness.clear();
      try { await this.startServices(id, run); await this.syncRun(id, run); }
      catch (error) {
        const failure = run.error, canceled = run.controller.signal.aborted && !failure;
        run.controller.abort(); await this.killChildren(run);
        run.phase = canceled ? 'stopped' : 'failed'; run.error = canceled ? undefined : failure ?? String(error instanceof Error ? error.message : error);
        if (run.error) this.log(id, run, 'host', run.error, 'stderr');
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
      child.once('exit', (code, signal) => {
        if (this.invocations.get(id) !== run || run.controller !== controller || controller.signal.aborted) return;
        run.error = `${spec.name} exited (${code ?? signal})`; run.phase = 'failed';
        this.log(id, run, spec.name, run.error, 'stderr'); controller.abort(); this.changed();
        void Promise.resolve(this.options.onRunEnded?.(id, run.id, 'failed')).then(() => this.killChildren(run)).then(() => this.syncRun(id, run)).catch(error => this.log(id, run, 'host', String(error)));
      });
      await this.waitReady(id, run, spec, child, port);
    }
    if (run.manifest.entry.type === 'static' && !(await fs.stat(await containedFile(run.workspace, run.manifest.entry.path))).isFile()) throw new Error('The application entry is not a file');
    controller.signal.throwIfAborted();
    run.phase = 'running'; this.log(id, run, 'host', 'Application is ready'); this.changed();
  }
  private async launch(id: string, run: Invocation, step: LocalAppStep, name: string, extra: NodeJS.ProcessEnv, createEnvironment: boolean): Promise<ChildProcess> {
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
          const child = this.spawn(id, run, name, prepare, cwd, env);
          await this.waitExit(child, run.controller.signal);
        }
      }
      await fs.access(executable);
      env.PATH = `${dirname(executable)}${process.platform === 'win32' ? ';' : ':'}${env.PATH ?? ''}`;
      Object.assign(env, { VIRTUAL_ENV: venv, PYTHONNOUSERSITE: '1' });
    }
    return this.spawn(id, run, name, command, cwd, env);
  }
  private spawn(id: string, run: Invocation, name: string, command: string, cwd: string, env: NodeJS.ProcessEnv): ChildProcess {
    run.controller.signal.throwIfAborted();
    const shell = this.options.shell ?? (process.platform === 'win32' ? 'bash.exe' : '/bin/bash');
    const guarded = process.platform !== 'win32';
    const child = spawn(shell, guarded ? guardedProcessArguments(command) : ['-c', command], { cwd, env, stdio: guarded ? ['ignore', 'pipe', 'pipe', 'pipe'] : ['ignore', 'pipe', 'pipe'], detached: guarded, windowsHide: true });
    run.children.add(child);
    // Drain descendants as soon as the root closes, then discard its pid.
    // Keeping exited roots until a later restart risks signaling a recycled pid.
    child.once('close', () => {
      void stopTree(child).then(() => run.children.delete(child)).catch(error => this.log(id, run, name, String(error), 'stderr'));
    });
    child.stdout?.setEncoding('utf8').on('data', data => this.log(id, run, name, data, 'stdout'));
    child.stderr?.setEncoding('utf8').on('data', data => this.log(id, run, name, data, 'stderr'));
    child.on('error', error => this.log(id, run, name, error.message, 'stderr'));
    return child;
  }
  private waitExit(child: ChildProcess, signal: AbortSignal): Promise<void> {
    return new Promise((resolve, reject) => {
      const clean = () => { signal.removeEventListener('abort', abort); child.removeListener('error', error); child.removeListener('close', close); };
      const error = (err: Error) => { clean(); reject(err); };
      const close = (code: number | null) => { clean(); code === 0 ? resolve() : reject(new Error(`Command exited with code ${code}`)); };
      const abort = () => { clean(); reject(new Error('Application operation cancelled')); };
      child.once('error', error); child.once('close', close); signal.addEventListener('abort', abort, { once: true });
      if (signal.aborted) abort(); else if (child.exitCode !== null) close(child.exitCode);
    });
  }
  private async waitReady(id: string, run: Invocation, spec: LocalAppProcess, child: ChildProcess, port: number): Promise<void> {
    const deadline = Date.now() + (spec.ready?.timeoutMs ?? 120000);
    while (Date.now() < deadline) {
      run.controller.signal.throwIfAborted();
      if (child.exitCode !== null || child.signalCode !== null || !child.pid) throw new Error(`${spec.name} exited before it was ready`);
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
  private async stopInvocation(id: string, run: Invocation): Promise<void> {
    run.controller.abort();
    const errors: unknown[] = [], pending: Invocation[] = [];
    for (const retired of run.retired ?? []) {
      try { await this.stopInvocation(id, retired); } catch (error) { errors.push(error); pending.push(retired); }
    }
    run.retired = pending;
    run.phase = 'stopping'; this.changed();
    // Always attempt process cleanup even if closing a broker connection fails.
    try { await this.options.onRunEnded?.(id, run.id, 'stopped'); } catch (error) { errors.push(error); }
    try { await this.killChildren(run); } catch (error) { errors.push(error); }
    run.phase = errors.length ? 'stopping' : 'stopped';
    run.error = errors.length ? errors.map(String).join('; ') : undefined;
    try { await this.syncRun(id, run); } catch (error) { errors.push(error); }
    this.changed();
    if (errors.length) throw new AggregateError(errors, 'Application cleanup needs retry');
  }
  private async stopCurrent(id: string, includePreparing = true): Promise<void> {
    const errors: unknown[] = [];
    const preparing = includePreparing ? this.preparing.get(id) : undefined;
    if (preparing) {
      try { await this.stopInvocation(id, preparing); this.preparing.delete(id); } catch (error) { errors.push(error); }
    }
    const run = this.invocations.get(id);
    if (run && run !== preparing) { try { await this.stopInvocation(id, run); } catch (error) { errors.push(error); } }
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
