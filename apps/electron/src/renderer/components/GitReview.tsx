import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import type { GitApi, GitBranch, GitCommit, GitConflict, GitDiff, GitFile, GitRequest, GitResult, GitScope, GitStash, GitStatus } from '../../shared/git';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { EXPLICIT_HIGHLIGHT_MAX_CHARS, highlightCodeForDisplay } from './CodeBlock';
import { reviewHunks, reviewLines, splitReviewLines, type ReviewLine } from './gitDiff';

type Mode = 'working' | 'staged' | 'branch' | 'commit' | 'stash';
type Page = 'changes' | 'history' | 'stash' | 'commit' | 'sync';
interface ViewState { page: Page; mode: Mode; path: string; filter: string; tree: boolean; split: boolean; base: string; target: string; message: string; scroll: number }
const initialView = (): ViewState => ({ page: 'changes', mode: 'working', path: '', filter: '', tree: true, split: false, base: 'HEAD', target: '', message: '', scroll: 0 });
const views = new Map<string, ViewState>();
interface GitContextValue {
  scope: GitScope | null; api?: GitApi; status?: GitStatus; branches: GitBranch[]; loading: boolean; busy: boolean; error: string; notice: string;
  refresh(): Promise<void>; run(request: GitRequest): Promise<GitResult | undefined>; read(request: GitRequest): Promise<GitResult | undefined>;
  view: ViewState; update(change: Partial<ViewState>): void; open(page?: Page): void; terminal(): void;
}
const GitContext = createContext<GitContextValue | null>(null);
function apiAvailable(): GitApi | undefined { return typeof window === 'undefined' ? undefined : (window.lingxi as unknown as { git?: GitApi } | undefined)?.git; }
export function GitWorkspaceProvider({ scope, onOpen, onTerminal, children }: { scope: GitScope | null; onOpen(): void; onTerminal(): void; children: ReactNode }) {
  // Keep the workspace tree mounted while requests remain scoped to their session.
  return <GitWorkspaceSession scope={scope} onOpen={onOpen} onTerminal={onTerminal}>{children}</GitWorkspaceSession>;
}
function GitWorkspaceSession({ scope, onOpen, onTerminal, children }: { scope: GitScope | null; onOpen(): void; onTerminal(): void; children: ReactNode }) {
  useI18n();
  const api = apiAvailable();
  const key = scope ? `${scope.projectPath}\0${scope.sessionId}` : '';
  const currentKey = useRef(key); const generation = useRef(0); if (currentKey.current !== key) { currentKey.current = key; generation.current++; } const epoch = generation.current;
  const [storedView, setView] = useState<{ key: string; value: ViewState }>(() => ({ key, value: views.get(key) ?? initialView() }));
  const view = storedView.key === key ? storedView.value : views.get(key) ?? initialView();
  const [status, setStatus] = useState<GitStatus>(); const [statusKey, setStatusKey] = useState(key); const [branches, setBranches] = useState<GitBranch[]>([]);
  const [loading, setLoading] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  const live = useRef(true); const refreshing = useRef<Promise<void> | null>(null); const pendingRefresh = useRef(false); const mutation = useRef<number | null>(null);
  useEffect(() => () => { live.current = false; }, []);
  const update = useCallback((change: Partial<ViewState>) => setView((old) => { const next = { ...(old.key === key ? old.value : views.get(key) ?? initialView()), ...change }; views.set(key, next); return { key, value: next }; }), [key]);
  const refresh = useCallback(async () => {
    if (!scope || !api || (currentKey.current !== key || generation.current !== epoch)) return;
    if (refreshing.current) { pendingRefresh.current = true; return refreshing.current; }
    const work = async () => {
      do {
        pendingRefresh.current = false;
        if (live.current && (currentKey.current === key && generation.current === epoch)) setLoading(true);
        try {
          const result = await api.request(scope, { kind: 'status' });
          if (!live.current || (currentKey.current !== key || generation.current !== epoch)) return;
          setStatus(result.status); setStatusKey(key);
          if (result.status?.repository) { const list = await api.request(scope, { kind: 'branches' }); if (live.current && (currentKey.current === key && generation.current === epoch)) setBranches(list.branches ?? []); }
        } catch (e) { if (live.current && (currentKey.current === key && generation.current === epoch)) setError(String(e instanceof Error ? e.message : e)); }
        finally { if (live.current && (currentKey.current === key && generation.current === epoch)) setLoading(false); }
      } while (pendingRefresh.current && live.current && (currentKey.current === key && generation.current === epoch));
    };
    const own = work(); refreshing.current = own; await own; if (refreshing.current === own) refreshing.current = null;
  }, [key, api, epoch]);
  useEffect(() => {
    live.current = true; setStatus(undefined); setBranches([]); setError(''); setNotice(''); setBusy(false); mutation.current = null; refreshing.current = null; pendingRefresh.current = false; void refresh();
    const off = api?.onChanged(() => { void refresh(); });
    const focus = () => { void refresh(); }; window.addEventListener('focus', focus);
    return () => { live.current = false; off?.(); window.removeEventListener('focus', focus); };
  }, [refresh, api]);
  const read = useCallback(async (request: GitRequest) => {
    if (!api || !scope) return undefined;
    try { const result = await api.request(scope, request); return live.current && (currentKey.current === key && generation.current === epoch) ? result : undefined; }
    catch (e) { if (live.current && (currentKey.current === key && generation.current === epoch)) setError(String(e instanceof Error ? e.message : e)); return undefined; }
  }, [key, api, epoch]);
  const run = useCallback(async (request: GitRequest) => {
    if (!api || !scope || mutation.current === epoch || (currentKey.current !== key || generation.current !== epoch)) return undefined;
    mutation.current = epoch; setBusy(true); setError(''); setNotice('');
    try { const result = await api.request(scope, request); if (!live.current || (currentKey.current !== key || generation.current !== epoch)) return undefined; setNotice(result.output?.trim() || tr('desktop_git_7f52b3cde4')); await refresh(); return result; }
    catch (e) { if (live.current && (currentKey.current === key && generation.current === epoch)) setError(String(e instanceof Error ? e.message : e)); await refresh(); return undefined; }
    finally { if (mutation.current === epoch) mutation.current = null; if (live.current && (currentKey.current === key && generation.current === epoch)) setBusy(false); }
  }, [key, api, refresh, epoch]);
  return <GitContext.Provider value={{ scope, api, status: statusKey === key ? status : undefined, branches: statusKey === key ? branches : [], loading, busy, error, notice, refresh, read, run, view, update, open: (page) => { if (page) update({ page }); onOpen(); }, terminal: onTerminal }}>{children}</GitContext.Provider>;
}
function useGit() { const value = useContext(GitContext); if (!value) throw new Error(tr('desktop_git_7a5bd8399b')); return value; }
function BranchIcon() { return <svg width="16" height="16" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><circle cx="5" cy="4" r="2"/><circle cx="5" cy="16" r="2"/><circle cx="15" cy="4" r="2"/><path d="M5 6v8m10-8v2c0 4-10 1-10 6"/></svg>; }
export function GitTopBar() {
  useI18n();
  const t = useT();
  const git = useContext(GitContext); const [open, setOpen] = useState(false); const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { if (!open) return; const pointer = (e: PointerEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); }; const key = (e: KeyboardEvent) => { if (e.key === 'Escape') { setOpen(false); ref.current?.querySelector('button')?.focus(); } }; document.addEventListener('pointerdown', pointer); document.addEventListener('keydown', key); return () => { document.removeEventListener('pointerdown', pointer); document.removeEventListener('keydown', key); }; }, [open]);
  useEffect(() => { setOpen(false); }, [git?.scope?.projectPath, git?.scope?.sessionId]);
  if (!git?.api || !git.scope) return null;
  return <div className="git-topbar no-drag" ref={ref} style={themeStyle(t)}>
    <button className="git-branch-pill" aria-label={tr('desktop_git_8e9928c7c5')} aria-expanded={open} onClick={() => setOpen(!open)}><BranchIcon/><span>{git.status?.repository ? git.status.branch || git.status.head.slice(0, 8) || tr('desktop_git_387c8b3c93') : 'Git'}</span>{!!git.status?.files.length && <small>{git.status.files.length}</small>}{!!(git.status?.ahead || git.status?.behind) && <small>↑{git.status.ahead} ↓{git.status.behind}</small>}<Icon name="chevron" size={12}/></button>
    {open && <div className="git-environment" role="dialog" aria-label={tr('desktop_git_8e9928c7c5')}><GitEnvironment onNavigate={() => setOpen(false)}/></div>}
  </div>;
}
export function GitEnvironment({ onNavigate }: { onNavigate?(): void }) {
  useI18n();
  const git = useContext(GitContext); const t = useT(); const [anchor, setAnchor] = useState<DOMRect>();
  useEffect(() => { setAnchor(undefined); }, [git?.scope?.projectPath, git?.scope?.sessionId]);
  if (!git?.api || !git.scope) return null;
  const navigate = (page: Page) => { git.open(page); onNavigate?.(); };
  return <section className="git-environment-content" aria-label={tr('desktop_git_9e471951a1')} style={themeStyle(t)}><h2>{tr('desktop_git_9e471951a1')}</h2>
    <button onClick={() => navigate('changes')}><Icon name="file" size={18}/><span>{tr('desktop_git_bbd4b6a86b')}</span><small>{git.status?.files.length ?? ''}</small></button>
    <button data-git-branch-trigger="true" aria-label={tr('desktop_git_5051902c7b')} aria-expanded={!!anchor} disabled={!git.status?.repository} onClick={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}><BranchIcon/><span>{git.status?.repository ? git.status.branch || git.status.head.slice(0, 8) || tr('desktop_git_387c8b3c93') : git.loading ? tr('desktop_git_a60f137fce') : tr('desktop_git_4465977c0a')}</span><Icon name="chevron" size={14}/></button>
    <button onClick={() => navigate('commit')}><Icon name="check" size={18}/><span>{tr('desktop_git_99e6d0c2cc')}</span></button>
    {git.error && <div className="git-message git-error" role="alert">{git.error}<button onClick={() => void git.refresh()}>{tr('visualization_retry')}</button></div>}
    {anchor && <BranchPicker anchor={anchor} onClose={() => setAnchor(undefined)}/>}</section>;
}
function Dialog({ title, children, onClose, anchor }: { title: string; children: ReactNode; onClose(): void; anchor?: DOMRect }) {
  const t = useT(); const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { const dialog = ref.current; const previous = document.activeElement as HTMLElement | null; dialog?.showModal(); return () => { dialog?.close(); previous?.focus(); }; }, []);
  return <dialog ref={ref} className={`git-dialog${anchor ? ' git-branch-popover' : ''}`} style={{ ...themeStyle(t), ...(anchor ? { position: 'fixed', margin: 0, width: Math.min(420, window.innerWidth - 24), left: Math.max(12, Math.min(anchor.left - 428, window.innerWidth - 432)), top: Math.max(12, Math.min(anchor.top, window.innerHeight - 540)), maxHeight: 'min(520px, calc(100vh - 24px))' } : {}) }} onCancel={(e) => { e.preventDefault(); onClose(); }} aria-label={title}><header><h2>{title}</h2><button aria-label={tr('desktop_git_c8df66c5fe')} onClick={onClose}>×</button></header>{children}</dialog>;
}
function BranchPicker({ onClose, anchor }: { onClose(): void; anchor?: DOMRect }) {
  const git = useGit(); const [query, setQuery] = useState(''); const [name, setName] = useState('');
  const changedFiles = git.status?.files.length ?? 0;
  const locked = git.busy || !!git.status?.busy || changedFiles > 0;
  const lockTitle = git.busy ? tr('desktop_git_c780ae89f1') : git.status?.busy ? tr('desktop_git_628db670ff') : tr('desktop_git_a388bd2d37');
  const lockDetail = git.busy
    ? tr('desktop_git_6dd24b2cc9')
    : git.status?.busy
      ? tr('desktop_git_c11bdc7e53')
      : tr('desktop_git_changed_files', changedFiles);
  const groups = [false, true].map((remote) => ({
    remote,
    title: remote ? tr('desktop_git_f91f9501b4') : tr('desktop_git_5ee6453874'),
    branches: git.branches
      .filter((branch) => branch.remote === remote && branch.name.toLowerCase().includes(query.trim().toLowerCase()))
      .sort((a, b) => Number(b.current) - Number(a.current) || a.name.localeCompare(b.name)),
  }));
  const visibleBranchCount = groups.reduce((count, group) => count + group.branches.length, 0);
  const checkout = async (branch: string, create = false) => { if (await git.run({ kind: 'checkout', branch, create, token: git.status!.token })) onClose(); };
  const projectName = git.scope?.projectPath.split(/[\\/]/).filter(Boolean).pop() ?? 'project';

  return <Dialog title={tr('desktop_git_5051902c7b')} onClose={onClose} anchor={anchor}>
    <div className="git-branch-picker">
      <label className="git-branch-search">
        <Icon name="search" size={16} />
        <input autoFocus aria-label={tr('desktop_git_00dd9d6327')} placeholder={tr('desktop_git_2730cc582d', projectName)} value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>

      {locked && <div className={`git-branch-warning${git.busy || git.status?.busy ? ' is-busy' : ''}`} role="status">
        <span className="git-branch-warning-icon" aria-hidden="true"><Icon name={git.busy || git.status?.busy ? 'info' : 'warning'} size={16} /></span>
        <div className="git-branch-warning-copy">
          <strong>{lockTitle}</strong>
          <p>{lockDetail}</p>
          {!git.status?.busy && !git.busy && changedFiles > 0 && <div className="git-branch-warning-actions">
            <button type="button" onClick={() => { git.open('commit'); onClose(); }}>{tr('desktop_git_82a9c46ffa')}</button>
            <button type="button" onClick={() => { git.open('stash'); onClose(); }}>{tr('desktop_git_38bdfeaa67')}</button>
          </div>}
        </div>
      </div>}

      <div className="git-branch-list" role="region" aria-label={tr('desktop_git_3302812bfb')}>
        {groups.map((group) => group.branches.length > 0 && <section className="git-branch-group" key={String(group.remote)}>
          <header><h3>{group.title}</h3><span>{group.branches.length}</span></header>
          {group.branches.map((branch) => {
            const inAnotherWorktree = !branch.current && !!branch.worktree && branch.worktree !== git.status?.root;
            return <button
              key={branch.name}
              className={`git-branch-row${branch.current ? ' is-current' : ''}`}
              aria-current={branch.current ? 'true' : undefined}
              disabled={locked || branch.current || inAnotherWorktree}
              title={branch.name}
              onClick={() => void checkout(branch.name)}
            >
              <span className="git-branch-row-symbol" aria-hidden="true"><BranchIcon /></span>
              <span className="git-branch-row-copy">
                <span className="git-branch-name">{branch.name}</span>
                <small className="git-branch-row-meta">
                  {branch.current && <span className="git-branch-current-tag">{tr('desktop_git_e0d1b68224')}</span>}
                  {inAnotherWorktree && <span title={branch.worktree}>{tr('desktop_git_5d5e81ea35')}</span>}
                  {branch.upstream && <span title={tr('desktop_git_693b525e8c', branch.upstream)}>{branch.upstream}</span>}
                </small>
              </span>
              {branch.current && <span className="git-branch-row-check" aria-label={tr('desktop_git_7c5b2da192')}><Icon name="check" size={15} stroke={2} /></span>}
            </button>;
          })}
        </section>)}
        {visibleBranchCount === 0 && <div className="git-branch-empty" role="status">
          <Icon name="branch" size={22} />
          <span>{git.loading ? tr('desktop_git_55704e8f60') : query.trim() ? tr('desktop_git_126c39db95', query.trim()) : tr('desktop_git_98436a9be2')}</span>
        </div>}
      </div>

      <form className="git-branch-create" onSubmit={(event) => { event.preventDefault(); void checkout(name.trim(), true); }}>
        <label>{tr('desktop_git_7f445154d6')}<input aria-label={tr('desktop_git_514cee61cf')} placeholder={tr('desktop_git_06f6bb7108')} value={name} onChange={(event) => setName(event.target.value)} /></label>
        <button className="git-primary" disabled={locked || !name.trim()}><Icon name="plus" size={14} stroke={2} />{tr('desktop_git_912927d9dd')}</button>
      </form>

      {git.error && <p className="git-branch-error" role="alert"><Icon name="circleAlert" size={15} />{git.error}</p>}
    </div>
  </Dialog>;
}
function Confirm({ title, description, paths, onConfirm, onClose }: { title: string; description: string; paths?: string[]; onConfirm(): Promise<unknown>; onClose(): void }) {
  const [busy, setBusy] = useState(false);
  return <Dialog title={title} onClose={onClose}><p>{description}</p>{paths && <ul className="git-confirm-paths">{paths.map((path) => <li key={path}>{path}</li>)}</ul>}<footer><button onClick={onClose} disabled={busy}>{tr('common_cancel')}</button><button className="git-danger" disabled={busy} onClick={async () => { setBusy(true); await onConfirm(); onClose(); }}>{busy ? tr('desktop_git_5474eef8d0') : title}</button></footer></Dialog>;
}
function themeStyle(t: ReturnType<typeof useT>): CSSProperties { return { '--git-bg': t.surface, '--git-fg': t.text, '--git-muted': t.text3, '--git-border': t.border, '--git-hover': t.surfaceHover, '--git-accent': t.accent, '--git-danger': t.danger, '--git-warning': t.warn, '--git-add': t.dark ? 'rgba(74, 190, 110, .13)' : 'rgba(49, 160, 80, .09)', '--git-remove': t.dark ? 'rgba(230, 83, 100, .13)' : 'rgba(220, 65, 85, .09)', ...Object.fromEntries(Object.entries(t.syntax).map(([syntaxClass, color]) => [`--git-syntax-${syntaxClass}`, color])), color: t.text } as CSSProperties; }
export function GitReview() {
  useI18n();
  const git = useContext(GitContext);
  return <GitReviewContent key={`${git?.scope?.projectPath ?? ''}\0${git?.scope?.sessionId ?? ''}`}/>;
}
function GitReviewContent() {
  const git = useContext(GitContext); const t = useT(); const [picker, setPicker] = useState(false);
  if (!git) return <div className="git-empty">{tr('desktop_git_bbeb21cae4')}</div>;
  const { status, view } = git;
  return <div className="git-review" style={themeStyle(t)}>
    <div className="git-review-toolbar"><select aria-label={tr('desktop_git_051da6ae6f')} value={view.page} onChange={(e) => git.update({ page: e.target.value as Page })}><option value="changes">{tr('desktop_git_bbd4b6a86b')}</option><option value="history">{tr('desktop_git_0e76960093')}</option><option value="stash">{tr('desktop_git_e45b6015e5')}</option><option value="commit">{tr('desktop_git_82a9c46ffa')}</option><option value="sync">{tr('desktop_git_8d261a372f')}</option></select><span className="git-spacer"/><button title={tr('desktop_git_52e029304e')} aria-label={tr('desktop_git_52e029304e')} disabled={git.loading || git.busy} onClick={() => void git.refresh()}><Icon name="refresh" size={16}/></button><button disabled={!status?.repository} aria-label={tr('desktop_git_5051902c7b')} onClick={() => setPicker(true)}><BranchIcon/>{status?.branch || tr('desktop_git_23b028a2c6')}</button></div>
    {git.error && <div className="git-message git-error" role="alert">{git.error}<button onClick={git.terminal}>{tr('desktop_git_acb1f43d28')}</button></div>}
    {git.busy && <div className="git-message" role="status">{tr('desktop_git_4cb3a82a31')}</div>}
    {!git.busy && git.notice && <div className="git-message git-success" role="status">{git.notice}</div>}
    {status?.busy && <div className="git-message">{tr('desktop_git_206e567a02')}</div>}
    {!git.api ? <div className="git-empty">{tr('desktop_git_0866af8400')}</div> : !status ? <div className="git-empty">{git.loading ? tr('desktop_git_156aba8664') : tr('desktop_git_e312efef68')}</div> : !status.available ? <div className="git-empty"><h3>{tr('desktop_git_67fa92995b')}</h3><p>{tr('desktop_git_7ec33f96d7')}</p><button onClick={git.terminal}>{tr('desktop_git_acb1f43d28')}</button></div> : !status.repository ? <div className="git-empty"><Icon name="file" size={36}/><h3>{tr('desktop_git_07e1425501')}</h3><p>{tr('desktop_git_f5b046cc7d')}</p><button className="git-primary" disabled={git.busy} onClick={() => void git.run({ kind: 'init' })}>{tr('desktop_git_5a00802202')}</button></div> : <>
      {status.merging && <MergeControls/>}
      {view.page === 'changes' && <Changes/>}{view.page === 'history' && <History/>}{view.page === 'stash' && <Stashes/>}{view.page === 'commit' && <Commit/>}{view.page === 'sync' && <Sync/>}
    </>}{picker && <BranchPicker onClose={() => setPicker(false)}/>}
  </div>;
}
const FILE_GROUP_LABELS: Record<string, string> = {"Conflicts":"desktop_git_9f5ea35dc7","Staged":"desktop_git_daec459ab6","Changes":"desktop_git_bbd4b6a86b","Untracked":"desktop_git_c7ba5477e1","Files":"desktop_git_abc7e98928"};
function Changes({ fixedMode, fixedTarget }: { fixedMode?: Mode; fixedTarget?: string }) {
  const git = useGit(); const { status, view } = git; const mode = fixedMode ?? view.mode; const target = fixedTarget ?? view.target;
  const [diff, setDiff] = useState<GitDiff>(); const [catalog, setCatalog] = useState<GitFile[]>([]); const [loading, setLoading] = useState(false);
  const [discard, setDiscard] = useState<{ file: GitFile; token: string }>(); const [conflict, setConflict] = useState<string>(); const scroll = useRef<HTMLDivElement>(null); const scrollReady = useRef(false);
  const comparison = JSON.stringify([mode, target, view.base, view.path]);
  const previousComparison = useRef<string>();
  useEffect(() => {
    let active = true;
    // Repository tokens cover every file. Revalidate in place so unrelated
    // edits do not unmount the preview or reset the user's scroll position.
    if (previousComparison.current !== comparison) {
      previousComparison.current = comparison;
      scrollReady.current = false;
      setDiff(undefined);
    }
    setLoading(true);
    if ((mode === 'branch' || mode === 'commit' || mode === 'stash') && !target) { setLoading(false); setCatalog([]); return; }
    void git.read({ kind: 'diff', mode, base: view.base, target, path: view.path || undefined }).then((r) => { if (active) { if (r?.diff) setDiff(r.diff); setLoading(false); } });
    return () => { active = false; };
  }, [comparison, mode, target, view.base, view.path, status?.token, git.read]);
  useEffect(() => { if (mode === 'working' || mode === 'staged') { setCatalog(status?.files ?? []); return; } let active = true; if (!target) { setCatalog([]); return; } void git.read({ kind: 'diff', mode, base: view.base, target }).then((r) => { if (active) setCatalog(r?.diff?.files ?? []); }); return () => { active = false; }; }, [mode, target, view.base, status?.token, git.read]);
  useLayoutEffect(() => { if (diff && scroll.current && !scrollReady.current) { scroll.current.scrollTop = view.scroll; scrollReady.current = true; } }, [diff, view.path, mode, target]);
  const groups: [string, GitFile[]][] = mode === 'working' || mode === 'staged' ? [
    ['Conflicts', catalog.filter((f) => f.conflict)], ['Staged', catalog.filter((f) => !f.conflict && f.index !== ' ' && f.index !== '?' && f.index !== '.')], ['Changes', catalog.filter((f) => !f.conflict && !f.untracked && f.working !== ' ' && f.working !== '.')], ['Untracked', catalog.filter((f) => f.untracked)],
  ] : [['Files', catalog]];
  const select = (f: GitFile, group: string) => { if (f.conflict) { setConflict(f.path); return; } git.update({ path: f.path, ...(fixedMode ? {} : { mode: group === 'Staged' ? 'staged' : mode === 'staged' ? 'working' : mode }), scroll: 0 }); };
  return <div className="git-changes">
    {!fixedMode && <div className="git-compare"><select aria-label={tr('desktop_git_571527339d')} value={mode} onChange={(e) => git.update({ mode: e.target.value as Mode, path: '', target: '' })}><option value="working">{tr('desktop_git_f90114ae9e')}</option><option value="staged">{tr('desktop_git_daec459ab6')}</option><option value="branch">{tr('desktop_git_52656e8104')}</option></select>{mode === 'branch' && <><select aria-label={tr('desktop_git_9acbb9ebea')} value={view.base} onChange={(e) => git.update({ base: e.target.value, path: '' })}><option value="HEAD">HEAD</option>{git.branches.map((b) => <option key={b.name}>{b.name}</option>)}</select><span title={tr('desktop_git_7dd8b15d0a')}>→</span><select aria-label={tr('desktop_git_4e239c0bbe')} value={target} onChange={(e) => git.update({ target: e.target.value, path: '' })}><option value="">{tr('desktop_git_2454b121ba')}</option>{git.branches.map((b) => <option key={b.name}>{b.name}</option>)}</select></>}</div>}
    {mode === 'branch' && target && <div className="git-caption">{tr('desktop_git_common_ancestor', view.base, target)}</div>}
    <div className="git-file-filter"><input aria-label={tr('desktop_git_56fc0ca26b')} placeholder={tr('desktop_git_b50efe925a')} value={view.filter} onChange={(e) => git.update({ filter: e.target.value })}/><button aria-label={view.tree ? tr('desktop_git_19576ee61a') : tr('desktop_git_0ddbd7920c')} aria-pressed={view.tree} onClick={() => git.update({ tree: !view.tree })}><Icon name="folder" size={17}/></button><button aria-label={tr('desktop_git_1ff31161b0')} aria-pressed={view.split} onClick={() => git.update({ split: !view.split })}>±</button></div>
    <div className="git-files">{groups.map(([label, entries]) => { const files = entries.filter((f) => f.path.toLowerCase().includes(view.filter.toLowerCase())).sort((a, b) => a.path.localeCompare(b.path)); return files.length > 0 && <section key={label}><header><h3>{tr(FILE_GROUP_LABELS[label] ?? label)} <small>{files.length}</small></h3>{(label === 'Staged' || label === 'Changes' || label === 'Untracked') && <button disabled={git.busy} onClick={() => void git.run({ kind: label === 'Staged' ? 'unstage' : 'stage', paths: files.map((f) => f.path), token: status!.token })}>{label === 'Staged' ? tr('desktop_git_2aed2dbe0b') : tr('desktop_git_24f4c8380e')}</button>}</header><DirectoryFiles files={files} tree={view.tree} renderFile={(file) => (<div className={`git-file-row${view.path === file.path ? ' selected' : ''}`}><button className="git-file-select" onClick={() => select(file, label)} title={file.path}><span className="git-file-status">{file.conflict ? '!' : file.untracked ? '?' : label === 'Staged' ? file.index : file.working}</span><span>{view.tree ? file.path.split('/').pop() : file.path}{file.oldPath && <small> ← {file.oldPath}</small>}{file.submodule && <small> {tr('desktop_git_d8566c566c')}</small>}</span><small className="git-stats">{file.binary ? tr('desktop_git_9a3a45d015') : <><i>+{file.additions}</i> <b>−{file.deletions}</b></>}</small></button>{!file.conflict && (label === 'Staged' || label === 'Changes' || label === 'Untracked') && <button title={label === 'Staged' ? tr('desktop_git_6ee823fc0f') : tr('desktop_git_ab131c2207')} aria-label={tr(label === 'Staged' ? 'desktop_git_unstage_named' : 'desktop_git_stage_named', file.path)} disabled={git.busy} onClick={() => void git.run({ kind: label === 'Staged' ? 'unstage' : 'stage', paths: [file.path], token: status!.token })}>{label === 'Staged' ? '−' : '+'}</button>}{(label === 'Changes' || label === 'Untracked') && <button title={tr('desktop_git_225bfe3302')} aria-label={tr('desktop_git_78aa270306', file.path)} disabled={git.busy || status?.busy} onClick={() => setDiscard({ file, token: status!.token })}>↶</button>}</div>)}/></section>; })}</div>
    {catalog.length === 0 && <div className="git-empty"><Icon name="file" size={40}/><h3>{mode === 'branch' && !target ? tr('desktop_git_493ff606cd') : tr('desktop_git_72cea34c41')}</h3><p>{tr('desktop_git_1329c11873')}</p></div>}
    <div ref={scroll} className="git-diff-scroll" onScroll={(e) => { if (scrollReady.current) git.update({ scroll: e.currentTarget.scrollTop }); }}>{diff ? <Diff diff={diff} mode={mode} path={view.path} refreshing={loading}/> : loading && <div className="git-empty">{tr('desktop_git_3944c70495')}</div>}</div>
    {discard && <Confirm title={discard.file.untracked ? tr('desktop_git_980764f407') : tr('desktop_git_f9bfa3dc7c')} description={discard.file.untracked ? tr('desktop_git_fffa724a42') : tr('desktop_git_dda244dc93')} paths={[discard.file.path]} onClose={() => setDiscard(undefined)} onConfirm={() => git.run({ kind: 'discard', paths: [discard.file.path], untracked: discard.file.untracked, token: discard.token })}/>}
    {conflict && <Conflict path={conflict} onClose={() => setConflict(undefined)}/>}
  </div>;
}
function DirectoryFiles({ files, tree, renderFile }: { files: GitFile[]; tree: boolean; renderFile(file: GitFile): ReactNode }) {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const renderLevel = (entries: GitFile[], prefix: string): ReactNode => {
    const folders = new Map<string, GitFile[]>(); const leaves: GitFile[] = [];
    for (const file of entries) { const relative = file.path.slice(prefix.length); const slash = relative.indexOf('/'); if (slash < 0) leaves.push(file); else { const folder = relative.slice(0, slash); folders.set(folder, [...(folders.get(folder) ?? []), file]); } }
    return <>{[...folders].map(([folder, descendants]) => { const path = prefix + folder + '/'; const open = !collapsed.has(path); return <div key={path}><button className="git-directory" aria-expanded={open} onClick={() => setCollapsed((old) => { const next = new Set(old); if (next.has(path)) next.delete(path); else next.add(path); return next; })}><span>{open ? '⌄' : '›'}</span><Icon name="folder" size={13}/>{folder}<small>{descendants.length}</small></button>{open && <div style={{ paddingLeft: 10 }}>{renderLevel(descendants, path)}</div>}</div>; })}{leaves.map((file) => <div key={file.path}>{renderFile(file)}</div>)}</>;
  };
  return tree ? renderLevel(files, '') : <>{files.map((file) => <div key={file.path}>{renderFile(file)}</div>)}</>;
}
function Diff({ diff, mode, path, refreshing }: { diff: GitDiff; mode: Mode; path: string; refreshing: boolean }) {
  const git = useGit(); const hunks = useMemo(() => reviewHunks(diff.patch), [diff.patch]);
  return <div className="git-diff" aria-label={tr('desktop_git_19a527b311')}>
    <div className="git-diff-title">{path || tr('desktop_git_4203b96f4f')}</div>
    {diff.truncated && <div className="git-message">{tr('desktop_git_0901d78a21')}</div>}
    {diff.binary ? <div className="git-empty">{tr('desktop_git_81bab591b7')}</div> : hunks.length ? hunks.map((hunk, index) => {
      const lines = reviewLines(hunk);
      // Match CodeBlock's size limit so large diffs do not trigger thousands
      // of small syntax-highlighting passes while the review pane renders.
      const language = diff.patch.length <= EXPLICIT_HIGHLIGHT_MAX_CHARS ? reviewLanguage(path || hunk.filePath) : undefined;
      return <section className="git-hunk" key={`${hunk.heading}-${index}`}>
        <header><code>{hunk.heading}</code>{(mode === 'working' || mode === 'staged') && <button disabled={git.busy || refreshing || diff.token !== git.status?.token || diff.truncated} onClick={() => void git.run({ kind: mode === 'staged' ? 'unstage' : 'stage', patch: hunk.patch, token: diff.token })}>{mode === 'staged' ? tr('desktop_git_80ea070d44') : tr('desktop_git_9e82989b98')}</button>}</header>
        {git.view.split
          ? <div className="git-split-diff">{splitReviewLines(lines).map(([left, right], i) => <div className="git-split-row" key={i}><DiffCell line={left} side="old" language={language}/><DiffCell line={right} side="next" language={language}/></div>)}</div>
          : <div>{lines.map((line, i) => <div className={`git-diff-line ${line.kind}`} key={i}><span className="git-line-number">{line.old}</span><span className="git-line-number">{line.next}</span><code>{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}<DiffCode text={line.text} language={line.kind === 'note' ? undefined : language}/></code></div>)}</div>}
      </section>;
    }) : diff.patch ? <pre className="git-raw-diff">{diff.patch}</pre> : <div className="git-empty">{tr('desktop_git_a1a1f50f2e')}</div>}
  </div>;
}
const REVIEW_LANGUAGES: Readonly<Record<string, string>> = {
  bash: 'bash', c: 'c', cc: 'cpp', cjs: 'javascript', cpp: 'cpp', cs: 'csharp', cts: 'typescript', css: 'css', go: 'go', graphql: 'graphql', h: 'c', hpp: 'cpp', htm: 'xml', html: 'xml', java: 'java', js: 'javascript', json: 'json', jsonc: 'json', kt: 'kotlin', kts: 'kotlin', less: 'less', lua: 'lua', md: 'markdown', mjs: 'javascript', mm: 'objectivec', mts: 'typescript', objc: 'objectivec', php: 'php', pl: 'perl', py: 'python', rb: 'ruby', rs: 'rust', sass: 'scss', scala: 'scala', scss: 'scss', sh: 'bash', sql: 'sql', swift: 'swift', ts: 'typescript', tsx: 'typescript', vb: 'vbnet', xml: 'xml', yaml: 'yaml', yml: 'yaml', zsh: 'bash',
};
function reviewLanguage(path: string): string | undefined {
  const name = path.replace(/\\/g, '/').split('/').at(-1)?.replace(/^"|"$/g, '').toLowerCase() ?? '';
  if (name === 'makefile' || name === 'gnumakefile') return 'makefile';
  if (name === 'dockerfile') return 'dockerfile';
  const extension = /\.([^./"]+)"?$/.exec(name)?.[1];
  const language = extension ? REVIEW_LANGUAGES[extension] : undefined;
  return language;
}
function DiffCode({ text, language }: { text: string; language?: string }) {
  const html = useMemo(() => language ? highlightCodeForDisplay(text, language, true).html : undefined, [language, text]);
  // highlight.js escapes source text while generating its own span markup.
  return html === undefined ? <>{text}</> : <span className="git-highlighted hljs" dangerouslySetInnerHTML={{ __html: html }}/>;
}
function DiffCell({ line, side, language }: { line?: ReviewLine; side: 'old' | 'next'; language?: string }) { return <div className={`git-diff-line ${line?.kind ?? 'blank'}`}><span className="git-line-number">{line?.[side]}</span><code>{line && <DiffCode text={line.text} language={line.kind === 'note' ? undefined : language}/>}</code></div>; }
function Commit() {
  const git = useGit(); const status = git.status!; const staged = status.files.filter((f) => !f.conflict && !f.untracked && f.index !== ' ' && f.index !== '.');
  return <div className="git-form"><h2>{tr('desktop_git_5c9f7a4e45')}</h2><p className="git-caption">{tr('desktop_git_staged_count', staged.length, status.branch || tr('desktop_git_448587ddd7'))}</p><div className="git-commit-files">{staged.map((file) => <button key={file.path} onClick={() => { git.update({ page: 'changes', mode: 'staged', path: file.path }); }}>{file.path}<span className="git-stats"><i>+{file.additions}</i> <b>−{file.deletions}</b></span></button>)}</div><label>{tr('desktop_git_26306d203c')}<textarea aria-label={tr('desktop_git_26306d203c')} rows={7} placeholder={tr('desktop_git_abb4177e47')} value={git.view.message} onChange={(e) => git.update({ message: e.target.value })}/></label><p className="git-caption">{tr('desktop_git_5613c9b79f')}</p><div className="git-actions"><button className="git-primary" disabled={git.busy || !staged.length || !git.view.message.trim() || status.files.some((f) => f.conflict)} onClick={async () => { if (await git.run({ kind: 'commit', message: git.view.message, token: status.token })) git.update({ message: '' }); }}>{tr('desktop_git_82a9c46ffa')}</button><button onClick={() => git.update({ page: 'sync' })}>{tr('desktop_git_2fb6d97a06')}</button><button onClick={() => git.update({ page: 'changes' })}>{tr('desktop_git_6a47708f4e')}</button></div></div>;
}
function Sync() {
  const git = useGit(); const status = git.status!; const [remote, setRemote] = useState(status.remotes.includes('origin') ? 'origin' : status.remotes[0] ?? ''); const [branch, setBranch] = useState(status.branch); const [merge, setMerge] = useState<{ branch: string; token: string }>();
  // Push sends `HEAD:refs/heads/<branch>`. This panel stays mounted across a
  // checkout, so without re-seeding, switching branches would publish the new
  // HEAD onto the previously checked-out branch's remote ref.
  const checkedOut = useRef(status.branch);
  useEffect(() => { if (status.branch !== checkedOut.current) { checkedOut.current = status.branch; setBranch(status.branch); } }, [status.branch]);
  const locked = git.busy || status.busy || !!status.files.length;
  return <div className="git-form"><h2>{tr('desktop_git_6be489c67b')}</h2><p>{tr('desktop_git_sync_counts', status.ahead, status.behind)}</p><p className="git-caption">{tr('desktop_git_upstream', status.upstream ?? tr('desktop_git_4895f73177'))}</p>{!status.remotes.length ? <p>{tr('desktop_git_ddfa13fe30')}</p> : <><label>{tr('desktop_git_ffa98e02f7')}<select aria-label={tr('desktop_git_ffa98e02f7')} value={remote} onChange={(e) => setRemote(e.target.value)}>{status.remotes.map((r) => <option key={r}>{r}</option>)}</select></label><label>{tr('desktop_git_354ff2bd47')}<input aria-label={tr('desktop_git_354ff2bd47')} value={branch} onChange={(e) => setBranch(e.target.value)}/></label><p className="git-caption">{tr('desktop_git_sync_caption', remote, branch || tr('desktop_git_71bf03ad1f'))}</p><div className="git-actions"><button disabled={git.busy || !remote} onClick={() => void git.run({ kind: 'fetch', remote })}>{tr('desktop_git_cd7d61bf7e')}</button><button disabled={locked || !branch || !remote} onClick={() => void git.run({ kind: 'pull', remote, branch, token: status.token })}>{tr('desktop_git_b66d044181')}</button><button className="git-primary" disabled={git.busy || !branch || !remote || !status.branch} onClick={() => void git.run({ kind: 'push', remote, branch, setUpstream: !status.upstream, token: status.token })}>{status.upstream ? tr('desktop_git_731ce7ed80') : tr('desktop_git_4ad272488a')}</button></div><button disabled={locked || !branch || !remote} onClick={() => setMerge({ branch: `${remote}/${branch}`, token: status.token })}>{tr('desktop_git_8eb3a07dc4')}</button>{locked && !git.busy && <p className="git-caption">{tr('desktop_git_31b8fd9c3f')}</p>}</>}<button onClick={git.terminal}>{tr('desktop_git_acb1f43d28')}</button>{merge && <Confirm title={tr('desktop_git_bf3f47b7a5')} description={tr('desktop_git_7bf7639c89', remote, branch, status.branch)} onClose={() => setMerge(undefined)} onConfirm={() => git.run({ kind: 'merge', branch: merge.branch, token: merge.token })}/>}</div>;
}
function History() {
  const git = useGit(); const [commits, setCommits] = useState<GitCommit[]>([]); const [hasMore, setHasMore] = useState(true); const [loading, setLoading] = useState(false); const [selected, setSelected] = useState<GitCommit>(); const request = useRef(0);
  useEffect(() => { const id = ++request.current; setLoading(true); void git.read({ kind: 'history', skip: 0, limit: 40 }).then((r) => { if (id === request.current) { setCommits(r?.commits ?? []); setHasMore((r?.commits?.length ?? 0) === 40); setLoading(false); } }); return () => { request.current++; }; }, [git.status?.head, git.read]);
  if (selected) return <div className="git-history-detail"><button className="git-back" onClick={() => setSelected(undefined)}>{tr('desktop_git_dd1768e5c8')}</button><div className="git-commit-meta"><h3>{selected.subject}</h3><p>{selected.author} · {selected.date}</p><code>{selected.id}</code>{selected.body && <pre>{selected.body}</pre>}</div><Changes fixedMode="commit" fixedTarget={selected.id}/></div>;
  return <div className="git-history">{commits.map((commit) => <button className="git-history-row" key={commit.id} onClick={() => { git.update({ path: '' }); setSelected(commit); }}><span className="git-history-dot"/><span><strong>{commit.subject}</strong><small>{commit.author} · {commit.date}</small></span><code>{commit.id.slice(0, 7)}</code></button>)}{!commits.length && !loading && <div className="git-empty">{tr('desktop_git_053cea8685')}</div>}{hasMore && <button disabled={loading} onClick={async () => { const id = request.current; setLoading(true); const result = await git.read({ kind: 'history', skip: commits.length, limit: 40 }); if (id !== request.current) return; setCommits((old) => [...old, ...(result?.commits ?? [])]); setHasMore((result?.commits?.length ?? 0) === 40); setLoading(false); }}>{loading ? tr('desktop_git_ba3bbbe10d') : tr('desktop_git_166d09744c')}</button>}</div>;
}
function Stashes() {
  const git = useGit(); const [stashes, setStashes] = useState<GitStash[]>([]); const [selected, setSelected] = useState<GitStash>(); const [message, setMessage] = useState(''); const [include, setInclude] = useState(false); const [drop, setDrop] = useState<GitStash>();
  useEffect(() => { let active = true; void git.read({ kind: 'stashList' }).then((r) => { if (active) setStashes(r?.stashes ?? []); }); return () => { active = false; }; }, [git.status?.token, git.notice, git.read]);
  if (selected) return <div className="git-history-detail"><button className="git-back" onClick={() => setSelected(undefined)}>{tr('desktop_git_f94ed142db')}</button><div className="git-commit-meta"><h3>{selected.subject}</h3><p>{selected.ref} · {selected.date}</p></div><Changes fixedMode="stash" fixedTarget={selected.id}/></div>;
  return <div className="git-form"><h2>{tr('desktop_git_e45b6015e5')}</h2><label>{tr('desktop_git_ec8df3d159')}<input aria-label={tr('desktop_git_ec8df3d159')} value={message} onChange={(e) => setMessage(e.target.value)} placeholder={tr('desktop_git_005119da15')}/></label><label className="git-checkbox"><input type="checkbox" checked={include} onChange={(e) => setInclude(e.target.checked)}/>{tr('desktop_git_7831c421fb')}</label><button className="git-primary" disabled={git.busy || git.status?.busy || !git.status?.files.length} onClick={async () => { if (await git.run({ kind: 'stashCreate', message, includeUntracked: include, token: git.status!.token })) setMessage(''); }}>{tr('desktop_git_48ac1a3287')}</button>{stashes.map((stash) => <article className="git-stash-row" key={stash.id}><button onClick={() => { git.update({ path: '' }); setSelected(stash); }}><strong>{stash.subject}</strong><small>{stash.ref} · {stash.date}</small></button><div className="git-actions"><button disabled={git.busy || git.status?.busy || !!git.status?.files.length} onClick={() => void git.run({ kind: 'stashApply', id: stash.id, token: git.status!.token })}>{tr('settings_provider_apply')}</button><button disabled={git.busy} onClick={() => setDrop(stash)}>{tr('common_delete')}</button></div></article>)}{!stashes.length && <p className="git-caption">{tr('desktop_git_a479f503ba')}</p>}<p className="git-caption">{tr('desktop_git_434d10c0af')}</p>{drop && <Confirm title={tr('desktop_git_af9a82db47')} description={tr('desktop_git_f5b2f91080', drop.ref, drop.subject)} onClose={() => setDrop(undefined)} onConfirm={() => git.run({ kind: 'stashDrop', id: drop.id, token: git.status!.token })}/>}</div>;
}
function MergeControls() {
  const git = useGit(); const [abort, setAbort] = useState<string>(); const [message, setMessage] = useState('Merge branch');
  return <div className="git-merge"><strong>{tr('desktop_git_945810042f')}</strong>{git.status?.mergeOwned ? <><input aria-label={tr('desktop_git_2132e189c7')} value={message} onChange={(e) => setMessage(e.target.value)}/><div className="git-actions"><button disabled={git.busy || git.status.busy || git.status.files.some((f) => f.conflict) || !message.trim()} onClick={() => void git.run({ kind: 'mergeContinue', message, token: git.status!.token })}>{tr('desktop_git_b435022dd9')}</button><button disabled={git.busy || git.status.busy} onClick={() => setAbort(git.status!.token)}>{tr('desktop_git_6c593be643')}</button></div></> : <p className="git-caption">{tr('desktop_git_9c4ed5a371')}</p>}{abort && <Confirm title={tr('desktop_git_049c1bec72')} description={tr('desktop_git_62f5427980')} onClose={() => setAbort(undefined)} onConfirm={() => git.run({ kind: 'mergeAbort', token: abort })}/>}</div>;
}
function Conflict({ path, onClose }: { path: string; onClose(): void }) {
  const git = useGit(); const [conflict, setConflict] = useState<GitConflict>(); const [content, setContent] = useState(''); const [saved, setSaved] = useState(false); const [side, setSide] = useState<'ours' | 'theirs' | undefined>(); const [savedToken, setSavedToken] = useState('');
  useEffect(() => { let active = true; void git.read({ kind: 'conflictRead', path }).then((r) => { if (active && r?.conflict) { setConflict(r.conflict); setContent(r.conflict.result); if (r.conflict.binary) setSide('ours'); } }); return () => { active = false; }; }, [path, git.read]);
  return <Dialog title={tr('desktop_git_94accfee2d', path)} onClose={onClose}>{!conflict ? <p>{tr('desktop_git_437469981f')}</p> : <><p className="git-caption">{tr('desktop_git_0098226f79')}</p>{conflict.binary ? <label>{tr('desktop_git_975f10ccff')}<select aria-label={tr('desktop_git_feee57a417')} value={side} onChange={(e) => { setSide(e.target.value as 'ours' | 'theirs'); setSaved(false); }}><option value="ours">{tr('desktop_git_84b8e9f505')}</option><option value="theirs">{tr('desktop_git_37c2c89293')}</option></select></label> : <><div className="git-conflict-sources">{(['base', 'ours', 'theirs'] as const).map((key) => <details key={key}><summary>{key === 'base' ? tr('desktop_git_7b47361aad') : key === 'ours' ? tr('desktop_git_84b8e9f505') : tr('desktop_git_37c2c89293')}{conflict[`${key}Exists`] === false ? tr('desktop_git_62d5c89ef7') : ''}</summary><pre>{conflict[key]}</pre></details>)}</div><div className="git-actions"><button onClick={() => { setContent(conflict.ours); setSide('ours'); setSaved(false); }}>{tr('desktop_git_a35de96690')}</button><button onClick={() => { setContent(conflict.theirs); setSide('theirs'); setSaved(false); }}>{tr('desktop_git_d6ebdd4d86')}</button><button onClick={() => { setContent(conflict.ours + (conflict.ours.endsWith('\n') ? '' : '\n') + conflict.theirs); setSide(undefined); setSaved(false); }}>{tr('desktop_git_93672212b4')}</button></div><label>{tr('desktop_git_6e7d50e84f')}<textarea className="mono" aria-label={tr('desktop_git_4a14bc19f4')} rows={14} value={content} onChange={(e) => { setContent(e.target.value); setSide(undefined); setSaved(false); }}/></label></>}<footer><button disabled={git.busy || git.status?.busy} onClick={async () => { const result = await git.run({ kind: 'conflictSave', path, ...(side ? { side } : { content }), token: conflict.token }); if (result) { if (result.status) { setConflict({ ...conflict, token: result.status.token }); setSavedToken(result.status.token); setSaved(true); } } }}>{tr('desktop_git_a6fdf772da')}</button><button className="git-primary" disabled={git.busy || git.status?.busy || !saved} onClick={async () => { if (await git.run({ kind: 'conflictResolve', path, token: savedToken })) onClose(); }}>{tr('desktop_git_d6d8eeddd8')}</button></footer>{git.error && <p role="alert">{git.error}</p>}</>}</Dialog>;
}
