import { useId, useRef, useState, type FormEvent, type ReactElement } from 'react';
import { DataTemplate } from './DataTemplate.js';
import type { TeamsApi } from './index.js';
import { parseDataRef, type DataRecordResult, type DataRef, type DataScope, type DataTemplateView, type VisibleDataSet } from './lingxi-data.js';

type ReadState = { state: 'idle' | 'loading' | 'unavailable' } | { state: 'loaded'; data: DataRecordResult };

const copy = {
  en: { open: 'Open record', title: 'Data record', loading: 'Loading record…', unavailable: 'This record is unavailable.', close: 'Close' },
  'zh-Hans': { open: '打开记录', title: '数据记录', loading: '正在读取记录…', unavailable: '当前无法读取此记录。', close: '关闭' },
  'zh-Hant': { open: '開啟記錄', title: '資料記錄', loading: '正在讀取記錄…', unavailable: '目前無法讀取此記錄。', close: '關閉' },
  ja: { open: 'レコードを開く', title: 'データレコード', loading: 'レコードを読み込み中…', unavailable: 'このレコードは利用できません。', close: '閉じる' },
  ko: { open: '레코드 열기', title: '데이터 레코드', loading: '레코드를 불러오는 중…', unavailable: '이 레코드를 사용할 수 없습니다.', close: '닫기' },
} as const;

function labels(locale: string) {
  const value = locale.toLowerCase();
  if (value.startsWith('ja')) return copy.ja;
  if (value.startsWith('ko')) return copy.ko;
  if (value.startsWith('zh-hant') || value === 'zh-tw' || value === 'zh-hk') return copy['zh-Hant'];
  if (value.startsWith('zh')) return copy['zh-Hans'];
  return copy.en;
}

function valueText(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value) ?? '';
}

