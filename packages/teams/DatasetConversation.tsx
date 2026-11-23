import { memo, useCallback, useId, useMemo, useRef, useState, type FormEvent } from 'react';
import { DATA_PROTOCOL_VERSION, type ResourceRef } from './dataset-resource.js';
import { messageErrorText } from './errors.js';
import type { TeamsApi } from './index.js';
import { useTeamsText } from './i18n.js';

type Mode = 'pinned' | 'live' | 'edit';
type Destination = { team: string; conversation: string };
type DataRef = Exclude<ResourceRef, { kind: 'pageVersion' }>;
type DatasetRef = Extract<DataRef, { kind: 'dataset' }>;
type Resource = { reference: DataRef; via?: string | null; access: Mode; canShare: boolean };
type Discovery = { resources: Resource[]; cursor: string | null };
type Description = { schemaVersion: number; access: Mode; schema: { title?: string; properties?: Record<string, unknown> } };
type Row = { reference: Extract<ResourceRef, { kind: 'record' }>; value: Record<string, unknown>; partition: string; archived: boolean };
type Page = { records: Row[]; cursor?: string | null; nextBlock?: number | null };
type ReportPage = {
  title: string;
  metric: { kind: 'count' } | { kind: 'sum'; field: string; scale: number };
  groups: { group: string; units: string }[];
  cursor: string | null;
};
type ShareIntent = {
  action: 'share'; dataset: string; via?: string | null; operation: string; destination: Destination; generation: number;
  mode: Mode; sharedBot?: string; projection: { fields: string[]; partitions?: string[] }; reference?: DataRef; text: string; audienceDigest: string;
};

function request(dataset: string, action: string, fields: Record<string, unknown> = {}, via?: string | null) {
  return { protocol: DATA_PROTOCOL_VERSION, dataset, ...(via ? { via } : {}), command: { action, ...fields } };
}

function codeOf(error: unknown): string {
  return typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string' ? error.code : 'DATA_UNAVAILABLE';
}

