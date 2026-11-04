/**
 * Tasks between the Bots of one computer: who may hand what to whom, how a
 * task runs, and what becomes of it. Free of Electron.
 *
 * The tools of the Bot router call in here, each for the session that holds
 * the token. A task is done by a session of its own in the receiving Bot's
 * workspace (it appears among that Bot's chats), started with a prompt that
 * says where the task came from and that what it says is not to be trusted.
 * The receiving Bot keeps its own permissions: when it needs an answer from its
 * owner, the task is `input-required` and waits for the owner to answer in that
 * Bot's chat. Nothing here answers for a person.
 *
 * A task that the app lost track of (a restart) is `failed`; it is never run
 * again by itself.
 */

import { createHash, randomBytes, randomUUID } from 'node:crypto';

import type { BotTasksUi } from '../../shared/bots.js';
import type { HostTurnOptions, HostTurnResult, RemoteObserver, SessionRef } from '../bridgeTypes.js';
import { followUpPrompt, taskPrompt } from './taskEnvelope.js';
import { FileRefusal, MAX_FILE_BYTES, MAX_TRANSFER_BYTES, MAX_TRANSFER_FILES, transferFiles } from './taskFiles.js';
import {
  DEFAULT_DEADLINE_MINUTES,
  DelegationError,
  engineToolName,
  type AttachedArtifact,
  type BotCard,
  type Caller,
  type DelegateArguments,
  type DelegationBackend,
  type TaskView,
} from './routerTools.js';
import {
  MAX_CHAIN,
  MAX_FOLLOW_UPS,
  MAX_REASON_CHARS,
  MAX_RESULT_CHARS,
  TaskStore,
  isActive,
  isSettled,
  isTaskId,
  type TaskFile,
  type TaskRecord,
} from './tasks.js';
import type { KeepAwake } from './keepAwake.js';
import type { BotRecord, BotStore } from './store.js';
import { tasksUi } from './taskUi.js';

/** At most this many tasks are being done at once by one Bot. */
export const MAX_ACTIVE_PER_BOT = 2;
/** At most this many tasks are handed on by the one task a Bot is doing. */
export const MAX_SUBTASKS = 5;
/** How long a follow-up gives the Bot. */
const FOLLOW_UP_MINUTES = DEFAULT_DEADLINE_MINUTES;
/** How long deleting a Bot waits for the work of its tasks to stop. */
const SETTLE_MS = 15_000;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/g;

/** What of a session's runtime a task needs. */
export interface TaskRuntime {
  readonly pendingInteractions: number;
  observeRemote(observer: RemoteObserver): () => void;
  runHostTurn(prompt: string, options?: HostTurnOptions): Promise<HostTurnResult>;
}

/** What of the runtime manager the tasks use: a session, kept open while the work is done. */
export interface TaskRuntimes {
  withSession<T>(ref: SessionRef, empty: boolean, model: string | undefined, operation: (runtime: TaskRuntime) => Promise<T>): Promise<T>;
}

export interface BotTasksOptions {
  store: BotStore;
  runtimes: TaskRuntimes;
  /** Whether Bots can run on this computer. */
  available(): boolean;
  /** Whether the Bot is being deleted. */
  isDeleting(botId: string): boolean;
  warn?: (message: string) => void;
  /** A task changed: these are the Bots it is between. */
  changed?: (botIds: readonly string[]) => void;
  /** The prompt of a task's turn was handed to the session that does the task. */
  promptSent?: (sessionId: string, text: string) => void;
  /** Keeps the computer awake while a Bot is working on a task, if the owner wants that. */
  keepAwake?: KeepAwake;
  now?: () => number;
  /** Milliseconds in a minute and in a second, and how long deleting a Bot waits for work to stop; shorter in tests. */
  minuteMs?: number;
  secondMs?: number;
  settleMs?: number;
}

interface Running {
  controller: AbortController;
  done: Promise<void>;
}

