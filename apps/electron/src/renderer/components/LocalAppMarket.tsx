import { useEffect, useRef, useState } from 'react';
import { desktopHost } from '../bridge/bridgeConnection';
import type { DesktopLocalApp, LocalAppMarketPublication, LocalAppMarketRow } from '../../shared/localApps';
function api() { const value = desktopHost()?.localApps; if (!value) throw new Error('Application host is unavailable'); return value; }
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

export function LocalAppMarket({ visible, installed, onInstalled }: { visible: boolean; installed: DesktopLocalApp[]; onInstalled: () => void }) {
  const [rows, setRows] = useState<LocalAppMarketRow[]>([]), [query, setQuery] = useState(''), [owned, setOwned] = useState(false);
  const [error, setError] = useState(''), [loading, setLoading] = useState(false), [busy, setBusy] = useState<string>();
  const generation = useRef(0);
  useEffect(() => {
    if (!visible) return;
    const token = ++generation.current; setLoading(true); setError('');
    const timeout = setTimeout(() => { void api().marketList(query, owned).then(result => {
      if (generation.current === token) setRows(result);
    }).catch(error => { if (generation.current === token) setError(message(error)); }).finally(() => { if (generation.current === token) setLoading(false); }); }, 200);
    return () => { clearTimeout(timeout); generation.current++; };
  }, [visible, query, owned]);
  if (!visible) return null;
  const install = async (app: LocalAppMarketRow, current?: DesktopLocalApp) => {
    if (busy) return; setBusy(app.id); setError('');
    try { await api().marketInstall(app.id, current?.id, current?.revision); onInstalled(); }
    catch (error) { setError(message(error)); } finally { setBusy(undefined); }
  };
  return <div className="local-app-market">
    <div className="local-app-market-controls"><label className="local-app-search"><span className="sr-only">Search market</span><input type="search" placeholder="Search market" value={query} onChange={event => setQuery(event.target.value)} /></label>
      <label><input type="checkbox" checked={owned} onChange={event => setOwned(event.target.checked)}/> My publications</label>
      <button onClick={() => void api().marketManage().catch(error => setError(message(error)))}>Manage publications</button></div>
    <p className="local-app-update">Install a compatible source release. Programs ask for permission before setup or execution; your local apps remain available offline.</p>
    {error && <p role="alert" className="local-app-error">{error}</p>}
    {loading ? <p role="status">Loading market…</p> : <div className="local-app-grid">{rows.map(app => {
      const current = installed.find(item => item.marketOrigin?.appId === app.id && !item.marketOrigin.remixedFrom);
      return <article className="local-app-tile" key={app.id}><h2>{app.name}</h2><p>{app.summary}</p>{app.visibility && <p className="local-app-update">{app.visibility}</p>}
        <button disabled={Boolean(busy)} onClick={() => void install(app, current)}>{busy === app.id ? 'Preparing source…' : current ? 'Check for update' : 'Install'}</button>
        {current && <p className="local-app-update">Installed · v{current.revision}</p>}</article>;
    })}</div>}
    {!loading && !error && !rows.length && <p className="local-app-no-results">No matching applications.</p>}
  </div>;
}

export function LocalAppPublish({ app, onClose }: { app: DesktopLocalApp; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null), [busy, setBusy] = useState(false), [error, setError] = useState(''), [result, setResult] = useState('');
  const [fields, setFields] = useState<LocalAppMarketPublication>({ name: app.name, summary: app.summary, description: '', category: 'other', notes: '' });
  useEffect(() => { dialog.current?.showModal(); }, []);
  const set = (field: keyof LocalAppMarketPublication, value: string) => setFields(previous => ({ ...previous, [field]: value }));
  return <dialog ref={dialog} className="local-app-publish" aria-labelledby="local-app-publish-title" onCancel={event => { if (busy) event.preventDefault(); }} onClose={onClose}>
    <form onSubmit={async event => { event.preventDefault(); if (busy) return; setBusy(true); setError(''); try { const receipt = await api().marketPublish(app.id, fields, app.revision); if (receipt) setResult(`Source submitted: ${receipt.status || 'validation queued'}. Use Manage publications to review progress, visibility and review feedback.`); } catch (error) { setError(message(error)); } finally { setBusy(false); } }}>
      <h2 id="local-app-publish-title">Publish source to the market</h2><p>Only files explicitly listed in this release are exported. You will review the file list before upload. Publishing here does not change your local runtime permissions.</p>
      <label>Name<input required maxLength={60} value={fields.name} onChange={e => set('name', e.target.value)}/></label>
      <label>Summary<input required maxLength={160} value={fields.summary} onChange={e => set('summary', e.target.value)}/></label>
      <label>Description<textarea maxLength={6000} value={fields.description} onChange={e => set('description', e.target.value)}/></label>
      <label>Category<select value={fields.category} onChange={e => set('category', e.target.value)}>{['tools', 'productivity', 'games', 'education', 'visualization', 'other'].map(category => <option key={category}>{category}</option>)}</select></label>
      <label>Release notes<textarea maxLength={2000} value={fields.notes} onChange={e => set('notes', e.target.value)}/></label>
      {error && <p role="alert" className="local-app-error">{error}</p>}{result && <p role="status">{result}</p>}
      <footer><button type="button" disabled={busy} onClick={() => dialog.current?.close()}>Close</button>{result ? <button type="button" onClick={() => void api().marketManage()}>Manage publications</button> : <button disabled={busy}>{busy ? 'Uploading…' : 'Review source and publish'}</button>}</footer>
    </form>
  </dialog>;
}
