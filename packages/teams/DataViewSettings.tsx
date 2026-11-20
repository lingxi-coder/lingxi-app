import { useState, type FormEvent, type ReactElement } from 'react';
import type { TeamsApi } from './index.js';
import type { DataDescription, DataScope } from './lingxi-data.js';

export function DataViewSettings({ api, scope, dataset, description, locale, saved }: {
  api: TeamsApi; scope: DataScope; dataset: string; description: DataDescription; locale: string; saved(): void;
}): ReactElement | null {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const zh = locale.toLowerCase().startsWith('zh');
  const fields = Object.keys(description.schema.properties ?? {});
  if (!api.data || !fields.length) return null;
  const data = api.data;

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const kind = form.get('kind');
    if (kind !== 'table' && kind !== 'chart' && kind !== 'form') return;
    const chosen = form.getAll('fields').map(String);
    if (!chosen.length) { setFailed(true); return; }
    setBusy(true); setFailed(false);
    void data({ scope, operation: crypto.randomUUID(), command: { action: 'setView', dataset, expectedEpoch: description.epoch,
      view: String(form.get('view')), definition: { kind, fields: chosen } } })
      .then(saved, () => setFailed(true)).finally(() => setBusy(false));
  }

  return <details className="tw-data-workspace">
    <summary>{zh ? '配置数据视图' : 'Configure a data view'}</summary>
    <form method="post" onSubmit={submit}>
      <label>{zh ? '视图 ID' : 'View ID'}<input name="view" required pattern="[A-Za-z0-9_\-]{1,64}" maxLength={64}/></label>
      <label>{zh ? '类型' : 'Type'}<select name="kind"><option value="table">{zh ? '表格' : 'Table'}</option>
        <option value="chart">{zh ? '图表' : 'Chart'}</option><option value="form">{zh ? '表单' : 'Form'}</option></select></label>
      <label>{zh ? '字段' : 'Fields'}<select name="fields" required multiple>
        {fields.map(field => <option key={field} value={field}>{field}</option>)}</select></label>
      {failed && <p role="alert">{zh ? '未能保存视图。请检查字段和当前版本。' : 'Could not save the view. Check the fields and current revision.'}</p>}
      <button type="submit" disabled={busy}>{zh ? '保存视图' : 'Save view'}</button>
    </form>
  </details>;
}