export function PersonalDatasetPicker({ api, locale, disabled, onPick }: {
  api: Pick<TeamsApi, 'datasets'>; locale: string; disabled?: boolean; onPick(reference: DatasetRef): void;
}) {
  const t = useTeamsText(locale);
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  const sequence = useRef(0);
  const [items, setItems] = useState<DatasetRef[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  async function list(after?: string): Promise<void> {
    if (!api.datasets) return;
    const current = ++sequence.current;
    if (!after) { setItems([]); setCursor(null); }
    setBusy(true);
    setError('');
    try {
      const result = await api.datasets<Discovery>(request('', 'discover', { scope: { kind: 'mine' }, ...(after ? { after } : {}) }));
      if (sequence.current !== current) return;
      const personal = result.resources.flatMap(({ reference, via }) => reference.kind === 'dataset' && !via ? [reference] : []);
      setItems(previous => after ? [...previous, ...personal] : personal);
      setCursor(result.cursor);
    } catch (failure) {
      if (sequence.current === current) setError(codeOf(failure));
    } finally {
      if (sequence.current === current) setBusy(false);
    }
  }

  return <>
    <button type="button" className="tw-link" disabled={disabled} onClick={() => { dialog.current?.showModal(); void list(); }}>
      {t('tw_dataset_add')}
    </button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={heading} onClose={() => { ++sequence.current; }}>
      <div className="tw-dialog-panel">
        <h2 id={heading} className="tw-dialog-title">{t('tw_dataset_mine')}</h2>
        {error && <p role="alert">{messageErrorText(error, locale)}</p>}
        {!busy && !items.length && !error && <p>{t('tw_dataset_empty')}</p>}
        <ul>{items.map(reference => <li key={reference.dataset}>
          <button type="button" className="tw-link" onClick={() => { onPick(reference); dialog.current?.close(); }}>{reference.dataset}</button>
        </li>)}</ul>
        {cursor && <button type="button" className="tw-link" disabled={busy} onClick={() => void list(cursor)}>{t('tw_dataset_more')}</button>}
        <form method="dialog"><button type="submit" className="tw-btn">{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}

export function DatasetConversationPicker({ api, destination, generation, locale, participantName, sharedBot }: {
  api: TeamsApi; destination: Destination; generation: number; locale: string; participantName(uid: string): string; sharedBot?: string;
}) {
  const t = useTeamsText(locale);
  const dialog = useRef<HTMLDialogElement>(null), heading = useId();
  const [items, setItems] = useState<Resource[]>([]);
  const [discovery, setDiscovery] = useState<{ scope: string; cursor: string | null }>({ scope: 'personal', cursor: null });
  const [choice, setChoice] = useState<{ resource: Resource; fields: string[]; text: string; audience: string[]; digest: string }>();
  const [fields, setFields] = useState<string[]>([]);
  const [partitions, setPartitions] = useState('');
  const [mode, setMode] = useState<Mode>('pinned');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [intent, setIntent] = useState<ShareIntent>();
  const sequence = useRef(0);
  if (!api.datasets) return null;

  async function discover(scope: string, cursor?: string): Promise<void> {
    const current = ++sequence.current;
    setError('');
    setBusy(true);
    if (!cursor) { setItems([]); setChoice(undefined); setIntent(undefined); }
    try {
      const source = scope === 'chat' ? { kind: 'conversation', destination } : { kind: 'mine', ...(scope === 'team' ? { team: destination.team } : {}) };
      const next = await api.datasets!<Discovery>(request('', 'discover', { scope: source, ...(cursor ? { after: cursor } : {}) }));
      if (current !== sequence.current) return;
      setDiscovery({ scope, cursor: next.cursor });
      setItems(previous => cursor ? [...previous, ...next.resources] : next.resources);
    } catch (failure) {
      if (current === sequence.current) {
        setDiscovery({ scope, cursor: null });
        setError(codeOf(failure));
      }
    } finally { if (current === sequence.current) setBusy(false); }
  }

  async function select(resource: Resource): Promise<void> {
    const current = ++sequence.current;
    const { reference, via } = resource;
    setChoice(undefined); setIntent(undefined); setError(''); setBusy(true);
    try {
      const [desc, audience] = await Promise.all([
        api.datasets!<Description>(request(reference.dataset, 'describe', {}, via)),
        api.datasets!<{ audience: Record<string, number>; audienceDigest: string }>(request(reference.dataset, 'audience', { destination }, via)),
      ]);
      if (current !== sequence.current) return;
      const choices = Object.keys(desc.schema.properties ?? {});
      setChoice({ resource, fields: choices, text: desc.schema.title || reference.dataset, audience: Object.keys(audience.audience), digest: audience.audienceDigest });
      setFields(choices); setPartitions(''); setMode('pinned');
    } catch (failure) { if (current === sequence.current) setError(codeOf(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }

  async function publish(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (!choice || !fields.length || busy) return;
    const { reference, via } = choice.resource;
    const pickedPartitions = [...new Set(partitions.split(',').map(partition => partition.trim()).filter(Boolean))];
    const next = intent ?? {
      action: 'share', dataset: reference.dataset, ...(via ? { via } : {}), operation: crypto.randomUUID(), destination, generation,
      mode, ...(sharedBot ? { sharedBot } : {}), projection: { fields, ...(pickedPartitions.length ? { partitions: pickedPartitions } : {}) },
      ...(reference.kind !== 'dataset' ? { reference } : {}), text: choice.text, audienceDigest: choice.digest,
    };
    const current = ++sequence.current;
    setIntent(next); setError(''); setBusy(true);
    try {
      const result = await api.datasets!<{ status: 'published' | 'building' }>({ ...next });
      if (current !== sequence.current) return;
      if (result.status === 'published') { setIntent(undefined); dialog.current?.close(); }
    } catch (failure) { if (current === sequence.current) setError(codeOf(failure)); }
    finally { if (current === sequence.current) setBusy(false); }
  }

  return <>
    <button type="button" className="tw-link" onClick={() => {
      dialog.current?.showModal();
      void discover('personal');
    }}>{t('tw_dataset_add')}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={heading}
      onCancel={event => { if (busy) event.preventDefault(); }}
      onClose={() => { ++sequence.current; setBusy(false); }}>
      <div className="tw-dialog-panel">
        <h2 id={heading} className="tw-dialog-title">{t('tw_dataset_add')}</h2>
        <label>{t('tw_dataset_location')} <select value={discovery.scope} disabled={busy || !!intent}
          onChange={event => void discover(event.currentTarget.value)}>
          <option value="personal">{t('tw_dataset_mine')}</option>
          <option value="team">{t('tw_dataset_team')}</option>
          <option value="chat">{t('tw_dataset_shared')}</option>
        </select></label>
        <ul>{items.map(resource => <li key={JSON.stringify([resource.reference, resource.via ?? null])}>
          <button type="button" className="tw-link" disabled={busy || !resource.canShare || !!intent} onClick={() => void select(resource)}>
            {resource.reference.dataset}
          </button>
        </li>)}</ul>
        {discovery.cursor && <button type="button" className="tw-link" disabled={busy || !!intent}
          onClick={() => void discover(discovery.scope, discovery.cursor!)}>{t('tw_dataset_more')}</button>}
        {!busy && !error && !items.length && <p>{t('tw_dataset_empty')}</p>}
        {error && <p role="alert">{messageErrorText(error, locale)}</p>}
        {choice && <form onSubmit={event => void publish(event)}>
          <h3>{choice.text}</h3>
          <p>{t('tw_dataset_audience_fmt', choice.audience.map(participantName).join(' · '))}</p>
          <label>{t('tw_dataset_access')} <select value={mode} disabled={busy || !!intent} onChange={event => setMode(event.currentTarget.value as Mode)}>
            <option value="pinned">{t('tw_dataset_fixed')}</option>
            {choice.resource.reference.kind === 'dataset' && choice.resource.access !== 'pinned' && <option value="live">{t('tw_dataset_live')}</option>}
            {choice.resource.reference.kind === 'dataset' && choice.resource.access === 'edit' && <option value="edit">{t('tw_dataset_edit')}</option>}
          </select></label>
          <fieldset disabled={busy || !!intent}>
            <legend>{t('tw_dataset_fields')}</legend>
            {choice.fields.map(field => <label key={field}>
              <input type="checkbox" checked={fields.includes(field)}
                onChange={event => setFields(current => event.target.checked ? [...current, field] : current.filter(id => id !== field))} /> {field}
            </label>)}
          </fieldset>
          <label>{t('tw_dataset_partitions')} <input disabled={busy || !!intent} value={partitions}
            onChange={event => setPartitions(event.currentTarget.value)} placeholder={t('tw_dataset_partition_hint')} /></label>
          <button type="submit" className="tw-primary" disabled={busy || !fields.length}>
            {intent ? t('tw_dataset_continue') : t('tw_dataset_share')}
          </button>
        </form>}
        <form method="dialog"><button type="submit" className="tw-btn" disabled={busy}>{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}

export function SharedDatasetCard({ api, reference, via, locale }: {
  api: TeamsApi; reference: DataRef; via: string; locale: string;
}) {
  const dialog = useRef<HTMLDialogElement>(null), heading = useId();
  const sequence = useRef(0);
  const [desc, describe] = useState<Description>();
  const [page, setPage] = useState<Page>();
  const [report, setReport] = useState<ReportPage>();
  const [editing, setEditing] = useState<{ row: Row; text: string; operation: string }>();
  const tableFields = useMemo(() => Object.keys(desc?.schema.properties ?? {}), [desc]);
  const editRow = useCallback((row: Row) => {
    setEditing({ row, text: JSON.stringify(row.value, null, 2), operation: crypto.randomUUID() });
  }, []);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const t = useTeamsText(locale);
  if (!api.datasets) return null;

  async function load(next?: string | number): Promise<void> {
    const current = ++sequence.current;
    const describeRequest = request(reference.dataset, 'describe', {}, via);
    setError(''); setBusy(true); setPage(undefined); setReport(undefined);
    try {
      if (reference.kind === 'reportRelease') {
        const report = await api.datasets!<ReportPage>(request(reference.dataset, 'openReport', {
          release: reference.release, ...(typeof next === 'string' ? { after: next } : {}),
        }, via));
        if (current === sequence.current) setReport(report);
        return;
      }
      const desc = await api.datasets!<Description>(describeRequest);
      const rows = await (async (): Promise<Page> => {
        switch (reference.kind) {
          case 'dataset': return api.datasets!<Page>(request(reference.dataset, 'list', typeof next === 'string' ? { after: next } : {}, via));
          case 'record': return { records: [await api.datasets!<Row>(request(reference.dataset, 'read', { reference }, via))] };
          case 'snapshot': return api.datasets!<Page>(request(reference.dataset, 'openSnapshot', { snapshot: reference.snapshot, block: next ?? 0 }, via));
          default: return { records: [] };
        }
      })();
      if (current !== sequence.current) return;
      describe(desc); setPage(rows); setEditing(undefined);
    } catch (failure) {
      if (current === sequence.current) { describe(undefined); setError(codeOf(failure)); }
    } finally { if (current === sequence.current) setBusy(false); }
  }

  async function save(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (!editing || !desc || busy) return;
    let value: unknown;
    try { value = JSON.parse(editing.text); } catch { setError('INVALID_DATA_REQUEST'); return; }
    if (!value || typeof value !== 'object' || Array.isArray(value)) { setError('INVALID_DATA_REQUEST'); return; }
    const current = ++sequence.current;
    setError(''); setBusy(true);
    try {
      const { row } = editing;
      await api.datasets!({ ...request(reference.dataset, 'put', {
        record: row.reference.record, expectedRevision: row.reference.revision,
        schemaVersion: desc.schemaVersion, partition: row.partition, value,
      }, via), operation: editing.operation });
      if (current === sequence.current) await load();
    } catch (failure) {
      if (current === sequence.current) { setError(codeOf(failure)); setBusy(false); }
    }
  }

  return <>
    <button type="button" className="tw-link" onClick={() => { dialog.current?.showModal(); void load(); }}>
      {reference.kind === 'dataset' ? t('tw_dataset_open_live') : t('tw_dataset_open_fixed')}
    </button>
    <dialog className="tw-dialog tw-data-record" ref={dialog} aria-labelledby={heading}
      onClose={() => { ++sequence.current; setBusy(false); setPage(undefined); setReport(undefined); describe(undefined); setEditing(undefined); }}>
      <div className="tw-dialog-panel">
        <h3 id={heading}>{report?.title ?? (desc?.schema.title || reference.dataset)}</h3>
        {error && <p role="alert">{messageErrorText(error, locale)}</p>}
        {page && <DatasetRows page={page} fields={tableFields} title={desc?.schema.title || reference.dataset}
          editLabel={t('tw_dataset_edit_record')} onEdit={reference.kind === 'dataset' && desc?.access === 'edit' && !busy
            ? editRow : undefined} />}
        {report && <div className="tw-data-table-scroll" tabIndex={0} role="region" aria-label={report.title}>
          <table className="tw-data-table">
            <caption>{report.title}</caption>
            <thead><tr><th scope="col">{t('tw_dataset_report_group')}</th>
              <th scope="col">{report.metric.kind === 'sum' ? report.metric.field : t('tw_dataset_report_count')}</th></tr></thead>
            <tbody>{report.groups.map(({ group, units }) => <tr key={group}>
              <th scope="row">{group}</th>
              <td>{formatReportUnits(units, report.metric.kind === 'sum' ? report.metric.scale : 0)}</td>
            </tr>)}</tbody>
          </table>
        </div>}
        {editing && <form onSubmit={event => void save(event)}>
          <label>{t('tw_dataset_record_fields')} <textarea rows={5} disabled={busy} value={editing.text}
            onChange={event => setEditing({ ...editing, text: event.currentTarget.value, operation: crypto.randomUUID() })} /></label>
          <button type="submit" className="tw-primary" disabled={busy}>{t('tw_dataset_save_record')}</button>
          <button type="button" className="tw-link" disabled={busy} onClick={() => setEditing(undefined)}>{t('tw_dataset_cancel_edit')}</button>
        </form>}
        {page && (page.cursor ?? page.nextBlock) != null && <button type="button" className="tw-link" disabled={busy}
          onClick={() => void load((page.cursor ?? page.nextBlock)!)}>{t('tw_dataset_next')}</button>}
        {report?.cursor != null && <button type="button" className="tw-link" disabled={busy}
          onClick={() => void load(report.cursor!)}>{t('tw_dataset_next')}</button>}
        {!busy && !editing && reference.kind === 'dataset' && <button type="button" className="tw-link" onClick={() => void load()}>{t('tw_dataset_refresh')}</button>}
        <form method="dialog"><button type="submit" className="tw-btn">{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}

function formatReportUnits(units: string, scale: number): string {
  if (scale === 0) return units;
  const negative = units.startsWith('-');
  const digits = (negative ? units.slice(1) : units).padStart(scale + 1, '0');
  return (negative ? '-' : '') + digits.slice(0, -scale) + '.' + digits.slice(-scale);
}

const DatasetRows = memo(function DatasetRows({ page, fields, title, onEdit, editLabel }: {
  page: Page; fields: string[]; title: string; onEdit?(row: Row): void; editLabel: string;
}) {
  return <div className="tw-data-table-scroll" tabIndex={0} role="region" aria-label={title}>
    <table className="tw-data-table">
      <caption>{title}</caption>
      <thead><tr><th scope="col">ID</th>{fields.map(field => <th key={field} scope="col">{field}</th>)}{onEdit && <th scope="col">{editLabel}</th>}</tr></thead>
      <tbody>{page.records.map(row => <tr key={row.reference.record}>
        <th scope="row">{row.reference.record}</th>
        {fields.map(field => <td key={field}>{typeof row.value[field] === 'string' ? row.value[field] as string : JSON.stringify(row.value[field]) ?? '—'}</td>)}
        {onEdit && <td><button type="button" className="tw-link" onClick={() => onEdit(row)}>{editLabel} {row.reference.record}</button></td>}
      </tr>)}</tbody>
    </table>
  </div>;
});
