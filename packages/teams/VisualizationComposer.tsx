import { useEffect, useId, useMemo, useRef, useState, type FormEvent } from 'react';
import { DATA_PROTOCOL_VERSION } from './dataset-resource.js';
import type { TeamsApi } from './index.js';
import { VisualizationSurface } from './VisualizationSurface.js';
import { type VisualizationAudience, type VisualizationBundle, type VisualizationDataRef, type VisualizationMode,
  type VisualizationScope, type VisualizationSource, visualizationLabels, visualizationRequest } from './visualization.js';

type Source = { reference: VisualizationDataRef; via?: string | null; access: VisualizationMode; canShare: boolean };
type Location = 'mine' | 'team' | 'shared';
const bytes = 2 * 1024 * 1024;
const safeAlias = /^[A-Za-z0-9_-]{1,64}$/u;
const list = (input: string) => [...new Set(input.split(',').map(value => value.trim()).filter(Boolean))];
const errorCode = (error: unknown) => error instanceof Error ? error.message : 'VISUALIZATION_UNAVAILABLE';

export function VisualizationComposer({ api, scope, locale, source, participantName, onPublished, forward }: {
  api: TeamsApi; scope: VisualizationScope; locale: string; source?: VisualizationBundle;
  participantName(uid: string): string; onPublished?(): void;
  forward?: { targets: { id: string; title: string }[]; close(): void };
}) {
  const labels = visualizationLabels(locale), heading = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const [audience, setAudience] = useState<VisualizationAudience>();
  const [name, setName] = useState(source?.title ?? '');
  const [html, setHtml] = useState(source?.html ?? '');
  const [preview, setPreview] = useState<VisualizationBundle>();
  const [error, setError] = useState('');
  const [sources, setSources] = useState<Source[]>([]);
  const [location, setLocation] = useState<Location>('mine');
  const [selected, setSelected] = useState<Source>();
  const [alias, setAlias] = useState('');
  const [mode, setMode] = useState<VisualizationMode>('pinned');
  const [fields, setFields] = useState('');
  const [partitions, setPartitions] = useState('');
  const [bindings, setBindings] = useState<VisualizationSource[]>([]);
  const [busy, setBusy] = useState(false);
  const [building, setBuilding] = useState(false);
  const sequence = useRef(0);
  const intent = useRef<{ digest: string; id: string }>();
  const { teamId, generation } = scope;
  const [conversationId, setConversationId] = useState(scope.conversationId);
  const previewAliases = useMemo(() => bindings.map(item => item.alias), [bindings]);
  const forwarding = forward !== undefined;

  useEffect(() => {
    let alive = true;
    setAudience(undefined);
    void visualizationRequest<VisualizationAudience>(api, { teamId, conversationId, generation }, { action: 'audience' })
      .then(value => { if (alive) setAudience(value); }, failure => { if (alive && forwarding) setError(errorCode(failure)); });
    return () => { alive = false; };
  }, [api, teamId, conversationId, generation, forwarding]);
  useEffect(() => { if (forwarding && !dialog.current?.open) dialog.current?.showModal(); }, [forwarding]);

  async function discover(next: Location, targetId = conversationId) {
    const current = ++sequence.current;
    setLocation(next); setSelected(undefined); setSources([]); setError('');
    if (!api.datasets) return;
    const destination = { team: teamId, conversation: targetId };
    const query = next === 'shared' ? { kind: 'conversation', destination } : { kind: 'mine', ...(next === 'team' ? { team: teamId } : {}) };
    try {
      const result = await api.datasets<{ resources: Source[] }>({ protocol: DATA_PROTOCOL_VERSION, dataset: '',
        command: { action: 'discover', scope: query, limit: 50 } });
      if (current === sequence.current) setSources(result.resources.filter(item => next === 'shared' ? !!item.via : item.canShare));
    } catch (failure) { if (current === sequence.current) setError(errorCode(failure)); }
  }

  function select(index: number) {
    const item = sources[index];
    setSelected(item);
    setAlias(item?.reference.dataset ?? '');
    setMode(item?.via ? item.access : 'pinned');
    setFields(''); setPartitions('');
  }
  function addBinding() {
    if (!selected || bindings.length >= 4 || !safeAlias.test(alias) || bindings.some(item => item.alias === alias)) return;
    const { reference, via } = selected;
    const item: VisualizationSource = location === 'shared' && via
      ? { kind: 'reuse', alias, reference, grant: via, mode }
      : { kind: 'share', alias, dataset: reference.dataset, ...(via ? { via } : {}), mode,
        ...(reference.kind === 'dataset' ? {} : { reference }), projection: {
          ...(fields.trim() ? { fields: list(fields) } : {}), ...(partitions.trim() ? { partitions: list(partitions) } : {}),
        } };
    setBindings(previous => [...previous, item]);
    setSelected(undefined); setAlias('');
  }

  function currentBundle(): VisualizationBundle {
    return { title: name.trim(), html, assets: source?.assets ?? {} };
  }
  async function importFile(file?: File) {
    if (!file) return;
    if (file.size > bytes) { setError('VISUALIZATION_TOO_LARGE'); return; }
    try {
      setHtml(await file.text());
      if (!name) setName(file.name.replace(/\.html?$/iu, '').slice(0, 80));
      setPreview(undefined);
    } catch (failure) { setError(errorCode(failure)); }
  }
  async function publish(event: FormEvent) {
    event.preventDefault();
    if (!audience || busy) return;
    const bundle = currentBundle();
    if (new TextEncoder().encode(JSON.stringify(bundle)).byteLength > bytes) { setError('VISUALIZATION_TOO_LARGE'); return; }
    const body = { action: 'publish', bundle, bindings, audienceDigest: audience.audienceDigest };
    const digest = JSON.stringify(body);
    if (intent.current?.digest !== digest) intent.current = { digest, id: crypto.randomUUID() };
    setBusy(true); setError('');
    try {
      const result = await visualizationRequest<{ status: 'published' | 'building' }>(api, { teamId, conversationId, generation },
        { ...body, operation: intent.current.id });
      if (result.status === 'building') { setBuilding(true); return; }
      intent.current = undefined;
      dialog.current?.close();
      setBuilding(false); setPreview(undefined); setBindings([]); setSelected(undefined);
      if (!source) { setName(''); setHtml(''); }
      onPublished?.();
    } catch (failure) {
      setError(errorCode(failure));
      if (errorCode(failure) === 'DATA_SHARE_CHANGED') {
        const updated = await visualizationRequest<VisualizationAudience>(api, { teamId, conversationId, generation }, { action: 'audience' }).catch(() => undefined);
        if (updated) setAudience(updated);
      }
    } finally { setBusy(false); }
  }

  if (!audience && !forward) return null;
  return <>
    {!forward && <button type="button" className="tw-link" onClick={() => {
      dialog.current?.showModal(); void discover('mine');
    }}>{labels.add}</button>}
    <dialog className="tw-dialog tw-data-record" ref={dialog} aria-labelledby={heading}
      onCancel={event => { if (busy) event.preventDefault(); }}
      onClose={() => { setPreview(undefined); ++sequence.current; forward?.close(); }}>
      <div className="tw-dialog-panel">
        <h2 id={heading}>{forward ? labels.forward : labels.add}</h2>
        <form onSubmit={event => void publish(event)}>
          {forward && <label>{labels.destination}<select value={conversationId} onChange={event => {
            const target = event.target.value;
            setConversationId(target); setAudience(undefined); setBindings([]); setPreview(undefined); void discover('mine', target);
          }}>{forward.targets.map(target => <option key={target.id} value={target.id}>{target.title}</option>)}</select></label>}
          <label>{labels.title}<input required maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
          <label>{labels.import}<input type="file" accept=".html,.htm,text/html" onChange={event => void importFile(event.target.files?.[0])} /></label>
          <label>{labels.source}<textarea required rows={8} value={html} onChange={event => { setHtml(event.target.value); setPreview(undefined); }} /></label>
          <button type="button" className="tw-btn" disabled={!name.trim() || !html.trim()} onClick={() => setPreview(currentBundle())}>{labels.preview}</button>
          {preview && <VisualizationSurface bundle={preview} aliases={previewAliases} locale={locale}
            request={async request => {
              if (request.action === 'state') return { ...(request.state as object), version: Number(request.expected) + 1 };
              if (request.action === 'read' || request.action === 'watch') return { records: [] };
              throw new Error('DATA_ACCESS_REQUIRED');
            }}
            onFailure={setError} />}
          <fieldset>
            <legend>{labels.choose}</legend>
            <select aria-label={labels.choose} value={location} onFocus={() => { if (!sources.length) void discover(location); }}
              onChange={event => void discover(event.target.value as Location)}>
              <option value="mine">{labels.mine}</option><option value="team">{labels.team}</option><option value="shared">{labels.shared}</option>
            </select>
            <select aria-label={labels.selected} value={selected ? sources.indexOf(selected) : ''} onChange={event => select(event.target.value ? Number(event.target.value) : -1)}>
              <option value="">{labels.none}</option>
              {sources.map((item, index) => <option key={index} value={index}>{item.reference.dataset} · {labels[item.access]}</option>)}
            </select>
            {selected && <>
              <label>{labels.alias}<input value={alias} maxLength={64} onChange={event => setAlias(event.target.value)} /></label>
              <select aria-label={labels.choose} value={mode} onChange={event => setMode(event.target.value as VisualizationMode)}>
                {location !== 'shared' || selected.access === 'pinned' ? <option value="pinned">{labels.pinned}</option> : null}
                {(location !== 'shared' && selected.reference.kind === 'dataset') || selected.access === 'live' || selected.access === 'edit'
                  ? <option value="live">{labels.live}</option> : null}
                {(location !== 'shared' && selected.reference.kind === 'dataset') || selected.access === 'edit'
                  ? <option value="edit">{labels.edit}</option> : null}
              </select>
              {location !== 'shared' && <>
                <label>{labels.fields}<input value={fields} onChange={event => setFields(event.target.value)} /></label>
                <label>{labels.partitions}<input value={partitions} onChange={event => setPartitions(event.target.value)} /></label>
              </>}
              <button type="button" className="tw-btn" disabled={!safeAlias.test(alias) || bindings.length >= 4 || bindings.some(item => item.alias === alias)}
                onClick={addBinding}>{labels.choose}</button>
            </>}
            {bindings.length > 0 && <ul>{bindings.map(item => <li key={item.alias}>{item.alias} · {labels[item.mode]} <button type="button"
              className="tw-link" onClick={() => setBindings(previous => previous.filter(binding => binding.alias !== item.alias))}>{labels.remove}</button></li>)}</ul>}
          </fieldset>
          {audience && <p>{labels.audience} {Object.keys(audience.audience).map(participantName).join(' · ')}</p>}
          {error && <p role="alert">{error}</p>}
          {building && <p role="status">{labels.building}</p>}
          <button className="tw-btn" type="button" disabled={busy} onClick={() => dialog.current?.close()}>{labels.close}</button>
          <button className="tw-primary" type="submit" disabled={!audience || busy || !name.trim() || !html.trim() || !!selected}>
            {building ? labels.retry : labels.send}
          </button>
        </form>
      </div>
    </dialog>
  </>;
}
