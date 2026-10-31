/**
 * How scheduled tasks are put into words: shared by the Scheduled page and by
 * the routines panel of a Bot, which shows the same tasks.
 */

import { currentLocale, tr } from '../i18n/locale';
import type { ScheduledScope } from '../../shared/scheduled';
import type { ScheduledCronJob, ScheduledTaskDraft } from './scheduledTaskDraft';

/** The state a task is in; a task without a configuration is one the engine treats as active. */
export function jobStatus(job: Pick<ScheduledCronJob, 'automation'>): 'active' | 'paused' | 'completed' {
  return job.automation?.status ?? 'active';
}

export function taskStatusLabel(status: string): string {
  const keys: Record<string, string> = { all: 'drawer_filter_all', active: 'cron_task_status_active', paused: 'chat_status_paused', completed: 'chat_status_completed' };
  return keys[status] ? tr(keys[status]!) : status;
}

export function scheduleLabel(draft: ScheduledTaskDraft): string {
  if (draft.frequency === 'Custom') return `${draft.cron} (${draft.timezone})`;
  if (draft.frequency === 'Weekly') {
    const index = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'].indexOf(draft.day);
    const day = index < 0 ? draft.day : new Intl.DateTimeFormat(currentLocale(), { weekday: 'long' }).format(new Date(2024, 0, index + 1));
    return tr('desktop_scheduled_weekly_schedule', day, draft.time, draft.timezone);
  }
  return tr('desktop_scheduled_schedule', tr(draft.frequency === 'Daily' ? 'desktop_scheduled_daily' : 'desktop_scheduled_weekdays'), draft.time, draft.timezone);
}

/** When the task runs next, or why it does not: the line under a task's schedule. */
export function nextRunLabel(job: Pick<ScheduledCronJob, 'automation' | 'next_run_at'>): string {
  const status = jobStatus(job);
  return job.automation?.statusReason ?? (job.next_run_at ? tr('desktop_scheduled_next_run', new Date(job.next_run_at).toLocaleString(currentLocale())) : status === 'active' ? tr('desktop_scheduled_local') : taskStatusLabel(status));
}

/** What a scope is called where tasks are listed: a Bot's workspace carries the Bot's name, marked as a Bot's. */
export function scopeName(scope: Pick<ScheduledScope, 'label' | 'botId'>): string {
  return scope.botId !== undefined ? tr('bots_routine_scope', scope.label) : scope.label;
}
