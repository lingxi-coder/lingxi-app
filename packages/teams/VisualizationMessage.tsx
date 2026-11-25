import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TeamsApi } from './index.js';
import { type SurfaceRequest, type SurfaceValue, VisualizationSurface } from './VisualizationSurface.js';
import { VisualizationComposer } from './VisualizationComposer.js';
import { type TeamVisualization, type VisualizationBundle, type VisualizationScope, visualizationLabels, visualizationRequest } from './visualization.js';

interface PageData {
  data: { records?: { reference: { revision: number; schemaVersion: number }; value: Record<string, unknown>; partition: string; archived: boolean }[] };
  access: 'pinned' | 'live' | 'edit';
  schemaVersion: number;
  cursors?: string[] | null;
}
type Patch = { record: string; changes: Record<string, unknown> };
type Pending = { alias: string; patch: Patch; previous: Record<string, unknown>; changed: boolean; resolve(answer: boolean): void };
const authorizationErrors = new Set(['DATA_ACCESS_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'DATA_NOT_FOUND']);
const code = (error: unknown) => error instanceof Error ? error.message : 'DATA_UNAVAILABLE';
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);

export function VisualizationMessage({ api, scope, visualization, online, locale, forwardTargets, participantName, onDraft }: {
  api: TeamsApi; scope: VisualizationScope; visualization: TeamVisualization; online: boolean; locale: string;
  forwardTargets?: { id: string; title: string }[];
  participantName?(uid: string): string;
  onDraft?(text: string): void;
}) {
  const labels = visualizationLabels(locale);
  const { teamId, conversationId, generation } = scope;
  const { page: message, version } = visualization.reference;
  const [open, setOpen] = useState<{ bundle: VisualizationBundle; state: unknown }>();
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const [forward, setForward] = useState(false);
  const [pending, setPending] = useState<Pending>();
  const root = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const channel = useRef<(message: SurfaceValue) => void>();
  const active = useRef(false);
  const proposal = useRef<Pending>();
  const watches = useRef(new Map<string, { snapshot: PageData; refreshSoon?: boolean }>());
  const aliases = useMemo(() => visualization.bindings.map(binding => binding.alias), [visualization.bindings]);
  const call = useCallback(<T,>(body: Record<string, unknown>) => visualizationRequest<T>(api, { teamId, conversationId, generation },
    { ...body, message, version }), [api, teamId, conversationId, generation, message, version]);
  const data = useCallback(<T,>(alias: string, request: Record<string, unknown>, operation?: string) => call<T>({
    action: 'data', alias, request, ...(operation ? { operation } : {}),
  }), [call]);

  useEffect(() => {
    const element = root.current;
    if (!element || typeof IntersectionObserver === 'undefined') { setVisible(true); return; }
    const observer = new IntersectionObserver(entries => setVisible(entries.some(entry => entry.isIntersecting)),
      { rootMargin: '250px 0px' });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible || !online) { setOpen(undefined); return; }
    let live = true;
    active.current = true;
    setError('');
    void call<{ bundle: VisualizationBundle; state: unknown }>({ action: 'open' }).then(value => { if (live) setOpen(value); },
      failure => { if (live) setError(code(failure)); });
    return () => {
      live = false;
      active.current = false;
      watches.current.clear();
      proposal.current?.resolve(false);
      proposal.current = undefined;
      setPending(undefined);
      setOpen(undefined);
    };
  }, [attempt, visible, online, call]);

  useEffect(() => {
    if (!open) return;
    let alive = true, polling = false;
    async function tick() {
      if (polling || document.hidden) return;
      polling = true;
      try {
        await call({ action: 'check' });
        for (const [alias, watch] of watches.current) {
          if (!alive) return;
          const cursors = watch.snapshot.cursors;
          if (cursors && !watch.refreshSoon) {
            try {
              const result = await data<{ cursors: string[]; changed: boolean; schemaVersion: number }>(alias, { action: 'changes', cursors });
              if (!result.changed && result.schemaVersion === watch.snapshot.schemaVersion) {
                watch.snapshot.cursors = result.cursors;
                continue;
              }
            } catch (failure) {
              if (!['DATA_BACKFILL_REQUIRED', 'INVALID_DATA_REQUEST'].includes(code(failure))) throw failure;
            }
          }
          const fresh = await data<PageData>(alias, { action: 'read', watch: true });
          if (!alive) return;
          watch.refreshSoon = false;
          if (JSON.stringify(fresh.data) !== JSON.stringify(watch.snapshot.data) || fresh.access !== watch.snapshot.access) {
            channel.current?.({ type: 'data', value: { alias, value: fresh.data } });
          }
          watch.snapshot = fresh;
        }
      } catch (failure) {
        if (alive && authorizationErrors.has(code(failure))) {
          active.current = false;
          proposal.current?.resolve(false);
          watches.current.clear();
          setOpen(undefined);
          setError(code(failure));
        }
      } finally { polling = false; }
    }
    const interval = window.setInterval(() => void tick(), 3000);
    const refresh = () => { if (!document.hidden) void tick(); };
    document.addEventListener('visibilitychange', refresh);
    return () => { alive = false; window.clearInterval(interval); document.removeEventListener('visibilitychange', refresh); };
  }, [open, data, call]);

  useEffect(() => {
    if (pending && !dialog.current?.open) dialog.current?.showModal();
    else if (!pending && dialog.current?.open) dialog.current.close();
  }, [pending]);

  function complete(confirmed: boolean) {
    proposal.current?.resolve(confirmed);
    proposal.current = undefined;
    setPending(undefined);
  }

  async function patch(alias: string, raw: unknown): Promise<unknown> {
    if (visualization.bindings.find(binding => binding.alias === alias)?.mode !== 'edit' || proposal.current
        || !object(raw) || typeof raw.record !== 'string' || !object(raw.changes) || !Object.keys(raw.changes).length) {
      throw new Error('DATA_ACCESS_REQUIRED');
    }
    const patch = { record: raw.record, changes: raw.changes };
    for (let attempt = 0; attempt < 2; attempt++) {
        const current = await data<PageData>(alias, { action: 'read', record: patch.record });
        const row = current.data.records?.[0];
        if (!active.current || proposal.current || !row || row.archived || current.access !== 'edit') throw new Error('DATA_ACCESS_REQUIRED');
        const accepted = await new Promise<boolean>(resolve => {
          const value = { alias, patch, previous: row.value, changed: attempt > 0, resolve };
          proposal.current = value;
          setPending(value);
        });
        if (!accepted || !active.current) throw new Error('DATA_CANCELLED');
        try {
          const result = await data(alias, { action: 'patch', ...patch, partition: row.partition,
            expectedRevision: row.reference.revision, schemaVersion: current.schemaVersion }, crypto.randomUUID());
          const watch = watches.current.get(alias);
          if (watch) watch.refreshSoon = true;
          return result;
        } catch (failure) {
          if (code(failure) !== 'DATA_CONFLICT' || attempt === 1) throw failure;
        }
    }
    throw new Error('DATA_CONFLICT');
  }

  async function request(message: SurfaceRequest) {
    if (!active.current) throw new Error('VISUALIZATION_UNAVAILABLE');
    if (message.action === 'expand') { setExpanded(true); return; }
    if (message.action === 'draft' && typeof message.text === 'string') { onDraft?.(message.text.slice(0, 2000)); return; }
    if (message.action === 'state' && Number.isSafeInteger(message.expected) && object(message.state)) {
      return call({ action: 'state', expected: message.expected, state: message.state });
    }
    const alias = message.alias;
    if (typeof alias !== 'string' || !aliases.includes(alias)) throw new Error('DATA_ACCESS_REQUIRED');
    if (message.action === 'patch') return patch(alias, message.changes);
    if (message.action === 'read') {
      const result = await data<PageData>(alias, { action: 'read', after: message.after, block: message.block });
      return result.data;
    }
    if (message.action === 'watch') {
      const existing = watches.current.get(alias);
      if (existing) return existing.snapshot.data;
      const result = await data<PageData>(alias, { action: 'read', watch: true });
      if (active.current) watches.current.set(alias, { snapshot: result });
      return result.data;
    }
    throw new Error('INVALID_DATA_REQUEST');
  }

  return <div ref={root} className="tw-visualization">
    <div className="tw-visualization-title">
      <strong>{visualization.title}</strong>
      {open && !!forwardTargets?.length && <button type="button" className="tw-link" onClick={() => setForward(true)}>{labels.forward}</button>}
      {open && <button type="button" className="tw-link" onClick={() => setExpanded(value => !value)}>
        {expanded ? labels.collapse : labels.expand}
      </button>}
    </div>
    {error && <p role="status">{labels.unavailable} <button type="button" className="tw-link" onClick={() => setAttempt(value => value + 1)}>{labels.retry}</button></p>}
    {open && !error && <VisualizationSurface bundle={open.bundle} aliases={aliases} locale={locale} expanded={expanded}
      initialState={open.state} channel={channel} request={request} onFailure={setError} />}
    {open && forward && forwardTargets && <VisualizationComposer api={api} scope={scope} locale={locale}
      source={open.bundle} participantName={participantName ?? (uid => uid)}
      forward={{ targets: forwardTargets, close: () => setForward(false) }} />}
    <dialog ref={dialog} className="tw-dialog" aria-label={labels.confirm} onCancel={event => { event.preventDefault(); complete(false); }}>
      {pending && <div className="tw-dialog-panel">
        <h2>{labels.confirm}</h2>
        {pending.changed && <p role="status">{labels.conflict}</p>}
        <p>{pending.alias} · {pending.patch.record}</p>
        <table><thead><tr><th>{labels.field}</th><th>{labels.current}</th><th>{labels.next}</th></tr></thead>
          <tbody>{Object.entries(pending.patch.changes).map(([field, value]) => <tr key={field}>
            <th scope="row">{field}</th><td>{JSON.stringify(pending.previous[field])}</td><td>{JSON.stringify(value)}</td>
          </tr>)}</tbody></table>
        <button type="button" className="tw-btn" onClick={() => complete(false)}>{labels.cancel}</button>
        <button type="button" className="tw-primary" onClick={() => complete(true)}>{labels.change}</button>
      </div>}
    </dialog>
  </div>;
}
