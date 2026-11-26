import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TeamsApi } from './index.js';
import { type SurfaceRequest, type SurfaceValue, VisualizationSurface } from './VisualizationSurface.js';
import { VisualizationComposer } from './VisualizationComposer.js';
import { openVisualization } from './visualization-transfer.js';
import { type TeamVisualization, type VisualizationBundle, type VisualizationScope, observeVisualization, visualizationAccessLost, visualizationLabels, visualizationRequest } from './visualization.js';

interface PageData {
  data: { records?: { reference: { revision: number; schemaVersion: number }; value: Record<string, unknown>; partition: string; archived: boolean }[] };
  access: 'pinned' | 'live' | 'edit';
  schemaVersion: number;
  cursors?: string[] | null;
}
type Patch = { record: string; changes: Record<string, unknown> };
type Pending = { alias: string; patch: Patch; previous: Record<string, unknown>; changed: boolean; resolve(answer: boolean): void };
const code = (error: unknown) => error instanceof Error ? error.message : 'DATA_UNAVAILABLE';
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);

export function VisualizationMessage({ api, scope, visualization, online, locale, forwardTargets, participantName }: {
  api: TeamsApi; scope: VisualizationScope; visualization: TeamVisualization; online: boolean; locale: string;
  forwardTargets?: { id: string; title: string }[];
  participantName?(uid: string): string;
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
  const viewer = useRef<symbol>();
  const proposal = useRef<Pending>();
  const watches = useRef(new Map<string, { subscription: number; snapshot?: PageData; next: number }>());
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
    const mounted = Symbol();
    viewer.current = mounted;
    setError('');
    const transfer = new AbortController();
    void openVisualization(call, transfer.signal).then(value => {
      if (viewer.current === mounted) setOpen(value);
    }, failure => { if (viewer.current === mounted) setError(code(failure)); });
    return () => {
      transfer.abort();
      viewer.current = undefined;
      watches.current.clear();
      proposal.current?.resolve(false);
      proposal.current = undefined;
      setPending(undefined);
      setOpen(undefined);
    };
  }, [attempt, visible, online, call]);

  const closeCard = useCallback((error: string) => {
    viewer.current = undefined;
    proposal.current?.resolve(false);
    proposal.current = undefined;
    setPending(undefined);
    watches.current.clear();
    setOpen(undefined);
    setError(error);
  }, []);
  useEffect(() => {
    if (!open) return;
    return observeVisualization(api, { teamId, conversationId, generation }, { message, version, close: closeCard });
  }, [open, api, teamId, conversationId, generation, message, version, closeCard]);

  useEffect(() => {
    if (!open) return;
    let alive = true, polling = false;
    async function tick() {
      if (polling || document.hidden) return;
      polling = true;
      try {
        const due = [...watches.current].filter(([, watch]) => watch.next <= performance.now());
        for (const [alias, watch] of due) {
          if (!alive) return;
          const previous = watch.snapshot;
          if (!previous || watches.current.get(alias) !== watch) continue;
          const cursors = previous.cursors;
          if (cursors && watch.next !== 0) {
            try {
              const result = await data<{ cursors: string[]; changed: boolean; schemaVersion: number }>(alias, { action: 'changes', cursors });
              if (!alive) return;
              if (watches.current.get(alias) !== watch) continue;
              if (!result.changed && result.schemaVersion === previous.schemaVersion) {
                previous.cursors = result.cursors;
                watch.next = performance.now() + 12000;
                continue;
              }
            } catch (failure) {
              if (watches.current.get(alias) !== watch) continue;
              if (!['DATA_BACKFILL_REQUIRED', 'INVALID_DATA_REQUEST'].includes(code(failure))) throw failure;
            }
          }
          const fresh = await data<PageData>(alias, { action: 'read', watch: true }).catch(failure => {
            if (watches.current.get(alias) === watch) throw failure;
          });
          if (!fresh) continue;
          if (!alive) return;
          if (watches.current.get(alias) !== watch) continue;
          if (JSON.stringify(fresh.data) !== JSON.stringify(previous.data) || fresh.access !== previous.access) {
            channel.current?.({ type: 'data', value: { alias, subscription: watch.subscription, value: fresh.data } });
          }
          watch.snapshot = fresh;
          watch.next = performance.now() + (fresh.cursors ? 3000 : 30000);
        }
      } catch (failure) {
        if (alive && visualizationAccessLost(failure)) closeCard(code(failure));
      } finally { polling = false; }
    }
    let timer: number;
    async function schedule() {
      await tick();
      if (alive) timer = window.setTimeout(() => void schedule(), 3000 + Math.random() * 500);
    }
    timer = window.setTimeout(() => void schedule(), 3000 + Math.random() * 500);
    const refresh = () => {
      if (document.hidden) return;
      for (const watch of watches.current.values()) watch.next = Math.min(watch.next, performance.now());
      void tick();
    };
    document.addEventListener('visibilitychange', refresh);
    return () => { alive = false; window.clearTimeout(timer); document.removeEventListener('visibilitychange', refresh); };
  }, [open, data, closeCard]);

  useEffect(() => {
    if (pending && !dialog.current?.open) dialog.current?.showModal();
    else if (!pending && dialog.current?.open) dialog.current.close();
  }, [pending]);

  function complete(confirmed: boolean) {
    proposal.current?.resolve(confirmed);
    proposal.current = undefined;
    setPending(undefined);
  }

  async function patch(alias: string, raw: unknown, mounted: symbol): Promise<unknown> {
    if (visualization.bindings.find(binding => binding.alias === alias)?.mode !== 'edit' || proposal.current
        || !object(raw) || typeof raw.record !== 'string' || !object(raw.changes) || !Object.keys(raw.changes).length) {
      throw new Error('DATA_ACCESS_REQUIRED');
    }
    const patch = { record: raw.record, changes: raw.changes };
    for (let attempt = 0; attempt < 2; attempt++) {
        const current = await data<PageData>(alias, { action: 'read', record: patch.record });
        const row = current.data.records?.[0];
      if (viewer.current !== mounted || proposal.current || !row || row.archived || current.access !== 'edit') throw new Error('DATA_ACCESS_REQUIRED');
        const accepted = await new Promise<boolean>(resolve => {
          const value = { alias, patch, previous: row.value, changed: attempt > 0, resolve };
          proposal.current = value;
          setPending(value);
        });
        if (!accepted || viewer.current !== mounted) throw new Error('DATA_CANCELLED');
        try {
          const result = await data(alias, { action: 'patch', ...patch, partition: row.partition,
            expectedRevision: row.reference.revision, schemaVersion: current.schemaVersion }, crypto.randomUUID());
          const watch = watches.current.get(alias);
          if (watch) watch.next = 0;
          return result;
        } catch (failure) {
          if (code(failure) !== 'DATA_CONFLICT' || attempt === 1) throw failure;
        }
    }
    throw new Error('DATA_CONFLICT');
  }

  async function request(message: SurfaceRequest) {
    const mounted = viewer.current;
    if (!mounted) throw new Error('VISUALIZATION_UNAVAILABLE');
    if (message.action === 'expand') { setExpanded(true); return; }
    if (message.action === 'state' && Number.isSafeInteger(message.expected) && object(message.state)) {
      return call({ action: 'state', expected: message.expected, state: message.state });
    }
    const alias = message.alias;
    if (typeof alias !== 'string' || !aliases.includes(alias)) throw new Error('DATA_ACCESS_REQUIRED');
    if (message.action === 'patch') return patch(alias, message.changes, mounted);
    if (message.action === 'read') {
      const result = await data<PageData>(alias, { action: 'read', after: message.after, block: message.block });
      return result.data;
    }
    const subscription = message.subscription;
    if ((message.action === 'watch' || message.action === 'unwatch') && typeof subscription === 'number' && Number.isSafeInteger(subscription)) {
      const existing = watches.current.get(alias);
      if (message.action === 'unwatch') {
        if (existing?.subscription === subscription) watches.current.delete(alias);
        return;
      }
      if (existing?.subscription === subscription && existing.snapshot) return existing.snapshot.data;
      const watch: { subscription: number; snapshot?: PageData; next: number } = { subscription, next: Infinity };
      watches.current.set(alias, watch);
      try {
        const result = await data<PageData>(alias, { action: 'read', watch: true });
        if (viewer.current === mounted && watches.current.get(alias) === watch) {
          watch.snapshot = result;
          watch.next = performance.now() + (result.cursors ? 3000 : 30000);
        }
        return result.data;
      } catch (failure) {
        if (watches.current.get(alias) === watch) watches.current.delete(alias);
        throw failure;
      }
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
      initialState={open.state} channel={channel} request={request} onFailure={closeCard} />}
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
