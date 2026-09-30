/**
 * The word a lifecycle status is SHOWN as.
 *
 * Ported from claude-code's task rows: it keeps the machine status (`running`,
 * `pending`, `completed`, `failed`, `killed`, `cancelled`) and renders a
 * different word for some of them, per task type —
 *
 *  - a background agent: `completed` → `done`, everything else verbatim
 *  - a background shell: `completed` → `done`, `failed` → `error`,
 *    `killed`/`cancelled` → `stopped`
 *
 * …plus a `, unread` suffix on a completion the model has not been told about.
 *
 * Only the LABEL changes. Counters and colour lookups match on the machine
 * status, so handing them a label reports "0 done" for a panel of finished work.
 * That separation is also what let the engine drop the port's invented `idle`
 * status for a parked background agent (claude-code has no such status: it
 * renders a finished one as `done` whether or not it can be resumed, and uses
 * `idle` for the footer group those rows collapse into and for a teammate's own
 * state, which still arrives through `coordinator_worker`).
 */
export type AgentStatusKind = 'agent' | 'shell';

export function statusLabel(status: string, kind: AgentStatusKind = 'agent'): string {
  if (status === 'completed') return 'done';
  if (kind === 'shell' && status === 'failed') return 'error';
  if (kind === 'shell' && (status === 'cancelled' || status === 'killed')) return 'stopped';
  return status;
}
