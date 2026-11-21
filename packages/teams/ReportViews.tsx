import { useEffect, useRef, useState, type FormEvent, type ReactElement } from 'react';
import type { TeamsApi } from './index.js';
import type { TeamDataScope, VisibleDataSet } from './lingxi-data.js';
import { reportLink, type PublishedReport, type ReportCapture, type ReportContext, type ReportDraft,
  type ReportOrigin, type ReportPlan, type ReportRef, type ReportSpec } from './reports.js';

type Capture = (dataset: string, plan: ReportPlan) => Promise<ReportDraft & { capture: ReportCapture }>;
type ScopeProps = { api: TeamsApi; scope: TeamDataScope; locale: string };
type PublisherProps = ScopeProps & { origin: ReportOrigin; dataset?: string; fields: string[];
  capture?: Capture; share?: (link: string) => Promise<boolean> };

export function ReportCard({ api, scope, locale, reference }: ScopeProps & { reference: ReportRef }): ReactElement {
  const zh = locale.startsWith('zh');
  const [report, setReport] = useState<PublishedReport>();
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  useEffect(() => {
    setReport(undefined); setStatus(''); setBusy(false);
    return () => { generation.current++; };
  }, [reference.dataset, reference.release, scope.teamId, scope.conversationId, scope.generation, api]);
  async function open(release: string | null): Promise<void> {
    if (!api.data) return;
    const run = ++generation.current;
    setBusy(true); setReport(undefined); setStatus('');
    try {
      const result = await api.data<PublishedReport>({ scope, command: { action: 'getReport', dataset: reference.dataset, release } });
      if (run === generation.current) setReport(result);
    } catch { if (run === generation.current) setStatus(zh ? '报表不可用或无权查看。' : 'Report unavailable or access denied.'); }
    finally { if (run === generation.current) setBusy(false); }
  }
  async function review(partition: string, group: string): Promise<void> {
    if (!report || !api.data) return;
    const run = generation.current;
    setBusy(true); setStatus('');
    try {
      await api.data({ scope, command: { action: 'openReportReview', dataset: report.dataset, release: report.release, partition, group } });
      if (generation.current === run) setStatus(zh ? '复核单已打开' : 'Review case open');
    } catch { if (run === generation.current) { setReport(undefined); setStatus(zh ? '请重新打开报表。' : 'Reopen the report.'); } }
    finally { if (run === generation.current) setBusy(false); }
  }
  const maximum = report?.points.reduce((n, point) => Math.max(n, Math.abs(Number(point.value))), 0) || 1;
  return <section className="tw-report">
    <button type="button" disabled={busy} onClick={() => void open(reference.release)}>{zh ? '查看发布时的报表' : 'Open published report'}</button>
    <button type="button" disabled={busy} onClick={() => void open(null)}>{zh ? '查看最新报表' : 'Open latest report'}</button>
    {report && <><h4>{report.spec.title}</h4><small>{new Date(report.createdAt).toLocaleString(locale)} · {report.release.slice(0, 12)}</small>
      <details><summary>{zh ? '数据来源' : 'Data provenance'}</summary>
        <p>{report.provenance.kind} · {report.provenance.version} {report.provenance.inputDigest && <code>{report.provenance.inputDigest}</code>}</p></details>
      <table><caption>{report.spec.title}</caption><tbody>{report.points.map(point => <tr key={point.group}>
        <th scope="row">{point.group}</th><td><meter aria-label={point.group} min={0} max={maximum} value={Math.abs(Number(point.value))} /></td>
        <td>{point.value}</td></tr>)}</tbody></table>
      {report.points.length === 0 && <p>{zh ? '没有可见数据' : 'No visible data'}</p>}
      {report.canReview && report.cells.length > 0 && <details><summary>{zh ? '复核分区' : 'Review a partition'}</summary>
        {report.cells.map(cell => <p key={JSON.stringify([cell.partition, cell.group])}>{cell.partition} · {cell.group} · {cell.value}
          {' '}<button type="button" disabled={busy} onClick={() => void review(cell.partition, cell.group)}>{zh ? '打开复核单' : 'Open review case'}</button></p>)}</details>}
    </>}
    <p role="status">{status}</p>
  </section>;
}

