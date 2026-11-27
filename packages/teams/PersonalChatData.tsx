import { useEffect, useId, useRef, useState, type FormEvent, type ReactElement, type ReactNode } from 'react';
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

export type PersonalDatasetCatalog = {
  datasets: readonly string[];
  selected: string;
  status: 'loading' | 'ready' | 'error';
  busy: boolean;
  onSelect(dataset: string): void;
  onRetry(): void;
};

type PersonalDataPage = {
  heading: ReactNode;
  icon: ReactNode;
  renderCatalog?(catalog: PersonalDatasetCatalog): ReactNode;
  labels: Record<'newDataset' | 'create' | 'cancel' | 'loading' | 'loadFailed' | 'opening' | 'openFailed' | 'retry'
    | 'empty' | 'emptyHint' | 'dataset' | 'choose' | 'records' | 'noRecords' | 'schemaHelp' | 'schemaInvalid'
    | 'createFailed' | 'addRecord' | 'publish', string>;
};
type PersonalDataProps = { api: TeamsApi; uid: string; teams: TeamSummary[]; locale: string; chatReply?: string };
type PersonalDataPanelProps = PersonalDataProps & { page?: PersonalDataPage };

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
export function PersonalDataPanel(props: PersonalDataPanelProps): ReactElement | null {
  const data = props.api.data;
  return data ? <PersonalDataBody {...props} data={data} /> : null;
}

