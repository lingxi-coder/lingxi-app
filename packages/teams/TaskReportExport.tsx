import { useEffect, useRef, useState } from 'react';
import { authorizedTaskReport, taskReportCsv, type AuthorizedTaskReport } from './task-report.js';
import { messageErrorText } from './errors.js';
import { useTeamsText } from './i18n.js';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/** Kept beside recent tasks; only a user action fetches the complete report. */
export function TaskReportExport({ workspace, snapshot, locale }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string }) {
  const t = useTeamsText(locale);
  const [report, setReport] = useState<AuthorizedTaskReport>(), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [links, setLinks] = useState<{ json: string; csv: string }>();
  const live = useRef(true);
  useEffect(() => { live.current = true; return () => { live.current = false; }; }, []);
  useEffect(() => {
    if (!report) return;
    const json = URL.createObjectURL(new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' }));
    const csv = URL.createObjectURL(new Blob(['﻿', taskReportCsv(report)], { type: 'text/csv;charset=utf-8' }));
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
    <p className="teams-hint">{t('tw_report_intro')}</p>
    <button type="button" disabled={busy} onClick={() => void generate()}>{t('tw_report_export')}</button>
    {busy ? <progress aria-label={t('tw_report_generating')} /> : null}
    {error ? <p role="alert">{error === 'TASK_REPORT_CURSOR_LOOP' ? t('tw_report_cursor_loop') : messageErrorText(error, locale)}</p> : null}
    {report && links ? <div role="status"><p>{t(report.totals.tasks === 1 ? 'tw_report_ready_one_fmt' : 'tw_report_ready_other_fmt', report.totals.tasks)}</p>
      <p>{[t('tw_report_missing_fmt', report.totals.missingUsage), t('tw_report_partial_fmt', report.totals.partialUsage), t('tw_report_unpriced_fmt', report.totals.unpricedUsage)].join(' · ')}</p>
      {Object.keys(report.totals.completeCostMicrosByCurrency).length ? <ul>{Object.entries(report.totals.completeCostMicrosByCurrency).map(([currency, cost]) => <li key={currency}>{t('tw_report_cost_fmt', (cost / 1_000_000).toFixed(6), currency)}</li>)}</ul> : <p>{t('tw_report_no_cost')}</p>}
      <a href={links.json} download="my-task-summaries.json">{t('tw_report_download_json')}</a>{' · '}<a href={links.csv} download="my-task-summaries.csv">{t('tw_report_download_csv')}</a></div> : null}
  </div>;
}
