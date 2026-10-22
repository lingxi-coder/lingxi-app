import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { DesktopLocalApp, LocalAppLog, LocalAppPhase } from '../../shared/localApps';
import type { ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { useT } from '../theme/ThemeContext';

function appsApi() { const api = window.lingxi?.localApps; if (!api) throw new Error('Desktop application host is unavailable'); return api; }
const phaseLabels: Record<LocalAppPhase, string> = { stopped: 'Ready to open', installing: 'Installing dependencies', building: 'Building', starting: 'Starting', running: 'Running', stopping: 'Stopping', failed: 'Needs attention' };
export function AppsGlyph({ size = 20 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" aria-hidden="true"><rect x="3" y="3" width="7" height="7" rx="1.8"/><rect x="14" y="3" width="7" height="7" rx="1.8"/><rect x="3" y="14" width="7" height="7" rx="1.8"/><rect x="14" y="14" width="7" height="7" rx="1.8"/></svg>;
}
export function LocalApps({ visible, onCreate }: { visible: boolean; onCreate: () => void }) {
  const t = useT();
  const [apps, setApps] = useState<DesktopLocalApp[]>([]);
  const [query, setQuery] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [logs, setLogs] = useState<LocalAppLog[]>([]);
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const token = ++request.current;
    try {
      const [rows, output] = await Promise.all([appsApi().list(), selected ? appsApi().logs(selected) : Promise.resolve([])]);
      if (request.current === token) { setApps(rows); setLogs(output); }
    } catch (error) { if (request.current === token) setError(error instanceof Error ? error.message : String(error)); }
  }, [selected]);
  useEffect(() => {
    if (!visible || !window.lingxi?.localApps) return;
    void refresh(); const unsubscribe = appsApi().onChanged(() => { void refresh(); });
    return () => { request.current++; unsubscribe(); };
  }, [visible, refresh]);
  const act = async (id: string, operation: () => Promise<unknown>) => {
    setBusy(id); setError('');
    try { await operation(); await refresh(); } catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(null); }
  };
  const filtered = apps.filter(app => `${app.name} ${app.summary}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <section className="local-app-library" aria-label="Apps" style={{ '--apps-background': t.stageBg, '--apps-text': t.text, '--apps-muted': t.text3, '--apps-surface': t.surface, '--apps-border': t.border, '--apps-accent': t.accent, '--apps-error': t.danger } as CSSProperties}>
    <header className="local-app-library-header"><div><h1>Apps</h1><p>Made for you. Running on your computer.</p></div><button onClick={onCreate}>Create in chat</button></header>
    {error && <div className="local-app-error" role="alert">{error}<button onClick={() => { setError(''); void refresh(); }}>Retry</button></div>}
    <label className="local-app-search"><span className="sr-only">Search apps</span><input type="search" placeholder="Search your apps" value={query} onChange={event => setQuery(event.target.value)} /></label>
    {!apps.length && !error ? <div className="local-app-empty"><AppsGlyph size={38}/><h2>A place for your own apps</h2><p>Ask in chat for a tracker, a game, or a tool.<br/>Your finished app will be waiting here.</p><button onClick={onCreate}>Start creating</button></div> : <div className="local-app-grid">
      {filtered.map(app => <article className="local-app-tile" key={app.id}>
        <button className="local-app-tile-open" onClick={() => void act(app.id, () => appsApi().open(app.id))} disabled={busy === app.id}>
          <span className="local-app-avatar">{Array.from(app.name)[0]?.toLocaleUpperCase()}</span><h2>{app.name}</h2><p>{app.summary || 'Your local application'}</p>
        </button>
        <div className="local-app-tile-footer"><span className="local-app-phase" data-phase={app.phase}>{phaseLabels[app.phase]}</span><span>v{app.revision}</span></div>
        {app.runningRevision && app.runningRevision !== app.revision && <p className="local-app-update">Update ready — restart to use v{app.revision}</p>}
        {app.error && <p className="local-app-error">{app.error}</p>}
        <div className="local-app-actions"><button onClick={() => { setSelected(selected === app.id ? null : app.id); }}>Logs</button><button onClick={() => void act(app.id, () => appsApi().edit(app.id))}>Modify</button>
          <details><summary aria-label={`Manage ${app.name}`}>•••</summary><div className="local-app-menu">
            <button onClick={() => void act(app.id, () => appsApi().restart(app.id))}>Restart</button>
            <button onClick={() => void act(app.id, () => appsApi().stop(app.id))}>Stop</button>
            <button onClick={() => void act(app.id, () => appsApi().repair(app.id))}>Repair environment</button>
            <label><input type="checkbox" checked={app.background} onChange={event => void act(app.id, () => appsApi().setBackground(app.id, event.target.checked))}/> Keep running when closed</label>
            <button onClick={() => void act(app.id, () => appsApi().revoke(app.id))}>Revoke permissions</button>
            <button className="local-app-delete" onClick={() => void act(app.id, () => appsApi().remove(app.id))}>Delete app…</button>
          </div></details>
        </div>
      </article>)}
    </div>}
    {apps.length > 0 && !filtered.length && <p className="local-app-no-results">No apps match “{query}”.</p>}
    {selected && <aside className="local-app-logs" aria-label="Application logs"><header><strong>{apps.find(a => a.id === selected)?.name ?? 'Application'} · Logs</strong><button onClick={() => setSelected(null)} aria-label="Close logs">×</button></header><pre>{logs.length ? logs.map(l => `[${new Date(l.time).toLocaleTimeString()}] ${l.process}: ${l.text}`).join('\n') : 'No output yet. Open the app to start it.'}</pre></aside>}
  </section>;
}

export function LocalAppCard({ item }: { item: ToolRunItem }) {
  const t = useT();
  const app = localAppReference(item);
  const [error, setError] = useState('');
  const [opening, setOpening] = useState(false);
  const openingRef = useRef(false);
  if (!app) return null;
  return <div className="local-app-chat-card" style={{
    '--apps-text': t.text, '--apps-muted': t.text2, '--apps-surface': t.surface,
    '--apps-border': t.border, '--apps-accent': t.accent, '--apps-accent-bg': t.accentBg,
    '--apps-hover': t.surfaceHover, '--apps-error': t.danger,
  } as CSSProperties}>
    <span className="local-app-chat-icon"><AppsGlyph size={20}/></span>
    <div className="local-app-chat-info">
      <strong>{app.name}</strong>
      {app.summary && <p>{app.summary}</p>}
    </div>
    <button type="button" disabled={opening} aria-busy={opening} aria-label={`${opening ? 'Opening' : 'Open'} ${app.name}`} onClick={async () => {
      if (openingRef.current) return;
      openingRef.current = true;
      setOpening(true); setError('');
      try { await appsApi().open(app.id); }
      catch (error) { setError(error instanceof Error ? error.message : String(error)); }
      finally { openingRef.current = false; setOpening(false); }
    }}>{opening ? 'Opening…' : 'Open app'}</button>
    {error && <p className="local-app-chat-error" role="alert">{error}</p>}
  </div>;
}
