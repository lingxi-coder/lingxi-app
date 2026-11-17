import { useEffect, useRef, useState } from 'react';
import { authorizedTaskReport, taskReportCsv, type AuthorizedTaskReport } from './task-report.js';
import { messageErrorText } from './errors.js';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/** Kept beside recent tasks; only a user action fetches the complete report. */
export function TaskReportExport({ workspace, snapshot, zh }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; zh: boolean }) {
  const [report, setReport] = useState<AuthorizedTaskReport>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [links, setLinks] = useState<{ json: string; csv: string }>();
  const live = useRef(true);
  useEffect(() => { live.current = true; return () => { live.current = false; }; }, []);
  useEffect(() => {
    if (!report) return;
    const json = URL.createObjectURL(new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' }));
    const csv = URL.createObjectURL(new Blob(['\uFEFF', taskReportCsv(report)], { type: 'text/csv;charset=utf-8' }));
    setLinks({ json, csv });
    return () => { URL.revokeObjectURL(json); URL.revokeObjectURL(csv); };
  }, [report]);
  const generate = async () => {
    if (busy || !snapshot.team) return;
    const scope = { teamId: snapshot.team.id, generation: snapshot.team.generation };
    const isCurrent = () => live.current && workspace.isCurrentScope(scope, snapshot.authority);
    setBusy(true); setError(''); setReport(undefined); setLinks(undefined);
    try { const result = await authorizedTaskReport(workspace.api, scope, snapshot.uid, isCurrent); if (isCurrent()) setReport(result); }
    catch (e) { if (isCurrent()) setError(e instanceof Error ? e.message : 'SERVICE_UNAVAILABLE'); }
    finally { if (live.current) setBusy(false); }
  };
  return <div className="tw-task-export">
    <p className="teams-hint">{zh ? '仅导出本人当前成员身份有权访问的任务摘要，不含消息正文、资料或凭据。费用仅汇总完整且已定价的用量，按币种分别计算；缺失、不完整与未定价用量单独保留。' : 'Exports your authorized task summaries for this membership, without message bodies, resources or credentials. Cost totals include only complete priced usage, separated by currency; missing, partial and unpriced usage remain explicit.'}</p>
    <button type="button" disabled={busy} onClick={() => void generate()}>{zh ? '导出本人的任务摘要' : 'Export my task summaries'}</button>
    {busy ? <progress aria-label={zh ? '正在生成任务摘要' : 'Generating task summaries'} /> : null}
    {error ? <p role="alert">{error === 'TASK_REPORT_CURSOR_LOOP' ? (zh ? '分页游标重复，未导出不完整报告，请重试。' : 'The page cursor repeated. No incomplete report was exported. Retry.') : messageErrorText(error, zh)}</p> : null}
    {report && links ? <div role="status"><p>{zh ? `已生成 ${report.totals.tasks} 项任务摘要。` : `${report.totals.tasks} task summaries ready.`}</p>
      <p>{zh ? `用量缺失：${report.totals.missingUsage} · 不完整：${report.totals.partialUsage} · 费用未知：${report.totals.unpricedUsage}` : `Missing usage: ${report.totals.missingUsage} · Partial usage: ${report.totals.partialUsage} · Unpriced usage: ${report.totals.unpricedUsage}`}</p>
      {Object.keys(report.totals.completeCostMicrosByCurrency).length ? <ul>{Object.entries(report.totals.completeCostMicrosByCurrency).map(([currency, cost]) => <li key={currency}>{zh ? '完整已定价费用' : 'Complete priced cost'}: {(cost / 1_000_000).toFixed(6)} {currency}</li>)}</ul> : <p>{zh ? '暂无完整且已定价的费用。' : 'No complete priced usage is available.'}</p>}
      <a href={links.json} download="my-task-summaries.json">{zh ? '下载 JSON' : 'Download JSON'}</a>{' · '}<a href={links.csv} download="my-task-summaries.csv">{zh ? '下载 CSV' : 'Download CSV'}</a></div> : null}
  </div>;
}
