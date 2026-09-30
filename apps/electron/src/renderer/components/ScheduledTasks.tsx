import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { UseBridge } from '../bridge/bridgeTypes.js';
import {
  scheduledTaskFromJob,
  scheduledTaskInput,
  formatScheduledTaskSchedule,
  type ScheduledCronJob,
  type ScheduledTaskDraft,
} from '../bridge/scheduledTaskDraft';
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


type ScopedJob = { job: ScheduledCronJob; scopeId: string };
type Initial = { title: string; schedule: string; description: string; draft?: ScheduledTaskDraft };
const taskKey = (item: ScopedJob) => `${item.scopeId}:${item.job.id}`;
export function ScheduledTasks({ bridge, visible, onOpenChat }: { bridge: UseBridge; visible: boolean; onOpenChat?(): void }) {
  const [jobs, setJobs] = useState<ScopedJob[]>([]);
  const [selected, setSelected] = useState<ScopedJob | null>(null);
  const [initial, setInitial] = useState<Initial | null>(null);
  const [editorVersion, setEditorVersion] = useState(0);
  const [filter, setFilter] = useState('all');
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [dirty, setDirty] = useState(false);
  const [pending, setPending] = useState<(() => void) | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [history, setHistory] = useState<Awaited<ReturnType<UseBridge['readScheduledHistory']>>>([]);
  const [showHistory, setShowHistory] = useState(false);
  const mutation = useRef(false);
  const dirtyRef = useRef(dirty);
  const selectedRef = useRef(selected);
  dirtyRef.current = dirty;
  selectedRef.current = selected;
  const version = useRef(0);
  const scopes = bridge.scheduledScopes;
  const scopesKey = scopes.map((scope) => scope.id).join('\n');
  const refresh = useCallback(async (quiet = false) => {
    if (mutation.current || (quiet && dirtyRef.current)) return;
    // `setError('')` is gated with the spinner: a quiet poll is not a user
    // action, so it must not erase the message a failed delete or save just put
    // on screen. Its own failures still call `setError` below.
    const current = ++version.current; if (!quiet) { setLoading(true); setError(''); }
    const results = await Promise.allSettled(scopes.map(async (scope) => (await bridge.manageScheduled(scope.id, { action: 'list' })).map((job) => ({ job, scopeId: scope.id }))));
    // A superseded refresh must still clear the spinner it raised: the newer
    // run owns the data, but this one owns the `loading` it set, and leaving it
    // true wedges the panel on "Loading scheduled tasks…" with Refresh disabled.
    if (current !== version.current) { setLoading(false); return; }
    if (quiet && (dirtyRef.current || mutation.current)) { setLoading(false); return; }
    const loaded = results.flatMap((result) => result.status === 'fulfilled' ? result.value : []);
    setJobs(loaded);
    const before = selectedRef.current;
    if (quiet && before) {
      const latest = loaded.find((item) => taskKey(item) === taskKey(before));
      if (latest && JSON.stringify(latest.job) !== JSON.stringify(before.job)) {
        setSelected(latest); setEditorVersion((value) => value + 1);
        setHistory(latest.job.automation?.runs ?? []);
      }
    }
    const failed = results.flatMap((result, index) => result.status === 'rejected' ? [`${scopes[index].label}: ${String(result.reason)}`] : []);
    if (failed.length) setError(failed.join(' · '));
    setLoading(false);
  }, [scopesKey, bridge.manageScheduled]);
  useEffect(() => { if (visible) void refresh(); return () => { version.current++; }; }, [visible, refresh]);
  useEffect(() => {
    if (!visible) return;
    const timer = window.setInterval(() => { void refresh(true); }, 15_000);
    return () => window.clearInterval(timer);
  }, [visible, refresh]);
  const choose = (action: () => void) => {
    if (busy) return;
    if (dirty) setPending(() => action); else action();
  };
  const close = () => { setSelected(null); setInitial(null); setDirty(false); setShowHistory(false); };
  const open = (item: ScopedJob | null, value?: Initial) => {
    setSelected(item); setInitial(value ?? null); setEditorVersion((value) => value + 1); setDirty(false); setShowHistory(false); setDeleting(false); setNotice('');
  };
  async function save(draft: ScheduledTaskDraft) {
    if (mutation.current) throw new Error('Wait for the current task change to finish.');
    const input = scheduledTaskInput(draft);
    const scopeId = selected?.scopeId ?? draft.scopeId ?? 'global';
    mutation.current = true; setBusy(true); version.current++;
    try {
      const result = await bridge.manageScheduled(scopeId, { ...input, action: selected ? 'update' : 'create', ...(selected ? { id: selected.job.id } : {}) });
      setJobs((previous) => [...previous.filter((item) => item.scopeId !== scopeId), ...result.map((job) => ({ job, scopeId }))]);
      const saved = selected ? result.find((job) => job.id === selected.job.id) : result.find((job) => !jobs.some((item) => item.scopeId === scopeId && item.job.id === job.id));
      if (saved) setSelected({ job: saved, scopeId });
      setInitial(null); setDirty(false); setError(''); setNotice('Task saved.');
    } finally { mutation.current = false; setBusy(false); setLoading(false); }
  }
  async function remove() {
    if (!selected || mutation.current) return;
    mutation.current = true; setBusy(true); version.current++;
    try {
      const result = await bridge.manageScheduled(selected.scopeId, { action: 'delete', id: selected.job.id });
      setJobs((previous) => [...previous.filter((item) => item.scopeId !== selected.scopeId), ...result.map((job) => ({ job, scopeId: selected.scopeId }))]); close(); setNotice('Task deleted.');
    } catch (cause) { setError(String(cause)); }
    // `version.current++` above aborts any in-flight refresh before its own
    // trailing setLoading(false) runs, so this mutation owns clearing it —
    // exactly as save() does.
    finally { mutation.current = false; setBusy(false); setLoading(false); }
  }
  async function loadHistory() {
    if (!selected) return;
    try { setHistory(await bridge.readScheduledHistory(selected.scopeId, selected.job.id)); setShowHistory(true); }
    catch (cause) { setError(String(cause)); }
  }
  const t = useT();
  const variables = { '--scheduled-bg': t.windowBg, '--scheduled-text': t.text, '--scheduled-secondary': t.text2,
    '--scheduled-muted': t.text3, '--scheduled-border': t.border, '--scheduled-hover': t.surfaceHover, '--scheduled-accent': t.accent } as CSSProperties;
  const query = search.trim().toLocaleLowerCase();
  const filtered = jobs.filter(({ job, scopeId }) => (filter === 'all' || (job.automation?.status ?? 'active') === filter)
    && `${job.prompt} ${job.cron} ${scopes.find((scope) => scope.id === scopeId)?.label}`.toLocaleLowerCase().includes(query));
  const sessions = Object.entries(bridge.bootstrap?.projectCatalogs ?? {}).flatMap(([scopeId, catalog]) => catalog.sessions.map((session) => ({ id: session.uuid, title: session.title || `Chat ${session.uuid.slice(0, 8)}`, scopeId })));
  const editing = Boolean(selected || initial);
  return <section className={`scheduled-tasks${editing ? ' scheduled-has-detail' : ''}`} style={variables} aria-label="Scheduled tasks">
    <div className="scheduled-master">
      <header className="scheduled-tasks-toolbar"><div className="scheduled-tabs" role="group" aria-label="Task status filter">{['all', 'active', 'paused', 'completed'].map((status) => <button type="button" key={status} aria-pressed={filter === status} onClick={() => setFilter(status)}>{status[0].toUpperCase() + status.slice(1)}</button>)}</div>
        <button type="button" className="scheduled-create-button" disabled={busy} onClick={() => choose(() => open(null, { title: '', schedule: '', description: '' }))}>Create <Icon name="plus" size={14} /></button>
      </header>
      <div className="scheduled-tasks-content">
        <div className="scheduled-search"><Icon name="search" size={19} /><input type="search" aria-label="Search scheduled tasks" placeholder="Search scheduled tasks" value={search} onChange={(event) => setSearch(event.target.value)} /></div>
        {error && <p role="alert" className="scheduled-empty">{error}</p>}{notice && <p role="status" className="scheduled-empty">{notice}</p>}
        <div className="scheduled-list-heading"><span>{filtered.length} tasks</span><button type="button" disabled={loading || busy} onClick={() => void refresh()}>Refresh</button></div>
        {loading && <p role="status" className="scheduled-empty">Loading scheduled tasks…</p>}
        <div className="scheduled-suggestions">{filtered.map((item) => {
          const { job } = item; const draft = scheduledTaskFromJob(job); const status = job.automation?.status ?? 'active'; const latest = job.automation?.runs?.at(-1);
          return <button type="button" key={taskKey(item)} className={`scheduled-suggestion scheduled-job${selected && taskKey(selected) === taskKey(item) ? ' selected' : ''}`} disabled={busy || job.permanent || job.durable === false} onClick={() => choose(() => open(item))}>
            <span className={`scheduled-dot scheduled-dot-${status}`} aria-label={status} />
            <span className="scheduled-suggestion-copy"><span>{draft.title}</span><span className="scheduled-suggestion-description">{formatScheduledTaskSchedule(draft)} · {scopes.find((scope) => scope.id === item.scopeId)?.label ?? 'None'}</span><span className="scheduled-suggestion-description">{job.automation?.statusReason ?? (job.next_run_at ? `Next run ${new Date(job.next_run_at).toLocaleString()}` : status === 'active' ? 'Scheduled locally' : status)}</span>{latest && <span className="scheduled-suggestion-description">Latest: {latest.status}{latest.error ? ` · ${latest.error}` : ''}</span>}</span>
          </button>;
        })}</div>
        {!loading && filtered.length === 0 && <p className="scheduled-empty">{query || filter !== 'all' ? 'No matching tasks.' : 'No scheduled tasks yet. Create one to get started.'}</p>}
        <h2 className="scheduled-suggestions-heading">Suggestions</h2><div className="scheduled-suggestions">{SUGGESTIONS.filter((suggestion) => `${suggestion.title} ${suggestion.description}`.toLocaleLowerCase().includes(query)).map((suggestion) => <button className="scheduled-suggestion" type="button" key={suggestion.title} disabled={busy} onClick={() => choose(() => open(null, suggestion))}><Icon name={suggestion.icon} size={20} color={suggestion.color} /><span className="scheduled-suggestion-copy"><span className="scheduled-suggestion-heading">{suggestion.title}<span className="scheduled-suggestion-schedule">{suggestion.schedule}</span></span><span className="scheduled-suggestion-description">{suggestion.description}</span></span></button>)}</div>
      </div>
    </div>
    {editing && <aside className="scheduled-detail">
      <ScheduledTaskSetup key={editorVersion} initial={initial ?? undefined} editingJob={selected?.job} scopeId={selected?.scopeId ?? 'global'} scopes={scopes} sessions={sessions} models={bridge.desktop.modelDetails} defaultModel={bridge.desktop.currentModel} defaultReasoning={bridge.desktop.conversationControls?.reasoning.requested} onBack={() => choose(close)} onSave={save} onDirtyChange={setDirty} loadContext={bridge.scheduledContext} />
      {selected && <div className="scheduled-detail-actions"><button type="button" disabled={busy} onClick={() => void loadHistory()}>Run history</button><button type="button" disabled={busy} onClick={() => choose(() => open(null, { title: '', schedule: '', description: '', draft: { ...scheduledTaskFromJob(selected.job), automation: selected.job.automation ? { version: 2, model: selected.job.automation.model, reasoning: selected.job.automation.reasoning, notificationPolicy: selected.job.automation.notificationPolicy, status: 'active', statusReason: undefined, runMode: 'new_session', targetSessionId: undefined, ownedSessionId: undefined } : undefined } }))}>Copy to project</button>{deleting ? <><button type="button" disabled={busy} onClick={() => void remove()}>Confirm delete</button><button type="button" onClick={() => setDeleting(false)}>Cancel</button></> : <button type="button" disabled={busy} onClick={() => setDeleting(true)}>Delete task</button>}</div>}
      {showHistory && <section className="scheduled-history" aria-label="Run history"><h3>Run history</h3>{history.length === 0 && <p>No runs yet.</p>}{history.map((run) => <article key={run.id}><strong>{run.status}</strong><time>{new Date(run.scheduledAt).toLocaleString()}</time><p>{run.model}</p>{run.startedAt && <p>Started {new Date(run.startedAt).toLocaleString()}{run.finishedAt ? ` · Finished ${new Date(run.finishedAt).toLocaleString()}` : ''}</p>}<p>{run.error ?? run.summary}</p>{run.sessionId && selected && <button type="button" onClick={() => void bridge.openScheduledSession(selected.scopeId, run.sessionId!).then(() => onOpenChat?.()).catch((cause) => setError(String(cause)))}>Open result</button>}</article>)}</section>}
    </aside>}
    {pending && <div className="scheduled-confirm" role="alertdialog" aria-modal="true" aria-label="Unsaved changes" onKeyDown={(event) => {
      if (event.key === 'Escape') { event.preventDefault(); setPending(null); }
      if (event.key === 'Tab') {
        const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>('button');
        if (event.shiftKey && document.activeElement === buttons[0]) { event.preventDefault(); buttons[buttons.length - 1]?.focus(); }
        else if (!event.shiftKey && document.activeElement === buttons[buttons.length - 1]) { event.preventDefault(); buttons[0]?.focus(); }
      }
    }}><div><h2>Discard unsaved changes?</h2><p>Your task has changes that have not been saved.</p><button type="button" autoFocus onClick={() => setPending(null)}>Keep editing</button><button type="button" onClick={() => { pending(); setPending(null); }}>Discard changes</button></div></div>}
  </section>;
}
