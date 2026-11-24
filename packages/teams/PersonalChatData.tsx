import { useEffect, useId, useRef, useState, type FormEvent, type ReactElement } from 'react';
import { DataRecordButton } from './DataRecordButton.js';
import { DataTable } from './DataTable.js';
import { DataTemplate, DataTemplateSettings } from './DataTemplate.js';
import { DataViewSettings } from './DataViewSettings.js';
import { ReportPublisher } from './ReportViews.js';
import type { TeamSummary, TeamsApi } from './index.js';
import { parseDataRef, type DataAudience, type DataDescription, type DataRecordPage, type DataRef, type DataTemplateView, type TeamDataScope, type VisibleDataSet } from './lingxi-data.js';
import type { TeamConversation } from './messages.js';

const personal = { kind: 'personal' } as const;
type Confirmation = {
  target: TeamDataScope; audience: DataAudience; records: DataRef[]; operation: string; labels: Map<string, string>;
  dataset: string; schemaVersion: number; sourceEpoch: number; templates: { view: string; version: number }[];
};

function jsonObject(input: FormDataEntryValue | null): Record<string, unknown> {
  if (typeof input !== 'string') throw new Error('Expected JSON');
  const value: unknown = JSON.parse(input);
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected a JSON object');
  return value as Record<string, unknown>;
}

type PersonalDataProps = { api: TeamsApi; uid: string; teams: TeamSummary[]; locale: string; chatReply?: string };

/** The "Personal data" action of a chat: the personal datasets in a dialog, opened fresh each time. */
export function PersonalChatData(props: PersonalDataProps): ReactElement | null {
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  const sessions = useRef(0);
  const [session, setSession] = useState<number>();
  if (!props.api.data) return null;
  const zh = props.locale.toLowerCase().startsWith('zh');
  const title = zh ? '个人数据' : 'Personal data';

  function show(): void {
    if (!dialog.current || dialog.current.open) return;
    setSession(++sessions.current);
    dialog.current.showModal();
  }

  return <>
    <button type="button" className="tw-head-action no-drag" onClick={show}>{title}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record no-drag" aria-labelledby={heading}
      onClose={event => { if (event.currentTarget === event.target) setSession(undefined); }}>
      <div className="tw-dialog-panel tw-data-workspace">
        <h2 id={heading}>{title}</h2>
        {session !== undefined && <PersonalDataPanel key={session} {...props} />}
        <form method="dialog"><button type="submit">{zh ? '关闭' : 'Close'}</button></form>
      </div>
    </dialog>
  </>;
}

/**
 * The personal datasets of the signed-in person: pick one, edit its records, views and templates, and
 * publish it to a Team conversation. It loads when it appears; work still running when it goes away is dropped.
 */
export function PersonalDataPanel(props: PersonalDataProps): ReactElement | null {
  const data = props.api.data;
  return data ? <PersonalDataBody {...props} data={data} /> : null;
}

