import { LocalAppPublish } from './LocalAppPublish';
import { useI18n } from '../i18n/useI18n';
import { tr } from '../i18n/locale';
import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { DesktopLocalApp, LocalAppLog, LocalAppPhase, LocalAppVersion } from '../../shared/localApps';
import type { ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { useT } from '../theme/ThemeContext';
import { LocalAppAgentQuestions, LocalAppAgentSettingsDialog } from './LocalAppAgentSettings';

function appsApi() { const api = window.lingxi?.localApps; if (!api) throw new Error('Desktop application host is unavailable'); return api; }
const phaseLabels: Record<LocalAppPhase, string> = { stopped: 'desktop_apps_ready', installing: 'desktop_apps_installing', building: 'desktop_apps_building', starting: 'desktop_apps_starting', running: 'desktop_apps_running', stopping: 'desktop_apps_stopping', failed: 'desktop_apps_attention' };
export function AppsGlyph({ size = 20 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" aria-hidden="true"><rect x="3" y="3" width="7" height="7" rx="1.8"/><rect x="14" y="3" width="7" height="7" rx="1.8"/><rect x="3" y="14" width="7" height="7" rx="1.8"/><rect x="14" y="14" width="7" height="7" rx="1.8"/></svg>;
}
export function LocalAppsTopBar({ onOpenMarket }: { onOpenMarket: () => void }) {
  const t = useT();
  return <header className="drag-region desktop-topbar local-app-topbar" style={{
    '--toolbar-material': t.sidebarBg, '--topbar-action-color': t.text3,
    '--topbar-action-hover-color': t.text, '--topbar-action-focus': t.surfaceHover,
    '--topbar-action-ring': t.borderStrong,
  } as CSSProperties}>
    <span className="local-app-topbar-title">{tr('apps_title')}</span>
    <button type="button" className="no-drag desktop-topbar-action local-app-market-button"
      aria-label="Market · Open in browser" title="Market · Open in browser" onClick={onOpenMarket}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" aria-hidden="true">
        <path d="M3 10h18l-2-6H5l-2 6Z"/><path d="M3 10v2a3 3 0 0 0 6 0 3 3 0 0 0 6 0 3 3 0 0 0 6 0v-2M5 15v6h14v-6M10 21v-5h4v5M9 10l1-6m5 6-1-6"/>
      </svg>
    </button>
  </header>;
}
export function LocalApps({ visible, onCreate }: { visible: boolean; onCreate: () => void }) {
  const t = useT();
  const { locale } = useI18n();
  const [publication, setPublication] = useState<DesktopLocalApp>();
  const [apps, setApps] = useState<DesktopLocalApp[]>([]);
  const [remoteSettings, setRemoteSettings] = useState<Record<string, boolean>>({});
  const [remoteConfigured, setRemoteConfigured] = useState(false);
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
      const [rows, output, releases, sharing, configuration] = await Promise.all([appsApi().list(), selected ? appsApi().logs(selected) : Promise.resolve([]), selected ? appsApi().versions(selected) : Promise.resolve([]), appsApi().remoteSettings(), appsApi().remoteConfiguration()]);
      if (request.current === token) { setApps(rows); setLogs(output); setVersions(releases); setRemoteSettings(sharing); setRemoteConfigured(configuration.configured); }
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
  const filtered = apps.filter(app => `${app.name} ${app.summary}`.toLocaleLowerCase(locale).includes(query.toLocaleLowerCase(locale)));
  return <section className="local-app-library" aria-label={tr('apps_title')} style={{ '--apps-background': t.stageBg, '--apps-text': t.text, '--apps-muted': t.text3, '--apps-surface': t.surface, '--apps-border': t.border, '--apps-accent': t.accent, '--apps-error': t.danger } as CSSProperties}>
    <header className="local-app-library-header"><div><h1>{tr('apps_title')}</h1><p>{tr('desktop_apps_tagline')}</p></div><button onClick={onCreate}>{tr('desktop_apps_create_chat')}</button></header>
    <p className="local-app-update">{tr('desktop_apps_deployment_hint')}</p>
    {!remoteConfigured && <p className="local-app-update">{tr('desktop_apps_remote_unconfigured')}</p>}
    {error && <div className="local-app-error" role="alert">{error}<button onClick={() => { setError(''); void refresh(); }}>{tr('common_retry')}</button></div>}
    <label className="local-app-search"><span className="sr-only">{tr('apps_search_hint')}</span><input type="search" placeholder={tr('desktop_apps_search')} value={query} onChange={event => setQuery(event.target.value)} /></label>
    {!apps.length && !error ? <div className="local-app-empty"><AppsGlyph size={38}/><h2>{tr('desktop_apps_empty_title')}</h2><p>{tr('desktop_apps_empty_body')}</p><button onClick={onCreate}>{tr('desktop_apps_start_creating')}</button></div> : <div className="local-app-grid">
      {filtered.map(app => <article className="local-app-tile" key={app.id}>
        <button className="local-app-tile-open" onClick={() => void act(app.id, () => appsApi().open(app.id))} disabled={busy === app.id}>
          <span className="local-app-avatar">{Array.from(app.name)[0]?.toLocaleUpperCase(locale)}</span><h2>{app.name}</h2><p>{app.summary || tr('desktop_apps_fallback_summary')}</p>
        </button>
        <div className="local-app-tile-footer"><span className="local-app-phase" data-phase={app.phase}>{tr(phaseLabels[app.phase])}</span><span>v{app.revision}</span></div>
        {remoteSettings[app.id] && <p className="local-app-update">{tr('desktop_apps_remote_shared')}</p>}
        {app.runningRevision && app.runningRevision !== app.revision && <p className="local-app-update">{tr('desktop_apps_update', app.revision)}</p>}
        {app.error && <p className="local-app-error">{app.error}</p>}
        <div className="local-app-actions"><button onClick={() => { setSelected(selected === app.id ? null : app.id); }}>{tr('desktop_logs')}</button><button disabled={Boolean(app.marketOrigin && !app.authoring?.sourceDirectory && !app.manifest.remix?.allowed)} onClick={() => void act(app.id, () => app.marketOrigin && !app.authoring?.sourceDirectory ? appsApi().remix(app.id) : appsApi().edit(app.id))}>{app.marketOrigin && !app.authoring?.sourceDirectory ? 'Customize' : tr('desktop_modify')}</button>
          {app.manifest.capabilities?.includes('agent') && <button onClick={() => setAgentSettings(app.id)}>Agent</button>}
          <details><summary aria-label={`Manage ${app.name}`}>•••</summary><div className="local-app-menu">
            <button onClick={() => void act(app.id, () => appsApi().restart(app.id))}>{tr('desktop_restart')}</button>
            <button onClick={() => void act(app.id, () => appsApi().stop(app.id))}>{tr('desktop_stop')}</button>
            <button onClick={() => void act(app.id, () => appsApi().repair(app.id))}>{tr('desktop_apps_repair')}</button>
            <button onClick={() => void act(app.id, () => appsApi().openData(app.id))}>Open data folder</button>
            <label><input type="checkbox" checked={app.background} onChange={event => void act(app.id, () => appsApi().setBackground(app.id, event.target.checked))}/> {tr('desktop_apps_background')}</label>
            <label><input type="checkbox" checked={remoteSettings[app.id] === true} disabled={busy === app.id || (!remoteConfigured && !remoteSettings[app.id])} onChange={event => void act(app.id, () => appsApi().setRemoteAccess(app.id, event.target.checked))}/> {tr('desktop_apps_remote_access')}</label>
            <button onClick={() => setPublication(app)}>Publish to market…</button>
            {app.marketOrigin && !app.marketOrigin.remixedFrom && <button onClick={() => void act(app.id, () => appsApi().marketInstall(app.marketOrigin!.appId, app.id, app.revision))}>Check for update</button>}
            {(!app.marketOrigin || app.authoring?.sourceDirectory) && <button onClick={() => void act(app.id, () => appsApi().restoreSource(app.id))}>Restore source copy</button>}
            {app.manifest.remix?.allowed && <button onClick={() => void act(app.id, () => appsApi().remix(app.id))}>Create a custom copy</button>}
            <button onClick={() => void act(app.id, () => appsApi().revoke(app.id))}>{tr('desktop_apps_revoke')}</button>
            <button className="local-app-delete" onClick={() => void act(app.id, () => appsApi().remove(app.id))}>{tr('desktop_apps_delete')}</button>
          </div></details>
        </div>
      </article>)}
    </div>}
    {apps.length > 0 && !filtered.length && <p className="local-app-no-results">{tr('desktop_apps_no_results', query)}</p>}
    {selected && <aside className="local-app-logs" aria-label={tr('desktop_apps_logs')}><header><strong>{apps.find(a => a.id === selected)?.name ?? tr('desktop_app')} · {tr('desktop_logs')}</strong><button onClick={() => setSelected(null)} aria-label={tr('desktop_close_logs')}>×</button></header>
      <h3>Versions</h3><p>Switching code keeps application data. Data format changes require a verified migration.</p>
      {versions.map(version => { const app = apps.find(a => a.id === selected); return <div key={version.revision} className="local-app-actions"><span>v{version.revision} · {new Date(version.createdAtMs).toLocaleString(locale)}{app?.currentRevision === version.revision ? ' · Current' : ''}</span><button disabled={busy === selected || !app || app.currentRevision === undefined || app.currentRevision === version.revision} onClick={() => app?.currentRevision !== undefined && void act(selected, () => appsApi().useVersion(selected, version.revision, app.currentRevision!))}>Use version</button></div>; })}
      <h3>Permissions</h3><p>{apps.find(a => a.id === selected)?.grants.join(', ') || 'No capabilities granted for the latest release.'}</p>
      <h3>{tr('desktop_logs')}</h3><pre>{logs.length ? logs.map(l => `[${new Date(l.time).toLocaleTimeString(locale)}] ${l.process}: ${l.text}`).join('\n') : tr('desktop_apps_log_empty')}</pre></aside>}
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
    <button type="button" disabled={opening} aria-busy={opening} aria-label={tr(opening ? 'desktop_opening_app_named' : 'desktop_open_app_named', app.name)} onClick={async () => {
      if (openingRef.current) return;
      openingRef.current = true;
      setOpening(true); setError('');
      try { await appsApi().open(app.id); }
      catch (error) { setError(error instanceof Error ? error.message : String(error)); }
      finally { openingRef.current = false; setOpening(false); }
    }}>{opening ? tr('desktop_opening') : tr('desktop_open_app')}</button>
    {error && <p className="local-app-chat-error" role="alert">{error}</p>}
  </div>;
}
