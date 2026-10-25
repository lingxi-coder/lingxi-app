import { useI18n } from '../i18n/useI18n';
import { tr } from '../i18n/locale';
import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { DesktopLocalApp, LocalAppLog, LocalAppPhase } from '../../shared/localApps';
import type { ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { useT } from '../theme/ThemeContext';

function appsApi() { const api = window.lingxi?.localApps; if (!api) throw new Error('Desktop application host is unavailable'); return api; }
const phaseLabels: Record<LocalAppPhase, string> = { stopped: 'desktop_apps_ready', installing: 'desktop_apps_installing', building: 'desktop_apps_building', starting: 'desktop_apps_starting', running: 'desktop_apps_running', stopping: 'desktop_apps_stopping', failed: 'desktop_apps_attention' };
export function AppsGlyph({ size = 20 }: { size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65" aria-hidden="true"><rect x="3" y="3" width="7" height="7" rx="1.8"/><rect x="14" y="3" width="7" height="7" rx="1.8"/><rect x="3" y="14" width="7" height="7" rx="1.8"/><rect x="14" y="14" width="7" height="7" rx="1.8"/></svg>;
}
export function LocalApps({ visible, onCreate }: { visible: boolean; onCreate: () => void }) {
  const t = useT();
  const { locale } = useI18n();
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
  const filtered = apps.filter(app => `${app.name} ${app.summary}`.toLocaleLowerCase(locale).includes(query.toLocaleLowerCase(locale)));
  return <section className="local-app-library" aria-label={tr('apps_title')} style={{ '--apps-background': t.stageBg, '--apps-text': t.text, '--apps-muted': t.text3, '--apps-surface': t.surface, '--apps-border': t.border, '--apps-accent': t.accent, '--apps-error': t.danger } as CSSProperties}>
    <header className="local-app-library-header"><div><h1>{tr('apps_title')}</h1><p>{tr('desktop_apps_tagline')}</p></div><button onClick={onCreate}>{tr('desktop_apps_create_chat')}</button></header>
    {error && <div className="local-app-error" role="alert">{error}<button onClick={() => { setError(''); void refresh(); }}>{tr('common_retry')}</button></div>}
    <label className="local-app-search"><span className="sr-only">{tr('apps_search_hint')}</span><input type="search" placeholder={tr('desktop_apps_search')} value={query} onChange={event => setQuery(event.target.value)} /></label>
    {!apps.length && !error ? <div className="local-app-empty"><AppsGlyph size={38}/><h2>{tr('desktop_apps_empty_title')}</h2><p>{tr('desktop_apps_empty_body')}</p><button onClick={onCreate}>{tr('desktop_apps_start_creating')}</button></div> : <div className="local-app-grid">
      {filtered.map(app => <article className="local-app-tile" key={app.id}>
        <button className="local-app-tile-open" onClick={() => void act(app.id, () => appsApi().open(app.id))} disabled={busy === app.id}>
          <span className="local-app-avatar">{Array.from(app.name)[0]?.toLocaleUpperCase(locale)}</span><h2>{app.name}</h2><p>{app.summary || tr('desktop_apps_fallback_summary')}</p>
        </button>
        <div className="local-app-tile-footer"><span className="local-app-phase" data-phase={app.phase}>{tr(phaseLabels[app.phase])}</span><span>v{app.revision}</span></div>
        {app.runningRevision && app.runningRevision !== app.revision && <p className="local-app-update">{tr('desktop_apps_update', app.revision)}</p>}
        {app.error && <p className="local-app-error">{app.error}</p>}
        <div className="local-app-actions"><button onClick={() => { setSelected(selected === app.id ? null : app.id); }}>{tr('desktop_logs')}</button><button onClick={() => void act(app.id, () => appsApi().edit(app.id))}>{tr('desktop_modify')}</button>
          <details><summary aria-label={tr('desktop_manage_app', app.name)}>•••</summary><div className="local-app-menu">
            <button onClick={() => void act(app.id, () => appsApi().restart(app.id))}>{tr('desktop_restart')}</button>
            <button onClick={() => void act(app.id, () => appsApi().stop(app.id))}>{tr('desktop_stop')}</button>
            <button onClick={() => void act(app.id, () => appsApi().repair(app.id))}>{tr('desktop_apps_repair')}</button>
            <label><input type="checkbox" checked={app.background} onChange={event => void act(app.id, () => appsApi().setBackground(app.id, event.target.checked))}/> {tr('desktop_apps_background')}</label>
            <button onClick={() => void act(app.id, () => appsApi().revoke(app.id))}>{tr('desktop_apps_revoke')}</button>
            <button className="local-app-delete" onClick={() => void act(app.id, () => appsApi().remove(app.id))}>{tr('desktop_apps_delete')}</button>
          </div></details>
        </div>
      </article>)}
    </div>}
    {apps.length > 0 && !filtered.length && <p className="local-app-no-results">{tr('desktop_apps_no_results', query)}</p>}
    {selected && <aside className="local-app-logs" aria-label={tr('desktop_apps_logs')}><header><strong>{apps.find(a => a.id === selected)?.name ?? tr('desktop_app')} · {tr('desktop_logs')}</strong><button onClick={() => setSelected(null)} aria-label={tr('desktop_close_logs')}>×</button></header><pre>{logs.length ? logs.map(l => `[${new Date(l.time).toLocaleTimeString(locale)}] ${l.process}: ${l.text}`).join('\n') : tr('desktop_apps_log_empty')}</pre></aside>}
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
