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
    try { const result = await api.request(scope, request); if (!live.current || (currentKey.current !== key || generation.current !== epoch)) return undefined; setNotice(result.output?.trim() || 'Operation completed.'); await refresh(); return result; }
    catch (e) { if (live.current && (currentKey.current === key && generation.current === epoch)) setError(String(e instanceof Error ? e.message : e)); await refresh(); return undefined; }
    finally { if (mutation.current === epoch) mutation.current = null; if (live.current && (currentKey.current === key && generation.current === epoch)) setBusy(false); }
  }, [key, api, refresh, epoch]);
  return <GitContext.Provider value={{ scope, api, status: statusKey === key ? status : undefined, branches: statusKey === key ? branches : [], loading, busy, error, notice, refresh, read, run, view, update, open: (page) => { if (page) update({ page }); onOpen(); }, terminal: onTerminal }}>{children}</GitContext.Provider>;
}
function useGit() { const value = useContext(GitContext); if (!value) throw new Error('Git workspace is unavailable'); return value; }
function BranchIcon() { return <svg width="16" height="16" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><circle cx="5" cy="4" r="2"/><circle cx="5" cy="16" r="2"/><circle cx="15" cy="4" r="2"/><path d="M5 6v8m10-8v2c0 4-10 1-10 6"/></svg>; }
export function GitTopBar() {
  const t = useT();
  const git = useContext(GitContext); const [open, setOpen] = useState(false); const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { if (!open) return; const pointer = (e: PointerEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); }; const key = (e: KeyboardEvent) => { if (e.key === 'Escape') { setOpen(false); ref.current?.querySelector('button')?.focus(); } }; document.addEventListener('pointerdown', pointer); document.addEventListener('keydown', key); return () => { document.removeEventListener('pointerdown', pointer); document.removeEventListener('keydown', key); }; }, [open]);
  useEffect(() => { setOpen(false); }, [git?.scope?.projectPath, git?.scope?.sessionId]);
  if (!git?.api || !git.scope) return null;
  return <div className="git-topbar no-drag" ref={ref} style={themeStyle(t)}>
    <button className="git-branch-pill" aria-label="Git environment" aria-expanded={open} onClick={() => setOpen(!open)}><BranchIcon/><span>{git.status?.repository ? git.status.branch || git.status.head.slice(0, 8) || 'New repository' : 'Git'}</span>{!!git.status?.files.length && <small>{git.status.files.length}</small>}{!!(git.status?.ahead || git.status?.behind) && <small>↑{git.status.ahead} ↓{git.status.behind}</small>}<Icon name="chevron" size={12}/></button>
    {open && <div className="git-environment" role="dialog" aria-label="Git environment"><GitEnvironment onNavigate={() => setOpen(false)}/></div>}
  </div>;
}
export function GitEnvironment({ onNavigate }: { onNavigate?(): void }) {
  const git = useContext(GitContext); const t = useT(); const [anchor, setAnchor] = useState<DOMRect>();
  useEffect(() => { setAnchor(undefined); }, [git?.scope?.projectPath, git?.scope?.sessionId]);
  if (!git?.api || !git.scope) return null;
  const navigate = (page: Page) => { git.open(page); onNavigate?.(); };
  return <section className="git-environment-content" aria-label="Environment" style={themeStyle(t)}><h2>Environment</h2>
    <button onClick={() => navigate('changes')}><Icon name="file" size={18}/><span>Changes</span><small>{git.status?.files.length ?? ''}</small></button>
    <button data-git-branch-trigger="true" aria-label="Switch branch" aria-expanded={!!anchor} disabled={!git.status?.repository} onClick={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}><BranchIcon/><span>{git.status?.repository ? git.status.branch || git.status.head.slice(0, 8) || 'New repository' : git.loading ? 'Loading repository…' : 'No repository'}</span><Icon name="chevron" size={14}/></button>
    <button onClick={() => navigate('commit')}><Icon name="check" size={18}/><span>Commit or push</span></button>
    {git.error && <div className="git-message git-error" role="alert">{git.error}<button onClick={() => void git.refresh()}>Retry</button></div>}
    {anchor && <BranchPicker anchor={anchor} onClose={() => setAnchor(undefined)}/>}</section>;
}
function Dialog({ title, children, onClose, anchor }: { title: string; children: ReactNode; onClose(): void; anchor?: DOMRect }) {
  const t = useT(); const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { const dialog = ref.current; const previous = document.activeElement as HTMLElement | null; dialog?.showModal(); return () => { dialog?.close(); previous?.focus(); }; }, []);
  return <dialog ref={ref} className={`git-dialog${anchor ? ' git-branch-popover' : ''}`} style={{ ...themeStyle(t), ...(anchor ? { position: 'fixed', margin: 0, width: Math.min(420, window.innerWidth - 24), left: Math.max(12, Math.min(anchor.left - 428, window.innerWidth - 432)), top: Math.max(12, Math.min(anchor.top, window.innerHeight - 540)), maxHeight: 'min(520px, calc(100vh - 24px))' } : {}) }} onCancel={(e) => { e.preventDefault(); onClose(); }} aria-label={title}><header><h2>{title}</h2><button aria-label="Close dialog" onClick={onClose}>×</button></header>{children}</dialog>;
}
function BranchPicker({ onClose, anchor }: { onClose(): void; anchor?: DOMRect }) {
  const git = useGit(); const [query, setQuery] = useState(''); const [name, setName] = useState('');
  const locked = git.busy || !!git.status?.busy || !!git.status?.files.length;
  const checkout = async (branch: string, create = false) => { if (await git.run({ kind: 'checkout', branch, create, token: git.status!.token })) onClose(); };
  return <Dialog title="Switch branch" onClose={onClose} anchor={anchor}><input autoFocus aria-label="Search branches" placeholder={`Search ${git.scope?.projectPath.split(/[\\/]/).filter(Boolean).pop() ?? 'project'} branches`} value={query} onChange={(e) => setQuery(e.target.value)}/>{locked && <p className="git-message">{git.status?.busy ? 'A task is running in this worktree.' : 'Commit or stash your changes before switching branches.'} <button onClick={() => { git.open('commit'); onClose(); }}>Commit</button><button onClick={() => { git.open('stash'); onClose(); }}>Stash</button></p>}<div className="git-branch-list">{[false, true].map((remote) => <section key={String(remote)}><h3>{remote ? 'Remote branches' : 'Local branches'}</h3>{git.branches.filter((b) => b.remote === remote && b.name.toLowerCase().includes(query.toLowerCase())).sort((a, b) => Number(b.current) - Number(a.current) || a.name.localeCompare(b.name)).map((b) => <button key={b.name} className="git-branch-row" aria-current={b.current ? 'true' : undefined} disabled={locked || b.current || (!!b.worktree && b.worktree !== git.status?.root)} onClick={() => void checkout(b.name)}><BranchIcon/><span>{b.name}{b.current && <small>Uncommitted: {git.status?.files.length ?? 0} files</small>}{!b.current && b.worktree && b.worktree !== git.status?.root && <small>In use: {b.worktree}</small>}{b.upstream && <small>{b.upstream}</small>}</span>{b.current && <span>✓</span>}</button>)}</section>)}</div><form onSubmit={(e) => { e.preventDefault(); void checkout(name.trim(), true); }}><label>New branch<input aria-label="New branch name" placeholder="Branch name" value={name} onChange={(e) => setName(e.target.value)}/></label><button className="git-primary" disabled={locked || !name.trim()}>Create and switch</button></form>{git.error && <p role="alert">{git.error}</p>}</Dialog>;
}
function Confirm({ title, description, paths, onConfirm, onClose }: { title: string; description: string; paths?: string[]; onConfirm(): Promise<unknown>; onClose(): void }) {
  const [busy, setBusy] = useState(false);
  return <Dialog title={title} onClose={onClose}><p>{description}</p>{paths && <ul className="git-confirm-paths">{paths.map((path) => <li key={path}>{path}</li>)}</ul>}<footer><button onClick={onClose} disabled={busy}>Cancel</button><button className="git-danger" disabled={busy} onClick={async () => { setBusy(true); await onConfirm(); onClose(); }}>{busy ? 'Working…' : title}</button></footer></Dialog>;
}
function themeStyle(t: ReturnType<typeof useT>): CSSProperties { return { '--git-bg': t.surface, '--git-fg': t.text, '--git-muted': t.text3, '--git-border': t.border, '--git-hover': t.surfaceHover, '--git-accent': t.accent, '--git-danger': t.danger, '--git-add': t.dark ? 'rgba(74, 190, 110, .13)' : 'rgba(49, 160, 80, .09)', '--git-remove': t.dark ? 'rgba(230, 83, 100, .13)' : 'rgba(220, 65, 85, .09)', ...Object.fromEntries(Object.entries(t.syntax).map(([syntaxClass, color]) => [`--git-syntax-${syntaxClass}`, color])), color: t.text } as CSSProperties; }
export function GitReview() {
  const git = useContext(GitContext);
  return <GitReviewContent key={`${git?.scope?.projectPath ?? ''}\0${git?.scope?.sessionId ?? ''}`}/>;
}
function GitReviewContent() {
  const git = useContext(GitContext); const t = useT(); const [picker, setPicker] = useState(false);
  if (!git) return <div className="git-empty">Git management is available in Desktop.</div>;
  const { status, view } = git;
  return <div className="git-review" style={themeStyle(t)}>
    <div className="git-review-toolbar"><select aria-label="Review view" value={view.page} onChange={(e) => git.update({ page: e.target.value as Page })}><option value="changes">Changes</option><option value="history">History</option><option value="stash">Stashes</option><option value="commit">Commit</option><option value="sync">Sync</option></select><span className="git-spacer"/><button title="Refresh repository" aria-label="Refresh repository" disabled={git.loading || git.busy} onClick={() => void git.refresh()}><Icon name="refresh" size={16}/></button><button disabled={!status?.repository} aria-label="Switch branch" onClick={() => setPicker(true)}><BranchIcon/>{status?.branch || 'Detached HEAD'}</button></div>
    {git.error && <div className="git-message git-error" role="alert">{git.error}<button onClick={git.terminal}>Open terminal</button></div>}
    {git.busy && <div className="git-message" role="status">Running Git operation…</div>}
    {!git.busy && git.notice && <div className="git-message git-success" role="status">{git.notice}</div>}
    {status?.busy && <div className="git-message">A task is running. Operations that replace working files are unavailable.</div>}
    {!git.api ? <div className="git-empty">Git management requires the Desktop app.</div> : !status ? <div className="git-empty">{git.loading ? 'Reading repository…' : 'Repository unavailable.'}</div> : !status.available ? <div className="git-empty"><h3>Git is not installed</h3><p>Install Git and refresh this panel.</p><button onClick={git.terminal}>Open terminal</button></div> : !status.repository ? <div className="git-empty"><Icon name="file" size={36}/><h3>No Git repository</h3><p>Initialize Git in this project to track changes.</p><button className="git-primary" disabled={git.busy} onClick={() => void git.run({ kind: 'init' })}>Initialize repository</button></div> : <>
      {status.merging && <MergeControls/>}
      {view.page === 'changes' && <Changes/>}{view.page === 'history' && <History/>}{view.page === 'stash' && <Stashes/>}{view.page === 'commit' && <Commit/>}{view.page === 'sync' && <Sync/>}
    </>}{picker && <BranchPicker onClose={() => setPicker(false)}/>}
  </div>;
}
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
    {!fixedMode && <div className="git-compare"><select aria-label="Comparison" value={mode} onChange={(e) => git.update({ mode: e.target.value as Mode, path: '', target: '' })}><option value="working">Working tree</option><option value="staged">Staged</option><option value="branch">Branch</option></select>{mode === 'branch' && <><select aria-label="Base branch" value={view.base} onChange={(e) => git.update({ base: e.target.value, path: '' })}><option value="HEAD">HEAD</option>{git.branches.map((b) => <option key={b.name}>{b.name}</option>)}</select><span title="Common ancestor to target">→</span><select aria-label="Target branch" value={target} onChange={(e) => git.update({ target: e.target.value, path: '' })}><option value="">Select branch</option>{git.branches.map((b) => <option key={b.name}>{b.name}</option>)}</select></>}</div>}
    {mode === 'branch' && target && <div className="git-caption">Common ancestor of {view.base} and {target} → {target}</div>}
    <div className="git-file-filter"><input aria-label="Filter files" placeholder="Filter files…" value={view.filter} onChange={(e) => git.update({ filter: e.target.value })}/><button aria-label={view.tree ? 'Use flat file list' : 'Use directory tree'} aria-pressed={view.tree} onClick={() => git.update({ tree: !view.tree })}><Icon name="folder" size={17}/></button><button aria-label="Side by side diff" aria-pressed={view.split} onClick={() => git.update({ split: !view.split })}>±</button></div>
    <div className="git-files">{groups.map(([label, entries]) => { const files = entries.filter((f) => f.path.toLowerCase().includes(view.filter.toLowerCase())).sort((a, b) => a.path.localeCompare(b.path)); return files.length > 0 && <section key={label}><header><h3>{label} <small>{files.length}</small></h3>{(label === 'Staged' || label === 'Changes' || label === 'Untracked') && <button disabled={git.busy} onClick={() => void git.run({ kind: label === 'Staged' ? 'unstage' : 'stage', paths: files.map((f) => f.path), token: status!.token })}>{label === 'Staged' ? 'Unstage all' : 'Stage all'}</button>}</header><DirectoryFiles files={files} tree={view.tree} renderFile={(file) => (<div className={`git-file-row${view.path === file.path ? ' selected' : ''}`}><button className="git-file-select" onClick={() => select(file, label)} title={file.path}><span className="git-file-status">{file.conflict ? '!' : file.untracked ? '?' : label === 'Staged' ? file.index : file.working}</span><span>{view.tree ? file.path.split('/').pop() : file.path}{file.oldPath && <small> ← {file.oldPath}</small>}{file.submodule && <small> submodule</small>}</span><small className="git-stats">{file.binary ? 'binary' : <><i>+{file.additions}</i> <b>−{file.deletions}</b></>}</small></button>{!file.conflict && (label === 'Staged' || label === 'Changes' || label === 'Untracked') && <button title={label === 'Staged' ? 'Unstage file' : 'Stage file'} aria-label={`${label === 'Staged' ? 'Unstage' : 'Stage'} ${file.path}`} disabled={git.busy} onClick={() => void git.run({ kind: label === 'Staged' ? 'unstage' : 'stage', paths: [file.path], token: status!.token })}>{label === 'Staged' ? '−' : '+'}</button>}{(label === 'Changes' || label === 'Untracked') && <button title="Discard file changes" aria-label={`Discard ${file.path}`} disabled={git.busy || status?.busy} onClick={() => setDiscard({ file, token: status!.token })}>↶</button>}</div>)}/></section>; })}</div>
    {catalog.length === 0 && <div className="git-empty"><Icon name="file" size={40}/><h3>{mode === 'branch' && !target ? 'Select a branch to compare' : 'No file changes yet'}</h3><p>Changes in this project will appear here.</p></div>}
    <div ref={scroll} className="git-diff-scroll" onScroll={(e) => { if (scrollReady.current) git.update({ scroll: e.currentTarget.scrollTop }); }}>{diff ? <Diff diff={diff} mode={mode} path={view.path} refreshing={loading}/> : loading && <div className="git-empty">Loading diff…</div>}</div>
    {discard && <Confirm title={discard.file.untracked ? 'Delete untracked file' : 'Discard changes'} description={discard.file.untracked ? 'This permanently deletes the selected untracked file.' : 'Replace unstaged changes with the staged version. This cannot be undone from this panel.'} paths={[discard.file.path]} onClose={() => setDiscard(undefined)} onConfirm={() => git.run({ kind: 'discard', paths: [discard.file.path], untracked: discard.file.untracked, token: discard.token })}/>}
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
  return <div className="git-diff" aria-label="File diff">
    <div className="git-diff-title">{path || 'All file changes'}</div>
    {diff.truncated && <div className="git-message">Preview truncated. Open the file or terminal to inspect the complete diff. Hunk actions are disabled.</div>}
    {diff.binary ? <div className="git-empty">Binary file changed. No text preview.</div> : hunks.length ? hunks.map((hunk, index) => {
      const lines = reviewLines(hunk);
      // Match CodeBlock's size limit so large diffs do not trigger thousands
      // of small syntax-highlighting passes while the review pane renders.
      const language = diff.patch.length <= EXPLICIT_HIGHLIGHT_MAX_CHARS ? reviewLanguage(path || hunk.filePath) : undefined;
      return <section className="git-hunk" key={`${hunk.heading}-${index}`}>
        <header><code>{hunk.heading}</code>{(mode === 'working' || mode === 'staged') && <button disabled={git.busy || refreshing || diff.token !== git.status?.token || diff.truncated} onClick={() => void git.run({ kind: mode === 'staged' ? 'unstage' : 'stage', patch: hunk.patch, token: diff.token })}>{mode === 'staged' ? 'Unstage hunk' : 'Stage hunk'}</button>}</header>
        {git.view.split
          ? <div className="git-split-diff">{splitReviewLines(lines).map(([left, right], i) => <div className="git-split-row" key={i}><DiffCell line={left} side="old" language={language}/><DiffCell line={right} side="next" language={language}/></div>)}</div>
          : <div>{lines.map((line, i) => <div className={`git-diff-line ${line.kind}`} key={i}><span className="git-line-number">{line.old}</span><span className="git-line-number">{line.next}</span><code>{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}<DiffCode text={line.text} language={line.kind === 'note' ? undefined : language}/></code></div>)}</div>}
      </section>;
    }) : diff.patch ? <pre className="git-raw-diff">{diff.patch}</pre> : <div className="git-empty">No changes in this comparison.</div>}
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
  return <div className="git-form"><h2>Commit staged changes</h2><p className="git-caption">{staged.length} staged files · {status.branch || 'detached HEAD'}</p><div className="git-commit-files">{staged.map((file) => <button key={file.path} onClick={() => { git.update({ page: 'changes', mode: 'staged', path: file.path }); }}>{file.path}<span className="git-stats"><i>+{file.additions}</i> <b>−{file.deletions}</b></span></button>)}</div><label>Commit message<textarea aria-label="Commit message" rows={7} placeholder="Describe why this change is needed…" value={git.view.message} onChange={(e) => git.update({ message: e.target.value })}/></label><p className="git-caption">Only staged changes will be committed. Git hooks and signing settings apply.</p><div className="git-actions"><button className="git-primary" disabled={git.busy || !staged.length || !git.view.message.trim() || status.files.some((f) => f.conflict)} onClick={async () => { if (await git.run({ kind: 'commit', message: git.view.message, token: status.token })) git.update({ message: '' }); }}>Commit</button><button onClick={() => git.update({ page: 'sync' })}>Push / sync</button><button onClick={() => git.update({ page: 'changes' })}>Review changes</button></div></div>;
}
function Sync() {
  const git = useGit(); const status = git.status!; const [remote, setRemote] = useState(status.remotes.includes('origin') ? 'origin' : status.remotes[0] ?? ''); const [branch, setBranch] = useState(status.branch); const [merge, setMerge] = useState<{ branch: string; token: string }>();
  // Push sends `HEAD:refs/heads/<branch>`. This panel stays mounted across a
  // checkout, so without re-seeding, switching branches would publish the new
  // HEAD onto the previously checked-out branch's remote ref.
  const checkedOut = useRef(status.branch);
  useEffect(() => { if (status.branch !== checkedOut.current) { checkedOut.current = status.branch; setBranch(status.branch); } }, [status.branch]);
  const locked = git.busy || status.busy || !!status.files.length;
  return <div className="git-form"><h2>Remote sync</h2><p>{status.ahead} ahead · {status.behind} behind</p><p className="git-caption">Upstream: {status.upstream ?? 'Not set'}</p>{!status.remotes.length ? <p>No remote configured. Add a remote in the terminal.</p> : <><label>Remote<select aria-label="Remote" value={remote} onChange={(e) => setRemote(e.target.value)}>{status.remotes.map((r) => <option key={r}>{r}</option>)}</select></label><label>Remote branch<input aria-label="Remote branch" value={branch} onChange={(e) => setBranch(e.target.value)}/></label><p className="git-caption">{remote} / {branch || 'Select a branch'}. Pull only fast-forwards. Push never forces.</p><div className="git-actions"><button disabled={git.busy || !remote} onClick={() => void git.run({ kind: 'fetch', remote })}>Fetch</button><button disabled={locked || !branch || !remote} onClick={() => void git.run({ kind: 'pull', remote, branch, token: status.token })}>Pull</button><button className="git-primary" disabled={git.busy || !branch || !remote || !status.branch} onClick={() => void git.run({ kind: 'push', remote, branch, setUpstream: !status.upstream, token: status.token })}>{status.upstream ? 'Push' : 'Push and set upstream'}</button></div><button disabled={locked || !branch || !remote} onClick={() => setMerge({ branch: `${remote}/${branch}`, token: status.token })}>Merge remote branch…</button>{locked && !git.busy && <p className="git-caption">Commit or stash changes and wait for active tasks before pulling or merging.</p>}</>}<button onClick={git.terminal}>Open terminal</button>{merge && <Confirm title="Merge branch" description={`Merge ${remote}/${branch} into ${status.branch}. Conflicts will appear in Review.`} onClose={() => setMerge(undefined)} onConfirm={() => git.run({ kind: 'merge', branch: merge.branch, token: merge.token })}/>}</div>;
}
function History() {
  const git = useGit(); const [commits, setCommits] = useState<GitCommit[]>([]); const [hasMore, setHasMore] = useState(true); const [loading, setLoading] = useState(false); const [selected, setSelected] = useState<GitCommit>(); const request = useRef(0);
  useEffect(() => { const id = ++request.current; setLoading(true); void git.read({ kind: 'history', skip: 0, limit: 40 }).then((r) => { if (id === request.current) { setCommits(r?.commits ?? []); setHasMore((r?.commits?.length ?? 0) === 40); setLoading(false); } }); return () => { request.current++; }; }, [git.status?.head, git.read]);
  if (selected) return <div className="git-history-detail"><button className="git-back" onClick={() => setSelected(undefined)}>← History</button><div className="git-commit-meta"><h3>{selected.subject}</h3><p>{selected.author} · {selected.date}</p><code>{selected.id}</code>{selected.body && <pre>{selected.body}</pre>}</div><Changes fixedMode="commit" fixedTarget={selected.id}/></div>;
  return <div className="git-history">{commits.map((commit) => <button className="git-history-row" key={commit.id} onClick={() => { git.update({ path: '' }); setSelected(commit); }}><span className="git-history-dot"/><span><strong>{commit.subject}</strong><small>{commit.author} · {commit.date}</small></span><code>{commit.id.slice(0, 7)}</code></button>)}{!commits.length && !loading && <div className="git-empty">No commits yet.</div>}{hasMore && <button disabled={loading} onClick={async () => { const id = request.current; setLoading(true); const result = await git.read({ kind: 'history', skip: commits.length, limit: 40 }); if (id !== request.current) return; setCommits((old) => [...old, ...(result?.commits ?? [])]); setHasMore((result?.commits?.length ?? 0) === 40); setLoading(false); }}>{loading ? 'Loading…' : 'Load more commits'}</button>}</div>;
}
function Stashes() {
  const git = useGit(); const [stashes, setStashes] = useState<GitStash[]>([]); const [selected, setSelected] = useState<GitStash>(); const [message, setMessage] = useState(''); const [include, setInclude] = useState(false); const [drop, setDrop] = useState<GitStash>();
  useEffect(() => { let active = true; void git.read({ kind: 'stashList' }).then((r) => { if (active) setStashes(r?.stashes ?? []); }); return () => { active = false; }; }, [git.status?.token, git.notice, git.read]);
  if (selected) return <div className="git-history-detail"><button className="git-back" onClick={() => setSelected(undefined)}>← Stashes</button><div className="git-commit-meta"><h3>{selected.subject}</h3><p>{selected.ref} · {selected.date}</p></div><Changes fixedMode="stash" fixedTarget={selected.id}/></div>;
  return <div className="git-form"><h2>Stashes</h2><label>Stash message<input aria-label="Stash message" value={message} onChange={(e) => setMessage(e.target.value)} placeholder="Work in progress"/></label><label className="git-checkbox"><input type="checkbox" checked={include} onChange={(e) => setInclude(e.target.checked)}/>Include untracked files</label><button className="git-primary" disabled={git.busy || git.status?.busy || !git.status?.files.length} onClick={async () => { if (await git.run({ kind: 'stashCreate', message, includeUntracked: include, token: git.status!.token })) setMessage(''); }}>Create stash</button>{stashes.map((stash) => <article className="git-stash-row" key={stash.id}><button onClick={() => { git.update({ path: '' }); setSelected(stash); }}><strong>{stash.subject}</strong><small>{stash.ref} · {stash.date}</small></button><div className="git-actions"><button disabled={git.busy || git.status?.busy || !!git.status?.files.length} onClick={() => void git.run({ kind: 'stashApply', id: stash.id, token: git.status!.token })}>Apply</button><button disabled={git.busy} onClick={() => setDrop(stash)}>Delete</button></div></article>)}{!stashes.length && <p className="git-caption">No saved stashes.</p>}<p className="git-caption">Applying a stash keeps it in this list.</p>{drop && <Confirm title="Delete stash" description={`Permanently delete ${drop.ref}: ${drop.subject}?`} onClose={() => setDrop(undefined)} onConfirm={() => git.run({ kind: 'stashDrop', id: drop.id, token: git.status!.token })}/>}</div>;
}
function MergeControls() {
  const git = useGit(); const [abort, setAbort] = useState<string>(); const [message, setMessage] = useState('Merge branch');
  return <div className="git-merge"><strong>Merge in progress</strong>{git.status?.mergeOwned ? <><input aria-label="Merge commit message" value={message} onChange={(e) => setMessage(e.target.value)}/><div className="git-actions"><button disabled={git.busy || git.status.busy || git.status.files.some((f) => f.conflict) || !message.trim()} onClick={() => void git.run({ kind: 'mergeContinue', message, token: git.status!.token })}>Complete merge</button><button disabled={git.busy || git.status.busy} onClick={() => setAbort(git.status!.token)}>Abort merge…</button></div></> : <p className="git-caption">This merge was started outside Review. Complete or abort it in the terminal.</p>}{abort && <Confirm title="Abort merge" description="Discard this merge's in-progress resolution and restore the pre-merge state." onClose={() => setAbort(undefined)} onConfirm={() => git.run({ kind: 'mergeAbort', token: abort })}/>}</div>;
}
function Conflict({ path, onClose }: { path: string; onClose(): void }) {
  const git = useGit(); const [conflict, setConflict] = useState<GitConflict>(); const [content, setContent] = useState(''); const [saved, setSaved] = useState(false); const [side, setSide] = useState<'ours' | 'theirs' | undefined>(); const [savedToken, setSavedToken] = useState('');
  useEffect(() => { let active = true; void git.read({ kind: 'conflictRead', path }).then((r) => { if (active && r?.conflict) { setConflict(r.conflict); setContent(r.conflict.result); if (r.conflict.binary) setSide('ours'); } }); return () => { active = false; }; }, [path, git.read]);
  return <Dialog title={`Resolve ${path}`} onClose={onClose}>{!conflict ? <p>Loading conflict…</p> : <><p className="git-caption">Save a resolution, then explicitly mark this file resolved.</p>{conflict.binary ? <label>Binary version<select aria-label="Binary conflict version" value={side} onChange={(e) => { setSide(e.target.value as 'ours' | 'theirs'); setSaved(false); }}><option value="ours">Current (ours)</option><option value="theirs">Incoming (theirs)</option></select></label> : <><div className="git-conflict-sources">{(['base', 'ours', 'theirs'] as const).map((key) => <details key={key}><summary>{key === 'base' ? 'Base' : key === 'ours' ? 'Current (ours)' : 'Incoming (theirs)'}{conflict[`${key}Exists`] === false ? ' — deleted' : ''}</summary><pre>{conflict[key]}</pre></details>)}</div><div className="git-actions"><button onClick={() => { setContent(conflict.ours); setSide('ours'); setSaved(false); }}>Use current</button><button onClick={() => { setContent(conflict.theirs); setSide('theirs'); setSaved(false); }}>Use incoming</button><button onClick={() => { setContent(conflict.ours + (conflict.ours.endsWith('\n') ? '' : '\n') + conflict.theirs); setSide(undefined); setSaved(false); }}>Keep both</button></div><label>Result<textarea className="mono" aria-label="Conflict result" rows={14} value={content} onChange={(e) => { setContent(e.target.value); setSide(undefined); setSaved(false); }}/></label></>}<footer><button disabled={git.busy || git.status?.busy} onClick={async () => { const result = await git.run({ kind: 'conflictSave', path, ...(side ? { side } : { content }), token: conflict.token }); if (result) { if (result.status) { setConflict({ ...conflict, token: result.status.token }); setSavedToken(result.status.token); setSaved(true); } } }}>Save result</button><button className="git-primary" disabled={git.busy || git.status?.busy || !saved} onClick={async () => { if (await git.run({ kind: 'conflictResolve', path, token: savedToken })) onClose(); }}>Mark resolved</button></footer>{git.error && <p role="alert">{git.error}</p>}</>}</Dialog>;
}
