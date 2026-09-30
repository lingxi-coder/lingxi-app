import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import type { ModelDetailsDto, ReasoningSelectionDto } from '@lingxi/bridge-client';
import { localScheduledTimezone, localExpiryInput, scheduledTaskCron, scheduledTaskAdvice, scheduledTaskFromJob, type ScheduledCronJob, type ScheduledTaskDraft as Draft, type ScheduledTaskFrequency as Frequency, type ScheduledAutomation } from '../bridge/scheduledTaskDraft';
import './ScheduledTaskSetup.css';
import { DateTimePicker } from './ui/DateTimePicker';

export type ScheduledScope = { id: string; projectPath?: string; label: string };
export type ScheduledSession = { id: string; title: string; scopeId: string };
const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
export function ScheduledTaskSetup({ initial, editingJob, scopeId, scopes, sessions, models, defaultModel, defaultReasoning, onBack, onSave, onDirtyChange, loadContext }: {
  initial?: { title: string; schedule: string; description: string; draft?: Draft };
  editingJob?: ScheduledCronJob; scopeId: string; scopes: ScheduledScope[]; sessions: ScheduledSession[];
  loadContext: (scopeId: string) => Promise<{ models: ModelDetailsDto[]; currentModel: string; sessions: { uuid: string; title?: string }[] }>;
  models: readonly ModelDetailsDto[]; defaultModel?: string | null; defaultReasoning?: ReasoningSelectionDto;
  onBack: () => void; onSave: (draft: Draft) => Promise<void>; onDirtyChange: (dirty: boolean) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<Draft>(() => {
    const weekly = initial?.schedule.startsWith('Fridays');
    const base = initial?.draft ?? (editingJob ? scheduledTaskFromJob(editingJob) : {
      title: initial?.title ?? '', instructions: initial?.description ?? '',
      frequency: weekly ? 'Weekly' as const : 'Weekdays' as const, day: weekly ? 'Friday' : 'Monday',
      time: weekly ? '16:00' : initial?.title === 'Follow-up monitor' ? '09:00' : '08:00', timezone: localScheduledTimezone(), recurring: true, durable: true,
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
  const availableSessions = context ? context.sessions.map((session) => ({ id: session.uuid, title: session.title ?? `Chat ${session.uuid.slice(0, 8)}`, scopeId: draft.scopeId ?? 'global' })) : sessions;
  const selectedModel = availableModels.find((model) => model.reference === automation.model);
  const reasoningOptions = selectedModel?.reasoning.options.filter((option) => option.persistable).map((option) => option.selection) ?? [{ type: 'automatic' } as const];
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
      <label className="scheduled-setup-field">Repeat<select value={draft.frequency} onChange={(event) => update('frequency', event.target.value as Frequency)}>
        <option>Daily</option><option>Weekdays</option><option>Weekly</option><option>Custom</option>
      </select></label>
      {draft.frequency === 'Weekly' && <label className="scheduled-setup-field">On<select value={draft.day} onChange={(event) => update('day', event.target.value)}>{DAYS.map((day) => <option key={day}>{day}</option>)}</select></label>}
      {draft.frequency !== 'Custom' && <div className="scheduled-setup-field"><span>At</span><DateTimePicker label="At" mode="time" value={draft.time} onChange={(value) => update('time', value)} /></div>}
      {draft.frequency === 'Custom' && <label className="scheduled-setup-field">Cron expression<input required value={draft.cron ?? ''} placeholder="0 9 * * 1-5" onChange={(event) => update('cron', event.target.value)} /></label>}
      <div className="scheduled-setup-field"><span>Time zone</span><span>{draft.timezone} (system)</span></div>
    </>;
  }

  function repeatField() {
    return <label className="scheduled-setup-field">Execution<select value={draft.recurring ? 'repeat' : 'once'} onChange={(event) => update('recurring', event.target.value === 'repeat')}>
      <option value="repeat">Repeat on schedule</option><option value="once">Run once</option>
    </select></label>;
  }

  function expiryFields() {
    return <>
      <label className="scheduled-setup-field">Expires<select value={draft.expiresAt === undefined ? 'never' : 'date'} onChange={(event) => update('expiresAt', event.target.value === 'never' ? undefined : localExpiryInput(Date.now() + (advice.suggestedDays ?? 30) * 86400000))}>
        <option value="never">No expiry</option><option value="date">On a date</option>
      </select></label>
      {draft.expiresAt !== undefined && <div className="scheduled-setup-field"><span>End date</span><DateTimePicker label="End date" mode="datetime-local" value={draft.expiresAt} onChange={(value) => update('expiresAt', value)} /></div>}
      <p className="scheduled-setup-disclosure" style={{ marginTop: 12 }}>{advice.text}</p>
      {advice.suggestedDays && <button type="button" className="scheduled-setup-back" style={{ marginRight: 14, marginBottom: 12 }} onClick={() => update('expiresAt', localExpiryInput(Date.now() + advice.suggestedDays! * 86400000))}>Use suggested expiry</button>}
      {advice.suggestSlower && <button type="button" className="scheduled-setup-back" style={{ marginBottom: 12 }} onClick={() => setDraft((previous) => ({ ...previous, frequency: 'Custom', cron: `*/15 ${scheduledTaskCron(previous).split(/\s+/).slice(1).join(' ')}` }))}>Use a 15-minute interval</button>}
    </>;
  }

  return <section className="scheduled-setup" aria-label="Editable task details">
    {/* The close control stays OUTSIDE the fieldset: a disabled fieldset
        disables every descendant, and `contextLoading` covers a per-scope
        engine spawn. Trapping the user behind a pane that hides the task list
        below 800px for the length of that spawn is not an acceptable cost of
        guarding the inputs. */}
    <header className="scheduled-setup-header"><span className={`scheduled-status scheduled-status-${automation.status}`}>{automation.status}</span>
      <button type="button" className="scheduled-setup-back" onClick={onBack} aria-label="Close task details">×</button></header>
    <form onSubmit={saveTask}><fieldset disabled={saving || contextLoading}>
      {contextLoading && <p role="status" className="scheduled-setup-disclosure">Loading project settings…</p>}
      <label className="scheduled-setup-title-label" htmlFor={`${id}-title`}>Task name</label>
      <input className="scheduled-setup-title" id={`${id}-title`} required value={draft.title} placeholder="Name your task" onChange={(event) => update('title', event.target.value)} />
      <label className="scheduled-setup-title-label" htmlFor={`${id}-instructions`}>Instructions</label>
      <textarea id={`${id}-instructions`} required rows={4} value={draft.instructions} placeholder="What should LingXi do?" onChange={(event) => update('instructions', event.target.value)} />
      <h3>Details</h3><div className="scheduled-setup-group">
        <label className="scheduled-setup-field">Runs in<select value={automation.runMode} onChange={(event) => updateAutomation({ runMode: event.target.value as ScheduledAutomation['runMode'], targetSessionId: undefined, ownedSessionId: undefined })}>
          <option value="new_session">New chat each run</option><option value="selected_session">Selected chat</option><option value="task_session">Dedicated task chat</option>
        </select></label>
        {automation.runMode === 'selected_session' && <label className="scheduled-setup-field">Chat<select required value={automation.targetSessionId ?? ''} onChange={(event) => {
          const session = availableSessions.find((item) => item.id === event.target.value); if (!session) return;
          setDraft((previous) => ({ ...previous, scopeId: session.scopeId, automation: { ...previous.automation!, targetSessionId: session.id } }));
        }}><option value="">Choose a chat</option>{availableSessions.filter((session) => !editingJob || session.scopeId === scopeId).map((session) => <option key={`${session.scopeId}:${session.id}`} value={session.id}>{session.title}</option>)}</select></label>}
        <label className="scheduled-setup-field">Project<select value={draft.scopeId} disabled={Boolean(editingJob) || (automation.runMode === 'selected_session' && Boolean(automation.targetSessionId))} onChange={(event) => update('scopeId', event.target.value)}>{scopes.map((scope) => <option key={scope.id} value={scope.id}>{scope.label}</option>)}</select></label>
        <label className="scheduled-setup-field">Model<select required value={automation.model} onChange={(event) => {
          const model = availableModels.find((item) => item.reference === event.target.value);
          const compatible = model?.reasoning.options.some((option) => JSON.stringify(option.selection) === JSON.stringify(automation.reasoning));
          updateAutomation({ model: event.target.value, reasoning: compatible ? automation.reasoning : model?.reasoning.provider_default ?? { type: 'automatic' } });
          setNotice(compatible ? '' : 'Reasoning reset to the selected model’s default.');
        }}><option value="">Choose a model</option>{automation.model && !selectedModel && <option value={automation.model}>{automation.model} · unavailable</option>}{availableModels.map((model) => <option key={model.reference} value={model.reference}>{model.display_name} · {model.provider_label}</option>)}</select></label>
        <label className="scheduled-setup-field">Reasoning<select value={JSON.stringify(automation.reasoning)} onChange={(event) => updateAutomation({ reasoning: JSON.parse(event.target.value) as ReasoningSelectionDto })}>
          {!reasoningOptions.some((option) => JSON.stringify(option) === JSON.stringify(automation.reasoning)) && <option value={JSON.stringify(automation.reasoning)}>Saved selection</option>}
          {reasoningOptions.map((selection) => <option key={JSON.stringify(selection)} value={JSON.stringify(selection)}>{selection.type === 'level' ? selection.id : selection.type === 'token_budget' ? `${selection.tokens} tokens` : selection.type}</option>)}
        </select></label>
        <label className="scheduled-setup-field">Status<select value={automation.status} onChange={(event) => updateAutomation({ status: event.target.value as ScheduledAutomation['status'], statusReason: undefined })}><option value="active">Active</option><option value="paused">Paused</option><option value="completed">Completed</option></select></label>
      </div>
      {automation.statusReason && <p className="scheduled-setup-disclosure">{automation.statusReason}</p>}
      {draft.scopeId === 'global' && <p className="scheduled-setup-disclosure">No project: runs in a dedicated application workspace. Results appear in general chats.</p>}
      {notice && <p role="status" className="scheduled-setup-disclosure">{notice}</p>}
      <h3>Frequency</h3><div className="scheduled-setup-group">{timingFields()}{repeatField()}{expiryFields()}
        <label className="scheduled-setup-field">Notifications<select value={automation.notificationPolicy} onChange={(event) => updateAutomation({ notificationPolicy: event.target.value as ScheduledAutomation['notificationPolicy'] })}><option value="all">All runs</option><option value="failed">Failures only</option><option value="none">Off</option></select></label>
      </div>
      <p className="scheduled-setup-disclosure">Tasks run locally while LingXi is open and this computer is awake. Scheduled runs do not change your current chat or its model settings.</p>
      {error && <p role="alert" className="scheduled-setup-error">{error}</p>}
      <button type="submit" className="scheduled-setup-primary scheduled-setup-submit" disabled={!draft.title.trim() || !draft.instructions.trim() || !automation.model}>{saving ? 'Saving…' : editingJob ? 'Save changes' : 'Create task'}</button>
    </fieldset></form>
  </section>;
}