/** One line, as short as a reason may be. */
function oneLine(value: string, max = MAX_REASON_CHARS): string {
  const flat = value.replace(CONTROL, ' ').replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max - 1).trimEnd()}…` : flat;
}

function boundedText(value: string): string {
  return value.length > MAX_RESULT_CHARS ? `${value.slice(0, MAX_RESULT_CHARS - 20)}\n[cut short]` : value;
}

function iso(milliseconds: number): string {
  return new Date(milliseconds).toISOString();
}

export class BotTasks implements DelegationBackend {
  private readonly tasks = new Map<string, TaskRecord>();
  private readonly running = new Map<string, Running>();
  private readonly waiters = new Map<string, Set<() => void>>();
  /** For each task that is `working`, the way to let the computer sleep again. */
  private readonly awake = new Map<string, () => void>();
  private readonly taskStore: TaskStore;
  private readonly now: () => number;
  private readonly minuteMs: number;
  private readonly secondMs: number;

  constructor(private readonly options: BotTasksOptions) {
    this.taskStore = new TaskStore(options.store);
    this.now = options.now ?? Date.now;
    this.minuteMs = options.minuteMs ?? 60_000;
    this.secondMs = options.secondMs ?? 1_000;
  }

  private warn(message: string): void {
    this.options.warn?.(message);
  }

  /** Read the tasks on disk. What was still going on when the app stopped did not finish. */
  load(): void {
    const { tasks, problems } = this.taskStore.loadAll();
    for (const problem of problems) this.warn(`Bot task not read: ${problem}`);
    for (const task of tasks) {
      this.tasks.set(task.task_id, task);
      if (isActive(task.state)) {
        task.state = 'failed';
        task.reason = 'interrupted by a restart';
        this.touch(task);
      }
    }
  }

  /** Every task there is, newest first. */
  list(): TaskRecord[] {
    return [...this.tasks.values()].map((task) => structuredClone(task)).sort((a, b) => b.created_at - a.created_at);
  }

  /**
   * Fail every task whose time is up. Each task has a timer that does so, but
   * timers stand still while the computer sleeps, so the time left on them is
   * wrong when it wakes. The clock on the wall is not: ask it then.
   */
  checkDeadlines(): void {
    const now = this.now();
    for (const task of [...this.tasks.values()]) {
      if (isActive(task.state) && task.deadline_at <= now) this.expire(task, this.running.get(task.task_id)?.controller);
    }
  }

  // What the owner's window asks

  /** The tasks a Bot was given and the ones it handed out. */
  forOwner(botId: string): BotTasksUi {
    return tasksUi(this.tasks.values(), botId, {
      nameOf: (id) => this.record(id)?.name,
      workspaceOf: (id) => this.options.store.workspaceDir(id),
    });
  }

  /** The owner stops a task of the Bot, whichever Bot sent it. One that has ended stays as it ended. */
  cancelForOwner(botId: string, taskId: string): void {
    const task = isTaskId(taskId) ? this.tasks.get(taskId) : undefined;
    if (task === undefined || (task.sender.bot_id !== botId && task.receiver.bot_id !== botId)) throw new Error('This Bot has no such task.');
    if (!isActive(task.state)) return;
    this.settle(task, 'canceled', 'Canceled by the owner.');
    this.running.get(taskId)?.controller.abort();
  }

  /** The Bots of the task a session is doing at the moment, if it is doing one. */
  doing(sessionId: string): { receiver: string; sender: string } | undefined {
    for (const task of this.tasks.values()) {
      if (task.receiver.session_id === sessionId && isActive(task.state)) {
        return { receiver: this.nameOf(task.receiver.bot_id), sender: this.nameOf(task.sender.bot_id) };
      }
    }
    return undefined;
  }

  // What the router's tools ask

  listBots(caller: Caller): BotCard[] {
    this.assertCaller(caller);
    return this.records()
      .filter((record) => record.bot_id !== caller.botId && !this.options.isDeleting(record.bot_id))
      .sort((a, b) => a.name.localeCompare(b.name) || a.bot_id.localeCompare(b.bot_id))
      .map((record) => ({
        id: record.bot_id,
        name: record.name,
        description: record.description,
        accepts_tasks_from_you: record.accept_from?.includes(caller.botId) === true,
      }));
  }

  async delegate(caller: Caller, args: DelegateArguments): Promise<TaskView> {
    const plan = this.plan(caller, args);
    if (plan.existing !== undefined) return this.view(plan.existing);
    const taskId = randomBytes(16).toString('base64url');
    const created = this.now();
    const task: TaskRecord = {
      v: 1,
      task_id: taskId,
      created_at: created,
      updated_at: created,
      sender: { bot_id: caller.botId, session_id: caller.sessionId },
      receiver: { bot_id: plan.target.bot_id, session_id: randomUUID() },
      chain: plan.chain,
      ...(plan.parent !== undefined ? { parent_task_id: plan.parent.task_id } : {}),
      goal: args.goal,
      inputs: [],
      deadline_at: plan.deadlineAt,
      ...(args.idempotencyKey !== undefined ? { idempotency_key: args.idempotencyKey } : {}),
      request_digest: plan.digest,
      state: 'submitted',
      messages: [],
      artifacts: [],
      turns: 0,
    };
    // Counted from here on, so that a second call cannot slip in while the files are copied.
    this.tasks.set(taskId, task);
    try {
      const copies = await this.transfer(caller.botId, args.inputs, task.receiver.bot_id, taskId);
      task.inputs = copies;
    } catch (error) {
      this.tasks.delete(taskId);
      throw this.refusal(error);
    }
    if (task.state !== 'submitted') return this.view(task);
    this.touch(task);
    this.launch(task, taskPrompt({
      taskId,
      from: this.nameOf(caller.botId),
      goal: args.goal,
      inputs: task.inputs,
      deadline: task.deadline_at,
      attachTool: engineToolName('attach_artifact'),
    }, this.now()), true);
    return this.view(task);
  }

  async getTask(caller: Caller, taskId: string, waitSeconds: number, signal: AbortSignal): Promise<TaskView> {
    const task = this.sentBy(caller, taskId);
    if (waitSeconds > 0 && !isSettled(task.state)) await this.untilSettled(task, waitSeconds * this.secondMs, signal);
    return this.view(task);
  }

  async messageBot(caller: Caller, taskId: string, text: string): Promise<TaskView> {
    this.assertCaller(caller);
    const task = this.sentBy(caller, taskId);
    if (task.state !== 'completed' && task.state !== 'failed') {
      throw new DelegationError('state', task.state === 'working' || task.state === 'submitted'
        ? 'The Bot is still working on this task; wait for it to finish before sending a follow-up.'
        : task.state === 'input-required'
          ? "The Bot is waiting for its owner to answer in the Bot's chat; a follow-up cannot reach it until then."
          : `A task that is ${task.state} cannot be followed up.`);
    }
    if (task.messages.length >= MAX_FOLLOW_UPS) throw new DelegationError('state', `A task takes at most ${MAX_FOLLOW_UPS} follow-ups.`);
    const target = this.requireTarget(task);
    const refused = this.mayStart(task, target);
    if (refused !== undefined) throw refused;
    if (this.activeFor(task.receiver.bot_id) >= MAX_ACTIVE_PER_BOT) throw new DelegationError('busy', `${target.name} is already doing ${MAX_ACTIVE_PER_BOT} tasks; try again when one has finished.`);
    const parent = this.executing(caller);
    let deadlineAt = this.now() + FOLLOW_UP_MINUTES * this.minuteMs;
    if (parent !== undefined) {
      if (parent.deadline_at - this.now() < this.minuteMs) throw new DelegationError('deadline', 'The task you are doing has less than a minute left.');
      deadlineAt = Math.min(deadlineAt, parent.deadline_at);
    }
    task.messages.push({ at: this.now(), text });
    task.state = 'working';
    delete task.reason;
    delete task.result;
    task.deadline_at = deadlineAt;
    task.turns += 1;
    this.touch(task);
    this.launch(task, followUpPrompt({ taskId, from: this.nameOf(caller.botId), deadline: deadlineAt, attachTool: engineToolName('attach_artifact') }, text, this.now()), false);
    return this.view(task);
  }

  async cancelTask(caller: Caller, taskId: string): Promise<TaskView> {
    const task = this.sentBy(caller, taskId);
    if (isActive(task.state)) {
      this.settle(task, 'canceled', `Canceled by ${this.nameOf(caller.botId)}.`);
      this.running.get(taskId)?.controller.abort();
    }
    return this.view(task);
  }

  async attachArtifact(caller: Caller, taskId: string, path: string): Promise<AttachedArtifact> {
    const task = this.tasks.get(taskId);
    if (task === undefined || task.receiver.bot_id !== caller.botId || task.receiver.session_id !== caller.sessionId) {
      throw new DelegationError('not_found', 'This session is not doing a task by that id.');
    }
    if (task.state !== 'working' && task.state !== 'input-required') throw new DelegationError('state', `The task is ${task.state}; files can be attached only while it is being done.`);
    if (task.artifacts.length >= MAX_TRANSFER_FILES) throw new DelegationError('files', `A task takes at most ${MAX_TRANSFER_FILES} files back.`);
    if (this.options.isDeleting(task.sender.bot_id)) throw new DelegationError('unavailable', 'The Bot that sent the task is being deleted.');
    const used = task.artifacts.reduce((sum, file) => sum + file.size, 0);
    let copy: TaskFile;
    try {
      const copies = await this.transfer(caller.botId, [path], task.sender.bot_id, taskId, { maxFiles: 1, maxFileBytes: MAX_FILE_BYTES, maxTotalBytes: Math.max(0, MAX_TRANSFER_BYTES - used) });
      copy = copies[0]!;
    } catch (error) {
      throw this.refusal(error);
    }
    task.artifacts.push(copy);
    this.touch(task);
    return { task_id: taskId, artifact: { name: copy.name, path: copy.path, size: copy.size, sha256: copy.sha256 } };
  }

  // What deleting a Bot asks

  /**
   * A Bot is about to be deleted: the tasks it was given and the tasks it handed
   * out are canceled, and the work on them is stopped, so that nothing is
   * running in the Bot's workspace when it moves.
   */
  async botDeleting(botId: string): Promise<void> {
    const stopping: Array<Promise<void>> = [];
    for (const task of [...this.tasks.values()]) {
      if (!isActive(task.state) || (task.receiver.bot_id !== botId && task.sender.bot_id !== botId)) continue;
      this.settle(task, 'canceled', task.receiver.bot_id === botId ? 'The Bot was deleted.' : 'The Bot that sent the task was deleted.');
      const running = this.running.get(task.task_id);
      if (running !== undefined) {
        running.controller.abort();
        stopping.push(running.done);
      }
    }
    if (stopping.length === 0) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([
      Promise.all(stopping),
      new Promise<void>((resolve) => { timer = setTimeout(resolve, this.options.settleMs ?? SETTLE_MS); }),
    ]);
    clearTimeout(timer);
  }

  // Admission

  private records(): BotRecord[] {
    try {
      return this.options.store.entries().flatMap(({ record }) => (record === undefined ? [] : [record]));
    } catch {
      return [];
    }
  }

  private record(botId: string): BotRecord | undefined {
    try { return this.options.store.get(botId); } catch { return undefined; }
  }

  private nameOf(botId: string): string {
    return this.record(botId)?.name ?? botId;
  }

  private assertCaller(caller: Caller): void {
    if (!this.options.available()) throw new DelegationError('unavailable', 'Bots are available on macOS only.');
    if (this.options.isDeleting(caller.botId) || this.record(caller.botId) === undefined) throw new DelegationError('unavailable', 'Your Bot is no longer there.');
  }

  /** The Bot a task would go to, named by id or by name. */
  private resolve(reference: string): BotRecord {
    const records = this.records();
    const byId = records.find((record) => record.bot_id === reference);
    if (byId !== undefined) return byId;
    const wanted = reference.trim().toLowerCase();
    const named = records.filter((record) => record.name.toLowerCase() === wanted);
    if (named.length === 1) return named[0]!;
    if (named.length > 1) throw new DelegationError('invalid', `More than one Bot is named "${reference}"; use one of these ids: ${named.map((record) => record.bot_id).join(', ')}.`);
    throw new DelegationError('not_found', `No Bot "${reference}". list_bots shows the Bots there are.`);
  }

  private plan(caller: Caller, args: DelegateArguments): { existing?: TaskRecord; target: BotRecord; chain: string[]; parent?: TaskRecord; deadlineAt: number; digest: string } {
    this.assertCaller(caller);
    const target = this.resolve(args.bot);
    const digest = createHash('sha256').update(JSON.stringify([target.bot_id, args.goal, args.inputs, args.deadlineMinutes ?? null])).digest('hex');
    const parent = this.executing(caller);
    const chain = parent !== undefined ? [...parent.chain, caller.botId] : [caller.botId];
    // A retry with its key gets the task it started, whatever has become of it.
    if (args.idempotencyKey !== undefined) {
      const existing = [...this.tasks.values()].find((task) => task.sender.session_id === caller.sessionId && task.idempotency_key === args.idempotencyKey);
      if (existing !== undefined) {
        if (existing.request_digest !== digest) throw new DelegationError('conflict', 'That idempotency_key was used for a different task. Use a new key, or repeat the call as it was.');
        return { existing, target, chain, ...(parent !== undefined ? { parent } : {}), deadlineAt: existing.deadline_at, digest };
      }
    }
    if (target.bot_id === caller.botId) throw new DelegationError('invalid', 'You cannot hand a task to yourself.');
    if (this.options.isDeleting(target.bot_id)) throw new DelegationError('unavailable', `${target.name} is being deleted.`);
    if (target.accept_from?.includes(caller.botId) !== true) {
      throw new DelegationError('not_accepted', `${target.name} does not accept tasks from you. Its owner can allow it in ${target.name}'s settings.`);
    }
    if (chain.includes(target.bot_id)) {
      throw new DelegationError('cycle', `${target.name} is already part of this chain of tasks (${chain.map((id) => this.nameOf(id)).join(' to ')}); handing it a task would go round in a circle.`);
    }
    if (chain.length > MAX_CHAIN) throw new DelegationError('depth', `Tasks may be handed on ${MAX_CHAIN} times at most; this one already has been.`);
    if (this.activeFor(target.bot_id) >= MAX_ACTIVE_PER_BOT) throw new DelegationError('busy', `${target.name} is already doing ${MAX_ACTIVE_PER_BOT} tasks; try again when one has finished.`);
    if (parent !== undefined && this.subtasksOf(parent) >= MAX_SUBTASKS) throw new DelegationError('too_many_subtasks', `The task you are doing may hand on ${MAX_SUBTASKS} tasks at most.`);
    let deadlineAt = this.now() + (args.deadlineMinutes ?? DEFAULT_DEADLINE_MINUTES) * this.minuteMs;
    if (parent !== undefined) {
      if (parent.deadline_at - this.now() < this.minuteMs) throw new DelegationError('deadline', 'The task you are doing has less than a minute left.');
      deadlineAt = Math.min(deadlineAt, parent.deadline_at);
    }
    return { target, chain, ...(parent !== undefined ? { parent } : {}), deadlineAt, digest };
  }

  /** Why a task whose Bot has been asked to start it may no longer be started, if it may not. */
  private mayStart(task: TaskRecord, target: BotRecord | undefined): DelegationError | undefined {
    const { isDeleting, available } = this.options;
    if (!available()) return new DelegationError('unavailable', 'Bots are available on macOS only.');
    if (target === undefined) return new DelegationError('unavailable', 'The Bot is no longer there.');
    if (isDeleting(target.bot_id) || isDeleting(task.sender.bot_id)) return new DelegationError('unavailable', `${target.name} is being deleted.`);
    if (this.record(task.sender.bot_id) === undefined) return new DelegationError('unavailable', 'The Bot that sent the task is no longer there.');
    if (target.accept_from?.includes(task.sender.bot_id) !== true) return new DelegationError('not_accepted', `${target.name} no longer accepts tasks from the Bot that sent this one.`);
    return undefined;
  }

  private requireTarget(task: TaskRecord): BotRecord {
    const target = this.record(task.receiver.bot_id);
    if (target === undefined) throw new DelegationError('unavailable', 'The Bot is no longer there.');
    return target;
  }

  private activeFor(botId: string): number {
    let count = 0;
    for (const task of this.tasks.values()) if (task.receiver.bot_id === botId && isActive(task.state)) count += 1;
    return count;
  }

  private subtasksOf(parent: TaskRecord): number {
    let count = 0;
    for (const task of this.tasks.values()) if (task.parent_task_id === parent.task_id) count += 1;
    return count;
  }

  /** The task a session is doing at the moment, if it is doing one. */
  private executing(caller: Caller): TaskRecord | undefined {
    for (const task of this.tasks.values()) {
      if (task.receiver.session_id === caller.sessionId && task.receiver.bot_id === caller.botId && isActive(task.state)) return task;
    }
    return undefined;
  }

  /** A task the caller's Bot handed out; for anyone else, there is no such task. */
  private sentBy(caller: Caller, taskId: string): TaskRecord {
    const task = isTaskId(taskId) ? this.tasks.get(taskId) : undefined;
    if (task === undefined || task.sender.bot_id !== caller.botId) throw new DelegationError('not_found', 'You have not handed over a task by that id.');
    return task;
  }

  private async transfer(
    fromBot: string,
    paths: readonly string[],
    toBot: string,
    taskId: string,
    limits?: Parameters<typeof transferFiles>[4],
  ): Promise<TaskFile[]> {
    const { store } = this.options;
    const copies = await transferFiles(store.workspaceDir(fromBot), paths, store.workspaceDir(toBot), taskId, limits);
    return copies.map(({ name, path, size, sha256 }) => ({ name, path, size, sha256 }));
  }

  /** What a failure of the files means to the Bot that asked. */
  private refusal(error: unknown): unknown {
    if (error instanceof DelegationError) return error;
    if (error instanceof FileRefusal) return new DelegationError('files', error.message);
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return new DelegationError('unavailable', 'A workspace is no longer there.');
    return error;
  }

  // Doing the work

  private launch(task: TaskRecord, prompt: string, fresh: boolean): void {
    const controller = new AbortController();
    const done = this.execute(task, prompt, fresh, controller).catch((error: unknown) => {
      this.warn(`Bot task ${task.task_id}: ${error instanceof Error ? error.message : String(error)}`);
    });
    this.running.set(task.task_id, { controller, done });
    void done.finally(() => {
      if (this.running.get(task.task_id)?.controller === controller) this.running.delete(task.task_id);
    });
  }

  private async execute(task: TaskRecord, prompt: string, fresh: boolean, controller: AbortController): Promise<void> {
    const timer = setTimeout(() => this.expire(task, controller), Math.max(0, task.deadline_at - this.now()));
    timer.unref?.();
    try {
      const target = this.record(task.receiver.bot_id);
      const refused = this.mayStart(task, target);
      if (refused !== undefined) {
        this.settle(task, 'rejected', refused.message);
        return;
      }
      const ref: SessionRef = { projectPath: this.options.store.workspaceDir(task.receiver.bot_id), sessionId: task.receiver.session_id };
      const result = await this.options.runtimes.withSession(ref, fresh, target?.model, async (runtime) => {
        if (controller.signal.aborted || !isActive(task.state)) return undefined;
        const watch = runtime.observeRemote((observation) => {
          if (observation.type === 'pending') this.interaction(task, runtime);
        });
        try {
          if (task.state === 'submitted') {
            task.state = 'working';
            task.turns += 1;
            this.touch(task);
          }
          return await runtime.runHostTurn(prompt, {
            signal: controller.signal,
            // The prompt shows as the sending Bot's, not as a scheduled run's.
            source: { origin: 'bot', fromBotName: this.nameOf(task.sender.bot_id) },
            onSent: () => this.options.promptSent?.(task.receiver.session_id, prompt),
          });
        } finally {
          watch();
        }
      });
      if (result !== undefined) this.conclude(task, result);
    } catch (error) {
      this.settle(task, 'failed', `The Bot's session could not do the task: ${oneLine(error instanceof Error ? error.message : String(error), 200)}`);
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * The time of a task is up: it fails, and the turn that was doing it is
   * interrupted. A turn that was interrupted before has nothing more to answer
   * for, even if the task has since been given another one.
   */
  private expire(task: TaskRecord, controller: AbortController | undefined): void {
    if (!isActive(task.state) || controller?.signal.aborted) return;
    this.settle(task, 'failed', 'deadline');
    controller?.abort();
  }

  /** The Bot is waiting for an answer from its owner, or has been given one. */
  private interaction(task: TaskRecord, runtime: TaskRuntime): void {
    const waiting = runtime.pendingInteractions > 0;
    if (waiting && task.state === 'working') {
      task.state = 'input-required';
      task.reason = `Waiting for the owner of ${this.nameOf(task.receiver.bot_id)} to answer in the Bot's chat.`;
      this.touch(task);
    } else if (!waiting && task.state === 'input-required') {
      task.state = 'working';
      delete task.reason;
      this.touch(task);
    }
  }

  /** The turn ended. A task that was canceled or ran out of time meanwhile keeps that; only what the turn cost is added. */
  private conclude(task: TaskRecord, result: HostTurnResult): void {
    if (result.costUsd !== undefined) task.cost_usd = Math.round(((task.cost_usd ?? 0) + result.costUsd) * 1e6) / 1e6;
    if (!isActive(task.state)) {
      this.touch(task);
      return;
    }
    if (result.status === 'completed') this.settle(task, 'completed', undefined, result.text);
    else if (result.status === 'failed') this.settle(task, 'failed', oneLine(result.error ?? 'The Bot could not finish.'), result.text);
    else this.settle(task, 'canceled', "Stopped in the Bot's chat.", result.text);
  }

  /** Give an active task its end. Anything that has ended already stays as it ended. */
  private settle(task: TaskRecord, state: 'completed' | 'failed' | 'canceled' | 'rejected', reason?: string, result?: string): void {
    if (!isActive(task.state)) return;
    task.state = state;
    if (reason !== undefined && reason !== '') task.reason = oneLine(reason);
    else delete task.reason;
    if (result !== undefined && result.trim() !== '') task.result = boundedText(result);
    this.touch(task);
  }

  /** Record a change: stamp, save, say so to the window, and wake whoever waits for the task. */
  private touch(task: TaskRecord): void {
    task.updated_at = Math.max(this.now(), task.updated_at);
    this.keepAwakeWhileWorking(task);
    try {
      this.taskStore.save(task);
    } catch (error) {
      this.warn(`Bot task ${task.task_id} was not saved: ${error instanceof Error ? error.message : String(error)}`);
    }
    try {
      this.options.changed?.([task.sender.bot_id, task.receiver.bot_id]);
    } catch (error) {
      this.warn(`Bot task ${task.task_id}: the change was not announced: ${error instanceof Error ? error.message : String(error)}`);
    }
    for (const check of [...(this.waiters.get(task.task_id) ?? [])]) check();
  }

  /**
   * The computer stays awake for as long as the Bot is working on the task. It
   * may sleep while the Bot waits for its owner, which can take hours, and once
   * the task has ended.
   */
  private keepAwakeWhileWorking(task: TaskRecord): void {
    const release = this.awake.get(task.task_id);
    if (task.state === 'working' && release === undefined) {
      const hold = this.options.keepAwake?.hold();
      if (hold !== undefined) this.awake.set(task.task_id, hold);
    } else if (task.state !== 'working' && release !== undefined) {
      this.awake.delete(task.task_id);
      release();
    }
  }

  /** Wait until the task needs the caller's attention, the time is up, or the call is dropped. */
  private untilSettled(task: TaskRecord, milliseconds: number, signal: AbortSignal): Promise<void> {
    return new Promise((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const waiters = this.waiters.get(task.task_id) ?? new Set<() => void>();
      this.waiters.set(task.task_id, waiters);
      let finished = false;
      const done = (): void => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        signal.removeEventListener('abort', done);
        waiters.delete(check);
        if (waiters.size === 0) this.waiters.delete(task.task_id);
        resolve();
      };
      const check = (): void => { if (isSettled(task.state)) done(); };
      waiters.add(check);
      timer = setTimeout(done, milliseconds);
      timer.unref?.();
      signal.addEventListener('abort', done, { once: true });
      if (signal.aborted) done();
    });
  }

  private view(task: TaskRecord): TaskView {
    return {
      task_id: task.task_id,
      bot: task.receiver.bot_id,
      bot_name: this.nameOf(task.receiver.bot_id),
      status: { state: task.state, ...(task.reason !== undefined ? { message: task.reason } : {}), timestamp: iso(task.updated_at) },
      deadline: iso(task.deadline_at),
      ...(task.result !== undefined ? { result: task.result } : {}),
      artifacts: task.artifacts.map(({ name, path, size, sha256 }) => ({ name, path, size, sha256 })),
      ...(task.cost_usd !== undefined ? { cost_usd: task.cost_usd } : {}),
      follow_ups: task.messages.length,
    };
  }
}