export function DataRecordButton({ api, scope, reference, locale, onSaved, allowEdit, fields: selectedFields }: {
  api: TeamsApi; scope: DataScope; reference: DataRef; locale: string; onSaved?: () => void; allowEdit?: boolean; fields?: string[];
}): ReactElement | null {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const requestId = useRef(0);
  const [state, setState] = useState<ReadState>({ state: 'idle' });
  const [saving, setSaving] = useState(false);
  const [writeError, setWriteError] = useState('');
  const [conflict, setConflict] = useState(false);
  const [templates, setTemplates] = useState<DataTemplateView[]>([]);
  const [activeTemplate, setActiveTemplate] = useState('');
  const [canEdit, setCanEdit] = useState(false);
  const pendingWrite = useRef<{ payload: string; operation: string }>();
  const label = labels(locale);
  const zh = locale.toLowerCase().startsWith('zh');

  if (!api.data) return null;

  async function open(latest = false): Promise<void> {
    const node = dialog.current;
    if (!node || node.open !== latest || !api.data) return;
    const serial = ++requestId.current;
    if (!latest) node.showModal();
    setState({ state: 'loading' });
    setWriteError('');
    setConflict(false);
    setSaving(false);
    setTemplates([]);
    setActiveTemplate('');
    setCanEdit(false);
    const current = (): boolean => dialog.current === node && node.open && serial === requestId.current;
    try {
      const target = latest ? parseDataRef(await api.data<DataRef>({ scope, command: { action: 'latest',
        dataset: reference.dataset, record: reference.record } })) : reference;
      if (!target || target.dataset !== reference.dataset || target.record !== reference.record) throw new Error('Invalid record');
      const [data, layouts, access] = await Promise.all([
        api.data<DataRecordResult>({ scope, command: { action: 'get', reference: target } }),
        api.data<{ views: DataTemplateView[] }>({ scope, command: { action: 'templateViews', dataset: target.dataset } })
          .catch(error => {
            if (error instanceof Error && error.message === 'INVALID_DATA_REQUEST') return { views: [] };
            throw error;
          }),
        allowEdit || onSaved
          ? api.data<{ datasets: VisibleDataSet[] }>({ scope, command: { action: 'datasets' } })
          : Promise.resolve({ datasets: [] }),
      ]);
      if (!current()) return;
      const received = parseDataRef(data.reference);
      if (!received || received.dataset !== reference.dataset || received.record !== reference.record
        || received.revision !== target.revision || received.schemaVersion !== target.schemaVersion
        || !data.schema || typeof data.schema !== 'object' || Array.isArray(data.schema)
        || !data.record?.value || typeof data.record.value !== 'object' || Array.isArray(data.record.value)) {
        setState({ state: 'unavailable' });
        return;
      }
      setTemplates(layouts.views.filter(entry => entry.schemaVersion === target.schemaVersion));
      setCanEdit(access.datasets.some(entry => entry.dataset === target.dataset && entry.access !== 'read'));
      setState({ state: 'loaded', data });
    } catch {
      if (current()) setState({ state: 'unavailable' });
    }
  }

  async function save(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!canEdit || !api.data || state.state !== 'loaded' || saving) return;
    const status = requestId.current;
    const values = new FormData(event.currentTarget);
    const active = state.data.reference;
    const fields = schemaFields(state.data.schema).filter(([key]) => !selectedFields || selectedFields.includes(key));
    const value: Record<string, unknown> = Object.create(null);
    try {
      if (selectedFields) for (const [key, existing] of Object.entries(state.data.record.value)) {
        if (!selectedFields.includes(key)) value[key] = existing;
      }
      for (const [key, field] of fields) {
        const text = values.get(key);
        if (typeof text !== 'string') continue;
        value[key] = field.type === 'string' && !(Array.isArray(field.enum) && field.enum.length) ? text : JSON.parse(text);
      }
    } catch {
      setWriteError(zh ? '请检查字段格式。' : 'Check the field format.');
      return;
    }
    const payload = JSON.stringify([active, value]);
    if (pendingWrite.current?.payload !== payload) pendingWrite.current = { payload, operation: crypto.randomUUID() };
    setSaving(true);
    setWriteError('');
    setConflict(false);
    try {
      await api.data<DataRef>({
        scope, operation: pendingWrite.current.operation, command: { action: 'put',
          dataset: active.dataset, record: active.record, expectedRevision: active.revision,
          schemaVersion: active.schemaVersion, value },
      });
      pendingWrite.current = undefined;
      if (status === requestId.current && dialog.current?.open) {
        dialog.current.close();
        onSaved?.();
      }
    } catch (error) {
      if (status === requestId.current && dialog.current?.open) {
        const code = error instanceof Error ? error.message : '';
        if (['DATA_ACCESS_REQUIRED', 'INVALID_SESSION', 'CONVERSATION_REQUIRED', 'MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP'].includes(code)) {
          pendingWrite.current = undefined;
          setState({ state: 'unavailable' });
          return;
        }
        const changed = code === 'DATA_CONFLICT';
        setConflict(changed);
        setWriteError(changed
          ? (zh ? '数据已更新，请刷新后再编辑。' : 'The record changed. Reload before editing.')
          : (zh ? '无法保存这次修改。' : 'Could not save this change.'));
      }
    } finally { if (status === requestId.current) setSaving(false); }
  }

  const template = templates.find(entry => entry.view === activeTemplate);
  return <>
    <button type="button" className="tw-link" onClick={() => void open()}>{label.open}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={titleId} onClose={() => {
      requestId.current += 1;
      setState({ state: 'idle' });
    }}>
      <div className="tw-dialog-panel">
        <h2 id={titleId} className="tw-dialog-title">{label.title}</h2>
        {state.state === 'loading' || state.state === 'unavailable'
          ? <p role="status">{state.state === 'loading' ? label.loading : label.unavailable}</p> : null}
        {state.state === 'loaded' && templates.length > 0 && <label>{zh ? '视图' : 'View'}
          <select value={activeTemplate} onChange={event => setActiveTemplate(event.currentTarget.value)}>
            <option value="">{zh ? '字段' : 'Fields'}</option>
            {templates.map(entry => <option key={entry.view} value={entry.view}>{entry.view}</option>)}
          </select>
        </label>}
        {state.state === 'loaded' && template && <DataTemplate spec={template.definition} value={state.data.record.value} />}
        {state.state === 'loaded' && !template ? <dl className="tw-data-fields">
          {Object.entries(state.data.record.value).filter(([key]) => !selectedFields || selectedFields.includes(key)).map(([key, value]) => {
            const field = state.data.schema?.properties as Record<string, { title?: unknown }> | undefined;
            const title = field?.[key]?.title;
            return <div key={key}><dt>{typeof title === 'string' ? title : key}</dt><dd>{valueText(value)}</dd></div>;
          })}
        </dl> : null}
        {state.state === 'loaded' && canEdit && !state.data.record.archived && <form method="post" className="tw-form" onSubmit={event => void save(event)}>
          <fieldset disabled={saving || undefined} className="tw-form">
            <legend>{zh ? '编辑记录' : 'Edit record'}</legend>
            {schemaFields(state.data.schema).filter(([key]) => !selectedFields || selectedFields.includes(key)).map(([key, field]) => <RecordField key={key} name={key} field={field}
              required={Array.isArray(state.data.schema.required) && state.data.schema.required.includes(key)}
              record={state.data.record.value} zh={zh}/>)}
            <button type="submit" className="tw-btn">{zh ? '保存修改' : 'Save changes'}</button>
          </fieldset>
          {writeError && <p role="alert">{writeError}</p>}
          {conflict && <button type="button" className="tw-link" onClick={() => void open(true)}>{zh ? '读取最新修订' : 'Load latest revision'}</button>}
        </form>}
        <form method="dialog"><button type="submit" className="tw-btn" autoFocus>{label.close}</button></form>
      </div>
    </dialog>
  </>;
}

