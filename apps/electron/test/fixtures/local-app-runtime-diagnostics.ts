import { execFile, type ChildProcess } from 'node:child_process';
import type { LocalAppRuntime } from '../../src/main/local-apps/runtime';

/** Observe only this fixture's readiness requests and child lifecycle. */
export function traceLocalAppReadiness(runtime: LocalAppRuntime, appID: string, serviceName: string) {
  const previousFetch = globalThis.fetch;
  const previousSpawn = Reflect.get(runtime, 'spawn') as (...args: unknown[]) => ChildProcess;
  const attempts = new Map<string, number>();
  const processes: { name: string; child: ChildProcess; events: string[] }[] = [];
  let expectedPort: number | undefined;
  let processGroup: { pid: number; parentPID: number; state: string }[] | null = null;
  let processGroupError: string | null = null, lastGroupCheck = 0, checkingGroup = false;
  const observeGroup = () => {
    if (process.platform === 'win32' || checkingGroup || Date.now() - lastGroupCheck < 500) return;
    const child = [...processes].reverse().find(entry => entry.name === serviceName)?.child;
    if (!child?.pid || child.exitCode !== null || child.signalCode !== null) return;
    const groupID = child.pid;
    lastGroupCheck = Date.now(); checkingGroup = true;
    // Numeric process data only: never include command lines or environments.
    execFile('/bin/ps', ['-axo', 'pid=,ppid=,pgid=,stat='], { timeout: 1500, maxBuffer: 2 * 1024 * 1024 }, (error, output) => {
      checkingGroup = false;
      if (child.exitCode !== null || child.signalCode !== null) return;
      if (error) { processGroupError = 'process status unavailable'; return; }
      processGroup = output.split('\n').flatMap(line => {
        const match = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+([A-Za-z+<>-]+)$/);
        return match && Number(match[3]) === groupID ? [{ pid: Number(match[1]), parentPID: Number(match[2]), state: match[4]! }] : [];
      });
      processGroupError = null;
    });
  };
  const observe = (outcome: string) => attempts.set(outcome, (attempts.get(outcome) ?? 0) + 1);
  const observedFetch: typeof fetch = async (input, init) => {
    const service = runtime.service(appID, serviceName);
    let target: URL | undefined;
    try { if (typeof input === 'string') target = new URL(input); } catch { /* not a fixture URL */ }
    if (!service || !target || target.origin !== service.url) return previousFetch(input, init);
    expectedPort = service.port;
    observeGroup();
    try {
      const response = await previousFetch(input, init);
      observe(`http:${response.status}`);
      return response;
    } catch (error) {
      const value = error as { name?: unknown; cause?: { code?: unknown } };
      const name = typeof value?.name === 'string' && /^[A-Za-z]+$/.test(value.name) ? value.name : 'Error';
      const code = value?.cause?.code;
      observe(`fetch:${name}${typeof code === 'string' && /^[A-Z0-9_]+$/.test(code) ? `:${code}` : ''}`);
      throw error;
    }
  };
  globalThis.fetch = observedFetch;
  Reflect.set(runtime, 'spawn', (...args: unknown[]) => {
    const child = previousSpawn.apply(runtime, args);
    const name = String(args[2]);
    const observed = { name, child, events: [] as string[] };
    processes.push(observed);
    child.once('error', (error: NodeJS.ErrnoException) => observed.events.push(`error:${error.code ?? 'unknown'}`));
    child.once('exit', (code, signal) => observed.events.push(`exit:${code ?? 'none'}:${signal ?? 'none'}`));
    child.once('close', (code, signal) => observed.events.push(`close:${code ?? 'none'}:${signal ?? 'none'}`));
    return child;
  });
  const snapshot = () => ({
    expectedPort: expectedPort ?? null,
    attempts: Object.fromEntries(attempts),
    processGroup, processGroupError,
    processes: processes.map(({ name, child, events }) => {
      const pipe = child.stdio[3] as (NodeJS.WritableStream & { destroyed?: boolean; writableEnded?: boolean }) | null;
      return { name, pid: child.pid ?? null, exitCode: child.exitCode, signal: child.signalCode, killed: child.killed,
        guardianPipe: pipe ? { destroyed: pipe.destroyed, writableEnded: pipe.writableEnded } : null, events: [...events] };
    }),
  });
  return {
    snapshot,
    failure(label: string, error: unknown, logs: ReturnType<LocalAppRuntime['logs']>) {
      return new Error(`${label}: ${JSON.stringify({ logs, readiness: snapshot() })}`, { cause: error });
    },
    restore() {
      if (globalThis.fetch === observedFetch) globalThis.fetch = previousFetch;
      Reflect.set(runtime, 'spawn', previousSpawn);
    },
  };
}
