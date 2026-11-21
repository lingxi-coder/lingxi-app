import { useEffect, useId, useRef, useState, type FormEvent, type PointerEvent, type WheelEvent } from 'react';
import type { ResourceRef } from './dataset-resource.js';
import { messageErrorText } from './errors.js';
import { useTeamsText } from './i18n.js';
import type { TeamsApi } from './index.js';

type PageRef = Extract<ResourceRef, { kind: 'pageVersion' }>;
type Destination = { team: string; conversation: string };
type Point = { x: number; y: number };
type Frame = {
  session: string;
  png: string;
  accessibility: { id: string; role?: string; name: string; target?: number }[];
  selectedText?: string;
  approvals: ({ id: string; alias: string } & (
    | { mode: 'putRecord'; record: string; parameters: Record<string, unknown> }
    | { mode: 'startTask'; bot: string; prompt: string }
  ))[];
};

function diagnosis(error: unknown): string {
  return error instanceof Error ? error.message : 'DATA_UNAVAILABLE';
}

export function DatasetPagePublisher({ api, destination, generation, dataset, via, locale, initialPackage = '' }: {
  api: TeamsApi; destination: Destination; generation: number; dataset: string; via: string; locale: string; initialPackage?: string;
}) {
  const t = useTeamsText(locale);
  const dialog = useRef<HTMLDialogElement>(null), heading = useId();
  const [audience, setAudience] = useState<{ audience: Record<string, number>; audienceDigest: string }>();
  const [source, setSource] = useState(initialPackage);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const pending = useRef<{ page: string; operation: string; source: string }>();
  if (!api.datasets) return null;

  async function open(): Promise<void> {
    dialog.current?.showModal();
    setError(''); setAudience(undefined); setBusy(true);
    try {
      setAudience(await api.datasets!({ action: 'page', request: { action: 'prepare', destination, dataset, via } }));
    } catch (failure) { setError(diagnosis(failure)); }
    finally { setBusy(false); }
  }

  async function publish(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (!audience || busy) return;
    let bundle: unknown;
    try { bundle = JSON.parse(source); } catch { setError('INVALID_DATA_REQUEST'); return; }
    const current = pending.current?.source === source ? pending.current : { page: crypto.randomUUID(), operation: crypto.randomUUID(), source };
    pending.current = current;
    setBusy(true); setError('');
    try {
      await api.datasets!({ action: 'page', request: { action: 'publish', audienceDigest: audience.audienceDigest, destination, generation, dataset, via,
        page: current.page, operation: current.operation, bundle } });
      pending.current = undefined;
      setSource(initialPackage);
      dialog.current?.close();
    } catch (failure) { setError(diagnosis(failure)); }
    finally { setBusy(false); }
  }

  return <>
    <button type="button" className="tw-link" onClick={() => void open()}>{t('tw_dataset_publish_page')}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={heading} onCancel={event => { if (busy) event.preventDefault(); }}>
      <div className="tw-dialog-panel">
        <h3 id={heading}>{t('tw_dataset_publish_page')}</h3>
        {error && <p role="alert">{messageErrorText(error, locale)}</p>}
        {audience && <form onSubmit={event => void publish(event)}>
          <p>{t('tw_dataset_audience_fmt', Object.keys(audience.audience).join(' · '))}</p>
          <label>{t('tw_dataset_page_package')} <textarea rows={10} required value={source} disabled={busy}
            onChange={event => setSource(event.currentTarget.value)} /></label>
          <p>{t('tw_dataset_page_review')}</p>
          <button type="submit" className="tw-primary" disabled={busy}>{t('tw_dataset_publish_page')}</button>
        </form>}
        <form method="dialog"><button type="submit" className="tw-btn" disabled={busy}>{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}

export function DatasetPageCard({ api, reference, locale }: { api: Pick<TeamsApi, 'datasets'>; reference: PageRef; locale: string }) {
  const t = useTeamsText(locale);
  const dialog = useRef<HTMLDialogElement>(null), heading = useId();
  const [title, setTitle] = useState('');
  const [frame, setFrame] = useState<Frame>();
  const [selection, setSelection] = useState<string>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const dimensions = useRef({ width: 620, height: 480 });
  const active = useRef(false);
  const inFlight = useRef(false);
  const session = useRef<string>();
  const pointer = useRef<{ id: number; start: Point; clientX: number; clientY: number }>();
  const requestData = api.datasets;
  const { page, version } = reference;
  const openSession = frame?.session;

  useEffect(() => () => {
    active.current = false;
    const key = session.current;
    session.current = undefined;
    if (key && requestData) void requestData({ action: 'render', request: { action: 'close', page, version, session: key } })
      .catch(() => undefined);
  }, [page, requestData, version]);

  useEffect(() => {
    if (!requestData || !openSession) return;
    let cancelled = false;
    let checking = false;

    async function checkAccess(): Promise<void> {
      if (checking || inFlight.current || document.hidden || session.current !== openSession) return;
      checking = true;
      try {
        await requestData!({ action: 'page', request: { action: 'describe', page, version } });
      } catch (failure) {
        if (cancelled || session.current !== openSession) return;
        active.current = false;
        session.current = undefined;
        setFrame(undefined);
        setSelection(undefined);
        setError(diagnosis(failure));
        void requestData!({ action: 'render', request: { action: 'close', page, version, session: openSession } })
          .catch(() => undefined);
      } finally {
        checking = false;
      }
    }

    const timer = window.setInterval(() => void checkAccess(), 15_000);
    const onVisibility = () => void checkAccess();
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [openSession, page, requestData, version]);

  if (!requestData) return null;

  async function send(request: Record<string, unknown>): Promise<Frame | undefined> {
    if (!active.current || inFlight.current) return;
    inFlight.current = true;
    setBusy(true); setError('');
    try {
      const result = await api.datasets!<Frame>({ action: 'render', request: { page, version, session: session.current, ...request } });
      session.current = result.session;
      if (active.current) {
        const { selectedText, ...scene } = result;
        setFrame(scene);
        return result;
      }
      void closeSession();
    } catch (failure) {
      if (active.current) { setError(diagnosis(failure)); setFrame(undefined); setSelection(undefined); }
    } finally { inFlight.current = false; setBusy(false); }
  }

  async function closeSession(): Promise<void> {
    const key = session.current;
    session.current = undefined;
    if (key) await api.datasets!({ action: 'render', request: { action: 'close', page, version, session: key } }).catch(() => undefined);
  }

  async function open(): Promise<void> {
    if (inFlight.current) return;
    active.current = true;
    dimensions.current = { width: Math.max(176, Math.min(960, window.innerWidth - 48)), height: Math.max(160, Math.min(620, window.innerHeight - 180)) };
    if (!dialog.current?.open) dialog.current?.showModal();
    setTitle('');
    void api.datasets!<{ title: string }>({ action: 'page', request: { action: 'describe', page, version } })
      .then(value => { if (active.current) setTitle(value.title); }, () => {});
    await send({ action: 'mount', dimensions: dimensions.current });
  }

  function close(): void {
    active.current = false;
    setFrame(undefined);
    setSelection(undefined);
    void closeSession();
  }

  function point(event: PointerEvent<HTMLImageElement> | WheelEvent<HTMLImageElement>): Point {
    const bounds = event.currentTarget.getBoundingClientRect();
    const { width, height } = dimensions.current;
    return { x: Math.max(0, Math.min(width - 1, (event.clientX - bounds.left) * width / bounds.width)),
      y: Math.max(0, Math.min(height - 1, (event.clientY - bounds.top) * height / bounds.height)) };
  }

  function pointerDown(event: PointerEvent<HTMLImageElement>): void {
    if (!event.isPrimary || event.button !== 0) return;
    pointer.current = { id: event.pointerId, start: point(event), clientX: event.clientX, clientY: event.clientY };
    setSelection(undefined);
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function pointerUp(event: PointerEvent<HTMLImageElement>): void {
    const press = pointer.current;
    if (!press || press.id !== event.pointerId) return;
    pointer.current = undefined;
    const end = point(event);
    const dy = press.clientY - event.clientY;
    if (event.pointerType === 'touch') {
      if (Math.abs(dy) >= 10) {
        void send({ action: 'input', event: { kind: 'scroll', ...end, dy: Math.max(-2000, Math.min(2000, dy)) } });
        return;
      }
    } else if (Math.hypot(press.clientX - event.clientX, dy) > 5) {
      void send({ action: 'input', event: { kind: 'select', start: press.start, end } });
      return;
    }
    void send({ action: 'input', event: { kind: 'tap', ...end } });
  }

  async function copySelection(): Promise<void> {
    setSelection(undefined);
    const result = await send({ action: 'input', event: { kind: 'selection' } });
    const text = result?.selectedText;
    if (!active.current || !text) return;
    try { await navigator.clipboard.writeText(text); }
    catch { if (active.current) setSelection(text); }
  }

  return <>
    <button type="button" className="tw-link" disabled={inFlight.current && !active.current} onClick={() => void open()}>{t('tw_dataset_open_page')}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={heading} onClose={close}>
      <div className="tw-dialog-panel">
        <h3 id={heading}>{title || t('tw_dataset_open_page')}</h3>
        {error && <p role="alert">{messageErrorText(error, locale)}</p>}
        {frame && <>
          <div>
            <img src={'data:image/png;base64,' + frame.png} draggable={false} style={{ display: 'block', width: '100%', touchAction: 'pan-x pinch-zoom' }}
              alt={title || t('tw_dataset_open_page')} onPointerDown={pointerDown}
              onPointerCancel={() => { pointer.current = undefined; }} onPointerUp={pointerUp}
              onWheel={event => void send({ action: 'input', event: { kind: 'scroll', ...point(event), dy: Math.max(-2000, Math.min(2000, event.deltaY)) } })} />
          </div>
          <form onSubmit={event => {
            event.preventDefault();
            const input = new FormData(event.currentTarget).get('text');
            if (typeof input === 'string' && input) { event.currentTarget.reset(); void send({ action: 'input', event: { kind: 'text', text: input } }); }
          }}>
            <label>{t('tw_dataset_page_input')} <input name="text" maxLength={8192} disabled={busy} /></label>
            <button type="submit" className="tw-btn" disabled={busy}>{t('tw_dataset_page_send_input')}</button>
            {['Tab', 'Enter', 'Backspace'].map(key => <button type="button" key={key} className="tw-btn" disabled={busy}
              onClick={() => void send({ action: 'input', event: { kind: 'key', key } })}>{key}</button>)}
            <button type="button" className="tw-btn" disabled={busy} onClick={() => void copySelection()}>
              {t('tw_dataset_page_copy')}
            </button>
          </form>
          {selection && <textarea readOnly value={selection} onFocus={event => event.currentTarget.select()}
            aria-label={t('tw_dataset_page_selection')} />}
          <details><summary>{t('tw_dataset_page_accessibility')}</summary>
            <ul>{frame.accessibility.filter(node => node.name).map(node => <li key={node.id}>
              {node.target ? <button type="button" className="tw-link" disabled={busy}
                onClick={() => void send({ action: 'input', event: { kind: 'focusNode', node: node.target } })}>{node.name}</button> : node.name}
            </li>)}</ul>
          </details>
          {frame.approvals.map(approval => <section key={approval.id}>
            <h4>{approval.mode === 'startTask' ?
              t('tw_dataset_page_start_task') + ' · ' + approval.bot :
              t('tw_dataset_edit_record') + ' ' + approval.record}</h4>
            <pre>{approval.mode === 'startTask' ? approval.prompt : JSON.stringify(approval.parameters, null, 2)}</pre>
            <button type="button" className="tw-primary" disabled={busy}
              onClick={() => void send({ action: 'approve', id: approval.id })}>{t('tw_dataset_page_approve')}</button>
            <button type="button" className="tw-link" disabled={busy}
              onClick={() => void send({ action: 'deny', id: approval.id })}>{t('tw_dataset_page_deny')}</button>
          </section>)}
        </>}
        {session.current && <button type="button" className="tw-link" disabled={busy}
          onClick={() => void send({ action: 'refresh' })}>{t('tw_dataset_refresh')}</button>}
        <form method="dialog"><button type="submit" className="tw-btn">{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}
