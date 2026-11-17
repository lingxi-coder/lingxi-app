import { executionCommand, type ExecutionScope, type TeamTaskView, type TaskUsage } from './execution.js';
import type { TeamsApi } from './index.js';

export interface TaskSummary {
  taskId: string; state: TeamTaskView['state']; createdAt: number; deadline: number;
  activeSpentMs: number; hasReply: boolean; hasArtifact: boolean; usage: TaskUsage;
}
export interface AuthorizedTaskReport {
  schemaVersion: 1; scope: 'own-current-membership'; generatedAt: number;
  tasks: TaskSummary[];
  totals: { tasks: number; missingUsage: number; partialUsage: number; completeUsage: number; unpricedUsage: number; completeCostMicrosByCurrency: Record<string, number> };
}

/** An allowlist projection: no prompts, messages, resource names, host details or credentials. */
function taskSummary(task: TeamTaskView): TaskSummary {
  const usage = task.usage;
  return { taskId: task.taskId, state: task.state, createdAt: task.createdAt, deadline: task.deadline,
    activeSpentMs: task.activeSpentMs, hasReply: !!task.replyId, hasArtifact: !!task.artifactId,
    usage: !usage || usage.status === 'missing' ? { status: 'missing' } : { status: usage.status, inputTokens: usage.inputTokens, outputTokens: usage.outputTokens,
      ...(usage.costMicros !== undefined ? { costMicros: usage.costMicros } : {}), ...(usage.currency ? { currency: usage.currency } : {}) } };
}

/** Uses only listTasks' default viewer scope; never requests the maintainer's conversation scope. */
export async function authorizedTaskReport(api: TeamsApi, scope: ExecutionScope, uid: string, isCurrent: () => boolean, now = Date.now()): Promise<AuthorizedTaskReport> {
  const requireCurrent = () => { if (!isCurrent()) throw new Error('STALE_MEMBERSHIP'); };
  const tasks = new Map<string, TaskSummary>(), cursors = new Set<string>();
  let after: string | undefined;
  do {
    requireCurrent();
    const page = await api.messages<{ tasks: TeamTaskView[]; next: string | null }>(executionCommand(scope, 'listTasks', after ? { after } : {}));
    requireCurrent();
    for (const task of page.tasks) if (task.requester.uid === uid && task.requester.generation === scope.generation) tasks.set(task.taskId, taskSummary(task));
    const next = page.next;
    if (next !== null && (typeof next !== 'string' || !next || cursors.has(next))) throw new Error('TASK_REPORT_CURSOR_LOOP');
    if (next) cursors.add(next);
    after = next ?? undefined;
  } while (after);
  requireCurrent();
  const rows = [...tasks.values()].sort((a, b) => b.createdAt - a.createdAt || a.taskId.localeCompare(b.taskId));
  const totals: AuthorizedTaskReport['totals'] = { tasks: rows.length, missingUsage: 0, partialUsage: 0, completeUsage: 0, unpricedUsage: 0, completeCostMicrosByCurrency: {} };
  const costs = new Map<string, number>();
  for (const { usage } of rows) {
    if (usage.status === 'missing') { totals.missingUsage++; totals.unpricedUsage++; continue; }
    if (usage.status === 'partial') totals.partialUsage++; else totals.completeUsage++;
    if (usage.costMicros === undefined || !Number.isFinite(usage.costMicros) || !usage.currency) totals.unpricedUsage++;
    else if (usage.status === 'complete') costs.set(usage.currency, (costs.get(usage.currency) ?? 0) + usage.costMicros);
  }
  totals.completeCostMicrosByCurrency = Object.fromEntries(costs);
  return { schemaVersion: 1, scope: 'own-current-membership', generatedAt: now, tasks: rows, totals };
}

/** Quote CSV cells and neutralize formula prefixes, including identifiers supplied by the service. */
const csvCell = (value: unknown) => `"${String(value ?? '').replace(/^[\s]*[=+@-]/u, match => `'${match}`).replace(/"/gu, '""')}"`;
export function taskReportCsv(report: AuthorizedTaskReport): string {
  const headers = ['taskId', 'state', 'createdAt', 'deadline', 'activeSpentMs', 'hasReply', 'hasArtifact', 'usageStatus', 'inputTokens', 'outputTokens', 'costMicros', 'currency'];
  const rows = report.tasks.map(task => { const u = task.usage; return [task.taskId, task.state, task.createdAt, task.deadline, task.activeSpentMs, task.hasReply, task.hasArtifact, u.status, u.status === 'missing' ? '' : u.inputTokens, u.status === 'missing' ? '' : u.outputTokens, u.status === 'missing' ? '' : u.costMicros, u.status === 'missing' ? '' : u.currency]; });
  return [headers, ...rows].map(row => row.map(csvCell).join(',')).join('\r\n');
}