export function ReportPublisher({ api, scope, locale, origin, dataset, fields, capture, share }: PublisherProps): ReactElement {
  const zh = locale.startsWith('zh');
  const [targets, setTargets] = useState<VisibleDataSet[]>([]);
  const [target, setTarget] = useState(dataset ?? '');
  const [context, setContext] = useState<ReportContext>();
  const [prepared, setPrepared] = useState<{ plan: ReportPlan; draft: ReportDraft & { capture?: ReportCapture } }>();
  const [published, setPublished] = useState<ReportRef>();
  const [shared, setShared] = useState(false);
  const messageId = useRef('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('');
  const originKey = JSON.stringify(origin);
  const seq = useRef(0);
  useEffect(() => {
    setTarget(dataset ?? ''); setContext(undefined); setPrepared(undefined); setPublished(undefined); setBusy(false); setStatus('');
    return () => { seq.current++; };
  }, [api, dataset, scope.teamId, scope.conversationId, scope.generation, originKey]);

  async function run(work: (current: () => boolean) => Promise<void>): Promise<void> {
    const id = ++seq.current;
    setBusy(true); setStatus('');
    try { await work(() => id === seq.current); }
    catch { if (id === seq.current) { setPrepared(undefined); setStatus(zh ? '请核对字段、权限和发布版本后重试。' : 'Check fields, access and the release version, then retry.'); } }
    finally { if (id === seq.current) setBusy(false); }
  }
  function load(id: string): void {
    setContext(undefined); setPrepared(undefined); setPublished(undefined); setTarget(id);
    if (!api.data) return;
    const send = api.data;
    void run(async current => {
      if (!id) {
        const answer = await send<{ datasets: VisibleDataSet[] }>({ scope, command: { action: 'datasets' } });
        if (current()) setTargets(answer.datasets.filter(entry => entry.access === 'manage'));
      } else {
        const answer = await send<ReportContext>({ scope, command: { action: 'reportContext', dataset: id } });
        if (current()) setContext(answer);
      }
    });
  }
  function preview(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!context || !api.data) return;
    const form = new FormData(event.currentTarget);
    const value = (name: string) => String(form.get(name) ?? '').trim();
    const measure = value('measure');
    const spec: ReportSpec = { title: value('title'), partition: value('partition'), group: value('group'),
      metric: measure ? { kind: 'sum', field: measure, scale: Number(value('scale')) } : { kind: 'count' } };
    const readers = context.members.flatMap(({ uid }) => {
      const lines = value('reader-' + uid);
      return lines ? [{ uid, partitions: lines === '*' ? '*' as const : [...new Set(lines.split(/\r?\n/u).map(line => line.trim()).filter(Boolean))] }] : [];
    });
    const plan: ReportPlan = { origin, spec, readers, bots: value('bots').split(/\s+/u).filter(Boolean), parent: context.head, audienceRevision: context.audienceRevision };
    const send = api.data;
    void run(async current => {
      const result = capture ? await capture(target, plan) : await send<ReportDraft>({ scope, command: { action: 'prepareReport', dataset: target, plan } });
      if (current()) setPrepared({ plan, draft: result });
    });
  }
  function publish(): void {
    if (!prepared || !api.data) return;
    const { draft, plan } = prepared;
    const send = api.data;
    void run(async current => {
      const result = await send<ReportRef>({ scope, command: { action: 'publishReport', dataset: target, plan,
        confirmation: draft.confirmation, ...(draft.capture ? { capture: draft.capture } : {}) } });
      if (current()) {
        setPublished(result); setPrepared(undefined); setContext(undefined); setShared(false);
        messageId.current = crypto.randomUUID();
      }
    });
  }
  const choices = fields.map(name => <option key={name} value={name}>{name}</option>);
  const shareLabel = zh ? '发送报表到对话' : 'Share report to chat';
  const sharedLabel = zh ? '已发送' : 'Shared';
  return <details><summary>{zh ? '发布报表' : 'Publish a report'}</summary>
    <button type="button" disabled={busy} onClick={() => load(dataset ?? '')}>{zh ? '选择发布目标和受众' : 'Choose destination and audience'}</button>
    {!dataset && targets.length > 0 && <label>{zh ? '目标数据集' : 'Destination dataset'}<select value={target} onChange={e => load(e.target.value)}>
      <option value="">{zh ? '选择数据集' : 'Choose dataset'}</option>{targets.map(item => <option key={item.dataset}>{item.dataset}</option>)}</select></label>}
    {context && <form aria-label={zh ? '报表发布设置' : 'Report publication settings'} onSubmit={preview} onChange={() => setPrepared(undefined)}><fieldset disabled={busy}>
      <legend>{zh ? '指标与受众' : 'Measure and audience'}</legend>
      <label>{zh ? '报表名称' : 'Report title'}<input name="title" required maxLength={160} /></label>
      <label>{zh ? '权限分区字段' : 'Access partition field'}<select name="partition" required>{choices}</select></label>
      <label>{zh ? '分组字段' : 'Group field'}<select name="group" required>{choices}</select></label>
      <label>{zh ? '指标' : 'Measure'}<select name="measure"><option value="">{zh ? '记录数' : 'Record count'}</option>{choices}</select></label>
      <label>{zh ? '求和小数位' : 'Sum decimal places'}<input type="number" name="scale" defaultValue="2" min="0" max="12" /></label>
      <fieldset><legend>{zh ? '读者可看的分区，每行一个。* 表示全部，留空不分享。' : 'One partition per line. Use * for all or leave blank to omit a reader.'}</legend>
        {context.members.map(member => <label key={member.uid}>{member.uid}<textarea name={'reader-' + member.uid} rows={1}
          defaultValue={member.access === 'manage' ? '*' : ''} /></label>)}</fieldset>
      <label>{zh ? '可处理报表的 Bot ID（空格分隔）' : 'Allowed Bot IDs (space separated)'}<input name="bots" /></label>
      <button type="submit">{zh ? '预览发布内容' : 'Preview publication'}</button></fieldset></form>}
    {prepared && <section><h4>{prepared.plan.spec.title}</h4>
      <p>{prepared.plan.readers.map(reader => reader.uid + ' (' + (Array.isArray(reader.partitions) ? reader.partitions.join(', ') : '*') + ')').join(' · ')}</p>
      <table><caption>{zh ? '将发布的汇总' : 'Aggregates to publish'}</caption><tbody>
        {prepared.draft.facts.map(cell => <tr key={JSON.stringify([cell.partition, cell.group])}>
          <th scope="row">{cell.partition}</th><td>{cell.group}</td><td>{formatUnits(cell.units, prepared.plan.spec.metric)}</td></tr>)}</tbody></table>
      <button type="button" disabled={busy} onClick={publish}>{zh ? '确认发布此版本' : 'Confirm this release'}</button></section>}
    {published && <><ReportCard api={api} scope={scope} locale={locale} reference={published} />
      <button type="button" disabled={busy || shared} onClick={() => void run(async current => {
        const text = reportLink(published);
        if (share) {
          if (!await share(text)) throw new Error('Share failed');
        } else {
          await api.messages({ action: 'mutate', teamId: scope.teamId, generation: scope.generation, conversationId: scope.conversationId,
            messageId: messageId.current, changeId: messageId.current, operation: 'create', text });
        }
        if (current()) setShared(true);
      })}>{shared ? sharedLabel : shareLabel}</button></>}
    <p role="status">{status}</p>
  </details>;
}

function formatUnits(units: string, metric: ReportSpec['metric']): string {
  if (metric.kind === 'count' || metric.scale === 0) return units;
  const digits = units.replace(/^-/, '').padStart(metric.scale + 1, '0');
  return (units.startsWith('-') ? '-' : '') + digits.slice(0, -metric.scale) + '.' + digits.slice(-metric.scale);
}
