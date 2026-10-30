import { useEffect, useRef, useState } from 'react';
import { desktopHost } from '../bridge/bridgeConnection';
import type { DesktopLocalApp, LocalAppMarketPublication } from '../../shared/localApps';
function api() { const value = desktopHost()?.localApps; if (!value) throw new Error('Application host is unavailable'); return value; }
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

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
