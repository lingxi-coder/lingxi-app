/**
 * The routines of a Bot: the scheduled tasks that run as the Bot. They live in
 * the Bot's workspace, which the scheduler lists as a scope of its own; this
 * reads that scope's tasks and puts each in words for the Bot's page. Making
 * and changing a routine is the Scheduled page's work, which the Bot's page
 * sends the owner to.
 */

import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import { BOT_DEFAULT_WORKSPACE_ID } from '../../shared/bots';
import type { ScheduledScope } from '../../shared/scheduled';
import { ipcErrorMessage } from './botForm';
import { ownFolderTitle } from './botFolders';
import { ObservableState } from './observableState';
import { jobStatus, nextRunLabel, scheduleLabel, taskStatusLabel } from './scheduledPresentation';
import { scheduledTaskFromJob, type ScheduledTaskDraft } from './scheduledTaskDraft';

/** The scope a Bot's routines are in, once the scheduler has listed it. */
export function routineScopeOf(scopes: readonly Pick<ScheduledScope, 'id' | 'botId'>[], botId: string): string | undefined {
  return scopes.find((scope) => scope.botId === botId)?.id;
}

/** A folder a routine of a Bot can be told to run in, as the form offers it. */
export interface RoutineFolderOption {
  id: string;
  title: string;
}

/**
 * The folders a routine of the scope can run in, the Bot's own first and named as the window names it. None for a
 * scope that is not a Bot's, or a Bot that has no folder besides its own: there is nothing to choose then.
 */
export function routineFolderOptions(scope: Pick<ScheduledScope, 'folders'> | undefined): RoutineFolderOption[] {
  return (scope?.folders ?? []).map((folder) => ({ id: folder.id, title: folder.id === BOT_DEFAULT_WORKSPACE_ID ? ownFolderTitle() : folder.label }));
}

/**
 * What the folder choice shows: the folder the routine was given, the Bot's own for one that was given none, and
 * nothing (`''`) for one whose folder is not the Bot's any more, so that the owner is asked to choose again.
 */
export function routineFolderValue(options: readonly RoutineFolderOption[], chosen: string | undefined): string {
  const id = chosen ?? BOT_DEFAULT_WORKSPACE_ID;
  return options.some((folder) => folder.id === id) ? id : '';
}

/**
 * What the host is told when a routine is saved: the folder chosen, if it is one of the Bot's. Nothing leaves the
 * routine where it was put. A routine that works in a chat chosen for it is given none: it works where that chat does.
 */
export function routineFolderToSet(scope: Pick<ScheduledScope, 'folders'> | undefined, draft: Pick<ScheduledTaskDraft, 'folderId' | 'automation'>): string | undefined {
  if (draft.automation?.runMode === 'selected_session') return undefined;
  return draft.folderId !== undefined && routineFolderOptions(scope).some((folder) => folder.id === draft.folderId) ? draft.folderId : undefined;
}

/**
 * What the owner chose for the routines of every Bot that has folders, by scope and then routine; a routine that is
 * not listed runs in the Bot's own. A Bot that cannot say is left out, which shows its routines as in the own folder.
 */
export async function loadRoutineFolders(
  scopes: readonly Pick<ScheduledScope, 'id' | 'folders'>[],
  load: (scopeId: string) => Promise<Record<string, string>>,
): Promise<Record<string, Record<string, string>>> {
  const answers = await Promise.all(scopes.filter((scope) => scope.folders !== undefined).map(async (scope) => {
    try { return [[scope.id, await load(scope.id)] as const]; } catch { return []; }
  }));
  return Object.fromEntries(answers.flat());
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
