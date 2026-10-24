import { LocalAppMarket, LegacyLocalApps, LocalAppPublish } from './LocalAppMarket';
import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { DesktopLocalApp, LocalAppLog, LocalAppPhase, LocalAppVersion } from '../../shared/localApps';
import type { ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { useT } from '../theme/ThemeContext';
import { LocalAppAgentQuestions, LocalAppAgentSettingsDialog } from './LocalAppAgentSettings';

function appsApi() { const api = window.lingxi?.localApps; if (!api) throw new Error('Desktop application host is unavailable'); return api; }
const phaseLabels: Record<LocalAppPhase, string> = { stopped: 'Ready to open', installing: 'Installing dependencies', building: 'Building', starting: 'Starting', running: 'Running', stopping: 'Stopping', failed: 'Needs attention' };
export function AppsGlyph({ size = 20 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" aria-hidden="true"><rect x="3" y="3" width="7" height="7" rx="1.8"/><rect x="14" y="3" width="7" height="7" rx="1.8"/><rect x="3" y="14" width="7" height="7" rx="1.8"/><rect x="14" y="14" width="7" height="7" rx="1.8"/></svg>;
}
export function LocalApps({ visible, onCreate }: { visible: boolean; onCreate: () => void }) {
  const t = useT();
  const [tab, setTab] = useState<'mine' | 'market'>('mine');
  const [publication, setPublication] = useState<DesktopLocalApp>();
  const [apps, setApps] = useState<DesktopLocalApp[]>([]);
  const [query, setQuery] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [agentSettings, setAgentSettings] = useState<string | null>(null);
  const [logs, setLogs] = useState<LocalAppLog[]>([]);
  const [versions, setVersions] = useState<LocalAppVersion[]>([]);
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const token = ++request.current;
    try {
      const [rows, output, releases] = await Promise.all([appsApi().list(), selected ? appsApi().logs(selected) : Promise.resolve([]), selected ? appsApi().versions(selected) : Promise.resolve([])]);
      if (request.current === token) { setApps(rows); setLogs(output); setVersions(releases); }
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
    <nav className="local-app-tabs" aria-label="Application library"><button aria-current={tab === 'mine' ? 'page' : undefined} onClick={() => setTab('mine')}>My apps</button><button aria-current={tab === 'market' ? 'page' : undefined} onClick={() => setTab('market')}>Market</button></nav>
    <LocalAppMarket visible={visible && tab === 'market'} installed={apps} onInstalled={() => void refresh()}/>
    <div hidden={tab !== 'mine'}>
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
        <div className="local-app-actions"><button onClick={() => { setSelected(selected === app.id ? null : app.id); }}>Details &amp; logs</button><button disabled={Boolean(app.marketOrigin && !app.sourcePath && !app.manifest.remix?.allowed)} onClick={() => void act(app.id, () => app.marketOrigin && !app.sourcePath ? appsApi().remix(app.id) : appsApi().edit(app.id))}>{app.marketOrigin && !app.sourcePath ? 'Customize' : 'Continue developing'}</button>
          {app.manifest.capabilities?.includes('agent') && <button onClick={() => setAgentSettings(app.id)}>Agent</button>}
          <details><summary aria-label={`Manage ${app.name}`}>•••</summary><div className="local-app-menu">
            <button onClick={() => void act(app.id, () => appsApi().restart(app.id))}>Restart</button>
            <button onClick={() => void act(app.id, () => appsApi().stop(app.id))}>Stop</button>
            <button onClick={() => void act(app.id, () => appsApi().repair(app.id))}>Repair environment</button>
            <button onClick={() => void act(app.id, () => appsApi().openData(app.id))}>Open data folder</button>
            <label><input type="checkbox" checked={app.background} onChange={event => void act(app.id, () => appsApi().setBackground(app.id, event.target.checked))}/> Keep running when closed</label>
            <button onClick={() => setPublication(app)}>Publish to market…</button>
            {app.marketOrigin && !app.marketOrigin.remixedFrom && <button onClick={() => void act(app.id, () => appsApi().marketInstall(app.marketOrigin!.appId, app.id, app.revision))}>Check for update</button>}
            {(!app.marketOrigin || app.sourcePath) && <button onClick={() => void act(app.id, () => appsApi().restoreSource(app.id))}>Restore source copy</button>}
            {app.manifest.remix?.allowed && <button onClick={() => void act(app.id, () => appsApi().remix(app.id))}>Create a custom copy</button>}
            <button onClick={() => void act(app.id, () => appsApi().revoke(app.id))}>Revoke permissions</button>
            <button className="local-app-delete" onClick={() => void act(app.id, () => appsApi().remove(app.id))}>Remove app…</button>
          </div></details>
        </div>
      </article>)}
    </div>}
    {apps.length > 0 && !filtered.length && <p className="local-app-no-results">No apps match “{query}”.</p>}
    {selected && <aside className="local-app-logs" aria-label="Application details"><header><strong>{apps.find(a => a.id === selected)?.name ?? 'Application'}</strong><button onClick={() => setSelected(null)} aria-label="Close details">×</button></header>
      <h3>Versions</h3><p>Switching code keeps application data. Data format changes require a verified migration.</p>
      {versions.map(version => { const app = apps.find(a => a.id === selected); return <div key={version.revision} className="local-app-actions"><span>v{version.revision} · {new Date(version.createdAtMs).toLocaleString()}{app?.currentRevision === version.revision ? ' · Current' : ''}</span><button disabled={busy === selected || !app || app.currentRevision === undefined || app.currentRevision === version.revision} onClick={() => app?.currentRevision !== undefined && void act(selected, () => appsApi().useVersion(selected, version.revision, app.currentRevision!))}>Use version</button></div>; })}
      <h3>Permissions</h3><p>{apps.find(a => a.id === selected)?.grants.join(', ') || 'No capabilities granted for the latest release.'}</p>
      <h3>Logs</h3><pre>{logs.length ? logs.map(l => `[${new Date(l.time).toLocaleTimeString()}] ${l.process}: ${l.text}`).join('\n') : 'No output yet. Open the app to start it.'}</pre></aside>}
    <LegacyLocalApps visible={visible && tab === 'mine'} onImported={() => void refresh()}/>
    </div>
    {publication && <LocalAppPublish key={publication.id} app={publication} onClose={() => setPublication(undefined)}/>}
    {agentSettings && apps.find(app => app.id === agentSettings) && <LocalAppAgentSettingsDialog key={agentSettings} app={apps.find(app => app.id === agentSettings)!} onClose={() => setAgentSettings(null)} onSaved={() => void refresh()}/>}
    <LocalAppAgentQuestions/>
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
