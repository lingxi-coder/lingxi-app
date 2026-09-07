import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { Icon } from './Icon';
import './ScheduledTaskSetup.css';

import { formatScheduledTaskSchedule as scheduleSummary, localScheduledTimezone, localExpiryInput, scheduledTaskCron, scheduledTaskAdvice, scheduledTaskFromJob, type ScheduledCronJob, type ScheduledTaskDraft as Draft, type ScheduledTaskFrequency as Frequency } from '../bridge/scheduledTaskDraft';

type Message = { role: 'user' | 'assistant'; text: string };
const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
const QUESTIONS = [
  'First, what would you like LingXi to take care of automatically?',
  'What should the task cover, and what would you like included in the result?',
  'When should it run? Choose the frequency and local time.',
  'Should this task repeat, and when should it end?',
];
const INTRO = "Let's set up a scheduled task together. First, explain how scheduled tasks work. Then guide me through what I need scheduled and when it should run.";

function initialDraft(initial?: { title: string; schedule: string; description: string }): Draft {
  const weekly = initial?.schedule.startsWith('Fridays');
  return {
    title: initial?.title ?? '', instructions: initial?.description ?? '',
    frequency: weekly ? 'Weekly' : 'Weekdays', day: weekly ? 'Friday' : 'Monday',
    time: weekly ? '16:00' : initial?.title === 'Follow-up monitor' ? '09:00' : '08:00',
    timezone: localScheduledTimezone(), recurring: true, durable: true,
  };
}

