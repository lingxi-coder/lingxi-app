/**
 * The tasks a Bot was given by other Bots and the ones it handed to them, as
 * its page shows them. The host tells the window when tasks change, so the list
 * is read again then and not on a timer; stopping a task is asked about first.
 */

import type { BotTaskDirection, BotTaskStateUi, BotTaskUi, BotTasksUi, BotsApi } from '../../shared/bots';
import { tr } from '../i18n/locale';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';
import { formatClockTime } from './sessionPresentation';
import { relativeActivity } from './botSession';

const STATE_LABELS: Record<BotTaskStateUi, string> = {
  submitted: 'bots_task_state_submitted',
  working: 'bots_task_state_working',
  'input-required': 'bots_task_state_input_required',
  completed: 'bots_task_state_completed',
  failed: 'bots_task_state_failed',
  canceled: 'bots_task_state_canceled',
  rejected: 'bots_task_state_rejected',
};

export function taskStateLabel(state: BotTaskStateUi): string {
  return tr(STATE_LABELS[state]);
}

/** Whether work on the task is still going on or waited for, which is when it can still be stopped. */
export function taskActive(state: BotTaskStateUi): boolean {
  return state === 'submitted' || state === 'working' || state === 'input-required';
}

export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${Math.round(bytes / 1_024)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

export function formatCost(usd: number): string {
  if (!Number.isFinite(usd) || usd < 0) return '';
  return usd < 0.005 ? '<$0.01' : `$${usd.toFixed(2)}`;
}

/** One task as the Bot's page shows it. */
export interface BotTaskRow {
  id: string;
  direction: BotTaskDirection;
  /** "From Alice" or "To Bob". */
  title: string;
  state: BotTaskStateUi;
  stateLabel: string;
  summary: string;
  /** When it was last heard of; for a task still going on, by when it has to be done. */
  when: string;
  reason: string | undefined;
  result: { text: string; truncated: boolean } | undefined;
  /** The label over the files, which say whose workspace they were copied into. */
  filesLabel: string;
  files: Array<{ name: string; size: string; path: string }>;
  cost: string | undefined;
  /** It can be stopped. */
  active: boolean;
  /** The receiving Bot's session has taken a turn, so there is a chat to open. */
  openable: boolean;
  receiver: { botId: string; sessionId: string };
}

export function taskRow(task: BotTaskUi, now = Date.now()): BotTaskRow {
  const peer = task.peer.name ?? tr('bots_task_peer_gone');
  const active = taskActive(task.state);
  const updated = relativeActivity(task.updatedAt, now);
  return {
    id: task.taskId,
    direction: task.direction,
    title: tr(task.direction === 'received' ? 'bots_task_from' : 'bots_task_to', peer),
    state: task.state,
    stateLabel: taskStateLabel(task.state),
    summary: task.summary,
    when: active ? tr('bots_task_due', formatClockTime(task.deadlineAt)) : tr('bots_task_updated', updated),
    // What waiting means is said in the owner's language; the host's own words for it are English.
    reason: task.state === 'input-required' ? tr('bots_task_input_hint') : task.reason,
    result: task.result !== undefined ? { text: task.result, truncated: task.resultTruncated === true } : undefined,
    filesLabel: tr(task.direction === 'received' ? 'bots_task_files_returned' : 'bots_task_files_from', peer),
    files: task.artifacts.map((file) => ({ name: file.name, size: formatFileSize(file.size), path: file.path })),
    cost: task.costUsd !== undefined ? tr('bots_task_cost', formatCost(task.costUsd)) : undefined,
    active,
    openable: task.turns > 0,
    receiver: task.receiver,
  };
}

export interface BotTasksState {
  /** Unknown until the first read. */
  view: BotTasksUi | undefined;
  loading: boolean;
  error: string | undefined;
  /** The task the owner was asked whether to stop. */
  canceling: { task: BotTaskUi; busy: boolean; error: string | undefined } | undefined;
}

export class BotTasksController extends ObservableState<BotTasksState> {
  private request = 0;

  constructor(private readonly api: BotsApi['tasks'], private readonly botId: string) {
    super({ view: undefined, loading: false, error: undefined, canceling: undefined });
  }

  /** Read the Bot's tasks. An answer that a later read overtook is dropped. */
  async load(): Promise<void> {
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const view = await this.api.list(this.botId);
      if (request === this.request) this.update({ view, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }

  /** Read the tasks again whenever the host says some of this Bot's changed. Returns the way to stop. */
  watch(): () => void {
    return this.api.onChanged((change) => {
      if (change.botIds.includes(this.botId)) void this.load();
    });
  }

  askCancel(taskId: string): void {
    const task = this.getSnapshot().view?.tasks.find((entry) => entry.taskId === taskId);
    if (task && taskActive(task.state)) this.update({ canceling: { task, busy: false, error: undefined } });
  }

  dismissCancel(): void {
    if (!this.getSnapshot().canceling?.busy) this.update({ canceling: undefined });
  }

  async confirmCancel(): Promise<void> {
    const { canceling } = this.getSnapshot();
    if (!canceling || canceling.busy) return;
    this.update({ canceling: { ...canceling, busy: true, error: undefined } });
    try {
      await this.api.cancel(this.botId, canceling.task.taskId);
    } catch (error) {
      this.update({ canceling: { ...canceling, busy: false, error: ipcErrorMessage(error) } });
      return;
    }
    this.update({ canceling: undefined });
    await this.load();
  }
}
