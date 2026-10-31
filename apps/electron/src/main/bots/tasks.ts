/**
 * Tasks that one Bot hands another, on disk. Free of Electron.
 *
 * A task is one JSON file under the Bot that receives it,
 * `<bot>/tasks/<task id>.json` (mode 0600), so it goes to the trash with that
 * Bot. The states are those of the A2A protocol: `submitted`, `working`,
 * `input-required`, `completed`, `failed`, `canceled` and `rejected`. Files are
 * read strictly at startup, like `bot.json`.
 */

import { lstatSync, readFileSync, readdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';

import { isSessionId } from '../sessionIdentity.js';
import { writeFileAtomic } from './files.js';
import { isBotId, type BotStore } from './store.js';

export const TASK_RECORD_VERSION = 1;
/** How many Bots one task may pass through before it reaches the one doing the work. */
export const MAX_CHAIN = 3;
export const MAX_GOAL_CHARS = 20_000;
export const MAX_MESSAGE_CHARS = 8_000;
export const MAX_RESULT_CHARS = 32_000;
export const MAX_REASON_CHARS = 300;
export const MAX_FOLLOW_UPS = 20;
export const MAX_KEY_CHARS = 128;
const MAX_RECORD_BYTES = 2 * 1024 * 1024;
const MAX_FILES = 16;
const MAX_NAME_CHARS = 200;
const MAX_PATH_CHARS = 400;
const SHA256 = /^[0-9a-f]{64}$/;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

export type TaskState = 'submitted' | 'working' | 'input-required' | 'completed' | 'failed' | 'canceled' | 'rejected';

const TASK_STATES: readonly TaskState[] = ['submitted', 'working', 'input-required', 'completed', 'failed', 'canceled', 'rejected'];

/** Whether work on the task is still to be done or waited for. */
export function isActive(state: TaskState): boolean {
  return state === 'submitted' || state === 'working' || state === 'input-required';
}

/** Whether the task needs nothing more from the Bot that sent it, or is waiting for someone else's answer. */
export function isSettled(state: TaskState): boolean {
  return state !== 'submitted' && state !== 'working';
}

/** One side of a task: a Bot, and the session of it that sent or runs the task. */
export interface TaskParty {
  bot_id: string;
  session_id: string;
}

/** A file that came with the task or was attached to it, as copied into an inbox. */
export interface TaskFile {
  name: string;
  /** Relative to the workspace it was copied into: `inbox/<task id>/<name>`. */
  path: string;
  size: number;
  sha256: string;
}

export interface TaskMessage {
  at: number;
  text: string;
}

/** `<bot>/tasks/<task id>.json`, version 1. */
export interface TaskRecord {
  v: 1;
  task_id: string;
  /** Epoch milliseconds. */
  created_at: number;
  updated_at: number;
  sender: TaskParty;
  receiver: TaskParty;
  /** The Bots that handed the task down, first to last; the last is the sender. */
  chain: string[];
  /** The task the sending session was itself doing, if it was doing one. */
  parent_task_id?: string;
  goal: string;
  /** The files that came with the task, in the receiver's inbox. */
  inputs: TaskFile[];
  /** Epoch milliseconds. */
  deadline_at: number;
  idempotency_key?: string;
  /** What the request was, to tell a retry from a different request that reuses its key. */
  request_digest: string;
  state: TaskState;
  /** Why the task is in this state, when that is not obvious. */
  reason?: string;
  /** What the sender said after the task began. */
  messages: TaskMessage[];
  /** What the receiver ended with. */
  result?: string;
  /** The files the receiver attached, in the sender's inbox. */
  artifacts: TaskFile[];
  cost_usd?: number;
  /** How many turns the receiver took on it: one, and one for each follow-up. */
  turns: number;
}

function plainObject(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${what}`);
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, required: readonly string[], optional: readonly string[], what: string): void {
  if (required.some((key) => !(key in value)) || Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new Error(`${what} has unsupported fields`);
  }
}

function timestamp(value: unknown, what: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error(`invalid ${what}`);
  return value as number;
}

function text(value: unknown, what: string, max: number, nonEmpty = true): string {
  if (typeof value !== 'string' || value.length > max || (nonEmpty && value.length === 0) || value.includes('\0')) throw new Error(`invalid ${what}`);
  return value;
}

function line(value: unknown, what: string, max: number): string {
  const result = text(value, what, max);
  if (CONTROL.test(result)) throw new Error(`invalid ${what}`);
  return result;
}

function party(value: unknown, what: string): TaskParty {
  const input = plainObject(value, what);
  exactKeys(input, ['bot_id', 'session_id'], [], what);
  if (!isBotId(input['bot_id']) || !isSessionId(input['session_id'])) throw new Error(`invalid ${what}`);
  return { bot_id: input['bot_id'], session_id: input['session_id'] };
}

function files(value: unknown, what: string): TaskFile[] {
  if (!Array.isArray(value) || value.length > MAX_FILES) throw new Error(`invalid ${what}`);
  return value.map((entry) => {
    const input = plainObject(entry, what);
    exactKeys(input, ['name', 'path', 'size', 'sha256'], [], what);
    if (!Number.isSafeInteger(input['size']) || (input['size'] as number) < 0 || typeof input['sha256'] !== 'string' || !SHA256.test(input['sha256'])) {
      throw new Error(`invalid ${what}`);
    }
    return { name: line(input['name'], what, MAX_NAME_CHARS), path: line(input['path'], what, MAX_PATH_CHARS), size: input['size'] as number, sha256: input['sha256'] };
  });
}

export function isTaskId(value: unknown): value is string {
  return isBotId(value);
}

/** Read a task file strictly: the one version, the exact fields, nothing else. */
export function parseTask(source: string, expectedId: string, receiverBotId: string): TaskRecord {
  let parsed: unknown;
  try { parsed = JSON.parse(source); } catch { throw new Error('the task file is not valid JSON'); }
  const input = plainObject(parsed, 'task');
  if (input['v'] !== TASK_RECORD_VERSION) throw new Error('the task file has an unsupported version');
  exactKeys(input, [
    'v', 'task_id', 'created_at', 'updated_at', 'sender', 'receiver', 'chain', 'goal', 'inputs', 'deadline_at', 'request_digest', 'state', 'messages', 'artifacts', 'turns',
  ], ['parent_task_id', 'idempotency_key', 'reason', 'result', 'cost_usd'], 'task');
  if (input['task_id'] !== expectedId || !isTaskId(input['task_id'])) throw new Error('the task file belongs to a different task');
  const sender = party(input['sender'], 'sender');
  const receiver = party(input['receiver'], 'receiver');
  if (receiver.bot_id !== receiverBotId) throw new Error('the task file is in the folder of a different Bot');
  const chain = input['chain'];
  if (!Array.isArray(chain) || chain.length < 1 || chain.length > MAX_CHAIN || !chain.every(isBotId) || new Set(chain).size !== chain.length
    || chain[chain.length - 1] !== sender.bot_id || chain.includes(receiver.bot_id)) throw new Error('invalid chain');
  if (!TASK_STATES.includes(input['state'] as TaskState)) throw new Error('invalid state');
  if (typeof input['request_digest'] !== 'string' || !SHA256.test(input['request_digest'])) throw new Error('invalid request digest');
  if (!Array.isArray(input['messages']) || input['messages'].length > MAX_FOLLOW_UPS) throw new Error('invalid messages');
  const messages = input['messages'].map((entry): TaskMessage => {
    const message = plainObject(entry, 'message');
    exactKeys(message, ['at', 'text'], [], 'message');
    return { at: timestamp(message['at'], 'message time'), text: text(message['text'], 'message', MAX_MESSAGE_CHARS) };
  });
  if (!Number.isSafeInteger(input['turns']) || (input['turns'] as number) < 0 || (input['turns'] as number) > MAX_FOLLOW_UPS + 1) throw new Error('invalid turns');
  if (input['parent_task_id'] !== undefined && !isTaskId(input['parent_task_id'])) throw new Error('invalid parent task');
  if (input['cost_usd'] !== undefined && (typeof input['cost_usd'] !== 'number' || !Number.isFinite(input['cost_usd']) || input['cost_usd'] < 0)) throw new Error('invalid cost');
  return {
    v: TASK_RECORD_VERSION,
    task_id: expectedId,
    created_at: timestamp(input['created_at'], 'created_at'),
    updated_at: timestamp(input['updated_at'], 'updated_at'),
    sender,
    receiver,
    chain: [...chain],
    ...(input['parent_task_id'] !== undefined ? { parent_task_id: input['parent_task_id'] as string } : {}),
    goal: text(input['goal'], 'goal', MAX_GOAL_CHARS),
    inputs: files(input['inputs'], 'inputs'),
    deadline_at: timestamp(input['deadline_at'], 'deadline_at'),
    ...(input['idempotency_key'] !== undefined ? { idempotency_key: line(input['idempotency_key'], 'idempotency key', MAX_KEY_CHARS) } : {}),
    request_digest: input['request_digest'],
    state: input['state'] as TaskState,
    ...(input['reason'] !== undefined ? { reason: line(input['reason'], 'reason', MAX_REASON_CHARS) } : {}),
    messages,
    ...(input['result'] !== undefined ? { result: text(input['result'], 'result', MAX_RESULT_CHARS, false) } : {}),
    artifacts: files(input['artifacts'], 'artifacts'),
    ...(input['cost_usd'] !== undefined ? { cost_usd: input['cost_usd'] as number } : {}),
    turns: input['turns'] as number,
  };
}

export function serializeTask(task: TaskRecord): string {
  return `${JSON.stringify(task, null, 2)}\n`;
}

/** The task files of every Bot. */
export class TaskStore {
  constructor(private readonly store: BotStore) {}

  private path(task: Pick<TaskRecord, 'task_id' | 'receiver'>): string {
    return join(this.store.tasksDir(task.receiver.bot_id), `${task.task_id}.json`);
  }

  /** Every task of every Bot; what could not be read is described in `problems`. */
  loadAll(): { tasks: TaskRecord[]; problems: string[] } {
    const tasks: TaskRecord[] = [];
    const problems: string[] = [];
    for (const { botId, record } of this.store.entries()) {
      if (record === undefined) continue;
      const directory = this.store.tasksDir(botId);
      let names: string[];
      try { names = readdirSync(directory).sort(); } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') problems.push(`${directory}: ${(error as Error).message}`);
        continue;
      }
      for (const name of names) {
        const path = join(directory, name);
        try {
          if (!name.endsWith('.json') || name.length !== '.json'.length + 22) continue;
          const stats = lstatSync(path);
          if (!stats.isFile() || stats.size > MAX_RECORD_BYTES) throw new Error('not a regular file of a sensible size');
          tasks.push(parseTask(readFileSync(path, 'utf8'), name.slice(0, -'.json'.length), botId));
        } catch (error) {
          problems.push(`${path}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
    }
    return { tasks, problems };
  }

  /** Write the task's file. Nothing is written for a Bot whose folder is gone, which would bring the folder back. */
  save(task: TaskRecord): boolean {
    if (!existsSync(this.store.recordPath(task.receiver.bot_id))) return false;
    writeFileAtomic(this.path(task), serializeTask(task));
    return true;
  }
}
