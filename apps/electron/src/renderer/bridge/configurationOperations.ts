import type { ConfigurationDomainDto } from '@lingxi/bridge-client';
import type { ConfigurationOperationEvent } from './bridgeTypes.js';

export interface PendingConfigurationOperation {
  sessionId: string;
  resolve: (event: ConfigurationOperationEvent) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export type PendingConfigurationOperations = Map<string, PendingConfigurationOperation>;

function operationKey(sessionId: string, domain: ConfigurationDomainDto, operationId: number): string {
  return JSON.stringify([sessionId, domain, operationId]);
}

function rejectOperation(pending: PendingConfigurationOperations, key: string, error: Error, expected?: PendingConfigurationOperation): void {
  const operation = pending.get(key);
  if (!operation || (expected && operation !== expected)) return;
  pending.delete(key);
  clearTimeout(operation.timer);
  operation.reject(error);
}

/** Register ownership before dispatch, including transports that reply immediately. */
export function requestConfigurationOperation(
  pending: PendingConfigurationOperations,
  sessionId: string,
  domain: ConfigurationDomainDto,
  operationId: number,
  send: () => Promise<void>,
  timeoutMs: number,
): Promise<ConfigurationOperationEvent> {
  const key = operationKey(sessionId, domain, operationId);
  if (pending.has(key)) return Promise.reject(new Error(`configuration operation ${operationId} is already pending`));
  return new Promise((resolve, reject) => {
    let operation: PendingConfigurationOperation;
    const timer = setTimeout(() => rejectOperation(pending, key,
      new Error(`Timed out waiting for the ${domain} configuration operation.`), operation), timeoutMs);
    operation = { sessionId, resolve, reject, timer };
    pending.set(key, operation);
    try {
      void send().catch((cause: unknown) => rejectOperation(pending, key,
        cause instanceof Error ? cause : new Error(String(cause)), operation));
    } catch (cause) {
      rejectOperation(pending, key, cause instanceof Error ? cause : new Error(String(cause)), operation);
    }
  });
}

/** RPC completion belongs to its initiating runtime, regardless of UI navigation. */
export function settleConfigurationOperation(
  pending: PendingConfigurationOperations,
  sessionId: string,
  event: ConfigurationOperationEvent,
): void {
  if (event.status !== 'succeeded' && event.status !== 'failed') return;
  const key = operationKey(sessionId, event.domain, event.operation_id);
  const operation = pending.get(key);
  if (!operation) return;
  if (event.status === 'failed') {
    rejectOperation(pending, key, new Error(event.message ?? `${event.domain} configuration operation failed`));
    return;
  }
  pending.delete(key);
  clearTimeout(operation.timer);
  operation.resolve(event);
}

/** An old connection's requests cannot remain pending into its replacement. */
export function interruptConfigurationOperations(
  pending: PendingConfigurationOperations,
  sessionId?: string,
): void {
  for (const [key, operation] of pending) {
    if (sessionId === undefined || operation.sessionId === sessionId) {
      rejectOperation(pending, key, new Error('configuration operation was interrupted'));
    }
  }
}
