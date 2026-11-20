import { useEffect, useId, useRef, useState, type ReactElement } from 'react';
import { DataRecordButton } from './DataRecordButton.js';
import { DataChart, DataTable } from './DataTable.js';
import { DataTemplate, DataTemplateSettings } from './DataTemplate.js';
import { DataViewSettings } from './DataViewSettings.js';
import { DataAutomationSettings } from './DataAutomationSettings.js';
import { ReportPublisher } from './ReportViews.js';
import type { TeamsApi } from './index.js';
import { parseDataRef, type DataDescription, type DataEventsPage, type DataRecordPage, type DataRef, type DataScope, type DataTemplateView, type VisibleDataSet } from './lingxi-data.js';

type Scope = Extract<DataScope, { kind: 'team' }>;

export function ConversationData({ api, scope, locale, share, shareReport }: {
  api: TeamsApi; scope: Scope; locale: string; share?: (reference: DataRef, text: string) => Promise<boolean>;
  shareReport?: (link: string) => Promise<boolean>;
}): ReactElement | null {
  const dialog = useRef<HTMLDialogElement>(null);
  const headingId = useId();
  const sequence = useRef(0);
  const [datasets, setDatasets] = useState<VisibleDataSet[]>([]);
  const [selected, setSelected] = useState<VisibleDataSet>();
  const [page, setPage] = useState<DataRecordPage>();
  const [description, setDescription] = useState<DataDescription>();
  const [history, setHistory] = useState<DataEventsPage>();
  const [templates, setTemplates] = useState<DataTemplateView[]>([]);
  const [viewId, setViewId] = useState('');
  const [status, setStatus] = useState('');
  const [waiting, setWaiting] = useState(false);
  const [available, setAvailable] = useState(false);
  const { teamId, conversationId, generation } = scope;
  useEffect(() => {
    if (!api.data) return;
    let current = true;
    void api.data<{ datasets: VisibleDataSet[] }>({
      scope: { kind: 'team', teamId, conversationId, generation }, command: { action: 'datasets' },
    }).then(result => { if (current) setAvailable(result.datasets.length > 0); }, () => { if (current) setAvailable(false); });
    return () => { current = false; };
  }, [api, teamId, conversationId, generation]);
  const zh = locale.toLowerCase().startsWith('zh');
  const title = zh ? '会话数据' : 'Conversation data';
  const unavailable = zh ? '无法读取数据。' : 'Could not load the data.';
  if (!api.data || !available) return null;

  function active(request: number): boolean {
    return request === sequence.current && dialog.current?.open === true;
  }

  async function open(): Promise<void> {
    if (!dialog.current || dialog.current.open || !api.data) return;
    const request = ++sequence.current;
    dialog.current.showModal();
    setDatasets([]); setSelected(undefined); setPage(undefined); setDescription(undefined); setHistory(undefined);
    setStatus(''); setWaiting(true);
    try {
      const result = await api.data<{ datasets: VisibleDataSet[] }>({ scope, command: { action: 'datasets' } });
      if (active(request)) {
        setDatasets(result.datasets);
        if (!result.datasets.length) setStatus(zh ? '此会话没有可访问的数据集。' : 'No datasets are available in this conversation.');
      }
    } catch { if (active(request)) setStatus(unavailable); }
    finally { if (active(request)) setWaiting(false); }
  }

  async function show(dataset: VisibleDataSet, after?: string): Promise<void> {
    if (!api.data) return;
    const request = ++sequence.current;
    setSelected(dataset); setWaiting(true); setStatus('');
    if (!after) { setPage(undefined); setDescription(undefined); setHistory(undefined); setTemplates([]); setViewId(''); }
    try {
      const [next, details, layouts] = await Promise.all([
        api.data<DataRecordPage>({ scope, command: { action: 'list', dataset: dataset.dataset, ...(after ? { after } : {}) } }),
        after ? Promise.resolve(undefined) : api.data<DataDescription>({ scope, command: { action: 'describe', dataset: dataset.dataset } }),
        after ? Promise.resolve(undefined) : api.data<{ views: DataTemplateView[] }>({
          scope, command: { action: 'templateViews', dataset: dataset.dataset },
        }).catch(() => ({ views: [] })),
      ]);
      if (active(request)) {
        setPage(next);
        if (details) setDescription(details);
        if (layouts) setTemplates(layouts.views);
      }
    } catch { if (active(request)) setStatus(unavailable); }
    finally { if (active(request)) setWaiting(false); }
  }

  async function showHistory(dataset: string): Promise<void> {
    if (!api.data) return;
    const request = ++sequence.current;
    setWaiting(true);
    setStatus('');
    try {
      const next = await api.data<DataEventsPage>({ scope, command: { action: 'events', dataset, after: history?.cursor ?? 0, waitMs: 0 } });
      if (active(request)) setHistory(next);
    } catch { if (active(request)) setStatus(unavailable); }
    finally { if (active(request)) setWaiting(false); }
  }

  async function send(reference: DataRef): Promise<void> {
    if (!share) return;
    const request = sequence.current;
    setWaiting(true);
    try {
      const queued = await share(reference, zh ? '数据记录 ' + reference.record : 'Data record ' + reference.record);
      if (active(request) && queued) dialog.current?.close();
    } finally { if (active(request)) setWaiting(false); }
  }
  const views = Object.entries(description?.views ?? {});
  const view = description?.views?.[viewId];
  const currentTemplates = templates.filter(entry => entry.schemaVersion === description?.schemaVersion);
  const template = currentTemplates.find(entry => viewId === 'template:' + entry.view);
  const properties = description?.schema.properties;
  const fields = view?.fields ?? (properties && typeof properties === 'object' ? Object.keys(properties) : []);

  return <>
    <button type="button" className="tw-head-action" onClick={() => void open()}>{title}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={headingId} onClose={event => { if (event.target === event.currentTarget) sequence.current += 1; }}>
      <div className="tw-dialog-panel">
        <h2 id={headingId} className="tw-dialog-title">{title}</h2>
        <ul>{datasets.map(dataset => <li key={dataset.dataset}>
          <button type="button" className="tw-link" aria-pressed={selected?.dataset === dataset.dataset} onClick={() => void show(dataset)}>{dataset.dataset}</button>
        </li>)}</ul>
        {waiting && <p role="status">{zh ? '正在加载…' : 'Loading…'}</p>}
        {status && <p role="status">{status}</p>}
        {selected && page && <>
          {(views.length > 0 || currentTemplates.length > 0) && <label>{zh ? '视图' : 'View'} <select name="data-view" value={viewId} onChange={event => setViewId(event.currentTarget.value)}>
            <option value="">{zh ? '表格' : 'Table'}</option>
            {views.map(([id]) => <option key={id} value={id}>{id}</option>)}
            {currentTemplates.map(entry => <option key={entry.view} value={'template:' + entry.view}>{entry.view}</option>)}
          </select></label>}
          {view?.kind === 'chart' && view.fields.map(field => <DataChart key={field} rows={page.records} field={field} />)}
          <DataTable rows={page.records} fields={template ? [] : fields} title={selected.dataset} actionLabel={zh ? '操作' : 'Actions'}
            actions={({ id, record }) => {
            const reference = parseDataRef({ dataset: selected.dataset, record: id, revision: record.revision, schemaVersion: record.schemaVersion });
            return reference && <>
              {template && <DataTemplate spec={template.definition} value={record.value} />}
              <DataRecordButton api={api} scope={scope} reference={reference} locale={locale}
                fields={view?.kind === 'form' ? view.fields : undefined}
                onSaved={selected.access === 'read' ? undefined : () => { void show(selected); }}/>
              {share && <button type="button" className="tw-link" disabled={waiting} onClick={() => void send(reference)}>
                {zh ? '发到对话' : 'Share to chat'}
              </button>}
            </>;
          }}/>
          {page.cursor && <button type="button" className="tw-link" disabled={waiting} onClick={() => void show(selected, page.cursor!)}>{zh ? '下一页' : 'Next page'}</button>}
        </>}
        {selected && description && <>
          {selected.access === 'manage' && <ReportPublisher api={api} scope={scope} locale={locale} dataset={selected.dataset}
            fields={Object.keys(description.schema.properties ?? {})} share={shareReport}
            origin={{ kind: 'team', dataset: selected.dataset, epoch: description.epoch }} />}
          <p>{zh ? 'Schema 版本 ' : 'Schema version '}{description.schemaVersion}</p>
          {selected.access === 'manage' && <DataViewSettings api={api} scope={scope} dataset={selected.dataset}
            description={description} locale={locale} saved={() => { void show(selected); }} />}
          {selected.access === 'manage' && <DataTemplateSettings key={selected.dataset} api={api} scope={scope}
            dataset={selected.dataset} description={description} views={templates} locale={locale} saved={() => { void show(selected); }} />}
          {selected.access === 'manage' && <DataAutomationSettings key={selected.dataset} api={api} scope={scope}
            dataset={selected.dataset} fields={Object.keys(description.schema.properties ?? {})} locale={locale} />}
          {Object.keys(description.rules).length > 0 && <details>
            <summary>{zh ? '规则' : 'Rules'}</summary>
            <ul>{Object.entries(description.rules).map(([name, rule]) => <li key={name}>{name} · {rule.field} = {JSON.stringify(rule.equals)}</li>)}</ul>
          </details>}
          <button type="button" className="tw-link" disabled={waiting} onClick={() => void showHistory(selected.dataset)}>
            {history ? (zh ? '继续读取历史' : 'Read more history') : (zh ? '查看历史' : 'Show history')}
          </button>
          {history && (history.events.length ? <ul>{history.events.map(event => {
            const reference = parseDataRef(event.reference);
            return <li key={event.cursor}>{reference ? <>
              {reference.record} · {zh ? '修订 ' : 'Revision '}{reference.revision}{' '}
              <DataRecordButton api={api} scope={scope} reference={reference} locale={locale}/>
            </> : event.kind}</li>;
          })}</ul> : <p>{zh ? '没有更多事件。' : 'No more events.'}</p>)}
        </>}
        <form method="dialog"><button type="submit" className="tw-btn" autoFocus>{zh ? '关闭' : 'Close'}</button></form>
      </div>
    </dialog>
  </>;
}