export function ScheduledTaskSetup({ projectPath, sessionTitle, initial, editingJob, onBack, onSave }: {
  projectPath?: string;
  sessionTitle?: string;
  initial?: { title: string; schedule: string; description: string };
  onBack: () => void;
  editingJob?: ScheduledCronJob;
  onSave: (draft: Draft) => Promise<void>;
}) {
  const id = useId();
  const [step, setStep] = useState(editingJob ? 4 : 0);
  const [draft, setDraft] = useState<Draft>(() => editingJob ? scheduledTaskFromJob(editingJob) : initialDraft(initial));
  const [purpose, setPurpose] = useState(initial?.description ?? '');
  const [scope, setScope] = useState(projectPath ? `Cover ${projectPath.split('/').filter(Boolean).pop()}. Summarize completed work, current progress, and next steps.` : '');
  const [messages, setMessages] = useState<Message[]>([]);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const currentRef = useRef<HTMLDivElement>(null);
  const ready = step === 4;
  const advice = scheduledTaskAdvice(draft);
  const update = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((previous) => ({ ...previous, [key]: value }));

  useEffect(() => {
    currentRef.current?.scrollIntoView?.({ block: 'nearest', behavior: 'smooth' });
    currentRef.current?.querySelector<HTMLElement>('textarea, select, input')?.focus();
  }, [step]);

  function advance(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (step === 0 && !purpose.trim()) return;
    if (step === 1 && !scope.trim()) return;
    const answer = step === 0 ? purpose.trim() : step === 1 ? scope.trim() : step === 2 ? scheduleSummary(draft) : draft.recurring ? `Repeat on schedule · ${draft.expiresAt ? `Until ${draft.expiresAt}` : 'No expiry'}` : 'Run once at the next scheduled time';
    setMessages((previous) => [...previous, { role: 'assistant', text: QUESTIONS[step] }, { role: 'user', text: answer }]);
    if (step === 1) {
      setDraft((previous) => ({ ...previous,
        title: previous.title || purpose.trim().split('\n')[0].slice(0, 90),
        instructions: `${purpose.trim()}\n\n${scope.trim()}${projectPath ? `\n\nProject: ${projectPath}` : ''}`,
      }));
    }
    setStep((previous) => previous + 1);
  }

  async function saveTask(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (savingRef.current || !draft.title.trim() || !draft.instructions.trim()) return;
    savingRef.current = true;
    setSaving(true);
    setError('');
    try { await onSave(draft); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { savingRef.current = false; setSaving(false); }
  }

  function timingFields() {
    return <>
      <label className="scheduled-setup-field">Repeat<select value={draft.frequency} onChange={(event) => update('frequency', event.target.value as Frequency)}>
        <option>Daily</option><option>Weekdays</option><option>Weekly</option><option>Custom</option>
      </select></label>
      {draft.frequency === 'Weekly' && <label className="scheduled-setup-field">On<select value={draft.day} onChange={(event) => update('day', event.target.value)}>{DAYS.map((day) => <option key={day}>{day}</option>)}</select></label>}
      {draft.frequency !== 'Custom' && <label className="scheduled-setup-field">At<input required type="time" value={draft.time} onChange={(event) => update('time', event.target.value)} /></label>}
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
      {draft.expiresAt !== undefined && <label className="scheduled-setup-field">End date<input required type="datetime-local" value={draft.expiresAt} onChange={(event) => update('expiresAt', event.target.value)} /></label>}
      <p className="scheduled-setup-disclosure" style={{ marginTop: 12 }}>{advice.text}</p>
      {advice.suggestedDays && <button type="button" className="scheduled-setup-back" style={{ marginRight: 14, marginBottom: 12 }} onClick={() => update('expiresAt', localExpiryInput(Date.now() + advice.suggestedDays! * 86400000))}>Use suggested expiry</button>}
      {advice.suggestSlower && <button type="button" className="scheduled-setup-back" style={{ marginBottom: 12 }} onClick={() => setDraft((previous) => ({ ...previous, frequency: 'Custom', cron: `*/15 ${scheduledTaskCron(previous).split(/\s+/).slice(1).join(' ')}` }))}>Use a 15-minute interval</button>}
    </>;
  }

  return <section className={`scheduled-setup${ready ? ' scheduled-setup-ready' : ''}`} aria-label="Set up scheduled task">
    <header className="scheduled-setup-header">
      <button type="button" className="scheduled-setup-back" disabled={saving} onClick={onBack}><span aria-hidden="true">←</span> Back to scheduled</button>
      <span className="scheduled-setup-status"><Icon name="clock" size={14} />{editingJob ? 'Scheduled task' : 'New task'}</span>
    </header>
    <div className="scheduled-setup-workspace">
      <div className="scheduled-setup-conversation">
        <div className="scheduled-setup-transcript">
          <div className="scheduled-setup-message scheduled-setup-user">{INTRO}</div>
          <div className="scheduled-setup-message scheduled-setup-assistant">
            <h1>Let’s set up your scheduled task</h1>
            <p>A scheduled task runs saved instructions at a time you choose, such as preparing a daily briefing, reviewing a project, or checking for updates.</p>
            <p>You define <strong>what it should do</strong>, <strong>when it should run</strong>, and <strong>what results to report</strong>. We’ll work through these one step at a time, then you can edit the full task details.</p>
            <p className="scheduled-setup-disclosure">Tasks run locally while this project’s LingXi runtime is open. Keep the computer awake at the scheduled time.</p>
          </div>
          {messages.map((message, index) => <div key={index} className={`scheduled-setup-message scheduled-setup-${message.role}`}>{message.text}</div>)}
          <div ref={currentRef} className="scheduled-setup-current">
            {!ready ? <form onSubmit={advance}>
              <p className="scheduled-setup-progress">Step {step + 1} of 4</p>
              <h2 id={`${id}-question`}>{QUESTIONS[step]}</h2>
              {step === 0 && <><p className="scheduled-setup-hint">For example, a morning briefing, a weekly project summary, a reminder, or monitoring something for changes.</p>
                <textarea aria-labelledby={`${id}-question`} required rows={3} value={purpose} onChange={(event) => setPurpose(event.target.value)} placeholder="A weekly project summary…" /></>}
              {step === 1 && <textarea aria-labelledby={`${id}-question`} required rows={3} value={scope} onChange={(event) => setScope(event.target.value)} placeholder="Describe the scope, sources, and the result you want…" />}
              {step === 2 && <div className="scheduled-setup-group">{timingFields()}</div>}
              {step === 3 && <div className="scheduled-setup-group">{repeatField()}{expiryFields()}</div>}
              <div className="scheduled-setup-actions"><button className="scheduled-setup-primary" type="submit" disabled={step === 0 ? !purpose.trim() : step === 1 ? !scope.trim() : false}>{step === 3 ? 'Review task details' : 'Continue'}<span aria-hidden="true">→</span></button></div>
            </form> : <div className="scheduled-setup-message scheduled-setup-assistant"><h2>Your task draft is ready</h2><p>Review and edit the details, then save the task to the project scheduler.</p></div>}
          </div>
        </div>
      </div>
      {ready && <aside className="scheduled-setup-details" aria-label="Editable task details">
        <form onSubmit={saveTask}><fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
          <div className="scheduled-setup-details-heading"><Icon name="clock" size={20} /><h2>Task details</h2></div>
          <label className="scheduled-setup-title-label" htmlFor={`${id}-title`}>Task name</label>
          <input className="scheduled-setup-title" id={`${id}-title`} required value={draft.title} onChange={(event) => update('title', event.target.value)} />
          <label className="scheduled-setup-title-label" htmlFor={`${id}-instructions`}>Instructions</label>
          <textarea id={`${id}-instructions`} className="scheduled-setup-instructions" required rows={8} value={draft.instructions} onChange={(event) => update('instructions', event.target.value)} />
          <h3>Details</h3>
          <div className="scheduled-setup-group"><div className="scheduled-setup-field"><span>Associated chat</span><span title={projectPath}>{sessionTitle ?? 'Current chat'}</span></div></div>
          <h3>Frequency</h3>
          <div className="scheduled-setup-group">{timingFields()}{repeatField()}{expiryFields()}</div>
          <p className="scheduled-setup-disclosure">Saved tasks run in this project’s runtime using the system time zone. Recurring runs may be delayed by up to 30 minutes. Tasks keep running until their chosen expiry or until you remove them.</p>
          {error && <p role="alert" className="scheduled-setup-error">{error}</p>}
          <button type="submit" className="scheduled-setup-primary scheduled-setup-submit" disabled={!draft.title.trim() || !draft.instructions.trim()}>{saving ? 'Saving…' : editingJob ? 'Save changes' : 'Create task'}<span aria-hidden="true">→</span></button>
        </fieldset></form>
      </aside>}
    </div>
  </section>;
}
