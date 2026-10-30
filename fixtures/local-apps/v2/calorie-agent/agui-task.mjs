export const terminalStates = new Set(['succeeded', 'failed', 'cancelled', 'interrupted']);

const pendingKey = 'meal-notes.agui-v1.pending';
const runKey = 'meal-notes.agui-v1.run';

export function revisionStorage(storage, revision) {
  if (!Number.isSafeInteger(revision) || revision < 1) throw new Error('应用运行版本不可用，请重新打开应用。');
  const key = (name) => `revision-${revision}:${name}`;
  return {
    getItem: (name) => storage.getItem(key(name)),
    setItem: (name, value) => storage.setItem(key(name), value),
    removeItem: (name) => storage.removeItem(key(name)),
  };
}

function object(value) { return value !== null && typeof value === 'object' && !Array.isArray(value); }

function requireRunInput(value) {
  if (!object(value) || typeof value.threadId !== 'string' || !value.threadId
      || typeof value.runId !== 'string' || !value.runId || !Array.isArray(value.messages)
      || value.messages.length !== 1 || value.messages[0]?.role !== 'user') {
    throw new Error('AG-UI 请求记录无效；请放弃后重新选择照片。');
  }
  return value;
}

function requireTask(task, runId, threadId) {
  if (!object(task) || task.runId !== runId || task.threadId !== threadId
      || (!terminalStates.has(task.state) && !['queued', 'running'].includes(task.state))) {
    throw new Error('宿主返回了不匹配的 AG-UI Task。');
  }
  return task;
}

/** Local UI helper over official RunAgentInput and host-owned Task resources. */
export class MealAgentTask {
  constructor(agent, storage, { onEvent = () => {}, onTask = () => {}, onError = () => {} } = {}) {
    this.agent = agent;
    this.storage = storage;
    this.onEvent = onEvent;
    this.onTask = onTask;
    this.onError = onError;
    this.currentTask = null;
    this.runId = null;
    this.threadId = null;
    this.generation = 0;
    this.timer = undefined;
    this.timerResolve = undefined;
  }

  get pendingInput() {
    const value = this.storage.getItem(pendingKey);
    return value === null ? null : requireRunInput(JSON.parse(value));
  }

  get savedRun() {
    const value = this.storage.getItem(runKey);
    if (value === null) return null;
    const record = JSON.parse(value);
    if (!object(record) || typeof record.runId !== 'string' || !record.runId
        || typeof record.threadId !== 'string' || !record.threadId) {
      throw new Error('AG-UI Task 恢复记录无效；请保留餐食记录并重新打开应用。');
    }
    return record;
  }

  async createInput(file, prompt, outputSchema, options = {}) {
    if (!(file instanceof File)) throw new TypeError('请选择一张照片。');
    if (typeof prompt !== 'string' || !prompt.trim()) throw new TypeError('识别说明不能为空。');
    const part = await this.agent.file(file);
    return {
      threadId: crypto.randomUUID(),
      runId: crypto.randomUUID(),
      messages: [{
        id: crypto.randomUUID(),
        role: 'user',
        content: [{ type: 'text', text: prompt }, part],
      }],
      forwardedProps: {
        'com.lingxi.localApp': {
          version: 1,
          outputSchema,
          tools: [],
          maxTurns: options.maxTurns ?? 4,
          timeoutMs: options.timeoutMs ?? 120000,
        },
      },
    };
  }

  async submit(input) {
    if (this.currentTask && !terminalStates.has(this.currentTask.state)) throw new Error('当前识别任务仍在运行。');
    // A retry reuses the complete standard request, including its public IDs.
    const request = this.pendingInput ?? requireRunInput(input);
    this.storage.setItem(pendingKey, JSON.stringify(request));
    const accepted = await this.agent.tasks.start(request);
    if (!object(accepted) || accepted.runId !== request.runId) throw new Error('宿主没有保留 AG-UI runId。');
    const saved = { runId: request.runId, threadId: request.threadId };
    this.storage.setItem(runKey, JSON.stringify(saved));
    this.storage.removeItem(pendingKey);
    return this.connect(saved.runId, saved.threadId);
  }

  async resume() {
    if (this.pendingInput) return null;
    const saved = this.savedRun;
    if (!saved) return null;
    const task = requireTask(await this.agent.tasks.get(saved.runId), saved.runId, saved.threadId);
    return this.connect(saved.runId, saved.threadId, task);
  }

