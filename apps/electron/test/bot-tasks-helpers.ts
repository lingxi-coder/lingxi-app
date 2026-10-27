import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

import type { HostTurnOptions, HostTurnResult, RemoteObserver, SessionRef } from '../src/main/bridgeTypes.js';
import { BotTasks, type TaskRuntime, type TaskRuntimes } from '../src/main/bots/taskService.js';
import type { BotRecord } from '../src/main/bots/store.js';
import type { Caller, DelegateArguments } from '../src/main/bots/routerTools.js';
import type { TaskRecord } from '../src/main/bots/tasks.js';
import { botFixture } from './bot-helpers.js';

export interface FakeTurn {
  prompt: string;
  options: HostTurnOptions | undefined;
  finish(result: HostTurnResult): void;
  fail(error: Error): void;
}

/** A session of a Bot as the tasks see it: a turn lasts until the test says how it ends, or until it is cancelled. */
export class FakeSession implements TaskRuntime {
  pendingInteractions = 0;
  readonly turns: FakeTurn[] = [];
  /** A turn that does not stop when it is cancelled, until the test ends it. */
  ignoreAbort = false;
  private readonly observers = new Set<RemoteObserver>();

  observeRemote(observer: RemoteObserver): () => void {
    this.observers.add(observer);
    return () => { this.observers.delete(observer); };
  }

  runHostTurn(prompt: string, options?: HostTurnOptions): Promise<HostTurnResult> {
    return new Promise((resolve, reject) => {
      this.turns.push({ prompt, options, finish: resolve, fail: reject });
      options?.signal?.addEventListener('abort', () => { if (!this.ignoreAbort) resolve({ status: 'cancelled', text: '' }); }, { once: true });
    });
  }

  /** Someone's permission or question is now waiting in the chat, or has been answered. */
  waiting(count: number): void {
    this.pendingInteractions = count;
    for (const observer of [...this.observers]) observer({ type: 'pending' });
  }

  get last(): FakeTurn {
    const turn = this.turns[this.turns.length - 1];
    if (turn === undefined) throw new Error('no turn has started');
    return turn;
  }
}

export class FakeRuntimes implements TaskRuntimes {
  readonly sessions = new Map<string, FakeSession>();
  readonly opened: Array<{ ref: SessionRef; empty: boolean; model: string | undefined }> = [];
  failWith: Error | undefined;
  ignoreAbort = false;

  async withSession<T>(ref: SessionRef, empty: boolean, model: string | undefined, operation: (runtime: TaskRuntime) => Promise<T>): Promise<T> {
    this.opened.push({ ref, empty, model });
    if (this.failWith !== undefined) throw this.failWith;
    let session = this.sessions.get(ref.sessionId);
    if (session === undefined) {
      session = new FakeSession();
      session.ignoreAbort = this.ignoreAbort;
      this.sessions.set(ref.sessionId, session);
    }
    return operation(session);
  }
}

export const sessionId = (n: number): string => `${String(n).padStart(8, '0')}-2222-4333-8444-555555555555`;

export interface SetupOptions {
  minuteMs?: number;
  secondMs?: number;
  settleMs?: number;
  /** A clock the test moves, instead of the real one. */
  clock?: { time: number };
}

/** Bots on a computer, and the tasks between them with their sessions faked. */
export function setupTasks(t: { after(fn: () => void): void }, names: string[], options: SetupOptions = {}) {
  const f = botFixture(t);
  const runtimes = new FakeRuntimes();
  const warnings: string[] = [];
  const deleting = new Set<string>();
  const state = { available: true };
  const make = (): BotTasks => new BotTasks({
    store: f.store,
    runtimes,
    available: () => state.available,
    isDeleting: (botId) => deleting.has(botId),
    warn: (message) => warnings.push(message),
    ...(options.clock !== undefined ? { now: () => options.clock!.time } : {}),
    ...(options.minuteMs !== undefined ? { minuteMs: options.minuteMs } : {}),
    ...(options.secondMs !== undefined ? { secondMs: options.secondMs } : {}),
    ...(options.settleMs !== undefined ? { settleMs: options.settleMs } : {}),
  });
  const tasks = make();
  const bots: Record<string, BotRecord> = {};
  for (const name of names) bots[name] = f.store.create({ name, description: `${name} does things` });
  let counter = 100;
  return {
    ...f, runtimes, warnings, deleting, state, tasks, bots, make,
    /** A session that is not doing a task: a chat of the Bot's owner, or a routine. */
    chat(bot: BotRecord): Caller { return { botId: bot.bot_id, sessionId: sessionId(++counter) }; },
    /** The session that is doing a task, which speaks for its Bot. */
    doing(taskId: string): Caller {
      const task = tasks.list().find((entry) => entry.task_id === taskId)!;
      return { botId: task.receiver.bot_id, sessionId: task.receiver.session_id };
    },
    /** `target` accepts tasks from `senders`, in addition to those it accepted. */
    allow(target: BotRecord, ...senders: BotRecord[]): void {
      const current = f.store.get(target.bot_id);
      f.store.update(target.bot_id, current.revision, { acceptFrom: [...(current.accept_from ?? []), ...senders.map((sender) => sender.bot_id)] });
    },
    record(taskId: string): TaskRecord { return tasks.list().find((entry) => entry.task_id === taskId)!; },
    sessionOf(taskId: string): FakeSession { return runtimes.sessions.get(tasks.list().find((entry) => entry.task_id === taskId)!.receiver.session_id)!; },
    file(bot: BotRecord, path: string, contents: string | Buffer): string {
      const absolute = join(f.store.workspaceDir(bot.bot_id), path);
      mkdirSync(dirname(absolute), { recursive: true });
      writeFileSync(absolute, contents);
      return absolute;
    },
  };
}

export function delegation(bot: BotRecord | string, goal = 'Do the thing', extra: Partial<DelegateArguments> = {}): DelegateArguments {
  return { bot: typeof bot === 'string' ? bot : bot.bot_id, goal, inputs: [], ...extra };
}

/** Wait, without a fixed delay, for something that happens in the background. */
export async function eventually(check: () => boolean, what: string, milliseconds = 3000): Promise<void> {
  const until = Date.now() + milliseconds;
  while (!check()) {
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}
