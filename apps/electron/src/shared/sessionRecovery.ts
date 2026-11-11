/** Local recovery counters only: never transcript text, paths, credentials or tool inputs. */
export const CH_SESSION_RECOVERY = 'lingxi:sessionRecovery';
const phases = [
  'preload_subscribe', 'preload_request', 'preload_retry', 'preload_received', 'preload_failed', 'preload_dispose',
  'renderer_base', 'renderer_connection', 'renderer_bootstrap', 'renderer_commit', 'stage_commit',
] as const;
export interface SessionRecoveryDiagnostic {
  phase: typeof phases[number];
  documentId?: string;
  sessionId?: string;
  sessionKey?: string;
  eventType?: 'session_started' | 'session_resumed' | 'session_ended';
  connection?: 'idle' | 'spawning' | 'connecting' | 'connected' | 'restarting' | 'disconnected' | 'error';
  subscriptionId?: number;
  attempt?: number;
  durationMs?: number;
  sequence?: number;
  eventCount?: number;
  messageCount?: number;
  itemCount?: number;
  previousItemCount?: number;
  rowCount?: number;
  revision?: number;
  runtimeCount?: number;
  loading?: boolean;
}
const numbers = new Set(['subscriptionId', 'attempt', 'durationMs', 'sequence', 'eventCount', 'messageCount',
  'itemCount', 'previousItemCount', 'rowCount', 'revision', 'runtimeCount']);
const identities = new Set(['documentId', 'sessionId', 'sessionKey']);
const connections = new Set(['idle', 'spawning', 'connecting', 'connected', 'restarting', 'disconnected', 'error']);

/** Reject arbitrary renderer strings rather than relying on later redaction. */
export function parseSessionRecoveryDiagnostic(input: unknown): SessionRecoveryDiagnostic | undefined {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return;
  const value = input as Record<string, unknown>;
  if (!phases.includes(value.phase as SessionRecoveryDiagnostic['phase'])) return;
  for (const [key, field] of Object.entries(value)) {
    if (key === 'phase') continue;
    if (numbers.has(key) && Number.isSafeInteger(field) && Number(field) >= 0) continue;
    if (identities.has(key) && typeof field === 'string' && /^[a-zA-Z0-9_:-]{0,160}$/.test(field)) continue;
    if (key === 'connection' && connections.has(field as string)) continue;
    if (key === 'eventType' && ['session_started', 'session_resumed', 'session_ended'].includes(field as string)) continue;
    if (key === 'loading' && typeof field === 'boolean') continue;
    return;
  }
  return { ...value } as unknown as SessionRecoveryDiagnostic;
}
