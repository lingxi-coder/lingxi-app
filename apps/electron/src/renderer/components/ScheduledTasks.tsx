import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { UseBridge } from '../bridge/bridgeTypes.js';
import {
  scheduledTaskFromJob,
  scheduledTaskInput,
  type ScheduledCronJob,
  type ScheduledTaskDraft,
} from '../bridge/scheduledTaskDraft';
import { currentLocale, tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';
import type { ScheduledLinkRequest } from '../deepLinkRoutes';
import { ScheduledTaskSetup } from './ScheduledTaskSetup';
import { Icon } from './Icon';
import './ScheduledTasks.css';

const SUGGESTIONS = [
  { titleKey: 'desktop_scheduled_brief_title', scheduleKey: 'desktop_scheduled_brief_schedule', descriptionKey: 'desktop_scheduled_brief_description', icon: 'bell', color: '#488bff', preset: { frequency: 'Weekdays', day: 'Monday', time: '08:00' } },
  { titleKey: 'desktop_scheduled_review_title', scheduleKey: 'desktop_scheduled_review_schedule', descriptionKey: 'desktop_scheduled_review_description', icon: 'notebook', color: '#a45bff', preset: { frequency: 'Weekly', day: 'Friday', time: '16:00' } },
  { titleKey: 'desktop_scheduled_followup_title', scheduleKey: 'desktop_scheduled_followup_schedule', descriptionKey: 'desktop_scheduled_followup_description', icon: 'fileSearch', color: '#12ad52', preset: { frequency: 'Weekdays', day: 'Monday', time: '09:00' } },
] as const;


type ScopedJob = { job: ScheduledCronJob; scopeId: string };
type Initial = { title: string; schedule: string; description: string; draft?: ScheduledTaskDraft; preset?: Pick<ScheduledTaskDraft, 'frequency' | 'day' | 'time'> };
const taskKey = (item: ScopedJob) => `${item.scopeId}:${item.job.id}`;
export function ScheduledTasks({ bridge, visible, onOpenChat, initialTarget }: { bridge: UseBridge; visible: boolean; onOpenChat?(): void; initialTarget?: ScheduledLinkRequest }) {
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
  const consumedTarget = useRef<string>();
  const [targetRunId, setTargetRunId] = useState<string>();
  useEffect(() => {
    if (!visible || !initialTarget || consumedTarget.current === initialTarget.requestId || !scopes.length) return;
    let cancelled = false;
    const target = initialTarget;
    void (async () => {
      const owners = target.scope ? scopes.filter(scope => scope.id === target.scope) : scopes;
      if (!owners.length) throw new Error('desktop_scheduled_scope_unavailable');
      const lists = await Promise.all(owners.map(async scope => (await bridge.manageScheduled(scope.id, { action: 'list' })).map(job => ({ job, scopeId: scope.id }))));
      const all = lists.flat();
      const matches: { item: ScopedJob; runs?: typeof history }[] = [];
      for (const item of all) {
        if (target.kind === 'job') { if (item.job.id === target.id) matches.push({ item }); }
        else {
          const runs = await bridge.readScheduledHistory(item.scopeId, item.job.id);
          if (runs.some(run => run.id === target.id)) matches.push({ item, runs });
        }
      }
      if (cancelled) return;
      consumedTarget.current = target.requestId;
      if (matches.length !== 1) throw new Error(matches.length ? 'desktop_scheduled_scope_ambiguous' : 'cron_link_missing');
      const match = matches[0]!;
      const navigate = () => {
        open(match.item);
        setTargetRunId(target.kind === 'run' ? target.id : undefined);
        if (match.runs) { setHistory(match.runs); setShowHistory(true); }
      };
      if (dirtyRef.current) setPending(() => navigate); else navigate();
    })().catch(cause => { if (!cancelled) { consumedTarget.current = target.requestId; setError(cause instanceof Error ? cause.message : String(cause)); } });
    return () => { cancelled = true; };
  }, [visible, initialTarget?.requestId, scopesKey, bridge.manageScheduled, bridge.readScheduledHistory]);
  useEffect(() => {
    if (showHistory && targetRunId) requestAnimationFrame(() => document.getElementById(`scheduled-run-${targetRunId}`)?.scrollIntoView({ block: 'center' }));
  }, [showHistory, targetRunId]);
  async function save(draft: ScheduledTaskDraft) {
    if (mutation.current) throw new Error('desktop_scheduled_wait_change');
    const input = scheduledTaskInput(draft);
    const scopeId = selected?.scopeId ?? draft.scopeId ?? 'global';
    mutation.current = true; setBusy(true); version.current++;
    try {
      const result = await bridge.manageScheduled(scopeId, { ...input, action: selected ? 'update' : 'create', ...(selected ? { id: selected.job.id } : {}) });
      setJobs((previous) => [...previous.filter((item) => item.scopeId !== scopeId), ...result.map((job) => ({ job, scopeId }))]);
      const saved = selected ? result.find((job) => job.id === selected.job.id) : result.find((job) => !jobs.some((item) => item.scopeId === scopeId && item.job.id === job.id));
      if (saved) setSelected({ job: saved, scopeId });
      setInitial(null); setDirty(false); setError(''); setNotice('cron_task_updated');
    } finally { mutation.current = false; setBusy(false); setLoading(false); }
  }
  async function remove() {
    if (!selected || mutation.current) return;
    mutation.current = true; setBusy(true); version.current++;
    try {
      const result = await bridge.manageScheduled(selected.scopeId, { action: 'delete', id: selected.job.id });
      setJobs((previous) => [...previous.filter((item) => item.scopeId !== selected.scopeId), ...result.map((job) => ({ job, scopeId: selected.scopeId }))]); close(); setNotice('cron_task_deleted_message');
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
  const sessions = Object.entries(bridge.bootstrap?.projectCatalogs ?? {}).flatMap(([scopeId, catalog]) => catalog.sessions.map((session) => ({ id: session.uuid, title: session.title || `${tr('drawer_tab_chat')} ${session.uuid.slice(0, 8)}`, scopeId })));
  const suggestions = SUGGESTIONS.map((item) => ({ ...item, title: tr(item.titleKey), schedule: tr(item.scheduleKey), description: tr(item.descriptionKey) }));
  const editing = Boolean(selected || initial);
  return <section className={`scheduled-tasks${editing ? ' scheduled-has-detail' : ''}`} style={variables} aria-label={tr('settings_title_cron')}>
    <div className="scheduled-master">
      <header className="scheduled-tasks-toolbar"><div className="scheduled-tabs" role="group" aria-label={tr('desktop_scheduled_filter_aria')}>{['all', 'active', 'paused', 'completed'].map((status) => <button type="button" key={status} aria-pressed={filter === status} onClick={() => setFilter(status)}>{taskStatusLabel(status)}</button>)}</div>
        <button type="button" className="scheduled-create-button" disabled={busy} onClick={() => choose(() => open(null, { title: '', schedule: '', description: '' }))}>{tr('common_create')} <Icon name="plus" size={14} /></button>
      </header>
      <div className="scheduled-tasks-content">
        <div className="scheduled-search"><Icon name="search" size={19} /><input type="search" aria-label={tr('cron_search_tasks')} placeholder={tr('cron_search_tasks')} value={search} onChange={(event) => setSearch(event.target.value)} /></div>
        {error && <p role="alert" className="scheduled-empty">{scheduledErrorLabel(error)}</p>}{notice && <p role="status" className="scheduled-empty">{tr(notice)}</p>}
        <div className="scheduled-list-heading"><span>{tr('cron_task_count_header_fmt', filtered.length)}</span><button type="button" disabled={loading || busy} onClick={() => void refresh()}>{tr('settings_linux_refresh_button')}</button></div>
        {loading && <p role="status" className="scheduled-empty">{tr('desktop_scheduled_loading')}</p>}
        <div className="scheduled-suggestions">{filtered.map((item) => {
          const { job } = item; const draft = scheduledTaskFromJob(job); const status = job.automation?.status ?? 'active'; const latest = job.automation?.runs?.at(-1);
          return <button type="button" key={taskKey(item)} className={`scheduled-suggestion scheduled-job${selected && taskKey(selected) === taskKey(item) ? ' selected' : ''}`} disabled={busy || job.permanent || job.durable === false} onClick={() => choose(() => open(item))}>
            <span className={`scheduled-dot scheduled-dot-${status}`} aria-label={taskStatusLabel(status)} />
            <span className="scheduled-suggestion-copy"><span>{draft.title}</span><span className="scheduled-suggestion-description">{scheduleLabel(draft)} · {scopes.find((scope) => scope.id === item.scopeId)?.label ?? tr('common_none')}</span><span className="scheduled-suggestion-description">{job.automation?.statusReason ?? (job.next_run_at ? tr('desktop_scheduled_next_run', new Date(job.next_run_at).toLocaleString(currentLocale())) : status === 'active' ? tr('desktop_scheduled_local') : taskStatusLabel(status))}</span>{latest && <span className="scheduled-suggestion-description">{tr('desktop_scheduled_latest', runStatusLabel(latest.status))}{latest.error ? ` · ${latest.error}` : ''}</span>}</span>
          </button>;
        })}</div>
        {!loading && filtered.length === 0 && <p className="scheduled-empty">{query || filter !== 'all' ? tr('desktop_scheduled_no_matches') : `${tr('cron_no_tasks_yet')} ${tr('cron_empty_help')}`}</p>}
        <h2 className="scheduled-suggestions-heading">{tr('settings_section_suggestions')}</h2><div className="scheduled-suggestions">{suggestions.filter((suggestion) => `${suggestion.title} ${suggestion.description}`.toLocaleLowerCase().includes(query)).map((suggestion) => <button className="scheduled-suggestion" type="button" key={suggestion.title} disabled={busy} onClick={() => choose(() => open(null, suggestion))}><Icon name={suggestion.icon} size={20} color={suggestion.color} /><span className="scheduled-suggestion-copy"><span className="scheduled-suggestion-heading">{suggestion.title}<span className="scheduled-suggestion-schedule">{suggestion.schedule}</span></span><span className="scheduled-suggestion-description">{suggestion.description}</span></span></button>)}</div>
      </div>
    </div>
    {editing && <aside className="scheduled-detail">
      <ScheduledTaskSetup key={editorVersion} initial={initial ?? undefined} editingJob={selected?.job} scopeId={selected?.scopeId ?? 'global'} scopes={scopes} sessions={sessions} models={bridge.desktop.modelDetails} defaultModel={bridge.desktop.currentModel} defaultReasoning={bridge.desktop.conversationControls?.reasoning.requested} onBack={() => choose(close)} onSave={save} onDirtyChange={setDirty} loadContext={bridge.scheduledContext} />
      {selected && <div className="scheduled-detail-actions"><button type="button" disabled={busy} onClick={() => void loadHistory()}>{tr('cron_run_history_section')}</button><button type="button" disabled={busy} onClick={() => choose(() => open(null, { title: '', schedule: '', description: '', draft: { ...scheduledTaskFromJob(selected.job), automation: selected.job.automation ? { version: 2, model: selected.job.automation.model, reasoning: selected.job.automation.reasoning, notificationPolicy: selected.job.automation.notificationPolicy, status: 'active', statusReason: undefined, runMode: 'new_session', targetSessionId: undefined, ownedSessionId: undefined } : undefined } }))}>{tr('cron_copy_project')}</button>{deleting ? <><button type="button" disabled={busy} onClick={() => void remove()}>{tr('desktop_scheduled_confirm_delete')}</button><button type="button" onClick={() => setDeleting(false)}>{tr('common_cancel')}</button></> : <button type="button" disabled={busy} onClick={() => setDeleting(true)}>{tr('common_delete')}</button>}</div>}
      {showHistory && <section className="scheduled-history" aria-label={tr('cron_run_history_section')}><h3>{tr('cron_run_history_section')}</h3>{history.length === 0 && <p>{tr('desktop_scheduled_no_runs')}</p>}{history.map((run) => <article key={run.id} id={`scheduled-run-${run.id}`} aria-current={run.id === targetRunId ? 'true' : undefined} style={run.id === targetRunId ? { outline: '2px solid var(--scheduled-accent)', outlineOffset: 4 } : undefined}><strong>{runStatusLabel(run.status)}</strong><time>{new Date(run.scheduledAt).toLocaleString(currentLocale())}</time><p>{run.model}</p>{run.startedAt && <p>{tr('cron_started_at_label')}: {new Date(run.startedAt).toLocaleString(currentLocale())}{run.finishedAt ? ` · ${tr('cron_finished_at_label')}: ${new Date(run.finishedAt).toLocaleString(currentLocale())}` : ''}</p>}<p>{run.error ?? run.summary}</p>{run.sessionId && selected && <button type="button" onClick={() => void bridge.openScheduledSession(selected.scopeId, run.sessionId!).then(() => onOpenChat?.()).catch((cause) => setError(String(cause)))}>{tr('cron_open_chat')}</button>}</article>)}</section>}
    </aside>}
    {pending && <div className="scheduled-confirm" role="alertdialog" aria-modal="true" aria-label={tr('cron_discard_title')} onKeyDown={(event) => {
      if (event.key === 'Escape') { event.preventDefault(); setPending(null); }
      if (event.key === 'Tab') {
        const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>('button');
        if (event.shiftKey && document.activeElement === buttons[0]) { event.preventDefault(); buttons[buttons.length - 1]?.focus(); }
        else if (!event.shiftKey && document.activeElement === buttons[buttons.length - 1]) { event.preventDefault(); buttons[0]?.focus(); }
      }
    }}><div><h2>{tr('cron_discard_title')}</h2><p>{tr('desktop_scheduled_dirty_body')}</p><button type="button" autoFocus onClick={() => setPending(null)}>{tr('cron_keep_editing')}</button><button type="button" onClick={() => { pending(); setPending(null); }}>{tr('cron_discard')}</button></div></div>}
  </section>;
}

function taskStatusLabel(status: string): string {
  const keys: Record<string, string> = { all: 'drawer_filter_all', active: 'cron_task_status_active', paused: 'chat_status_paused', completed: 'chat_status_completed' };
  return keys[status] ? tr(keys[status]!) : status;
}
function runStatusLabel(status: string): string {
  const keys: Record<string, string> = { succeeded: 'cron_status_succeeded', completed: 'chat_status_completed', failed: 'chat_status_failed', cancelled: 'chat_status_cancelled', started: 'chat_status_running', running: 'chat_status_running', queued: 'cron_status_queued', interrupted: 'chat_result_interrupted' };
  return keys[status] ? tr(keys[status]!) : status;
}
function scheduleLabel(draft: ScheduledTaskDraft): string {
  if (draft.frequency === 'Custom') return `${draft.cron} (${draft.timezone})`;
  if (draft.frequency === 'Weekly') {
    const index = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'].indexOf(draft.day);
    const day = index < 0 ? draft.day : new Intl.DateTimeFormat(currentLocale(), { weekday: 'long' }).format(new Date(2024, 0, index + 1));
    return tr('desktop_scheduled_weekly_schedule', day, draft.time, draft.timezone);
  }
  return tr('desktop_scheduled_schedule', tr(draft.frequency === 'Daily' ? 'desktop_scheduled_daily' : 'desktop_scheduled_weekdays'), draft.time, draft.timezone);
}

function scheduledErrorLabel(error: string): string {
  return ['desktop_scheduled_scope_unavailable', 'desktop_scheduled_scope_ambiguous', 'cron_link_missing'].includes(error) ? tr(error) : error;
}
