import type { CronAutomationDto } from '@lingxi/bridge-client';

export type ScheduledAutomation = CronAutomationDto;
export type ScheduledTaskFrequency = 'Daily' | 'Weekdays' | 'Weekly' | 'Custom';
export interface ScheduledCronJob {
  id: string;
  cron: string;
  prompt: string;
  recurring?: boolean;
  durable?: boolean;
  permanent?: boolean;
  expires_at?: number;
  session_id?: string;
  automation?: ScheduledAutomation;
  next_run_at?: number;
}
export type ScheduledTaskDraft = {
  scopeId?: string;
  automation?: ScheduledAutomation;
  title: string;
  instructions: string;
  frequency: ScheduledTaskFrequency;
  day: string;
  time: string;
  timezone: string;
  cron?: string;
  expiresAt?: string;
  originalExpiresAt?: number;
  recurring?: boolean;
  durable?: boolean;
};
const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

export function localScheduledTimezone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
}

export function scheduledTaskCron(draft: ScheduledTaskDraft): string {
  if (draft.frequency === 'Custom') {
    if (!draft.cron?.trim()) throw new Error('Enter a five-field cron expression.');
    return draft.cron.trim();
  }
  if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(draft.time)) throw new Error('Choose a valid time.');
  const [hour, minute] = draft.time.split(':').map(Number);
  const day = DAYS.indexOf(draft.day);
  if (draft.frequency === 'Weekly' && day < 0) throw new Error('Choose a valid weekday.');
  return `${minute} ${hour} * * ${draft.frequency === 'Weekly' ? day : draft.frequency === 'Weekdays' ? '1-5' : '*'}`;
}

export function formatScheduledTaskSchedule(draft: ScheduledTaskDraft): string {
  return draft.frequency === 'Custom' ? `${draft.cron} (${draft.timezone})`
    : `${draft.frequency}${draft.frequency === 'Weekly' ? ` on ${draft.day}` : ''} at ${draft.time} (${draft.timezone})`;
}

export function scheduledTaskInput(draft: ScheduledTaskDraft) {
  if (!draft.title.trim() || !draft.instructions.trim()) throw new Error('Task name and instructions are required.');
  if (draft.timezone !== localScheduledTimezone()) throw new Error('Cron runs in this computer’s time zone. Reopen the task after changing the system time zone.');
  // Preserve the exact persisted instant (including seconds and DST offsets)
  // when the user edits another field without changing the displayed cutoff.
  const expires_at = draft.expiresAt === undefined ? undefined
    : draft.originalExpiresAt !== undefined && localExpiryInput(draft.originalExpiresAt) === draft.expiresAt
      ? draft.originalExpiresAt : new Date(draft.expiresAt).getTime();
  if (expires_at !== undefined && (!Number.isFinite(expires_at) || (expires_at <= Date.now() && (!draft.automation || draft.automation.status === 'active')))) throw new Error('Choose an expiry date in the future, or select No expiry.');
  if (draft.automation && !draft.automation.model.trim()) throw new Error('Choose a model before saving.');
  if (draft.automation?.runMode === 'selected_session' && !draft.automation.targetSessionId) throw new Error('Choose a target chat.');
  const automation = draft.automation ? { ...draft.automation, name: draft.title.trim().replace(/\s*\n\s*/g, ' ') } : undefined;
  if (automation) { delete automation.runs; delete automation.ownedSessionId; }
  return { ...(automation ? { automation } : {}), ...(expires_at === undefined ? { no_expiry: true } : { expires_at }), cron: scheduledTaskCron(draft), prompt: `# ${draft.title.trim().replace(/\s*\n\s*/g, ' ')}\n\n${draft.instructions.trim()}`,
    recurring: draft.recurring ?? true, durable: draft.durable ?? true };
}

export function scheduledTaskFromJob(job: ScheduledCronJob): ScheduledTaskDraft {
  const heading = /^# ([^\n]+)\n\n([\s\S]*)$/.exec(job.prompt);
  const parts = job.cron.trim().split(/\s+/);
  const simple = parts.length === 5 && /^\d+$/.test(parts[0]) && /^\d+$/.test(parts[1])
    && Number(parts[0]) < 60 && Number(parts[1]) < 24 && parts[2] === '*' && parts[3] === '*';
  const frequency = simple && parts[4] === '*' ? 'Daily' : simple && parts[4] === '1-5' ? 'Weekdays'
    : simple && /^[0-6]$/.test(parts[4]) ? 'Weekly' : 'Custom';
  return { automation: job.automation, title: job.automation?.name ?? heading?.[1] ?? (job.prompt.split('\n')[0].slice(0, 90) || job.id),
    instructions: heading?.[2] ?? job.prompt, frequency, day: DAYS[Number(parts[4])] ?? 'Monday',
    time: simple ? `${parts[1].padStart(2, '0')}:${parts[0].padStart(2, '0')}` : '09:00',
    timezone: localScheduledTimezone(), cron: job.cron, expiresAt: job.expires_at ? localExpiryInput(job.expires_at) : undefined, originalExpiresAt: job.expires_at, recurring: job.recurring ?? false, durable: job.durable ?? true };
}

export function localExpiryInput(timestamp: number): string {
  const date = new Date(timestamp);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

// Estimate density only for numeric fields understood here; advanced expressions
// stay editable without presenting an invented frequency estimate.
function fieldCount(field: string, min: number, max: number): number | null {
  const values = new Set<number>();
  for (const part of field.split(',')) {
    const match = /^(\*|\d+(?:-\d+)?)(?:\/(\d+))?$/.exec(part);
    if (!match) return null;
    const step = Number(match[2] ?? 1);
    if (step < 1) return null;
    const [start, end] = match[1] === '*' ? [min, max] : match[1].includes('-')
      ? match[1].split('-').map(Number) : [Number(match[1]), match[2] ? max : Number(match[1])];
    if (start < min || end > max || start > end) return null;
    for (let value = start; value <= end; value += step) values.add(value);
  }
  return values.size;
}

export function scheduledTaskAdvice(draft: ScheduledTaskDraft): { text: string; suggestedDays?: number; suggestSlower?: boolean } {
  let fields: string[];
  try { fields = scheduledTaskCron(draft).split(/\s+/); }
  catch { return { text: 'Choose a valid interval to see scheduling guidance.' }; }
  if (draft.recurring === false) return { text: 'This task runs once. An expiry date is optional and can prevent a late run.' };
  const minutes = fieldCount(fields[0] ?? '', 0, 59);
  const hours = fieldCount(fields[1] ?? '', 0, 23);
  if (fields.length !== 5 || minutes === null || hours === null) return { text: 'For a temporary task, choose an end date. Ongoing tasks can run without an expiry date.' };
  const runsOnActiveDay = minutes * hours;
  if (runsOnActiveDay > 96) return { text: `This schedule can run ${runsOnActiveDay} times on an active day. Consider at least 15 minutes between runs and a 24-hour expiry for short-term monitoring.`, suggestedDays: 1, suggestSlower: true };
  if (runsOnActiveDay >= 24) return { text: `This schedule can run ${runsOnActiveDay} times on an active day. For temporary monitoring, consider a 7-day expiry; ongoing tasks can use No expiry.`, suggestedDays: 7 };
  return { text: 'No expiry works well for ongoing summaries and routine checks. Set an end date for temporary work.' };
}
