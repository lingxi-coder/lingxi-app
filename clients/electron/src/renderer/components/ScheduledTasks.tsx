import { useCallback, useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import type { UseBridge } from '../bridge/useBridge';
import { scheduledTaskFromJob, scheduledTaskInput, formatScheduledTaskSchedule, type ScheduledCronJob, type ScheduledTaskDraft } from '../bridge/scheduledTaskDraft';
import { useT } from '../theme/ThemeContext';
import { ScheduledTaskSetup } from './ScheduledTaskSetup';
import { Icon } from './Icon';
import './ScheduledTasks.css';

const SUGGESTIONS = [
  {
    title: 'Daily brief', schedule: 'Weekdays at 8:00 AM', icon: 'bell', color: '#488bff',
    description: 'Start each weekday with a summary of your calendar, unread email, and priorities',
  },
  {
    title: 'Weekly review', schedule: 'Fridays at 4:00 PM', icon: 'notebook', color: '#a45bff',
    description: 'Turn your recent work into a concise status update every Friday',
  },
  {
    title: 'Follow-up monitor', schedule: 'Weekdays at 9:00 AM', icon: 'fileSearch', color: '#12ad52',
    description: 'Review recent email and calendar activity and flag anything that needs your attention',
  },
];

export function ScheduledTasks({ bridge, visible }: { bridge: UseBridge; visible: boolean }) {
  const projectPath = bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path;
  const available = Boolean(bridge.connected && bridge.activeSession && !bridge.sessionLoading);
  const [jobs, setJobs] = useState<ScheduledCronJob[]>([]);
  const [editingJob, setEditingJob] = useState<ScheduledCronJob | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [deleting, setDeleting] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const requestVersion = useRef(0);
  const mutationInFlight = useRef(false);
  const refresh = useCallback(async () => {
    if (!available || mutationInFlight.current) { setLoading(false); return; }
    const version = ++requestVersion.current;
    setLoading(true);
    setError('');
    try { const result = await bridge.manageCron({ action: 'list' }); if (version === requestVersion.current) setJobs(result); }
    catch (cause) { if (version === requestVersion.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (version === requestVersion.current) setLoading(false); }
  }, [available, bridge.manageCron]);
  useEffect(() => {
    if (visible) void refresh();
    return () => { requestVersion.current++; };
  }, [visible, refresh]);
  async function save(draft: ScheduledTaskDraft) {
    if (!available) throw new Error('Open a connected project before saving a scheduled task.');
    if (mutationInFlight.current) throw new Error('Wait for the current task change to finish.');
    const input = scheduledTaskInput(draft);
    mutationInFlight.current = true;
    setBusy(true);
    requestVersion.current++;
    setLoading(false);
    try {
      const result = await bridge.manageCron({ ...input, action: editingJob ? 'update' : 'create', ...(editingJob ? { id: editingJob.id } : {}) });
      requestVersion.current++;
      setJobs(result);
      setError('');
      setNotice(editingJob ? 'Task updated.' : 'Task created.');
      setDraft(null);
      setEditingJob(null);
    } finally {
      mutationInFlight.current = false;
      setBusy(false);
      setLoading(false);
    }
  }

  async function remove(id: string) {
    if (mutationInFlight.current || !available) return;
    mutationInFlight.current = true;
    requestVersion.current++;
    setBusy(true);
    setError('');
    try { const result = await bridge.manageCron({ action: 'delete', id }); requestVersion.current++; setJobs(result); setDeleting(null); setNotice('Task deleted.'); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { mutationInFlight.current = false; setBusy(false); setLoading(false); }
  }
  const t = useT();
  const id = useId();
  const [search, setSearch] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
  const [draft, setDraft] = useState<{ title: string; schedule: string; description: string } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const closeOutside = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('pointerdown', closeOutside);
    return () => document.removeEventListener('pointerdown', closeOutside);
  }, [menuOpen]);

  const openDraft = (value = { title: '', schedule: '', description: '' }) => {
    if (!available || mutationInFlight.current) return;
    setMenuOpen(false);
    setNotice('');
    setEditingJob(null);
    setDraft(value);
  };
  const query = search.trim().toLocaleLowerCase();
  const suggestions = SUGGESTIONS.filter((suggestion) =>
    `${suggestion.title} ${suggestion.schedule} ${suggestion.description}`.toLocaleLowerCase().includes(query));
  const variables = {
    '--scheduled-bg': t.windowBg, '--scheduled-text': t.text, '--scheduled-secondary': t.text2,
    '--scheduled-muted': t.text3, '--scheduled-border': t.border, '--scheduled-hover': t.surfaceHover,
    '--scheduled-accent': t.accent,
  } as CSSProperties;

  const ownerSessionId = editingJob?.session_id ?? (!editingJob ? bridge.activeSession?.sessionId : undefined);
  const ownerTitle = ownerSessionId ? bridge.bootstrap?.projectCatalogs?.[projectPath ?? '']?.sessions.find((session) => session.uuid === ownerSessionId)?.title ?? `Chat ${ownerSessionId.slice(0, 8)}` : 'Project-wide task';

  if (draft || editingJob) return (
    <section className="scheduled-tasks" style={variables} aria-label="Set up a scheduled task">
      <ScheduledTaskSetup initial={draft ?? undefined} editingJob={editingJob ?? undefined} projectPath={projectPath} sessionTitle={ownerTitle} onBack={() => {
        setDraft(null);
        setEditingJob(null);
        window.requestAnimationFrame(() => menuButtonRef.current?.focus());
      }} onSave={save} />
    </section>
  );

  return (
    <section className="scheduled-tasks" style={variables} aria-labelledby={`${id}-heading`}>
      <div className="scheduled-tasks-toolbar">
        <div className="scheduled-create" ref={menuRef} onKeyDown={(event) => {
          if (event.key === 'Escape') { setMenuOpen(false); menuButtonRef.current?.focus(); }
        }}>
          <button ref={menuButtonRef} type="button" className="scheduled-create-button"
            disabled={!available || busy} aria-expanded={menuOpen} aria-controls={`${id}-create-options`} onClick={() => setMenuOpen(!menuOpen)}>
            Create <Icon name="chevron" size={14} />
          </button>
          {menuOpen && <div className="scheduled-create-options" id={`${id}-create-options`}>
            <button type="button" onClick={() => openDraft()}><Icon name="plus" /> New scheduled task</button>
            {SUGGESTIONS.map((suggestion) => <button key={suggestion.title} type="button" onClick={() => openDraft(suggestion)}>
              <Icon name={suggestion.icon} color={suggestion.color} />{suggestion.title}
            </button>)}
          </div>}
        </div>
      </div>
      <div className="scheduled-tasks-content">
        <h1 id={`${id}-heading`}>Scheduled tasks</h1>
        <p className="scheduled-tasks-subtitle">Schedule tasks, set reminders, or monitor for updates in {projectPath?.split('/').filter(Boolean).pop() ?? 'your project'}</p>
        {!available && <p role="status" className="scheduled-empty">Open a connected project to manage scheduled tasks.</p>}
        {error && <p role="alert" className="scheduled-empty">{error}</p>}
        {notice && <p role="status" className="scheduled-empty">{notice}</p>}
        <div className="scheduled-search">
          <Icon name="search" size={19} />
          <input type="search" aria-label="Search scheduled tasks" placeholder="Search scheduled tasks"
            value={search} onChange={(event) => setSearch(event.target.value)} />
        </div>
        <div className="scheduled-list-heading"><h2 className="scheduled-suggestions-heading">Tasks</h2>
          <button type="button" disabled={!available || loading || busy} onClick={() => void refresh()}>Refresh</button></div>
        {loading && <p role="status" className="scheduled-empty">Loading scheduled tasks…</p>}
        <div className="scheduled-suggestions">
          {jobs.filter((job) => `${job.prompt} ${job.cron}`.toLocaleLowerCase().includes(query)).map((job) => {
            const details = scheduledTaskFromJob(job);
            return <div key={job.id} className="scheduled-job-row">
              <button type="button" className="scheduled-suggestion" disabled={!available || busy || job.permanent || job.durable === false} onClick={() => { setDraft(null); setEditingJob(job); }}>
                <span aria-hidden="true"><Icon name="clock" size={21} /></span>
                <span className="scheduled-suggestion-copy"><span>{details.title}</span>
                  <span className="scheduled-suggestion-description">{formatScheduledTaskSchedule(details)} · {job.recurring ? 'Repeats' : 'Runs once'}{job.expires_at ? ` · ${job.expires_at <= Date.now() ? 'Expired' : 'Expires'} ${new Date(job.expires_at).toLocaleString()}` : ' · No expiry'}{job.permanent ? ' · System task' : ''}</span>
                </span>
              </button>
              {deleting === job.id ? <div className="scheduled-job-actions">
                <button type="button" disabled={busy} onClick={() => void remove(job.id)}>Confirm delete</button>
                <button type="button" disabled={busy} onClick={() => setDeleting(null)}>Cancel</button>
              </div> : <button type="button" disabled={!available || busy || job.permanent || job.durable === false} aria-label={`Delete ${details.title}`} onClick={() => setDeleting(job.id)}>Delete</button>}
            </div>;
          })}
          {!loading && available && !error && jobs.length === 0 && <p className="scheduled-empty">No scheduled tasks yet. Create one to get started.</p>}
          {!loading && jobs.length > 0 && !jobs.some((job) => `${job.prompt} ${job.cron}`.toLocaleLowerCase().includes(query)) && <p className="scheduled-empty">No tasks match your search.</p>}
        </div>
        <h2 className="scheduled-suggestions-heading">Suggestions</h2>
        <div className="scheduled-suggestions">
          {suggestions.map((suggestion) => <button className="scheduled-suggestion" type="button"
            key={suggestion.title} disabled={!available || busy} onClick={() => openDraft(suggestion)}>
            <span className="scheduled-suggestion-icon" aria-hidden="true"><Icon name={suggestion.icon} size={21} color={suggestion.color} /></span>
            <span className="scheduled-suggestion-copy">
              <span className="scheduled-suggestion-heading"><span>{suggestion.title}</span><span className="scheduled-suggestion-schedule">{suggestion.schedule}</span></span>
              <span className="scheduled-suggestion-description">{suggestion.description}</span>
            </span>
          </button>)}
          {suggestions.length === 0 && <p className="scheduled-empty" role="status">No suggestions match “{search.trim()}”. Try another search or create a task.</p>}
        </div>
      </div>

    </section>
  );
}
