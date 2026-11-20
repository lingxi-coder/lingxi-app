import type { ReactElement, ReactNode } from 'react';
import type { DataRecordPage } from './lingxi-data.js';

export function DataTable({ rows, fields, title, actions, actionLabel }: {
  rows: DataRecordPage['records']; fields: string[]; title: string;
  actions?: (row: DataRecordPage['records'][number]) => ReactNode; actionLabel?: string;
}): ReactElement {
  return <div className="tw-data-table-scroll" tabIndex={0} aria-label={title} role="region">
    <table className="tw-data-table">
      <caption>{title}</caption>
      <thead><tr><th scope="col">ID</th>{fields.map(field => <th key={field} scope="col">{field}</th>)}
        {actions && <th scope="col">{actionLabel}</th>}</tr></thead>
      <tbody>{rows.map(row => <tr key={row.id}>
        <th scope="row">{row.id}</th>
        {fields.map(field => {
          const value = row.record.value[field];
          return <td key={field}>{typeof value === 'string' ? value : JSON.stringify(value) ?? '—'}</td>;
        })}
        {actions && <td>{actions(row)}</td>}
      </tr>)}</tbody>
    </table>
  </div>;
}

export function DataChart({ rows, field }: { rows: DataRecordPage['records']; field: string }): ReactElement {
  const points = rows.flatMap(({ id, record }) => {
    const value = record.value[field];
    return typeof value === 'number' && Number.isFinite(value) ? [{ id, value }] : [];
  });
  const bound = Math.max(1, ...points.map(({ value }) => Math.abs(value)));
  return <figure className="tw-data-chart">
    <figcaption>{field}</figcaption>
    <ul>{points.map(({ id, value }) => <li key={id}><span>{id}</span>
      <span aria-hidden="true" className="tw-data-bar" style={{ inlineSize: `${100 * Math.abs(value) / bound}%` }} />
      <span>{value}</span></li>)}</ul>
  </figure>;
}
