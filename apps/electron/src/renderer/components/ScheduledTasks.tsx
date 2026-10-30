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
  { key: 'prices', emoji: '🏷️', frequency: 'Daily', day: 'Monday', time: '09:00' },
  { key: 'concerts', emoji: '♫', frequency: 'Weekly', day: 'Friday', time: '09:00' },
  { key: 'weekend', emoji: '🎉', frequency: 'Weekly', day: 'Thursday', time: '09:00' },
  { key: 'finances', emoji: '💰', frequency: 'Weekly', day: 'Friday', time: '16:00' },
  { key: 'meals', emoji: '🥗', frequency: 'Weekly', day: 'Sunday', time: '10:00' },
  { key: 'flights', emoji: '✈️', frequency: 'Daily', day: 'Monday', time: '09:00' },
] as const;


type ScopedJob = { job: ScheduledCronJob; scopeId: string };
type Initial = { title: string; description: string; draft?: ScheduledTaskDraft; preset?: Pick<ScheduledTaskDraft, 'frequency' | 'day' | 'time'> };
const taskKey = (item: ScopedJob) => `${item.scopeId}:${item.job.id}`;
export function ScheduledTasks({ bridge, visible, onOpenChat, initialTarget }: { bridge: UseBridge; visible: boolean; onOpenChat?(): void; initialTarget?: ScheduledLinkRequest }) {
  const [jobs, setJobs] = useState<ScopedJob[]>([]);
  const [selected, setSelected] = useState<ScopedJob | null>(null);
  const [initial, setInitial] = useState<Initial | null>(null);
  const [editorVersion, setEditorVersion] = useState(0);
  const [editorOpen, setEditorOpen] = useState(false);
  const [menuKey, setMenuKey] = useState<string | null>(null);
  const [filterOpen, setFilterOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTrigger = useRef<HTMLButtonElement | null>(null);
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
  useEffect(() => {
    if (!menuKey) return;
    const dismiss = (event: PointerEvent) => { if (!menuRef.current?.contains(event.target as Node) && !menuTrigger.current?.contains(event.target as Node)) setMenuKey(null); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { setMenuKey(null); menuTrigger.current?.focus(); } };
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('keydown', escape);
    menuRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => { document.removeEventListener('pointerdown', dismiss); document.removeEventListener('keydown', escape); };
  }, [menuKey]);
  const close = () => { setEditorOpen(false); setNotice(''); setMenuKey(null); setSelected(null); setInitial(null); setDirty(false); setShowHistory(false); };
  const open = (item: ScopedJob | null, value?: Initial) => {
    setEditorOpen(!item || Boolean(value)); setMenuKey(null); setHistory(item?.job.automation?.runs ?? []);
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
    } catch (cause) { setError(String(cause)); setDeleting(false); }
    // `version.current++` above aborts any in-flight refresh before its own
    // trailing setLoading(false) runs, so this mutation owns clearing it —
    // exactly as save() does.
    finally { mutation.current = false; setBusy(false); setLoading(false); }
  }
  async function loadHistory(item: ScopedJob) {
    try {
      const runs = await bridge.readScheduledHistory(item.scopeId, item.job.id);
      if (selectedRef.current && taskKey(selectedRef.current) === taskKey(item)) { setHistory(runs); setShowHistory(true); }
    } catch (cause) { setError(String(cause)); }
  }
  function copyTask(item: ScopedJob) {
    open(null, { title: '', description: '', draft: {
      ...scheduledTaskFromJob(item.job),
      automation: item.job.automation ? {
        version: 2, model: item.job.automation.model, reasoning: item.job.automation.reasoning,
        notificationPolicy: item.job.automation.notificationPolicy, status: 'active', runMode: 'new_session',
      } : undefined,
    } });
  }
  async function toggleStatus(item: ScopedJob) {
    if (mutation.current) return;
    mutation.current = true; setBusy(true); version.current++;
    try {
      const result = await bridge.manageScheduled(item.scopeId, { action: item.job.automation?.status === 'paused' ? 'resume' : 'pause', id: item.job.id });
      setJobs(previous => [...previous.filter(other => other.scopeId !== item.scopeId), ...result.map(job => ({ job, scopeId: item.scopeId }))]);
      const updated = result.find(job => job.id === item.job.id);
      if (updated && selectedRef.current && taskKey(selectedRef.current) === taskKey(item)) { setSelected({ job: updated, scopeId: item.scopeId }); setEditorVersion(value => value + 1); }
    } catch (cause) { setError(String(cause)); }
    finally { mutation.current = false; setBusy(false); setLoading(false); }
  }
  const t = useT();
  const variables = { '--scheduled-bg': t.windowBg, '--scheduled-text': t.text, '--scheduled-secondary': t.text2,
    '--scheduled-muted': t.text3, '--scheduled-border': t.border, '--scheduled-hover': t.surfaceHover, '--scheduled-accent': t.accent } as CSSProperties;
  const query = search.trim().toLocaleLowerCase();
  const filtered = jobs.filter(({ job, scopeId }) => (filter === 'all' || (job.automation?.status ?? 'active') === filter)
    && `${job.prompt} ${job.cron} ${scopes.find((scope) => scope.id === scopeId)?.label}`.toLocaleLowerCase().includes(query));
  const sessions = Object.entries(bridge.bootstrap?.projectCatalogs ?? {}).flatMap(([scopeId, catalog]) => catalog.sessions.map((session) => ({ id: session.uuid, title: session.title || `${tr('drawer_tab_chat')} ${session.uuid.slice(0, 8)}`, scopeId })));
  const suggestions = SUGGESTIONS.map(item => ({ ...item, title: tr(`desktop_scheduled_${item.key}_title`), description: tr(`desktop_scheduled_${item.key}_description`), preset: { frequency: item.frequency, day: item.day, time: item.time } }));
  const editing = Boolean(selected || initial);
  return <section className={`scheduled-tasks${editing ? ' scheduled-has-detail' : ''}`} style={variables} aria-label={tr('settings_title_cron')}>
    <div className="scheduled-master">
      <header className="scheduled-tasks-toolbar"><h1>{tr('settings_title_cron')}</h1><div className="scheduled-toolbar-actions">
        <button type="button" className="scheduled-icon-button" aria-label={tr('desktop_scheduled_filter_aria')} aria-expanded={filterOpen} onClick={() => setFilterOpen(value => !value)}><Icon name="sliders" size={17} /></button>
        <button type="button" className="scheduled-icon-button" aria-label={tr('cron_search_tasks')} aria-expanded={searchOpen} onClick={() => setSearchOpen(value => !value)}><Icon name="search" size={18} /></button>
      </div></header>
      <button type="button" className="scheduled-create-button" disabled={busy} onClick={() => choose(() => open(null, { title: '', description: '' }))}><Icon name="plus" size={18} />{tr('desktop_scheduled_new_task')}</button>
      <div className="scheduled-tasks-content">
        {(searchOpen || search) && <div className="scheduled-search"><Icon name="search" size={17} /><input autoFocus type="search" aria-label={tr('cron_search_tasks')} placeholder={tr('cron_search_tasks')} value={search} onChange={(event) => setSearch(event.target.value)} /></div>}
        <div className="scheduled-filter-controls" hidden={!filterOpen}><label className="scheduled-filter"><span className="scheduled-sr-only">{tr('desktop_scheduled_filter_aria')}</span><select value={filter} onChange={event => setFilter(event.target.value)}>{['all', 'active', 'paused', 'completed'].map(status => <option key={status} value={status}>{taskStatusLabel(status)}</option>)}</select></label><button type="button" className="scheduled-icon-button" aria-label={tr('settings_linux_refresh_button')} disabled={loading || busy} onClick={() => void refresh()}><Icon name="refresh" size={16} /></button></div>
        {error && <p role="alert" className="scheduled-empty">{scheduledErrorLabel(error)}</p>}{notice && <p role="status" className="scheduled-empty">{tr(notice)}</p>}
        {loading && <p role="status" className="scheduled-empty">{tr('desktop_scheduled_loading')}</p>}
        {['active', 'paused', 'completed'].map(group => {
          const items = filtered.filter(({ job }) => (job.automation?.status ?? 'active') === group);
          if (!items.length) return null;
          return <section className="scheduled-task-group" key={group} aria-label={taskStatusLabel(group)}><h2>{taskStatusLabel(group)}</h2>{items.map(item => {
            const { job } = item; const draft = scheduledTaskFromJob(job);
            const selectedItem = selected && taskKey(selected) === taskKey(item);
            const locked = busy || job.permanent || job.durable === false;
            return <div key={taskKey(item)} className={`scheduled-job-row${selectedItem ? ' selected' : ''}`}>
              <button type="button" className="scheduled-suggestion scheduled-job" disabled={busy} aria-pressed={Boolean(selectedItem)} onClick={() => choose(() => open(item))}>
                <span className="scheduled-suggestion-copy"><span className="scheduled-job-title">{draft.title}</span><span className="scheduled-suggestion-description">{job.next_run_at ? `${new Date(job.next_run_at).toLocaleString(currentLocale(), { weekday: 'short', hour: 'numeric', minute: '2-digit' })} · ` : ''}{scheduleLabel(draft)}</span></span>
              </button>
              <button type="button" className="scheduled-icon-button scheduled-more" disabled={busy} aria-label={tr('desktop_scheduled_task_actions', draft.title)} aria-expanded={menuKey === taskKey(item)} onClick={event => { menuTrigger.current = event.currentTarget; setMenuKey(menuKey === taskKey(item) ? null : taskKey(item)); }}><Icon name="more" size={18} /></button>
              {menuKey === taskKey(item) && <div ref={menuRef} className="scheduled-context-menu" onKeyDown={event => {
                if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
                event.preventDefault();
                const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
                const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
                buttons[(index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
              }}>
                <button type="button" disabled={locked || !job.automation} onClick={() => choose(() => { setMenuKey(null); void toggleStatus(item); })}><Icon name={group === 'paused' ? 'playCircle' : 'pauseCircle'} size={18} />{tr(group === 'paused' ? 'desktop_scheduled_resume' : 'desktop_scheduled_pause')}</button>
                <button type="button" disabled={locked} onClick={() => choose(() => { open(item); setEditorOpen(true); })}><Icon name="pencil" size={18} />{tr('desktop_scheduled_edit')}</button>
                <button type="button" onClick={() => choose(() => { open(item); void loadHistory(item); })}><Icon name="clock" size={18} />{tr('cron_run_history_section')}</button>
                <button type="button" disabled={busy} onClick={() => choose(() => copyTask(item))}><Icon name="copy" size={18} />{tr('cron_copy_project')}</button>
                <div className="scheduled-menu-divider" />
                <button type="button" className="scheduled-menu-delete" disabled={locked} onClick={() => choose(() => { open(item); setDeleting(true); })}><Icon name="trash" size={18} />{tr('common_delete')}</button>
              </div>}
            </div>;
          })}</section>;
        })}
        {!loading && filtered.length === 0 && <p className="scheduled-empty">{query || filter !== 'all' ? tr('desktop_scheduled_no_matches') : tr('cron_no_tasks_yet')}</p>}
      </div>
    </div>
    {!editing && <div className="scheduled-welcome"><div className="scheduled-welcome-icon"><span aria-hidden="true">🕘</span></div><h2>{tr('desktop_scheduled_welcome_title')}</h2><p>{tr('desktop_scheduled_welcome_description')}</p><div className="scheduled-suggestions">{suggestions.map(suggestion => <button className="scheduled-suggestion" type="button" key={suggestion.title} disabled={busy} onClick={() => choose(() => open(null, suggestion))}><span className="scheduled-suggestion-emoji" aria-hidden="true">{suggestion.emoji}</span><span className="scheduled-suggestion-copy"><span>{suggestion.title}</span><span className="scheduled-suggestion-description">{suggestion.description}</span></span></button>)}</div></div>}
    {editing && <aside className={`scheduled-detail${editorOpen ? ' scheduled-detail-editing' : ''}`}>
      {!editorOpen && selected && <ScheduledOverview
        item={selected}
        bridge={bridge}
        busy={busy}
        showHistory={showHistory}
        onClose={close}
        onEdit={() => setEditorOpen(true)}
      />}
      {editorOpen &&
      <ScheduledTaskSetup key={editorVersion} initial={initial ?? undefined} editingJob={selected?.job} scopeId={selected?.scopeId ?? 'global'} scopes={scopes} sessions={sessions} models={bridge.desktop.modelDetails} defaultModel={bridge.desktop.currentModel} defaultReasoning={bridge.desktop.conversationControls?.reasoning.requested} onBack={() => choose(close)} onSave={save} onDirtyChange={setDirty} loadContext={bridge.scheduledContext} />}
      {showHistory && <section className="scheduled-history" aria-label={tr('cron_run_history_section')}><h3>{tr('cron_run_history_section')}</h3>{history.length === 0 && <p>{tr('desktop_scheduled_no_runs')}</p>}{history.map((run) => <article key={run.id} id={`scheduled-run-${run.id}`} aria-current={run.id === targetRunId ? 'true' : undefined} style={run.id === targetRunId ? { outline: '2px solid var(--scheduled-accent)', outlineOffset: 4 } : undefined}><strong>{runStatusLabel(run.status)}</strong><time>{new Date(run.scheduledAt).toLocaleString(currentLocale())}</time><p>{run.model}</p>{run.startedAt && <p>{tr('cron_started_at_label')}: {new Date(run.startedAt).toLocaleString(currentLocale())}{run.finishedAt ? ` · ${tr('cron_finished_at_label')}: ${new Date(run.finishedAt).toLocaleString(currentLocale())}` : ''}</p>}<p>{run.error ?? run.summary}</p>{run.sessionId && selected && <button type="button" onClick={() => void bridge.openScheduledSession(selected.scopeId, run.sessionId!).then(() => onOpenChat?.()).catch((cause) => setError(String(cause)))}>{tr('cron_open_chat')}</button>}</article>)}</section>}
    </aside>}
    {deleting && selected && <div className="scheduled-confirm" role="alertdialog" aria-modal="true" aria-label={tr('desktop_scheduled_confirm_delete')} onKeyDown={event => {
      if (event.key === 'Escape' && !busy) setDeleting(false);
      if (event.key === 'Tab') {
        const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>('button');
        if (event.shiftKey && document.activeElement === buttons[0]) { event.preventDefault(); buttons[buttons.length - 1]?.focus(); }
        else if (!event.shiftKey && document.activeElement === buttons[buttons.length - 1]) { event.preventDefault(); buttons[0]?.focus(); }
      }
    }}><div><h2>{tr('desktop_scheduled_confirm_delete')}</h2><p>{scheduledTaskFromJob(selected.job).title}</p><button type="button" autoFocus disabled={busy} onClick={() => setDeleting(false)}>{tr('common_cancel')}</button><button type="button" disabled={busy} onClick={() => void remove()}>{tr('desktop_scheduled_confirm_delete')}</button></div></div>}
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

function ScheduledOverview({ item, bridge, busy, showHistory, onClose, onEdit }: {
  item: ScopedJob;
  bridge: UseBridge;
  busy: boolean;
  showHistory: boolean;
  onClose(): void;
  onEdit(): void;
}) {
  const { job, scopeId } = item;
  const draft = scheduledTaskFromJob(job);
  const latest = job.automation?.runs?.at(-1);
  return <>
    <header className="scheduled-overview-header">
      <button type="button" className="scheduled-icon-button" onClick={onClose} aria-label={tr('desktop_scheduled_close_details')}><Icon name="arrowLeft" size={18} /></button>
      <span>{draft.title}</span>
      <span className="scheduled-overview-status">{taskStatusLabel(job.automation?.status ?? 'active')}</span>
    </header>
    <div className="scheduled-overview">
      <div className="scheduled-conversation">
        <div className="scheduled-prompt">{draft.instructions}</div>
        <div className="scheduled-response">
          <p className="scheduled-response-label">{tr('cron_frequency')}</p>
          <p>{scheduleLabel(draft)}</p>
          {job.automation?.statusReason && <p>{job.automation.statusReason}</p>}
          <button type="button" className="scheduled-task-card" disabled={busy || job.permanent || job.durable === false} onClick={onEdit}>
            <span className="scheduled-card-icon"><Icon name="clock" size={24} /></span>
            <span><strong>{draft.title}</strong><small>{scheduleLabel(draft)}</small></span>
            <span className="scheduled-card-open">{tr('desktop_scheduled_edit')}</span>
          </button>
        </div>
        {!showHistory && <section className="scheduled-latest">
          <h3>{tr('cron_run_history_section')}</h3>
          {latest ? <><p>{runStatusLabel(latest.status)}</p><p>{latest.error ?? latest.summary}</p></> : <p>{tr('desktop_scheduled_no_runs')}</p>}
        </section>}
      </div>
      <aside className="scheduled-inspector">
        <h2>{tr('settings_title_cron')}</h2>
        <div className="scheduled-inspector-task"><Icon name="clock" size={19} /><strong>{draft.title}</strong></div>
        <p>{scheduleLabel(draft)}</p>
        <div className="scheduled-inspector-section">
          <h3>{tr('cron_run_project_label')}</h3>
          <p>{bridge.scheduledScopes.find(scope => scope.id === scopeId)?.label ?? tr('common_none')}</p>
        </div>
        {job.automation?.model && <div className="scheduled-inspector-section">
          <h3>{tr('session_details_model')}</h3>
          <p>{bridge.desktop.modelDetails.find(model => model.reference === job.automation?.model)?.display_name ?? job.automation.model}</p>
        </div>}
      </aside>
    </div>
  </>;
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