  async connect(runId, threadId, knownTask) {
    this.stopWatching();
    const generation = this.generation;
    this.runId = runId;
    this.threadId = threadId;
    try {
      const task = requireTask(knownTask ?? await this.agent.tasks.get(runId), runId, threadId);
      if (generation !== this.generation) return null;
      this.currentTask = task;
      this.onTask(task);

      let monitor = true;
      const monitorPromise = this.monitorTask(generation, () => monitor);
      try {
        // A new official client connects read-only to the recorded run. It
        // assembles RunAgentResult from standard BaseEvents; it never starts a
        // second model request on reload.
        const client = this.agent.createClient({ threadId });
        let terminalEvent = null;
        let terminalEventError = null;
        const result = await client.connectAgent({ runId }, {
          onEvent: ({ event }) => {
            if (generation !== this.generation) return;
            this.onEvent(event);
            if (event.type === 'RUN_ERROR' || event.type === 'RUN_FINISHED') {
              if (terminalEvent) {
                terminalEventError ??= new Error('AG-UI 连接返回了多个终止事件。');
                terminalEventError.code = 'multiple_terminal_events';
                monitor = false;
                this.currentTask = { ...(this.currentTask ?? {}), runId, threadId, state: 'failed' };
                this.onTask(this.currentTask);
                return;
              }
              terminalEvent = event;
              monitor = false;
              const hasOutcome = Object.hasOwn(event, 'outcome');
              const outcome = event.outcome?.type;
              const state = event.type === 'RUN_ERROR' ? 'failed'
                : outcome === 'cancelled' ? 'cancelled'
                  : outcome === 'interrupt' ? 'interrupted'
                    : !hasOutcome || outcome === 'success' ? 'succeeded' : 'failed';
              this.currentTask = { ...(this.currentTask ?? {}), runId, threadId, state };
              this.onTask(this.currentTask);
            }
          },
        });
        if (terminalEventError) throw terminalEventError;
        if (!terminalEvent) {
          monitor = false;
          this.currentTask = { ...(this.currentTask ?? {}), runId, threadId, state: 'failed' };
          this.onTask(this.currentTask);
          const error = new Error('AG-UI 连接没有返回终止事件。');
          error.code = 'missing_terminal_event';
          throw error;
        }
        if (terminalEvent.type === 'RUN_ERROR') {
          const error = new Error(terminalEvent.message || 'AG-UI 任务失败。');
          error.code = terminalEvent.code || 'agent_run_error';
          error.event = terminalEvent;
          throw error;
        }
        if (terminalEvent.outcome?.type === 'cancelled') {
          const error = new Error('AG-UI 任务已取消。');
          error.code = 'run_cancelled';
          throw error;
        }
        if (terminalEvent.outcome?.type === 'interrupt') {
          const error = new Error('AG-UI 任务正在等待外部输入，当前结果未完成。');
          error.code = 'run_interrupted';
          throw error;
        }
        if (Object.hasOwn(terminalEvent, 'outcome') && terminalEvent.outcome?.type !== 'success') {
          const error = new Error('AG-UI 任务没有以成功结果结束。');
          error.code = 'invalid_terminal_outcome';
          throw error;
        }
        if (!result || !Array.isArray(result.newMessages)) throw new Error('AG-UI reconnect returned an invalid RunAgentResult.');
        return result;
      } finally {
        monitor = false;
        this.stopWatching(false);
        await monitorPromise;
      }
    } catch (error) {
      if (generation === this.generation) this.onError(error);
      throw error;
    }
  }

  async monitorTask(generation, active) {
    while (generation === this.generation && active()) {
      await new Promise((resolve) => {
        this.timerResolve = resolve;
        this.timer = setTimeout(() => {
          this.timer = undefined;
          this.timerResolve = undefined;
          resolve();
        }, 500);
      });
      if (generation !== this.generation || !active()) return;
      const task = requireTask(await this.agent.tasks.get(this.runId), this.runId, this.threadId);
      if (generation !== this.generation || !active()) return;
      this.currentTask = task;
      this.onTask(task);
      if (terminalStates.has(task.state)) return;
    }
  }

  async cancel() {
    if (this.pendingInput) throw new Error('请求尚未确认受理，请先重试同一请求或明确放弃。');
    if (!this.runId || !this.currentTask || terminalStates.has(this.currentTask.state)) return;
    await this.agent.tasks.cancel(this.runId);
  }

  forgetCompleted() {
    if (this.pendingInput || (this.currentTask && !terminalStates.has(this.currentTask.state))) {
      throw new Error('请等待识别结束，或先处理未确认的请求。');
    }
    this.stopWatching();
    this.storage.removeItem(runKey);
    this.currentTask = null;
    this.runId = null;
    this.threadId = null;
  }

  discardPending(confirmed = false) {
    if (!confirmed) throw new Error('请先确认放弃未确认的请求。');
    if (!this.pendingInput) return false;
    this.storage.removeItem(pendingKey);
    this.storage.removeItem(runKey);
    this.stopWatching();
    this.currentTask = null;
    this.runId = null;
    this.threadId = null;
    return true;
  }

  stopWatching(invalidate = true) {
    if (invalidate) this.generation += 1;
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = undefined;
    const resolve = this.timerResolve;
    this.timerResolve = undefined;
    resolve?.();
  }

  dispose() { this.stopWatching(); }
}
