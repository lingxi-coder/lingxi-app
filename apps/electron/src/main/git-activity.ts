import type { ClientEvent } from '@lingxi/bridge-client';

/** Keep the worktree and runtime alive for every task that outlives the main turn. */
export class GitActivityTracker {
  private readonly agents = new Map<string, boolean>();
  private readonly workers = new Map<string, boolean>();
  private readonly tasks = new Set<string>();
  private workerCount = 0;
  get active(): boolean { return this.tasks.size > 0 || this.workerCount > 0 || [...this.agents.values(), ...this.workers.values()].some(Boolean); }

  private taskStatus(taskId: string, status: string): void {
    if (['pending', 'running', 'paused'].includes(status)) this.tasks.add(taskId);
    else if (['completed', 'failed', 'cancelled', 'killed', 'stopped'].includes(status)) this.tasks.delete(taskId);
  }

  /** Ordinary task lists may be filtered or partial; only this RPC replaces ownership. */
  replaceSnapshot(events: readonly ClientEvent[]): void {
    const completions = events.filter((event) => event.type === 'task_list_complete');
    if (completions.length !== 1 || completions[0].type !== 'task_list_complete'
      || completions[0].request_id !== 'desktop-runtime-snapshot' || completions[0].error
      || events.filter((event) => event.type === 'session_agent_list').length !== 1
      || events.filter((event) => event.type === 'coordinator_status').length !== 1) {
      throw new Error('Runtime background snapshot is incomplete.');
    }
    this.reset();
    for (const event of events) this.accept(event);
  }

  accept(event: ClientEvent): void {
    // `unknown` is what the router assigns to a historical subagent row whose
    // transcript carries no recognised `status` — it means "we could not tell",
    // not "still running". Counting it as running pinned the worktree guard and
    // the session's engine runtime for the life of the app, with no way to clear
    // it. Worker labels are open-ended, so everything else still defaults to
    // active.
    const running = (status: string) => !['completed', 'failed', 'killed', 'cancelled', 'idle', 'stopped', 'unknown'].includes(status);
    // `main` is the conversation's own turn, not a worker that outlives it, and
    // this tracker exists only for the ones that DO. The roster reports it as
    // `running` whenever a turn is active, but the engine never emits
    // `session_agent_updated` for `main` — so a roster requested mid-turn (a
    // Runtime Center refresh, activating a running session) would leave the
    // worktree guard latched after the turn ended, refusing every checkout /
    // discard / pull / merge with "An agent is using this worktree" and pinning
    // the runtime against cache eviction until some other event happened to
    // replace the roster. Foreground turns are already tracked separately.
    const tracked = (agentId: string) => agentId !== 'main';
    if (event.type === 'session_agent_list') {
      this.agents.clear();
      for (const agent of event.agents) if (tracked(agent.agent_id)) this.agents.set(agent.agent_id, running(agent.status));
    } else if (event.type === 'session_agent_updated') { if (tracked(event.agent.agent_id)) this.agents.set(event.agent.agent_id, running(event.agent.status)); }
    else if (event.type === 'coordinator_worker') this.workers.set(event.worker.agent_id, running(event.worker.status));
    else if (event.type === 'coordinator_status') {
      this.workerCount = event.active_workers;
      if (!event.active_workers) this.workers.clear();
    } else if (event.type === 'task_row') this.taskStatus(event.task.task_id, event.task.status.type);
    else if (event.type === 'task_status_changed') this.taskStatus(event.task_id, event.status.type);
    else if (event.type === 'workflow_resumed') {
      this.tasks.delete(event.previous_task_id);
      this.taskStatus(event.task.task_id, event.task.status.type);
    } else if (event.type === 'task_lifecycle') {
      // Registry mutation receipts arrive without a foreground turn (including
      // shell/workflow/fusion starts). Status-only updates must not erase other
      // live tasks, and malformed or unrelated SDK receipts carry no ownership.
      let receipt: unknown;
      try { receipt = JSON.parse(event.event_json); } catch { return; }
      if (!receipt || typeof receipt !== 'object' || Array.isArray(receipt)) return;
      const task = receipt as Record<string, unknown>;
      if (task.type !== 'system' || typeof task.task_id !== 'string' || !task.task_id) return;
      if (task.subtype === 'task_started') this.tasks.add(task.task_id);
      else if (task.subtype === 'task_notification' && typeof task.status === 'string') this.taskStatus(task.task_id, task.status);
      else if (task.subtype === 'task_updated' && task.patch && typeof task.patch === 'object' && !Array.isArray(task.patch)) {
        const status = (task.patch as Record<string, unknown>).status;
        if (typeof status === 'string') this.taskStatus(task.task_id, status);
      }
    }
  }
  reset(): void { this.agents.clear(); this.workers.clear(); this.tasks.clear(); this.workerCount = 0; }
}
