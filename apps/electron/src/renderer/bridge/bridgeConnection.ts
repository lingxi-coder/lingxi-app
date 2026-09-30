import type { LingxiApi } from './lingxi.js';

/** Stop explicit agent aliases through the session they belong to, even across UI navigation. */
export async function stopSessionSubagents(
  host: Pick<LingxiApi, 'command'>,
  sessionId: string,
  agentIds: readonly string[],
): Promise<void> {
  const results = await Promise.allSettled(agentIds.map((agentId) =>
    host.command(sessionId, { type: 'task_stop', task_id: agentId })));
  // Refresh after dispatch so reconnect/stale roster entries can reconcile.
  await host.command(sessionId, { type: 'list_session_agents' });
  const failures = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected');
  if (failures.length) throw new Error(`Failed to stop background agents: ${failures.map((result) => result.reason instanceof Error ? result.reason.message : String(result.reason)).join('; ')}`);
}

export function messageFrom(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return 'The desktop host could not complete that action.';
}

/** An interaction can race an engine-side expiry or another window's answer. */
export function isPermissionRequestGone(error: unknown): boolean {
  return /\bpermission request is not pending\b/.test(messageFrom(error));
}

export const BRIDGE_RESTART_TIMEOUT_MS = 20_000;

export function restartBridgePreconditionError(
  sessionLoading: boolean,
  hasHost: boolean,
  sessionId: string | null | undefined,
): Error | null {
  if (sessionLoading) {
    return new Error('Cannot restart the engine while a session is loading. Please wait for it to finish opening.');
  }
  if (!hasHost) return new Error('Desktop host unavailable.');
  if (!sessionId) return new Error('Open a session before restarting the engine.');
  return null;
}

/**
 * Keep renderer actions bounded even if an IPC handler never settles. The
 * underlying restart is intentionally not cancelled: the host owns that
 * lifecycle and may still finish after the renderer has entered recovery.
 */
export async function restartBridgeWithTimeout(
  restart: () => Promise<void>,
  timeoutMs = BRIDGE_RESTART_TIMEOUT_MS,
): Promise<void> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new Error('invalid bridge restart timeout');
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const operation = Promise.resolve().then(restart);
    await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Timed out waiting for the engine to restart.')), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

/**
 * Share one host restart per session. A renderer timeout must not release the
 * slot while the host is still stopping/starting that session.
 */
export function restartBridgeSingleFlight(
  inFlight: Map<string, Promise<void>>,
  sessionId: string,
  restart: () => Promise<void>,
): Promise<void> {
  const current = inFlight.get(sessionId);
  if (current) return current;

  const operation = Promise.resolve().then(restart);
  inFlight.set(sessionId, operation);
  const clear = (): void => {
    if (inFlight.get(sessionId) === operation) inFlight.delete(sessionId);
  };
  // Supplying both handlers prevents a rejected operation's cleanup promise
  // from becoming an unhandled rejection while preserving the original error
  // for every caller awaiting `operation`.
  void operation.then(clear, clear);
  return operation;
}