type Field = { type?: unknown; title?: unknown; enum?: unknown[] };

function schemaFields(schema: Record<string, unknown>): [string, Field][] {
  const properties = schema.properties;
  if (!properties || typeof properties !== 'object' || Array.isArray(properties)) return [];
  return Object.entries(properties).filter((entry): entry is [string, Field] =>
    !!entry[1] && typeof entry[1] === 'object' && !Array.isArray(entry[1]));
}

function RecordField({ name, field, required, record, zh }: {
  name: string; field: Field; required: boolean; record: Record<string, unknown>; zh: boolean;
}): ReactElement {
  const [include, setInclude] = useState(required || Object.prototype.hasOwnProperty.call(record, name));
  const title = typeof field.title === 'string' ? field.title : name;
  const choices = Array.isArray(field.enum) ? field.enum : [];
  const value = record[name];
  const initial = choices.length && value === undefined ? valueText(choices[0]) : valueText(value);
  return <div className="tw-form">
    {!required && <label className="tw-check">
      <input type="checkbox" checked={include} onChange={event => setInclude(event.target.checked)}/>
      {zh ? '包含字段 ' + title : 'Include ' + title}
    </label>}
    <label>{title}
      {choices.length > 0 ? <select name={name} disabled={!include} defaultValue={
        JSON.stringify(value === undefined ? choices[0] : value)
      }>{choices.map((choice, index) => <option key={index} value={JSON.stringify(choice)}>{valueText(choice)}</option>)}</select>
        : field.type === 'string' ? <input name={name} disabled={!include} defaultValue={initial}/>
          : <textarea name={name} disabled={!include} rows={2} defaultValue={JSON.stringify(value) ?? ''}
            aria-label={title + (zh ? '（JSON）' : ' (JSON)')}/>}
    </label>
  </div>;
}
