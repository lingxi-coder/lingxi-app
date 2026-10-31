/**
 * How the tasks between Bots read to the owner in the window. Free of Electron.
 *
 * A task's record holds more than the owner is shown (file hashes, the key a
 * retry is told by, the digest of the request), so the window gets a record
 * made for it. The only paths in it are those of the files a task moved, which
 * are relative to the workspace they were copied into.
 */

import type { BotTaskUi, BotTasksUi } from '../../shared/bots.js';
import { isActive, type TaskRecord } from './tasks.js';

/** How much of what was asked is shown in a row. */
export const TASK_SUMMARY_CHARS = 200;
/** How much of what a Bot ended with is sent to the window; the rest is cut. */
export const TASK_RESULT_CHARS = 8_000;
/** At most this many of a Bot's tasks are listed, the newest. */
export const TASKS_LISTED_MAX = 100;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/g;

/** What of the Bots the mapping needs to know. */
export interface TaskBots {
  /** The Bot's name, while the Bot is there. */
  nameOf(botId: string): string | undefined;
  /** The Bot's workspace folder. */
  workspaceOf(botId: string): string;
}

/** The first line of what was asked, on one line and short. */
function summaryOf(goal: string): string {
  const first = goal.split(/\r?\n/).find((line) => line.trim() !== '') ?? '';
  const flat = first.replace(CONTROL, ' ').replace(/\s+/g, ' ').trim();
  return flat.length > TASK_SUMMARY_CHARS ? `${flat.slice(0, TASK_SUMMARY_CHARS - 1).trimEnd()}…` : flat;
}

/** A reason may quote an error that names a file; inside a Bot's workspace it is named by its place there. */
function withinWorkspaces(reason: string, folders: readonly string[]): string {
  let shown = reason;
  for (const folder of folders) shown = shown.split(`${folder}/`).join('').split(folder).join('.');
  return shown;
}

export function taskUi(task: TaskRecord, viewer: string, bots: TaskBots): BotTaskUi {
  const received = task.receiver.bot_id === viewer;
  const peerId = received ? task.sender.bot_id : task.receiver.bot_id;
  const peerName = bots.nameOf(peerId);
  const result = task.result;
  const cut = result !== undefined && result.length > TASK_RESULT_CHARS;
  return {
    taskId: task.task_id,
    direction: received ? 'received' : 'sent',
    peer: { botId: peerId, ...(peerName !== undefined ? { name: peerName } : {}) },
    state: task.state,
    summary: summaryOf(task.goal),
    ...(task.reason !== undefined ? { reason: withinWorkspaces(task.reason, [bots.workspaceOf(task.sender.bot_id), bots.workspaceOf(task.receiver.bot_id)]) } : {}),
    ...(result !== undefined ? { result: cut ? result.slice(0, TASK_RESULT_CHARS) : result } : {}),
    ...(cut ? { resultTruncated: true } : {}),
    artifacts: task.artifacts.map(({ name, path, size }) => ({ name, path, size })),
    ...(task.cost_usd !== undefined ? { costUsd: task.cost_usd } : {}),
    createdAt: task.created_at,
    updatedAt: task.updated_at,
    deadlineAt: task.deadline_at,
    receiver: { botId: task.receiver.bot_id, sessionId: task.receiver.session_id },
    turns: task.turns,
  };
}

/** The tasks a Bot was given and the ones it handed out, newest first, the newest {@link TASKS_LISTED_MAX} of them. */
export function tasksUi(tasks: Iterable<TaskRecord>, viewer: string, bots: TaskBots): BotTasksUi {
  const mine = [...tasks]
    .filter((task) => task.receiver.bot_id === viewer || task.sender.bot_id === viewer)
    .sort((a, b) => b.created_at - a.created_at || a.task_id.localeCompare(b.task_id));
  const active = { received: 0, sent: 0 };
  for (const task of mine) if (isActive(task.state)) active[task.receiver.bot_id === viewer ? 'received' : 'sent'] += 1;
  return { tasks: mine.slice(0, TASKS_LISTED_MAX).map((task) => taskUi(task, viewer, bots)), truncated: mine.length > TASKS_LISTED_MAX, active };
}
