import { useEffect, useState, type FormEvent, type ReactElement } from 'react';
import type { DesktopLocalApp, LocalAppDataBinding, LocalAppsApi } from '../../shared/localApps.js';

export function LocalAppDataSettings({ app, api }: { app: DesktopLocalApp; api: LocalAppsApi }): ReactElement {
  return <section aria-label="Data bindings">
    <h3>Data bindings</h3>
    {Object.entries(app.manifest.dataBindings ?? {}).map(([alias, access]) =>
      <BindingForm key={alias} app={app} api={api} alias={alias} access={access}/>)}
  </section>;
}

function BindingForm({ app, api, alias, access }: {
  app: DesktopLocalApp; api: LocalAppsApi; alias: string; access: 'read' | 'edit';
}): ReactElement {
  const [binding, setBinding] = useState<LocalAppDataBinding | null>(null);
  const [formVersion, setFormVersion] = useState(0);
  const [busy, setBusy] = useState(true);
  const [status, setStatus] = useState('');

  useEffect(() => {
    let current = true;
    void api.dataBinding(app.id, app.revision, alias).then(value => {
      if (!current) return;
      setBinding(value);
      setStatus('');
      setBusy(false);
    }, () => {
      if (current) { setStatus('Data settings are unavailable.'); setBusy(false); }
    });
    return () => { current = false; };
  }, [api, app.id, app.revision, alias]);

  async function apply(value: LocalAppDataBinding | null): Promise<void> {
    setBusy(true);
    try {
      await api.setDataBinding(app.id, app.revision, alias, value);
      setBinding(value);
      setFormVersion(index => index + 1);
      setStatus(value ? 'Data binding saved.' : 'Data binding removed.');
    } catch {
      setStatus('Could not save this binding. Check that you can open this Dataset with the requested access.');
    } finally { setBusy(false); }
  }

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const dataset = String(data.get('dataset')).trim();
    const via = String(data.get('grant')).trim();
    const snapshot = String(data.get('snapshot') ?? '').trim();
    void apply({ reference: snapshot ? { kind: 'snapshot', dataset, snapshot } : { kind: 'dataset', dataset },
      ...(via ? { via } : {}) });
  }

  return <form key={formVersion + ':' + (binding?.reference.dataset ?? '')} onSubmit={submit}>
    <fieldset disabled={busy}>
      <legend>{alias} ({access === 'edit' ? 'Read and write' : 'Read'})</legend>
      <label>Dataset ID <input name="dataset" required maxLength={64} defaultValue={binding?.reference.dataset} /></label>
      <label>Share grant <input name="grant" maxLength={64} defaultValue={binding?.via} /></label>
      {access === 'read' && <label>Fixed snapshot <input name="snapshot" maxLength={64}
        defaultValue={binding?.reference.kind === 'snapshot' ? binding.reference.snapshot : undefined} /></label>}
      <button type="submit">Bind dataset</button>
      {binding && <button type="button" onClick={() => void apply(null)}>Remove binding</button>}
    </fieldset>
    {status && <p role="status">{status}</p>}
  </form>;
}
