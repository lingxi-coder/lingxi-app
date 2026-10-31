import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import type { ModelDetailsDto, ReasoningSelectionDto } from '@lingxi/bridge-client';
import { scopeName } from '../bridge/scheduledPresentation';
import { localScheduledTimezone, localExpiryInput, scheduledTaskCron, scheduledTaskAdvice, scheduledTaskFromJob, type ScheduledCronJob, type ScheduledTaskDraft as Draft, type ScheduledTaskFrequency as Frequency, type ScheduledAutomation } from '../bridge/scheduledTaskDraft';
import './ScheduledTaskSetup.css';
import { currentLocale, tr } from '../i18n/locale';
import { DateTimePicker } from './ui/DateTimePicker';

export type ScheduledScope = { id: string; projectPath?: string; label: string; botId?: string };
export type ScheduledSession = { id: string; title: string; scopeId: string };
const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
export function ScheduledTaskSetup({ initial, editingJob, scopeId, scopes, sessions, models, defaultModel, defaultReasoning, onBack, onSave, onDirtyChange, loadContext }: {
  initial?: { title: string; description: string; draft?: Draft; preset?: Pick<Draft, 'frequency' | 'day' | 'time'> };
  editingJob?: ScheduledCronJob; scopeId: string; scopes: ScheduledScope[]; sessions: ScheduledSession[];
  loadContext: (scopeId: string) => Promise<{ models: ModelDetailsDto[]; currentModel: string; sessions: { uuid: string; title?: string }[] }>;
  models: readonly ModelDetailsDto[]; defaultModel?: string | null; defaultReasoning?: ReasoningSelectionDto;
  onBack: () => void; onSave: (draft: Draft) => Promise<void>; onDirtyChange: (dirty: boolean) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<Draft>(() => {
    const base = initial?.draft ?? (editingJob ? scheduledTaskFromJob(editingJob) : {
      title: initial?.title ?? '', instructions: initial?.description ?? '',
      frequency: initial?.preset?.frequency ?? 'Weekdays' as const, day: initial?.preset?.day ?? 'Monday',
      time: initial?.preset?.time ?? '08:00', timezone: localScheduledTimezone(), recurring: true, durable: true,
    });
    return { ...base, scopeId, automation: base.automation ?? { version: 2, status: 'active', model: defaultModel ?? '',
      reasoning: defaultReasoning ?? { type: 'automatic' }, runMode: 'new_session', notificationPolicy: 'all' } };
  });
  const [context, setContext] = useState<{ models: ModelDetailsDto[]; currentModel: string; sessions: { uuid: string; title?: string }[] } | null>(null);
  const [contextLoading, setContextLoading] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const baseline = useRef(JSON.stringify(draft));
  useEffect(() => { onDirtyChange(JSON.stringify(draft) !== baseline.current); }, [draft, onDirtyChange]);
  const advice = scheduledTaskAdvice(draft);
  const automation = draft.automation!;
  useEffect(() => {
    let cancelled = false; setContext(null); setContextLoading(true);
    void loadContext(draft.scopeId ?? 'global').then((value) => {
      if (cancelled) return; setContext(value);
      if (!editingJob && !initial?.draft) setDraft((previous) => {
        if (previous.automation?.model && value.models.some((model) => model.reference === previous.automation?.model)) return previous;
        const next = { ...previous, automation: { ...previous.automation!, model: value.currentModel, reasoning: value.models.find((model) => model.reference === value.currentModel)?.reasoning.provider_default ?? { type: 'automatic' as const } } };
        return next;
      });
    }).catch((cause) => { if (!cancelled) setError(String(cause)); }).finally(() => { if (!cancelled) setContextLoading(false); });
    return () => { cancelled = true; };
  }, [draft.scopeId, loadContext]);
  const availableModels = context?.models ?? models;
  const availableSessions = context ? context.sessions.map((session) => ({ id: session.uuid, title: session.title ?? `${tr('drawer_tab_chat')} ${session.uuid.slice(0, 8)}`, scopeId: draft.scopeId ?? 'global' })) : sessions;
  const selectedModel = availableModels.find((model) => model.reference === automation.model);
  const reasoningOptions = selectedModel?.reasoning.options.filter((option) => option.persistable && option.selection.type !== 'automatic' && option.selection.type !== 'disabled').map((option) => option.selection) ?? [];
  const update = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((previous) => ({ ...previous, [key]: value }));
  const updateAutomation = (patch: Partial<ScheduledAutomation>) => setDraft((previous) => ({ ...previous, automation: { ...previous.automation!, ...patch } }));
  async function saveTask(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (savingRef.current) return;
    savingRef.current = true; setSaving(true); setError('');
    try { await onSave(draft); baseline.current = JSON.stringify(draft); onDirtyChange(false); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { savingRef.current = false; setSaving(false); }
  }
  function timingFields() {
    return <>
      <label className="scheduled-setup-field">{tr('cron_recurring_toggle')}<select value={draft.frequency} onChange={(event) => update('frequency', event.target.value as Frequency)}>
        <option value="Daily">{tr('desktop_scheduled_daily')}</option><option value="Weekdays">{tr('desktop_scheduled_weekdays')}</option><option value="Weekly">{tr('desktop_scheduled_weekly')}</option><option value="Custom">{tr('desktop_scheduled_custom')}</option>
      </select></label>
      {draft.frequency === 'Weekly' && <label className="scheduled-setup-field">{tr('desktop_scheduled_on')}<select value={draft.day} onChange={(event) => update('day', event.target.value)}>{DAYS.map((day, index) => <option key={day} value={day}>{new Intl.DateTimeFormat(currentLocale(), { weekday: 'long' }).format(new Date(2024, 0, index + 1))}</option>)}</select></label>}
      {draft.frequency !== 'Custom' && <div className="scheduled-setup-field"><span>{tr('desktop_scheduled_at')}</span><DateTimePicker label={tr('desktop_scheduled_at')} mode="time" value={draft.time} onChange={(value) => update('time', value)} /></div>}
      {draft.frequency === 'Custom' && <label className="scheduled-setup-field">{tr('cron_expression_field_label')}<input required value={draft.cron ?? ''} placeholder="0 9 * * 1-5" onChange={(event) => update('cron', event.target.value)} /></label>}
      <div className="scheduled-setup-field"><span>{tr('cron_time_zone')}</span><span>{tr('desktop_scheduled_system_timezone', draft.timezone)}</span></div>
    </>;
  }

  function repeatField() {
    return <label className="scheduled-setup-field">{tr('desktop_scheduled_execution')}<select value={draft.recurring ? 'repeat' : 'once'} onChange={(event) => update('recurring', event.target.value === 'repeat')}>
      <option value="repeat">{tr('cron_recurring_toggle')}</option><option value="once">{tr('cron_run_once_toggle')}</option>
    </select></label>;
  }

  function expiryFields() {
    return <>
      <label className="scheduled-setup-field">{tr('desktop_scheduled_expires')}<select value={draft.expiresAt === undefined ? 'never' : 'date'} onChange={(event) => update('expiresAt', event.target.value === 'never' ? undefined : localExpiryInput(Date.now() + (advice.suggestedDays ?? 30) * 86400000))}>
        <option value="never">{tr('desktop_scheduled_no_expiry')}</option><option value="date">{tr('desktop_scheduled_on_date')}</option>
      </select></label>
      {draft.expiresAt !== undefined && <div className="scheduled-setup-field"><span>{tr('desktop_scheduled_end_date')}</span><DateTimePicker label={tr('desktop_scheduled_end_date')} mode="datetime-local" value={draft.expiresAt} onChange={(value) => update('expiresAt', value)} /></div>}
      <p className="scheduled-setup-disclosure" style={{ marginTop: 12 }}>{adviceLabel(advice)}</p>
      {advice.suggestedDays && <button type="button" className="scheduled-setup-back" style={{ marginRight: 14, marginBottom: 12 }} onClick={() => update('expiresAt', localExpiryInput(Date.now() + advice.suggestedDays! * 86400000))}>{tr('desktop_scheduled_use_expiry')}</button>}
      {advice.suggestSlower && <button type="button" className="scheduled-setup-back" style={{ marginBottom: 12 }} onClick={() => setDraft((previous) => ({ ...previous, frequency: 'Custom', cron: `*/15 ${scheduledTaskCron(previous).split(/\s+/).slice(1).join(' ')}` }))}>{tr('desktop_scheduled_use_interval')}</button>}
    </>;
  }

  return <section className="scheduled-setup" aria-label={tr('desktop_scheduled_details_aria')}>
    {/* The close control stays OUTSIDE the fieldset: a disabled fieldset
        disables every descendant, and `contextLoading` covers a per-scope
        engine spawn. Trapping the user behind a pane that hides the task list
        below 800px for the length of that spawn is not an acceptable cost of
        guarding the inputs. */}
    <header className="scheduled-setup-header"><span className={`scheduled-status scheduled-status-${automation.status}`}>{taskStatusLabel(automation.status)}</span>
      <button type="button" className="scheduled-setup-back" onClick={onBack} aria-label={tr('desktop_scheduled_close_details')}>×</button></header>
    <form onSubmit={saveTask}><fieldset disabled={saving || contextLoading}>
      {contextLoading && <p role="status" className="scheduled-setup-disclosure">{tr('desktop_scheduled_loading_context')}</p>}
      <label className="scheduled-setup-title-label" htmlFor={`${id}-title`}>{tr('cron_task_name')}</label>
      <input className="scheduled-setup-title" id={`${id}-title`} required value={draft.title} placeholder={tr('desktop_scheduled_name_placeholder')} onChange={(event) => update('title', event.target.value)} />
      <label className="scheduled-setup-title-label" htmlFor={`${id}-instructions`}>{tr('cron_instructions')}</label>
      <textarea id={`${id}-instructions`} required rows={4} value={draft.instructions} placeholder={tr('desktop_scheduled_instructions_placeholder')} onChange={(event) => update('instructions', event.target.value)} />
      <h3>{tr('session_details_button')}</h3><div className="scheduled-setup-group">
        <label className="scheduled-setup-field">{tr('cron_runs_in')}<select value={automation.runMode} onChange={(event) => updateAutomation({ runMode: event.target.value as ScheduledAutomation['runMode'], targetSessionId: undefined, ownedSessionId: undefined })}>
          <option value="new_session">{tr('cron_run_mode_new_chat')}</option><option value="selected_session">{tr('cron_run_mode_selected_chat')}</option><option value="task_session">{tr('cron_run_mode_task_chat')}</option>
        </select></label>
        {automation.runMode === 'selected_session' && <label className="scheduled-setup-field">{tr('drawer_tab_chat')}<select required value={automation.targetSessionId ?? ''} onChange={(event) => {
          const session = availableSessions.find((item) => item.id === event.target.value); if (!session) return;
          setDraft((previous) => ({ ...previous, scopeId: session.scopeId, automation: { ...previous.automation!, targetSessionId: session.id } }));
        }}><option value="">{tr('cron_select_chat')}</option>{availableSessions.filter((session) => !editingJob || session.scopeId === scopeId).map((session) => <option key={`${session.scopeId}:${session.id}`} value={session.id}>{session.title}</option>)}</select></label>}
        <label className="scheduled-setup-field">{tr('cron_run_project_label')}<select value={draft.scopeId} disabled={Boolean(editingJob) || (automation.runMode === 'selected_session' && Boolean(automation.targetSessionId))} onChange={(event) => update('scopeId', event.target.value)}>{scopes.map((scope) => <option key={scope.id} value={scope.id}>{scopeName(scope)}</option>)}</select></label>
        <label className="scheduled-setup-field">{tr('session_details_model')}<select required value={automation.model} onChange={(event) => {
          const model = availableModels.find((item) => item.reference === event.target.value);
          const compatible = model?.reasoning.options.some((option) => JSON.stringify(option.selection) === JSON.stringify(automation.reasoning));
          updateAutomation({ model: event.target.value, reasoning: compatible ? automation.reasoning : model?.reasoning.provider_default ?? { type: 'automatic' } });
          setNotice(compatible ? '' : 'desktop_scheduled_reasoning_reset');
        }}><option value="">{tr('composer_select_model')}</option>{automation.model && !selectedModel && <option value={automation.model}>{tr('desktop_scheduled_model_unavailable', automation.model)}</option>}{availableModels.map((model) => <option key={model.reference} value={model.reference}>{model.display_name} · {model.provider_label}</option>)}</select></label>
        <label className="scheduled-setup-field">{tr('model_detail_reasoning')}<select value={JSON.stringify(automation.reasoning)} onChange={(event) => updateAutomation({ reasoning: JSON.parse(event.target.value) as ReasoningSelectionDto })}>
          {!reasoningOptions.some((option) => JSON.stringify(option) === JSON.stringify(automation.reasoning)) && <option value={JSON.stringify(automation.reasoning)}>{reasoningLabel(automation.reasoning)}</option>}
          {reasoningOptions.map((selection) => <option key={JSON.stringify(selection)} value={JSON.stringify(selection)}>{reasoningLabel(selection)}</option>)}
        </select></label>
        <label className="scheduled-setup-field">{tr('session_details_status')}<select value={automation.status} onChange={(event) => updateAutomation({ status: event.target.value as ScheduledAutomation['status'], statusReason: undefined })}><option value="active">{tr('cron_task_status_active')}</option><option value="paused">{tr('chat_status_paused')}</option><option value="completed">{tr('chat_status_completed')}</option></select></label>
      </div>
      {automation.statusReason && <p className="scheduled-setup-disclosure">{automation.statusReason}</p>}
      {draft.scopeId === 'global' && <p className="scheduled-setup-disclosure">{tr('desktop_scheduled_no_project')}</p>}
      {scopes.find((scope) => scope.id === draft.scopeId)?.botId !== undefined && <p className="scheduled-setup-disclosure">{tr('bots_routine_scope_note')}</p>}
      {notice && <p role="status" className="scheduled-setup-disclosure">{tr(notice)}</p>}
      <h3>{tr('cron_frequency')}</h3><div className="scheduled-setup-group">{timingFields()}{repeatField()}{expiryFields()}
        <label className="scheduled-setup-field">{tr('settings_notifications')}<select value={automation.notificationPolicy} onChange={(event) => updateAutomation({ notificationPolicy: event.target.value as ScheduledAutomation['notificationPolicy'] })}><option value="all">{tr('cron_notify_all')}</option><option value="failed">{tr('cron_notify_failed')}</option><option value="none">{tr('composer_effort_off')}</option></select></label>
      </div>
      <p className="scheduled-setup-disclosure">{tr('desktop_scheduled_local_note')}</p>
      {error && <p role="alert" className="scheduled-setup-error">{scheduleErrorLabel(error)}</p>}
      <button type="submit" className="scheduled-setup-primary scheduled-setup-submit" disabled={!draft.title.trim() || !draft.instructions.trim() || !automation.model}>{saving ? tr('audio_settings_saving') : editingJob ? tr('settings_save') : tr('common_create')}</button>
    </fieldset></form>
  </section>;
}

function taskStatusLabel(status: string): string {
  return tr(status === 'active' ? 'cron_task_status_active' : status === 'paused' ? 'chat_status_paused' : 'chat_status_completed');
}
function reasoningLabel(selection: ReasoningSelectionDto): string {
  if (selection.type === 'level') {
    return ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(selection.id) ? tr(`composer_effort_${selection.id}`) : selection.id;
  }
  if (selection.type === 'token_budget') return tr('composer_effort_budget', selection.tokens);
  return tr(selection.type === 'disabled' ? 'composer_effort_off' : selection.type === 'enabled' ? 'composer_effort_on' : 'composer_effort_auto');
}
function adviceLabel(advice: ReturnType<typeof scheduledTaskAdvice>): string {
  const text = advice.text;
  const count = /^This schedule can run (\d+) times/.exec(text)?.[1];
  if (count) return tr(advice.suggestSlower ? 'desktop_scheduled_advice_frequent' : 'desktop_scheduled_advice_hourly', count);
  const key = text.startsWith('Choose a valid interval') ? 'invalid' : text.startsWith('This task runs once') ? 'once' : text.startsWith('For a temporary task') ? 'temporary' : 'ongoing';
  return tr(`desktop_scheduled_advice_${key}`);
}
function scheduleErrorLabel(error: string): string {
  if (error === 'desktop_scheduled_wait_change') return tr(error);
  const keys: Record<string, string> = {
    'Enter a five-field cron expression.': 'desktop_scheduled_error_cron',
    'Choose a valid time.': 'desktop_scheduled_error_time',
    'Choose a valid weekday.': 'desktop_scheduled_error_weekday',
    'Task name and instructions are required.': 'desktop_scheduled_error_required',
    'Cron runs in this computer’s time zone. Reopen the task after changing the system time zone.': 'desktop_scheduled_error_timezone',
    'Choose an expiry date in the future, or select No expiry.': 'desktop_scheduled_error_expiry',
    'Choose a model before saving.': 'desktop_scheduled_error_model',
    'Choose a target chat.': 'desktop_scheduled_error_chat',
  };
  return keys[error] ? tr(keys[error]!) : error;
}
