/**
 * The routines of a Bot: the scheduled tasks that run as the Bot. They live in
 * the Bot's workspace, which the scheduler lists as a scope of its own; this
 * reads that scope's tasks and puts each in words for the Bot's page. Making
 * and changing a routine is the Scheduled page's work, which the Bot's page
 * sends the owner to.
 */

import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import type { ScheduledScope } from '../../shared/scheduled';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';
import { jobStatus, nextRunLabel, scheduleLabel, taskStatusLabel } from './scheduledPresentation';
import { scheduledTaskFromJob } from './scheduledTaskDraft';

/** The scope a Bot's routines are in, once the scheduler has listed it. */
export function routineScopeOf(scopes: readonly Pick<ScheduledScope, 'id' | 'botId'>[], botId: string): string | undefined {
  return scopes.find((scope) => scope.botId === botId)?.id;
}

/**
 * What changes which scopes the scheduler lists for Bots: which Bots there are
 * and what each is called. The window asks for the scopes again when this does.
 */
export function botScopesKey(bots: readonly { botId: string; name: string }[]): string {
  return bots.map((bot) => `${bot.botId}\n${bot.name}`).sort().join('\n\n');
}

/** One routine as the Bot's page shows it. */
export interface BotRoutineRow {
  id: string;
  title: string;
  schedule: string;
  status: 'active' | 'paused' | 'completed';
  statusLabel: string;
  /** When it runs next, or why it does not. */
  next: string;
  /** A task that lasts only as long as a chat is not kept, so there is nothing to open. */
  openable: boolean;
}

export function routineRow(job: CronJobDto): BotRoutineRow {
  const draft = scheduledTaskFromJob(job);
  const status = jobStatus(job);
  return {
    id: job.id,
    title: draft.title,
    schedule: scheduleLabel(draft),
    status,
    statusLabel: taskStatusLabel(status),
    next: nextRunLabel(job),
    openable: !job.permanent && job.durable !== false,
  };
}

export interface BotRoutinesState {
  /** Unknown until the first read. */
  jobs: readonly CronJobDto[] | undefined;
  loading: boolean;
  error: string | undefined;
}

export class BotRoutinesController extends ObservableState<BotRoutinesState> {
  private request = 0;

  constructor(private readonly manage: (scopeId: string, request: CronRequestDto) => Promise<CronJobDto[]>) {
    super({ jobs: undefined, loading: false, error: undefined });
  }

  /** Read the tasks of the Bot's scope. An answer that a later read overtook is dropped. */
  async load(scopeId: string): Promise<void> {
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const jobs = await this.manage(scopeId, { action: 'list' });
      if (request === this.request) this.update({ jobs, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }
}
