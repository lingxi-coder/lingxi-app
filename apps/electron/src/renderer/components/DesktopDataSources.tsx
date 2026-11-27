import { useEffect, useId, useRef, useState, type FormEvent, type ReactElement, type ReactNode } from 'react';
import type { TeamSummary, TeamsApi } from '../../../../../packages/teams/index.js';
import { DATA_SOURCE_KINDS, type DataScope, type DataSourceKind, type ManagedDataSource, type SourceDraft, type TeamDataScope } from '../../../../../packages/teams/lingxi-data.js';
import type { TeamConversation } from '../../../../../packages/teams/messages.js';
import { ReportPublisher } from '../../../../../packages/teams/ReportViews.js';
import { tr } from '../i18n/locale';
import { BotReachList } from './BotReachList';

type Row = ManagedDataSource & { running: boolean };
type OpenDataSources = (kind?: DataSourceKind) => void;

export function DesktopDataSources({ api, uid, teams, locale, children }: {
  api: TeamsApi; uid: string; teams: TeamSummary[]; locale: string;
  children?(show?: OpenDataSources): ReactNode;
}): ReactElement | null {
  const desktop = window.lingxi?.teams;
  const zh = locale.toLowerCase().startsWith('zh');
  const heading = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const catalogRequest = useRef(0);
  const [open, setOpen] = useState(false);
  const [kind, setKind] = useState<DataSourceKind>();
  const [rows, setRows] = useState<Row[]>([]);
  const [catalog, setCatalog] = useState<'loading' | 'ready' | 'error'>('loading');
  const [draft, setDraft] = useState<SourceDraft>();
  const [editing, setEditing] = useState<Row>();
  const [preview, setPreview] = useState<{ source: string; rows: Record<string, unknown>[] }>();
  const [reportSource, setReportSource] = useState<{ row: Row; fields: string[] }>();
  const [teamId, setTeamId] = useState('');
  const [roomId, setRoomId] = useState('');
  const [rooms, setRooms] = useState<TeamConversation[]>([]);
  const [running, setRunning] = useState(false);
  const [status, setStatus] = useState('');
  const title = kind ? tr('apps_datasets_sources_for', DATA_SOURCE_KINDS[kind]) : tr('apps_datasets_sources');
  const visibleRows = kind ? rows.filter(source => source.kind === kind) : rows;
  const team = teams.find(value => value.id === teamId && value.role !== 'member');
  const generation = team?.generation;
  const room = rooms.find(value => value.id === roomId);
  let destination: TeamDataScope | undefined;
  if (reportSource?.row.scope.kind === 'team') destination = reportSource.row.scope;
  else if (reportSource && team && room) {
    destination = { kind: 'team', teamId: team.id, conversationId: room.id, generation: team.generation };
  }

  useEffect(() => {
    if (!open || !teamId || generation === undefined) return;
    return api.watchMessages({ teamId, generation }, '', value => {
      if (value.kind === 'directory' && !value.fromCache) setRooms(value.conversations.filter(room => room.participants.includes('user:' + uid)));
    });
  }, [api, open, teamId, generation, uid]);

  if (!desktop?.localDataSource) return children ? <>{children()}</> : null;
  const send = desktop.localDataSource;

  async function readCatalog(): Promise<Row[]> {
    const result = await send(uid, { action: 'list' });
    if (!Array.isArray(result)) throw new Error('DATA_UNAVAILABLE');
    return result;
  }

  async function refresh(): Promise<void> {
    const result = await readCatalog();
    if (dialog.current?.open) setRows(result);
  }

  async function run(action: () => Promise<void>): Promise<void> {
    setRunning(true); setStatus('');
    try { await action(); }
    catch (error) {
      setStatus(kind && error instanceof Error && error.message === 'DATA_SOURCE_KIND_MISMATCH'
        ? tr('apps_datasets_source_mismatch', DATA_SOURCE_KINDS[kind])
        : tr('apps_datasets_source_action_failed'));
    }
    finally { setRunning(false); }
  }

  async function loadCatalog(): Promise<void> {
    const request = ++catalogRequest.current;
    const current = () => request === catalogRequest.current && dialog.current?.open;
    setCatalog('loading');
    try {
      const result = await readCatalog();
      if (current()) {
        setRows(result);
        setCatalog('ready');
      }
    } catch {
      if (current()) setCatalog('error');
    }
  }

  function show(provider?: DataSourceKind): void {
    if (!dialog.current || dialog.current.open) return;
    setKind(provider);
    setRows([]);
    setDraft(undefined);
    setEditing(undefined);
    setPreview(undefined);
    setReportSource(undefined);
    setTeamId(''); setRoomId(''); setRooms([]);
    setStatus('');
    dialog.current.showModal();
    setOpen(true);
    void loadCatalog();
  }

  function add(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!draft || (teamId && (!team || !room))) return;
    const form = new FormData(event.currentTarget);
    const scope: DataScope = editing?.scope ?? (team && room
      ? { kind: 'team', teamId: team.id, conversationId: room.id, generation: team.generation } : { kind: 'personal' });
    const grants = (room?.participants ?? []).filter(value => value.startsWith('shared_bot:')).flatMap(participant => {
      const sharedBotId = participant.slice(11);
      const fields = form.getAll('fields-' + sharedBotId).map(String);
      return fields.length ? [{ sharedBotId, fields, preset: 'main' }] : [];
    });
    void run(async () => {
      try {
        await send(uid, { action: 'configure', input: { source: draft.source, name: editing?.name ?? String(form.get('name')), scope,
          expectedVersion: editing?.version ?? 0, enabled: editing?.enabled ?? true, requesters: form.getAll('requesters').map(String), grants } });
        setDraft(undefined);
        setEditing(undefined);
        if (!editing) await send(uid, { action: 'start', source: draft.source });
      } finally { await refresh(); }
    });
  }

  return <>
    {children ? children(show) : <button type="button" className="tw-head-action no-drag" onClick={() => show()}>
      {tr('apps_datasets_sources')}
    </button>}
    <dialog ref={dialog} className="tw-dialog tw-data-record no-drag desktop-data-sources" aria-labelledby={heading}
      onCancel={event => { if (running) event.preventDefault(); }}
      onClose={event => {
        if (event.currentTarget !== event.target || event.currentTarget.open) return;
        setOpen(false);
        setPreview(undefined);
        setReportSource(undefined);
      }}>
      <div className="tw-dialog-panel tw-data-workspace">
        <h2 id={heading}>{title}</h2>
        {catalog === 'loading' && <p role="status">{tr('apps_datasets_sources_loading')}</p>}
        {catalog === 'error' && <div className="personal-dataset-notice">
          <p role="alert">{tr('apps_datasets_sources_failed')}</p>
          <button type="button" onClick={() => void loadCatalog()}>{tr('common_retry')}</button>
        </div>}
        {status && <p role="status">{status}</p>}
        {visibleRows.length > 0 && <ul>{visibleRows.map(source => <li key={source.source}>
          <strong>{source.name}</strong> · {DATA_SOURCE_KINDS[source.kind] ?? source.kind} · {source.running ? (zh ? '宿主运行中' : 'Host running') :
            source.enabled ? (zh ? '已启用' : 'Enabled') : (zh ? '已停用' : 'Disabled')}
          <div>
            <button type="button" disabled={running} onClick={() => void run(async () => {
              await send(uid, { action: 'check', source: source.source });
              setStatus(zh ? '本机配置与宿主密钥可用。' : 'The local configuration and host key are available.');
            })}>{zh ? '检查配置' : 'Check configuration'}</button>
            {source.enabled && <button type="button" disabled={running} onClick={() => void run(async () => {
              await send(uid, { action: source.running ? 'stop' : 'start', source: source.source }); await refresh();
            })}>{source.running ? (zh ? '停止宿主' : 'Stop host') : (zh ? '启动宿主' : 'Start host')}</button>}
            {source.scope.kind === 'personal' && source.enabled && source.running && <button type="button" disabled={running}
              onClick={() => void run(async () => {
                const result = await send(uid, { action: 'preview', source: source.source });
                if (dialog.current?.open && result && !Array.isArray(result) && 'rows' in result) setPreview({ source: source.source, rows: result.rows });
              })}>{zh ? '测试并预览' : 'Test and preview'}</button>}
            {source.enabled && !source.pending && <button type="button" disabled={running} onClick={() => void run(async () => {
              const result = await send(uid, { action: 'details', source: source.source });
              if (!dialog.current?.open || !result || Array.isArray(result) || !('fields' in result)) return;
              setDraft(undefined); setTeamId(''); setRoomId(''); setRooms([]);
              setReportSource({ row: source, fields: result.fields });
            })}>{zh ? '准备报表' : 'Prepare report'}</button>}
            {source.scope.kind === 'team' && !source.pending && <button type="button" disabled={running}
              onClick={() => void run(async () => {
                const result = await send(uid, { action: 'details', source: source.source });
                if (source.scope.kind !== 'team' || !result || Array.isArray(result) || !('fields' in result)) return;
                if (source.scope.teamId !== teamId) setRooms([]);
                setTeamId(source.scope.teamId); setRoomId(source.scope.conversationId);
                setReportSource(undefined); setEditing(source); setDraft(result);
              })}>{zh ? '编辑预览许可' : 'Edit preview permissions'}</button>}
            <button type="button" disabled={running} onClick={() => void run(async () => {
              const { source: id, scope, name, version, enabled, requesters, grants, pending } = source;
              setPreview(undefined);
              try {
                await send(uid, { action: 'configure', input: { source: id, scope, name, expectedVersion: pending ? version - 1 : version,
                  enabled: pending ? source.desiredEnabled === true : !enabled, requesters, grants } });
              } finally { await refresh(); }
            })}>{source.pending ? (zh ? '重试同步' : 'Retry sync') : source.enabled ? (zh ? '停用并撤权' : 'Disable and revoke') : (zh ? '重新启用' : 'Enable')}</button>
          </div>
          {preview?.source === source.source && <pre>{JSON.stringify(preview.rows, null, 2)}</pre>}
          {open && <BotReachList target={{ kind: 'dataSource', source: source.source }} />}
        </li>)}</ul>}
        {catalog === 'ready' && !draft && <button type="button" disabled={running} onClick={() => void run(async () => {
          const result = await send(uid, { action: 'import', ...(kind ? { kind } : {}) });
          if (result && !Array.isArray(result) && 'fields' in result) {
            setTeamId(''); setRoomId(''); setRooms([]); setReportSource(undefined); setEditing(undefined); setDraft(result);
          }
        })}>{tr('apps_datasets_source_choose_config')}</button>}
        {reportSource?.row.scope.kind === 'personal' && <fieldset><legend>{zh ? '个人数据源的发布目标' : 'Destination for this personal source'}</legend>
          <label>{zh ? '团队' : 'Team'}<select value={teamId} onChange={event => {
            setTeamId(event.currentTarget.value); setRoomId(''); setRooms([]);
          }}><option value="">{zh ? '选择团队' : 'Choose a team'}</option>
            {teams.filter(item => item.role !== 'member').map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
          {team && <label>{zh ? '会话' : 'Conversation'}<select value={roomId} onChange={event => setRoomId(event.currentTarget.value)}>
            <option value="">{zh ? '选择会话' : 'Choose a conversation'}</option>
            {rooms.map(item => <option key={item.id} value={item.id}>{item.title}</option>)}</select></label>}
        </fieldset>}
        {reportSource && destination && <ReportPublisher api={api} locale={locale} scope={destination}
          fields={reportSource.fields} origin={{ kind: 'source', source: reportSource.row.source, version: reportSource.row.version }}
          capture={async (dataset, plan) => {
            const { row } = reportSource;
            const result = await send(uid, { action: 'captureReport', scope: destination, source: row.source, plan, dataset });
            if (!result || Array.isArray(result) || !('capture' in result)) throw new Error('Capture failed');
            return result;
          }} />}
        {draft && <form key={draft.source + (editing ? '-edit' : '-add')} method="post" onSubmit={add}>
          <p>{editing?.name ?? draft.source} · {DATA_SOURCE_KINDS[draft.kind] ?? draft.kind}</p>
          {!editing && <label>{zh ? '名称' : 'Name'}<input name="name" required maxLength={120} autoFocus /></label>}
          <label>{zh ? '范围' : 'Scope'}<select name="scope" value={teamId} disabled={Boolean(editing)} onChange={event => {
            setTeamId(event.currentTarget.value); setRoomId(''); setRooms([]);
          }}><option value="">{zh ? '个人' : 'Personal'}</option>
            {teams.filter(value => value.role !== 'member').map(value => <option key={value.id} value={value.id}>{value.name}</option>)}</select></label>
          {teamId && <label>{zh ? '会话' : 'Conversation'}<select name="conversation" required value={roomId} disabled={Boolean(editing)}
            onChange={event => setRoomId(event.currentTarget.value)}>
            <option value="">{zh ? '选择会话' : 'Select a conversation'}</option>
            {rooms.map(value => <option key={value.id} value={value.id}>{value.title}</option>)}</select></label>}
          {room && <fieldset key={room.id}><legend>{zh ? '私有预览许可' : 'Private preview permissions'}</legend>
            <label>{zh ? '可请求查看的成员' : 'Requesters'}<select name="requesters" multiple defaultValue={editing?.requesters}>
              {room.participants.filter(value => value.startsWith('user:')).map(value => <option key={value} value={value.slice(5)}>{value.slice(5)}</option>)}</select></label>
            {room.participants.filter(value => value.startsWith('shared_bot:')).map(participant => {
              const bot = participant.slice(11);
              return <label key={bot}>{bot} · {zh ? '可预览字段' : 'Preview fields'}
                <select name={'fields-' + bot} multiple defaultValue={editing?.grants.find(grant => grant.sharedBotId === bot)?.fields}>
                  {draft.fields.map(value => <option key={value} value={value}>{value}</option>)}
                </select>
              </label>;
            })}
          </fieldset>}
          <div className="desktop-data-source-actions">
            <button type="button" disabled={running} onClick={() => {
              setDraft(undefined); setEditing(undefined); setStatus('');
            }}>{zh ? '取消' : 'Cancel'}</button>
            <button type="submit" className={children ? 'personal-dataset-primary' : undefined}
              disabled={running || Boolean(teamId && !room)}>{zh ? '保存数据源' : 'Save source'}</button>
          </div>
        </form>}
        {(!children || !draft) && <form method="dialog"><button type="submit" disabled={running}>{zh ? '关闭' : 'Close'}</button></form>}
      </div>
    </dialog>
  </>;
}
