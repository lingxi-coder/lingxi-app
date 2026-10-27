import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { LocalAppCoreClient, type LocalAppCoreTransport } from '../../src/main/local-apps/coreClient';

/** Fixture-only process evidence; requests and service output are unchanged. */
export function tracedLocalAppCore(executable: string, configHome: string, trace: (message: string) => void): LocalAppCoreTransport {
  const client = new LocalAppCoreClient(executable, configHome, message => trace(`library stderr: ${message}`));
  const observed = new WeakSet<ChildProcessWithoutNullStreams>();
  trace(`library executable: ${executable}`);
  return {
    request<T>(request: Record<string, unknown>): Promise<T> {
      const response = client.request<T>(request);
      const child = Reflect.get(client, 'child') as ChildProcessWithoutNullStreams | undefined;
      if (child && !observed.has(child)) {
        observed.add(child);
        const pid = child.pid ?? 'unknown';
        trace(`library process started: pid=${pid}`);
        child.once('error', (error: NodeJS.ErrnoException) => trace(`library process error: pid=${pid} code=${error.code ?? 'unknown'}`));
        child.once('exit', (code, signal) => trace(`library process exit: pid=${pid} code=${code ?? 'none'} signal=${signal ?? 'none'}`));
        child.once('close', (code, signal) => trace(`library process close: pid=${pid} code=${code ?? 'none'} signal=${signal ?? 'none'}`));
      }
      return response;
    },
    dispose: () => client.dispose(),
    onDisconnected: listener => client.onDisconnected(listener),
  };
}
