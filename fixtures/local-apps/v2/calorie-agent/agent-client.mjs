export const terminalStates = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);
const pendingKey = 'meal-notes.agent.pending';
const runKey = 'meal-notes.agent.run';

export function revisionStorage(storage, revision) {
  if (!Number.isSafeInteger(revision) || revision < 1) throw new Error('应用运行版本不可用，请重新打开应用。');
  const key = (name) => `revision-${revision}:${name}`;
  return {
    getItem: (name) => storage.getItem(key(name)),
    setItem: (name, value) => storage.setItem(key(name), value),
    removeItem: (name) => storage.removeItem(key(name)),
  };
}

/** Reconnect to a host-owned task without resubmitting a new request. */
export class MealAgentClient {
  constructor(agent, storage, onEvent) {
    this.agent = agent;
    this.storage = storage;
    this.onEvent = onEvent;
    this.runId = null;
    this.state = null;
    this.lastSequence = -1;
    this.unsubscribe = null;
    this.submitting = false;
  }

  get pending() {
    const saved = this.storage.getItem(pendingKey);
    return saved ? JSON.parse(saved) : null;
  }

  async submit(input) {
    if (this.submitting) throw new Error('请求正在提交，请稍候。');
    this.submitting = true;
    try {
      // If the first IPC response was lost, preserve its exact request ID and
      // payload. The host's durable idempotency ledger prevents a second run.
      const request = this.pending ?? input;
      this.storage.setItem(pendingKey, JSON.stringify(request));
      const { runId } = await this.agent.run(request);
      this.storage.setItem(runKey, runId);
      this.storage.removeItem(pendingKey);
      await this.watch(runId);
      return runId;
    } finally {
      this.submitting = false;
    }
  }

  async resume() {
    // A rejected request may never have been admitted. Reload must not turn it
    // into a new model call after settings change. Let the user retry/discard.
    if (this.pending) return null;
    const runId = this.storage.getItem(runKey);
    if (runId) await this.watch(runId);
    return runId;
  }

  async watch(runId) {
    this.dispose();
    this.runId = runId;
    this.state = null;
    this.lastSequence = -1;
    const accept = (event) => {
      if (event.runId !== this.runId) return;
      if (event.type === 'connection') { this.onEvent(event); return; }
      if (event.sequence <= this.lastSequence) return;
      this.lastSequence = event.sequence;
      if (event.type === 'state') this.state = event.run.state;
      this.onEvent(event);
    };
    this.unsubscribe = this.agent.subscribe(runId, accept);
    const run = await this.agent.get(runId);
    accept({ type: 'state', runId, sequence: run.sequence, time: run.updatedAt, run });
  }

  async cancel() {
    // Cancellation must never admit an as-yet unconfirmed request just to get
    // its run ID. The user can explicitly retry, then cancel an accepted run.
    if (this.pending) throw new Error('请求尚未确认受理；请先重试同一请求，或明确放弃此请求。');
    if (this.runId) await this.agent.cancel(this.runId);
  }

  forgetCompleted() {
    if (this.submitting || this.pending || (this.runId && !terminalStates.has(this.state))) {
      throw new Error('请先等待当前任务结束，或处理未确认的请求，再更换照片。');
    }
    // A new photo must not restore the previous photo's result after refresh.
    this.storage.removeItem(runKey);
    this.dispose();
    this.runId = null;
    this.state = null;
    this.lastSequence = -1;
  }

  discardPending(confirmed = false) {
    if (!confirmed) throw new Error('请先确认放弃未确认的请求。');
    if (this.submitting) throw new Error('请求仍在提交，请等待返回后再放弃。');
    if (!this.pending) return false;
    // This clears only the local recovery payload. It makes no claim that an
    // uncertain host task was cancelled and never calls run/cancel implicitly.
    this.storage.removeItem(pendingKey);
    this.storage.removeItem(runKey);
    this.dispose();
    this.runId = null;
    this.state = null;
    this.lastSequence = -1;
    return true;
  }

  dispose() {
    this.unsubscribe?.();
    this.unsubscribe = null;
  }
}