function PersonalDataBody({ api, data: requestData, uid, teams, locale, chatReply, page }: PersonalDataPanelProps & {
  data: NonNullable<TeamsApi['data']>;
}): ReactElement {
  const life = useRef(0);
  const selection = useRef('');
  const detailRequest = useRef(0);
  const createDialog = useRef<HTMLDialogElement>(null);
  const createAttempt = useRef<{ draft: string; operation: string }>();
  const addRecordDisclosure = useRef<HTMLDetailsElement>(null);
  const createHeading = useId();
  const schemaInput = useId();
  const [datasets, setDatasets] = useState<string[]>([]);
  const [catalog, setCatalog] = useState<PersonalDatasetCatalog['status']>('loading');
  const [selected, setSelected] = useState('');
  const [openFailed, setOpenFailed] = useState(false);
  const [active, setActive] = useState<{ dataset: string; description: DataDescription; page: DataRecordPage; templates: DataTemplateView[] }>();
  const [chosen, setChosen] = useState<DataRef[]>([]);
  const [team, setTeam] = useState('');
  const [room, setRoom] = useState('');
  const [rooms, setRooms] = useState<TeamConversation[]>([]);
  const [confirm, setConfirm] = useState<Confirmation>();
  const [pending, setPending] = useState(0);
  const [feedback, setFeedback] = useState<{ role: 'alert' | 'status'; text: string }>();
  const [schemaDraft, setSchemaDraft] = useState('');
  const [createError, setCreateError] = useState<'json' | 'request'>();
  const busy = pending > 0;
  const labels = page?.labels;
  const renderCatalog = page?.renderCatalog;
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
    void readCatalog();
    return () => { life.current++; };
  }, []);

  async function perform(work: (current: () => boolean) => Promise<void>, failed?: () => void): Promise<void> {
    const mark = life.current;
    const current = () => life.current === mark;
    setPending(count => count + 1);
    setFeedback(undefined);
    try { await work(current); }
    catch {
      if (current()) {
        if (failed) failed();
        else {
          setFeedback({ role: 'alert', text: zh ? '操作未完成。请检查输入、当前权限和数据版本。' : 'Could not finish. Check the input, access and current revisions.' });
        }
      }
    }
    finally { if (current()) setPending(count => count - 1); }
  }

  async function readCatalog(): Promise<void> {
    const mark = life.current;
    setCatalog('loading');
    try {
      const result = await requestData<{ datasets: VisibleDataSet[] }>({ scope: personal, command: { action: 'datasets' } });
      if (life.current !== mark) return;
      const next = result.datasets.map(entry => entry.dataset);
      setDatasets(next);
      setCatalog('ready');
      if (page && !next.includes(selection.current)) selectDataset(next[0] ?? '');
    } catch {
      if (life.current === mark) setCatalog('error');
    }
  }

  async function load(dataset: string, current: () => boolean, after?: string): Promise<void> {
    if (selection.current !== dataset) return;
    const request = ++detailRequest.current;
    const [description, page, layouts] = await Promise.all([
      requestData<DataDescription>({ scope: personal, command: { action: 'describe', dataset } }),
      requestData<DataRecordPage>({ scope: personal, command: { action: 'list', dataset, ...(after ? { after } : {}) } }),
      requestData<{ views: DataTemplateView[] }>({ scope: personal, command: { action: 'templateViews', dataset } }),
    ]);
    if (current() && request === detailRequest.current && selection.current === dataset) {
      setActive({ dataset, description, page, templates: layouts.views });
    }
  }

  function selectDataset(dataset: string): void {
    selection.current = dataset;
    setSelected(dataset);
    setActive(undefined);
    setChosen([]);
    setConfirm(undefined);
    setOpenFailed(false);
    setFeedback(undefined);
    if (dataset) void perform(current => load(dataset, current), () => setOpenFailed(true));
  }

  function newDataset(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    let schema: Record<string, unknown>;
    try { schema = jsonObject(schemaDraft); }
    catch { setCreateError('json'); return; }
    setCreateError(undefined);
    const attempt = createAttempt.current?.draft === schemaDraft
      ? createAttempt.current : { draft: schemaDraft, operation: crypto.randomUUID() };
    createAttempt.current = attempt;
    void perform(async current => {
      const created = await requestData<{ dataset: string }>({ scope: personal, operation: attempt.operation,
        command: { action: 'create', schema },
      });
      if (!current()) return;
      createAttempt.current = undefined;
      setDatasets(previous => previous.includes(created.dataset) ? previous : [...previous, created.dataset]);
      createDialog.current?.close();
      if (page) setSchemaDraft('');
      selectDataset(created.dataset);
    }, () => setCreateError('request'));
  }

  function newRecord(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const formElement = event.currentTarget;
    const form = new FormData(formElement);
    if (!active) return;
    const { dataset, description: { schemaVersion } } = active;
    void perform(async current => {
      await requestData<DataRef>({ scope: personal, operation: crypto.randomUUID(), command: { action: 'put',
        dataset, record: String(form.get('record')), expectedRevision: 0, schemaVersion, value: jsonObject(form.get('value')) },
      });
      if (current()) {
        setConfirm(undefined);
        setChosen([]);
        const disclosure = addRecordDisclosure.current;
        if (page && disclosure) {
          const restoreFocus = formElement.contains(document.activeElement);
          formElement.reset();
          disclosure.open = false;
          if (restoreFocus) disclosure.querySelector('summary')?.focus();
        }
      }
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
        setFeedback({ role: 'status', text: zh ? '已发布到团队会话。' : 'Published to the Team conversation.' });
        setConfirm(undefined);
      }
    });
  }

  const createForm = <form method="post" onSubmit={newDataset}>
    {chatSchema && <button type="button" disabled={busy} onClick={() => setSchemaDraft(chatSchema)}>
      {zh ? '使用最近回复中的 Schema' : 'Use schema from the latest reply'}
    </button>}
    <label htmlFor={schemaInput}>JSON Schema</label>
    <p id={`${schemaInput}-help`}>{labels?.schemaHelp ?? (zh ? '对象字段需声明 additionalProperties 为 false。' : 'Object fields must declare additionalProperties as false.')}</p>
    <textarea id={schemaInput} name="schema" required rows={page ? 9 : 4} spellCheck={false}
      aria-describedby={`${schemaInput}-help${createError ? ` ${schemaInput}-error` : ''}`}
      aria-invalid={createError === 'json' || undefined} readOnly={busy}
      value={schemaDraft} onChange={event => { setSchemaDraft(event.currentTarget.value); setCreateError(undefined); }} />
    {createError && <p id={`${schemaInput}-error`} role="alert">{createError === 'json'
      ? labels?.schemaInvalid ?? (zh ? '请输入有效的 JSON 对象。' : 'Enter a valid JSON object.')
      : labels?.createFailed ?? (zh ? '无法新建数据集。请检查 Schema 后重试。' : 'Could not create the dataset. Check the schema and try again.')}</p>}
    <div className="personal-dataset-form-actions">
      {labels && <button type="button" disabled={busy} onClick={() => createDialog.current?.close()}>{labels.cancel}</button>}
      <button type="submit" className={page ? 'personal-dataset-primary' : undefined} disabled={busy || catalog === 'loading'}>
        {labels?.create ?? (zh ? '新建数据集' : 'Create dataset')}
      </button>
    </div>
  </form>;

  const picker = <label className="personal-dataset-picker">{labels?.dataset ?? (zh ? '数据集' : 'Dataset')}
    <select name="dataset" value={selected} disabled={busy || catalog === 'loading'}
      onChange={event => selectDataset(event.currentTarget.value)}>
      <option value="" disabled={Boolean(page)}>{labels?.choose ?? (zh ? '选择数据集' : 'Select a dataset')}</option>
      {datasets.map(dataset => <option key={dataset} value={dataset}>{dataset}</option>)}
    </select>
  </label>;

  const retry = labels?.retry ?? (zh ? '重试' : 'Retry');
  const records = active && <>
    {page && !active.page.records.length ? <p className="personal-dataset-records-empty">{page.labels.noRecords}</p>
      : <DataTable rows={active.page.records} title={active.dataset} actionLabel={zh ? '选择与编辑' : 'Select and edit'}
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
      }} />}
    {active.page.cursor && <button type="button" disabled={busy} onClick={() => void perform(current => load(active.dataset, current, active.page.cursor!))}>
      {zh ? '下一页' : 'Next page'}</button>}
  </>;
  const views = active && <>
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
  </>;
  const report = active && target && room && <ReportPublisher api={api} locale={locale}
    scope={{ kind: 'team', teamId: team, generation: target.generation, conversationId: room }}
    origin={{ kind: 'personal', dataset: active.dataset, epoch: active.description.epoch }}
    fields={Object.keys(active.description.schema.properties ?? {})} />;
  const addRecord = <details className="personal-dataset-add-record" ref={addRecordDisclosure}><summary>{labels?.addRecord ?? (zh ? '添加记录' : 'Add record')}</summary>
    <form method="post" onSubmit={newRecord}>
      <label>{zh ? '记录 ID' : 'Record ID'}<input name="record" required pattern="[A-Za-z0-9_\-]{1,64}" maxLength={64} /></label>
      <label>{zh ? '记录 JSON' : 'Record JSON'}<textarea name="value" required rows={4} spellCheck={false} /></label>
      <button type="submit" disabled={busy}>{zh ? '保存记录' : 'Save record'}</button>
    </form>
  </details>;
  const publication = <>
    <form method="post" onSubmit={preview}>
      <label>{page ? (zh ? '团队' : 'Team') : (zh ? '发布到团队' : 'Publish to Team')}<select name="team" value={team} required disabled={busy} onChange={event => {
        setTeam(event.currentTarget.value); setRoom(''); setRooms([]); setConfirm(undefined);
      }}>
        <option value="">{zh ? '选择团队' : 'Select a Team'}</option>
        {eligible.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}
      </select></label>
      <label>{zh ? '目标会话' : 'Destination conversation'}<select name="room" value={room} required disabled={busy || !team} onChange={event => {
        setRoom(event.currentTarget.value); setConfirm(undefined);
      }}>
        <option value="">{zh ? '选择会话' : 'Select a conversation'}</option>
        {rooms.map(value => <option key={value.id} value={value.id}>{value.title}</option>)}
      </select></label>
      <button type="submit" disabled={busy || !target || !room}>{zh ? '预览发布受众' : 'Preview publication audience'}</button>
    </form>
    {confirm && <form method="post" onSubmit={publish}>
      <p>{zh ? `发布当前 Schema、视图和 ${confirm.records.length} 条选定记录` : `Publish the current schema, views and ${confirm.records.length} selected records`}</p>
      {confirm.templates.length > 0 && <p>{zh ? '自定义视图 ' : 'Custom views '}{confirm.templates.map(entry => entry.view).join(', ')}</p>}
      <ul>{confirm.audience.audience.map(member => <li key={member.uid}>
        {confirm.labels.get(member.uid) ?? member.uid} · {member.access === 'manage' ? (zh ? '管理' : 'Manage') : (zh ? '读取' : 'Read')}
      </li>)}</ul>
      <button type="submit" disabled={busy}>{zh ? '确认发布' : 'Confirm publication'}</button>
    </form>}
  </>;

  return <>
    {page && <>
      <header className="personal-dataset-header">
        {page.heading}
        <button type="button" className="personal-dataset-primary" disabled={catalog === 'loading'}
          onClick={() => createDialog.current?.showModal()}>
          <span aria-hidden="true">+</span>{page.labels.newDataset}
        </button>
      </header>
      <dialog ref={createDialog} className="personal-dataset-create no-drag" aria-labelledby={createHeading}
        onCancel={event => { if (busy) event.preventDefault(); }}>
        <h2 id={createHeading}>{page.labels.newDataset}</h2>
        {createForm}
      </dialog>
    </>}
    {renderCatalog ? renderCatalog({ datasets, selected, status: catalog, busy,
      onSelect: selectDataset, onRetry: () => void readCatalog() }) : <>
      {catalog === 'loading' && <p className="personal-dataset-progress" role="status">{labels?.loading ?? (zh ? '正在载入数据集…' : 'Loading datasets…')}</p>}
      {catalog === 'error' && <div className="personal-dataset-notice">
        <p role="alert">{labels?.loadFailed ?? (zh ? '无法载入数据集。' : 'Could not load datasets.')}</p>
        <button type="button" disabled={busy} onClick={() => void readCatalog()}>{retry}</button>
      </div>}
      {(!page || datasets.length > 0) && picker}
    </>}
    {!page && <details><summary>{zh ? '用 JSON Schema 新建数据集' : 'Create a dataset from JSON Schema'}</summary>{createForm}</details>}
    {page && catalog === 'ready' && !datasets.length && <div className="personal-dataset-empty">
      <span className="personal-dataset-empty-icon">{page.icon}</span>
      <h2>{page.labels.empty}</h2><p>{page.labels.emptyHint}</p>
    </div>}
    {selected && !active && !openFailed && <p className="personal-dataset-progress" role="status">{labels?.opening ?? (zh ? '正在打开数据集…' : 'Opening dataset…')}</p>}
    {openFailed && <div className="personal-dataset-notice">
      <p role="alert">{labels?.openFailed ?? (zh ? '无法打开此数据集。' : 'Could not open this dataset.')}</p>
      <button type="button" disabled={busy} onClick={() => selectDataset(selected)}>{retry}</button>
    </div>}
    {feedback && <p className="personal-dataset-action-message" role={feedback.role}>{feedback.text}</p>}
    {active && (page ? <div className="personal-dataset-detail" key={active.dataset}>
      <section className="personal-dataset-records" aria-label={page.labels.records}>
        <h2>{renderCatalog ? active.dataset : page.labels.records}</h2>
        {records}
        {addRecord}
      </section>
      <div className="personal-dataset-settings">
        {views}
        <details><summary>{page.labels.publish}</summary>{publication}{report}</details>
      </div>
    </div> : <>{records}{views}{report}{addRecord}{publication}</>)}
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
