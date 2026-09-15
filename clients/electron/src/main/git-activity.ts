import type { ClientEvent } from '@lingxi/bridge-client';

/** Keep Git's worktree guard aware of workers that outlive the main turn. */
export class GitActivityTracker {
  private readonly agents = new Map<string, boolean>();
  private readonly workers = new Map<string, boolean>();
  private workerCount = 0;
  get active(): boolean { return this.workerCount > 0 || [...this.agents.values(), ...this.workers.values()].some(Boolean); }
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
    }
  }
  reset(): void { this.agents.clear(); this.workers.clear(); this.workerCount = 0; }
}