function PersonalDataBody({ api, data: requestData, uid, teams, locale, chatReply }: PersonalDataProps & {
  data: NonNullable<TeamsApi['data']>;
}): ReactElement {
  const life = useRef(0);
  const [datasets, setDatasets] = useState<VisibleDataSet[]>([]);
  const [active, setActive] = useState<{ dataset: string; description: DataDescription; page: DataRecordPage; templates: DataTemplateView[] }>();
  const [chosen, setChosen] = useState<DataRef[]>([]);
  const [team, setTeam] = useState('');
  const [room, setRoom] = useState('');
  const [rooms, setRooms] = useState<TeamConversation[]>([]);
  const [confirm, setConfirm] = useState<Confirmation>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [schemaDraft, setSchemaDraft] = useState('');
  const zh = locale.toLowerCase().startsWith('zh');
  const eligible = teams.filter(entry => entry.role === 'admin' || entry.role === 'owner');
  const target = eligible.find(entry => entry.id === team);
  const generation = target?.generation;
  const chatSchema = chatReply ? schemaFromReply(chatReply) : undefined;

  useEffect(() => {
    if (!team || generation === undefined) return;
    return api.watchMessages({ teamId: team, generation }, '', snapshot => {
      if (snapshot.kind === 'directory' && !snapshot.fromCache) setRooms(snapshot.conversations.filter(room => room.participants.includes('user:' + uid)));
    });
  }, [api, team, generation, uid]);

  useEffect(() => {
    life.current++;
    void perform(refresh);
    return () => { life.current++; };
  }, []);

  async function perform(work: (current: () => boolean) => Promise<void>): Promise<void> {
    const mark = life.current;
    const current = () => life.current === mark;
    setBusy(true); setMessage('');
    try { await work(current); }
    catch { if (current()) setMessage(zh ? '操作未完成。请检查输入、当前权限和数据版本。' : 'Could not finish. Check the input, access and current revisions.'); }
    finally { if (current()) setBusy(false); }
  }

  async function refresh(current: () => boolean): Promise<void> {
    const result = await requestData<{ datasets: VisibleDataSet[] }>({ scope: personal, command: { action: 'datasets' } });
    if (current()) setDatasets(result.datasets);
  }

  async function load(dataset: string, current: () => boolean, after?: string): Promise<void> {
    const [description, page, layouts] = await Promise.all([
      requestData<DataDescription>({ scope: personal, command: { action: 'describe', dataset } }),
      requestData<DataRecordPage>({ scope: personal, command: { action: 'list', dataset, ...(after ? { after } : {}) } }),
      requestData<{ views: DataTemplateView[] }>({ scope: personal, command: { action: 'templateViews', dataset } }),
    ]);
    if (current()) setActive({ dataset, description, page, templates: layouts.views });
  }

  function newDataset(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    void perform(async current => {
      const created = await requestData<{ dataset: string }>({ scope: personal, operation: crypto.randomUUID(),
        command: { action: 'create', schema: jsonObject(form.get('schema')) },
      });
      await refresh(current);
      await load(created.dataset, current);
    });
  }

  function newRecord(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    if (!active) return;
    const { dataset, description: { schemaVersion } } = active;
    void perform(async current => {
      await requestData<DataRef>({ scope: personal, operation: crypto.randomUUID(), command: { action: 'put',
        dataset, record: String(form.get('record')), expectedRevision: 0, schemaVersion, value: jsonObject(form.get('value')) },
      });
      if (current()) { setConfirm(undefined); setChosen([]); }
      await load(dataset, current);
    });
  }

  function preview(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!active || !target || !rooms.some(entry => entry.id === room)) return;
    const scope: TeamDataScope = { kind: 'team', teamId: target.id, conversationId: room, generation: target.generation };
    void perform(async current => {
      const [audience, detail] = await Promise.all([
        requestData<DataAudience>({ scope, command: { action: 'previewAudience', editors: [] } }),
        api.detail(scope.teamId),
      ]);
      if (current()) setConfirm({ target: scope, audience, records: chosen, operation: crypto.randomUUID(),
        dataset: active.dataset, schemaVersion: active.description.schemaVersion, sourceEpoch: active.description.epoch,
        templates: active.templates.filter(entry => entry.schemaVersion === active.description.schemaVersion)
          .map(({ view, version }) => ({ view, version })),
        labels: new Map(detail.members.map(member => [member.uid, member.displayName])) });
    });
  }

  function publish(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!confirm) return;
    const { target, audience, records, operation, dataset, schemaVersion, sourceEpoch, templates } = confirm;
    void perform(async current => {
      await requestData({ scope: target, operation, command: { action: 'publishPersonal', dataset,
        schemaVersion, sourceEpoch, audienceRevision: audience.audienceRevision, editors: [], records, templates },
      });
      if (current()) {
        setMessage(zh ? '已发布到团队会话。' : 'Published to the Team conversation.');
        setConfirm(undefined);
      }
    });
  }

  return <>
    {message && <p role="status">{message}</p>}
    <label>{zh ? '数据集' : 'Dataset'}<select name="dataset" value={active?.dataset ?? ''} disabled={busy}
      onChange={event => { const dataset = event.currentTarget.value; setChosen([]); setConfirm(undefined);
        if (dataset) void perform(current => load(dataset, current)); else setActive(undefined); }}>
      <option value="">{zh ? '选择数据集' : 'Select a dataset'}</option>
      {datasets.map(({ dataset }) => <option key={dataset} value={dataset}>{dataset}</option>)}
    </select></label>
    <details><summary>{zh ? '用 JSON Schema 新建数据集' : 'Create a dataset from JSON Schema'}</summary>
      <form method="post" onSubmit={newDataset}>
        {chatSchema && <button type="button" disabled={busy} onClick={() => setSchemaDraft(chatSchema)}>
          {zh ? '使用最近回复中的 Schema' : 'Use schema from the latest reply'}
        </button>}
        <label>JSON Schema<textarea name="schema" required rows={4} spellCheck={false}
          value={schemaDraft} onChange={event => setSchemaDraft(event.currentTarget.value)} /></label>
        <p>{zh ? '对象字段需声明 additionalProperties 为 false。' : 'Object fields must declare additionalProperties as false.'}</p>
        <button type="submit" disabled={busy}>{zh ? '新建数据集' : 'Create dataset'}</button>
      </form>
    </details>
    {active && <>
      <DataTable rows={active.page.records} title={active.dataset} actionLabel={zh ? '选择与编辑' : 'Select and edit'}
        fields={Object.keys(active.description.schema.properties ?? {})} actions={({ id, record }) => {
          const reference = parseDataRef({ dataset: active.dataset, record: id, revision: record.revision, schemaVersion: record.schemaVersion });
          if (!reference) return null;
          return <>
            <label><input type="checkbox" name="publication-record" value={id} disabled={busy}
              checked={chosen.some(entry => entry.record === id)} onChange={event => {
                const checked = event.currentTarget.checked;
                setChosen(current => checked ? [...current.filter(entry => entry.record !== id), reference] : current.filter(entry => entry.record !== id));
                setConfirm(undefined);
              }}/>{zh ? '选择发布' : 'Select to publish'}</label>
            <DataRecordButton api={api} scope={personal} reference={reference} locale={locale} onSaved={() => {
              setChosen([]); setConfirm(undefined); void perform(current => load(active.dataset, current));
            }}/>
          </>;
        }} />
      {active.page.cursor && <button type="button" disabled={busy} onClick={() => void perform(current => load(active.dataset, current, active.page.cursor!))}>
        {zh ? '下一页' : 'Next page'}</button>}
      <DataViewSettings api={api} scope={personal} dataset={active.dataset} description={active.description} locale={locale}
        saved={() => { setConfirm(undefined); void perform(current => load(active.dataset, current)); }} />
      <DataTemplateSettings key={active.dataset} api={api} scope={personal} dataset={active.dataset} description={active.description}
        views={active.templates} locale={locale} saved={() => { setConfirm(undefined); void perform(current => load(active.dataset, current)); }} />
      {active.templates.filter(entry => entry.schemaVersion === active.description.schemaVersion).map(entry => {
        const record = active.page.records[0]?.record;
        return record && <details key={entry.view}>
          <summary>{entry.view}</summary>
          <DataTemplate spec={entry.definition} value={record.value} />
        </details>;
      })}
      {target && room && <ReportPublisher api={api} locale={locale}
        scope={{ kind: 'team', teamId: team, generation: target.generation, conversationId: room }}
        origin={{ kind: 'personal', dataset: active.dataset, epoch: active.description.epoch }}
        fields={Object.keys(active.description.schema.properties ?? {})} />}
      <details><summary>{zh ? '添加记录' : 'Add record'}</summary>
        <form method="post" onSubmit={newRecord}>
          <label>{zh ? '记录 ID' : 'Record ID'}<input name="record" required pattern="[A-Za-z0-9_\-]{1,64}" maxLength={64} /></label>
          <label>{zh ? '记录 JSON' : 'Record JSON'}<textarea name="value" required rows={4} spellCheck={false} /></label>
          <button type="submit" disabled={busy}>{zh ? '保存记录' : 'Save record'}</button>
        </form>
      </details>
      <form method="post" onSubmit={preview}>
        <label>{zh ? '发布到团队' : 'Publish to Team'}<select name="team" value={team} required disabled={busy} onChange={event => {
          setTeam(event.currentTarget.value); setRoom(''); setRooms([]); setConfirm(undefined);
        }}>
          <option value="">{zh ? '选择团队' : 'Select a Team'}</option>
          {eligible.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}
        </select></label>
        <label>{zh ? '目标会话' : 'Destination conversation'}<select name="room" value={room} required disabled={busy} onChange={event => {
          setRoom(event.currentTarget.value); setConfirm(undefined);
        }}>
          <option value="">{zh ? '选择会话' : 'Select a conversation'}</option>
          {rooms.map(value => <option key={value.id} value={value.id}>{value.title}</option>)}
        </select></label>
        <button type="submit" disabled={busy}>{zh ? '预览发布受众' : 'Preview publication audience'}</button>
      </form>
      {confirm && <form method="post" onSubmit={publish}>
        <p>{zh ? `发布当前 Schema、视图和 ${confirm.records.length} 条选定记录` : `Publish the current schema, views and ${confirm.records.length} selected records`}</p>
        {confirm.templates.length > 0 && <p>{zh ? '自定义视图 ' : 'Custom views '}{confirm.templates.map(entry => entry.view).join(', ')}</p>}
        <ul>{confirm.audience.audience.map(member => <li key={member.uid}>
          {confirm.labels.get(member.uid) ?? member.uid} · {member.access === 'manage' ? (zh ? '管理' : 'Manage') : (zh ? '读取' : 'Read')}
        </li>)}</ul>
        <button type="submit" disabled={busy}>{zh ? '确认发布' : 'Confirm publication'}</button>
      </form>}
    </>}
  </>;
}

function schemaFromReply(reply: string): string | undefined {
  if (reply.length > 32000) return undefined;
  const blocks = [...reply.matchAll(/^```(?:json|json-schema)[ \t]*\r?\n([\s\S]*?)^\s*```[ \t]*$/gimu)];
  if (blocks.length !== 1) return undefined;
  try {
    const value = jsonObject(blocks[0]?.[1] ?? null);
    if (value.type === 'object' && value.properties && typeof value.properties === 'object' && !Array.isArray(value.properties)) {
      return JSON.stringify(value, null, 2);
    }
  } catch { /* Ordinary model replies need no import action. */ }
  return undefined;
}
