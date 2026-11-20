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
  const [team, setTeam] = useState(false);
  const [formVersion, setFormVersion] = useState(0);
  const [busy, setBusy] = useState(true);
  const [status, setStatus] = useState('');

  useEffect(() => {
    let current = true;
    void api.dataBinding(app.id, app.revision, alias).then(value => {
      if (!current) return;
      setBinding(value);
      setTeam(value?.scope.kind === 'team');
      setStatus('');
      setBusy(false);
    }, () => {
      if (current) setStatus('Data settings are unavailable.');
    });
    return () => { current = false; };
  }, [api, app.id, app.revision, alias]);

  async function apply(value: LocalAppDataBinding | null): Promise<void> {
    setBusy(true);
    try {
      await api.setDataBinding(app.id, app.revision, alias, value);
      setBinding(value);
      setTeam(value?.scope.kind === 'team');
      setFormVersion(index => index + 1);
      setStatus(value ? 'Data binding saved.' : 'Data binding removed.');
    } catch {
      setStatus('Could not save this binding. Check that you can open the dataset in the selected scope.');
    } finally { setBusy(false); }
  }

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const scope = team
      ? { kind: 'team' as const, teamId: String(data.get('team')), conversationId: String(data.get('conversation')), generation: Number(data.get('generation')) }
      : { kind: 'personal' as const };
    void apply({ dataset: String(data.get('dataset')), scope });
  }

  const scope = binding?.scope.kind === 'team' ? binding.scope : undefined;
  return <form key={formVersion + ':' + (binding?.dataset ?? '')} onSubmit={submit}>
    <fieldset disabled={busy}>
      <legend>{alias} ({access === 'edit' ? 'Read and write' : 'Read'})</legend>
      <label>Dataset ID <input name="dataset" required maxLength={64} defaultValue={binding?.dataset}/></label>
      <label>Scope <select value={team ? 'team' : 'personal'} onChange={event => setTeam(event.target.value === 'team')}>
        <option value="personal">Personal</option><option value="team">Team</option>
      </select></label>
      {team && <>
        <label>Team ID <input name="team" required maxLength={64} defaultValue={scope?.teamId}/></label>
        <label>Conversation ID <input name="conversation" required maxLength={64} defaultValue={scope?.conversationId}/></label>
        <label>Membership generation <input name="generation" required type="number" min={1} step={1} defaultValue={scope?.generation}/></label>
      </>}
      <button type="submit">Bind dataset</button>
      {binding && <button type="button" onClick={() => void apply(null)}>Remove binding</button>}
    </fieldset>
    {status && <p role="status">{status}</p>}
  </form>;
}
